//! Floating-point semantics checked against values derived from IEEE 754 and the Arm ARM
//! pseudocode. The differential suite compares the same code with Unicorn over many states.
use wl_runtime::{fp, simd, Cpu};

const RN: u64 = 0;
const RP: u64 = 1 << 22;
const RM: u64 = 2 << 22;
const RZ: u64 = 3 << 22;
const FZ: u64 = 1 << 24;
const DN: u64 = 1 << 25;
fn cpu(fpcr: u64) -> Cpu {
    let mut c = Cpu::new(0);
    c.fpcr = fpcr;
    c
}
fn d(x: f64) -> u64 {
    x.to_bits()
}
const SNAN_D: u64 = 0x7ff0_0000_0000_0001;
const QNAN_D: u64 = 0x7ff8_0000_0000_0001;

#[test]
fn nearest_rounding_and_inexact_flag() {
    let mut c = cpu(RN);
    assert_eq!(
        fp::add(&mut c, fp::D, d(0.1), d(0.2), false),
        0x3fd3_3333_3333_3334
    );
    assert_eq!(c.fpsr, fp::IXC);
    let mut c = cpu(RN);
    assert_eq!(fp::add(&mut c, fp::D, d(1.5), d(2.25), false), d(3.75));
    assert_eq!(c.fpsr, 0, "exact results raise no flags");
}
#[test]
fn directed_rounding_of_one_third() {
    // 1/3 = 0x3FD5555555555555.55..., so RN and RZ/RM round down, RP rounds up.
    for (mode, bits) in [
        (RN, 0x3fd5_5555_5555_5555),
        (RZ, 0x3fd5_5555_5555_5555),
        (RM, 0x3fd5_5555_5555_5555),
        (RP, 0x3fd5_5555_5555_5556),
    ] {
        let mut c = cpu(mode);
        assert_eq!(
            fp::div(&mut c, fp::D, d(1.0), d(3.0)),
            bits,
            "mode {mode:#x}"
        );
        assert_eq!(c.fpsr, fp::IXC);
    }
    // -1/3 rounds toward minus infinity in RM (magnitude up).
    let mut c = cpu(RM);
    assert_eq!(
        fp::div(&mut c, fp::D, d(-1.0), d(3.0)),
        0xbfd5_5555_5555_5556
    );
}
#[test]
fn invalid_operations_produce_the_default_nan() {
    let mut c = cpu(RN);
    assert_eq!(
        fp::add(&mut c, fp::D, d(f64::INFINITY), d(f64::NEG_INFINITY), false),
        0x7ff8_0000_0000_0000
    );
    assert_eq!(c.fpsr, fp::IOC);
    let mut c = cpu(RN);
    assert_eq!(fp::mul(&mut c, fp::S, 0x7f80_0000, 0, false), 0x7fc0_0000);
    let mut c = cpu(RN);
    assert_eq!(fp::sqrt(&mut c, fp::D, d(-4.0)), 0x7ff8_0000_0000_0000);
    let mut c = cpu(RN);
    assert_eq!(
        fp::div(&mut c, fp::D, d(1.0), d(-0.0)),
        d(f64::NEG_INFINITY)
    );
    assert_eq!(c.fpsr, fp::DZC);
}
#[test]
fn nan_propagation_quiets_signalling_nans_first() {
    // A signalling NaN wins over an earlier quiet NaN and is returned quieted with IOC.
    let mut c = cpu(RN);
    assert_eq!(
        fp::add(&mut c, fp::D, QNAN_D, SNAN_D, false),
        0x7ff8_0000_0000_0001
    );
    assert_eq!(c.fpsr, fp::IOC);
    // FSUB processes NaNs before negating: the NaN keeps its sign.
    let mut c = cpu(RN);
    assert_eq!(fp::add(&mut c, fp::D, d(1.0), QNAN_D, true), QNAN_D);
    // Default-NaN mode replaces the payload.
    let mut c = cpu(DN);
    assert_eq!(
        fp::add(&mut c, fp::D, d(1.0), 0xfff8_0000_0000_dead, false),
        0x7ff8_0000_0000_0000
    );
}
#[test]
fn flush_to_zero_inputs_and_outputs() {
    let tiny = 1u64; // smallest double subnormal
    let mut c = cpu(FZ);
    assert_eq!(fp::add(&mut c, fp::D, tiny, d(1.0), false), d(1.0));
    assert_eq!(c.fpsr, fp::IDC, "denormal input flushed");
    // MinNormal * 0.5 is tiny before rounding: flushed to +0 with UFC and without IXC.
    let mut c = cpu(FZ);
    assert_eq!(
        fp::mul(&mut c, fp::D, 0x0010_0000_0000_0000, d(0.5), false),
        0
    );
    assert_eq!(c.fpsr, fp::UFC);
    // Without FZ the same product is an exact subnormal: no flags.
    let mut c = cpu(RN);
    assert_eq!(
        fp::mul(&mut c, fp::D, 0x0010_0000_0000_0000, d(0.5), false),
        0x0008_0000_0000_0000
    );
    assert_eq!(c.fpsr, 0);
}
#[test]
fn underflow_is_detected_before_rounding() {
    // (MinNormal - ulp/2) rounds up to MinNormal in RN but was tiny: UFC and IXC.
    // x = 0x000FFFFFFFFFFFFF + 0.5 ulp = (2^53 - 1) * 2^-1075.
    let mut c = cpu(RN);
    let r = fp::round(fp::D, false, (1u128 << 53) - 1, -1075, c.fpcr, &mut c.fpsr);
    assert_eq!(r, 0x0010_0000_0000_0000);
    assert_eq!(c.fpsr, fp::UFC | fp::IXC);
}
#[test]
fn overflow_depends_on_rounding_mode() {
    let max = d(f64::MAX);
    for (mode, expected) in [
        (RN, d(f64::INFINITY)),
        (RZ, max),
        (RM, max),
        (RP, d(f64::INFINITY)),
    ] {
        let mut c = cpu(mode);
        assert_eq!(fp::mul(&mut c, fp::D, max, d(2.0), false), expected);
        assert_eq!(c.fpsr, fp::OFC | fp::IXC);
    }
}
#[test]
fn conversions_saturate_and_signal() {
    let mut c = cpu(RN);
    assert_eq!(
        fp::to_int(&mut c, fp::D, d(3.0e10), 32, true, fp::ROUND_ZERO),
        0x7fff_ffff
    );
    assert_eq!(c.fpsr, fp::IOC);
    let mut c = cpu(RN);
    assert_eq!(
        fp::to_int(&mut c, fp::D, QNAN_D, 64, true, fp::ROUND_ZERO),
        0
    );
    assert_eq!(c.fpsr, fp::IOC);
    let mut c = cpu(RN);
    assert_eq!(
        fp::to_int(&mut c, fp::D, d(-2.5), 64, true, fp::ROUND_NEAREST_EVEN),
        (-2i64) as u64
    );
    assert_eq!(
        fp::to_int(&mut c, fp::D, d(-2.5), 64, true, fp::ROUND_NEAREST_AWAY),
        (-3i64) as u64
    );
    assert_eq!(
        fp::to_int(&mut c, fp::D, d(-0.5), 32, false, fp::ROUND_ZERO),
        0
    );
    assert_eq!(c.fpsr, fp::IXC);
    let mut c = cpu(RN);
    assert_eq!(
        fp::to_int(&mut c, fp::D, d(-1.0), 32, false, fp::ROUND_ZERO),
        0
    );
    assert_eq!(c.fpsr, fp::IOC, "negative to unsigned saturates at 0");
}
#[test]
fn min_max_numeric_forms_ignore_one_quiet_nan() {
    let mut c = cpu(RN);
    assert_eq!(
        fp::min_max(&mut c, fp::D, QNAN_D, d(2.0), true, true),
        d(2.0)
    );
    assert_eq!(
        fp::min_max(&mut c, fp::D, QNAN_D, d(2.0), true, false),
        QNAN_D
    );
    assert_eq!(fp::min_max(&mut c, fp::D, d(0.0), d(-0.0), true, false), 0);
    assert_eq!(
        fp::min_max(&mut c, fp::D, d(0.0), d(-0.0), false, false),
        1 << 63
    );
    assert_eq!(c.fpsr, 0);
}
#[test]
fn compare_flags() {
    let mut c = cpu(RN);
    assert_eq!(
        fp::compare(&mut c, fp::S, 0x3f80_0000, 0x4000_0000, false),
        0b1000
    );
    assert_eq!(fp::compare(&mut c, fp::S, 0x8000_0000, 0, false), 0b0110);
    assert_eq!(fp::compare(&mut c, fp::S, 0x7fc0_0000, 0, false), 0b0011);
    assert_eq!(c.fpsr, 0, "FCMP is quiet for quiet NaNs");
    assert_eq!(fp::compare(&mut c, fp::S, 0x7fc0_0000, 0, true), 0b0011);
    assert_eq!(c.fpsr, fp::IOC, "FCMPE signals");
}
#[test]
fn precision_conversion_and_immediates() {
    let mut c = cpu(RN);
    // 65520 is the half-way point between 65504 (max half) and 65536: rounds to infinity.
    assert_eq!(
        fp::convert(&mut c, fp::S, fp::H, 65520f32.to_bits().into()).unwrap(),
        0x7c00
    );
    assert_eq!(c.fpsr, fp::OFC | fp::IXC);
    let mut c = cpu(RN);
    assert_eq!(
        fp::convert(&mut c, fp::H, fp::D, 0x3555).unwrap(),
        d(0.333251953125)
    );
    assert_eq!(fp::expand_imm(fp::S, 0x70), 0x3f80_0000);
    assert_eq!(fp::expand_imm(fp::D, 0x78), d(1.5));
    // imm8 0xC4: sign 1, exponent 0b01111100 (2^-3), fraction 0b0100: -1.25 * 2^-3.
    assert_eq!(fp::expand_imm(fp::S, 0xc4), (-0.15625f32).to_bits().into());
}
#[test]
fn fast_path_matches_exact_path() {
    // With IXC already set and default FPCR, host arithmetic is used; results must agree.
    for (a, b) in [(0.1, 0.2), (1.0e300, 3.7e-12), (-7.25, 1.0 / 3.0)] {
        let mut slow = cpu(RN);
        let mut fast = cpu(RN);
        fast.fpsr = fp::IXC;
        for op in 0..3 {
            let (x, y) = match op {
                0 => (
                    fp::add(&mut slow, fp::D, d(a), d(b), false),
                    fp::add(&mut fast, fp::D, d(a), d(b), false),
                ),
                1 => (
                    fp::mul(&mut slow, fp::D, d(a), d(b), false),
                    fp::mul(&mut fast, fp::D, d(a), d(b), false),
                ),
                _ => (
                    fp::div(&mut slow, fp::D, d(a), d(b)),
                    fp::div(&mut fast, fp::D, d(a), d(b)),
                ),
            };
            assert_eq!(x, y);
        }
    }
}
#[test]
fn lanes_are_little_endian_and_asymmetric() {
    let v: u128 = 0x0f0e_0d0c_0b0a_0908_0706_0504_0302_0100;
    assert_eq!(simd::lane(v, 8, 0), 0x00);
    assert_eq!(simd::lane(v, 16, 7), 0x0f0e);
    assert_eq!(simd::lane(v, 32, 1), 0x0706_0504);
    let a: u128 = 0x0000_0003_0000_0002_0000_0001_0000_0000;
    let b: u128 = 0x0000_0007_0000_0006_0000_0005_0000_0004;
    assert_eq!(
        simd::permute(simd::ZIP1, 32, 4, a, b),
        0x0000_0005_0000_0001_0000_0004_0000_0000
    );
    assert_eq!(
        simd::permute(simd::UZP2, 32, 4, a, b),
        0x0000_0007_0000_0005_0000_0003_0000_0001
    );
    assert_eq!(simd::ext(v, 0, 3, 16), v >> 24);
    // Narrowing into the upper half keeps the lower half of the destination.
    let mut c = cpu(RN);
    let narrowed = simd::narrow(&mut c, simd::XTN, 8, 8, v, u128::from(u64::MAX), true, 0);
    assert_eq!(narrowed as u64, u64::MAX);
    assert_eq!((narrowed >> 64) as u64, 0x0e0c_0a08_0604_0200);
}
#[test]
fn alternative_half_precision_has_no_infinity_or_nan() {
    const AHP: u64 = 1 << 26;
    // 65520 rounds to 2^16, an ordinary number in the alternative format (exponent 31).
    let mut c = cpu(AHP);
    assert_eq!(
        fp::convert(&mut c, fp::S, fp::H, 65520f32.to_bits().into()).unwrap(),
        0x7c00
    );
    assert_eq!(c.fpsr, fp::IXC);
    // Beyond the largest value (0x7FFF = 131008): saturation with Invalid Operation only.
    let mut c = cpu(AHP);
    assert_eq!(
        fp::convert(&mut c, fp::S, fp::H, 1.0e6f32.to_bits().into()).unwrap(),
        0x7fff
    );
    assert_eq!(c.fpsr, fp::IOC);
    let mut c = cpu(AHP);
    assert_eq!(
        fp::convert(&mut c, fp::D, fp::H, d(f64::NEG_INFINITY)).unwrap(),
        0xffff
    );
    assert_eq!(fp::convert(&mut c, fp::D, fp::H, QNAN_D).unwrap(), 0);
    assert_eq!(c.fpsr, fp::IOC);
    // Reading the alternative format: 0x7C00 is 65536, not infinity.
    let mut c = cpu(AHP);
    assert_eq!(
        fp::convert(&mut c, fp::H, fp::S, 0x7c00).unwrap(),
        65536f32.to_bits().into()
    );
}
