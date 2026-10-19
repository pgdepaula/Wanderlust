//! Literal encodings assembled with devkitA64 GNU as for Armv8-A.
use wl_aarch64::{Op, SimdOp, decode};
use wl_common::GuestAddr;
#[test]
fn scalar_and_vector_estimate_forms() {
    for (word, esize, lanes, op) in [
        (0x0ea1d820, 32, 2, 14),
        (0x4ea1d820, 32, 4, 14),
        (0x4ee1d820, 64, 2, 14),
        (0x2ea1d820, 32, 2, 15),
        (0x6ea1d820, 32, 4, 15),
        (0x6ee1d820, 64, 2, 15),
        (0x5ea1d820, 32, 1, 14),
        (0x5ee1d820, 64, 1, 14),
        (0x7ea1d820, 32, 1, 15),
        (0x7ee1d820, 64, 1, 15),
    ] {
        let decoded = decode(word, GuestAddr(0x10000)).unwrap();
        let Op::Simd(SimdOp::FpUnary {
            shape,
            op: actual,
            d,
            n,
            mode,
            fbits,
        }) = decoded.op
        else {
            panic!("wrong estimate decode {word:08x}")
        };
        assert_eq!(
            (shape.esize, shape.lanes, actual, d, n, mode, fbits),
            (esize, lanes, op, 0, 1, 0, 0)
        );
    }
}
#[test]
fn double_requires_a_128_bit_vector_and_unsigned_estimates_remain_unsupported() {
    for word in [0x0ee1d820, 0x2ee1d820, 0x4ea1c820, 0x6ea1c820] {
        assert!(decode(word, GuestAddr(0x10000)).is_err(), "{word:08x}");
    }
}
