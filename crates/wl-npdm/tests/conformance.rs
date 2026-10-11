//! Independent conformance tests using literal byte-level fixtures and hardcoded expected values.
//!
//! Grounded strictly in documented Switchbrew / Atmosphère offsets and specifications,
//! without reliance on SyntheticNpdmBuilder.

use wl_npdm::*;

#[test]
fn test_literal_byte_fixture_conformance() {
    let mut data = vec![0u8; 0x480];

    // ==========================================
    // META Header: 0x00..0x80
    // ==========================================
    // Magic at 0x00: "META"
    data[0x00..0x04].copy_from_slice(b"META");
    // Signature key generation at 0x04: 2
    data[0x04..0x08].copy_from_slice(&2u32.to_le_bytes());
    // Flags0 at 0x0C: 0x09 (bit 0 = 64-bit, bits 1..3 = 4 (AddressSpace64Bit42))
    data[0x0C] = 0x09;
    // Flags1 at 0x0D: 0x01 (Shadow stack enabled)
    data[0x0D] = 0x01;
    // Main thread priority at 0x0E: 32
    data[0x0E] = 32;
    // Default CPU core at 0x0F: 2
    data[0x0F] = 2;
    // System resource size at 0x14: 0x100000 (1 MiB, page-aligned)
    data[0x14..0x18].copy_from_slice(&0x10_0000u32.to_le_bytes());
    // Version at 0x18: 0x0200_0000
    data[0x18..0x1C].copy_from_slice(&0x0200_0000u32.to_le_bytes());
    // Main thread stack size at 0x1C: 0x80000 (512 KiB, page-aligned)
    data[0x1C..0x20].copy_from_slice(&0x08_0000u32.to_le_bytes());
    // Program name at 0x20: "TotK-Target\0\0\0\0\0"
    data[0x20..0x2B].copy_from_slice(b"TotK-Target");
    // Product code at 0x30: "AX5PA\0\0\0\0\0\0\0\0\0\0\0"
    data[0x30..0x35].copy_from_slice(b"AX5PA");
    // ACI0 offset at 0x70: 0x380
    data[0x70..0x74].copy_from_slice(&0x380u32.to_le_bytes());
    // ACI0 size at 0x74: 0x100
    data[0x74..0x78].copy_from_slice(&0x100u32.to_le_bytes());
    // ACID offset at 0x78: 0x80
    data[0x78..0x7C].copy_from_slice(&0x80u32.to_le_bytes());
    // ACID size at 0x7C: 0x300
    data[0x7C..0x80].copy_from_slice(&0x300u32.to_le_bytes());

    // ==========================================
    // ACID Section: 0x80..0x380 (size 0x300)
    // ==========================================
    let acid_base = 0x80;
    // Magic at +0x200: "ACID"
    data[acid_base + 0x200..acid_base + 0x204].copy_from_slice(b"ACID");
    // Signed payload size at +0x204: 0x200 (covers up to +0x300 from ACID base)
    data[acid_base + 0x204..acid_base + 0x208].copy_from_slice(&0x200u32.to_le_bytes());
    // Flags at +0x20C: 1 (Retail)
    data[acid_base + 0x20C..acid_base + 0x210].copy_from_slice(&1u32.to_le_bytes());
    // Program ID Min at +0x210: 0x0100_7EF0_0011_E000
    data[acid_base + 0x210..acid_base + 0x218]
        .copy_from_slice(&0x0100_7EF0_0011_E000u64.to_le_bytes());
    // Program ID Max at +0x218: 0x0100_7EF0_0011_E000
    data[acid_base + 0x218..acid_base + 0x220]
        .copy_from_slice(&0x0100_7EF0_0011_E000u64.to_le_bytes());

    // ACID FAC at +0x240, size 0x2C
    data[acid_base + 0x220..acid_base + 0x224].copy_from_slice(&0x240u32.to_le_bytes());
    data[acid_base + 0x224..acid_base + 0x228].copy_from_slice(&0x2Cu32.to_le_bytes());
    data[acid_base + 0x240] = 1; // FAC version 1
    data[acid_base + 0x244..acid_base + 0x24C]
        .copy_from_slice(&0x00FF_FFFF_FFFF_FFFFu64.to_le_bytes());

    // ACID SAC at +0x270, size 8 (server "fsp-srv\0")
    data[acid_base + 0x228..acid_base + 0x22C].copy_from_slice(&0x270u32.to_le_bytes());
    data[acid_base + 0x22C..acid_base + 0x230].copy_from_slice(&8u32.to_le_bytes());
    data[acid_base + 0x270] = 6 | 0x80; // server bit set, len = 7
    data[acid_base + 0x271..acid_base + 0x278].copy_from_slice(b"fsp-srv");

    // ACID KAC at +0x280, size 16 (4 words)
    data[acid_base + 0x230..acid_base + 0x234].copy_from_slice(&0x280u32.to_le_bytes());
    data[acid_base + 0x234..acid_base + 0x238].copy_from_slice(&16u32.to_le_bytes());
    // Word 0: KernelFlags (0b0111 | (60 << 4) | (10 << 10) | (0 << 16) | (3 << 24))
    data[acid_base + 0x280..acid_base + 0x284]
        .copy_from_slice(&(0b0111u32 | (60 << 4) | (10 << 10) | (3 << 24)).to_le_bytes());
    // Word 1: ApplicationType (0b01111111111111 | (1 << 14))
    data[acid_base + 0x284..acid_base + 0x288]
        .copy_from_slice(&(0b01111111111111u32 | (1 << 14)).to_le_bytes());
    // Word 2: HandleTable (0b0111111111111111 | (1024 << 16))
    data[acid_base + 0x288..acid_base + 0x28C]
        .copy_from_slice(&(0b0111111111111111u32 | (1024 << 16)).to_le_bytes());
    // Word 3: Padding (0xFFFFFFFF)
    data[acid_base + 0x28C..acid_base + 0x290].copy_from_slice(&0xFFFF_FFFFu32.to_le_bytes());

    // ==========================================
    // ACI0 Section: 0x380..0x480 (size 0x100)
    // ==========================================
    let aci_base = 0x380;
    // Magic at +0x00: "ACI0"
    data[aci_base..aci_base + 0x04].copy_from_slice(b"ACI0");
    // Program ID at +0x10: 0x0100_7EF0_0011_E000
    data[aci_base + 0x10..aci_base + 0x18].copy_from_slice(&0x0100_7EF0_0011_E000u64.to_le_bytes());

    // ACI0 FAH at +0x40, size 0x24 (0x1C header + 8 bytes owner ID)
    data[aci_base + 0x20..aci_base + 0x24].copy_from_slice(&0x40u32.to_le_bytes());
    data[aci_base + 0x24..aci_base + 0x28].copy_from_slice(&0x24u32.to_le_bytes());
    data[aci_base + 0x40] = 1; // FAH version 1
    data[aci_base + 0x44..aci_base + 0x4C].copy_from_slice(&0x0000_0000_0000_0021u64.to_le_bytes()); // perms: bit 0 + bit 5
    // ContentOwnerInfo at offset 0x1C relative to FAH start, size 8
    data[aci_base + 0x4C..aci_base + 0x50].copy_from_slice(&0x1Cu32.to_le_bytes());
    data[aci_base + 0x50..aci_base + 0x54].copy_from_slice(&8u32.to_le_bytes());
    // SaveDataOwnerInfo at 0, size 0
    data[aci_base + 0x54..aci_base + 0x58].copy_from_slice(&0u32.to_le_bytes());
    data[aci_base + 0x58..aci_base + 0x5C].copy_from_slice(&0u32.to_le_bytes());
    // Owner ID value at FAH + 0x1C (0x40 + 0x1C = 0x5C): 0x0100_7EF0_0011_E000
    data[aci_base + 0x5C..aci_base + 0x64].copy_from_slice(&0x0100_7EF0_0011_E000u64.to_le_bytes());

    // ACI0 SAC at +0x70, size 11 ("sm:*" client access, "nvdrv")
    data[aci_base + 0x28..aci_base + 0x2C].copy_from_slice(&0x70u32.to_le_bytes());
    data[aci_base + 0x2C..aci_base + 0x30].copy_from_slice(&11u32.to_le_bytes());
    // Entry 1: "sm:*" (4 chars, ctrl = 3)
    data[aci_base + 0x70] = 3;
    data[aci_base + 0x71..aci_base + 0x75].copy_from_slice(b"sm:*");
    // Entry 2: "nvdrv" (5 chars, ctrl = 4)
    data[aci_base + 0x75] = 4;
    data[aci_base + 0x76..aci_base + 0x7B].copy_from_slice(b"nvdrv");

    // ACI0 KAC at +0x80, size 16
    data[aci_base + 0x30..aci_base + 0x34].copy_from_slice(&0x80u32.to_le_bytes());
    data[aci_base + 0x34..aci_base + 0x38].copy_from_slice(&16u32.to_le_bytes());
    // KAC 0: KernelFlags (prio 20..=50, core 0..=3)
    data[aci_base + 0x80..aci_base + 0x84]
        .copy_from_slice(&(0b0111u32 | (50 << 4) | (20 << 10) | (3 << 24)).to_le_bytes());
    // KAC 1: SyscallMask slot 0 mask 0x01 (SVC 0 allowed)
    data[aci_base + 0x84..aci_base + 0x88].copy_from_slice(&(0b01111u32 | (1 << 5)).to_le_bytes());
    // KAC 2: IoMemoryMap page 0x8000_0000 (shifted by 12 = 0x80000, descriptor 0x0080_007F)
    data[aci_base + 0x88..aci_base + 0x8C]
        .copy_from_slice(&(0b01111111u32 | (0x80000 << 8)).to_le_bytes());
    // KAC 3: ApplicationType (1 = Application)
    data[aci_base + 0x8C..aci_base + 0x90]
        .copy_from_slice(&(0b01111111111111u32 | (1 << 14)).to_le_bytes());

    // ==========================================
    // Run Conformance Assertions
    // ==========================================
    let npdm = Npdm::parse(&data).expect("literal conformance NPDM parses successfully");

    // 1. META Header exact values
    let meta = npdm.meta();
    assert_eq!(meta.signature_key_generation, 2);
    assert!(meta.is_64_bit_instruction);
    assert_eq!(meta.address_space, ProcessAddressSpace::AddressSpace64Bit42);
    assert!(meta.enable_shadow_stack);
    assert_eq!(meta.main_thread_priority, 32);
    assert_eq!(meta.default_cpu_id, 2);
    assert_eq!(meta.system_resource_size, 0x10_0000);
    assert_eq!(meta.main_thread_stack_size, 0x08_0000);
    assert_eq!(meta.program_name, "TotK-Target");
    assert_eq!(meta.product_code, "AX5PA");

    // 2. ACID exact values
    let acid = npdm.acid();
    assert_eq!(acid.size, 0x200);
    assert!(acid.is_retail);
    assert_eq!(acid.program_id_min, 0x0100_7EF0_0011_E000);
    assert_eq!(acid.program_id_max, 0x0100_7EF0_0011_E000);

    // 3. ACI0 exact values
    let aci0 = npdm.aci0();
    assert_eq!(aci0.program_id, 0x0100_7EF0_0011_E000);

    // 4. Filesystem Access Control exact values
    let fs = npdm.filesystem_access().unwrap().expect("valid fs");
    assert_eq!(fs.fac_version, 1);
    assert_eq!(fs.fah_version, 1);
    assert_eq!(fs.fac_permissions, 0x00FF_FFFF_FFFF_FFFF);
    assert_eq!(fs.fah_permissions, 0x0000_0000_0000_0021);
    assert_eq!(fs.effective_permissions, 0x0000_0000_0000_0021);
    assert!(fs.has_permission(0x01)); // MountContent
    assert!(fs.has_permission(0x20)); // SaveDataBackUp
    assert!(!fs.has_permission(0x04));
    assert_eq!(fs.fah_content_owner_ids.len(), 8);
    assert_eq!(
        u64::from_le_bytes(fs.fah_content_owner_ids[..8].try_into().unwrap()),
        0x0100_7EF0_0011_E000
    );

    // 5. Service Access Control entries
    let services: Vec<_> = npdm.aci0_services().collect::<Result<Vec<_>, _>>().unwrap();
    assert_eq!(services.len(), 2);
    assert!(!services[0].is_server);
    assert_eq!(services[0].name, "sm:*");
    assert!(services[0].matches("sm:m"));
    assert!(services[0].matches("sm:"));
    assert!(!services[0].matches("fsp-srv"));
    assert!(!services[1].is_server);
    assert_eq!(services[1].name, "nvdrv");
    assert!(services[1].matches("nvdrv"));
    assert!(!services[1].matches("nvdrv:a"));

    // 6. Kernel Capabilities
    let caps: Vec<_> = npdm
        .aci0_capabilities()
        .collect::<Result<Vec<_>, _>>()
        .unwrap();
    assert_eq!(caps.len(), 4);
    assert!(matches!(
        caps[0],
        Capability::KernelFlags {
            highest_thread_priority: 20,
            lowest_thread_priority: 50,
            lowest_cpu_id: 0,
            highest_cpu_id: 3,
        }
    ));
    assert!(matches!(
        caps[1],
        Capability::SyscallMask { slot: 0, mask: 1 }
    ));
    assert!(matches!(
        caps[2],
        Capability::IoMemoryMap {
            address: 0x8000_0000
        }
    ));
    assert!(matches!(
        caps[3],
        Capability::ApplicationType {
            application_type: 1
        }
    ));

    // 7. ProcessMetadata conversion
    let proc_meta = npdm.to_process_metadata().expect("process metadata");
    assert_eq!(proc_meta.program_id, 0x0100_7EF0_0011_E000);
    assert_eq!(proc_meta.main_thread_priority, 32);
    assert_eq!(proc_meta.ideal_core, 2);
    assert_eq!(proc_meta.main_thread_stack_size, 0x08_0000);
    assert_eq!(proc_meta.system_resource_size, 0x10_0000);
    assert_eq!(proc_meta.handle_table_size, 1024);
    assert!(proc_meta.is_application);
    assert!(proc_meta.syscall_bitmap.is_allowed(0));
    assert!(!proc_meta.syscall_bitmap.is_allowed(1));
    assert_eq!(proc_meta.effective_fs_permissions, 0x21);

    // 8. Preflight
    let preflight = npdm.preflight();
    assert!(preflight.structural_validation_passed);
    assert_eq!(preflight.known_capability_count, 4);
    assert!(preflight.unknown_capabilities.is_empty());
    assert_eq!(preflight.allowed_services.len(), 2);
    assert_eq!(preflight.signature_verification, "not performed");
    assert_eq!(preflight.horizon_runtime_compatibility, "AArch64 candidate");
}
