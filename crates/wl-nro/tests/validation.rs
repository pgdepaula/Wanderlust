use wl_common::{GuestAddr, ModuleOffset};
use wl_nro::{
    EntryConvention, LoaderConfig, LoaderConfigEntry, LoaderConfigKey, MAGIC_ASET, MAGIC_MOD0,
    MAGIC_NRO, NRO_BASE_HEADER_SIZE, Nro, NroError, NroFileOffset,
};

/// Helper to build a valid synthetic NRO binary.
fn make_synthetic_nro() -> Vec<u8> {
    let mut buf = vec![0u8; 0x3000]; // 12 KiB

    // NroStart (0x00..0x10)
    buf[0x00..0x04].copy_from_slice(&0x14000020u32.to_le_bytes()); // b +0x80
    buf[0x04..0x08].copy_from_slice(&0x80u32.to_le_bytes()); // mod0_offset at 0x80
    // padding at 0x08..0x10 is 0

    // NroHeader (0x10..0x80)
    buf[0x10..0x14].copy_from_slice(&MAGIC_NRO); // "NRO0"
    buf[0x14..0x18].copy_from_slice(&0u32.to_le_bytes()); // version 0
    buf[0x18..0x1C].copy_from_slice(&0x3000u32.to_le_bytes()); // size: 0x3000
    buf[0x1C..0x20].copy_from_slice(&0u32.to_le_bytes()); // flags

    // Text: file offset 0x0, size 0x1000
    buf[0x20..0x24].copy_from_slice(&0u32.to_le_bytes());
    buf[0x24..0x28].copy_from_slice(&0x1000u32.to_le_bytes());

    // RoData: file offset 0x1000, size 0x1000
    buf[0x28..0x2C].copy_from_slice(&0x1000u32.to_le_bytes());
    buf[0x2C..0x30].copy_from_slice(&0x1000u32.to_le_bytes());

    // Data: file offset 0x2000, size 0x1000
    buf[0x30..0x34].copy_from_slice(&0x2000u32.to_le_bytes());
    buf[0x34..0x38].copy_from_slice(&0x1000u32.to_le_bytes());

    // BSS: size 0x1000
    buf[0x38..0x3C].copy_from_slice(&0x1000u32.to_le_bytes());

    // Module ID (0x40..0x60): test pattern
    for i in 0..32 {
        buf[0x40 + i] = (i + 1) as u8;
    }

    // MOD0 header at offset 0x80
    buf[0x80..0x84].copy_from_slice(&MAGIC_MOD0); // "MOD0"

    buf
}

#[test]
fn test_valid_synthetic_nro_parsing() {
    let bytes = make_synthetic_nro();
    let nro = Nro::parse(&bytes).expect("valid NRO must parse cleanly");

    assert_eq!(nro.header.magic, MAGIC_NRO);
    assert_eq!(nro.header.version, 0);
    assert_eq!(nro.header.size, 0x3000);
    assert_eq!(nro.header.text.file_offset, NroFileOffset(0));
    assert_eq!(nro.header.text.size, 0x1000);
    assert_eq!(nro.header.ro.file_offset, NroFileOffset(0x1000));
    assert_eq!(nro.header.ro.size, 0x1000);
    assert_eq!(nro.header.data.file_offset, NroFileOffset(0x2000));
    assert_eq!(nro.header.data.size, 0x1000);
    assert_eq!(nro.header.bss_size, 0x1000);
    assert_eq!(nro.start.mod0_offset, NroFileOffset(0x80));
    assert_eq!(nro.text_slice().len(), 0x1000);
    assert_eq!(nro.ro_slice().len(), 0x1000);
    assert_eq!(nro.data_slice().len(), 0x1000);
    assert!(nro.assets().is_none());

    let image = nro.to_image().expect("image generation must succeed");
    assert_eq!(image.text.memory_offset, ModuleOffset(0));
    assert_eq!(image.rodata.memory_offset, ModuleOffset(0x1000));
    assert_eq!(image.data.memory_offset, ModuleOffset(0x2000));
    assert_eq!(image.bss_offset, ModuleOffset(0x3000));
    assert_eq!(image.bss_size, 0x1000);
    assert_eq!(image.total_memory_size, 0x4000);

    let mappings = image.create_guest_mappings(GuestAddr(0x800_0000));
    assert_eq!(mappings.len(), 3);
    assert_eq!(mappings[0].base, GuestAddr(0x800_0000));
    assert!(mappings[0].execute);
    assert!(!mappings[0].write);
    assert_eq!(mappings[1].base, GuestAddr(0x800_1000));
    assert!(!mappings[1].execute);
    assert!(!mappings[1].write);
    assert_eq!(mappings[2].base, GuestAddr(0x800_2000));
    assert!(mappings[2].write);
    assert!(!mappings[2].execute);
    // Data + BSS = 0x1000 + 0x1000 = 0x2000 bytes
    assert_eq!(mappings[2].size, 0x2000);
}

#[test]
fn test_wrong_magic_failures() {
    let mut bytes = make_synthetic_nro();
    bytes[0x10..0x14].copy_from_slice(b"NSO0");

    let err = Nro::parse(&bytes).unwrap_err();
    assert_eq!(
        err,
        NroError::InvalidMagic {
            expected: MAGIC_NRO,
            found: *b"NSO0"
        }
    );
}

#[test]
fn test_truncated_buffer_failures() {
    let bytes = make_synthetic_nro();

    let err = Nro::parse(&bytes[..0x7F]).unwrap_err();
    assert_eq!(
        err,
        NroError::BufferTooSmall {
            required: NRO_BASE_HEADER_SIZE,
            actual: 0x7F
        }
    );
}

#[test]
fn test_invalid_declared_size() {
    let mut bytes = make_synthetic_nro();
    // Declared size smaller than 0x80
    bytes[0x18..0x1C].copy_from_slice(&0x40u32.to_le_bytes());
    let err = Nro::parse(&bytes).unwrap_err();
    assert_eq!(
        err,
        NroError::InvalidDeclaredSize {
            declared: 0x40,
            file_size: bytes.len()
        }
    );

    // Declared size exceeding buffer length
    bytes[0x18..0x1C].copy_from_slice(&0x5000u32.to_le_bytes());
    let err2 = Nro::parse(&bytes).unwrap_err();
    assert_eq!(
        err2,
        NroError::InvalidDeclaredSize {
            declared: 0x5000,
            file_size: bytes.len()
        }
    );
}

#[test]
fn test_section_out_of_bounds() {
    let mut bytes = make_synthetic_nro();
    // Data section extends past declared 0x3000 size
    bytes[0x34..0x38].copy_from_slice(&0x2000u32.to_le_bytes()); // 0x2000 + 0x2000 = 0x4000 > 0x3000
    let err = Nro::parse(&bytes).unwrap_err();
    assert_eq!(
        err,
        NroError::SectionOutOfBounds {
            section: "data",
            offset: 0x2000,
            size: 0x2000,
            container_size: 0x3000
        }
    );
}

#[test]
fn test_section_overlap() {
    let mut bytes = make_synthetic_nro();
    // RoData offset at 0x800 overlaps with Text (0..0x1000)
    bytes[0x28..0x2C].copy_from_slice(&0x800u32.to_le_bytes());
    let err = Nro::parse(&bytes).unwrap_err();
    assert_eq!(
        err,
        NroError::SectionOverlap {
            first: "text",
            second: "rodata"
        }
    );
}

#[test]
fn test_alignment_violations() {
    let mut bytes = make_synthetic_nro();
    // Unaligned text size (0x1001)
    bytes[0x24..0x28].copy_from_slice(&0x1001u32.to_le_bytes());
    let err = Nro::parse(&bytes).unwrap_err();
    assert_eq!(
        err,
        NroError::AlignmentViolation {
            field: "text_size",
            offset: 0x1001,
            required: 4
        }
    );

    let mut bytes2 = make_synthetic_nro();
    // Unaligned bss size (0x1003)
    bytes2[0x38..0x3C].copy_from_slice(&0x1003u32.to_le_bytes());
    let err2 = Nro::parse(&bytes2).unwrap_err();
    assert_eq!(
        err2,
        NroError::AlignmentViolation {
            field: "bss_size",
            offset: 0x1003,
            required: 4
        }
    );
}

#[test]
fn test_mod0_validation() {
    let mut bytes = make_synthetic_nro();
    // Corrupt MOD0 magic at offset 0x80
    bytes[0x80..0x84].copy_from_slice(b"BAD0");
    let err = Nro::parse(&bytes).unwrap_err();
    assert_eq!(
        err,
        NroError::MissingMod0Magic {
            offset: 0x80,
            found: *b"BAD0"
        }
    );

    // MOD0 offset out of bounds
    let mut bytes2 = make_synthetic_nro();
    bytes2[0x04..0x08].copy_from_slice(&0x3500u32.to_le_bytes());
    let err2 = Nro::parse(&bytes2).unwrap_err();
    assert_eq!(
        err2,
        NroError::InvalidMod0Offset {
            offset: 0x3500,
            file_size: 0x3000
        }
    );
}

#[test]
fn test_asset_trailer_parsing() {
    let mut bytes = make_synthetic_nro();

    // Append ASET trailer
    let mut aset = vec![0u8; 0x1000]; // 4 KiB asset trailer

    aset[0x00..0x04].copy_from_slice(&MAGIC_ASET);
    aset[0x04..0x08].copy_from_slice(&0u32.to_le_bytes()); // version 0

    // Icon: offset 0x38, size 0x100
    aset[0x08..0x10].copy_from_slice(&0x38u64.to_le_bytes());
    aset[0x10..0x18].copy_from_slice(&0x100u64.to_le_bytes());

    // NACP: offset 0x138, size 0x200
    aset[0x18..0x20].copy_from_slice(&0x138u64.to_le_bytes());
    aset[0x20..0x28].copy_from_slice(&0x200u64.to_le_bytes());

    // RomFS: offset 0x338, size 0x400
    aset[0x28..0x30].copy_from_slice(&0x338u64.to_le_bytes());
    aset[0x30..0x38].copy_from_slice(&0x400u64.to_le_bytes());

    bytes.extend_from_slice(&aset);

    let nro = Nro::parse(&bytes).expect("NRO with ASET must parse cleanly");
    assert!(nro.assets.is_some());
    let assets = nro.assets().unwrap();
    assert_eq!(assets.header.magic, MAGIC_ASET);
    assert_eq!(assets.header.version, 0);
    assert_eq!(assets.icon_bytes.unwrap().len(), 0x100);
    assert_eq!(assets.nacp_bytes.unwrap().len(), 0x200);
    assert_eq!(assets.romfs_bytes.unwrap().len(), 0x400);

    let report = nro.inspect();
    assert!(report.has_aset);
    assert_eq!(report.icon_size, 0x100);
    assert_eq!(report.nacp_size, 0x200);
    assert_eq!(report.romfs_size, 0x400);
}

#[test]
fn test_asset_out_of_bounds_and_overlap() {
    let bytes = make_synthetic_nro();

    // 1. Asset extending beyond trailer length
    let mut aset = vec![0u8; 0x100];
    aset[0x00..0x04].copy_from_slice(&MAGIC_ASET);
    aset[0x08..0x10].copy_from_slice(&0x38u64.to_le_bytes());
    aset[0x10..0x18].copy_from_slice(&0x200u64.to_le_bytes()); // 0x38 + 0x200 > 0x100
    let mut b1 = bytes.clone();
    b1.extend_from_slice(&aset);
    let err = Nro::parse(&b1).unwrap_err();
    assert_eq!(
        err,
        NroError::AssetOutOfBounds {
            asset: "icon",
            offset: 0x38,
            size: 0x200,
            container_size: 0x100
        }
    );

    // 2. Overlapping asset sections
    let mut aset2 = vec![0u8; 0x500];
    aset2[0x00..0x04].copy_from_slice(&MAGIC_ASET);
    aset2[0x08..0x10].copy_from_slice(&0x38u64.to_le_bytes());
    aset2[0x10..0x18].copy_from_slice(&0x100u64.to_le_bytes());
    // NACP starts at 0x80 (overlaps icon 0x38..0x138)
    aset2[0x18..0x20].copy_from_slice(&0x80u64.to_le_bytes());
    aset2[0x20..0x28].copy_from_slice(&0x100u64.to_le_bytes());
    let mut b2 = bytes.clone();
    b2.extend_from_slice(&aset2);
    let err2 = Nro::parse(&b2).unwrap_err();
    assert_eq!(
        err2,
        NroError::AssetOverlap {
            first: "icon",
            second: "nacp"
        }
    );
}

#[test]
fn test_homebrew_abi_and_loader_config() {
    let convention = EntryConvention::default_homebrew(GuestAddr(0x1000_0000));
    match convention {
        EntryConvention::HomebrewNro {
            x0_loader_config,
            x1_sentinel,
            lr_returnable,
        } => {
            assert_eq!(x0_loader_config, GuestAddr(0x1000_0000));
            assert_eq!(x1_sentinel, 0xFFFF_FFFF_FFFF_FFFF);
            assert!(lr_returnable);
        }
        _ => panic!("expected HomebrewNro"),
    }

    // LoaderConfig serialize and parse
    let mut config = LoaderConfig::default();
    config.entries.push(LoaderConfigEntry::new(
        LoaderConfigKey::MainThreadHandle,
        1,
        0xDEADBEEF,
        0,
    ));
    config.entries.push(LoaderConfigEntry::new(
        LoaderConfigKey::SyscallAvailableHint,
        0,
        0x12345678,
        0,
    ));
    config
        .entries
        .push(LoaderConfigEntry::new(LoaderConfigKey::EndOfList, 1, 0, 0));

    let encoded = config.to_bytes();
    assert_eq!(encoded.len(), 3 * LoaderConfigEntry::SIZE);

    let parsed = LoaderConfig::parse(&encoded).expect("loader config parse must succeed");
    assert_eq!(parsed.entries.len(), 3);
    assert_eq!(parsed.entries[0].key, LoaderConfigKey::MainThreadHandle);
    assert_eq!(parsed.entries[0].value[0], 0xDEADBEEF);
    assert!(parsed.entries[0].is_mandatory());
    assert_eq!(parsed.entries[1].key, LoaderConfigKey::SyscallAvailableHint);
    assert_eq!(parsed.entries[2].key, LoaderConfigKey::EndOfList);
}
