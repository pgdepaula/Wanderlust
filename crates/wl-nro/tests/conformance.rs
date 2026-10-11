use wl_nro::{MAGIC_ASET, MAGIC_NRO, Nro, NroFileOffset};

#[test]
fn test_literal_byte_fixture_conformance() {
    let mut fixture = vec![0u8; 0x100];

    // Offsets according to Switchbrew NRO0 specification & libnx nro.h
    fixture[0x00..0x04].copy_from_slice(&0x14000020u32.to_le_bytes()); // branch: b +0x80
    fixture[0x04..0x08].copy_from_slice(&0x90u32.to_le_bytes()); // mod0_offset: 0x90
    fixture[0x10..0x14].copy_from_slice(b"NRO0"); // magic: NRO0
    fixture[0x14..0x18].copy_from_slice(&0u32.to_le_bytes()); // version: 0
    fixture[0x18..0x1C].copy_from_slice(&0x100u32.to_le_bytes()); // size: 0x100
    fixture[0x1C..0x20].copy_from_slice(&1u32.to_le_bytes()); // flags: AlignedHeader = 1

    fixture[0x20..0x24].copy_from_slice(&0u32.to_le_bytes()); // text offset: 0
    fixture[0x24..0x28].copy_from_slice(&0x80u32.to_le_bytes()); // text size: 0x80

    fixture[0x28..0x2C].copy_from_slice(&0x80u32.to_le_bytes()); // ro offset: 0x80
    fixture[0x2C..0x30].copy_from_slice(&0x40u32.to_le_bytes()); // ro size: 0x40

    fixture[0x30..0x34].copy_from_slice(&0xC0u32.to_le_bytes()); // data offset: 0xC0
    fixture[0x34..0x38].copy_from_slice(&0x40u32.to_le_bytes()); // data size: 0x40

    fixture[0x38..0x3C].copy_from_slice(&0x1000u32.to_le_bytes()); // bss size: 0x1000
    fixture[0x3C..0x40].copy_from_slice(&0u32.to_le_bytes()); // reserved

    // Module ID (0x40..0x60)
    for i in 0..32 {
        fixture[0x40 + i] = 0xAA;
    }

    fixture[0x60..0x64].copy_from_slice(&0x88u32.to_le_bytes()); // dso_handle
    fixture[0x68..0x6C].copy_from_slice(&0x84u32.to_le_bytes()); // embedded_offset
    fixture[0x6C..0x70].copy_from_slice(&0x10u32.to_le_bytes()); // embedded_size
    fixture[0x70..0x74].copy_from_slice(&0x94u32.to_le_bytes()); // dyn_str_offset
    fixture[0x74..0x78].copy_from_slice(&0x20u32.to_le_bytes()); // dyn_str_size
    fixture[0x78..0x7C].copy_from_slice(&0xB4u32.to_le_bytes()); // dyn_sym_offset
    fixture[0x7C..0x80].copy_from_slice(&0x0Cu32.to_le_bytes()); // dyn_sym_size

    // MOD0 header at offset 0x90
    fixture[0x90..0x94].copy_from_slice(b"MOD0");

    let nro = Nro::parse(&fixture).expect("conformance parse must succeed");

    assert_eq!(nro.start.branch_instruction, 0x14000020);
    assert_eq!(nro.start.mod0_offset, NroFileOffset(0x90));
    assert_eq!(nro.header.magic, MAGIC_NRO);
    assert_eq!(nro.header.version, 0);
    assert_eq!(nro.header.size, 0x100);
    assert_eq!(nro.header.flags, 1);
    assert_eq!(nro.header.text.file_offset, NroFileOffset(0));
    assert_eq!(nro.header.text.size, 0x80);
    assert_eq!(nro.header.ro.file_offset, NroFileOffset(0x80));
    assert_eq!(nro.header.ro.size, 0x40);
    assert_eq!(nro.header.data.file_offset, NroFileOffset(0xC0));
    assert_eq!(nro.header.data.size, 0x40);
    assert_eq!(nro.header.bss_size, 0x1000);
    assert_eq!(nro.header.module_id, [0xAA; 32]);
    assert_eq!(nro.header.dso_handle_offset, 0x88);
    assert_eq!(nro.header.embedded_info.offset, 0x84);
    assert_eq!(nro.header.embedded_info.size, 0x10);
    assert_eq!(nro.header.dyn_str.offset, 0x94);
    assert_eq!(nro.header.dyn_str.size, 0x20);
    assert_eq!(nro.header.dyn_sym.offset, 0xB4);
    assert_eq!(nro.header.dyn_sym.size, 0x0C);
}

#[test]
fn test_literal_aset_fixture_conformance() {
    let mut aset_fixture = vec![0u8; 0x100];
    aset_fixture[0x00..0x04].copy_from_slice(b"ASET");
    aset_fixture[0x04..0x08].copy_from_slice(&0u32.to_le_bytes()); // version: 0

    aset_fixture[0x08..0x10].copy_from_slice(&0x38u64.to_le_bytes()); // icon off: 0x38
    aset_fixture[0x10..0x18].copy_from_slice(&0x40u64.to_le_bytes()); // icon size: 0x40

    aset_fixture[0x18..0x20].copy_from_slice(&0x78u64.to_le_bytes()); // nacp off: 0x78
    aset_fixture[0x20..0x28].copy_from_slice(&0x40u64.to_le_bytes()); // nacp size: 0x40

    aset_fixture[0x28..0x30].copy_from_slice(&0xB8u64.to_le_bytes()); // romfs off: 0xB8
    aset_fixture[0x30..0x38].copy_from_slice(&0x40u64.to_le_bytes()); // romfs size: 0x40

    let assets = wl_nro::NroAssets::parse(&aset_fixture).expect("ASET parse must succeed");
    assert_eq!(assets.header.magic, MAGIC_ASET);
    assert_eq!(assets.header.version, 0);
    assert_eq!(assets.header.icon.offset, 0x38);
    assert_eq!(assets.header.icon.size, 0x40);
    assert_eq!(assets.header.nacp.offset, 0x78);
    assert_eq!(assets.header.nacp.size, 0x40);
    assert_eq!(assets.header.romfs.offset, 0xB8);
    assert_eq!(assets.header.romfs.size, 0x40);
    assert_eq!(assets.icon_bytes.unwrap().len(), 0x40);
    assert_eq!(assets.nacp_bytes.unwrap().len(), 0x40);
    assert_eq!(assets.romfs_bytes.unwrap().len(), 0x40);
}
