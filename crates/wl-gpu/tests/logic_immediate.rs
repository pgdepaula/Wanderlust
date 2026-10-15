use wl_gpu::shader_program::{FragmentProgram, LogicalOperator, Operation, VertexProgram};

const BIN: &[u8] = include_bytes!("fixtures/maxwell/lop32i.bin");
const VERTEX: &[u8] = include_bytes!("fixtures/maxwell/vertex-lop32i.bin");
const ALU: u64 = 0x7e6;
const READY: u64 = 0xfe6;
const WRITE: u64 = 0x706;
const NOP: u64 = 0x50b0000000070f00;
const EXIT: u64 = 0xe30000000007000f;

// Independent literal form table: operator bits 54:53, source inversion 55,
// immediate inversion 56. Every form contains the full 32-bit immediate.
const FORMS: [(u64, LogicalOperator, bool, bool); 12] = [
    (0x0400000000070000, LogicalOperator::And, false, false),
    (0x0480000000070000, LogicalOperator::And, true, false),
    (0x0500000000070000, LogicalOperator::And, false, true),
    (0x0580000000070000, LogicalOperator::And, true, true),
    (0x0420000000070000, LogicalOperator::Or, false, false),
    (0x04a0000000070000, LogicalOperator::Or, true, false),
    (0x0520000000070000, LogicalOperator::Or, false, true),
    (0x05a0000000070000, LogicalOperator::Or, true, true),
    (0x0440000000070000, LogicalOperator::Xor, false, false),
    (0x04c0000000070000, LogicalOperator::Xor, true, false),
    (0x0540000000070000, LogicalOperator::Xor, false, true),
    (0x05c0000000070000, LogicalOperator::Xor, true, true),
];
fn instruction(base: u64, dst: u8, src: u8, bits: u32) -> u64 {
    base | u64::from(dst) | (u64::from(src) << 8) | (u64::from(bits) << 20)
}
fn expected(
    dst: u8,
    src: u8,
    bits: u32,
    operator: LogicalOperator,
    invert_src: bool,
    invert_immediate: bool,
) -> Operation {
    Operation::LogicImmediate {
        dst,
        src,
        bits,
        operator,
        invert_src,
        invert_immediate,
    }
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
fn mov32i(dst: u8, bits: u32) -> (u64, u64) {
    (
        0x010000000007f000 | (u64::from(bits) << 20) | u64::from(dst),
        ALU,
    )
}
fn initialized() -> Vec<(u64, u64)> {
    [0x3f800000, 0x3f000000, 0x3e800000, 0x3f800000, 0x80000000]
        .into_iter()
        .enumerate()
        .map(|(r, bits)| mov32i(r as u8, bits))
        .collect()
}

#[test]
fn original_assembly_words_and_all_twelve_forms_have_exact_stage_metadata() {
    assert_eq!((BIN.len(), VERTEX.len()), (144, 144));
    for (offset, word, op, inv_src, inv_imm, bits) in [
        (
            0x60,
            0x04089abcdef70400u64,
            LogicalOperator::And,
            false,
            false,
            0x89abcdef,
        ),
        (
            0x68,
            0x05a89abcdef70401,
            LogicalOperator::Or,
            true,
            true,
            0x89abcdef,
        ),
        (
            0x78,
            0x044ffffffff70402,
            LogicalOperator::Xor,
            false,
            false,
            u32::MAX,
        ),
        (
            0x80,
            0x054ffffffff70403,
            LogicalOperator::Xor,
            false,
            true,
            u32::MAX,
        ),
    ] {
        assert_eq!(
            u64::from_le_bytes(BIN[offset..offset + 8].try_into().unwrap()),
            word
        );
        let p = FragmentProgram::decode(BIN).unwrap();
        let dst = (word & 255) as u8;
        assert!(
            p.operations()
                .contains(&expected(dst, 4, bits, op, inv_src, inv_imm))
        );
    }
    assert_eq!(
        u64::from_le_bytes(VERTEX[0x68..0x70].try_into().unwrap()),
        0x0448000000070000
    );
    for (base, operator, invert_src, invert_immediate) in FORMS {
        let mut bytes = BIN.to_vec();
        change(&mut bytes, 0x60, |_| instruction(base, 0, 4, 0x89abcdef));
        let p = FragmentProgram::decode(&bytes).unwrap();
        assert_eq!(p.required_registers(), 5);
        assert_eq!(
            p.operations()[1],
            expected(0, 4, 0x89abcdef, operator, invert_src, invert_immediate)
        );
        validate(&p.wgsl());
        assert!(VertexProgram::decode(&bytes).is_err());

        let mut bytes = VERTEX.to_vec();
        change(&mut bytes, 0x68, |_| instruction(base, 0, 0, 0x89abcdef));
        let p = VertexProgram::decode(&bytes).unwrap();
        assert_eq!(p.required_registers(), 6);
        assert!(p.operations().contains(&expected(
            0,
            0,
            0x89abcdef,
            operator,
            invert_src,
            invert_immediate
        )));
        validate(&p.wgsl());
        assert!(FragmentProgram::decode(&bytes).is_err());
    }
}

#[test]
fn every_immediate_bit_is_data_including_positions_used_as_gpr_predicate_fields() {
    let mut values = vec![
        0,
        1,
        0x80000000,
        u32::MAX,
        0x89abcdef,
        0x70000000,
        0x10000000,
    ];
    for bit in 0..32 {
        values.push(1 << bit);
        values.push(!(1 << bit));
    }
    for (base, operator, invert_src, invert_immediate) in FORMS {
        for &bits in &values {
            let mut bytes = BIN.to_vec();
            change(&mut bytes, 0x60, |_| instruction(base, 0, 4, bits));
            let p = FragmentProgram::decode(&bytes).unwrap();
            assert_eq!(
                p.operations()[1],
                expected(0, 4, bits, operator, invert_src, invert_immediate)
            );
            // Immediate bits 28..30 occupy instruction bits 48..50. None is a
            // predicate output in this instruction, including an all-zero field.
            assert_eq!(p.required_registers(), 5);
        }
        for bits in [0, 0x80000000, u32::MAX] {
            let mut bytes = VERTEX.to_vec();
            change(&mut bytes, 0x68, |_| instruction(base, 0, 0, bits));
            let p = VertexProgram::decode(&bytes).unwrap();
            assert!(p.operations().contains(&expected(
                0,
                0,
                bits,
                operator,
                invert_src,
                invert_immediate
            )));
            validate(&p.wgsl());
        }
    }
}

#[test]
fn source_initialization_rz_in_place_and_largest_register_are_not_confused_with_immediate() {
    for (base, _, _, _) in FORMS {
        let mut bytes = BIN.to_vec();
        change(&mut bytes, 0x60, |_| instruction(base, 0, 5, u32::MAX));
        error(&bytes, "unready");
        change(&mut bytes, 0x60, |_| instruction(base, 0, 255, u32::MAX));
        let p = FragmentProgram::decode(&bytes).unwrap();
        assert_eq!(p.required_registers(), 5); // RZ is not an allocated GPR.
        validate(&p.wgsl());
        change(&mut bytes, 0x60, |_| instruction(base, 255, 4, u32::MAX));
        error(&bytes, "RZ destination");

        for (dst, src) in [(0, 0), (4, 4), (254, 4), (0, 254)] {
            let mut code = initialized();
            if src == 254 {
                code.push(mov32i(254, 0xdeadbeef));
            }
            code.extend([(instruction(base, dst, src, 0x800000ff), ALU), (EXIT, ALU)]);
            let p = FragmentProgram::decode(&program(BIN, &code)).unwrap();
            assert_eq!(
                p.required_registers(),
                if dst == 254 || src == 254 { 255 } else { 5 }
            );
            validate(&p.wgsl());
        }
    }
}

#[test]
fn immediate_operations_require_ready_sources_free_destinations_and_fixed_alu_schedule() {
    for (base, _, _, _) in FORMS {
        let mut bytes = BIN.to_vec();
        change(&mut bytes, 0x60, |_| instruction(base, 0, 4, 0));
        change(&mut bytes, 0x50, |c| c & !(1 << (21 + 11)));
        error(&bytes, "unready");
        let mut bytes = BIN.to_vec();
        change(&mut bytes, 0x60, |_| instruction(base, 0, 4, 0));
        change(&mut bytes, 0x50, |c| c & !(7 << (21 + 5)));
        error(&bytes, "unready immediate logic operand/control");
        let mut bytes = BIN.to_vec();
        change(&mut bytes, 0x60, |_| instruction(base, 0, 4, 0));
        change(&mut bytes, 0x50, |c| (c & !(31 << 21)) | (13 << 21));
        error(&bytes, "fixed-latency schedule");
        let mut bytes = VERTEX.to_vec();
        change(&mut bytes, 0x68, |_| instruction(base, 0, 0, 0));
        change(&mut bytes, 0x50, |c| c & !(1 << (42 + 11)));
        let e = VertexProgram::decode(&bytes).unwrap_err().to_string();
        assert!(e.contains("unready"), "{e}");

        let mut code = initialized();
        code.extend([
            (0xef9400000107ff06, WRITE),
            (instruction(base, 6, 255, 0), ALU),
            (EXIT, READY),
        ]);
        error(&program(BIN, &code), "write to pending register");
        code[6] = (instruction(base, 0, 6, 0), ALU);
        error(&program(BIN, &code), "unready");
        code[6].1 = READY;
        validate(
            &FragmentProgram::decode(&program(BIN, &code))
                .unwrap()
                .wgsl(),
        );
    }
}

#[test]
fn execution_predicates_cc_x_and_unimplemented_pass_b_are_explicit_failures() {
    for (base, _, _, _) in FORMS {
        // The immediate is zero so CC bit52 cannot be hidden by data overlap.
        for bit in [16, 17, 18, 19, 52, 57, 59, 60, 63] {
            let mut bytes = BIN.to_vec();
            change(&mut bytes, 0x60, |_| {
                instruction(base, 0, 4, 0) ^ (1 << bit)
            });
            error(&bytes, "unsupported instruction");
        }
    }
    for flags in [0, 1 << 55, 1 << 56, (1 << 55) | (1 << 56)] {
        let mut bytes = BIN.to_vec();
        change(&mut bytes, 0x60, |_| {
            instruction(0x0460000000070000 | flags, 0, 4, 0x89abcdef)
        });
        error(&bytes, "unsupported instruction");
    }
}

#[test]
fn logical_immediate_cannot_preserve_constant_handle_or_perspective_origin() {
    for (base, _, _, _) in FORMS {
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
            code.insert(at, (instruction(base, register, register, u32::MAX), READY));
            error(&program(bin, &code), message);
        }
    }
}

#[test]
fn immediate_logic_retains_float_multiply_taint_until_an_actual_untainted_overwrite() {
    for (base, _, _, _) in FORMS {
        for dst in [1, 2] {
            let mut code = items(include_bytes!("fixtures/maxwell/alu.bin"));
            code[5] = (instruction(base, dst, 1, u32::MAX), ALU);
            code[6] = (0x5c5800000ff70003 | (u64::from(dst) << 8), ALU);
            error(&program(BIN, &code), "unfused rounding");
        }
        let mut code = initialized();
        code.extend([
            (0x5c68000000170004, ALU),
            (instruction(base, 4, 2, u32::MAX), ALU),
            (0x5c5800000ff70400, ALU),
            (EXIT, ALU),
        ]);
        validate(
            &FragmentProgram::decode(&program(BIN, &code))
                .unwrap()
                .wgsl(),
        );
    }
}
