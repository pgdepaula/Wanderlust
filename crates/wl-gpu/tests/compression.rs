mod common;
use common::compressed::*;
use wl_gpu::{compression::decode_rgba8, format::Format};

#[test]
fn independent_astc_and_bc_reference_pixels() {
    assert_eq!(
        decode_rgba8(Format::Astc4x4, 4, 4, &ASTC_REFERENCE).unwrap(),
        ASTC_PIXELS
    );
    for (block, alpha) in [(BC1, false), (BC1_ALPHA, true)] {
        assert_eq!(
            decode_rgba8(Format::Bc1Rgba, 4, 4, &block).unwrap(),
            bc_pixels(alpha)
        );
    }
    let color = [23, 197, 41, 73];
    assert_eq!(
        decode_rgba8(Format::Astc4x4, 1, 1, &astc_constant(color)).unwrap(),
        color
    );
    for format in [Format::Astc4x4, Format::Bc1Rgba] {
        let (blocks, expected) = compressed_mip(format, 37, 35, 2);
        assert_eq!(decode_rgba8(format, 37, 35, &blocks).unwrap(), expected);
    }
}
#[test]
fn compressed_invalid_input_is_rejected() {
    for size in [0, 1, 15, 17] {
        assert!(decode_rgba8(Format::Astc4x4, 4, 4, &vec![0; size]).is_err());
    }
    assert!(decode_rgba8(Format::Astc4x4, 4, 4, &[0; 16]).is_err());
    assert!(decode_rgba8(Format::Bc1Rgba, 0, 4, &[]).is_err());
    assert!(decode_rgba8(Format::Bc1Rgba, u32::MAX, u32::MAX, &[]).is_err());
    let mut hdr = astc_constant([0; 4]);
    hdr[1] |= 2;
    assert!(decode_rgba8(Format::Astc4x4, 4, 4, &hdr).is_err());
    let mut reserved = astc_constant([0; 4]);
    reserved[1] &= !4;
    assert!(decode_rgba8(Format::Astc4x4, 4, 4, &reserved).is_err());
    let mut multi = ASTC_REFERENCE;
    multi[1] |= 8;
    assert!(decode_rgba8(Format::Astc4x4, 4, 4, &multi).is_err());
    let bits = (u128::from_le_bytes(ASTC_REFERENCE) & !(15 << 13)) | (15 << 13);
    assert!(decode_rgba8(Format::Astc4x4, 4, 4, &bits.to_le_bytes()).is_err());
    assert!(decode_rgba8(Format::Bc3Rgba, 4, 4, &[0; 16]).is_err());
}
