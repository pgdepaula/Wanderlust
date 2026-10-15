//! Advanced SIMD lane semantics on 128-bit register values.
//!
//! Lane `i` of an `e`-bit arrangement is bits `[i*e, (i+1)*e)` of the register, which with
//! little-endian memory places lane 0 at the lowest address. Every helper returns the full
//! 128-bit result of its operation; the generated code masks it to the destination width
//! (`total`), which zeroes the upper half for 64-bit arrangements and scalar forms.
// Helpers take the operand list of the instruction they implement.
#![allow(clippy::too_many_arguments)]
use crate::{fp, Cpu};

/// FPSR.QC, the cumulative saturation flag.
pub const QC: u64 = 1 << 27;

#[inline]
pub fn emask(e: u32) -> u64 {
    if e >= 64 {
        u64::MAX
    } else {
        (1u64 << e) - 1
    }
}
#[inline]
pub fn total_mask(bits: u32) -> u128 {
    if bits >= 128 {
        u128::MAX
    } else {
        (1u128 << bits) - 1
    }
}
#[inline]
pub fn lane(v: u128, e: u32, i: u32) -> u64 {
    (v >> (i * e)) as u64 & emask(e)
}
#[inline]
pub fn with_lane(v: u128, e: u32, i: u32, x: u64) -> u128 {
    let m = u128::from(emask(e)) << (i * e);
    (v & !m) | ((u128::from(x & emask(e))) << (i * e))
}
#[inline]
fn sext(x: u64, e: u32) -> i64 {
    crate::sign_extend(x, e as u8)
}
#[inline]
fn signed_max(e: u32) -> i64 {
    (emask(e) >> 1) as i64
}
#[inline]
fn signed_min(e: u32) -> i64 {
    -signed_max(e) - 1
}

// ---- integer binary operations (three registers, same arrangement) -------------------------
pub const ADD: u8 = 0;
pub const SUB: u8 = 1;
pub const MUL: u8 = 2;
pub const MLA: u8 = 3;
pub const MLS: u8 = 4;
pub const CMGT: u8 = 5;
pub const CMHI: u8 = 6;
pub const CMGE: u8 = 7;
pub const CMHS: u8 = 8;
pub const CMTST: u8 = 9;
pub const CMEQ: u8 = 10;
pub const SSHL: u8 = 11;
pub const USHL: u8 = 12;
pub const SMAX: u8 = 13;
pub const UMAX: u8 = 14;
pub const SMIN: u8 = 15;
pub const UMIN: u8 = 16;
pub const SABD: u8 = 17;
pub const UABD: u8 = 18;
pub const SHADD: u8 = 19;
pub const UHADD: u8 = 20;
pub const SRHADD: u8 = 21;
pub const URHADD: u8 = 22;
pub const SHSUB: u8 = 23;
pub const UHSUB: u8 = 24;
pub const SQADD: u8 = 25;
pub const UQADD: u8 = 26;
pub const SQSUB: u8 = 27;
pub const UQSUB: u8 = 28;
pub const SABA: u8 = 29;
pub const UABA: u8 = 30;
/// Signed saturating doubling multiply returning the high half (SQDMULH).
pub const SQDMULH: u8 = 31;
/// SQDMULH with rounding (SQRDMULH).
pub const SQRDMULH: u8 = 32;

/// Shifts by the signed low byte of `amount` (SSHL/USHL): positive left, negative right.
fn shift_by_register(x: u64, amount: u64, e: u32, signed: bool) -> u64 {
    let s = amount as u8 as i8 as i32;
    let r = if s >= 0 {
        if s >= e as i32 {
            0
        } else {
            x << s
        }
    } else {
        let s = -s;
        if signed {
            let v = sext(x, e);
            (if s >= e as i32 { v >> (e - 1) } else { v >> s }) as u64
        } else if s >= e as i32 {
            0
        } else {
            x >> s
        }
    };
    r & emask(e)
}
fn int_lane(op: u8, x: u64, y: u64, d: u64, e: u32, qc: &mut bool) -> u64 {
    let (sx, sy) = (sext(x, e), sext(y, e));
    let saturate = |v: i128, lo: i128, hi: i128, qc: &mut bool| -> u64 {
        if v > hi {
            *qc = true;
            hi as u64
        } else if v < lo {
            *qc = true;
            lo as u64
        } else {
            v as u64
        }
    };
    let r = match op {
        ADD => x.wrapping_add(y),
        SUB => x.wrapping_sub(y),
        MUL => x.wrapping_mul(y),
        MLA => d.wrapping_add(x.wrapping_mul(y)),
        MLS => d.wrapping_sub(x.wrapping_mul(y)),
        CMGT => {
            if sx > sy {
                u64::MAX
            } else {
                0
            }
        }
        CMHI => {
            if x > y {
                u64::MAX
            } else {
                0
            }
        }
        CMGE => {
            if sx >= sy {
                u64::MAX
            } else {
                0
            }
        }
        CMHS => {
            if x >= y {
                u64::MAX
            } else {
                0
            }
        }
        CMTST => {
            if x & y != 0 {
                u64::MAX
            } else {
                0
            }
        }
        CMEQ => {
            if x == y {
                u64::MAX
            } else {
                0
            }
        }
        SSHL => shift_by_register(x, y, e, true),
        USHL => shift_by_register(x, y, e, false),
        SMAX => sx.max(sy) as u64,
        UMAX => x.max(y),
        SMIN => sx.min(sy) as u64,
        UMIN => x.min(y),
        SABD => (i128::from(sx) - i128::from(sy)).unsigned_abs() as u64,
        UABD => x.abs_diff(y),
        SABA => d.wrapping_add((i128::from(sx) - i128::from(sy)).unsigned_abs() as u64),
        UABA => d.wrapping_add(x.abs_diff(y)),
        SHADD => ((i128::from(sx) + i128::from(sy)) >> 1) as u64,
        UHADD => ((u128::from(x) + u128::from(y)) >> 1) as u64,
        SRHADD => ((i128::from(sx) + i128::from(sy) + 1) >> 1) as u64,
        URHADD => ((u128::from(x) + u128::from(y) + 1) >> 1) as u64,
        SHSUB => ((i128::from(sx) - i128::from(sy)) >> 1) as u64,
        UHSUB => ((i128::from(x) - i128::from(y)) >> 1) as u64,
        SQADD => saturate(
            i128::from(sx) + i128::from(sy),
            signed_min(e).into(),
            signed_max(e).into(),
            qc,
        ),
        UQADD => saturate(i128::from(x) + i128::from(y), 0, emask(e).into(), qc),
        SQSUB => saturate(
            i128::from(sx) - i128::from(sy),
            signed_min(e).into(),
            signed_max(e).into(),
            qc,
        ),
        UQSUB => saturate(i128::from(x) - i128::from(y), 0, emask(e).into(), qc),
        SQDMULH | SQRDMULH => {
            let round = if op == SQRDMULH { 1i128 << (e - 1) } else { 0 };
            saturate(
                (2 * i128::from(sx) * i128::from(sy) + round) >> e,
                signed_min(e).into(),
                signed_max(e).into(),
                qc,
            )
        }
        _ => 0,
    };
    r & emask(e)
}
/// Lanewise integer operation over `n` lanes of `e` bits; `d` supplies accumulators.
pub fn int2(c: &mut Cpu, op: u8, e: u32, n: u32, a: u128, b: u128, d: u128) -> u128 {
    let mut qc = false;
    let mut r = 0u128;
    for i in 0..n {
        r = with_lane(
            r,
            e,
            i,
            int_lane(op, lane(a, e, i), lane(b, e, i), lane(d, e, i), e, &mut qc),
        );
    }
    if qc {
        c.fpsr |= QC;
    }
    r
}
/// Pairwise operations (ADDP, SMAXP, ...): lanes of `b:a` are combined in adjacent pairs.
pub fn pairwise(op: u8, e: u32, n: u32, a: u128, b: u128) -> u128 {
    let mut r = 0u128;
    let mut qc = false;
    for i in 0..n {
        let (src, j) = if i < n / 2 {
            (a, 2 * i)
        } else {
            (b, 2 * (i - n / 2))
        };
        r = with_lane(
            r,
            e,
            i,
            int_lane(op, lane(src, e, j), lane(src, e, j + 1), 0, e, &mut qc),
        );
    }
    r
}

// ---- whole-register logical operations ------------------------------------------------------
pub const AND: u8 = 0;
pub const BIC: u8 = 1;
pub const ORR: u8 = 2;
pub const ORN: u8 = 3;
pub const EOR: u8 = 4;
pub const BSL: u8 = 5;
pub const BIT: u8 = 6;
pub const BIF: u8 = 7;
#[inline]
pub fn logical(op: u8, a: u128, b: u128, d: u128) -> u128 {
    match op {
        AND => a & b,
        BIC => a & !b,
        ORR => a | b,
        ORN => a | !b,
        EOR => a ^ b,
        BSL => (d & a) | (!d & b),
        BIT => (a & b) | (d & !b),
        _ => (a & !b) | (d & b),
    }
}

// ---- floating-point lanewise operations ------------------------------------------------------
pub const FADD: u8 = 0;
pub const FSUB: u8 = 1;
pub const FMUL: u8 = 2;
pub const FDIV: u8 = 3;
pub const FMAX: u8 = 4;
pub const FMIN: u8 = 5;
pub const FMAXNM: u8 = 6;
pub const FMINNM: u8 = 7;
pub const FABD: u8 = 8;
pub const FMULX: u8 = 9;
pub const FMLA: u8 = 10;
pub const FMLS: u8 = 11;
pub const FCMEQ: u8 = 12;
pub const FCMGE: u8 = 13;
pub const FCMGT: u8 = 14;
pub const FACGE: u8 = 15;
pub const FACGT: u8 = 16;
pub const FRECPS: u8 = 17;
pub const FRSQRTS: u8 = 18;
fn fp_lane(c: &mut Cpu, op: u8, e: u32, x: u64, y: u64, d: u64) -> u64 {
    let f = fp::format(e);
    match op {
        FADD => fp::add(c, f, x, y, false),
        FSUB => fp::add(c, f, x, y, true),
        FMUL => fp::mul(c, f, x, y, false),
        FDIV => fp::div(c, f, x, y),
        FMAX => fp::min_max(c, f, x, y, true, false),
        FMIN => fp::min_max(c, f, x, y, false, false),
        FMAXNM => fp::min_max(c, f, x, y, true, true),
        FMINNM => fp::min_max(c, f, x, y, false, true),
        FABD => fp::abs(f, fp::add(c, f, x, y, true)),
        FMULX => fp::mulx(c, f, x, y),
        FMLA => fp::mul_add(c, f, d, x, y),
        FMLS => fp::mul_add(c, f, d, fp::neg(f, x), y),
        FCMEQ => fp::compare_mask(c, f, x, y, 0, false),
        FCMGE => fp::compare_mask(c, f, x, y, 1, false),
        FCMGT => fp::compare_mask(c, f, x, y, 2, false),
        FACGE => fp::compare_mask(c, f, x, y, 1, true),
        FACGT => fp::compare_mask(c, f, x, y, 2, true),
        FRECPS => fp::recip_step(c, f, x, y),
        _ => fp::rsqrt_step(c, f, x, y),
    }
}
/// Lanewise FP operation; lanes are processed in ascending order, so FPSR accumulates in
/// architectural element order.
pub fn fp2(c: &mut Cpu, op: u8, e: u32, n: u32, a: u128, b: u128, d: u128) -> u128 {
    let mut r = 0u128;
    for i in 0..n {
        let v = fp_lane(c, op, e, lane(a, e, i), lane(b, e, i), lane(d, e, i));
        r = with_lane(r, e, i, v);
    }
    r
}
/// FP operation with the second operand taken from one element of `b` (by-element forms).
pub fn fp2_element(
    c: &mut Cpu,
    op: u8,
    e: u32,
    n: u32,
    a: u128,
    b: u128,
    index: u32,
    d: u128,
) -> u128 {
    let y = lane(b, e, index);
    let mut r = 0u128;
    for i in 0..n {
        let v = fp_lane(c, op, e, lane(a, e, i), y, lane(d, e, i));
        r = with_lane(r, e, i, v);
    }
    r
}
/// Integer operation with the second operand taken from one element of `b`.
pub fn int2_element(
    c: &mut Cpu,
    op: u8,
    e: u32,
    n: u32,
    a: u128,
    b: u128,
    index: u32,
    d: u128,
) -> u128 {
    let y = lane(b, e, index);
    let mut splat = 0u128;
    for i in 0..n {
        splat = with_lane(splat, e, i, y);
    }
    int2(c, op, e, n, a, splat, d)
}
/// FADDP/FMAXP/FMINP/FMAXNMP/FMINNMP: adjacent pairs of `b:a`.
pub fn fp_pairwise(c: &mut Cpu, op: u8, e: u32, n: u32, a: u128, b: u128) -> u128 {
    let mut r = 0u128;
    for i in 0..n {
        let (src, j) = if i < n / 2 {
            (a, 2 * i)
        } else {
            (b, 2 * (i - n / 2))
        };
        let v = fp_lane(c, op, e, lane(src, e, j), lane(src, e, j + 1), 0);
        r = with_lane(r, e, i, v);
    }
    r
}

// ---- unary operations (two-register miscellaneous) -------------------------------------------
pub const NOT: u8 = 0;
pub const NEG: u8 = 1;
pub const ABS: u8 = 2;
pub const CNT: u8 = 3;
pub const RBIT: u8 = 4;
pub const REV16: u8 = 5;
pub const REV32: u8 = 6;
pub const REV64: u8 = 7;
pub const CMGT0: u8 = 8;
pub const CMGE0: u8 = 9;
pub const CMEQ0: u8 = 10;
pub const CMLE0: u8 = 11;
pub const CMLT0: u8 = 12;
pub const CLZ: u8 = 13;
pub const CLS: u8 = 14;
pub const SQABS: u8 = 15;
pub const SQNEG: u8 = 16;
pub fn int1(c: &mut Cpu, op: u8, e: u32, n: u32, a: u128) -> u128 {
    if matches!(op, REV16 | REV32 | REV64) {
        // Reverse the order of e-bit elements inside each container.
        let container = match op {
            REV16 => 16,
            REV32 => 32,
            _ => 64,
        };
        let per = container / e;
        let mut r = 0u128;
        for i in 0..n {
            let j = (i / per) * per + (per - 1 - i % per);
            r = with_lane(r, e, i, lane(a, e, j));
        }
        return r;
    }
    let mut qc = false;
    let mut r = 0u128;
    for i in 0..n {
        let x = lane(a, e, i);
        let s = sext(x, e);
        let v = match op {
            NOT => !x,
            NEG => x.wrapping_neg(),
            ABS => s.unsigned_abs(),
            CNT => u64::from(x.count_ones()),
            RBIT => x.reverse_bits() >> (64 - e),
            CMGT0 => {
                if s > 0 {
                    u64::MAX
                } else {
                    0
                }
            }
            CMGE0 => {
                if s >= 0 {
                    u64::MAX
                } else {
                    0
                }
            }
            CMEQ0 => {
                if x == 0 {
                    u64::MAX
                } else {
                    0
                }
            }
            CMLE0 => {
                if s <= 0 {
                    u64::MAX
                } else {
                    0
                }
            }
            CMLT0 => {
                if s < 0 {
                    u64::MAX
                } else {
                    0
                }
            }
            CLZ => u64::from(x.leading_zeros() - (64 - e)),
            CLS => {
                let y = ((x >> 1) ^ x) & (emask(e) >> 1);
                u64::from(y.leading_zeros() - (64 - e) - 1)
            }
            SQABS | SQNEG => {
                let v = if op == SQABS {
                    s.unsigned_abs() as i128
                } else {
                    -i128::from(s)
                };
                if v > i128::from(signed_max(e)) {
                    qc = true;
                    signed_max(e) as u64
                } else {
                    v as u64
                }
            }
            _ => 0,
        };
        r = with_lane(r, e, i, v);
    }
    if qc {
        c.fpsr |= QC;
    }
    r
}
pub const FNEG: u8 = 0;
pub const FABS: u8 = 1;
pub const FSQRT: u8 = 2;
/// FRINT with mode `fp::ROUND_*`; FRINTX is FRINTX_BASE (+ FPCR mode).
pub const FRINT: u8 = 3;
pub const FRINTX: u8 = 4;
pub const FCVT_S: u8 = 5;
pub const FCVT_U: u8 = 6;
pub const SCVTF: u8 = 7;
pub const UCVTF: u8 = 8;
pub const FCMGT0: u8 = 9;
pub const FCMGE0: u8 = 10;
pub const FCMEQ0: u8 = 11;
pub const FCMLE0: u8 = 12;
pub const FCMLT0: u8 = 13;
/// FP lanewise unary operation. `mode` is the rounding mode for FRINT/FCVT forms and the
/// number of fraction bits for fixed-point SCVTF/UCVTF/FCVTZ forms (`fbits`).
pub fn fp1(c: &mut Cpu, op: u8, e: u32, n: u32, a: u128, mode: u8, fbits: u32) -> u128 {
    let f = fp::format(e);
    let mut r = 0u128;
    for i in 0..n {
        let x = lane(a, e, i);
        let zero = 0u64;
        let v = match op {
            FNEG => fp::neg(f, x),
            FABS => fp::abs(f, x),
            FSQRT => fp::sqrt(c, f, x),
            FRINT => fp::round_int(c, f, x, mode, false),
            FRINTX => fp::round_int(c, f, x, fp::ROUND_FPCR, true),
            FCVT_S => fp::to_fixed(c, f, x, e, true, mode, fbits),
            FCVT_U => fp::to_fixed(c, f, x, e, false, mode, fbits),
            SCVTF => fp::from_int(c, f, x, e, true, fbits),
            UCVTF => fp::from_int(c, f, x, e, false, fbits),
            FCMGT0 => fp::compare_mask(c, f, x, zero, 2, false),
            FCMGE0 => fp::compare_mask(c, f, x, zero, 1, false),
            FCMEQ0 => fp::compare_mask(c, f, x, zero, 0, false),
            // x <= 0 is 0 >= x; x < 0 is 0 > x.
            FCMLE0 => fp::compare_mask(c, f, zero, x, 1, false),
            _ => fp::compare_mask(c, f, zero, x, 2, false),
        };
        r = with_lane(r, e, i, v);
    }
    r
}

// ---- shifts by immediate, widening and narrowing --------------------------------------------
pub const SHL: u8 = 0;
pub const USHR: u8 = 1;
pub const SSHR: u8 = 2;
pub const USRA: u8 = 3;
pub const SSRA: u8 = 4;
pub const URSHR: u8 = 5;
pub const SRSHR: u8 = 6;
pub const URSRA: u8 = 7;
pub const SRSRA: u8 = 8;
pub const SLI: u8 = 9;
pub const SRI: u8 = 10;
pub fn shift_imm(op: u8, e: u32, n: u32, a: u128, d: u128, amount: u32) -> u128 {
    let mut r = 0u128;
    for i in 0..n {
        let x = lane(a, e, i);
        let acc = lane(d, e, i);
        let right_u = |x: u64| if amount >= 64 { 0 } else { x >> amount };
        let right_s = |x: u64| (sext(x, e) >> amount.min(63)) as u64;
        let round_u = |x: u64| ((u128::from(x) + (1u128 << (amount - 1))) >> amount) as u64;
        let round_s =
            |x: u64| ((i128::from(sext(x, e)) + (1i128 << (amount - 1))) >> amount) as u64;
        let v = match op {
            SHL => x << amount,
            USHR => right_u(x),
            SSHR => right_s(x),
            USRA => acc.wrapping_add(right_u(x)),
            SSRA => acc.wrapping_add(right_s(x)),
            URSHR => round_u(x),
            SRSHR => round_s(x),
            URSRA => acc.wrapping_add(round_u(x)),
            SRSRA => acc.wrapping_add(round_s(x)),
            SLI => {
                let m = (emask(e) << amount) & emask(e);
                (acc & !m) | ((x << amount) & m)
            }
            _ => {
                let m = if amount >= e { 0 } else { emask(e) >> amount };
                (acc & !m) | (right_u(x) & m)
            }
        };
        r = with_lane(r, e, i, v);
    }
    r
}
/// SSHLL/USHLL (and SXTL/UXTL): lanes of the selected source half widened to `2e` bits.
pub fn widen_shift(e: u32, n: u32, a: u128, upper: bool, signed: bool, amount: u32) -> u128 {
    let src = if upper { a >> 64 } else { a };
    let mut r = 0u128;
    for i in 0..n {
        let x = lane(src, e, i);
        let w = if signed { sext(x, e) as u64 } else { x };
        r = with_lane(r, 2 * e, i, w << amount);
    }
    r
}
pub const XTN: u8 = 0;
pub const SQXTN: u8 = 1;
pub const UQXTN: u8 = 2;
pub const SQXTUN: u8 = 3;
pub const SHRN: u8 = 4;
pub const RSHRN: u8 = 5;
pub const SQSHRN: u8 = 6;
pub const SQRSHRN: u8 = 7;
pub const UQSHRN: u8 = 8;
pub const UQRSHRN: u8 = 9;
pub const SQSHRUN: u8 = 10;
pub const SQRSHRUN: u8 = 11;
/// Narrowing: `n` source lanes of `2e` bits become `e`-bit lanes, written to the lower half
/// (upper half zeroed) or, for the "2" forms, to the upper half with the lower half kept.
pub fn narrow(
    c: &mut Cpu,
    op: u8,
    e: u32,
    n: u32,
    a: u128,
    d: u128,
    upper: bool,
    amount: u32,
) -> u128 {
    let mut qc = false;
    let mut half = 0u128;
    let w = 2 * e;
    for i in 0..n {
        let x = lane(a, w, i);
        let s = sext(x, w);
        let v = match op {
            XTN => x,
            SHRN => x >> amount,
            RSHRN => ((u128::from(x) + (1u128 << (amount - 1))) >> amount) as u64,
            SQXTN => {
                let v = s.clamp(signed_min(e), signed_max(e));
                qc |= v != s;
                v as u64
            }
            UQXTN => {
                let v = x.min(emask(e));
                qc |= v != x;
                v
            }
            SQSHRN | SQRSHRN | UQSHRN | UQRSHRN | SQSHRUN | SQRSHRUN => {
                // Shift right (rounding for the R forms) at full precision, then saturate:
                // signed source for SQ*, unsigned for UQ*; SQ*UN saturate to unsigned.
                let rounding = matches!(op, SQRSHRN | UQRSHRN | SQRSHRUN);
                let source: i128 = if matches!(op, UQSHRN | UQRSHRN) {
                    i128::from(x)
                } else {
                    i128::from(s)
                };
                let shifted = (source + if rounding { 1i128 << (amount - 1) } else { 0 }) >> amount;
                let (lo, hi): (i128, i128) = if matches!(op, SQSHRN | SQRSHRN) {
                    (signed_min(e).into(), signed_max(e).into())
                } else {
                    (0, emask(e).into())
                };
                let v = shifted.clamp(lo, hi);
                qc |= v != shifted;
                v as u64 & emask(e)
            }
            _ => {
                let v = s.clamp(0, emask(e) as i64);
                qc |= v != s;
                v as u64
            }
        };
        half = with_lane(half, e, i, v);
    }
    if qc {
        c.fpsr |= QC;
    }
    if upper {
        (d & total_mask(64)) | (half << 64)
    } else {
        half
    }
}
/// FCVTL/FCVTL2: half→single or single→double for the selected source half.
pub fn fp_widen(c: &mut Cpu, e: u32, n: u32, a: u128, upper: bool) -> crate::RuntimeResult<u128> {
    let src = if upper { a >> 64 } else { a };
    let mut r = 0u128;
    for i in 0..n {
        let v = fp::convert(c, fp::format(e), fp::format(2 * e), lane(src, e, i))?;
        r = with_lane(r, 2 * e, i, v);
    }
    Ok(r)
}
/// FCVTN/FCVTN2: single→half or double→single.
pub fn fp_narrow(
    c: &mut Cpu,
    e: u32,
    n: u32,
    a: u128,
    d: u128,
    upper: bool,
) -> crate::RuntimeResult<u128> {
    let mut half = 0u128;
    for i in 0..n {
        let v = fp::convert(c, fp::format(2 * e), fp::format(e), lane(a, 2 * e, i))?;
        half = with_lane(half, e, i, v);
    }
    Ok(if upper {
        (d & total_mask(64)) | (half << 64)
    } else {
        half
    })
}

// ---- reductions --------------------------------------------------------------------------------
pub const ADDV: u8 = 0;
pub const SMAXV: u8 = 1;
pub const UMAXV: u8 = 2;
pub const SMINV: u8 = 3;
pub const UMINV: u8 = 4;
pub const SADDLV: u8 = 5;
pub const UADDLV: u8 = 6;
pub fn reduce(op: u8, e: u32, n: u32, a: u128) -> u128 {
    let mut acc = lane(a, e, 0);
    let mut wide: i128 = match op {
        SADDLV => sext(acc, e).into(),
        _ => acc.into(),
    };
    for i in 1..n {
        let x = lane(a, e, i);
        let (sa, sx) = (sext(acc, e), sext(x, e));
        acc = match op {
            ADDV => acc.wrapping_add(x) & emask(e),
            SMAXV => sa.max(sx) as u64 & emask(e),
            UMAXV => acc.max(x),
            SMINV => sa.min(sx) as u64 & emask(e),
            UMINV => acc.min(x),
            _ => acc,
        };
        wide += if op == SADDLV {
            i128::from(sx)
        } else {
            i128::from(x)
        };
    }
    if matches!(op, SADDLV | UADDLV) {
        (wide as u128) & total_mask(2 * e)
    } else {
        u128::from(acc)
    }
}
/// FMAXV/FMINV/FMAXNMV/FMINNMV: pairwise tree reduction, as `Reduce()` in the pseudocode.
pub fn fp_reduce(c: &mut Cpu, op: u8, e: u32, n: u32, a: u128) -> u128 {
    let mut values: Vec<u64> = (0..n).map(|i| lane(a, e, i)).collect();
    while values.len() > 1 {
        let half = values.len() / 2;
        let (lo, hi) = values.split_at(half);
        values = lo
            .iter()
            .zip(hi)
            .map(|(x, y)| fp_lane(c, op, e, *x, *y, 0))
            .collect::<Vec<_>>();
    }
    u128::from(values[0])
}

// ---- permutes, extraction and table lookup ---------------------------------------------------
pub const UZP1: u8 = 1;
pub const TRN1: u8 = 2;
pub const ZIP1: u8 = 3;
pub const UZP2: u8 = 5;
pub const TRN2: u8 = 6;
pub const ZIP2: u8 = 7;
pub fn permute(op: u8, e: u32, n: u32, a: u128, b: u128) -> u128 {
    let part = u32::from(op >> 2 & 1);
    let mut r = 0u128;
    for i in 0..n {
        let v = match op & 3 {
            1 => {
                // UZP: even (part 0) or odd (part 1) elements of b:a.
                let j = 2 * i + part;
                if j < n {
                    lane(a, e, j)
                } else {
                    lane(b, e, j - n)
                }
            }
            2 => {
                // TRN: interleave corresponding even/odd elements.
                let pair = i / 2 * 2 + part;
                if i % 2 == 0 {
                    lane(a, e, pair)
                } else {
                    lane(b, e, pair)
                }
            }
            _ => {
                // ZIP: interleave the lower (part 0) or upper (part 1) halves.
                let j = part * n / 2 + i / 2;
                if i % 2 == 0 {
                    lane(a, e, j)
                } else {
                    lane(b, e, j)
                }
            }
        };
        r = with_lane(r, e, i, v);
    }
    r
}
/// EXT: bytes `index..index+total` of the concatenation b:a.
pub fn ext(a: u128, b: u128, index: u32, total_bytes: u32) -> u128 {
    let mut r = 0u128;
    for i in 0..total_bytes {
        let j = index + i;
        let byte = if j < total_bytes {
            lane(a, 8, j)
        } else {
            lane(b, 8, j - total_bytes)
        };
        r = with_lane(r, 8, i, byte);
    }
    r
}
/// TBL (`keep = false`: out-of-range indices give 0) and TBX (`keep = true`: they keep `d`).
pub fn table(table: &[u128], indices: u128, d: u128, total_bytes: u32, keep: bool) -> u128 {
    let mut r = 0u128;
    for i in 0..total_bytes {
        let index = lane(indices, 8, i) as usize;
        let byte = if index < 16 * table.len() {
            lane(table[index / 16], 8, (index % 16) as u32)
        } else if keep {
            lane(d, 8, i)
        } else {
            0
        };
        r = with_lane(r, 8, i, byte);
    }
    r
}
/// Replicates the low `e` bits of `x` into `n` lanes.
#[inline]
pub fn dup(x: u64, e: u32, n: u32) -> u128 {
    let mut r = 0u128;
    for i in 0..n {
        r = with_lane(r, e, i, x);
    }
    r
}

/// LD1-LD4/ST1-ST4 (multiple structures), LDnR and single-lane forms, from `address`.
/// Element order follows the pseudocode: repetitions, then elements, then structure members;
/// register `t + r + s` (mod 32) receives element `e`. Loads write `total` bits per register
/// (a 64-bit form zeroes the upper half); stores read the same lanes. Returns bytes accessed.
#[allow(clippy::too_many_arguments)]
pub fn structures(
    c: &mut Cpu,
    m: &mut crate::Memory,
    load: bool,
    selem: u32,
    repeats: u32,
    e: u32,
    total: u32,
    t: u32,
    lane_index: Option<u32>,
    replicate: bool,
    address: u64,
) -> crate::RuntimeResult<u64> {
    let bytes = e / 8;
    let mask = total_mask(total);
    let mut offset = 0u64;
    if replicate || lane_index.is_some() {
        for s in 0..selem {
            let reg = ((t + s) % 32) as usize;
            let at = address.wrapping_add(offset);
            if load {
                let x = m.load_vector(at, bytes as u8)? as u64;
                c.v[reg] = match lane_index {
                    Some(i) => with_lane(c.v[reg], e, i, x) & mask,
                    None => dup(x, e, total / e),
                };
            } else if let Some(i) = lane_index {
                m.store_vector(at, bytes as u8, u128::from(lane(c.v[reg], e, i)))?;
            }
            offset += u64::from(bytes);
        }
        return Ok(offset);
    }
    let elements = total / e;
    let mut staged = c.v;
    if load {
        for r in 0..repeats {
            for s in 0..selem {
                staged[((t + r + s) % 32) as usize] &= mask;
            }
        }
    }
    for r in 0..repeats {
        for i in 0..elements {
            for s in 0..selem {
                let reg = ((t + r + s) % 32) as usize;
                let at = address.wrapping_add(offset);
                if load {
                    let x = m.load_vector(at, bytes as u8)? as u64;
                    staged[reg] = with_lane(staged[reg], e, i, x);
                } else {
                    m.store_vector(at, bytes as u8, u128::from(lane(c.v[reg], e, i)))?;
                }
                offset += u64::from(bytes);
            }
        }
    }
    if load {
        c.v = staged;
    }
    Ok(offset)
}

// ---- three registers, different widths ----------------------------------------------------
pub const ADDL: u8 = 0;
pub const ADDW: u8 = 1;
pub const SUBL: u8 = 2;
pub const SUBW: u8 = 3;
pub const ABAL: u8 = 4;
pub const ABDL: u8 = 5;
pub const MLAL: u8 = 6;
pub const MLSL: u8 = 7;
pub const MULL: u8 = 8;
pub const ADDHN: u8 = 9;
pub const SUBHN: u8 = 10;
pub const RADDHN: u8 = 11;
pub const RSUBHN: u8 = 12;
/// Widening (`L`), wide (`W`) and high-narrowing (`HN`) operations. `e` is the narrow element
/// size; `upper` selects the upper half of narrow sources ("2" forms). Wide results have
/// `n` lanes of `2e` bits; high-narrowing results are written as by `narrow`.
#[allow(clippy::too_many_arguments)]
pub fn long3(op: u8, e: u32, n: u32, a: u128, b: u128, d: u128, upper: bool, signed: bool) -> u128 {
    let w = 2 * e;
    let half = |v: u128| if upper { v >> 64 } else { v };
    let widen = |x: u64| -> i128 {
        if signed {
            i128::from(sext(x, e))
        } else {
            i128::from(x)
        }
    };
    let wide = |x: u64| -> i128 {
        if signed {
            i128::from(sext(x, w))
        } else {
            i128::from(x)
        }
    };
    if matches!(op, ADDHN | SUBHN | RADDHN | RSUBHN) {
        let mut r = 0u128;
        for i in 0..n {
            let (x, y) = (u128::from(lane(a, w, i)), u128::from(lane(b, w, i)));
            let mut v = if matches!(op, ADDHN | RADDHN) {
                x.wrapping_add(y)
            } else {
                x.wrapping_sub(y)
            };
            if matches!(op, RADDHN | RSUBHN) {
                v = v.wrapping_add(1u128 << (e - 1));
            }
            r = with_lane(r, e, i, (v >> e) as u64);
        }
        return if upper {
            (d & total_mask(64)) | (r << 64)
        } else {
            r
        };
    }
    let mut r = 0u128;
    for i in 0..n {
        let y = widen(lane(half(b), e, i));
        let acc = wide(lane(d, w, i));
        let v = match op {
            ADDL => widen(lane(half(a), e, i)) + y,
            ADDW => wide(lane(a, w, i)) + y,
            SUBL => widen(lane(half(a), e, i)) - y,
            SUBW => wide(lane(a, w, i)) - y,
            ABAL => acc + (widen(lane(half(a), e, i)) - y).abs(),
            ABDL => (widen(lane(half(a), e, i)) - y).abs(),
            MLAL => acc + widen(lane(half(a), e, i)) * y,
            MLSL => acc - widen(lane(half(a), e, i)) * y,
            _ => widen(lane(half(a), e, i)) * y,
        };
        r = with_lane(r, w, i, v as u64);
    }
    r
}

/// MULL/MLAL/MLSL by element: `long3` with the second operand taken from element `index`
/// of `b` (source element size `e`).
#[allow(clippy::too_many_arguments)]
pub fn long3_element(
    op: u8,
    e: u32,
    n: u32,
    a: u128,
    b: u128,
    index: u32,
    d: u128,
    upper: bool,
    signed: bool,
) -> u128 {
    let y = lane(b, e, index);
    let mut splat = 0u128;
    for i in 0..128 / e {
        splat = with_lane(splat, e, i, y);
    }
    long3(op, e, n, a, splat, d, upper, signed)
}
/// SADDLP/UADDLP (and SADALP/UADALP when `accumulate`): adjacent pairs of `e`-bit lanes
/// of `a` added into `n` lanes of `2e` bits.
pub fn add_long_pairwise(e: u32, n: u32, a: u128, d: u128, signed: bool, accumulate: bool) -> u128 {
    let w = 2 * e;
    let mut r = 0u128;
    for i in 0..n {
        let (x, y) = (lane(a, e, 2 * i), lane(a, e, 2 * i + 1));
        let sum = if signed {
            (sext(x, e) as i128 + sext(y, e) as i128) as u64
        } else {
            x.wrapping_add(y)
        };
        let acc = if accumulate { lane(d, w, i) } else { 0 };
        r = with_lane(r, w, i, sum.wrapping_add(acc) & emask(w));
    }
    r
}
