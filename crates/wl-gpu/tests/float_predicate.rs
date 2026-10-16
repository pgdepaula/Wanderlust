use wl_gpu::shader_program::{FloatComparison, FragmentProgram, Operation, VertexProgram};

const BIN: &[u8] = include_bytes!("fixtures/maxwell/float-predicate.bin");
const VERTEX: &[u8] = include_bytes!("fixtures/maxwell/vertex-float-predicate.bin");
const ALU: u64 = 0x7e6;
const WAIT: u64 = 0xfe6;
const WRITE: u64 = 0x706;
const COMPARE: u64 = 0x7ed;
const EXIT: u64 = 0xe30000000007000f;
const NOP: u64 = 0x50b0000000070f00;
const MODES: [FloatComparison; 6] = [
    FloatComparison::Less,
    FloatComparison::Equal,
    FloatComparison::LessEqual,
    FloatComparison::Greater,
    FloatComparison::NotEqual,
    FloatComparison::GreaterEqual,
];
fn mov(dst: u8, bits: u32) -> (u64, u64) {
    (
        0x010000000007f000 | (u64::from(bits) << 20) | u64::from(dst),
        ALU,
    )
}
fn compare(mode: u8, left: u8, right: u8) -> (u64, u64) {
    (
        0x5bb0038000070007
            | (u64::from(mode) << 48)
            | (u64::from(left) << 8)
            | (u64::from(right) << 20),
        COMPARE,
    )
}
fn initialized(registers: u8) -> Vec<(u64, u64)> {
    (0..registers).map(|r| mov(r, 0x3f800000)).collect()
}
fn items(bin: &[u8]) -> Vec<(u64, u64)> {
    bin[80..]
        .chunks_exact(32)
        .flat_map(|bundle| {
            let control = u64::from_le_bytes(bundle[..8].try_into().unwrap());
            (0..3).map(move |n| {
                (
                    u64::from_le_bytes(bundle[8 + n * 8..16 + n * 8].try_into().unwrap()),
                    (control >> (n * 21)) & 0x1fffff,
                )
            })
        })
        .collect()
}
fn program(header: &[u8], code: &[(u64, u64)]) -> Vec<u8> {
    let mut out = header[..80].to_vec();
    for group in code.chunks(3) {
        let control = (0..3).fold(0u64, |v, n| {
            v | (group.get(n).map_or(ALU, |instruction| instruction.1) << (n * 21))
        });
        out.extend(control.to_le_bytes());
        for n in 0..3 {
            out.extend(
                group
                    .get(n)
                    .map_or(NOP, |instruction| instruction.0)
                    .to_le_bytes(),
            );
        }
    }
    out
}
fn error(bytes: &[u8], message: &str) {
    let error = FragmentProgram::decode(bytes).unwrap_err().to_string();
    assert!(error.contains(message), "expected {message}: {error}");
}
fn validate(source: &str) {
    let module = naga::front::wgsl::parse_str(source).unwrap();
    naga::valid::Validator::new(
        naga::valid::ValidationFlags::all(),
        naga::valid::Capabilities::empty(),
    )
    .validate(&module)
    .unwrap();
}

#[test]
fn independent_six_ordered_comparisons_and_vertex_predicate_linkage() {
    let p = FragmentProgram::decode(BIN).unwrap();
    assert_eq!(p.required_registers(), 8);
    let decoded: Vec<_> = p
        .operations()
        .iter()
        .filter_map(|op| {
            if let Operation::FloatPredicateCompare {
                left,
                right,
                comparison,
            } = *op
            {
                Some((left, right, comparison))
            } else {
                None
            }
        })
        .collect();
    assert_eq!(decoded, MODES.map(|comparison| (4, 5, comparison)));
    assert_eq!(
        u64::from_le_bytes(BIN[0xa0..0xa8].try_into().unwrap()),
        0x5bb1038000570407
    );
    validate(&p.wgsl());
    let v = VertexProgram::decode(VERTEX).unwrap();
    assert_eq!(v.required_registers(), 8);
    assert!(v.operations().contains(&Operation::FloatPredicateCompare {
        left: 0,
        right: 6,
        comparison: FloatComparison::Less,
    }));
    validate(&v.wgsl());
    assert!(FragmentProgram::decode(VERTEX).is_err());
    assert!(VertexProgram::decode(BIN).is_err());
}

#[test]
fn all_comparison_modes_are_classified_and_rz_is_not_a_gpr_destination() {
    for mode in 0u8..16 {
        for (left, right) in [(0, 1), (255, 0), (0, 255), (255, 255)] {
            let mut code = initialized(4);
            code.extend([compare(mode, left, right), (EXIT, ALU)]);
            let bytes = program(BIN, &code);
            if (1..=6).contains(&mode) {
                let p = FragmentProgram::decode(&bytes).unwrap();
                assert_eq!(
                    p.required_registers(),
                    4,
                    "P0/PT output must not allocate R7"
                );
                assert_eq!(
                    p.operations()[4],
                    Operation::FloatPredicateCompare {
                        left,
                        right,
                        comparison: MODES[usize::from(mode - 1)],
                    }
                );
                validate(&p.wgsl());
            } else {
                error(&bytes, "unsupported ordered floating comparison mode");
            }
        }
    }
}

#[test]
fn unsupported_predicate_outputs_accumulators_boolean_ftz_and_operand_modifiers() {
    // Primary P0 and discarded PT are bits5:3 and2:0. Remaining bits cover
    // operand abs/neg, execution predicate, accumulator/inversion, BoolOp and FTZ.
    for bit in [
        0, 1, 2, 3, 4, 5, 6, 7, 16, 17, 18, 19, 28, 38, 39, 40, 41, 42, 43, 44, 45, 46, 47,
    ] {
        let mut code = initialized(8);
        let mut op = compare(1, 4, 5);
        op.0 ^= 1 << bit;
        code.extend([op, (EXIT, ALU)]);
        error(&program(BIN, &code), "unsupported instruction");
    }
    for predicate in 0u64..7 {
        let mut code = initialized(8);
        let mut op = compare(1, 4, 5);
        op.0 = (op.0 & !(15 << 16)) | (predicate << 16);
        code.extend([op, (EXIT, ALU)]);
        error(&program(BIN, &code), "unsupported instruction");
    }
}

#[test]
fn floating_predicate_has_its_own_latency_readiness_and_barrier_contract() {
    for (left, right) in [(8, 4), (4, 8)] {
        let mut code = initialized(8);
        code.extend([compare(1, left, right), (EXIT, ALU)]);
        error(&program(BIN, &code), "unready floating predicate operand");
    }
    for schedule in [
        COMPARE & !(7 << 5),
        COMPARE | (1 << 17),
        COMPARE & !(7 << 8),
    ] {
        let mut code = initialized(8);
        code.extend([(compare(1, 4, 5).0, schedule), (EXIT, ALU)]);
        assert!(
            FragmentProgram::decode(&program(BIN, &code)).is_err(),
            "schedule {schedule:x}"
        );
    }
    let mut code = initialized(8);
    code.extend([(compare(1, 4, 5).0, ALU), (EXIT, ALU)]);
    error(&program(BIN, &code), "fixed-latency schedule");
    for (left, right) in [(6, 4), (4, 6)] {
        let mut code = initialized(8);
        code.extend([
            (0xef9400000107ff06, WRITE),
            compare(1, left, right),
            (EXIT, ALU),
        ]);
        error(&program(BIN, &code), "unready floating predicate operand");
        code[9].1 |= 1 << 11;
        validate(
            &FragmentProgram::decode(&program(BIN, &code))
                .unwrap()
                .wgsl(),
        );
    }
}

#[test]
fn predicate_output_cannot_initialize_clear_or_ready_gpr_seven() {
    let selector = (0x5ca000000ff70700, ALU); // SEL R0,R7,RZ,P0
    let mut code = initialized(7);
    code.extend([compare(1, 4, 5), selector, (EXIT, ALU)]);
    error(&program(BIN, &code), "unready selector operand");
    let mut code = initialized(7);
    code.extend([
        (0xef9400000107ff07, WRITE),
        compare(1, 4, 5),
        selector,
        (EXIT, ALU),
    ]);
    error(&program(BIN, &code), "unready selector operand");
    code[9].1 = WAIT;
    validate(
        &FragmentProgram::decode(&program(BIN, &code))
            .unwrap()
            .wgsl(),
    );
    // FSETP low byte7 must also retain an existing R7 multiplication hazard.
    let mut code = initialized(8);
    code.extend([
        (0x5c68000000570407, ALU),
        compare(1, 255, 255),
        (0x5c5800000ff70702, ALU),
        (EXIT, ALU),
    ]);
    error(&program(BIN, &code), "unfused rounding");
}

#[test]
fn float_predicate_multiply_taint_survives_selection_and_conditional_move() {
    for multiply_dst in [4u8, 5] {
        for consumer in [
            0x5ca0000000770600u64, // SEL R0,R6,R7,P0
            0x5ca0040000770600,    // SEL R0,R6,R7,!P0
            0x5c98078000600000,    // @P0 MOV R0,R6
            0x5c98078000680000,    // @!P0 MOV R0,R6
        ] {
            let mut code = initialized(8);
            code.extend([
                (0x5c68000000570400 | u64::from(multiply_dst), ALU),
                compare(1, 4, 5),
                (consumer, ALU),
                (0x5c5800000ff70002, ALU),
                (EXIT, ALU),
            ]);
            error(&program(BIN, &code), "unfused rounding");
        }
    }
}

fn inject_compare(bin: &[u8], at: usize) -> Vec<u8> {
    let mut code = items(bin);
    let mut op = compare(1, 255, 255);
    op.1 |= 1 << 11;
    code.insert(at, op);
    program(bin, &code)
}
#[test]
fn predicate_writes_preserve_unrelated_handle_position_and_reciprocal_proofs() {
    let mut code = items(include_bytes!("fixtures/maxwell/tex.bin"));
    code[0].0 = (code[0].0 & !255) | 7; // LDC handle into R7
    code[3].0 = (code[3].0 & !(255 << 20)) | (7 << 20); // TEX uses R7
    let bin = program(include_bytes!("fixtures/maxwell/tex.bin"), &code);
    let p = FragmentProgram::decode(&inject_compare(&bin, 3)).unwrap();
    assert_eq!(p.texture_input(), Some(0));
    validate(&p.wgsl());
    for at in [2, 3] {
        let p = FragmentProgram::decode(&inject_compare(
            include_bytes!("fixtures/maxwell/perspective.bin"),
            at,
        ))
        .unwrap();
        validate(&p.wgsl());
    }
}
