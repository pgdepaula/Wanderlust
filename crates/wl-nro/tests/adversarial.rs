use wl_nro::{MAGIC_MOD0, MAGIC_NRO, Nro};

fn make_valid_nro() -> Vec<u8> {
    let mut buf = vec![0u8; 0x3000];
    buf[0x00..0x04].copy_from_slice(&0x14000020u32.to_le_bytes());
    buf[0x04..0x08].copy_from_slice(&0x80u32.to_le_bytes());
    buf[0x10..0x14].copy_from_slice(&MAGIC_NRO);
    buf[0x14..0x18].copy_from_slice(&0u32.to_le_bytes());
    buf[0x18..0x1C].copy_from_slice(&0x3000u32.to_le_bytes());
    buf[0x20..0x24].copy_from_slice(&0u32.to_le_bytes());
    buf[0x24..0x28].copy_from_slice(&0x1000u32.to_le_bytes());
    buf[0x28..0x2C].copy_from_slice(&0x1000u32.to_le_bytes());
    buf[0x2C..0x30].copy_from_slice(&0x1000u32.to_le_bytes());
    buf[0x30..0x34].copy_from_slice(&0x2000u32.to_le_bytes());
    buf[0x34..0x38].copy_from_slice(&0x1000u32.to_le_bytes());
    buf[0x38..0x3C].copy_from_slice(&0x1000u32.to_le_bytes());
    buf[0x80..0x84].copy_from_slice(&MAGIC_MOD0);
    buf
}

#[test]
fn test_all_truncation_lengths_never_panic() {
    let base = make_valid_nro();
    for len in 0..=base.len() {
        let truncated = &base[..len];
        let _ = Nro::parse(truncated);
    }
}

#[test]
fn test_boundary_integer_mutations_never_panic() {
    let base = make_valid_nro();
    let mutation_values = [
        0u32,
        1,
        2,
        3,
        4,
        0x10,
        0x7F,
        0x80,
        0x100,
        0x1000,
        0x2000,
        0x3000,
        0x7FFF_FFFF,
        0x8000_0000,
        0xFFFF_FFFE,
        u32::MAX,
    ];

    let test_offsets = [
        0x04, // mod0_offset
        0x14, // version
        0x18, // size
        0x1C, // flags
        0x20, // text offset
        0x24, // text size
        0x28, // ro offset
        0x2C, // ro size
        0x30, // data offset
        0x34, // data size
        0x38, // bss size
        0x60, // dso handle
        0x68, // embedded offset
        0x6C, // embedded size
    ];

    for &off in &test_offsets {
        for &val in &mutation_values {
            let mut corrupted = base.clone();
            corrupted[off..off + 4].copy_from_slice(&val.to_le_bytes());
            let _ = Nro::parse(&corrupted);
        }
    }
}

#[test]
fn test_adversarial_trailer_mutations_never_panic() {
    let mut base = make_valid_nro();
    let trailer_start = base.len();
    let mut trailer = vec![0u8; 0x500];
    trailer[0x00..0x04].copy_from_slice(b"ASET");
    base.extend_from_slice(&trailer);

    let u64_mutations = [
        0u64,
        1,
        0x37,
        0x38,
        0x100,
        0x500,
        0x1000,
        0x7FFF_FFFF_FFFF_FFFF,
        0x8000_0000_0000_0000,
        u64::MAX - 1,
        u64::MAX,
    ];

    let asset_offsets = [0x08, 0x10, 0x18, 0x20, 0x28, 0x30];

    for &off in &asset_offsets {
        for &val in &u64_mutations {
            let mut corrupted = base.clone();
            let target = trailer_start + off;
            corrupted[target..target + 8].copy_from_slice(&val.to_le_bytes());
            let _ = Nro::parse(&corrupted);
        }
    }
}

#[test]
fn test_bit_flip_fuzzing_never_panic() {
    let base = make_valid_nro();
    let mut lcg = 0x12345678u64;

    for _ in 0..1000 {
        let mut corrupted = base.clone();
        for _ in 0..5 {
            lcg = lcg.wrapping_mul(6364136223846793005).wrapping_add(1);
            let byte_idx = (lcg as usize) % corrupted.len();
            lcg = lcg.wrapping_mul(6364136223846793005).wrapping_add(1);
            let bit_mask = 1u8 << ((lcg as usize) % 8);
            corrupted[byte_idx] ^= bit_mask;
        }
        let _ = Nro::parse(&corrupted);
    }
}
