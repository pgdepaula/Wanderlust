use wl_gpu::shader_program::{FragmentProgram, IntegerOperator, Operation, VertexProgram};
const BIN: &[u8] = include_bytes!("fixtures/maxwell/shift.bin");
const BASES: [u64; 4] = [
    0x5c48000000070000,
    0x5c48008000070000,
    0x5c28000000070000,
    0x5c28008000070000,
];
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
fn independent_shift_words_operands_and_unsigned_lowering() {
    let p = FragmentProgram::decode(BIN).unwrap();
    assert_eq!(p.required_registers(), 6);
    for (n, operator) in [
        IntegerOperator::ShiftLeft { wrap: false },
        IntegerOperator::ShiftLeft { wrap: true },
        IntegerOperator::ShiftRight { wrap: false },
        IntegerOperator::ShiftRight { wrap: true },
    ]
    .into_iter()
    .enumerate()
    {
        assert_eq!(
            p.operations()[n + 2],
            Operation::Integer {
                dst: n as u8,
                left: 4,
                right: 5,
                operator,
            }
        );
    }
    for line in [
        "r[0]=select(0u,r[4] << (r[5] & 31u),r[5] < 32u);",
        "r[1]=r[4] << (r[5] & 31u);",
        "r[2]=select(0u,r[4] >> (r[5] & 31u),r[5] < 32u);",
        "r[3]=r[4] >> (r[5] & 31u);",
    ] {
        assert!(p.wgsl().contains(line));
    }
    validate(&p.wgsl());
    assert!(VertexProgram::decode(BIN).is_err());
    let v = VertexProgram::decode(include_bytes!("fixtures/maxwell/vertex-shift.bin")).unwrap();
    validate(&v.wgsl());
    assert!(v.operations().contains(&Operation::Integer {
        dst: 0,
        left: 0,
        right: 6,
        operator: IntegerOperator::ShiftLeft { wrap: false },
    }));
}
#[test]
fn shift_modifier_predicate_and_operand_forms_reject() {
    for (base, bits) in [(BASES[0], vec![43, 47, 48]), (BASES[2], vec![44, 47, 40])] {
        for bit in bits {
            let mut b = BIN.to_vec();
            change(&mut b, 0x68, |_| base | (5 << 20) | (4 << 8) | (1 << bit));
            assert!(
                FragmentProgram::decode(&b)
                    .unwrap_err()
                    .to_string()
                    .contains("unsupported instruction")
            );
        }
    }
    for base in BASES {
        for pred in [0u64, 8] {
            let mut b = BIN.to_vec();
            change(&mut b, 0x68, |_| {
                (base & !(15 << 16)) | (pred << 16) | (5 << 20) | (4 << 8)
            });
            assert!(FragmentProgram::decode(&b).is_err());
        }
    }
    // Independent signed SHR must retain its interpretation, not unsigned fallback.
    let mut b = BIN.to_vec();
    change(&mut b, 0x68, |_| 0x5c29000000570400);
    assert_eq!(
        FragmentProgram::decode(&b).unwrap().operations()[2],
        Operation::Integer {
            dst: 0,
            left: 4,
            right: 5,
            operator: IntegerOperator::ShiftRightSigned { wrap: false }
        }
    );
}
#[test]
fn shift_dependency_and_zero_register_rules() {
    for base in BASES {
        for field in [8, 20] {
            let mut b = BIN.to_vec();
            change(&mut b, 0x68, |_| base | (5 << 20) | (4 << 8));
            change(&mut b, 0x68, |w| (w & !(255 << field)) | (6 << field));
            assert!(
                FragmentProgram::decode(&b)
                    .unwrap_err()
                    .to_string()
                    .contains("unready integer")
            );
            change(&mut b, 0x68, |w| w | (255 << field));
            assert!(FragmentProgram::decode(&b).is_ok());
        }
        let mut b = BIN.to_vec();
        change(&mut b, 0x68, |_| base | (5 << 20) | (4 << 8));
        change(&mut b, 0x50, |w| w & !(1 << (42 + 11)));
        assert!(
            FragmentProgram::decode(&b)
                .unwrap_err()
                .to_string()
                .contains("unready integer")
        );
        let mut b = BIN.to_vec();
        change(&mut b, 0x68, |_| base | (5 << 20) | (4 << 8) | 255);
        assert!(FragmentProgram::decode(&b).is_err());
        let mut b = BIN.to_vec();
        change(&mut b, 0x50, |w| w & !(7 << (42 + 5)));
        assert!(
            FragmentProgram::decode(&b)
                .unwrap_err()
                .to_string()
                .contains("unready integer")
        );
    }
}
#[test]
fn shift_identity_cannot_hide_unfused_rounding_hazards() {
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
fn shift_identities_clear_resource_and_interpolation_provenance() {
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
