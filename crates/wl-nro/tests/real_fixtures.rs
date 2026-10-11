use std::path::{Path, PathBuf};
use std::process::Command;
use wl_common::{GuestAddr, ModuleOffset};
use wl_nro::{MAGIC_ASET, MAGIC_MOD0, MAGIC_NRO, Nro};

fn devkitpro_dir() -> Option<PathBuf> {
    let repo_root = Path::new(env!("CARGO_MANIFEST_DIR")).parent()?.parent()?;
    let dkp = repo_root.join("target/toolchains/devkitpro/root/opt/devkitpro");
    if dkp.join("devkitA64/bin/aarch64-none-elf-gcc").is_file()
        && dkp.join("tools/bin/elf2nro").is_file()
    {
        Some(dkp)
    } else {
        None
    }
}

static TEST_COUNTER: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);

fn build_real_nro(dkp: &Path, c_code: &str, with_nacp: bool) -> Vec<u8> {
    let id = TEST_COUNTER.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    let tmp = std::env::temp_dir().join(format!(
        "wl_nro_test_{}_{}_{}",
        std::process::id(),
        id,
        nanos
    ));
    let _ = std::fs::create_dir_all(&tmp);

    let src = tmp.join("app.c");
    let elf = tmp.join("app.elf");
    let nacp = tmp.join("app.nacp");
    let nro = tmp.join("app.nro");

    std::fs::write(&src, c_code).expect("write c source");

    let gcc = dkp.join("devkitA64/bin/aarch64-none-elf-gcc");
    let elf2nro = dkp.join("tools/bin/elf2nro");
    let nacptool = dkp.join("tools/bin/nacptool");
    let specs = dkp.join("libnx/switch.specs");

    let mut gcc_cmd = Command::new(&gcc);
    gcc_cmd
        .arg("-march=armv8-a+crc+crypto")
        .arg("-mtune=cortex-a57")
        .arg("-mtp=soft")
        .arg("-fPIE")
        .arg("-O2")
        .arg("-Wall")
        .arg("-ffunction-sections")
        .arg("-D__SWITCH__")
        .arg("-I")
        .arg(dkp.join("libnx/include"))
        .arg(&src)
        .arg(format!("-specs={}", specs.display()))
        .arg("-L")
        .arg(dkp.join("libnx/lib"))
        .arg("-lnx")
        .arg("-o")
        .arg(&elf)
        .env("DEVKITPRO", dkp);

    let gcc_status = gcc_cmd.status().expect("run gcc");
    assert!(gcc_status.success(), "gcc compilation failed");

    let mut elf2nro_cmd = Command::new(&elf2nro);
    elf2nro_cmd.arg(&elf).arg(&nro).env("DEVKITPRO", dkp);

    if with_nacp {
        let mut nacp_cmd = Command::new(&nacptool);
        nacp_cmd
            .arg("--create")
            .arg("RealTestApp")
            .arg("WanderlustTeam")
            .arg("1.0.0")
            .arg(&nacp)
            .env("DEVKITPRO", dkp);
        let nacp_status = nacp_cmd.status().expect("run nacptool");
        assert!(nacp_status.success(), "nacptool failed");

        elf2nro_cmd.arg(format!("--nacp={}", nacp.display()));
    }

    let elf2nro_status = elf2nro_cmd.status().expect("run elf2nro");
    assert!(elf2nro_status.success(), "elf2nro conversion failed");

    let bytes = std::fs::read(&nro).expect("read produced nro");

    // Clean up temporary files
    let _ = std::fs::remove_dir_all(&tmp);

    bytes
}

const MINIMAL_C_SOURCE: &str = r#"
#include <switch.h>
#include <string.h>

void __appInit(void) {}
void __appExit(void) {}

static const char greeting[] = "Wanderlust Genuine AArch64 Guest NRO";
static unsigned counter = 100;
// BSS spanning page boundaries: 8192 bytes = 2 full 4K pages
static volatile unsigned bss_array[2048];

static unsigned calc_hash(const char* s) {
    unsigned h = 5381;
    while (*s) {
        h = ((h << 5) + h) + (unsigned char)(*s++);
    }
    return h;
}

int main(int argc, char** argv) {
    (void)argc;
    (void)argv;
    counter += calc_hash(greeting);
    for (int i = 0; i < 2048; i++) {
        bss_array[i] = counter ^ (unsigned)i;
    }
    svcOutputDebugString(greeting, strlen(greeting));
    return (int)bss_array[2047];
}
"#;

#[test]
fn test_real_independently_compiled_nro_plain() {
    let Some(dkp) = devkitpro_dir() else {
        eprintln!("devkitpro toolchain not found; skipping dynamic compilation test");
        return;
    };

    let nro_bytes = build_real_nro(&dkp, MINIMAL_C_SOURCE, false);
    let nro = Nro::parse(&nro_bytes).expect("real plain NRO must parse cleanly");

    assert_eq!(nro.header.magic, MAGIC_NRO);
    assert_eq!(nro.header.version, 0);
    assert_eq!(nro.header.size as usize, nro_bytes.len());
    assert!(nro.assets.is_none());

    // Verified nonzero BSS spanning page boundaries
    assert!(
        nro.header.bss_size > 0,
        "BSS size must be demonstrably nonzero"
    );
    assert_eq!(
        nro.header.bss_size, 0x2000,
        "BSS must span multiple 4K pages (8192 bytes)"
    );
    assert_eq!(
        nro.header.bss_size % 0x1000,
        0,
        "BSS size must be page aligned"
    );

    // Branch instruction at offset 0
    let branch = nro.start.branch_instruction;
    assert_eq!(branch, 0x14000020, "first word must be 'b +0x80'");

    // MOD0 dynamic header pointer
    assert_ne!(nro.start.mod0_offset.0, 0);
    let mod0_off = nro.start.mod0_offset.0 as usize;
    assert_eq!(&nro_bytes[mod0_off..mod0_off + 4], &MAGIC_MOD0);

    // RoData slice contains the greeting string
    let ro_slice = nro.ro_slice();
    assert!(
        ro_slice
            .windows("Wanderlust Genuine AArch64 Guest NRO".len())
            .any(|w| w == b"Wanderlust Genuine AArch64 Guest NRO"),
        "rodata slice must contain the compiled C constant string"
    );

    // Image conversion and guest memory mapping
    let image = nro.to_image().expect("image generation");
    assert_eq!(image.entrypoint_offset, ModuleOffset(0));
    assert_eq!(image.bss_size, 0x2000);
    assert_eq!(
        image.bss_offset.0,
        (nro.header.data.file_offset.0 + nro.header.data.size) as u64
    );
    assert!(image.total_memory_size >= nro.header.size as u64 + 0x2000);

    let mappings = image.create_guest_mappings(GuestAddr(0x800_0000));
    assert_eq!(mappings.len(), 3);
    assert_eq!(mappings[0].base, GuestAddr(0x800_0000));
    assert!(mappings[0].execute);
    assert!(!mappings[0].write);

    assert_eq!(mappings[1].base, GuestAddr(0x800_2000));
    assert!(mappings[1].read);
    assert!(!mappings[1].write);
    assert!(!mappings[1].execute);

    assert_eq!(mappings[2].base, GuestAddr(0x800_3000));
    assert!(mappings[2].read);
    assert!(mappings[2].write);
    assert!(!mappings[2].execute);

    // Verify BSS zero-initialization semantics in mapped guest data segment
    let data_file_sz = nro.header.data.size as usize;
    assert!(
        mappings[2].bytes.len() >= data_file_sz + 0x2000,
        "data mapping must contain data bytes plus BSS"
    );
    let bss_bytes = &mappings[2].bytes[data_file_sz..data_file_sz + 0x2000];
    assert!(
        bss_bytes.iter().all(|&b| b == 0),
        "mapped BSS region must be entirely zero-initialized"
    );
    // Nonzero data bytes immediately before the BSS boundary
    assert!(
        mappings[2].bytes[..data_file_sz].iter().any(|&b| b != 0),
        "data section before BSS must contain nonzero initialized data"
    );

    // Execution plan generation
    let plan = nro.plan().expect("plan generation");
    assert_eq!(plan.entry_offset, 0);
    assert_eq!(plan.page_alignment, 0x1000);
    assert_eq!(plan.zero_fill_ranges.len(), 1);
    assert_eq!(plan.zero_fill_ranges[0].name, "bss");
    assert_eq!(plan.zero_fill_ranges[0].size, 0x2000);
    assert_eq!(plan.zero_fill_ranges[0].memory_offset, image.bss_offset);

    assert!(matches!(
        plan.relocation,
        wl_nro::RelocationRequirement::GuestSelfRelocating { .. }
    ));
    assert!(plan.startup.requires_loader_config);
    assert_eq!(plan.startup.x1_sentinel, 0xFFFF_FFFF_FFFF_FFFF);
    assert!(plan.startup.returnable_lr);
    assert_eq!(plan.startup.recommended_stack_size, 0x10_0000);
    assert!(plan.unsupported_features.is_empty());
}

#[test]
fn test_real_independently_compiled_nro_with_aset() {
    let Some(dkp) = devkitpro_dir() else {
        eprintln!("devkitpro toolchain not found; skipping dynamic compilation test");
        return;
    };

    let nro_bytes = build_real_nro(&dkp, MINIMAL_C_SOURCE, true);
    let nro = Nro::parse(&nro_bytes).expect("real NRO with ASET must parse cleanly");

    assert_eq!(nro.header.magic, MAGIC_NRO);
    assert_eq!(nro.header.version, 0);
    assert!(nro_bytes.len() > nro.header.size as usize);

    let assets = nro.assets().expect("ASET section must be present");
    assert_eq!(assets.header.magic, MAGIC_ASET);
    assert_eq!(assets.header.nacp.size, 16384); // Standard 16 KiB NACP

    let nacp_bytes = assets.nacp_bytes.expect("nacp bytes must be present");
    assert_eq!(nacp_bytes.len(), 16384);
    assert!(
        nacp_bytes
            .windows(b"RealTestApp".len())
            .any(|w| w == b"RealTestApp"),
        "NACP must contain declared application title"
    );

    let report = nro.inspect();
    assert!(report.has_aset);
    assert_eq!(report.nacp_size, 16384);
}

#[test]
fn test_authoritative_probe_nro_fixture() {
    use sha2::{Digest, Sha256};

    let manifest_dir = Path::new(env!("CARGO_MANIFEST_DIR"));
    let fixture_path = manifest_dir.join("tests/fixtures/probe.nro");
    let nro_bytes = std::fs::read(&fixture_path).expect("authoritative probe.nro must exist");

    assert_eq!(nro_bytes.len(), 94208);
    let digest = Sha256::digest(&nro_bytes);
    let hex = digest
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect::<String>();
    assert_eq!(
        hex,
        "d205f4e9279880373f3df5beaa76fe7d07a68fb2a51ef1864706c6a6f6b5618f"
    );

    let nro = Nro::parse(&nro_bytes).expect("authoritative probe.nro must parse cleanly");
    assert_eq!(nro.header.magic, MAGIC_NRO);
    assert_eq!(nro.header.version, 0);
    assert_eq!(nro.header.size, 94208);
    assert_eq!(nro.header.text.file_offset.0, 0);
    assert_eq!(nro.header.text.size, 0x10000);
    assert_eq!(nro.header.ro.file_offset.0, 0x10000);
    assert_eq!(nro.header.ro.size, 0x3000);
    assert_eq!(nro.header.data.file_offset.0, 0x13000);
    assert_eq!(nro.header.data.size, 0x4000);
    assert_eq!(nro.header.bss_size, 0x1000);

    let report = nro.inspect();
    assert_eq!(report.nro_size, 94208);
    assert_eq!(report.file_size, 94208);
    assert_eq!(report.total_memory_size, 0x18000);
    assert!(report.has_mod0);
    assert_eq!(report.mod0_offset, 0xF0);
    assert!(!report.has_aset);

    let plan = nro.plan().expect("plan generation");
    assert_eq!(plan.entry_offset, 0);
    assert_eq!(plan.page_alignment, 0x1000);
    assert_eq!(plan.image_end, 0x18000);
    assert_eq!(plan.mappable_ranges.len(), 3);
    assert_eq!(plan.mappable_ranges[0].name, "text");
    assert_eq!(plan.mappable_ranges[0].file_offset.0, 0);
    assert_eq!(plan.mappable_ranges[0].file_size, 0x10000);
    assert_eq!(plan.mappable_ranges[0].memory_offset, ModuleOffset(0));
    assert_eq!(plan.mappable_ranges[0].memory_size, 0x10000);
    assert!(plan.mappable_ranges[0].read);
    assert!(!plan.mappable_ranges[0].write);
    assert!(plan.mappable_ranges[0].execute);

    assert_eq!(plan.mappable_ranges[1].name, "rodata");
    assert_eq!(plan.mappable_ranges[1].file_offset.0, 0x10000);
    assert_eq!(plan.mappable_ranges[1].file_size, 0x3000);
    assert_eq!(plan.mappable_ranges[1].memory_offset, ModuleOffset(0x10000));
    assert_eq!(plan.mappable_ranges[1].memory_size, 0x3000);
    assert!(plan.mappable_ranges[1].read);
    assert!(!plan.mappable_ranges[1].write);
    assert!(!plan.mappable_ranges[1].execute);

    assert_eq!(plan.mappable_ranges[2].name, "data");
    assert_eq!(plan.mappable_ranges[2].file_offset.0, 0x13000);
    assert_eq!(plan.mappable_ranges[2].file_size, 0x4000);
    assert_eq!(plan.mappable_ranges[2].memory_offset, ModuleOffset(0x13000));
    assert_eq!(plan.mappable_ranges[2].memory_size, 0x4000);
    assert!(plan.mappable_ranges[2].read);
    assert!(plan.mappable_ranges[2].write);
    assert!(!plan.mappable_ranges[2].execute);

    assert_eq!(plan.zero_fill_ranges.len(), 1);
    assert_eq!(plan.zero_fill_ranges[0].name, "bss");
    assert_eq!(
        plan.zero_fill_ranges[0].memory_offset,
        ModuleOffset(0x17000)
    );
    assert_eq!(plan.zero_fill_ranges[0].size, 0x1000);

    assert!(matches!(
        plan.relocation,
        wl_nro::RelocationRequirement::GuestSelfRelocating { .. }
    ));
    assert!(plan.startup.requires_loader_config);
    assert_eq!(plan.startup.x1_sentinel, 0xFFFF_FFFF_FFFF_FFFF);
    assert!(plan.startup.returnable_lr);
    assert_eq!(plan.startup.recommended_stack_size, 0x10_0000);
}
