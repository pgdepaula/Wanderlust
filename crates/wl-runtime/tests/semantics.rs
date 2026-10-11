use wl_runtime::*;
#[test]
fn checked_endian_unaligned_and_widths() {
    let mut m = Memory::default();
    m.map(0x1000, vec![0; 32], true, true, false).unwrap();
    m.store(0x1001, 8, 0x8877665544332211).unwrap();
    assert_eq!(
        m.read_bytes(0x1001, 8).unwrap(),
        [0x11, 0x22, 0x33, 0x44, 0x55, 0x66, 0x77, 0x88]
    );
    assert_eq!(m.load(0x1003, 4).unwrap(), 0x66554433);
    m.store(0x1002, 1, 0x1ff).unwrap();
    assert_eq!(m.load(0x1002, 2).unwrap(), 0x33ff);
}
#[test]
fn bounds_overflow_and_overlap() {
    let mut m = Memory::default();
    m.map(8, vec![0; 8], true, true, false).unwrap();
    assert!(m.load(7, 1).is_err());
    assert!(m.load(15, 2).is_err());
    assert!(m.load(u64::MAX, 8).is_err());
    assert!(m.map(12, vec![0; 8], true, true, false).is_err());
    assert!(m.map(u64::MAX, vec![0; 8], true, true, false).is_err());
    assert!(m.load(8, 3).is_err());
}
#[test]
fn permissions_and_atomic_fault() {
    let mut m = Memory::default();
    m.map(0, vec![1; 8], true, false, true).unwrap();
    assert!(m.store(0, 1, 2).is_err());
    assert_eq!(m.load(0, 1).unwrap(), 1);
    assert!(m.check_execute(4).is_ok());
    assert!(m.check_execute(1).is_err());
    assert!(m.check_execute(8).is_err());
    m.map(8, vec![0; 8], false, true, false).unwrap();
    assert!(m.load(8, 1).is_err());
    m.store(8, 1, 2).unwrap();
}
#[test]
fn overlapping_copy_is_explicit() {
    let mut m = Memory::default();
    m.map(0, vec![1, 2, 3, 4, 5], true, true, false).unwrap();
    let copy = m.read_bytes(0, 4).unwrap().to_vec();
    m.write_bytes(1, &copy).unwrap();
    assert_eq!(m.read_bytes(0, 5).unwrap(), [1, 1, 2, 3, 4]);
}
#[test]
fn arithmetic_flag_edges() {
    let mut c = Cpu::new(0);
    for (a, b, r, bits, sub, expected) in [
        (u64::MAX, 1, 0, 64, false, 6),
        (0x7fffffffffffffff, 1, 0x8000000000000000, 64, false, 9),
        (0, 1, u64::MAX, 64, true, 8),
        (1, 1, 0, 64, true, 6),
        (0x80000000, 1, 0x7fffffff, 32, true, 3),
        (0xffffffff, 1, 0, 32, false, 6),
    ] {
        c.flags(a, b, r, bits, sub);
        assert_eq!(c.nzcv, expected);
    }
}
#[test]
fn all_conditions() {
    let mut c = Cpu::new(0);
    for flags in 0..16 {
        c.nzcv = flags;
        let n = flags & 8 != 0;
        let z = flags & 4 != 0;
        let carry = flags & 2 != 0;
        let v = flags & 1 != 0;
        let expect = [
            z,
            !z,
            carry,
            !carry,
            n,
            !n,
            v,
            !v,
            carry && !z,
            !carry || z,
            n == v,
            n != v,
            !z && n == v,
            z || n != v,
            true,
            true,
        ];
        for (cond, value) in expect.into_iter().enumerate() {
            assert_eq!(c.condition(cond as u8), value, "flags={flags} cond={cond}");
        }
    }
}
#[test]
fn shifts_and_rotates() {
    assert_eq!(shift(0x80000000, 2, 31, 32), 0xffffffff);
    assert_eq!(shift(0x80000000, 1, 31, 32), 1);
    assert_eq!(shift(1, 3, 1, 32), 0x80000000);
    assert_eq!(shift(u64::MAX, 0, 1, 32), 0xfffffffe);
}
#[test]
fn budget_and_services() {
    let mut c = Cpu::new(4);
    c.limit = 1;
    c.tick(4).unwrap();
    assert!(c.tick(8).unwrap_err().contains("budget"));
    let old = c.x;
    // Harness mode (no Horizon kernel): only ExitProcess is defined.
    let mut p = Process {
        memory: Memory::default(),
        kernel: None,
    };
    assert!(
        service(&mut c, &mut p, 1)
            .unwrap_err()
            .contains("guest_pc=0x8")
    );
    assert_eq!(c.x, old);
    assert_eq!(service(&mut c, &mut p, 7).unwrap(), Some("exit_process"));
}
