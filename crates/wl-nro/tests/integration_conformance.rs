//! Integration conformance tests verifying that HomebrewImagePlan produces
//! authoritative process memory layouts and meets all Horizon bootstrap requirements.
use sha2::{Digest, Sha256};
use std::path::Path;
use wl_common::{GuestAddr, ModuleOffset};
use wl_nro::{MAGIC_NRO, Nro, RelocationRequirement};

struct FixtureMeta {
    filename: &'static str,
    size: usize,
    sha256: &'static str,
    mod0_offset: u32,
    bss_size: u32,
}

const FIXTURES: &[FixtureMeta] = &[
    FixtureMeta {
        filename: "probe.nro",
        size: 94208,
        sha256: "d205f4e9279880373f3df5beaa76fe7d07a68fb2a51ef1864706c6a6f6b5618f",
        mod0_offset: 0xF0,
        bss_size: 0x1000,
    },
    FixtureMeta {
        filename: "ipc_probe.nro",
        size: 94208,
        sha256: "77ce3acb0385f5886b08763d3bd66f22d0192a663b7105bb5005bac7a3c39c7e",
        mod0_offset: 0xF0,
        bss_size: 0x1000,
    },
    FixtureMeta {
        filename: "display_probe.nro",
        size: 94208,
        sha256: "fa055f9f15ee72e0b2d5b1aeeb84f73777c0805bbf11066f9a0b4021550fcaa8",
        mod0_offset: 0xF0,
        bss_size: 0x1000,
    },
];

fn load_fixture(name: &str) -> Vec<u8> {
    let manifest_dir = Path::new(env!("CARGO_MANIFEST_DIR"));
    let fixture_path = manifest_dir.join("tests/fixtures").join(name);
    std::fs::read(&fixture_path)
        .unwrap_or_else(|e| panic!("failed to load {}: {e}", fixture_path.display()))
}

#[test]
fn test_stage1_parsing_conformance_matrix() {
    for fix in FIXTURES {
        let bytes = load_fixture(fix.filename);
        assert_eq!(
            bytes.len(),
            fix.size,
            "fixture {} size mismatch",
            fix.filename
        );

        let digest = Sha256::digest(&bytes);
        let hex = digest
            .iter()
            .map(|b| format!("{b:02x}"))
            .collect::<String>();
        assert_eq!(hex, fix.sha256, "fixture {} sha256 mismatch", fix.filename);

        let nro = Nro::parse(&bytes).expect("parse must succeed");
        assert_eq!(nro.header.magic, MAGIC_NRO);
        assert_eq!(nro.header.version, 0);
        assert_eq!(nro.header.size as usize, fix.size);
        assert_eq!(nro.header.bss_size, fix.bss_size);
        assert_eq!(nro.start.mod0_offset.0, fix.mod0_offset);

        // Standard 64-bit libnx segment sizes
        assert_eq!(nro.header.text.file_offset.0, 0);
        assert_eq!(nro.header.text.size, 0x10000);
        assert_eq!(nro.header.ro.file_offset.0, 0x10000);
        assert_eq!(nro.header.ro.size, 0x3000);
        assert_eq!(nro.header.data.file_offset.0, 0x13000);
        assert_eq!(nro.header.data.size, 0x4000);
    }
}

#[test]
fn test_stage2_mapping_and_zero_fill_invariants() {
    let guest_base = GuestAddr(0x800_0000);

    for fix in FIXTURES {
        let bytes = load_fixture(fix.filename);
        let nro = Nro::parse(&bytes).unwrap();
        let plan = nro.plan().expect("plan generation must succeed");

        assert_eq!(plan.entry_offset, 0, "entrypoint PC must be base + 0");
        assert_eq!(plan.page_alignment, 0x1000, "page alignment must be 4 KiB");
        assert_eq!(plan.image_end, 0x18000, "mapped image end must be 0x18000");

        // Validate mappable ranges
        assert_eq!(plan.mappable_ranges.len(), 3);
        let text = &plan.mappable_ranges[0];
        assert_eq!(text.name, "text");
        assert_eq!(text.file_offset.0, 0);
        assert_eq!(text.file_size, 0x10000);
        assert_eq!(text.memory_offset, ModuleOffset(0));
        assert_eq!(text.memory_size, 0x10000);
        assert!(text.read && !text.write && text.execute);

        let rodata = &plan.mappable_ranges[1];
        assert_eq!(rodata.name, "rodata");
        assert_eq!(rodata.file_offset.0, 0x10000);
        assert_eq!(rodata.file_size, 0x3000);
        assert_eq!(rodata.memory_offset, ModuleOffset(0x10000));
        assert_eq!(rodata.memory_size, 0x3000);
        assert!(rodata.read && !rodata.write && !rodata.execute);

        let data = &plan.mappable_ranges[2];
        assert_eq!(data.name, "data");
        assert_eq!(data.file_offset.0, 0x13000);
        assert_eq!(data.file_size, 0x4000);
        assert_eq!(data.memory_offset, ModuleOffset(0x13000));
        assert_eq!(data.memory_size, 0x4000);
        assert!(data.read && data.write && !data.execute);

        // Validate zero-fill BSS range
        assert_eq!(plan.zero_fill_ranges.len(), 1);
        let bss = &plan.zero_fill_ranges[0];
        assert_eq!(bss.name, "bss");
        assert_eq!(bss.memory_offset, ModuleOffset(0x17000));
        assert_eq!(bss.size, 0x1000);

        // Image mapping projection
        let image = nro.to_image().unwrap();
        let mappings = image.create_guest_mappings(guest_base);
        assert_eq!(mappings.len(), 3);

        // Text mapping
        assert_eq!(mappings[0].base, guest_base);
        assert_eq!(mappings[0].bytes.len(), 0x10000);
        assert!(mappings[0].execute && !mappings[0].write);

        // RoData mapping
        assert_eq!(mappings[1].base, GuestAddr(guest_base.0 + 0x10000));
        assert_eq!(mappings[1].bytes.len(), 0x3000);
        assert!(mappings[1].read && !mappings[1].write && !mappings[1].execute);

        // Data + BSS mapping
        assert_eq!(mappings[2].base, GuestAddr(guest_base.0 + 0x13000));
        assert_eq!(
            mappings[2].bytes.len(),
            0x4000 + 0x1000,
            "data mapping must include zero-filled BSS"
        );
        assert!(mappings[2].read && mappings[2].write && !mappings[2].execute);

        // Ensure BSS bytes are all zero
        let bss_bytes = &mappings[2].bytes[0x4000..];
        assert!(
            bss_bytes.iter().all(|&b| b == 0),
            "all BSS bytes must be zeroed"
        );
        // Ensure initialized data before BSS contains nonzero content
        assert!(
            mappings[2].bytes[..0x4000].iter().any(|&b| b != 0),
            "data section must have content"
        );
    }
}

#[test]
fn test_stage3_relocation_ownership_invariants() {
    for fix in FIXTURES {
        let bytes = load_fixture(fix.filename);
        let nro = Nro::parse(&bytes).unwrap();
        let plan = nro.plan().unwrap();

        // Must strictly declare GuestSelfRelocating
        match plan.relocation {
            RelocationRequirement::GuestSelfRelocating {
                mod0_offset,
                dynamic_offset,
            } => {
                assert_eq!(mod0_offset, fix.mod0_offset);
                assert!(
                    dynamic_offset > 0,
                    "dynamic_offset must be discovered from MOD0"
                );
            }
            other => panic!(
                "fixture {} must be GuestSelfRelocating, got {:?}",
                fix.filename, other
            ),
        }

        // Branch instruction at offset 0 must be `b 0x80` (opcode 0x14000020)
        let branch_op = u32::from_le_bytes(bytes[0..4].try_into().unwrap());
        assert_eq!(
            branch_op, 0x14000020,
            "first instruction must branch over header (b 0x80)"
        );
    }
}

#[test]
fn test_stage4_abi_and_startup_contract_invariants() {
    for fix in FIXTURES {
        let bytes = load_fixture(fix.filename);
        let nro = Nro::parse(&bytes).unwrap();
        let plan = nro.plan().unwrap();

        assert!(
            plan.startup.requires_loader_config,
            "homebrew ABI requires X0 loader config"
        );
        assert_eq!(
            plan.startup.x1_sentinel, 0xFFFF_FFFF_FFFF_FFFF,
            "X1 must be u64::MAX sentinel"
        );
        assert!(
            plan.startup.returnable_lr,
            "homebrew ABI requires returnable LR"
        );
        assert_eq!(
            plan.startup.recommended_stack_size, 0x10_0000,
            "stack size should be at least 1 MiB"
        );
        assert!(
            plan.startup.requires_main_thread_handle,
            "libnx crt0 requires main thread handle"
        );
        assert!(
            plan.unsupported_features.is_empty(),
            "no unsupported features in standard NROs"
        );
    }
}
