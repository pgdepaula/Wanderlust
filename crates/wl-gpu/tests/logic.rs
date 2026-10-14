use wl_gpu::shader_program::{
    FragmentProgram, IntegerOperator, LogicalOperator, Operation, VertexProgram,
};

const BIN: &[u8] = include_bytes!("fixtures/maxwell/lop-invert.bin");
const BYTES: &[u8] = include_bytes!("fixtures/maxwell/lop-bytes.bin");
const VERTEX: &[u8] = include_bytes!("fixtures/maxwell/vertex-lop.bin");
const ALU: u64 = 0x7e6;
const READY: u64 = 0xfe6;
const WRITE: u64 = 0x706;
const NOP: u64 = 0x50b0000000070f00;
const EXIT: u64 = 0xe30000000007000f;

// Literal GM107 encodings. The three old, uninverted IR variants remain stable.
// PASS_B has only the canonical discarded A=RZ, without an A inversion.
const FORMS: [(u64, IntegerOperator); 14] = [
    (0x5c47000000070000, IntegerOperator::And),
    (
        0x5c47008000070000,
        logical(LogicalOperator::And, true, false),
    ),
    (
        0x5c47010000070000,
        logical(LogicalOperator::And, false, true),
    ),
    (
        0x5c47018000070000,
        logical(LogicalOperator::And, true, true),
    ),
    (0x5c47020000070000, IntegerOperator::Or),
    (
        0x5c47028000070000,
        logical(LogicalOperator::Or, true, false),
    ),
    (
        0x5c47030000070000,
        logical(LogicalOperator::Or, false, true),
    ),
    (0x5c47038000070000, logical(LogicalOperator::Or, true, true)),
    (0x5c47040000070000, IntegerOperator::Xor),
    (
        0x5c47048000070000,
        logical(LogicalOperator::Xor, true, false),
    ),
    (
        0x5c47050000070000,
        logical(LogicalOperator::Xor, false, true),
    ),
    (
        0x5c47058000070000,
        logical(LogicalOperator::Xor, true, true),
    ),
    (
        0x5c47060000070000,
        logical(LogicalOperator::PassB, false, false),
    ),
    (
        0x5c47070000070000,
        logical(LogicalOperator::PassB, false, true),
    ),
];
const fn logical(
    operator: LogicalOperator,
    invert_left: bool,
    invert_right: bool,
) -> IntegerOperator {
    IntegerOperator::Logical {
        operator,
        invert_left,
        invert_right,
    }
}
fn pass_b(operator: IntegerOperator) -> bool {
    matches!(
        operator,
        IntegerOperator::Logical {
            operator: LogicalOperator::PassB,
            ..
        }
    )
}
fn instruction(base: u64, dst: u8, left: u8, right: u8) -> u64 {
    base | u64::from(dst) | (u64::from(left) << 8) | (u64::from(right) << 20)
}
fn change(bytes: &mut [u8], offset: usize, f: impl FnOnce(u64) -> u64) {
    let word = u64::from_le_bytes(bytes[offset..offset + 8].try_into().unwrap());
    bytes[offset..offset + 8].copy_from_slice(&f(word).to_le_bytes());
}
fn validate(wgsl: &str) {
    let module = naga::front::wgsl::parse_str(wgsl).unwrap();
    naga::valid::Validator::new(
        naga::valid::ValidationFlags::all(),
        naga::valid::Capabilities::empty(),
    )
    .validate(&module)
    .unwrap();
}
fn error(bytes: &[u8], message: &str) {
    let actual = FragmentProgram::decode(bytes).unwrap_err().to_string();
    assert!(actual.contains(message), "expected {message}: {actual}");
}
fn items(bytes: &[u8]) -> Vec<(u64, u64)> {
    bytes[80..]
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
        let control = (0..3).fold(0, |v, n| {
            v | (group.get(n).map_or(ALU, |x| x.1) << (n * 21))
        });
        out.extend(control.to_le_bytes());
        for n in 0..3 {
            out.extend(group.get(n).map_or(NOP, |x| x.0).to_le_bytes());
        }
    }
    out
}
fn initialized() -> Vec<(u64, u64)> {
    [
        0x3f800000u32,
        0x3f000000,
        0x3e800000,
        0x3f800000,
        0x80000000,
        0xffffffff,
    ]
    .into_iter()
    .enumerate()
    .map(|(r, bits)| (0x010000000007f000 | (u64::from(bits) << 20) | r as u64, ALU))
    .collect()
}

#[test]
fn fourteen_literal_forms_preserve_legacy_ir_and_validate_in_both_stages() {
    assert_eq!((BIN.len(), BYTES.len(), VERTEX.len()), (144, 464, 176));
    for (offset, word) in [
        (0x60, 0x5c47008000570400u64),
        (0x68, 0x5c47030000570401),
        (0x78, 0x5c47058000570402),
        (0x80, 0x5c4707000047ff03),
    ] {
        assert_eq!(
            u64::from_le_bytes(BIN[offset..offset + 8].try_into().unwrap()),
            word
        );
    }
    for (base, operator) in FORMS {
        let left = if pass_b(operator) { 255 } else { 4 };
        let mut bytes = BIN.to_vec();
        change(&mut bytes, 0x60, |_| instruction(base, 0, left, 5));
        let p = FragmentProgram::decode(&bytes).unwrap();
        assert_eq!(p.required_registers(), 6);
        assert_eq!(
            p.operations()[2],
            Operation::Integer {
                dst: 0,
                left,
                right: 5,
                operator
            }
        );
        validate(&p.wgsl());
        assert!(VertexProgram::decode(&bytes).is_err());

        let left = if pass_b(operator) { 255 } else { 0 };
        let mut bytes = VERTEX.to_vec();
        change(&mut bytes, 0x78, |_| instruction(base, 0, left, 6));
        let p = VertexProgram::decode(&bytes).unwrap();
        assert_eq!(p.required_registers(), 7);
        assert!(p.operations().contains(&Operation::Integer {
            dst: 0,
            left,
            right: 6,
            operator
        }));
        validate(&p.wgsl());
        assert!(FragmentProgram::decode(&bytes).is_err());
    }
    let p = FragmentProgram::decode(BYTES).unwrap();
    assert_eq!(p.required_registers(), 13);
    assert_eq!(
        p.operations()[7],
        Operation::Integer {
            dst: 9,
            left: 4,
            right: 5,
            operator: logical(LogicalOperator::And, true, false),
        }
    );
    validate(&p.wgsl());
}

#[test]
fn source_initialization_zero_register_and_in_place_reads_are_checked() {
    for (base, operator) in FORMS {
        let left = if pass_b(operator) { 255 } else { 4 };
        for field in if pass_b(operator) {
            &[20][..]
        } else {
            &[8, 20][..]
        } {
            let mut bytes = BIN.to_vec();
            change(&mut bytes, 0x60, |_| instruction(base, 0, left, 5));
            change(&mut bytes, 0x60, |w| (w & !(255 << field)) | (9 << field));
            error(&bytes, "unready integer");
            change(&mut bytes, 0x60, |w| w | (255 << field));
            validate(&FragmentProgram::decode(&bytes).unwrap().wgsl());
        }
        for dst in [0, 4, 5, 254] {
            let mut code = initialized();
            code.extend([(instruction(base, dst, left, 5), ALU), (EXIT, ALU)]);
            let p = FragmentProgram::decode(&program(BIN, &code)).unwrap();
            assert_eq!(p.required_registers(), if dst == 254 { 255 } else { 6 });
            validate(&p.wgsl());
        }
        let mut code = initialized();
        code.extend([(instruction(base, 0, 255, 255), ALU), (EXIT, ALU)]);
        validate(
            &FragmentProgram::decode(&program(BIN, &code))
                .unwrap()
                .wgsl(),
        );
        let mut bytes = BIN.to_vec();
        change(&mut bytes, 0x60, |_| instruction(base, 255, left, 5));
        error(&bytes, "RZ destination");
    }
}

#[test]
fn logical_sources_destinations_and_fixed_latency_obey_the_scoreboard() {
    for (base, operator) in FORMS {
        let left = if pass_b(operator) { 255 } else { 4 };
        let mut bytes = BIN.to_vec();
        change(&mut bytes, 0x60, |_| instruction(base, 0, left, 5));
        change(&mut bytes, 0x50, |c| c & !(1 << (21 + 11)));
        error(&bytes, "unready integer");
        let mut bytes = BIN.to_vec();
        change(&mut bytes, 0x60, |_| instruction(base, 0, left, 5));
        change(&mut bytes, 0x50, |c| c & !(7 << (21 + 5)));
        error(&bytes, "unready integer");
        let mut bytes = BIN.to_vec();
        change(&mut bytes, 0x60, |_| instruction(base, 0, left, 5));
        change(&mut bytes, 0x50, |c| (c & !(31 << 21)) | (13 << 21));
        error(&bytes, "fixed-latency schedule");

        let mut code = initialized();
        code.extend([
            (0xef9500000107ff06, WRITE), // LDC R6 remains pending.
            (instruction(base, 6, 255, 255), ALU),
            (EXIT, READY),
        ]);
        error(&program(BIN, &code), "write to pending register");
        code[7] = (instruction(base, 0, left, 6), ALU);
        error(&program(BIN, &code), "unready integer");
        code[7].1 = READY;
        validate(
            &FragmentProgram::decode(&program(BIN, &code))
                .unwrap()
                .wgsl(),
        );
    }
    let mut vertex = VERTEX.to_vec();
    change(&mut vertex, 0x50, |c| c & !(1 << (42 + 11)));
    let error = VertexProgram::decode(&vertex).unwrap_err().to_string();
    // MOV R6 is independent of the pending UV load. The original LOP consumes
    // ready position/R6, so missing the UV wait is first exposed by AST UV.
    assert!(error.contains("invalid/unready AST group"), "{error}");
    // Make LOP itself consume pending UV R4 and require rejection there.
    change(&mut vertex, 0x78, |word| (word & !(255 << 20)) | (4 << 20));
    let error = VertexProgram::decode(&vertex).unwrap_err().to_string();
    assert!(error.contains("unready integer"), "{error}");
}

#[test]
fn cc_extended_predicate_condition_and_alternate_sources_are_not_accepted() {
    let valid = 0x5c47008000570400u64;
    // Preserve initialized sources so a negative is a real encoding failure.
    for bit in [16, 19, 28, 38, 43, 44, 45, 46, 47, 48, 49, 50, 51] {
        let mut bytes = BIN.to_vec();
        change(&mut bytes, 0x60, |_| valid ^ (1 << bit));
        error(&bytes, "unsupported instruction");
    }
    for word in [
        0x3847008000570400, // Immediate operand family.
        0x4c47008000570400, // Constant operand family.
        0x5c47060000570400, // PASS_B with noncanonical initialized A=R4.
        0x5c4706800057ff00, // PASS_B with inverted RZ A.
        0x5c4707800057ff00, // Invert both inputs of PASS_B.
    ] {
        let mut bytes = BIN.to_vec();
        change(&mut bytes, 0x60, |_| word);
        error(&bytes, "unsupported instruction");
    }
}

#[test]
fn every_logical_form_erases_resource_and_perspective_provenance() {
    for (base, operator) in FORMS {
        for (bin, at, register, message) in [
            (
                include_bytes!("fixtures/maxwell/tex.bin").as_slice(),
                3,
                6,
                "TEX handle",
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
                "reciprocal",
            ),
        ] {
            let mut code = items(bin);
            let (left, right) = if pass_b(operator) {
                (255, register)
            } else {
                (register, 255)
            };
            code.insert(at, (instruction(base, register, left, right), READY));
            error(&program(bin, &code), message);
        }
    }
}

#[test]
fn integer_identity_and_inversion_cannot_hide_unfused_float_multiply_hazards() {
    for (base, operator) in FORMS {
        let sources = if pass_b(operator) {
            &[(255, 1)][..]
        } else {
            &[(1, 255), (255, 1)][..]
        };
        for &(left, right) in sources {
            for dst in [1, 2] {
                let mut code = items(include_bytes!("fixtures/maxwell/alu.bin"));
                code[5] = (instruction(base, dst, left, right), ALU);
                code[6] = (0x5c5800000ff70003 | (u64::from(dst) << 8), ALU);
                error(&program(BIN, &code), "unfused rounding");
            }
        }
        let mut code = initialized();
        code.extend([
            (0x5c68000000170004, ALU), // FMUL R4,R0,R1.
            (
                instruction(base, 4, if pass_b(operator) { 255 } else { 2 }, 3),
                ALU,
            ),
            (0x5c5800000ff70400, ALU), // FADD R0,R4,RZ, now untainted.
            (EXIT, ALU),
        ]);
        validate(
            &FragmentProgram::decode(&program(BIN, &code))
                .unwrap()
                .wgsl(),
        );
    }
}
