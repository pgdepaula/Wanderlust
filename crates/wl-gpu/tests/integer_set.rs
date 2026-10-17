use wl_gpu::shader_program::{FragmentProgram, IntegerComparison as C, Operation, VertexProgram};
const BIN: &[u8] = include_bytes!("fixtures/maxwell/integer-set-u-mask.bin");
const BF: &[u8] = include_bytes!("fixtures/maxwell/integer-set-u-float.bin");
const SIGNED: &[u8] = include_bytes!("fixtures/maxwell/integer-set-s-mask.bin");
const SIGNED_BF: &[u8] = include_bytes!("fixtures/maxwell/integer-set-s-float.bin");
const VERTEX: &[u8] = include_bytes!("fixtures/maxwell/vertex-integer-set.bin");
const ALU: u64 = 0x7e6;
const EXIT: u64 = 0xe30000000007000f;
const NOP: u64 = 0x50b0000000070f00;
fn iset(dst: u8, left: u8, right: u8, condition: u8, signed: bool, bf: bool) -> (u64, u64) {
    (
        0x5b50038000070000
            | u64::from(dst)
            | (u64::from(left) << 8)
            | (u64::from(right) << 20)
            | (u64::from(condition) << 49)
            | (u64::from(signed) << 48)
            | (u64::from(bf) << 44),
        ALU,
    )
}
fn comparison(condition: u8, signed: bool) -> C {
    match (condition, signed) {
        (1, false) => C::Less,
        (2, false) => C::Equal,
        (3, false) => C::LessEqual,
        (4, false) => C::Greater,
        (5, false) => C::NotEqual,
        (6, false) => C::GreaterEqual,
        (1, true) => C::LessSigned,
        (2, true) => C::EqualSigned,
        (3, true) => C::LessEqualSigned,
        (4, true) => C::GreaterSigned,
        (5, true) => C::NotEqualSigned,
        (6, true) => C::GreaterEqualSigned,
        _ => panic!("unsupported condition"),
    }
}
fn immediate(dst: u8) -> (u64, u64) {
    (0x0103f8000007f000 | u64::from(dst), ALU)
}
fn initialized() -> Vec<(u64, u64)> {
    (0..4).map(immediate).collect()
}
fn program(header: &[u8], code: &[(u64, u64)]) -> Vec<u8> {
    let mut out = header[..80].to_vec();
    for group in code.chunks(3) {
        let c = (0..3).fold(0u64, |v, n| {
            v | (group.get(n).map_or(ALU, |x| x.1) << (n * 21))
        });
        out.extend(c.to_le_bytes());
        for n in 0..3 {
            out.extend(group.get(n).map_or(NOP, |x| x.0).to_le_bytes());
        }
    }
    out
}
fn items(bin: &[u8]) -> Vec<(u64, u64)> {
    bin[80..]
        .chunks_exact(32)
        .flat_map(|b| {
            let c = u64::from_le_bytes(b[..8].try_into().unwrap());
            (0..3).map(move |n| {
                (
                    u64::from_le_bytes(b[8 + n * 8..16 + n * 8].try_into().unwrap()),
                    (c >> (n * 21)) & 0x1fffff,
                )
            })
        })
        .collect()
}
fn error(code: &[(u64, u64)], text: &str) {
    let e = FragmentProgram::decode(&program(BIN, code))
        .unwrap_err()
        .to_string();
    assert!(e.contains(text), "expected {text}: {e}");
}
fn validate(wgsl: &str) {
    let m = naga::front::wgsl::parse_str(wgsl).unwrap();
    naga::valid::Validator::new(
        naga::valid::ValidationFlags::all(),
        naga::valid::Capabilities::empty(),
    )
    .validate(&m)
    .unwrap();
}
#[test]
fn independent_integer_set_fixtures_distinguish_signed48_and_float44() {
    for (bin, signed, float_result, word) in [
        (BIN, false, false, 0x5b52038000570404),
        (BF, false, true, 0x5b52138000570404),
        (SIGNED, true, false, 0x5b53038000570404),
        (SIGNED_BF, true, true, 0x5b53138000570404),
    ] {
        assert_eq!(
            u64::from_le_bytes(bin[0x78..0x80].try_into().unwrap()),
            word
        );
        let p = FragmentProgram::decode(bin).unwrap();
        assert!(p.operations().contains(&Operation::IntegerSet {
            dst: 4,
            left: 4,
            right: 5,
            comparison: comparison(1, signed),
            float_result,
        }));
        assert_eq!(p.required_registers(), 8);
        // Auxiliary zero-producing ISET follows ISETP; it must preserve P0.
        assert_eq!(p.wgsl().matches("p0=").count(), 1);
        validate(&p.wgsl());
        assert!(VertexProgram::decode(bin).is_err());
    }
    assert_eq!(
        u64::from_le_bytes(VERTEX[0x78..0x80].try_into().unwrap()),
        0x5b5913800ff70006
    );
    let v = VertexProgram::decode(VERTEX).unwrap();
    assert!(v.operations().contains(&Operation::IntegerSet {
        dst: 6,
        left: 0,
        right: 255,
        comparison: C::GreaterSigned,
        float_result: true,
    }));
    assert_eq!(v.required_registers(), 9);
    validate(&v.wgsl());
    assert!(FragmentProgram::decode(VERTEX).is_err());
}
#[test]
fn all_24_modes_and_destination_aliases_validate_without_predicate_outputs() {
    for condition in 1..=6 {
        for signed in [false, true] {
            for float_result in [false, true] {
                for dst in [0, 1, 2] {
                    let mut code = initialized();
                    code.extend([
                        iset(dst, 1, 2, condition, signed, float_result),
                        (EXIT, ALU),
                    ]);
                    let p = FragmentProgram::decode(&program(BIN, &code)).unwrap();
                    assert_eq!(
                        p.operations()[4],
                        Operation::IntegerSet {
                            dst,
                            left: 1,
                            right: 2,
                            comparison: comparison(condition, signed),
                            float_result,
                        }
                    );
                    assert_eq!(p.required_registers(), 4);
                    validate(&p.wgsl());
                }
            }
        }
    }
}
#[test]
fn both_gpr_fields_and_rz_have_complete_register_metadata() {
    for high in [4, 16, 32, 64, 128, 254, 255] {
        for (left, right) in [(high, 1), (1, high), (high, high)] {
            let mut code = initialized();
            if high != 255 {
                code.push(immediate(high));
            }
            code.extend([iset(0, left, right, 2, true, true), (EXIT, ALU)]);
            let p = FragmentProgram::decode(&program(BIN, &code)).unwrap();
            assert_eq!(
                p.required_registers(),
                if high == 255 { 4 } else { u16::from(high) + 1 }
            );
            validate(&p.wgsl());
        }
    }
}
#[test]
fn unsupported_predicates_boolean_sources_cc_x_and_forms_stay_rejected() {
    for bit in [
        16, 17, 18, 19, 28, 32, 39, 40, 41, 42, 43, 45, 46, 47, 52, 56,
    ] {
        let mut code = initialized();
        let mut op = iset(0, 1, 2, 1, false, false);
        op.0 ^= 1 << bit;
        code.extend([op, (EXIT, ALU)]);
        error(&code, "unsupported instruction");
    }
    for condition in [0, 7] {
        for signed in [false, true] {
            for bf in [false, true] {
                let mut code = initialized();
                code.extend([iset(0, 1, 2, condition, signed, bf), (EXIT, ALU)]);
                error(&code, "unsupported");
            }
        }
    }
    for word in [0x3652038000170100, 0x4b52038000170100] {
        let mut code = initialized();
        code.extend([(word, ALU), (EXIT, ALU)]);
        error(&code, "unsupported instruction");
    }
    let mut code = initialized();
    code.extend([iset(255, 1, 2, 1, false, false), (EXIT, ALU)]);
    error(&code, "RZ destination unsupported");
}
#[test]
fn each_source_pending_destination_and_fixed_six_schedule_remain_checked() {
    for (left, right) in [(4, 2), (1, 4)] {
        let mut code = initialized();
        code.extend([iset(0, left, right, 1, false, false), (EXIT, ALU)]);
        error(&code, "unready ISET operand");
    }
    for pending in [1u8, 2] {
        let mut code = initialized();
        code.extend([
            (0xef9400000107ff00 | u64::from(pending), 0x706),
            iset(0, 1, 2, 1, false, true),
            (EXIT, ALU),
        ]);
        error(&code, "unready ISET operand");
        code[5].1 |= 1 << 11;
        validate(
            &FragmentProgram::decode(&program(BIN, &code))
                .unwrap()
                .wgsl(),
        );
    }
    let mut code = initialized();
    code.extend([
        (0xef9400000107ff00, 0x706),
        iset(0, 1, 2, 1, false, false),
        (EXIT, ALU),
    ]);
    error(&code, "write to pending register");
    for schedule in [
        0x706,
        0x7ed,
        ALU | (1 << 17),
        ALU | (2 << 11),
        ALU ^ (1 << 8),
    ] {
        let mut code = initialized();
        code.extend([(iset(0, 1, 2, 1, false, false).0, schedule), (EXIT, ALU)]);
        assert!(FragmentProgram::decode(&program(BIN, &code)).is_err());
    }
}
#[test]
fn both_comparison_sources_retain_rounding_taint_and_cannot_initialize_p0() {
    for source in [1u8, 2] {
        for condition in 1..=6 {
            for bf in [false, true] {
                let mut code = initialized();
                code.extend([
                    (0x5c68000000270100 | u64::from(source), ALU),
                    iset(0, 1, 2, condition, true, bf),
                    (0x5c5800000ff70003, ALU),
                    (EXIT, ALU),
                ]);
                error(&code, "unfused rounding");
            }
        }
    }
    let mut code = initialized();
    code.extend([
        iset(0, 1, 2, 1, false, true),
        (0x5ca0000000370100, ALU),
        (EXIT, ALU),
    ]);
    error(&code, "uninitialized selector predicate P0");
}
#[test]
fn integer_boolean_outputs_erase_resource_and_perspective_proofs() {
    for (bin, at, reg, text) in [
        (
            include_bytes!("fixtures/maxwell/tex.bin").as_slice(),
            1,
            6,
            "TEX handle must originate",
        ),
        (
            include_bytes!("fixtures/maxwell/perspective.bin").as_slice(),
            2,
            7,
            "PositionW",
        ),
        (
            include_bytes!("fixtures/maxwell/perspective.bin").as_slice(),
            3,
            7,
            "PositionW reciprocal",
        ),
    ] {
        for (left, right) in [(reg, 255), (255, reg)] {
            let mut code = items(bin);
            let mut op = iset(reg, left, right, 2, false, true);
            op.1 |= 1 << 11;
            code.insert(at, op);
            let e = FragmentProgram::decode(&program(bin, &code))
                .unwrap_err()
                .to_string();
            assert!(e.contains(text), "expected {text}: {e}");
        }
    }
}
