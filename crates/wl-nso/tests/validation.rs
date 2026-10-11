use sha2::{Digest, Sha256};
use wl_nso::Module;
fn put(b: &mut [u8], at: usize, v: u32) {
    b[at..at + 4].copy_from_slice(&v.to_le_bytes());
}
fn fixture(compressed: bool) -> Vec<u8> {
    let mut b = vec![0; 0x100];
    b[..4].copy_from_slice(b"NSO0");
    put(&mut b, 0xc, 0x38 | if compressed { 7 } else { 0 });
    put(&mut b, 0x3c, 32);
    for (i, data) in [
        0xd65f03c0u32.to_le_bytes().to_vec(),
        vec![42; 64],
        vec![17; 16],
    ]
    .iter()
    .enumerate()
    {
        let stored = if compressed {
            lz4_flex::block::compress(data)
        } else {
            data.clone()
        };
        let offset = b.len() as u32;
        put(&mut b, 0x10 + i * 16, offset);
        put(&mut b, 0x14 + i * 16, (i as u32) * 0x1000);
        put(&mut b, 0x18 + i * 16, data.len() as u32);
        put(&mut b, 0x60 + i * 4, stored.len() as u32);
        b[0xa0 + i * 32..0xc0 + i * 32].copy_from_slice(&Sha256::digest(data));
        b.extend(stored);
    }
    b
}
#[test]
fn reconstructs_segments_and_bss() {
    for compressed in [false, true] {
        let m = Module::parse(&fixture(compressed)).unwrap();
        assert_eq!(m.segments[0].bytes, 0xd65f03c0u32.to_le_bytes());
        assert_eq!(m.segments[1].bytes, vec![42; 64]);
        assert_eq!(m.bss_offset.0, 0x2010);
        assert_eq!(m.bss_size, 32);
        assert!(m.segments.iter().all(|s| s.hash_verified));
        assert_eq!(m.segments[0].compressed, compressed);
    }
}
#[test]
fn rejects_every_truncation() {
    let b = fixture(true);
    for end in 0..b.len() {
        assert!(Module::parse(&b[..end]).is_err(), "accepted length {end}");
    }
}
#[test]
fn validates_magic_version_and_flags() {
    for (at, value) in [(0, 0), (4, 1), (0xc, 0x80), (0xc, 0x100)] {
        let mut b = fixture(false);
        put(&mut b, at, value);
        assert!(Module::parse(&b).is_err());
    }
}
#[test]
fn execute_only_is_preserved() {
    let mut b = fixture(false);
    put(&mut b, 0xc, 0x78);
    let m = Module::parse(&b).unwrap();
    assert!(!m.segments[0].readable);
    assert!(m.segments[0].executable);
}
#[test]
fn rejects_file_and_memory_overlap() {
    for (at, value) in [(0x20, 0x100), (0x24, 0), (0x34, 0xff0)] {
        let mut b = fixture(false);
        put(&mut b, at, value);
        assert!(Module::parse(&b).is_err());
    }
}
#[test]
fn rejects_bss_overlap() {
    let mut b = fixture(false);
    put(&mut b, 0x24, 0x2010);
    assert!(Module::parse(&b).unwrap_err().contains("BSS overlaps"));
}
#[test]
fn rejects_large_and_overflowing_ranges() {
    for (at, value) in [
        (0x14, u32::MAX - 3),
        (0x18, 0x40000000),
        (0x10, u32::MAX),
        (0x3c, u32::MAX),
    ] {
        let mut b = fixture(false);
        put(&mut b, at, value);
        assert!(Module::parse(&b).is_err());
    }
}
#[test]
fn rejects_hash_corruption() {
    let mut b = fixture(false);
    b[0x100] ^= 1;
    assert!(Module::parse(&b).unwrap_err().contains("hash mismatch"));
}
#[test]
fn rejects_bad_lz4_and_wrong_size() {
    let mut b = fixture(true);
    b[0x100] = 0xff;
    assert!(Module::parse(&b).is_err());
    let mut b = fixture(true);
    put(&mut b, 0x18, 8);
    assert!(Module::parse(&b).unwrap_err().contains("decompressed size"));
}
#[test]
fn metadata_ranges_are_checked() {
    for at in [0x88, 0x90, 0x98] {
        let mut b = fixture(false);
        put(&mut b, at, 63);
        put(&mut b, at + 4, 2);
        assert!(Module::parse(&b).is_err());
    }
}
#[test]
fn checks_module_name_and_instruction_alignment() {
    for (at, value) in [(0x2c, 1), (0x14, 1), (0x18, 3)] {
        let mut b = fixture(false);
        put(&mut b, at, value);
        assert!(Module::parse(&b).is_err());
    }
}
fn mod_fixture() -> Vec<u8> {
    let mut b = fixture(false);
    put(&mut b, 0xc, 0);
    put(&mut b, 0x18, 8); // text grows into otherwise unused bytes below (move ro file data)
    let ro = 0x104usize; // parser reads text word 1 here; place MOD0 pointer while relocating ro storage
    let old = b[ro..ro + 64].to_vec();
    let new = b.len();
    b.extend(old);
    put(&mut b, 0x20, new as u32);
    put(&mut b, 0x104, 0x1000);
    b[new..new + 4].copy_from_slice(b"MOD0");
    put(&mut b, new + 4, 0x20);
    put(&mut b, new + 8, 0x1010);
    put(&mut b, new + 12, 0x1030);
    b[new + 0x20..new + 64].fill(0);
    b
}
#[test]
fn reads_mod0_dynamic_tags() {
    let b = mod_fixture();
    let m = Module::parse(&b).unwrap();
    let h = m.mod0.unwrap();
    assert_eq!(h.offset.0, 0x1000);
    assert_eq!(h.dynamic_offset.0, 0x1020);
    assert_eq!(h.bss_start.0, 0x2010);
}
#[test]
fn rejects_unmapped_dynamic_table() {
    let mut b = mod_fixture();
    let offset = u32::from_le_bytes(b[0x20..0x24].try_into().unwrap()) as usize;
    put(&mut b, offset + 4, 0x100000);
    assert!(Module::parse(&b).is_err());
}
