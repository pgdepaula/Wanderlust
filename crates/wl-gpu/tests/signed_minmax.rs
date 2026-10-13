use wl_gpu::shader_program::{FragmentProgram, IntegerOperator, Operation, VertexProgram};
const BIN: &[u8] = include_bytes!("fixtures/maxwell/signed-minmax.bin");
const BASES: [u64; 2] = [0x5c21038000070000, 0x5c21078000070000];
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
fn independent_signed_imnmx_fields_and_stage_lowering() {
    let p = FragmentProgram::decode(BIN).unwrap();
    assert_eq!(p.required_registers(), 9);
    for (n, (left, right, operator)) in [
        (4, 5, IntegerOperator::MinSigned),
        (4, 5, IntegerOperator::MaxSigned),
        (255, 4, IntegerOperator::MinSigned),
        (5, 4, IntegerOperator::MaxSigned),
    ]
    .into_iter()
    .enumerate()
    {
        assert_eq!(
            p.operations()[n + 5],
            Operation::Integer {
                dst: n as u8,
                left,
                right,
                operator
            }
        );
    }
    for line in [
        "r[0]=bitcast<u32>(min(bitcast<i32>(r[4]),bitcast<i32>(r[5])));",
        "r[1]=bitcast<u32>(max(bitcast<i32>(r[4]),bitcast<i32>(r[5])));",
        "r[2]=bitcast<u32>(min(bitcast<i32>(r[255]),bitcast<i32>(r[4])));",
        "r[3]=bitcast<u32>(max(bitcast<i32>(r[5]),bitcast<i32>(r[4])));",
    ] {
        assert!(p.wgsl().contains(line));
    }
    validate(&p.wgsl());
    assert!(VertexProgram::decode(BIN).is_err());
    let bin = include_bytes!("fixtures/maxwell/vertex-signed-minmax.bin");
    let v = VertexProgram::decode(bin).unwrap();
    validate(&v.wgsl());
    assert!(v.operations().contains(&Operation::Integer {
        dst: 0,
        left: 0,
        right: 255,
        operator: IntegerOperator::MinSigned,
    }));
    assert!(v.operations().contains(&Operation::Integer {
        dst: 1,
        left: 1,
        right: 255,
        operator: IntegerOperator::MaxSigned,
    }));
    assert!(FragmentProgram::decode(bin).is_err());
}
#[test]
fn signed_imnmx_rejects_dynamic_predicates_extended_and_cc() {
    // Independently assembled XLO and CC forms. Signed forms are tested separately.
    for word in [0x5c210b8000570403, 0x5c21838000570406] {
        let mut b = BIN.to_vec();
        change(&mut b, 0x80, |_| word);
        assert!(
            FragmentProgram::decode(&b)
                .unwrap_err()
                .to_string()
                .contains("unsupported instruction")
        );
    }
    for base in BASES {
        for pred in 0u64..7 {
            // Selection predicate (bits41:39) differs from outer execution P7.
            let mut b = BIN.to_vec();
            change(&mut b, 0x80, |_| {
                (base & !(7 << 39)) | (pred << 39) | (5 << 20) | (4 << 8)
            });
            assert!(
                FragmentProgram::decode(&b)
                    .unwrap_err()
                    .to_string()
                    .contains("unsupported instruction")
            );
        }
        let mut b = BIN.to_vec();
        change(&mut b, 0x80, |_| {
            (base & !(15 << 16)) | (5 << 20) | (4 << 8)
        });
        assert!(FragmentProgram::decode(&b).is_err());
    }
}
#[test]
fn signed_imnmx_initialization_scoreboard_and_rz_rules() {
    for base in BASES {
        for field in [8, 20] {
            let mut b = BIN.to_vec();
            change(&mut b, 0x80, |_| base | (5 << 20) | (4 << 8));
            change(&mut b, 0x80, |w| (w & !(255 << field)) | (9 << field));
            assert!(
                FragmentProgram::decode(&b)
                    .unwrap_err()
                    .to_string()
                    .contains("unready integer")
            );
            change(&mut b, 0x80, |w| w | (255 << field));
            assert!(FragmentProgram::decode(&b).is_ok());
        }
        let mut b = BIN.to_vec();
        change(&mut b, 0x80, |_| base | (5 << 20) | (4 << 8));
        change(&mut b, 0x50, |w| w & !(1 << (21 + 11)));
        assert!(
            FragmentProgram::decode(&b)
                .unwrap_err()
                .to_string()
                .contains("unready integer")
        );
        let mut b = BIN.to_vec();
        change(&mut b, 0x80, |w| w | 255);
        assert!(FragmentProgram::decode(&b).is_err());
        let mut b = BIN.to_vec();
        change(&mut b, 0x70, |w| w & !(7 << (21 + 5)));
        assert!(
            FragmentProgram::decode(&b)
                .unwrap_err()
                .to_string()
                .contains("unready integer")
        );
    }
}
#[test]
fn signed_imnmx_preserves_unfused_rounding_hazard_taint() {
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
fn signed_imnmx_cannot_manufacture_resource_or_interpolation_provenance() {
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
