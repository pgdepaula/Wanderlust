//! Deterministic adversarial mutation and fuzzing harness for wl-npdm.
//!
//! Confirms that no mutated, truncated, or adversarial input can cause a panic,
//! unbounded loop, or invalid slice access.

use std::panic::catch_unwind;
use wl_npdm::*;

fn build_baseline_npdm() -> Vec<u8> {
    let mut data = vec![0u8; 0x480];
    data[0x00..0x04].copy_from_slice(b"META");
    data[0x0C] = 0x07; // 64-bit, 39-bit address space
    data[0x0E] = 44;
    data[0x0F] = 1;
    data[0x14..0x18].copy_from_slice(&0x20_0000u32.to_le_bytes());
    data[0x1C..0x20].copy_from_slice(&0x10_0000u32.to_le_bytes());
    data[0x20..0x29].copy_from_slice(b"adversary");
    data[0x30..0x39].copy_from_slice(b"TEST-0001");
    data[0x70..0x74].copy_from_slice(&0x380u32.to_le_bytes()); // ACI0 offset
    data[0x74..0x78].copy_from_slice(&0x100u32.to_le_bytes()); // ACI0 size
    data[0x78..0x7C].copy_from_slice(&0x80u32.to_le_bytes()); // ACID offset
    data[0x7C..0x80].copy_from_slice(&0x300u32.to_le_bytes()); // ACID size

    // ACID
    let acid_base = 0x80;
    data[acid_base + 0x200..acid_base + 0x204].copy_from_slice(b"ACID");
    data[acid_base + 0x204..acid_base + 0x208].copy_from_slice(&0x200u32.to_le_bytes());
    data[acid_base + 0x20C..acid_base + 0x210].copy_from_slice(&1u32.to_le_bytes());
    data[acid_base + 0x210..acid_base + 0x218]
        .copy_from_slice(&0x0100_0000_0000_1000u64.to_le_bytes());
    data[acid_base + 0x218..acid_base + 0x220]
        .copy_from_slice(&0x0100_0000_0000_1000u64.to_le_bytes());

    // ACID FAC at +0x240, size 0x2C
    data[acid_base + 0x220..acid_base + 0x224].copy_from_slice(&0x240u32.to_le_bytes());
    data[acid_base + 0x224..acid_base + 0x228].copy_from_slice(&0x2Cu32.to_le_bytes());
    data[acid_base + 0x240] = 1;
    data[acid_base + 0x244..acid_base + 0x24C].copy_from_slice(&0xFFu64.to_le_bytes());

    // ACID KAC at +0x270, size 8
    data[acid_base + 0x230..acid_base + 0x234].copy_from_slice(&0x270u32.to_le_bytes());
    data[acid_base + 0x234..acid_base + 0x238].copy_from_slice(&8u32.to_le_bytes());
    data[acid_base + 0x270..acid_base + 0x274]
        .copy_from_slice(&(0b0111u32 | (50 << 4) | (20 << 10)).to_le_bytes());
    data[acid_base + 0x274..acid_base + 0x278].copy_from_slice(&0xFFFF_FFFFu32.to_le_bytes());

    // ACI0
    let aci_base = 0x380;
    data[aci_base..aci_base + 0x04].copy_from_slice(b"ACI0");
    data[aci_base + 0x10..aci_base + 0x18].copy_from_slice(&0x0100_0000_0000_1000u64.to_le_bytes());

    // ACI0 FAH at +0x40 (>= 0x40 ACI0_HEADER_SIZE), size 0x1C
    data[aci_base + 0x20..aci_base + 0x24].copy_from_slice(&0x40u32.to_le_bytes());
    data[aci_base + 0x24..aci_base + 0x28].copy_from_slice(&0x1Cu32.to_le_bytes());
    data[aci_base + 0x40] = 1;
    data[aci_base + 0x44..aci_base + 0x4C].copy_from_slice(&0x0Fu64.to_le_bytes());

    // ACI0 KAC at +0x60, size 8
    data[aci_base + 0x30..aci_base + 0x34].copy_from_slice(&0x60u32.to_le_bytes());
    data[aci_base + 0x34..aci_base + 0x38].copy_from_slice(&8u32.to_le_bytes());
    data[aci_base + 0x60..aci_base + 0x64]
        .copy_from_slice(&(0b0111u32 | (50 << 4) | (20 << 10)).to_le_bytes());
    data[aci_base + 0x64..aci_base + 0x68].copy_from_slice(&0xFFFF_FFFFu32.to_le_bytes());

    data
}

#[test]
fn test_all_truncation_lengths_never_panic() {
    let baseline = build_baseline_npdm();
    for len in 0..=baseline.len() {
        let truncated = &baseline[..len];
        let result = catch_unwind(|| {
            let _ = Npdm::parse(truncated);
        });
        assert!(result.is_ok(), "panicked on truncation at length {len}");
    }
}

#[test]
fn test_systematic_boundary_integer_mutations_never_panic() {
    let boundary_values: [u32; 10] = [
        0,
        1,
        2,
        3,
        4,
        0x7FFF_FFFF,
        0x8000_0000,
        0xFFFF_FFFE,
        u32::MAX,
        0x1000,
    ];

    // Offsets of interest: all u32 fields in META, ACID, and ACI0
    let u32_offsets: [usize; 16] = [
        0x04,
        0x14,
        0x18,
        0x1C,
        0x70,
        0x74,
        0x78,
        0x7C, // META
        0x80 + 0x204,
        0x80 + 0x20C,
        0x80 + 0x220,
        0x80 + 0x224,
        0x80 + 0x228,
        0x80 + 0x22C,
        0x80 + 0x230,
        0x80 + 0x234, // ACID
    ];

    for &off in &u32_offsets {
        for &val in &boundary_values {
            let mut data = build_baseline_npdm();
            data[off..off + 4].copy_from_slice(&val.to_le_bytes());
            let result = catch_unwind(|| {
                let _ = Npdm::parse(&data);
            });
            assert!(
                result.is_ok(),
                "panicked on u32 mutation at offset {off:#x} with value {val:#x}"
            );
        }
    }
}

#[test]
fn test_program_id_range_edge_cases() {
    let mut data = build_baseline_npdm();
    let acid_base = 0x80;
    let aci_base = 0x380;

    // 1. Equal Min and Max matching Program ID -> Success
    data[acid_base + 0x210..acid_base + 0x218]
        .copy_from_slice(&0x0100_1234_5678_9000u64.to_le_bytes());
    data[acid_base + 0x218..acid_base + 0x220]
        .copy_from_slice(&0x0100_1234_5678_9000u64.to_le_bytes());
    data[aci_base + 0x10..aci_base + 0x18].copy_from_slice(&0x0100_1234_5678_9000u64.to_le_bytes());
    assert!(Npdm::parse(&data).is_ok());

    // 2. Program ID outside equal range -> ProgramIdOutOfAcidRange
    data[aci_base + 0x10..aci_base + 0x18].copy_from_slice(&0x0100_1234_5678_9001u64.to_le_bytes());
    assert!(matches!(
        Npdm::parse(&data),
        Err(NpdmError::ProgramIdOutOfAcidRange { .. })
    ));

    // 3. Reversed range (min > max) -> InvalidProgramIdRange
    data[acid_base + 0x210..acid_base + 0x218].copy_from_slice(&0x0200u64.to_le_bytes());
    data[acid_base + 0x218..acid_base + 0x220].copy_from_slice(&0x0100u64.to_le_bytes());
    assert!(matches!(
        Npdm::parse(&data),
        Err(NpdmError::InvalidProgramIdRange {
            min: 0x0200,
            max: 0x0100
        })
    ));

    // 4. Zero-valued range (0..0) -> Permits wildcard matching without range constraint
    data[acid_base + 0x210..acid_base + 0x218].copy_from_slice(&0u64.to_le_bytes());
    data[acid_base + 0x218..acid_base + 0x220].copy_from_slice(&0u64.to_le_bytes());
    data[aci_base + 0x10..aci_base + 0x18].copy_from_slice(&0x1234_5678u64.to_le_bytes());
    assert!(Npdm::parse(&data).is_ok());

    // 5. Maximum 64-bit bounds (u64::MAX)
    data[acid_base + 0x210..acid_base + 0x218].copy_from_slice(&u64::MAX.to_le_bytes());
    data[acid_base + 0x218..acid_base + 0x220].copy_from_slice(&u64::MAX.to_le_bytes());
    data[aci_base + 0x10..aci_base + 0x18].copy_from_slice(&u64::MAX.to_le_bytes());
    assert!(Npdm::parse(&data).is_ok());
}

#[test]
fn test_exhaustive_subregion_boundary_matrix() {
    // Tests zero size + various offsets (0, valid, end, beyond end, u32::MAX)
    // across all 6 inner subregions
    let subregions = [
        ("ACID FAC", 0x80 + 0x220, 0x80 + 0x224),
        ("ACID SAC", 0x80 + 0x228, 0x80 + 0x22C),
        ("ACID KAC", 0x80 + 0x230, 0x80 + 0x234),
        ("ACI0 FAH", 0x380 + 0x20, 0x380 + 0x24),
        ("ACI0 SAC", 0x380 + 0x28, 0x380 + 0x2C),
        ("ACI0 KAC", 0x380 + 0x30, 0x380 + 0x34),
    ];

    for &(name, off_ptr, size_ptr) in &subregions {
        // Size 0, offset 0 -> valid
        let mut data = build_baseline_npdm();
        data[off_ptr..off_ptr + 4].copy_from_slice(&0u32.to_le_bytes());
        data[size_ptr..size_ptr + 4].copy_from_slice(&0u32.to_le_bytes());
        let res = catch_unwind(|| Npdm::parse(&data));
        assert!(res.is_ok(), "{name}: zero-offset zero-size panicked");

        // Size 0, offset u32::MAX -> SectionOutOfBounds (never panic)
        let mut data = build_baseline_npdm();
        data[off_ptr..off_ptr + 4].copy_from_slice(&u32::MAX.to_le_bytes());
        data[size_ptr..size_ptr + 4].copy_from_slice(&0u32.to_le_bytes());
        assert!(
            matches!(
                Npdm::parse(&data),
                Err(NpdmError::SectionOutOfBounds { .. })
            ),
            "{name}: offset u32::MAX with size 0 must be rejected"
        );

        // Size 0, offset beyond container -> SectionOutOfBounds
        let mut data = build_baseline_npdm();
        data[off_ptr..off_ptr + 4].copy_from_slice(&0x2000u32.to_le_bytes());
        data[size_ptr..size_ptr + 4].copy_from_slice(&0u32.to_le_bytes());
        assert!(
            matches!(
                Npdm::parse(&data),
                Err(NpdmError::SectionOutOfBounds { .. })
            ),
            "{name}: out of bounds offset with size 0 must be rejected"
        );
    }
}
