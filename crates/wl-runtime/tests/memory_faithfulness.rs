use std::collections::BTreeSet;
use wl_runtime::Memory;

#[test]
fn scalar_vector_and_borrowed_reads_observe_identical_guest_bytes() {
    // These numeric addresses exercised former address-dependent substitutions.
    // No executable, process identity, game data or kernel is involved.
    let addresses = [
        0xb906d98,
        0xbad2de8,
        0xbad2df0,
        0xbad2e18,
        0xbad2b78,
        0xbad2b80,
        0x208d900318,
        0x208e000000,
        0x208e000038,
        0x208e000040,
        0x208e0000c0,
        0x208e0000cc,
        0x208e000200,
        0x208e0002a8,
        0x208e001000,
        0x208e002000,
        0x208e011020,
        0x208e011050,
        0x208e012000,
        0x208e012068,
        0x208e905b88,
        0x208e907e38,
        0x208e908468,
        0x208e913368,
        0x2098000000,
        0x20a8000000,
        0x20e8000000,
    ];
    let mut memory = Memory::default();
    for page in addresses
        .iter()
        .map(|a| a & !0xfff)
        .collect::<BTreeSet<_>>()
    {
        memory.map(page, vec![0; 4096], true, true, false).unwrap();
    }
    for address in addresses {
        for bits in [
            0u64,
            1,
            u64::MAX,
            0x1122_3344_5566_7788,
            0x8000_0000_0000_0000,
        ] {
            memory.store(address, 8, bits).unwrap();
            let raw = bits.to_le_bytes();
            assert_eq!(memory.read_bytes(address, 8).unwrap(), raw);
            let mut copied = [0xff; 8];
            memory.read_into(address, &mut copied).unwrap();
            assert_eq!(copied, raw);
            for width in [1u8, 2, 4, 8] {
                let mask = u64::MAX >> ((8 - width) * 8);
                let expected = bits & mask;
                assert_eq!(
                    memory.load(address, width).unwrap(),
                    expected,
                    "{address:#x}/{width}"
                );
                assert_eq!(
                    memory.load_vector(address, width).unwrap(),
                    u128::from(expected)
                );
            }
        }
    }
}

#[test]
fn unmapped_writes_do_not_create_guest_pages_or_mutate_existing_pages() {
    let mut memory = Memory::default();
    memory
        .map(0x3000_001000, vec![0x7b; 4096], true, true, false)
        .unwrap();
    let before = memory.regions[0].bytes.clone();
    for address in [0x3000_000000, 0x3000_002000, 0x3400_000000, u64::MAX] {
        assert!(memory.validate_write(address, 8).is_err());
        assert!(memory.write_bytes(address, &[1; 8]).is_err());
        assert!(memory.store(address, 8, 1).is_err());
        assert_eq!(memory.regions.len(), 1);
        assert_eq!(memory.regions[0].bytes, before);
    }
}

#[test]
fn spanning_reads_and_writes_preserve_little_endian_order() {
    let mut memory = Memory::default();
    memory
        .map(0x1000, vec![0x11; 5], true, true, false)
        .unwrap();
    memory
        .map(0x1005, vec![0x22; 11], true, true, false)
        .unwrap();
    let bytes = [0x80, 0x01, 0x23, 0x45, 0x67, 0x89, 0xab, 0xcd];
    memory.validate_write(0x1002, bytes.len()).unwrap();
    memory.write_bytes(0x1002, &bytes).unwrap();
    assert!(memory.read_bytes(0x1002, 8).is_err()); // A borrow is never a hidden gather.
    assert_eq!(memory.load(0x1002, 8).unwrap(), u64::from_le_bytes(bytes));
    assert_eq!(
        memory.load_vector(0x1002, 8).unwrap(),
        u128::from(u64::from_le_bytes(bytes))
    );
    let mut out = [0; 8];
    memory.read_into(0x1002, &mut out).unwrap();
    assert_eq!(out, bytes);
    assert_eq!(memory.load(0x1000, 2).unwrap(), 0x1111);
    assert_eq!(memory.load(0x100a, 2).unwrap(), 0x2222);
}

#[test]
fn complete_preflight_prevents_partial_writes_and_reads() {
    let mut memory = Memory::default();
    memory
        .map(0x1000, vec![0xa5; 4], true, true, false)
        .unwrap();
    memory
        .map(0x1004, vec![0x5a; 4], false, false, false)
        .unwrap();
    assert!(memory.validate_write(0x1002, 4).is_err());
    assert!(memory.write_bytes(0x1002, &[1, 2, 3, 4]).is_err());
    assert_eq!(memory.regions[0].bytes, [0xa5; 4]);
    assert_eq!(memory.regions[1].bytes, [0x5a; 4]);
    let mut out = [0x33; 4];
    assert!(memory.read_into(0x1002, &mut out).is_err());
    assert_eq!(out, [0x33; 4]);
    memory.regions[1].read = true;
    memory.regions[1].write = true;
    memory.validate_write(0x1002, 4).unwrap();
    memory.write_bytes(0x1002, &[1, 2, 3, 4]).unwrap();
    assert_eq!(memory.regions[0].bytes, [0xa5, 0xa5, 1, 2]);
    assert_eq!(memory.regions[1].bytes, [3, 4, 0x5a, 0x5a]);
}

#[test]
fn empty_ranges_are_non_dereferencing_and_invalid_widths_are_errors() {
    let mut memory = Memory::default();
    for address in [0, 0x3000_000000, u64::MAX] {
        assert_eq!(memory.read_bytes(address, 0).unwrap(), &[]);
        memory.read_into(address, &mut []).unwrap();
        memory.validate_write(address, 0).unwrap();
        memory.write_bytes(address, &[]).unwrap();
    }
    assert!(memory.regions.is_empty());
    for width in [0, 3, 5, 7, 9, 16, 255] {
        assert!(memory.load(0, width).is_err());
        assert!(memory.store(0, width, 0).is_err());
    }
    for width in [0, 17, 255] {
        assert!(memory.load_vector(0, width).is_err());
        assert!(memory.store_vector(0, width, 0).is_err());
    }
}

#[test]
fn overflow_gap_and_permissions_fail_without_guest_side_effects() {
    let mut memory = Memory::default();
    memory
        .map(u64::MAX - 15, vec![0x7a; 15], true, true, false)
        .unwrap();
    let before = memory.regions[0].bytes.clone();
    assert!(memory.validate_write(u64::MAX - 3, 8).is_err());
    assert!(memory.write_bytes(u64::MAX - 3, &[0; 8]).is_err());
    assert!(memory.load(u64::MAX - 3, 8).is_err());
    assert_eq!(memory.regions[0].bytes, before);
    let mut out = [0xa5; 8];
    assert!(memory.read_into(u64::MAX - 3, &mut out).is_err());
    assert_eq!(out, [0xa5; 8]);
    memory
        .map(0x2000, vec![0x55; 4], true, true, false)
        .unwrap();
    memory
        .map(0x2008, vec![0xaa; 4], true, true, false)
        .unwrap();
    assert!(memory.validate_write(0x2002, 8).is_err());
    assert!(memory.write_bytes(0x2002, &[1; 8]).is_err());
    assert_eq!(memory.read_bytes(0x2000, 4).unwrap(), [0x55; 4]);
    assert_eq!(memory.read_bytes(0x2008, 4).unwrap(), [0xaa; 4]);
}

#[test]
fn read_and_write_permissions_are_validated_independently() {
    let mut memory = Memory::default();
    memory
        .map(0x1000, vec![0x12; 16], true, false, false)
        .unwrap();
    memory
        .map(0x2000, vec![0x34; 16], false, true, false)
        .unwrap();
    assert_eq!(memory.load(0x1000, 8).unwrap(), 0x1212_1212_1212_1212);
    assert!(memory.validate_write(0x1000, 8).is_err());
    assert!(memory.store(0x1000, 8, 0).is_err());
    assert_eq!(memory.read_bytes(0x1000, 16).unwrap(), [0x12; 16]);
    memory.validate_write(0x2000, 8).unwrap();
    memory.store(0x2000, 8, 0x8877_6655_4433_2211).unwrap();
    assert_eq!(
        &memory.regions[1].bytes[..8],
        &0x8877_6655_4433_2211u64.to_le_bytes()
    );
    assert!(memory.load(0x2000, 8).is_err());
    assert!(memory.read_bytes(0x2000, 8).is_err());
    let mut out = [0; 8];
    assert!(memory.read_into(0x2000, &mut out).is_err());
}

#[test]
fn every_vector_width_reads_and_stores_only_its_low_bytes() {
    let bits = 0xfedc_ba98_7654_3210_0123_4567_89ab_cdefu128;
    let mut memory = Memory::default();
    memory
        .map(0x1000, vec![0x5a; 32], true, true, false)
        .unwrap();
    for width in 1..=16u8 {
        memory.write_bytes(0x1000, &[0x5a; 32]).unwrap();
        memory.store_vector(0x1004, width, bits).unwrap();
        let mut expected = [0u8; 16];
        expected[..usize::from(width)].copy_from_slice(&bits.to_le_bytes()[..usize::from(width)]);
        assert_eq!(
            memory.load_vector(0x1004, width).unwrap(),
            u128::from_le_bytes(expected)
        );
        assert_eq!(memory.read_bytes(0x1000, 4).unwrap(), [0x5a; 4]);
        assert!(
            memory
                .read_bytes(0x1004 + u64::from(width), 28 - usize::from(width))
                .unwrap()
                .iter()
                .all(|b| *b == 0x5a)
        );
    }
    assert_eq!(memory.load128(0x1004).unwrap(), bits);
}
