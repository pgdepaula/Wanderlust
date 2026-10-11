//! Comprehensive synthetic validation and edge-case tests for wl-npdm.
//!
//! Covers: valid image, truncation, invalid magic, bounds violations, overflow,
//! section overlaps, alignment violations, malformed/unsupported capabilities,
//! process field limits, SAC parsing, FAC/FAH validation, and adversarial inputs.

use std::panic::catch_unwind;
use wl_npdm::*;

struct SyntheticNpdmBuilder {
    meta_magic: [u8; 4],
    flags0: u8,
    flags1: u8,
    main_prio: u8,
    default_cpu: u8,
    system_resource_size: u32,
    stack_size: u32,
    program_name: String,
    product_code: String,

    // ACID
    acid_magic: [u8; 4],
    acid_payload_size: Option<u32>,
    program_id_min: u64,
    program_id_max: u64,
    acid_flags: u32,
    acid_kac: Vec<u32>,
    acid_sac: Vec<u8>,
    acid_fac: Vec<u8>,

    // ACI0
    aci0_magic: [u8; 4],
    program_id: u64,
    aci0_kac: Vec<u32>,
    aci0_sac: Vec<u8>,
    aci0_fah: Vec<u8>,

    // Mutators
    corrupt_acid_offset: Option<u32>,
    corrupt_acid_size: Option<u32>,
    corrupt_aci_offset: Option<u32>,
    corrupt_aci_size: Option<u32>,
    corrupt_aci_kac_offset: Option<u32>,
    overlap_sections: bool,
    unaligned_kac_size: bool,
}

impl SyntheticNpdmBuilder {
    fn new() -> Self {
        // Construct valid 0x2C-byte FAC: version=1, counts=0, perms=0xFF
        let mut fac = vec![0u8; 0x2C];
        fac[0] = 1; // version 1
        fac[1] = 0; // content owner count
        fac[2] = 0; // save data owner count
        fac[4..12].copy_from_slice(&0xFFu64.to_le_bytes()); // perms

        // Construct valid 0x1C-byte FAH: version=1, perms=0x0F
        let mut fah = vec![0u8; 0x1C];
        fah[0] = 1; // version 1
        fah[4..12].copy_from_slice(&0x0Fu64.to_le_bytes()); // perms

        Self {
            meta_magic: MAGIC_META,
            flags0: 0x07, // 64-bit instruction (bit 0 = 1) | AddressSpace64Bit39 (bits 1..3 = 3)
            flags1: 0x00,
            main_prio: 44,
            default_cpu: 0,
            system_resource_size: 0x20_0000,
            stack_size: 0x10_0000,
            program_name: "test-proc".into(),
            product_code: "TEST-0001".into(),

            acid_magic: MAGIC_ACID,
            acid_payload_size: None,
            program_id_min: 0x0100_0000_0000_1000,
            program_id_max: 0x0100_0000_0000_1000,
            acid_flags: 1, // Retail/production
            acid_kac: Vec::new(),
            acid_sac: Vec::new(),
            acid_fac: fac,

            aci0_magic: MAGIC_ACI0,
            program_id: 0x0100_0000_0000_1000,
            aci0_kac: Vec::new(),
            aci0_sac: Vec::new(),
            aci0_fah: fah,

            corrupt_acid_offset: None,
            corrupt_acid_size: None,
            corrupt_aci_offset: None,
            corrupt_aci_size: None,
            corrupt_aci_kac_offset: None,
            overlap_sections: false,
            unaligned_kac_size: false,
        }
    }

    fn add_standard_capabilities(&mut self) {
        // KernelFlags (ID 3, trailing ones = 3)
        // lowest_prio=50 (bits 4..9), highest_prio=20 (bits 10..15), lowest_cpu=0, highest_cpu=3
        let kflags: u32 = 0b0111 | (50 << 4) | (20 << 10) | (3 << 24);
        self.aci0_kac.push(kflags);

        // SyscallMask (ID 4, trailing ones = 4)
        // slot=0, mask=0x00FF_FFFF (SVCs 0..=23)
        let syscall: u32 = 0b01111 | (0x00FF_FFFF << 5);
        self.aci0_kac.push(syscall);

        // MapRange (ID 6, trailing ones = 6) - 2 words
        let map_addr: u32 = 0b0111111 | ((0x8000_0000u32 >> 12) << 7); // RW
        let map_size: u32 = 0b0111111 | ((0x0001_0000u32 >> 12) << 7); // IO
        self.aci0_kac.push(map_addr);
        self.aci0_kac.push(map_size);

        // IoMemoryMap (ID 7, trailing ones = 7)
        let io_page: u32 = 0b01111111 | ((0x7000_0000u32 >> 12) << 8);
        self.aci0_kac.push(io_page);

        // MapRegion (ID 10, trailing ones = 10)
        let map_reg: u32 = 0b01111111111 | (1 << 11) | (1 << 17); // region 1, RO
        self.aci0_kac.push(map_reg);

        // InterruptPair (ID 11, trailing ones = 11)
        let irq_pair: u32 = 0b011111111111 | (42 << 12) | (0x3FF << 22); // irq0 = 42, irq1 = None
        self.aci0_kac.push(irq_pair);

        // ApplicationType (ID 13, trailing ones = 13)
        let app_type: u32 = 0b01111111111111 | (1 << 14); // 1 = Application
        self.aci0_kac.push(app_type);

        // KernelVersion (ID 14, trailing ones = 14)
        let kern_ver: u32 = 0b011111111111111 | (13 << 19); // 13.0
        self.aci0_kac.push(kern_ver);

        // HandleTable (ID 15, trailing ones = 15)
        let htable: u32 = 0b0111111111111111 | (512 << 16);
        self.aci0_kac.push(htable);

        // DebugFlags (ID 16, trailing ones = 16)
        let dbg_flags: u32 = 0b01111111111111111 | (1 << 17); // allow_debug
        self.aci0_kac.push(dbg_flags);

        // Empty padding (0xFFFFFFFF, trailing ones = 32)
        self.aci0_kac.push(0xFFFF_FFFF);
    }

    fn add_standard_services(&mut self) {
        // SAC client access: ctrl = (len - 1), no bit 7
        // "fsp-srv" (7 chars) -> ctrl = 6
        self.aci0_sac.push(6);
        self.aci0_sac.extend_from_slice(b"fsp-srv");

        // SAC server host: ctrl = 0x80 | (len - 1)
        // "my:srv" (6 chars) -> ctrl = 0x80 | 5 = 0x85
        self.aci0_sac.push(0x85);
        self.aci0_sac.extend_from_slice(b"my:srv");

        // SAC wildcard: "sm:*" (4 chars) -> ctrl = 3
        self.aci0_sac.push(3);
        self.aci0_sac.extend_from_slice(b"sm:*");
    }

    fn build(&self) -> Vec<u8> {
        let mut buffer = vec![0u8; META_HEADER_SIZE];

        // Fill META header
        buffer[0..4].copy_from_slice(&self.meta_magic);
        buffer[0x04..0x08].copy_from_slice(&1u32.to_le_bytes()); // key gen
        buffer[0x0C] = self.flags0;
        buffer[0x0D] = self.flags1;
        buffer[0x0E] = self.main_prio;
        buffer[0x0F] = self.default_cpu;
        buffer[0x14..0x18].copy_from_slice(&self.system_resource_size.to_le_bytes());
        buffer[0x18..0x1C].copy_from_slice(&0x0100_0000u32.to_le_bytes()); // version
        buffer[0x1C..0x20].copy_from_slice(&self.stack_size.to_le_bytes());

        let name_bytes = self.program_name.as_bytes();
        let name_len = name_bytes.len().min(16);
        buffer[0x20..0x20 + name_len].copy_from_slice(&name_bytes[..name_len]);

        let prod_bytes = self.product_code.as_bytes();
        let prod_len = prod_bytes.len().min(16);
        buffer[0x30..0x30 + prod_len].copy_from_slice(&prod_bytes[..prod_len]);

        // Construct ACID body
        let acid_offset = META_HEADER_SIZE as u32;
        let mut acid_buf = vec![0u8; ACID_HEADER_SIZE];
        acid_buf[0x200..0x204].copy_from_slice(&self.acid_magic);
        acid_buf[0x20C..0x210].copy_from_slice(&self.acid_flags.to_le_bytes());
        acid_buf[0x210..0x218].copy_from_slice(&self.program_id_min.to_le_bytes());
        acid_buf[0x218..0x220].copy_from_slice(&self.program_id_max.to_le_bytes());

        let mut acid_cursor = ACID_HEADER_SIZE as u32;

        let acid_fac_offset = if self.acid_fac.is_empty() {
            0
        } else {
            acid_cursor
        };
        let acid_fac_size = self.acid_fac.len() as u32;
        acid_buf.extend_from_slice(&self.acid_fac);
        if !self.acid_fac.is_empty() {
            acid_cursor += acid_fac_size;
        }

        let acid_sac_offset = if self.acid_sac.is_empty() {
            0
        } else {
            acid_cursor
        };
        let acid_sac_size = self.acid_sac.len() as u32;
        acid_buf.extend_from_slice(&self.acid_sac);
        if !self.acid_sac.is_empty() {
            acid_cursor += acid_sac_size;
        }

        let acid_kac_offset = if self.acid_kac.is_empty() {
            0
        } else {
            acid_cursor
        };
        let acid_kac_size = (self.acid_kac.len() * 4) as u32;
        for word in &self.acid_kac {
            acid_buf.extend_from_slice(&word.to_le_bytes());
        }

        acid_buf[0x220..0x224].copy_from_slice(&acid_fac_offset.to_le_bytes());
        acid_buf[0x224..0x228].copy_from_slice(&acid_fac_size.to_le_bytes());
        acid_buf[0x228..0x22C].copy_from_slice(&acid_sac_offset.to_le_bytes());
        acid_buf[0x22C..0x230].copy_from_slice(&acid_sac_size.to_le_bytes());
        acid_buf[0x230..0x234].copy_from_slice(&acid_kac_offset.to_le_bytes());
        acid_buf[0x234..0x238].copy_from_slice(&acid_kac_size.to_le_bytes());

        let acid_size = acid_buf.len() as u32;
        let payload_size = self
            .acid_payload_size
            .unwrap_or(acid_size.saturating_sub(0x100));
        acid_buf[0x204..0x208].copy_from_slice(&payload_size.to_le_bytes());

        // Construct ACI0 body
        let aci0_offset = if self.overlap_sections {
            acid_offset + 0x50 // Overlaps ACID
        } else {
            acid_offset + acid_size
        };

        let mut aci0_buf = vec![0u8; ACI0_HEADER_SIZE];
        aci0_buf[0x00..0x04].copy_from_slice(&self.aci0_magic);
        aci0_buf[0x10..0x18].copy_from_slice(&self.program_id.to_le_bytes());

        let mut aci0_cursor = ACI0_HEADER_SIZE as u32;

        let aci_fah_offset = if self.aci0_fah.is_empty() {
            0
        } else {
            aci0_cursor
        };
        let aci_fah_size = self.aci0_fah.len() as u32;
        aci0_buf.extend_from_slice(&self.aci0_fah);
        if !self.aci0_fah.is_empty() {
            aci0_cursor += aci_fah_size;
        }

        let aci_sac_offset = if self.aci0_sac.is_empty() {
            0
        } else {
            aci0_cursor
        };
        let aci_sac_size = self.aci0_sac.len() as u32;
        aci0_buf.extend_from_slice(&self.aci0_sac);
        if !self.aci0_sac.is_empty() {
            aci0_cursor += aci_sac_size;
        }

        let mut aci_kac_offset = if self.aci0_kac.is_empty() && !self.unaligned_kac_size {
            0
        } else {
            aci0_cursor
        };
        if let Some(corrupt) = self.corrupt_aci_kac_offset {
            aci_kac_offset = corrupt;
        }
        let mut aci_kac_size = (self.aci0_kac.len() * 4) as u32;
        for word in &self.aci0_kac {
            aci0_buf.extend_from_slice(&word.to_le_bytes());
        }
        if self.unaligned_kac_size {
            aci0_buf.push(0xAA); // Break 4-byte alignment
            aci_kac_size += 1;
        }

        aci0_buf[0x20..0x24].copy_from_slice(&aci_fah_offset.to_le_bytes());
        aci0_buf[0x24..0x28].copy_from_slice(&aci_fah_size.to_le_bytes());
        aci0_buf[0x28..0x2C].copy_from_slice(&aci_sac_offset.to_le_bytes());
        aci0_buf[0x2C..0x30].copy_from_slice(&aci_sac_size.to_le_bytes());
        aci0_buf[0x30..0x34].copy_from_slice(&aci_kac_offset.to_le_bytes());
        aci0_buf[0x34..0x38].copy_from_slice(&aci_kac_size.to_le_bytes());

        let aci0_size = aci0_buf.len() as u32;

        // Write offsets to META
        let final_acid_offset = self.corrupt_acid_offset.unwrap_or(acid_offset);
        let final_acid_size = self.corrupt_acid_size.unwrap_or(acid_size);
        let final_aci_offset = self.corrupt_aci_offset.unwrap_or(aci0_offset);
        let final_aci_size = self.corrupt_aci_size.unwrap_or(aci0_size);

        buffer[0x70..0x74].copy_from_slice(&final_aci_offset.to_le_bytes());
        buffer[0x74..0x78].copy_from_slice(&final_aci_size.to_le_bytes());
        buffer[0x78..0x7C].copy_from_slice(&final_acid_offset.to_le_bytes());
        buffer[0x7C..0x80].copy_from_slice(&final_acid_size.to_le_bytes());

        buffer.extend_from_slice(&acid_buf);
        buffer.extend_from_slice(&aci0_buf);

        buffer
    }
}

#[test]
fn test_valid_synthetic_npdm_parsing() {
    let mut builder = SyntheticNpdmBuilder::new();
    builder.add_standard_capabilities();
    builder.add_standard_services();
    let bytes = builder.build();

    let npdm = Npdm::parse(&bytes).expect("synthetic NPDM must parse cleanly");

    // Header validations
    assert_eq!(npdm.meta().program_name, "test-proc");
    assert_eq!(npdm.meta().product_code, "TEST-0001");
    assert_eq!(npdm.meta().main_thread_priority, 44);
    assert_eq!(npdm.meta().default_cpu_id, 0);
    assert_eq!(npdm.meta().system_resource_size, 0x20_0000);
    assert_eq!(npdm.meta().main_thread_stack_size, 0x10_0000);
    assert_eq!(
        npdm.meta().address_space,
        ProcessAddressSpace::AddressSpace64Bit39
    );
    assert!(npdm.meta().address_space.is_64_bit());
    assert!(npdm.acid().is_retail);
    assert_eq!(npdm.acid().pool_partition, 0);
    assert_eq!(npdm.aci0().program_id, 0x0100_0000_0000_1000);

    // Capabilities iterator validation
    let caps: Vec<Capability> = npdm
        .aci0_capabilities()
        .collect::<Result<Vec<_>, _>>()
        .expect("capabilities must parse cleanly");
    assert_eq!(caps.len(), 10); // 10 distinct capabilities, 0xFFFFFFFF empty was skipped

    match caps[0] {
        Capability::KernelFlags {
            highest_thread_priority,
            lowest_thread_priority,
            lowest_cpu_id,
            highest_cpu_id,
        } => {
            assert_eq!(highest_thread_priority, 20);
            assert_eq!(lowest_thread_priority, 50);
            assert_eq!(lowest_cpu_id, 0);
            assert_eq!(highest_cpu_id, 3);
        }
        _ => panic!("expected KernelFlags"),
    }

    match caps[1] {
        Capability::SyscallMask { slot, mask } => {
            assert_eq!(slot, 0);
            assert_eq!(mask, 0x00FF_FFFF);
        }
        _ => panic!("expected SyscallMask"),
    }

    match caps[2] {
        Capability::MapRange {
            address,
            size,
            is_read_only,
            is_io,
        } => {
            assert_eq!(address, 0x8000_0000);
            assert_eq!(size, 0x0001_0000);
            assert!(!is_read_only);
            assert!(is_io);
        }
        _ => panic!("expected MapRange"),
    }

    match caps[3] {
        Capability::IoMemoryMap { address } => {
            assert_eq!(address, 0x7000_0000);
        }
        _ => panic!("expected IoMemoryMap"),
    }

    match caps[5] {
        Capability::InterruptPair { irq0, irq1 } => {
            assert_eq!(irq0, Some(42));
            assert_eq!(irq1, None);
        }
        _ => panic!("expected InterruptPair"),
    }

    match caps[8] {
        Capability::HandleTable { size } => {
            assert_eq!(size, 512);
        }
        _ => panic!("expected HandleTable"),
    }

    // SAC Services iterator validation
    let services: Vec<SacEntry> = npdm
        .aci0_services()
        .collect::<Result<Vec<_>, _>>()
        .expect("services must parse cleanly");
    assert_eq!(services.len(), 3);
    assert_eq!(services[0].name, "fsp-srv");
    assert!(!services[0].is_server);
    assert!(!services[0].is_wildcard());
    assert!(services[0].matches("fsp-srv"));
    assert!(!services[0].matches("other"));

    assert_eq!(services[1].name, "my:srv");
    assert!(services[1].is_server);

    assert_eq!(services[2].name, "sm:*");
    assert!(services[2].is_wildcard());
    assert!(services[2].matches("sm:m"));
    assert!(services[2].matches("sm:user"));
    assert!(!services[2].matches("fsp:ldr"));

    // Filesystem access
    let fs = npdm
        .filesystem_access()
        .unwrap()
        .expect("FS control must exist");
    assert_eq!(fs.fac_version, 1);
    assert_eq!(fs.fah_version, 1);
    assert_eq!(fs.fac_permissions, 0xFF);
    assert_eq!(fs.fah_permissions, 0x0F);
    assert_eq!(fs.effective_permissions, 0x0F);
    assert!(fs.has_permission(0x01));
    assert!(!fs.has_permission(0x80));

    // ProcessMetadata bridge
    let proc_meta = npdm.to_process_metadata().expect("process metadata build");
    assert_eq!(proc_meta.program_id, 0x0100_0000_0000_1000);
    assert_eq!(proc_meta.ideal_core, 0);
    assert_eq!(proc_meta.handle_table_size, 512);
    assert!(proc_meta.is_application);
    assert!(proc_meta.syscall_bitmap.is_allowed(0));
    assert!(proc_meta.syscall_bitmap.is_allowed(23));
    assert!(!proc_meta.syscall_bitmap.is_allowed(24));
}

#[test]
fn test_truncated_buffer_failures() {
    assert!(matches!(
        Npdm::parse(&[]),
        Err(NpdmError::BufferTooSmall { expected: 0x80, .. })
    ));

    let builder = SyntheticNpdmBuilder::new();
    let bytes = builder.build();
    assert!(matches!(
        Npdm::parse(&bytes[..0x40]),
        Err(NpdmError::BufferTooSmall { expected: 0x80, .. })
    ));
}

#[test]
fn test_wrong_magic_failures() {
    let mut builder = SyntheticNpdmBuilder::new();
    builder.meta_magic = *b"NOPE";
    let bytes = builder.build();
    assert!(matches!(
        Npdm::parse(&bytes),
        Err(NpdmError::InvalidMetaMagic {
            found: [b'N', b'O', b'P', b'E']
        })
    ));

    let mut builder = SyntheticNpdmBuilder::new();
    builder.acid_magic = *b"ACIX";
    let bytes = builder.build();
    assert!(matches!(
        Npdm::parse(&bytes),
        Err(NpdmError::InvalidAcidMagic {
            found: [b'A', b'C', b'I', b'X']
        })
    ));

    let mut builder = SyntheticNpdmBuilder::new();
    builder.aci0_magic = *b"ACI9";
    let bytes = builder.build();
    assert!(matches!(
        Npdm::parse(&bytes),
        Err(NpdmError::InvalidAci0Magic {
            found: [b'A', b'C', b'I', b'9']
        })
    ));
}

#[test]
fn test_out_of_bounds_and_overflow() {
    let mut builder = SyntheticNpdmBuilder::new();
    builder.corrupt_acid_size = Some(0xFFFF_0000);
    let bytes = builder.build();
    assert!(matches!(
        Npdm::parse(&bytes),
        Err(NpdmError::SectionOutOfBounds {
            section: "ACID",
            ..
        })
    ));

    let mut builder = SyntheticNpdmBuilder::new();
    builder.corrupt_aci_offset = Some(0xFFFF_FFF0);
    builder.corrupt_aci_size = Some(0x1000);
    let bytes = builder.build();
    assert!(matches!(
        Npdm::parse(&bytes),
        Err(NpdmError::OffsetOverflow {
            section: "ACI0",
            ..
        })
    ));
}

#[test]
fn test_overlapping_sections() {
    let mut builder = SyntheticNpdmBuilder::new();
    builder.overlap_sections = true;
    let bytes = builder.build();
    assert!(matches!(
        Npdm::parse(&bytes),
        Err(NpdmError::SectionOverlap {
            first: "ACID",
            second: "ACI0"
        })
    ));
}

#[test]
fn test_alignment_violations() {
    let mut builder = SyntheticNpdmBuilder::new();
    builder.stack_size = 0x1234; // not 0x1000 aligned
    let bytes = builder.build();
    assert!(matches!(
        Npdm::parse(&bytes),
        Err(NpdmError::InvalidStackSize { size: 0x1234 })
    ));

    let mut builder = SyntheticNpdmBuilder::new();
    builder.system_resource_size = 0x1234; // not 0x1000 aligned
    let bytes = builder.build();
    assert!(matches!(
        Npdm::parse(&bytes),
        Err(NpdmError::InvalidSystemResourceSize { size: 0x1234 })
    ));

    let mut builder = SyntheticNpdmBuilder::new();
    builder.unaligned_kac_size = true;
    let bytes = builder.build();
    assert!(matches!(
        Npdm::parse(&bytes),
        Err(NpdmError::UnalignedSection {
            section: "ACI0 KAC",
            alignment: 4,
            ..
        })
    ));

    // KAC offset unaligned (e.g. 0x41)
    let mut builder = SyntheticNpdmBuilder::new();
    builder.corrupt_aci_kac_offset = Some(0x41);
    builder.aci0_kac.push(0xFFFF_FFFF);
    let bytes = builder.build();
    assert!(matches!(
        Npdm::parse(&bytes),
        Err(NpdmError::UnalignedSection {
            section: "ACI0 KAC",
            offset: 0x41,
            alignment: 4
        })
    ));
}

#[test]
fn test_invalid_process_field_ranges() {
    let mut builder = SyntheticNpdmBuilder::new();
    builder.main_prio = 64; // Max is 63
    let bytes = builder.build();
    assert!(matches!(
        Npdm::parse(&bytes),
        Err(NpdmError::InvalidPriority { priority: 64 })
    ));

    let mut builder = SyntheticNpdmBuilder::new();
    builder.default_cpu = 4; // Max is 3
    let bytes = builder.build();
    assert!(matches!(
        Npdm::parse(&bytes),
        Err(NpdmError::InvalidCpuId { cpu_id: 4 })
    ));

    let mut builder = SyntheticNpdmBuilder::new();
    builder.system_resource_size = 0x2000_0000; // Exceeds 0x1FE00000
    let bytes = builder.build();
    assert!(matches!(
        Npdm::parse(&bytes),
        Err(NpdmError::InvalidSystemResourceSize { .. })
    ));

    let mut builder = SyntheticNpdmBuilder::new();
    builder.flags0 = (5 << 1) | 1; // AddressSpace 5 is invalid
    let bytes = builder.build();
    assert!(matches!(
        Npdm::parse(&bytes),
        Err(NpdmError::InvalidAddressSpace { raw: 5 })
    ));

    let mut builder = SyntheticNpdmBuilder::new();
    builder.program_id = 0x0100_0000_0000_9999; // Outside ACID min/max
    let bytes = builder.build();
    assert!(matches!(
        Npdm::parse(&bytes),
        Err(NpdmError::ProgramIdOutOfAcidRange { .. })
    ));

    // program_id_min > program_id_max
    let mut builder = SyntheticNpdmBuilder::new();
    builder.program_id_min = 0x0200;
    builder.program_id_max = 0x0100;
    let bytes = builder.build();
    assert!(matches!(
        Npdm::parse(&bytes),
        Err(NpdmError::InvalidProgramIdRange {
            min: 0x0200,
            max: 0x0100
        })
    ));
}

#[test]
fn test_acid_payload_size_bounds() {
    let mut builder = SyntheticNpdmBuilder::new();
    builder.acid_payload_size = Some(0x50); // Less than 0x140 minimum header
    let bytes = builder.build();
    assert!(matches!(
        Npdm::parse(&bytes),
        Err(NpdmError::InvalidAcidPayloadSize { .. })
    ));

    let mut builder = SyntheticNpdmBuilder::new();
    builder.acid_payload_size = Some(0xFFFF_0000); // Exceeds section bounds
    let bytes = builder.build();
    assert!(matches!(
        Npdm::parse(&bytes),
        Err(NpdmError::InvalidAcidPayloadSize { .. })
    ));
}

#[test]
fn test_capability_negatives_and_diagnostics() {
    // Unpaired MapRange
    let mut builder = SyntheticNpdmBuilder::new();
    let map_addr: u32 = 0b0111111 | ((0x8000_0000u32 >> 12) << 7);
    builder.aci0_kac.push(map_addr); // Solo MapRange without size pair
    let bytes = builder.build();
    assert!(matches!(
        Npdm::parse(&bytes),
        Err(NpdmError::UnpairedMapRangeDescriptor { .. })
    ));

    // Unsupported descriptor bit patterns are preserved as Unknown in diagnostic form
    let mut builder = SyntheticNpdmBuilder::new();
    builder.aci0_kac.push(0b0100); // Trailing ones = 0 (raw = 4)
    builder.aci0_kac.push(0b0101); // Trailing ones = 1 (raw = 5)
    let bytes = builder.build();
    let npdm = Npdm::parse(&bytes).expect("unknown capability should be parsed in diagnostic form");
    let caps: Vec<Capability> = npdm
        .aci0_capabilities()
        .collect::<Result<Vec<_>, _>>()
        .unwrap();
    assert!(matches!(caps[0], Capability::Unknown { id: 0, raw: 4 }));
    assert!(matches!(caps[1], Capability::Unknown { id: 1, raw: 5 }));

    // Invalid priority bounds: highest (50) > lowest (20)
    let mut builder = SyntheticNpdmBuilder::new();
    let kflags_bad: u32 = 0b0111 | (20 << 4) | (50 << 10) | (3 << 24);
    builder.aci0_kac.push(kflags_bad);
    let bytes = builder.build();
    assert!(matches!(
        Npdm::parse(&bytes),
        Err(NpdmError::InvalidThreadPriorityBounds {
            highest: 50,
            lowest: 20
        })
    ));

    // Invalid core ID bounds
    let mut builder = SyntheticNpdmBuilder::new();
    let kflags_bad_core: u32 = 0b0111 | (50 << 4) | (20 << 10) | (2 << 16) | (1 << 24);
    builder.aci0_kac.push(kflags_bad_core);
    let bytes = builder.build();
    assert!(matches!(
        Npdm::parse(&bytes),
        Err(NpdmError::InvalidCpuIdBounds {
            lowest: 2,
            highest: 1
        })
    ));

    // Handle table size > 1024
    let mut builder = SyntheticNpdmBuilder::new();
    let htable_bad: u32 = 0b0111111111111111 | (1025 << 16);
    builder.aci0_kac.push(htable_bad);
    let bytes = builder.build();
    assert!(matches!(
        Npdm::parse(&bytes),
        Err(NpdmError::InvalidHandleTableSize { size: 1025 })
    ));
}

#[test]
fn test_adversarial_zero_size_subregions_never_panic() {
    // Test that corrupt offsets with size=0 are either safely rejected or don't panic
    let mut builder = SyntheticNpdmBuilder::new();
    builder.acid_kac.clear();
    builder.acid_sac.clear();
    builder.aci0_kac.clear();
    builder.aci0_sac.clear();
    let mut bytes = builder.build();

    let res = catch_unwind(|| {
        let npdm = Npdm::parse(&bytes).expect("empty subregions should parse");
        // Iterators must be empty and must never panic
        assert_eq!(npdm.acid_capabilities().count(), 0);
        assert_eq!(npdm.aci0_capabilities().count(), 0);
        assert_eq!(npdm.acid_services().count(), 0);
        assert_eq!(npdm.aci0_services().count(), 0);
    });
    assert!(res.is_ok(), "must not panic on zero-size subregions");

    // Overwrite ACI0 KAC offset with u32::MAX while size=0
    let aci0_offset = u32::from_le_bytes(bytes[0x70..0x74].try_into().unwrap()) as usize;
    bytes[aci0_offset + 0x30..aci0_offset + 0x34].copy_from_slice(&u32::MAX.to_le_bytes());
    // Parse must reject the out-of-parent offset even when size==0
    assert!(matches!(
        Npdm::parse(&bytes),
        Err(NpdmError::SectionOutOfBounds {
            section: "ACI0 KAC",
            ..
        })
    ));
}

#[test]
fn test_inner_subregion_errors() {
    // Inner FAC subregion starts before ACID header size (0x240)
    let builder = SyntheticNpdmBuilder::new();
    let mut bytes = builder.build();
    // Overwrite acid_fac_offset in ACID (offset 0x80 + 0x220 = 0x2A0) with 0x200 (less than 0x240)
    bytes[0x2A0..0x2A4].copy_from_slice(&0x200u32.to_le_bytes());
    assert!(matches!(
        Npdm::parse(&bytes),
        Err(NpdmError::SectionOutOfBounds {
            section: "ACID FAC",
            ..
        })
    ));

    // Inner subregions overlap in ACI0 (FAH and SAC overlap)
    let mut builder = SyntheticNpdmBuilder::new();
    builder.aci0_fah = vec![1, 0, 0, 0, 0xFF, 0, 0, 0, 0, 0, 0, 0];
    builder.aci0_sac = vec![3, b's', b'm', b':', b'*'];
    let mut bytes = builder.build();
    let aci0_offset = u32::from_le_bytes(bytes[0x70..0x74].try_into().unwrap()) as usize;
    // Set aci_sac_offset equal to aci_fah_offset
    let fah_off = bytes[aci0_offset + 0x20..aci0_offset + 0x24].to_vec();
    bytes[aci0_offset + 0x28..aci0_offset + 0x2C].copy_from_slice(&fah_off);
    assert!(matches!(
        Npdm::parse(&bytes),
        Err(NpdmError::SectionOverlap {
            first: "ACI0 FAH",
            second: "ACI0 SAC"
        })
    ));
}

#[test]
fn test_fac_fah_validation_errors() {
    // FAC truncated below 0x2C
    let mut builder = SyntheticNpdmBuilder::new();
    builder.acid_fac = vec![1, 0, 0];
    let bytes = builder.build();
    assert!(matches!(
        Npdm::parse(&bytes),
        Err(NpdmError::MalformedFsAccessControl {
            section: "ACID FAC",
            ..
        })
    ));

    // FAC version != 1
    let mut builder = SyntheticNpdmBuilder::new();
    let mut fac = vec![0u8; 0x2C];
    fac[0] = 2; // invalid version
    builder.acid_fac = fac;
    let bytes = builder.build();
    assert!(matches!(
        Npdm::parse(&bytes),
        Err(NpdmError::MalformedFsAccessControl {
            section: "ACID FAC",
            reason: "unsupported FAC version (must be 1)"
        })
    ));

    // FAH truncated below 0x1C
    let mut builder = SyntheticNpdmBuilder::new();
    builder.aci0_fah = vec![1, 0, 0, 0, 0];
    let bytes = builder.build();
    assert!(matches!(
        Npdm::parse(&bytes),
        Err(NpdmError::MalformedFsAccessControl {
            section: "ACI0 FAH",
            ..
        })
    ));
}

#[test]
fn test_convenience_parse_and_error_display() {
    let mut builder = SyntheticNpdmBuilder::new();
    builder.add_standard_capabilities();
    let bytes = builder.build();
    let res = parse_npdm(&bytes);
    assert!(res.is_ok());

    let err = NpdmError::InvalidPriority { priority: 70 };
    let err_str = err.to_string();
    assert!(err_str.contains("70"));
    let as_string: String = err.into();
    assert_eq!(as_string, err_str);
}

#[test]
fn test_sac_malformed_records() {
    let mut builder = SyntheticNpdmBuilder::new();
    // SAC control byte indicates 7-byte name, but buffer only has 2 bytes
    builder.aci0_sac = vec![6, b'a', b'b'];
    let bytes = builder.build();
    assert!(matches!(
        Npdm::parse(&bytes),
        Err(NpdmError::MalformedServiceAccessControl { .. })
    ));
}

#[test]
fn test_process_address_space_variants() {
    assert_eq!(
        ProcessAddressSpace::from_raw(0).unwrap(),
        ProcessAddressSpace::AddressSpace32Bit
    );
    assert_eq!(
        ProcessAddressSpace::from_raw(1).unwrap(),
        ProcessAddressSpace::AddressSpace64Bit36
    );
    assert_eq!(
        ProcessAddressSpace::from_raw(2).unwrap(),
        ProcessAddressSpace::AddressSpace32BitNoReserved
    );
    assert_eq!(
        ProcessAddressSpace::from_raw(3).unwrap(),
        ProcessAddressSpace::AddressSpace64Bit39
    );
    assert_eq!(
        ProcessAddressSpace::from_raw(4).unwrap(),
        ProcessAddressSpace::AddressSpace64Bit42
    );
    assert!(ProcessAddressSpace::from_raw(5).is_err());

    assert!(!ProcessAddressSpace::AddressSpace32Bit.is_64_bit());
    assert!(!ProcessAddressSpace::AddressSpace32BitNoReserved.is_64_bit());
    assert!(ProcessAddressSpace::AddressSpace64Bit36.is_64_bit());
    assert!(ProcessAddressSpace::AddressSpace64Bit39.is_64_bit());
    assert!(ProcessAddressSpace::AddressSpace64Bit42.is_64_bit());
}

#[test]
fn test_syscall_bitmap_coverage() {
    let mut bm = SyscallBitmap::default();
    assert!(!bm.is_allowed(0));
    assert!(!bm.is_allowed(255)); // out of bounds returns false

    // Slot 1 covers SVCs 24..47
    bm.apply_mask(1, 0b101); // enables SVC 24 and SVC 26
    assert!(bm.is_allowed(24));
    assert!(!bm.is_allowed(25));
    assert!(bm.is_allowed(26));

    // Slot > 7 ignored
    bm.apply_mask(8, 0xFF);
}

#[test]
fn test_signed_body_excludes_fac_regression() {
    let mut b = SyntheticNpdmBuilder::new();
    b.acid_payload_size = Some(0x140);
    let data = b.build();
    assert!(
        Npdm::parse(&data).is_err(),
        "ACID FAC beyond signed end +0x240 must be rejected"
    );
}

#[test]
fn test_unknown_kac_rejects_bootstrap_authorization_regression() {
    let mut b = SyntheticNpdmBuilder::new();
    b.aci0_kac.push(4); // trailing_ones = 0 (unknown descriptor)
    let data = b.build();
    let n = Npdm::parse(&data).expect("unknown capability should be accepted for inspection");
    assert!(
        n.to_process_metadata().is_err(),
        "unknown capability cannot silently become usable process metadata"
    );
}

#[test]
fn test_fah_info_bounds_regression() {
    let b = SyntheticNpdmBuilder::new();
    let mut data = b.build();
    let aci = u32::from_le_bytes(data[0x70..0x74].try_into().unwrap()) as usize;
    let fah = aci + u32::from_le_bytes(data[aci + 0x20..aci + 0x24].try_into().unwrap()) as usize;
    data[fah + 0x0c..fah + 0x10].copy_from_slice(&u32::MAX.to_le_bytes());
    data[fah + 0x10..fah + 0x14].copy_from_slice(&8u32.to_le_bytes());
    assert!(
        Npdm::parse(&data).is_err(),
        "FAH ContentOwnerInfoOffset+Size must be checked"
    );
}

#[test]
fn test_npdm_preflight_inspection() {
    let mut b = SyntheticNpdmBuilder::new();
    b.add_standard_capabilities();
    b.add_standard_services();
    b.aci0_kac.push(4); // add an unknown capability
    let data = b.build();

    let preflight = preflight_npdm(&data).expect("preflight succeeded");
    assert!(preflight.structural_validation_passed);
    assert_eq!(preflight.program_id, 0x0100_0000_0000_1000);
    assert_eq!(preflight.program_name, "test-proc");
    assert_eq!(preflight.product_code, "TEST-0001");
    assert_eq!(preflight.signature_verification, "not performed");
    assert_eq!(preflight.unknown_capabilities.len(), 1);
    assert_eq!(preflight.unknown_capabilities[0].raw, 4);
    assert_eq!(preflight.unknown_capabilities[0].id, 0);

    let display = format!("{}", preflight);
    assert!(display.contains("NPDM structural validation: passed"));
    assert!(display.contains("Program ID: 0x0100000000001000"));
    assert!(display.contains("Signature verification: not performed"));
}

#[test]
fn test_libnx_probe_core_priority_descriptor() {
    // Descriptor 0x020073b7: libnx template highest=28, lowest=59, cores 0..=2
    let desc: u32 = 0x020073b7;
    let bytes = desc.to_le_bytes();
    let mut caps = Capabilities::new(&bytes);
    let cap = caps
        .next()
        .expect("has capability")
        .expect("descriptor parses");
    assert_eq!(
        cap,
        Capability::KernelFlags {
            highest_thread_priority: 28,
            lowest_thread_priority: 59,
            lowest_cpu_id: 0,
            highest_cpu_id: 2,
        }
    );
}

#[test]
fn test_missing_core_priority_fails_process_metadata() {
    let builder = SyntheticNpdmBuilder::new();
    // Do not add KernelFlags
    let bytes = builder.build();
    let npdm = Npdm::parse(&bytes).expect("parse succeeds");
    assert!(matches!(
        npdm.to_process_metadata(),
        Err(NpdmError::MissingCorePriorityCapability)
    ));
}
