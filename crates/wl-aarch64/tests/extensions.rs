use wl_aarch64::{
    CounterRegister, Crc32Op, CryptoOp, ExtensionOp, Reg, SystemCounterOp, decode_crc32,
    decode_crypto, decode_extension, decode_system_counter,
};
use wl_common::GuestAddr;

const PC: GuestAddr = GuestAddr(0x800_0000);

#[test]
fn test_system_counter_mrs_decoding() {
    // mrs x0, cntpct_el0 => 0xd53be020
    let op = decode_system_counter(0xd53be020, PC).unwrap();
    assert_eq!(
        op,
        Some(SystemCounterOp {
            read: true,
            register: CounterRegister::CntpctEl0,
            reg: Reg::Gpr(0),
        })
    );

    // mrs x8, cntpct_el0 => 0xd53be028
    let op = decode_system_counter(0xd53be028, PC).unwrap();
    assert_eq!(
        op,
        Some(SystemCounterOp {
            read: true,
            register: CounterRegister::CntpctEl0,
            reg: Reg::Gpr(8),
        })
    );

    // mrs x20, cntpct_el0 => 0xd53be034
    let op = decode_system_counter(0xd53be034, PC).unwrap();
    assert_eq!(
        op,
        Some(SystemCounterOp {
            read: true,
            register: CounterRegister::CntpctEl0,
            reg: Reg::Gpr(20),
        })
    );

    // mrs x1, cntvct_el0 => 0xd53be041
    let op = decode_system_counter(0xd53be041, PC).unwrap();
    assert_eq!(
        op,
        Some(SystemCounterOp {
            read: true,
            register: CounterRegister::CntvctEl0,
            reg: Reg::Gpr(1),
        })
    );

    // mrs x2, cntfrq_el0 => 0xd53be002
    let op = decode_system_counter(0xd53be002, PC).unwrap();
    assert_eq!(
        op,
        Some(SystemCounterOp {
            read: true,
            register: CounterRegister::CntfrqEl0,
            reg: Reg::Gpr(2),
        })
    );
}

#[test]
fn test_system_counter_msr_rejected_at_el0() {
    // msr cntpct_el0, x0 => bit 21 is 0 (0xd51be020)
    let res = decode_system_counter(0xd51be020, PC);
    assert!(res.is_err(), "EL0 write to cntpct_el0 must be rejected");

    // msr cntvct_el0, x0 => 0xd51be040
    let res = decode_system_counter(0xd51be040, PC);
    assert!(res.is_err(), "EL0 write to cntvct_el0 must be rejected");

    // msr cntfrq_el0, x0 => 0xd51be000
    let res = decode_system_counter(0xd51be000, PC);
    assert!(res.is_err(), "EL0 write to cntfrq_el0 must be rejected");
}

#[test]
fn test_crc32_and_crc32c_decoding() {
    // crc32b w8, w8, w11 (from TOTK sdk: 0x1acb4108)
    // Rd = 8, Rn = 8, Rm = 11, sz = 0, C = 0
    let op = decode_crc32(0x1acb4108, PC).unwrap();
    assert_eq!(
        op,
        Some(Crc32Op {
            dst: Reg::Gpr(8),
            acc: Reg::Gpr(8),
            val: Reg::Gpr(11),
            sz: 0,
            c: false,
        })
    );

    // crc32h w0, w1, w2 (0x1ac24420)
    // Rd = 0, Rn = 1, Rm = 2, sz = 1, C = 0
    let op = decode_crc32(0x1ac24420, PC).unwrap();
    assert_eq!(
        op,
        Some(Crc32Op {
            dst: Reg::Gpr(0),
            acc: Reg::Gpr(1),
            val: Reg::Gpr(2),
            sz: 1,
            c: false,
        })
    );

    // crc32w w0, w1, w2 (0x1ac24820)
    // Rd = 0, Rn = 1, Rm = 2, sz = 2, C = 0
    let op = decode_crc32(0x1ac24820, PC).unwrap();
    assert_eq!(
        op,
        Some(Crc32Op {
            dst: Reg::Gpr(0),
            acc: Reg::Gpr(1),
            val: Reg::Gpr(2),
            sz: 2,
            c: false,
        })
    );

    // crc32x w8, w8, x4 (from TOTK sdk: 0x9ac44d08)
    // Rd = 8, Rn = 8, Rm = 4, sz = 3, C = 0
    let op = decode_crc32(0x9ac44d08, PC).unwrap();
    assert_eq!(
        op,
        Some(Crc32Op {
            dst: Reg::Gpr(8),
            acc: Reg::Gpr(8),
            val: Reg::Gpr(4),
            sz: 3,
            c: false,
        })
    );

    // crc32cb w8, w8, w11 (from TOTK sdk: 0x1acb5108)
    // Rd = 8, Rn = 8, Rm = 11, sz = 0, C = 1
    let op = decode_crc32(0x1acb5108, PC).unwrap();
    assert_eq!(
        op,
        Some(Crc32Op {
            dst: Reg::Gpr(8),
            acc: Reg::Gpr(8),
            val: Reg::Gpr(11),
            sz: 0,
            c: true,
        })
    );

    // crc32ch w0, w1, w2 (0x1ac25420)
    // Rd = 0, Rn = 1, Rm = 2, sz = 1, C = 1
    let op = decode_crc32(0x1ac25420, PC).unwrap();
    assert_eq!(
        op,
        Some(Crc32Op {
            dst: Reg::Gpr(0),
            acc: Reg::Gpr(1),
            val: Reg::Gpr(2),
            sz: 1,
            c: true,
        })
    );

    // crc32cw w0, w1, w2 (0x1ac25820)
    // Rd = 0, Rn = 1, Rm = 2, sz = 2, C = 1
    let op = decode_crc32(0x1ac25820, PC).unwrap();
    assert_eq!(
        op,
        Some(Crc32Op {
            dst: Reg::Gpr(0),
            acc: Reg::Gpr(1),
            val: Reg::Gpr(2),
            sz: 2,
            c: true,
        })
    );

    // crc32cx w8, w8, x9 (from TOTK sdk: 0x9ac95d08)
    // Rd = 8, Rn = 8, Rm = 9, sz = 3, C = 1
    let op = decode_crc32(0x9ac95d08, PC).unwrap();
    assert_eq!(
        op,
        Some(Crc32Op {
            dst: Reg::Gpr(8),
            acc: Reg::Gpr(8),
            val: Reg::Gpr(9),
            sz: 3,
            c: true,
        })
    );
}

#[test]
fn test_crc32_reserved_encodings_rejected() {
    // sz = 3 (64-bit) with sf = 0 must be rejected
    // 0x1ac24c20 has sz = 3 but sf = 0
    let res = decode_crc32(0x1ac24c20, PC);
    assert!(res.is_err(), "sz=3 with sf=0 must be rejected");

    // sz = 0 (8-bit) with sf = 1 must be rejected
    // 0x9ac24020 has sz = 0 but sf = 1
    let res = decode_crc32(0x9ac24020, PC);
    assert!(res.is_err(), "sz=0 with sf=1 must be rejected");

    // sz = 1 (16-bit) with sf = 1 must be rejected
    let res = decode_crc32(0x9ac24420, PC);
    assert!(res.is_err(), "sz=1 with sf=1 must be rejected");

    // sz = 2 (32-bit) with sf = 1 must be rejected
    let res = decode_crc32(0x9ac24820, PC);
    assert!(res.is_err(), "sz=2 with sf=1 must be rejected");
}

#[test]
fn test_cryptographic_instructions_decoding() {
    // aese v0.16b, v1.16b => 0x4e284820
    let op = decode_crypto(0x4e284820, PC).unwrap();
    assert_eq!(op, Some(CryptoOp::Aese { dst: 0, src: 1 }));

    // aesd v0.16b, v1.16b => 0x4e285820
    let op = decode_crypto(0x4e285820, PC).unwrap();
    assert_eq!(op, Some(CryptoOp::Aesd { dst: 0, src: 1 }));

    // aesimc v1.16b, v1.16b => 0x4e287821
    let op = decode_crypto(0x4e287821, PC).unwrap();
    assert_eq!(op, Some(CryptoOp::Aesimc { dst: 1, src: 1 }));

    // sha1c q22, s0, v2.4s => 0x5e020016
    let op = decode_crypto(0x5e020016, PC).unwrap();
    assert_eq!(
        op,
        Some(CryptoOp::Sha1c {
            dst: 22,
            src1: 0,
            src2: 2,
        })
    );

    // sha256su0 v0.4s, v1.4s => 0x5e282820
    let op = decode_crypto(0x5e282820, PC).unwrap();
    assert_eq!(op, Some(CryptoOp::Sha256su0 { dst: 0, src: 1 }));

    // pmull v4.1q, v0.1d, v1.1d => 0x0ee1e004
    let op = decode_crypto(0x0ee1e004, PC).unwrap();
    assert_eq!(
        op,
        Some(CryptoOp::Pmull {
            dst: 4,
            lhs: 0,
            rhs: 1,
        })
    );
}

#[test]
fn test_unified_decode_extension() {
    // mrs x0, cntpct_el0
    let op = decode_extension(0xd53be020, PC).unwrap();
    assert!(matches!(op, Some(ExtensionOp::Counter(_))));

    // crc32b w8, w8, w11
    let op = decode_extension(0x1acb4108, PC).unwrap();
    assert!(matches!(op, Some(ExtensionOp::Crc(_))));

    // aese v0.16b, v1.16b
    let op = decode_extension(0x4e284820, PC).unwrap();
    assert!(matches!(op, Some(ExtensionOp::Crypto(_))));

    // NOP (not an extension instruction)
    let op = decode_extension(0xd503201f, PC).unwrap();
    assert_eq!(op, None);
}
