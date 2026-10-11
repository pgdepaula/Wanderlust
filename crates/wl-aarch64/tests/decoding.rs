use wl_aarch64::{Op, Operand, Reg, SystemRegister, Width, decode};
use wl_common::GuestAddr;
fn op(w: u32) -> Op {
    decode(w, GuestAddr(0x10000)).unwrap().op
}
#[test]
fn move_wide_fields() {
    assert!(matches!(
        op(0xd28000e0),
        Op::Wide {
            width: Width::X64,
            dst: Reg::Gpr(0),
            imm: 7,
            ..
        }
    ));
    assert!(matches!(
        op(0xf2e00020),
        Op::Wide {
            keep: true,
            shift: 48,
            imm: 0x1000000000000,
            ..
        }
    ));
}
#[test]
fn sp_and_zero_are_contextual() {
    assert!(matches!(
        op(0x910003ff),
        Op::Arithmetic {
            dst: Reg::Sp,
            lhs: Reg::Sp,
            ..
        }
    ));
    assert!(matches!(
        op(0xf10003ff),
        Op::Arithmetic {
            dst: Reg::Zero,
            lhs: Reg::Sp,
            flags: true,
            ..
        }
    ));
    assert!(matches!(
        op(0xaa1f03e0),
        Op::Logical {
            lhs: Reg::Zero,
            rhs: Operand::Shifted { reg: Reg::Zero, .. },
            ..
        }
    ));
}
#[test]
fn widths_and_shifts() {
    assert!(matches!(
        op(0x4b810000),
        Op::Arithmetic {
            width: Width::W32,
            sub: true,
            rhs: Operand::Shifted {
                kind: 2,
                amount: 0,
                ..
            },
            ..
        }
    ));
}
#[test]
fn backward_branch_and_call() {
    assert!(matches!(
        op(0x17ffffff),
        Op::Branch {
            target: GuestAddr(0xfffc),
            link: false
        }
    ));
    assert!(matches!(
        op(0x94000004),
        Op::Branch {
            target: GuestAddr(0x10010),
            link: true
        }
    ));
    assert!(matches!(
        op(0xd65f03c0),
        Op::Indirect {
            src: Reg::Gpr(30),
            link: false
        }
    ));
}
#[test]
fn conditional_branch_classes() {
    assert!(matches!(
        op(0x54ffffe1),
        Op::Conditional {
            target: GuestAddr(0xfffc),
            condition: 1
        }
    ));
    assert!(matches!(
        op(0xb5000040),
        Op::CompareBranch {
            width: Width::X64,
            nonzero: true,
            ..
        }
    ));
    assert!(matches!(
        op(0xb6f80040),
        Op::TestBranch {
            bit: 63,
            nonzero: false,
            ..
        }
    ));
}
#[test]
fn address_generation() {
    assert!(matches!(
        op(0x10000020),
        Op::Address {
            value: GuestAddr(0x10004),
            ..
        }
    ));
    assert!(matches!(
        op(0xb0000000),
        Op::Address {
            value: GuestAddr(0x11000),
            ..
        }
    ));
}
#[test]
fn memory_addressing() {
    assert!(matches!(
        op(0xf9400440),
        Op::Memory {
            load: true,
            bytes: 8,
            offset: 8,
            mode: 0,
            ..
        }
    ));
    assert!(matches!(
        op(0xf81f8fe0),
        Op::Memory {
            load: false,
            bytes: 8,
            base: Reg::Sp,
            offset: -8,
            mode: 3,
            ..
        }
    ));
    assert!(matches!(
        op(0xa9bf7bfd),
        Op::Pair {
            load: false,
            bytes: 8,
            base: Reg::Sp,
            offset: -16,
            mode: 3,
            ..
        }
    ));
}
#[test]
fn syscall_and_nop() {
    assert!(matches!(op(0xd40000e1), Op::Svc { id: 7 }));
    assert!(matches!(op(0xd503201f), Op::Nop));
}
#[test]
fn reserved_and_unsupported_are_explicit() {
    for w in [
        // 0x4ea1d820 is FRECPE (a SIMD reciprocal estimate, not implemented).
        0, 0xffffffff, 0x52c00000, 0x32a00000, 0x0bc00000, 0x0b008000, 0x4ea1d820, 0xf8400c00,
    ] {
        let e = decode(w, GuestAddr(0x10000)).unwrap_err();
        assert!(e.contains("0x10000"));
        assert!(e.contains("opcode"));
    }
}
#[test]
fn rejects_unaligned_and_overflowing_pc() {
    assert!(decode(0xd503201f, GuestAddr(1)).is_err());
    assert!(decode(0x17ffffff, GuestAddr(0)).is_err());
}

fn rejected(w: u32) -> bool {
    decode(w, GuestAddr(0x10000)).is_err()
}
#[test]
fn decode_bit_masks_matches_architectural_examples() {
    use wl_aarch64::decode_bit_masks;
    // 0x5555... has 2-bit elements: imms = 0b111100, immr = 0.
    assert_eq!(
        decode_bit_masks(0, 0b111100, 0, true),
        Some((0x5555_5555_5555_5555, 0x5555_5555_5555_5555))
    );
    // All-ones elements are reserved for logical immediates.
    assert_eq!(decode_bit_masks(1, 63, 0, true), None);
    // UBFM LSR #4 (immr = 4, imms = 63): wmask is ROR(Ones(64), 4), tmask Ones(60).
    assert_eq!(
        decode_bit_masks(1, 63, 4, false),
        Some((u64::MAX, u64::MAX >> 4))
    );
}
#[test]
fn new_integer_families_decode_to_expected_operations() {
    assert!(matches!(
        op(0x92401c03),
        Op::Logical {
            kind: 0,
            rhs: Operand::Imm(0xff),
            ..
        }
    ));
    assert!(matches!(op(0x927ff81f), Op::Logical { dst: Reg::Sp, .. })); // AND SP, X0, #~1
    assert!(matches!(
        op(0xd3441c03),
        Op::Bitfield {
            kind: 2,
            rotate: 4,
            sign_bit: 7,
            ..
        }
    ));
    assert!(matches!(
        op(0x9a8103e3),
        Op::Select {
            condition: 0,
            invert: false,
            increment: false,
            ..
        }
    ));
    assert!(matches!(op(0x9ac10c03), Op::Divide { signed: true, .. }));
    assert!(matches!(
        op(0x9b417c03),
        Op::MultiplyHigh { signed: true, .. }
    ));
    assert!(matches!(
        op(0xb8a37845),
        Op::Memory {
            signed: true,
            bytes: 4,
            index: Some(_),
            ..
        }
    ));
    assert!(matches!(
        op(0x98000045),
        Op::LoadLiteral {
            bytes: 4,
            signed: true,
            ..
        }
    ));
    assert!(matches!(
        op(0xc85f7c45),
        Op::Exclusive {
            load: true,
            exclusive: true,
            pair: false,
            bytes: 8,
            ..
        }
    ));
    assert!(matches!(
        op(0xd53bd060),
        Op::SystemRegister {
            read: true,
            register: SystemRegister::TpidrroEl0,
            ..
        }
    ));
    assert!(matches!(op(0xd4200020), Op::Trap { kind: 0, imm: 1 }));
    assert!(matches!(op(0xd503233f), Op::Nop)); // PACIASP executes as a hint on ARMv8.0
    assert!(matches!(op(0xf9800040), Op::Nop)); // PRFM
}
#[test]
fn rejects_unallocated_and_unpredictable_forms() {
    for w in [
        0xb8e36845,                           // LDRSW encoding with opc=11 (unallocated)
        0x12400000 | 1 << 22,                 // 32-bit logical immediate with N = 1
        0x92407c00 | 0x3f << 10 | 0x3f << 16, // all-ones element (reserved immediate)
        0x13000000 | 1 << 22,                 // 32-bit bitfield with N = 1
        0x93c00000 | 1 << 21,                 // EXTR with o0 = 1
        0xd51bd060,                           // MSR TPIDRRO_EL0 from EL0
        0xc8a07c41,                           // CAS (ARMv8.1 LSE)
        0xf8200041,                           // LDADD (ARMv8.1 LSE)
        0xc8057c45,              // STXR W5, X5, [X2]: status register equals data register
        0xc87f1445,              // LDXP X5, X5, [X2]
        0xb8400845,              // LDTR (unprivileged)
        0xb8a04845, // register offset with option 010 for size 2 is fine; option 000 below is not
        0xf8600845 & !(7 << 13), // register offset with option 000
        0x1ac01000 | 0x3f << 10, // unallocated 2-source opcode
    ]
    .iter()
    .copied()
    .filter(|w| *w != 0xb8a04845)
    {
        assert!(rejected(w), "{w:#010x} should be rejected");
    }
    assert!(!rejected(0xb8a04845));
}

#[test]
fn simd_and_fp_families_decode() {
    use wl_aarch64::{FpBinary, FpOp, Shape, SimdOp};
    // FADD D0, D1, D2.
    assert!(matches!(
        op(0x1e622820),
        Op::Fp(FpOp::Binary {
            bits: 64,
            op: FpBinary::Add,
            d: 0,
            n: 1,
            m: 2
        })
    ));
    // CMEQ V0.4S, V1.4S, V2.4S must not be taken for a SIMD&FP pair (bits 29:25 differ).
    assert!(matches!(
        op(0x6ea28c20),
        Op::Simd(SimdOp::Int {
            shape: Shape {
                esize: 32,
                lanes: 4
            },
            ..
        })
    ));
    // LDP Q10, Q11, [X2, #64].
    assert!(matches!(
        op(0xad422c4a),
        Op::VecPair {
            load: true,
            bytes: 16,
            first: 10,
            second: 11,
            offset: 64,
            ..
        }
    ));
    // LDR Q0, [X2] zero-extends nothing: a 16-byte load.
    assert!(matches!(
        op(0x3dc00040),
        Op::VecMemory {
            load: true,
            bytes: 16,
            vt: 0,
            ..
        }
    ));
    // FMLA V0.4S, V1.4S, V2.S[3] (by element).
    assert!(matches!(
        op(0x4fa21820),
        Op::Simd(SimdOp::FpElement { index: 3, m: 2, .. })
    ));
    // UMULL V3.2D, V0.2S, V1.2S.
    assert!(matches!(
        op(0x2ea1c003),
        Op::Simd(SimdOp::Long3 {
            esize: 32,
            upper: false,
            signed: false,
            ..
        })
    ));
    // LD4 {V11.16B-V14.16B}, [X2].
    assert!(matches!(op(0x4c40004b), Op::VecStructures(s) if s.selem == 4 && s.load));
}
#[test]
fn rejects_simd_and_fp_encodings_outside_armv8_0_or_not_implemented() {
    for w in [
        0x1ee22820u32,        // FADD H0, H1, H2: half-precision arithmetic is ARMv8.2
        0x4ea1d820,           // FRECPE V0.4S, V1.4S: reciprocal estimate not implemented
        0x1e22c020 | 0x2000,  // FCVT with identical source and destination precision
        0x2ee0c003,           // UMULL with size 3 (reserved)
        0x0e208400 | 3 << 22, // ADD V.1D (64-bit lanes in a 64-bit vector)
    ] {
        assert!(rejected(w), "{w:#010x} should be rejected");
    }
}
