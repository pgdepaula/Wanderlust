use wl_gpu::shader_program::{FragmentProgram, IntegerOperator, Operation, VertexProgram};
const BIN: &[u8] = include_bytes!("fixtures/maxwell/iadd-bytes.bin");
const BASES: [u64; 2] = [0x5c11000000070000, 0x5c12000000070000];
fn change(bytes: &mut [u8], offset: usize, f: impl FnOnce(u64) -> u64) {
    let w = u64::from_le_bytes(bytes[offset..offset + 8].try_into().unwrap());
    bytes[offset..offset + 8].copy_from_slice(&f(w).to_le_bytes());
}
fn validate(wgsl: &str) {
    let module = naga::front::wgsl::parse_str(wgsl).unwrap();
    naga::valid::Validator::new(
        naga::valid::ValidationFlags::all(),
        naga::valid::Capabilities::all(),
    )
    .validate(&module)
    .unwrap();
}
#[test]
fn independent_subtraction_forms_normalize_operands_and_preserve_raw_bits() {
    for (bin, left, right) in [
        (BIN, 4, 5),
        (
            include_bytes!("fixtures/maxwell/iadd-bytes-left.bin").as_slice(),
            5,
            4,
        ),
    ] {
        let p = FragmentProgram::decode(bin).unwrap();
        assert_eq!(p.required_registers(), 13);
        assert_eq!(
            p.operations()[7],
            Operation::Integer {
                dst: 9,
                left,
                right,
                operator: IntegerOperator::Subtract
            }
        );
        assert!(p.wgsl().contains(&format!("r[9]=r[{left}] - r[{right}];")));
        validate(&p.wgsl());
        assert!(VertexProgram::decode(bin).is_err());
    }
    let bin = include_bytes!("fixtures/maxwell/vertex-iadd.bin");
    let p = VertexProgram::decode(bin).unwrap();
    assert_eq!(p.required_registers(), 7);
    for (dst, left, right) in [(0, 0, 6), (1, 1, 6)] {
        assert!(p.operations().contains(&Operation::Integer {
            dst,
            left,
            right,
            operator: IntegerOperator::Subtract
        }));
    }
    validate(&p.wgsl());
}
#[test]
fn subtract_rejects_po_and_other_unverified_modifiers() {
    let po = include_bytes!("fixtures/maxwell/iadd-bytes-po-negative.bin");
    assert_eq!(
        u64::from_le_bytes(po[0x98..0xa0].try_into().unwrap()),
        0x5c13000000570409
    );
    assert!(
        FragmentProgram::decode(po)
            .unwrap_err()
            .to_string()
            .contains("unsupported instruction")
    );
    for base in BASES {
        for bit in [43, 47, 50, 16, 19] {
            let mut b = BIN.to_vec();
            change(&mut b, 0x98, |_| {
                (base | (4 << 8) | (5 << 20) | 9) ^ (1 << bit)
            });
            assert!(
                FragmentProgram::decode(&b)
                    .unwrap_err()
                    .to_string()
                    .contains("unsupported instruction")
            );
        }
    }
}
#[test]
fn subtract_readiness_rz_and_source_destination_aliasing() {
    for base in BASES {
        for field in [8, 20] {
            let mut b = BIN.to_vec();
            change(&mut b, 0x98, |_| base | (4 << 8) | (5 << 20) | 9);
            change(&mut b, 0x98, |w| (w & !(255 << field)) | (13 << field));
            assert!(
                FragmentProgram::decode(&b)
                    .unwrap_err()
                    .to_string()
                    .contains("unready integer")
            );
            change(&mut b, 0x98, |w| w | (255 << field));
            validate(&FragmentProgram::decode(&b).unwrap().wgsl());
        }
        let mut b = BIN.to_vec();
        change(&mut b, 0x98, |_| base | (4 << 8) | (5 << 20) | 9);
        change(&mut b, 0x50, |w| w & !(1 << (21 + 11)));
        assert!(
            FragmentProgram::decode(&b)
                .unwrap_err()
                .to_string()
                .contains("unready integer")
        );
        let mut b = BIN.to_vec();
        change(&mut b, 0x98, |_| base | (4 << 8) | (5 << 20) | 9);
        change(&mut b, 0x90, |w| w & !(7 << 5));
        assert!(
            FragmentProgram::decode(&b)
                .unwrap_err()
                .to_string()
                .contains("unready integer")
        );
        // A normalized in-place destination must still read its old value.
        for dst in [4, 5] {
            let mut b = BIN.to_vec();
            change(&mut b, 0x98, |_| base | (4 << 8) | (5 << 20) | dst);
            for bundle in b[80..].chunks_exact_mut(32) {
                for bytes in bundle[8..].chunks_exact_mut(8) {
                    let word = u64::from_le_bytes(bytes.try_into().unwrap());
                    if word >> 48 == 0x5c28 && (word >> 8) & 255 == 9 {
                        bytes.copy_from_slice(&((word & !(255 << 8)) | (dst << 8)).to_le_bytes());
                    }
                }
            }
            validate(&FragmentProgram::decode(&b).unwrap().wgsl());
        }
    }
}
#[test]
fn subtract_preserves_unfused_rounding_hazard_taint() {
    for base in BASES {
        let mut b = include_bytes!("fixtures/maxwell/alu.bin").to_vec();
        change(&mut b, 0x88, |_| base | (255 << 20) | (1 << 8) | 2);
        change(&mut b, 0x98, |_| 0x5c5800000ff70203);
        assert!(
            FragmentProgram::decode(&b)
                .unwrap_err()
                .to_string()
                .contains("unfused rounding")
        );
    }
}
fn insert_integer(bin: &[u8], at: usize, register: u8, base: u64) -> Vec<u8> {
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
fn subtract_cannot_manufacture_resource_or_interpolation_provenance() {
    for base in BASES {
        let b = insert_integer(include_bytes!("fixtures/maxwell/tex.bin"), 3, 6, base);
        assert!(
            FragmentProgram::decode(&b)
                .unwrap_err()
                .to_string()
                .contains("TEX handle")
        );
        let bin = include_bytes!("fixtures/maxwell/perspective.bin");
        for (at, message) in [(2, "PositionW"), (3, "reciprocal")] {
            let b = insert_integer(bin, at, 7, base);
            assert!(
                FragmentProgram::decode(&b)
                    .unwrap_err()
                    .to_string()
                    .contains(message)
            );
        }
    }
}
