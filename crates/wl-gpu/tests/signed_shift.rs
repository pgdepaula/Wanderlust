use wl_gpu::shader_program::{FragmentProgram, IntegerOperator, Operation, VertexProgram};
const BIN: &[u8] = include_bytes!("fixtures/maxwell/signed-shr-bytes.bin");
const BASES: [u64; 2] = [0x5c29000000070000, 0x5c29008000070000];
fn change(b: &mut [u8], off: usize, f: impl FnOnce(u64) -> u64) {
    let word = u64::from_le_bytes(b[off..off + 8].try_into().unwrap());
    b[off..off + 8].copy_from_slice(&f(word).to_le_bytes());
}
fn validate(wgsl: &str) {
    let m = naga::front::wgsl::parse_str(wgsl).unwrap();
    naga::valid::Validator::new(
        naga::valid::ValidationFlags::all(),
        naga::valid::Capabilities::all(),
    )
    .validate(&m)
    .unwrap();
}
fn error(b: &[u8], text: &str) {
    let e = FragmentProgram::decode(b).unwrap_err().to_string();
    assert!(e.contains(text), "{e}");
}
#[test]
fn independent_signed_shr_forms_preserve_unsigned_counts_and_signed_values() {
    for (bin, wrap, word, count) in [
        (BIN, false, 0x5c29000000570409, "min(r[5],31u)"),
        (
            include_bytes!("fixtures/maxwell/signed-shr-wrap-bytes.bin").as_slice(),
            true,
            0x5c29008000570409,
            "(r[5] & 31u)",
        ),
    ] {
        assert_eq!(
            u64::from_le_bytes(bin[0x98..0xa0].try_into().unwrap()),
            word
        );
        let p = FragmentProgram::decode(bin).unwrap();
        assert_eq!(p.required_registers(), 13);
        assert_eq!(
            p.operations()[7],
            Operation::Integer {
                dst: 9,
                left: 4,
                right: 5,
                operator: IntegerOperator::ShiftRightSigned { wrap }
            }
        );
        assert!(p.wgsl().contains(&format!(
            "r[9]=bitcast<u32>(bitcast<i32>(r[4]) >> {count});"
        )));
        validate(&p.wgsl());
        assert!(VertexProgram::decode(bin).is_err());
    }
    let v =
        VertexProgram::decode(include_bytes!("fixtures/maxwell/vertex-signed-shr.bin")).unwrap();
    assert_eq!(v.required_registers(), 7);
    assert!(v.operations().contains(&Operation::Integer {
        dst: 6,
        left: 0,
        right: 6,
        operator: IntegerOperator::ShiftRightSigned { wrap: false }
    }));
    validate(&v.wgsl());
}
#[test]
fn signed_shift_unverified_modifiers_predicates_and_operands_fail() {
    for base in BASES {
        for bit in [44, 47, 40, 16, 19, 28] {
            let mut b = BIN.to_vec();
            change(&mut b, 0x98, |_| {
                (base | (4 << 8) | (5 << 20) | 9) ^ (1 << bit)
            });
            error(&b, "unsupported instruction");
        }
        let mut b = BIN.to_vec();
        change(&mut b, 0x98, |_| base | (4 << 8) | (5 << 20) | 255);
        error(&b, "RZ destination");
    }
}
#[test]
fn signed_shift_readiness_rz_and_in_place_operands() {
    for base in BASES {
        for field in [8, 20] {
            let mut b = BIN.to_vec();
            change(&mut b, 0x98, |_| base | (4 << 8) | (5 << 20) | 9);
            change(&mut b, 0x98, |w| (w & !(255 << field)) | (13 << field));
            error(&b, "unready integer");
            change(&mut b, 0x98, |w| w | (255 << field));
            validate(&FragmentProgram::decode(&b).unwrap().wgsl());
        }
        let mut b = BIN.to_vec();
        change(&mut b, 0x98, |_| base | (4 << 8) | (5 << 20) | 9);
        change(&mut b, 0x50, |w| w & !(1 << (21 + 11)));
        error(&b, "unready integer");
        let mut b = BIN.to_vec();
        change(&mut b, 0x98, |_| base | (4 << 8) | (5 << 20) | 9);
        change(&mut b, 0x90, |w| w & !(7 << 5));
        error(&b, "unready integer");
        for dst in [4, 5] {
            let mut b = BIN.to_vec();
            change(&mut b, 0x98, |_| base | (4 << 8) | (5 << 20) | dst);
            for bundle in b[80..].chunks_exact_mut(32) {
                for bytes in bundle[8..].chunks_exact_mut(8) {
                    let w = u64::from_le_bytes(bytes.try_into().unwrap());
                    if w >> 48 == 0x5c28 && (w >> 8) & 255 == 9 {
                        bytes.copy_from_slice(&((w & !(255 << 8)) | (dst << 8)).to_le_bytes());
                    }
                }
            }
            validate(&FragmentProgram::decode(&b).unwrap().wgsl());
        }
    }
}
#[test]
fn signed_shift_preserves_multiply_hazard_from_value_and_count() {
    for base in BASES {
        for (left, right) in [(1, 255), (255, 1)] {
            let mut b = include_bytes!("fixtures/maxwell/alu.bin").to_vec();
            change(&mut b, 0x88, |_| base | (right << 20) | (left << 8) | 2);
            change(&mut b, 0x98, |_| 0x5c5800000ff70203);
            error(&b, "unfused rounding");
        }
    }
}
fn insert_identity(bin: &[u8], at: usize, register: u8, base: u64) -> Vec<u8> {
    let mut items: Vec<(u64, u64)> = bin[80..]
        .chunks_exact(32)
        .flat_map(|bundle| {
            let ctrl = u64::from_le_bytes(bundle[..8].try_into().unwrap());
            (0..3).map(move |n| {
                (
                    u64::from_le_bytes(bundle[8 + n * 8..16 + n * 8].try_into().unwrap()),
                    (ctrl >> (n * 21)) & 0x1fffff,
                )
            })
        })
        .collect();
    items.insert(
        at,
        (
            base | (255 << 20) | (u64::from(register) << 8) | u64::from(register),
            0xfe6,
        ),
    );
    let mut out = bin[..80].to_vec();
    for group in items.chunks(3) {
        let control = (0..3).fold(0u64, |a, n| {
            a | (group.get(n).map_or(0x7e6, |v| v.1) << (n * 21))
        });
        out.extend(control.to_le_bytes());
        for n in 0..3 {
            out.extend(
                group
                    .get(n)
                    .map_or(0x50b0000000070f00, |v| v.0)
                    .to_le_bytes(),
            );
        }
    }
    out
}
#[test]
fn signed_shift_identities_clear_resource_and_interpolation_provenance() {
    for base in BASES {
        let b = insert_identity(include_bytes!("fixtures/maxwell/tex.bin"), 3, 6, base);
        assert!(
            FragmentProgram::decode(&b)
                .unwrap_err()
                .to_string()
                .contains("TEX handle")
        );
        let bin = include_bytes!("fixtures/maxwell/perspective.bin");
        for (at, message) in [(2, "PositionW"), (3, "reciprocal")] {
            let b = insert_identity(bin, at, 7, base);
            assert!(
                FragmentProgram::decode(&b)
                    .unwrap_err()
                    .to_string()
                    .contains(message)
            );
        }
    }
}
