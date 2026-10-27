use wl_analysis::{Image, Mapping, unwind::function_index};
use wl_common::GuestAddr;

fn hex(s: &str) -> Vec<u8> {
    s.as_bytes()
        .chunks_exact(2)
        .map(|pair| u8::from_str_radix(std::str::from_utf8(pair).unwrap(), 16).unwrap())
        .collect()
}
fn fixture() -> Image {
    // GNU as/ld 2.46: two .cfi_startproc MOV/RET functions at 0x10000,
    // linker script places .eh_frame_hdr at 0x20000 and .eh_frame at 0x20020.
    let text = hex("e00080d2c0035fd6600180d2c0035fd6");
    let mut ro = hex("011b033b1c000000020000000000ffff340000000800ffff48000000");
    ro.resize(32, 0);
    ro.extend(hex("1000000000000000017a520004781e011b0c1f001000000018000000c4fffeff0800000000000000100000002c000000b8fffeff0800000000000000"));
    let mut image = Image::raw(text, GuestAddr(0x10000)).unwrap();
    image.mappings.push(Mapping {
        base: GuestAddr(0x20000),
        bytes: ro,
        read: true,
        write: false,
        execute: false,
    });
    image
}
fn put(image: &mut Image, offset: usize, value: u32) {
    image.mappings[1].bytes[offset..offset + 4].copy_from_slice(&value.to_le_bytes());
}
#[test]
fn independently_linked_function_index_matches_fde_locations_and_ranges() {
    let image = fixture();
    let original = image.mappings[1].bytes.clone();
    let index = function_index(&image, GuestAddr(0x20000), 28).unwrap();
    assert_eq!(index.frame, GuestAddr(0x20020));
    assert_eq!(
        index
            .functions
            .iter()
            .map(|f| (f.start.0, f.length, f.fde.0))
            .collect::<Vec<_>>(),
        [(0x10000, 8, 0x20034), (0x10008, 8, 0x20048)]
    );
    assert_eq!(image.mappings[1].bytes, original);
}
#[test]
fn unsupported_header_encodings_and_versions_are_explicit_errors() {
    for position in 0..4 {
        let mut image = fixture();
        image.mappings[1].bytes[position] = 0xff;
        assert!(function_index(&image, GuestAddr(0x20000), 28).is_err());
    }
}
#[test]
fn truncated_ranges_and_declared_counts_do_not_return_partial_roots() {
    for size in [0, 11, 20, 27, 29, u64::MAX] {
        assert!(function_index(&fixture(), GuestAddr(0x20000), size).is_err());
    }
    let mut image = fixture();
    image.mappings[1].bytes.truncate(27);
    assert!(function_index(&image, GuestAddr(0x20000), 28).is_err());
    let mut image = fixture();
    put(&mut image, 8, 1);
    assert!(function_index(&image, GuestAddr(0x20000), 28).is_err());
}
#[test]
fn excessive_count_fails_before_allocating_or_reading_entries() {
    let mut image = fixture();
    put(&mut image, 8, wl_analysis::unwind::MAX_FUNCTIONS as u32 + 1);
    assert!(
        function_index(&image, GuestAddr(0x20000), 28)
            .unwrap_err()
            .contains("budget")
    );
}
#[test]
fn fde_mismatch_bad_cie_reference_and_bad_extent_are_rejected() {
    for (offset, value) in [(60, 0), (56, 0), (64, u32::MAX), (64, 0)] {
        let mut image = fixture();
        put(&mut image, offset, value);
        assert!(function_index(&image, GuestAddr(0x20000), 28).is_err());
    }
}
#[test]
fn unsorted_locations_and_relative_address_underflow_are_rejected() {
    let mut image = fixture();
    put(&mut image, 12, 0x80000000);
    assert!(function_index(&image, GuestAddr(0x20000), 28).is_err());
    let mut image = fixture();
    let first = image.mappings[1].bytes[12..20].to_vec();
    let second = image.mappings[1].bytes[20..28].to_vec();
    image.mappings[1].bytes[12..20].copy_from_slice(&second);
    image.mappings[1].bytes[20..28].copy_from_slice(&first);
    assert!(function_index(&image, GuestAddr(0x20000), 28).is_err());
}
#[test]
fn mutable_metadata_and_nonexecutable_functions_do_not_establish_roots() {
    let mut image = fixture();
    image.mappings[1].write = true;
    assert!(function_index(&image, GuestAddr(0x20000), 28).is_err());
    let mut image = fixture();
    image.mappings[0].execute = false;
    assert!(function_index(&image, GuestAddr(0x20000), 28).is_err());
}
#[test]
fn unsupported_cie_version_and_corrupt_augmentation_are_rejected() {
    for (offset, value) in [(40, 3), (41, b'x'), (47, 127), (48, 0)] {
        let mut image = fixture();
        image.mappings[1].bytes[offset] = value;
        assert!(function_index(&image, GuestAddr(0x20000), 28).is_err());
    }
}
#[test]
fn empty_canonical_index_contains_no_function_claims() {
    let mut image = fixture();
    put(&mut image, 8, 0);
    let index = function_index(&image, GuestAddr(0x20000), 12).unwrap();
    assert!(index.functions.is_empty());
}

#[test]
fn gnu_personality_and_lsda_augmentations_do_not_require_executing_them() {
    // GNU as/ld 2.46: .cfi_personality 0x1c,personality; .cfi_lsda 0x1b,lsda.
    // Personality and LSDA payloads are skipped, never dereferenced or run.
    let mut image = fixture();
    let mut ro = hex("011b033b1c000000020000000000ffff400000000800ffff6c000000");
    ro.resize(32, 0);
    ro.extend(hex("1c00000000000000017a504c520004781e0b1cddfffeffffffffff1b1b0c1f001400000024000000b8fffeff08000000042f0000000000001000000000000000017a520004781e011b0c1f00100000001800000094fffeff0800000000000000"));
    image.mappings[1].bytes = ro;
    let index = function_index(&image, GuestAddr(0x20000), 28).unwrap();
    assert_eq!(
        index
            .functions
            .iter()
            .map(|f| f.start.0)
            .collect::<Vec<_>>(),
        [0x10000, 0x10008]
    );
    assert_eq!(index.functions[0].fde, GuestAddr(0x20040));
    assert_eq!(index.functions[1].fde, GuestAddr(0x2006c));
}
