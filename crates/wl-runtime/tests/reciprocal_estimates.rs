use wl_runtime::{Cpu, fp, simd};
#[test]
fn every_normal_quantization_bin_matches_independent_real_arithmetic() {
    for f in [fp::S, fp::D] {
        for exponent in [-10, -1, 0, 1, 10] {
            for bin in 0..256u64 {
                let mut c = Cpu::new(0);
                let a = (((1 << (f.ebits - 1)) - 1 + exponent) as u64) << f.mbits
                    | (bin << (f.mbits - 8));
                let scaled = (256.0 + bin as f64 + 0.5) / 512.0;
                let expected = ((256.0 / scaled).round() / 512.0) * 2.0f64.powi(-exponent);
                let bits = if f == fp::S {
                    u64::from((expected as f32).to_bits())
                } else {
                    expected.to_bits()
                };
                assert_eq!(fp::reciprocal_estimate(&mut c, f, a, false), bits);
                assert_eq!(c.fpsr, 0);
                // Sqrt bins have seven significand bits; test both halves of
                // each bin and both unbiased exponent parities.
                let scaled = (128.0 + (bin / 2) as f64 + 0.5)
                    / if exponent & 1 == 0 { 512.0 } else { 256.0 };
                let scale_exp = if exponent & 1 == 0 {
                    -exponent / 2 - 1
                } else {
                    -(exponent + 1) / 2
                };
                let expected = ((256.0 / scaled.sqrt()).round() / 256.0) * 2.0f64.powi(scale_exp);
                let bits = if f == fp::S {
                    u64::from((expected as f32).to_bits())
                } else {
                    expected.to_bits()
                };
                assert_eq!(fp::reciprocal_estimate(&mut c, f, a, true), bits);
                assert_eq!(c.fpsr, 0);
            }
        }
    }
}
#[test]
fn special_values_flags_and_default_nan() {
    for f in [fp::S, fp::D] {
        let sign = 1u64 << (f.ebits + f.mbits);
        let infinity = ((1u64 << f.ebits) - 1) << f.mbits;
        let quiet = 1 << (f.mbits - 1);
        for sqrt in [false, true] {
            for negative in [0, sign] {
                let mut c = Cpu::new(0);
                assert_eq!(
                    fp::reciprocal_estimate(&mut c, f, negative, sqrt),
                    infinity | negative
                );
                assert_eq!(c.fpsr, fp::DZC);
            }
            let mut c = Cpu::new(0);
            assert_eq!(
                fp::reciprocal_estimate(&mut c, f, infinity | 7, sqrt),
                infinity | quiet | 7
            );
            assert_eq!(c.fpsr, fp::IOC);
            c.fpcr = 1 << 25;
            assert_eq!(
                fp::reciprocal_estimate(&mut c, f, infinity | quiet | 7, sqrt),
                infinity | quiet
            );
            assert_eq!(c.fpsr, fp::IOC, "sticky invalid flag retained");
        }
        let mut c = Cpu::new(0);
        assert_eq!(
            fp::reciprocal_estimate(&mut c, f, infinity | sign, false),
            sign
        );
        assert_eq!(
            fp::reciprocal_estimate(&mut c, f, infinity | sign, true),
            infinity | quiet
        );
        assert_eq!(c.fpsr, fp::IOC);
    }
}
#[test]
fn flush_input_underflow_overflow_and_rounding() {
    let mut c = Cpu::new(0);
    c.fpcr = 1 << 24;
    assert_eq!(fp::reciprocal_estimate(&mut c, fp::S, 1, false), 0x7f800000);
    assert_eq!(c.fpsr, fp::IDC | fp::DZC);
    c.fpsr = 0;
    assert_eq!(fp::reciprocal_estimate(&mut c, fp::S, 0x7f7fffff, false), 0);
    assert_eq!(c.fpsr, fp::UFC);
    for (mode, expected) in [
        (0, 0x7f800000),
        (1, 0x7f800000),
        (2, 0x7f7fffff),
        (3, 0x7f7fffff),
    ] {
        c.fpcr = mode << 22;
        c.fpsr = 0;
        assert_eq!(fp::reciprocal_estimate(&mut c, fp::S, 1, false), expected);
        assert_eq!(c.fpsr, fp::OFC | fp::IXC);
    }
}
#[test]
fn lane_evaluation_and_64_bit_vector_upper_half() {
    let mut c = Cpu::new(0);
    let a = 0x40000000_3f800000u128;
    assert_eq!(
        simd::fp1(&mut c, simd::FRECPE, 32, 2, a, 0, 0),
        0x3eff8000_3f7f8000
    );
    assert_eq!(c.fpsr, 0);
    assert_eq!(simd::fp1(&mut c, simd::FRSQRTE, 32, 1, 0, 0, 0), 0x7f800000);
    assert_eq!(c.fpsr, fp::DZC);
}
