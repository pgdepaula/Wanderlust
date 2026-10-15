//! AArch64 floating-point semantics on raw IEEE-754 bit patterns.
//!
//! Every operation follows the Arm ARM pseudocode (`FPUnpack`, `FPProcessNaNs`, `FPRound`,
//! `FPMulAdd`, `FPCompare`, `FPRoundInt`, `FPToFixed`, `FPConvert`): values are unpacked to an
//! exact `(sign, mantissa: u128, exponent)` form, combined exactly (inexact low bits are
//! "jammed" into a sticky least-significant bit), and rounded once under FPCR control,
//! accumulating FPSR flags. Host floating-point is used only on fast paths whose result is
//! provably identical: FPCR in round-to-nearest without FZ/DN, FPSR.IXC already set (so
//! exactness no longer changes architectural state), non-NaN operands, and a normal result
//! well clear of the underflow and overflow thresholds.
use crate::Cpu;
use std::cmp::Ordering;

pub const IOC: u64 = 1;
pub const DZC: u64 = 1 << 1;
pub const OFC: u64 = 1 << 2;
pub const UFC: u64 = 1 << 3;
pub const IXC: u64 = 1 << 4;
pub const IDC: u64 = 1 << 7;
const FPCR_AHP: u64 = 1 << 26;
const FPCR_DN: u64 = 1 << 25;
const FPCR_FZ: u64 = 1 << 24;

/// An IEEE binary interchange format.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Format {
    pub ebits: u32,
    pub mbits: u32,
}
pub const H: Format = Format {
    ebits: 5,
    mbits: 10,
};
pub const S: Format = Format {
    ebits: 8,
    mbits: 23,
};
pub const D: Format = Format {
    ebits: 11,
    mbits: 52,
};
/// Format for a register size in bits (16, 32, 64).
#[inline]
pub fn format(bits: u32) -> Format {
    match bits {
        16 => H,
        32 => S,
        _ => D,
    }
}
impl Format {
    #[inline]
    fn width(self) -> u32 {
        1 + self.ebits + self.mbits
    }
    #[inline]
    fn bias(self) -> i32 {
        (1 << (self.ebits - 1)) - 1
    }
    #[inline]
    fn emin(self) -> i32 {
        1 - self.bias()
    }
    #[inline]
    fn exp_mask(self) -> u64 {
        ((1u64 << self.ebits) - 1) << self.mbits
    }
    #[inline]
    fn frac_mask(self) -> u64 {
        (1u64 << self.mbits) - 1
    }
    #[inline]
    fn quiet_bit(self) -> u64 {
        1u64 << (self.mbits - 1)
    }
    #[inline]
    fn sign_bit(self) -> u64 {
        1u64 << (self.width() - 1)
    }
    #[inline]
    fn mask(self) -> u64 {
        if self.width() == 64 {
            u64::MAX
        } else {
            (1u64 << self.width()) - 1
        }
    }
    #[inline]
    pub fn default_nan(self) -> u64 {
        self.exp_mask() | self.quiet_bit()
    }
    #[inline]
    pub fn infinity(self, sign: bool) -> u64 {
        self.exp_mask() | if sign { self.sign_bit() } else { 0 }
    }
    #[inline]
    pub fn zero(self, sign: bool) -> u64 {
        if sign {
            self.sign_bit()
        } else {
            0
        }
    }
    #[inline]
    fn max_normal(self, sign: bool) -> u64 {
        (self.exp_mask() - (1u64 << self.mbits)) | self.frac_mask() | self.zero(sign)
    }
    /// FPCR.AHP selects the alternative half-precision format (no infinities or NaNs; the
    /// all-ones exponent encodes ordinary numbers). It affects conversions only.
    #[inline]
    fn alternative(self, fpcr: u64) -> bool {
        self == H && fpcr & FPCR_AHP != 0
    }
    /// Flush-to-zero applies to single and double precision only (ARMv8.0 has no FZ16).
    #[inline]
    fn flushes(self, fpcr: u64) -> bool {
        fpcr & FPCR_FZ != 0 && self != H
    }
}

/// Exact unpacked value: finite values are `(-1)^sign * mant * 2^exp`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Value {
    Zero(bool),
    Inf(bool),
    QNaN,
    SNaN,
    Finite { sign: bool, mant: u128, exp: i32 },
}
impl Value {
    #[inline]
    fn sign(self) -> bool {
        match self {
            Value::Zero(s) | Value::Inf(s) | Value::Finite { sign: s, .. } => s,
            _ => false,
        }
    }
    #[inline]
    fn is_nan(self) -> bool {
        matches!(self, Value::QNaN | Value::SNaN)
    }
}

/// `FPUnpack`: denormal inputs are flushed (setting IDC) when FPCR.FZ applies.
#[inline]
pub fn unpack(f: Format, bits: u64, fpcr: u64, fpsr: &mut u64) -> Value {
    let sign = bits & f.sign_bit() != 0;
    let e = (bits & f.exp_mask()) >> f.mbits;
    let frac = bits & f.frac_mask();
    if e == 0 {
        if frac == 0 {
            Value::Zero(sign)
        } else if f.flushes(fpcr) {
            *fpsr |= IDC;
            Value::Zero(sign)
        } else {
            Value::Finite {
                sign,
                mant: u128::from(frac),
                exp: f.emin() - f.mbits as i32,
            }
        }
    } else if e == (1 << f.ebits) - 1 && !f.alternative(fpcr) {
        if frac == 0 {
            Value::Inf(sign)
        } else if frac & f.quiet_bit() != 0 {
            Value::QNaN
        } else {
            Value::SNaN
        }
    } else {
        Value::Finite {
            sign,
            mant: u128::from(frac | (1 << f.mbits)),
            exp: e as i32 - f.bias() - f.mbits as i32,
        }
    }
}

/// `FPProcessNaN`: quiets a signalling NaN (IOC) and applies default-NaN mode.
#[inline]
fn process_nan(f: Format, bits: u64, signaling: bool, fpcr: u64, fpsr: &mut u64) -> u64 {
    let quiet = if signaling {
        *fpsr |= IOC;
        bits | f.quiet_bit()
    } else {
        bits
    };
    if fpcr & FPCR_DN != 0 {
        f.default_nan()
    } else {
        quiet & f.mask()
    }
}
/// `FPProcessNaNs`/`FPProcessNaNs3`: signalling NaNs take priority, then operand order.
#[inline]
fn process_nans(f: Format, ops: &[(Value, u64)], fpcr: u64, fpsr: &mut u64) -> Option<u64> {
    for want in [Value::SNaN, Value::QNaN] {
        if let Some((_, bits)) = ops.iter().find(|(v, _)| *v == want) {
            return Some(process_nan(f, *bits, want == Value::SNaN, fpcr, fpsr));
        }
    }
    None
}
#[inline]
fn invalid(f: Format, fpsr: &mut u64) -> u64 {
    *fpsr |= IOC;
    f.default_nan()
}

/// `FPRound`: rounds `(-1)^sign * mant * 2^exp` to `f` under FPCR, accumulating FPSR flags.
/// A set least-significant bit in `mant` may be a jammed sticky bit; callers keep at least
/// two bits below the rounding position whenever they jam.
pub fn round(f: Format, sign: bool, mant: u128, exp: i32, fpcr: u64, fpsr: &mut u64) -> u64 {
    if mant == 0 {
        return f.zero(sign);
    }
    let msb = 127 - mant.leading_zeros() as i32;
    let unrounded_exp = msb + exp;
    let emin = f.emin();
    if f.flushes(fpcr) && unrounded_exp < emin {
        *fpsr |= UFC;
        return f.zero(sign);
    }
    let p = f.mbits as i32 + 1;
    let lsb_exp = (unrounded_exp - (p - 1)).max(emin - f.mbits as i32);
    let shift = lsb_exp - exp;
    let (mut q, inexact, versus_half) = split(mant, shift);
    let round_up = match (fpcr >> 22) & 3 {
        0 => versus_half == Ordering::Greater || (versus_half == Ordering::Equal && q & 1 == 1),
        1 => inexact && !sign,
        2 => inexact && sign,
        _ => false,
    };
    if round_up {
        q += 1;
    }
    let mut result_exp = lsb_exp;
    if q >> p != 0 {
        q >>= 1;
        result_exp += 1;
    }
    let biased = if q >> (p - 1) == 0 {
        0
    } else {
        result_exp + f.mbits as i32 + f.bias()
    };
    if f.alternative(fpcr) && biased >= 1 << f.ebits {
        // Alternative half precision saturates with Invalid Operation; the pseudocode clears
        // the rounding error, so neither Inexact nor Underflow is raised.
        *fpsr |= IOC;
        return f.zero(sign) | 0x7fff;
    }
    if inexact {
        *fpsr |= IXC;
        if unrounded_exp < emin {
            *fpsr |= UFC;
        }
    }
    if f.alternative(fpcr) {
        return f.zero(sign) | ((biased as u64) << f.mbits) | (q as u64 & f.frac_mask());
    }
    if biased >= (1 << f.ebits) - 1 {
        *fpsr |= OFC | IXC;
        let to_infinity = match (fpcr >> 22) & 3 {
            0 => true,
            1 => !sign,
            2 => sign,
            _ => false,
        };
        return if to_infinity {
            f.infinity(sign)
        } else {
            f.max_normal(sign)
        };
    }
    f.zero(sign) | ((biased as u64) << f.mbits) | (q as u64 & f.frac_mask())
}

/// Splits `mant` at bit `shift`: (quotient, discarded bits non-zero, discarded part versus
/// one half of the quotient's unit). `shift <= 0` is exact.
#[inline]
fn split(mant: u128, shift: i32) -> (u128, bool, Ordering) {
    if shift <= 0 {
        (mant << (-shift) as u32, false, Ordering::Less)
    } else if shift > 128 {
        (0, mant != 0, Ordering::Less)
    } else if shift == 128 {
        (0, mant != 0, mant.cmp(&(1u128 << 127)))
    } else {
        let rem = mant & ((1u128 << shift as u32) - 1);
        (
            mant >> shift as u32,
            rem != 0,
            rem.cmp(&(1u128 << (shift as u32 - 1))),
        )
    }
}
/// Normalizes a non-zero mantissa so its most significant bit is at `bit`.
#[inline]
fn normalize(mant: u128, exp: i32, bit: u32) -> (u128, i32) {
    let msb = 127 - mant.leading_zeros();
    if msb <= bit {
        (mant << (bit - msb), exp - (bit - msb) as i32)
    } else {
        let s = msb - bit;
        let lost = mant & ((1u128 << s) - 1) != 0;
        ((mant >> s) | lost as u128, exp + s as i32)
    }
}
/// Exact (up to a jammed sticky bit) sum of two finite non-zero values.
fn exact_add(a: (bool, u128, i32), b: (bool, u128, i32)) -> (bool, u128, i32) {
    let (sa, ma, ea) = a;
    let (sb, mb, eb) = b;
    let (ma, ea) = normalize(ma, ea, 124);
    let (mb, eb) = normalize(mb, eb, 124);
    let ((sa, ma, ea), (sb, mb, eb)) = if (ea, ma) >= (eb, mb) {
        ((sa, ma, ea), (sb, mb, eb))
    } else {
        ((sb, mb, eb), (sa, ma, ea))
    };
    let d = (ea - eb) as u32;
    let mb = if d == 0 {
        mb
    } else if d >= 128 {
        1
    } else {
        (mb >> d) | ((mb & ((1u128 << d) - 1)) != 0) as u128
    };
    if sa == sb {
        (sa, ma + mb, ea)
    } else if ma >= mb {
        (sa, ma - mb, ea)
    } else {
        (sb, mb - ma, ea)
    }
}
#[inline]
fn finite(v: Value) -> (bool, u128, i32) {
    match v {
        Value::Finite { sign, mant, exp } => (sign, mant, exp),
        _ => (false, 0, 0),
    }
}
#[inline]
fn round_mode(fpcr: u64) -> u64 {
    (fpcr >> 22) & 3
}

// ---- host fast paths ----------------------------------------------------------------------

/// True when host IEEE arithmetic in round-to-nearest cannot differ observably.
#[inline]
fn fast(fpcr: u64, fpsr: u64) -> bool {
    fpcr & (FPCR_FZ | FPCR_DN | (3 << 22)) == 0 && fpsr & IXC != 0
}
#[inline]
fn fast_result_ok(f: Format, r: u64) -> bool {
    // Normal, finite, strictly above the smallest normal (so no tininess before rounding).
    let e = (r & f.exp_mask()) >> f.mbits;
    e > 1 && e < (1 << f.ebits) - 1
}
#[inline]
fn host2(f: Format, a: u64, b: u64, op: u8) -> Option<u64> {
    let quiet_ok = |bits: u64| (bits & f.exp_mask()) != f.exp_mask();
    if !quiet_ok(a) || !quiet_ok(b) {
        return None;
    }
    let r = if f == D {
        let (x, y) = (f64::from_bits(a), f64::from_bits(b));
        (match op {
            0 => x + y,
            1 => x - y,
            2 => x * y,
            _ => x / y,
        })
        .to_bits()
    } else if f == S {
        let (x, y) = (f32::from_bits(a as u32), f32::from_bits(b as u32));
        u64::from(
            (match op {
                0 => x + y,
                1 => x - y,
                2 => x * y,
                _ => x / y,
            })
            .to_bits(),
        )
    } else {
        return None;
    };
    fast_result_ok(f, r).then_some(r)
}

// ---- arithmetic -----------------------------------------------------------------------------

/// FADD (`negate = false`) and FSUB (`negate = true`). NaN operands are processed before the
/// subtrahend is negated, as in `FPSub`.
pub fn add(c: &mut Cpu, f: Format, a: u64, b: u64, negate: bool) -> u64 {
    if fast(c.fpcr, c.fpsr) {
        if let Some(r) = host2(f, a, b, negate as u8) {
            return r;
        }
    }
    let (fpcr, fpsr) = (c.fpcr, &mut c.fpsr);
    let va = unpack(f, a, fpcr, fpsr);
    let mut vb = unpack(f, b, fpcr, fpsr);
    if let Some(n) = process_nans(f, &[(va, a), (vb, b)], fpcr, fpsr) {
        return n;
    }
    if negate {
        vb = match vb {
            Value::Zero(s) => Value::Zero(!s),
            Value::Inf(s) => Value::Inf(!s),
            Value::Finite { sign, mant, exp } => Value::Finite {
                sign: !sign,
                mant,
                exp,
            },
            other => other,
        };
    }
    match (va, vb) {
        (Value::Inf(x), Value::Inf(y)) if x != y => invalid(f, fpsr),
        (Value::Inf(x), _) | (_, Value::Inf(x)) => f.infinity(x),
        (Value::Zero(x), Value::Zero(y)) => f.zero(if x == y { x } else { round_mode(fpcr) == 2 }),
        (Value::Zero(_), Value::Finite { .. }) | (Value::Finite { .. }, Value::Zero(_)) => {
            let v = if matches!(va, Value::Zero(_)) { vb } else { va };
            let (sign, mant, exp) = finite(v);
            round(f, sign, mant, exp, fpcr, fpsr)
        }
        _ => {
            let (sign, mant, exp) = exact_add(finite(va), finite(vb));
            if mant == 0 {
                f.zero(round_mode(fpcr) == 2)
            } else {
                round(f, sign, mant, exp, fpcr, fpsr)
            }
        }
    }
}
/// FMUL (and FNMUL with `negate`, which negates the rounded product as `FPNeg(FPMul())`).
pub fn mul(c: &mut Cpu, f: Format, a: u64, b: u64, negate: bool) -> u64 {
    let r = mul_inner(c, f, a, b);
    // FNMUL is FPNeg(FPMul(...)): the sign flips for every result, NaNs included.
    if negate {
        r ^ f.sign_bit()
    } else {
        r
    }
}
fn mul_inner(c: &mut Cpu, f: Format, a: u64, b: u64) -> u64 {
    if fast(c.fpcr, c.fpsr) {
        if let Some(r) = host2(f, a, b, 2) {
            return r;
        }
    }
    let (fpcr, fpsr) = (c.fpcr, &mut c.fpsr);
    let va = unpack(f, a, fpcr, fpsr);
    let vb = unpack(f, b, fpcr, fpsr);
    if let Some(n) = process_nans(f, &[(va, a), (vb, b)], fpcr, fpsr) {
        return n;
    }
    let sign = va.sign() != vb.sign();
    match (va, vb) {
        (Value::Inf(_), Value::Zero(_)) | (Value::Zero(_), Value::Inf(_)) => invalid(f, fpsr),
        (Value::Inf(_), _) | (_, Value::Inf(_)) => f.infinity(sign),
        (Value::Zero(_), _) | (_, Value::Zero(_)) => f.zero(sign),
        _ => {
            let (_, ma, ea) = finite(va);
            let (_, mb, eb) = finite(vb);
            round(f, sign, ma * mb, ea + eb, fpcr, fpsr)
        }
    }
}
/// FMULX: as FMUL, except infinity times zero yields 2.0 with the product's sign.
pub fn mulx(c: &mut Cpu, f: Format, a: u64, b: u64) -> u64 {
    let (fpcr, mut flags) = (c.fpcr, 0);
    let va = unpack(f, a, fpcr, &mut flags);
    let vb = unpack(f, b, fpcr, &mut flags);
    if !va.is_nan() && !vb.is_nan() {
        if let (Value::Inf(_), Value::Zero(_)) | (Value::Zero(_), Value::Inf(_)) = (va, vb) {
            c.fpsr |= flags;
            let two = round(f, va.sign() != vb.sign(), 2, 0, fpcr, &mut c.fpsr);
            return two;
        }
    }
    mul(c, f, a, b, false)
}
pub fn div(c: &mut Cpu, f: Format, a: u64, b: u64) -> u64 {
    if fast(c.fpcr, c.fpsr) {
        if let Some(r) = host2(f, a, b, 3) {
            return r;
        }
    }
    let (fpcr, fpsr) = (c.fpcr, &mut c.fpsr);
    let va = unpack(f, a, fpcr, fpsr);
    let vb = unpack(f, b, fpcr, fpsr);
    if let Some(n) = process_nans(f, &[(va, a), (vb, b)], fpcr, fpsr) {
        return n;
    }
    let sign = va.sign() != vb.sign();
    match (va, vb) {
        (Value::Inf(_), Value::Inf(_)) | (Value::Zero(_), Value::Zero(_)) => invalid(f, fpsr),
        (Value::Inf(_), _) | (_, Value::Zero(_)) => {
            if matches!(vb, Value::Zero(_)) && !matches!(va, Value::Inf(_)) {
                *fpsr |= DZC;
            }
            f.infinity(sign)
        }
        (Value::Zero(_), _) | (_, Value::Inf(_)) => f.zero(sign),
        _ => {
            let (_, ma, ea) = finite(va);
            let (_, mb, eb) = finite(vb);
            let (ma, ea) = normalize(ma, ea, 125);
            let (mb, eb) = normalize(mb, eb, 63);
            let q = ma / mb;
            let sticky = (ma % mb != 0) as u128;
            round(f, sign, q | sticky, ea - eb, fpcr, fpsr)
        }
    }
}
fn isqrt(n: u128) -> u128 {
    // Digit-by-digit square root; exact floor for any u128.
    let mut x = n;
    let mut result = 0u128;
    let mut bit = 1u128 << 126;
    while bit > n {
        bit >>= 2;
    }
    while bit != 0 {
        if x >= result + bit {
            x -= result + bit;
            result = (result >> 1) + bit;
        } else {
            result >>= 1;
        }
        bit >>= 2;
    }
    result
}
pub fn sqrt(c: &mut Cpu, f: Format, a: u64) -> u64 {
    if f != H && fast(c.fpcr, c.fpsr) && a & f.sign_bit() == 0 && fast_result_ok(f, a) {
        // The square root of a positive normal number is normal.
        return if f == D {
            f64::from_bits(a).sqrt().to_bits()
        } else {
            u64::from(f32::from_bits(a as u32).sqrt().to_bits())
        };
    }
    let (fpcr, fpsr) = (c.fpcr, &mut c.fpsr);
    let v = unpack(f, a, fpcr, fpsr);
    match v {
        Value::QNaN | Value::SNaN => process_nan(f, a, v == Value::SNaN, fpcr, fpsr),
        Value::Zero(s) => f.zero(s),
        Value::Inf(false) => f.infinity(false),
        Value::Inf(true) | Value::Finite { sign: true, .. } => invalid(f, fpsr),
        Value::Finite { mant, exp, .. } => {
            let (mut m, mut e) = normalize(mant, exp, 124);
            if e & 1 != 0 {
                m <<= 1;
                e -= 1;
            }
            let r = isqrt(m);
            let sticky = (r * r != m) as u128;
            round(f, false, r | sticky, e / 2, fpcr, fpsr)
        }
    }
}
/// `FPMulAdd(addend, op1, op2)`: `addend + op1 * op2` with a single rounding. FMSUB, FNMADD
/// and FNMSUB negate operands (bit patterns, NaNs included) before calling this.
pub fn mul_add(c: &mut Cpu, f: Format, addend: u64, op1: u64, op2: u64) -> u64 {
    let (fpcr, fpsr) = (c.fpcr, &mut c.fpsr);
    let va = unpack(f, addend, fpcr, fpsr);
    let v1 = unpack(f, op1, fpcr, fpsr);
    let v2 = unpack(f, op2, fpcr, fpsr);
    let inf_times_zero = matches!(
        (v1, v2),
        (Value::Inf(_), Value::Zero(_)) | (Value::Zero(_), Value::Inf(_))
    );
    if let Some(n) = process_nans(f, &[(va, addend), (v1, op1), (v2, op2)], fpcr, fpsr) {
        if va == Value::QNaN && inf_times_zero {
            return invalid(f, fpsr);
        }
        return n;
    }
    let sign_p = v1.sign() != v2.sign();
    let inf_p = matches!(v1, Value::Inf(_)) || matches!(v2, Value::Inf(_));
    let zero_p = matches!(v1, Value::Zero(_)) || matches!(v2, Value::Zero(_));
    if inf_times_zero || (matches!(va, Value::Inf(_)) && inf_p && va.sign() != sign_p) {
        return invalid(f, fpsr);
    }
    if let Value::Inf(s) = va {
        return f.infinity(s);
    }
    if inf_p {
        return f.infinity(sign_p);
    }
    if let Value::Zero(sa) = va {
        if zero_p {
            return f.zero(if sa == sign_p {
                sa
            } else {
                round_mode(fpcr) == 2
            });
        }
    }
    if zero_p {
        let (s, m, e) = finite(va);
        return round(f, s, m, e, fpcr, fpsr);
    }
    let (_, m1, e1) = finite(v1);
    let (_, m2, e2) = finite(v2);
    let product = (sign_p, m1 * m2, e1 + e2);
    let (sign, mant, exp) = if matches!(va, Value::Zero(_)) {
        product
    } else {
        exact_add(finite(va), product)
    };
    if mant == 0 {
        return f.zero(round_mode(fpcr) == 2);
    }
    round(f, sign, mant, exp, fpcr, fpsr)
}
/// FRECPS (`FPRecipStepFused`): `2.0 - op1 * op2` with one rounding; infinity times zero
/// yields 2.0.
pub fn recip_step(c: &mut Cpu, f: Format, a: u64, b: u64) -> u64 {
    step_fused(c, f, a, b, false)
}
/// FRSQRTS (`FPRSqrtStepFused`): `(3.0 - op1 * op2) / 2` with one rounding; infinity times
/// zero yields 1.5.
pub fn rsqrt_step(c: &mut Cpu, f: Format, a: u64, b: u64) -> u64 {
    step_fused(c, f, a, b, true)
}
fn step_fused(c: &mut Cpu, f: Format, a: u64, b: u64, rsqrt: bool) -> u64 {
    let (fpcr, fpsr) = (c.fpcr, &mut c.fpsr);
    // op1 is negated before NaN processing, as in the pseudocode.
    let a = neg(f, a);
    let v1 = unpack(f, a, fpcr, fpsr);
    let v2 = unpack(f, b, fpcr, fpsr);
    if let Some(n) = process_nans(f, &[(v1, a), (v2, b)], fpcr, fpsr) {
        return n;
    }
    let (constant, scale) = if rsqrt { (3u128, -1) } else { (2u128, 0) };
    match (v1, v2) {
        (Value::Inf(_), Value::Zero(_)) | (Value::Zero(_), Value::Inf(_)) => {
            round(f, false, constant, scale, 0, &mut 0)
        }
        (Value::Inf(_), _) | (_, Value::Inf(_)) => f.infinity(v1.sign() != v2.sign()),
        (Value::Zero(_), _) | (_, Value::Zero(_)) => round(f, false, constant, scale, fpcr, fpsr),
        _ => {
            let (s1, m1, e1) = finite(v1);
            let (s2, m2, e2) = finite(v2);
            let (sign, mant, exp) = exact_add((false, constant, 0), (s1 != s2, m1 * m2, e1 + e2));
            if mant == 0 {
                f.zero(round_mode(fpcr) == 2)
            } else {
                round(f, sign, mant, exp + scale, fpcr, fpsr)
            }
        }
    }
}
#[inline]
fn is_nan_bits(f: Format, bits: u64) -> bool {
    bits & f.exp_mask() == f.exp_mask() && bits & f.frac_mask() != 0
}

/// FMAX/FMIN (`numeric = false`) and FMAXNM/FMINNM (`numeric = true`).
pub fn min_max(c: &mut Cpu, f: Format, a: u64, b: u64, max: bool, numeric: bool) -> u64 {
    let (fpcr, fpsr) = (c.fpcr, &mut c.fpsr);
    let (mut a, mut b) = (a, b);
    if numeric {
        let qa = is_nan_bits(f, a) && a & f.quiet_bit() != 0;
        let qb = is_nan_bits(f, b) && b & f.quiet_bit() != 0;
        // A quiet NaN facing a number is replaced by the infinity that loses the comparison.
        if qa && !qb {
            a = f.infinity(max);
        } else if qb && !qa {
            b = f.infinity(max);
        }
    }
    let va = unpack(f, a, fpcr, fpsr);
    let vb = unpack(f, b, fpcr, fpsr);
    if let Some(n) = process_nans(f, &[(va, a), (vb, b)], fpcr, fpsr) {
        return n;
    }
    if let (Value::Zero(x), Value::Zero(y)) = (va, vb) {
        return f.zero(if max { x && y } else { x || y });
    }
    let a_greater = compare_values(va, vb) == Ordering::Greater;
    let (pick, bits) = if a_greater == max { (va, a) } else { (vb, b) };
    match pick {
        Value::Zero(s) => f.zero(s),
        Value::Inf(s) => f.infinity(s),
        _ => bits,
    }
}
/// Orders two non-NaN unpacked values.
fn compare_values(a: Value, b: Value) -> Ordering {
    let key = |v: Value| -> (i8, bool, u128, i32) {
        match v {
            Value::Inf(true) => (-2, false, 0, 0),
            Value::Finite {
                sign: true,
                mant,
                exp,
            } => (-1, true, mant, exp),
            Value::Zero(_) => (0, false, 0, 0),
            Value::Finite {
                sign: false,
                mant,
                exp,
            } => (1, false, mant, exp),
            _ => (2, false, 0, 0),
        }
    };
    let (ka, kb) = (key(a), key(b));
    if ka.0 != kb.0 || ka.0.abs() != 1 {
        return ka.0.cmp(&kb.0);
    }
    let (ma, ea) = normalize(ka.2, ka.3, 120);
    let (mb, eb) = normalize(kb.2, kb.3, 120);
    let magnitude = (ea, ma).cmp(&(eb, mb));
    if ka.1 {
        magnitude.reverse()
    } else {
        magnitude
    }
}
/// `FPCompare` as NZCV: unordered 0011, equal 0110, less 1000, greater 0010. `signal`
/// (FCMPE) raises IOC for any NaN; otherwise only signalling NaNs raise it.
pub fn compare(c: &mut Cpu, f: Format, a: u64, b: u64, signal: bool) -> u8 {
    let (fpcr, fpsr) = (c.fpcr, &mut c.fpsr);
    let va = unpack(f, a, fpcr, fpsr);
    let vb = unpack(f, b, fpcr, fpsr);
    if va.is_nan() || vb.is_nan() {
        if signal || va == Value::SNaN || vb == Value::SNaN {
            *fpsr |= IOC;
        }
        return 0b0011;
    }
    match compare_values(va, vb) {
        Ordering::Equal => 0b0110,
        Ordering::Less => 0b1000,
        Ordering::Greater => 0b0010,
    }
}
/// Vector/scalar compare producing a mask: EQ (`kind` 0), GE (1), GT (2); `absolute` for
/// FACGE/FACGT. EQ signals only on signalling NaNs; GE and GT signal on any NaN.
pub fn compare_mask(c: &mut Cpu, f: Format, a: u64, b: u64, kind: u8, absolute: bool) -> u64 {
    let (a, b) = if absolute {
        (a & !f.sign_bit(), b & !f.sign_bit())
    } else {
        (a, b)
    };
    let (fpcr, fpsr) = (c.fpcr, &mut c.fpsr);
    let va = unpack(f, a, fpcr, fpsr);
    let vb = unpack(f, b, fpcr, fpsr);
    if va.is_nan() || vb.is_nan() {
        if kind != 0 || va == Value::SNaN || vb == Value::SNaN {
            *fpsr |= IOC;
        }
        return 0;
    }
    let ord = compare_values(va, vb);
    let holds = match kind {
        0 => ord == Ordering::Equal,
        1 => ord != Ordering::Less,
        _ => ord == Ordering::Greater,
    };
    if holds {
        f.mask()
    } else {
        0
    }
}

/// Rounding selection for integral rounding and conversions.
pub const ROUND_NEAREST_EVEN: u8 = 0;
pub const ROUND_PLUS: u8 = 1;
pub const ROUND_MINUS: u8 = 2;
pub const ROUND_ZERO: u8 = 3;
pub const ROUND_NEAREST_AWAY: u8 = 4;
/// Use FPCR.RMode.
pub const ROUND_FPCR: u8 = 5;
#[inline]
fn resolve(mode: u8, fpcr: u64) -> u8 {
    if mode == ROUND_FPCR {
        round_mode(fpcr) as u8
    } else {
        mode
    }
}
/// Rounds `mant * 2^exp` (exp < 0) to an integer magnitude; returns (integer, inexact).
fn to_integer(sign: bool, mant: u128, exp: i32, mode: u8) -> (u128, bool) {
    if exp >= 0 {
        // Saturates to u128::MAX when the integer would not fit; callers range-check.
        let fits = exp < 128 && (mant.leading_zeros() as i32) >= exp;
        return (if fits { mant << exp as u32 } else { u128::MAX }, false);
    }
    let (q, inexact, versus_half) = split(mant, -exp);
    let up = match mode {
        ROUND_NEAREST_EVEN => {
            versus_half == Ordering::Greater || (versus_half == Ordering::Equal && q & 1 == 1)
        }
        ROUND_NEAREST_AWAY => inexact && versus_half != Ordering::Less,
        ROUND_PLUS => inexact && !sign,
        ROUND_MINUS => inexact && sign,
        _ => false,
    };
    (q + up as u128, inexact)
}
/// `FPRoundInt`: FRINTN/P/M/Z/A use their own rounding; FRINTI uses FPCR; FRINTX uses FPCR
/// and signals IXC when the result differs from the operand.
pub fn round_int(c: &mut Cpu, f: Format, a: u64, mode: u8, exact: bool) -> u64 {
    let (fpcr, fpsr) = (c.fpcr, &mut c.fpsr);
    let v = unpack(f, a, fpcr, fpsr);
    match v {
        Value::QNaN | Value::SNaN => process_nan(f, a, v == Value::SNaN, fpcr, fpsr),
        Value::Zero(s) => f.zero(s),
        Value::Inf(s) => f.infinity(s),
        // With a non-negative exponent the value is already integral.
        Value::Finite { exp, .. } if exp >= 0 => a,
        Value::Finite { sign, mant, exp } => {
            let (int, inexact) = to_integer(sign, mant, exp, resolve(mode, fpcr));
            if exact && inexact {
                *fpsr |= IXC;
            }
            if int == 0 {
                f.zero(sign)
            } else {
                round(f, sign, int, 0, 0, &mut 0)
            }
        }
    }
}
/// `FPToFixed` with no fraction bits: saturating conversion to a signed or unsigned integer
/// of `bits` width. NaN converts to 0; out-of-range values saturate; both raise IOC.
pub fn to_int(c: &mut Cpu, f: Format, a: u64, bits: u32, signed: bool, mode: u8) -> u64 {
    to_fixed(c, f, a, bits, signed, mode, 0)
}
/// `FPToFixed` with `fbits` fraction bits (FCVTZS/FCVTZU fixed-point forms).
pub fn to_fixed(
    c: &mut Cpu,
    f: Format,
    a: u64,
    bits: u32,
    signed: bool,
    mode: u8,
    fbits: u32,
) -> u64 {
    let (fpcr, fpsr) = (c.fpcr, &mut c.fpsr);
    let v = unpack(f, a, fpcr, fpsr);
    let max_pos: u128 = if signed {
        (1u128 << (bits - 1)) - 1
    } else {
        (1u128 << bits) - 1
    };
    let max_neg: u128 = if signed { 1u128 << (bits - 1) } else { 0 };
    let mask = if bits == 64 {
        u64::MAX
    } else {
        (1u64 << bits) - 1
    };
    let saturate = |sign: bool, fpsr: &mut u64| -> u64 {
        *fpsr |= IOC;
        if sign {
            (max_neg as u64).wrapping_neg() & mask
        } else {
            max_pos as u64
        }
    };
    match v {
        Value::QNaN | Value::SNaN => {
            *fpsr |= IOC;
            0
        }
        Value::Zero(_) => 0,
        Value::Inf(s) => saturate(s, fpsr),
        Value::Finite { sign, mant, exp } => {
            let (int, inexact) = to_integer(sign, mant, exp + fbits as i32, resolve(mode, fpcr));
            let limit = if sign { max_neg } else { max_pos };
            if int > limit {
                return saturate(sign, fpsr);
            }
            if inexact {
                *fpsr |= IXC;
            }
            if sign {
                (int as u64).wrapping_neg() & mask
            } else {
                int as u64
            }
        }
    }
}
/// SCVTF/UCVTF of a `bits`-wide integer, with `fbits` fraction bits.
pub fn from_int(c: &mut Cpu, f: Format, value: u64, bits: u32, signed: bool, fbits: u32) -> u64 {
    let value = if bits == 64 {
        value
    } else {
        value & ((1u64 << bits) - 1)
    };
    let negative = signed && (value >> (bits - 1)) & 1 == 1;
    let magnitude = if negative {
        (value.wrapping_neg())
            & if bits == 64 {
                u64::MAX
            } else {
                (1u64 << bits) - 1
            }
    } else {
        value
    };
    let magnitude = u128::from(magnitude);
    if fast(c.fpcr, c.fpsr) && fbits == 0 && f != H {
        let r = match (f == D, signed, bits) {
            (true, true, 64) => (value as i64 as f64).to_bits(),
            (true, false, 64) => (value as f64).to_bits(),
            (true, true, _) => (value as u32 as i32 as f64).to_bits(),
            (true, false, _) => (value as u32 as f64).to_bits(),
            (false, true, 64) => u64::from((value as i64 as f32).to_bits()),
            (false, false, 64) => u64::from((value as f32).to_bits()),
            (false, true, _) => u64::from((value as u32 as i32 as f32).to_bits()),
            (false, false, _) => u64::from((value as u32 as f32).to_bits()),
        };
        return r;
    }
    round(f, negative, magnitude, -(fbits as i32), c.fpcr, &mut c.fpsr)
}
/// `FPConvert` between formats (FCVT). NaNs keep sign and the high fraction bits.
pub fn convert(c: &mut Cpu, from: Format, to: Format, a: u64) -> crate::RuntimeResult<u64> {
    let (fpcr, fpsr) = (c.fpcr, &mut c.fpsr);
    let v = unpack(from, a, fpcr, fpsr);
    let alternative = to.alternative(fpcr);
    Ok(match v {
        // The alternative format has no NaN or infinity: zero / largest value, Invalid Operation.
        Value::QNaN | Value::SNaN if alternative => {
            *fpsr |= IOC;
            to.zero(a & from.sign_bit() != 0)
        }
        Value::Inf(s) if alternative => {
            *fpsr |= IOC;
            to.zero(s) | 0x7fff
        }
        Value::QNaN | Value::SNaN => {
            if v == Value::SNaN {
                *fpsr |= IOC;
            }
            if fpcr & FPCR_DN != 0 {
                to.default_nan()
            } else {
                let frac = a & from.frac_mask();
                let frac = if to.mbits >= from.mbits {
                    frac << (to.mbits - from.mbits)
                } else {
                    frac >> (from.mbits - to.mbits)
                };
                to.zero(a & from.sign_bit() != 0) | to.exp_mask() | to.quiet_bit() | frac
            }
        }
        Value::Zero(s) => to.zero(s),
        Value::Inf(s) => to.infinity(s),
        Value::Finite { sign, mant, exp } => round(to, sign, mant, exp, fpcr, fpsr),
    })
}
/// FABS/FNEG/FMOV are pure bit operations: no NaN processing and no flags.
#[inline]
pub fn abs(f: Format, a: u64) -> u64 {
    a & !f.sign_bit()
}
#[inline]
pub fn neg(f: Format, a: u64) -> u64 {
    (a ^ f.sign_bit()) & f.mask()
}
/// `VFPExpandImm`: the 8-bit FMOV immediate as a value of `f`.
pub fn expand_imm(f: Format, imm8: u8) -> u64 {
    let sign = u64::from(imm8 >> 7);
    let b6 = u64::from((imm8 >> 6) & 1);
    let low = u64::from(imm8 & 0x3f);
    let e = f.ebits;
    // exp = NOT(b6) : Replicate(b6, E-3) : imm8<5:4>
    let exp = ((1 - b6) << (e - 1))
        | (if b6 == 1 {
            ((1u64 << (e - 3)) - 1) << 2
        } else {
            0
        })
        | (low >> 4);
    let frac = (low & 15) << (f.mbits - 4);
    (sign << (e + f.mbits)) | (exp << f.mbits) | frac
}
