use wl_gpu::shader_program::{FloatComparison, FragmentProgram, Operation, VertexProgram};
const BIN: &[u8] = include_bytes!("fixtures/maxwell/float-set.bin");
const FLOAT: &[u8] = include_bytes!("fixtures/maxwell/float-set-float.bin");
const VERTEX: &[u8] = include_bytes!("fixtures/maxwell/vertex-float-set.bin");
const ALU: u64 = 0x7e6;
const WRITE: u64 = 0x706;
const READY: u64 = 0xfe6;
const EXIT: u64 = 0xe30000000007000f;
const NOP: u64 = 0x50b0000000070f00;
const MODES: [FloatComparison; 16] = [
    FloatComparison::False,
    FloatComparison::Less,
    FloatComparison::Equal,
    FloatComparison::LessEqual,
    FloatComparison::Greater,
    FloatComparison::NotEqual,
    FloatComparison::GreaterEqual,
    FloatComparison::Numeric,
    FloatComparison::Nan,
    FloatComparison::LessUnordered,
    FloatComparison::EqualUnordered,
    FloatComparison::LessEqualUnordered,
    FloatComparison::GreaterUnordered,
    FloatComparison::NotEqualUnordered,
    FloatComparison::GreaterEqualUnordered,
    FloatComparison::True,
];
fn fset(dst: u8, left: u8, right: u8, mode: u8, float_result: bool) -> u64 {
    0x5800038000070000
        | u64::from(dst)
        | (u64::from(left) << 8)
        | (u64::from(right) << 20)
        | (u64::from(mode) << 48)
        | (u64::from(float_result) << 52)
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
fn independently_assembled_gpr_results_in_both_shader_stages() {
    for (bin, float_result) in [(BIN, false), (FLOAT, true)] {
        assert_eq!(
            u64::from_le_bytes(bin[0x78..0x80].try_into().unwrap()),
            fset(4, 4, 5, 1, float_result)
        );
        let p = FragmentProgram::decode(bin).unwrap();
        assert!(p.operations().contains(&Operation::FloatSet {
            dst: 4,
            left: 4,
            right: 5,
            comparison: FloatComparison::Less,
            float_result
        }));
        assert_eq!(p.required_registers(), 8);
        validate(&p.wgsl());
        assert!(VertexProgram::decode(bin).is_err());
    }
    assert_eq!(
        u64::from_le_bytes(VERTEX[0x78..0x80].try_into().unwrap()),
        fset(6, 0, 7, 1, true)
    );
    let v = VertexProgram::decode(VERTEX).unwrap();
    assert!(v.operations().contains(&Operation::FloatSet {
        dst: 6,
        left: 0,
        right: 7,
        comparison: FloatComparison::Less,
        float_result: true
    }));
    assert_eq!(v.required_registers(), 10);
    validate(&v.wgsl());
    assert!(FragmentProgram::decode(VERTEX).is_err());
}
#[test]
fn all_comparisons_and_result_modes_use_scoped_condition_without_p0() {
    for (mode, &comparison) in MODES.iter().enumerate() {
        for float_result in [false, true] {
            let mut code = initialized();
            code.extend([(fset(0, 1, 2, mode as u8, float_result), ALU), (EXIT, ALU)]);
            let p = FragmentProgram::decode(&program(BIN, &code)).unwrap();
            assert!(p.operations().contains(&Operation::FloatSet {
                dst: 0,
                left: 1,
                right: 2,
                comparison,
                float_result
            }));
            let s = p.wgsl();
            assert!(!s.contains("p0"));
            let bits = if float_result {
                0x3f800000u32
            } else {
                u32::MAX
            };
            assert!(s.contains(&format!("r[0]=select(0u,{bits}u,condition);")));
            validate(&s);
        }
    }
}
#[test]
fn aliases_rz_and_high_operands_have_correct_gpr_metadata() {
    for src in [0, 1, 2, 3, 4, 16, 64, 128, 254, 255u8] {
        for (left, right) in [(src, 255), (255, src)] {
            for float_result in [false, true] {
                let mut code = initialized();
                if src > 3 && src != 255 {
                    code.push(immediate(src));
                }
                let dst = if src == 255 { 0 } else { src };
                code.extend([(fset(dst, left, right, 2, float_result), ALU), (EXIT, ALU)]);
                let p = FragmentProgram::decode(&program(BIN, &code)).unwrap();
                assert_eq!(
                    p.required_registers(),
                    if src == 255 {
                        4
                    } else {
                        u16::from(src).max(3) + 1
                    }
                );
                validate(&p.wgsl());
            }
        }
    }
}
#[test]
fn source_readiness_and_pending_destinations_are_enforced() {
    for mode in [0, 1, 15] {
        let mut code = initialized();
        code.extend([(fset(0, 4, 1, mode, false), ALU), (EXIT, ALU)]);
        error(&code, "unready FSET operand");
        code[4].0 = fset(0, 1, 4, mode, false);
        error(&code, "unready FSET operand");
    }
    let mut code = initialized();
    code.extend([
        (0xef9400000107ff04, WRITE),
        (fset(0, 4, 255, 1, true), ALU),
        (EXIT, ALU),
    ]);
    error(&code, "unready FSET operand");
    code[5].1 = READY;
    validate(
        &FragmentProgram::decode(&program(BIN, &code))
            .unwrap()
            .wgsl(),
    );
    code[5] = (fset(4, 1, 2, 1, false), ALU);
    error(&code, "write to pending register");
}
#[test]
fn unsupported_modifiers_predicates_sources_and_latency_fail_explicitly() {
    for bit in [
        16, 17, 18, 19, 28, 32, 38, 39, 40, 41, 42, 43, 44, 45, 46, 47, 53, 54, 55, 56,
    ] {
        let mut code = initialized();
        code.extend([(fset(0, 1, 2, 1, false) ^ (1 << bit), ALU), (EXIT, ALU)]);
        error(&code, "unsupported instruction");
    }
    for word in [0x3001038000170100, 0x4801038001070100] {
        let mut code = initialized();
        code.extend([(word, ALU), (EXIT, ALU)]);
        error(&code, "unsupported instruction");
    }
    for schedule in [WRITE, 0x7ed, ALU | (1 << 17), ALU ^ (1 << 8)] {
        let mut code = initialized();
        code.extend([(fset(0, 1, 2, 1, false), schedule), (EXIT, ALU)]);
        assert!(FragmentProgram::decode(&program(BIN, &code)).is_err());
    }
    let mut code = initialized();
    code.extend([(fset(255, 1, 2, 1, false), ALU), (EXIT, ALU)]);
    error(&code, "RZ destination unsupported");
}
#[test]
fn fset_does_not_initialize_predicates_and_retains_operand_rounding_hazards() {
    let mut code = initialized();
    code.extend([
        (fset(0, 1, 2, 2, true), ALU),
        (0x5ca0000000370100, ALU),
        (EXIT, ALU),
    ]);
    error(&code, "uninitialized selector predicate P0");
    for (left, right) in [(1, 2), (2, 1)] {
        let mut code = initialized();
        code.extend([
            (0x5c68000000270101, ALU),
            (fset(0, left, right, 1, false), ALU),
            (0x5c5800000ff70002, ALU),
            (EXIT, ALU),
        ]);
        error(&code, "unfused rounding");
    }
}
#[test]
fn gpr_comparison_result_cannot_inherit_resource_proofs() {
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
        let mut code = items(bin);
        code.insert(at, (fset(reg, reg, 255, 1, true), READY));
        let e = FragmentProgram::decode(&program(bin, &code))
            .unwrap_err()
            .to_string();
        assert!(e.contains(text), "expected {text}: {e}");
    }
}
