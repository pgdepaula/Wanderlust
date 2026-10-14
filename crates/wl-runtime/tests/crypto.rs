//! CRC32/CRC32C against independent golden values (tools/isa/reference_semantics.py, checked
//! there against Unicorn and published constants) and the system counter contract. The
//! crypto-extension forms are compared with Unicorn by the differential suite (crypto_crc).
use wl_runtime::*;

#[test]
fn crc32_matches_reference_vectors() {
    let cases: [(u64, u64, u8, bool, u64); 10] = [
        (0, 0, 1, false, 0),
        (0, 0x41, 1, false, 0x01DB_7106),
        (0x1234_5678, 0xABCD, 2, false, 0x5ABE_C668),
        (0xFFFF_FFFF, 0x1234_5678, 4, false, 0x5092_782D),
        (0, 0x0123_4567_89AB_CDEF, 8, false, 0x2119_3D2E),
        (0, 0, 1, true, 0),
        (0, 0x41, 1, true, 0xB310_9EBF),
        (0x1234_5678, 0xABCD, 2, true, 0xAAE3_2043),
        (0xFFFF_FFFF, 0x1234_5678, 4, true, 0x4DEC_E20C),
        (0, 0x0123_4567_89AB_CDEF, 8, true, 0xE998_6AA9),
    ];
    for (acc, value, bytes, castagnoli, expected) in cases {
        assert_eq!(
            crc32(acc, value, bytes, castagnoli),
            expected,
            "{acc:#x} {value:#x} {bytes} {castagnoli}"
        );
    }
    // Only the low `bytes` bytes of the value and the low word of the accumulator matter.
    assert_eq!(crc32(0xffff_ffff_0000_0000, 0xff41, 1, false), 0x01DB_7106);
}

#[test]
fn aes_tables_match_fips_197() {
    // FIPS-197 S-box examples: S(0x00) = 0x63, S(0x53) = 0xed; AESE with a zero key
    // substitutes after ShiftRows, which leaves byte 0 in place.
    assert_eq!(aese(0, 0) as u8, 0x63);
    assert_eq!(aese(0x53, 0) as u8, 0xed);
    assert_eq!(aesd(aese(0x53, 0), 0) as u8, 0x53);
    // InvMixColumns of FIPS-197's MixColumns example column (db 13 53 45 -> 8e 4d a1 bc).
    let mixed = u128::from_le_bytes([0x8e, 0x4d, 0xa1, 0xbc, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0]);
    assert_eq!(aesimc(mixed).to_le_bytes()[..4], [0xdb, 0x13, 0x53, 0x45]);
    assert_eq!(pmull64(0b11, 0b11), 0b101);
    assert_eq!(pmull64(u64::MAX, 2), u128::from(u64::MAX) << 1);
}

#[test]
fn system_counter_follows_the_virtual_clock() {
    use wl_runtime::horizon::*;
    let mut m = Memory::default();
    m.map_state(0x800_0000, vec![0; 0x1000], true, false, true, state::CODE)
        .unwrap();
    let config = ProcessConfig {
        program_id: 1,
        process_id: 0x51,
        main_thread_id: 0x72,
        main_thread_stack_size: 0x1000,
        main_thread_priority: 44,
        ideal_core: 0,
        core_mask: 1,
        priority_mask: u64::MAX,
        memory_limit: 0x100_0000,
        handle_table_size: 0,
        thread_limit: 4,
        is_application: true,
        svc_permissions: [u64::MAX; 3],
        service_access: &[],
    };
    let (mut p, mut c) = Process::horizon(
        m,
        0x800_0000,
        Range {
            start: 0x800_0000,
            end: 0x800_1000,
        },
        config,
    )
    .unwrap();
    assert_eq!(COUNTER_FREQUENCY, 19_200_000);
    c.steps = 1_000_000; // 1 ms of virtual time in the current slice
    let counter = system_counter(&c, &p);
    assert_eq!(counter, 19_200);
    // svcGetSystemTick reports the same clock.
    assert_eq!(dispatch(&mut c, &mut p, 0x1E).unwrap(), None);
    assert_eq!(c.x[0], counter);
    // Harness mode: the retired-instruction count at one instruction per nanosecond.
    let harness = Process {
        memory: Memory::default(),
        kernel: None,
    };
    assert_eq!(system_counter(&c, &harness), 19_200);
}
