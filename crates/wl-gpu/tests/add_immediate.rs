use wl_gpu::shader_program::{FragmentProgram, Operation, VertexProgram};

const BIN: &[u8] = include_bytes!("fixtures/maxwell/iadd32i.bin");
const VERTEX: &[u8] = include_bytes!("fixtures/maxwell/vertex-iadd32i.bin");
const ALU: u64 = 0x7e6;
const READY: u64 = 0xfe6;
const WRITE: u64 = 0x706;
const NOP: u64 = 0x50b0000000070f00;
const EXIT: u64 = 0xe30000000007000f;
// Source negation is instruction bit56. The immediate remains raw bits51:20.
const FORMS: [(u64, bool); 2] = [(0x1c00000000070000, false), (0x1d00000000070000, true)];

fn instruction(base: u64, dst: u8, src: u8, bits: u32) -> u64 {
    base | u64::from(dst) | (u64::from(src) << 8) | (u64::from(bits) << 20)
}
fn expected(dst: u8, src: u8, bits: u32, negate_src: bool) -> Operation {
    Operation::AddImmediate {
        dst,
        src,
        bits,
        negate_src,
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
fn literal_plain_and_source_negated_words_decode_in_both_stages() {
    assert_eq!((BIN.len(), VERTEX.len()), (144, 144));
    let original = FragmentProgram::decode(BIN).unwrap();
    for (offset, word, dst, bits, negate_src) in [
        (0x60, 0x1c0ffffffff70400u64, 0, u32::MAX, false),
        (0x68, 0x1d08000000070401, 1, 0x80000000, true),
        (0x78, 0x1c089abcdef70402, 2, 0x89abcdef, false),
        (0x80, 0x1d089abcdef70403, 3, 0x89abcdef, true),
    ] {
        assert_eq!(
            u64::from_le_bytes(BIN[offset..offset + 8].try_into().unwrap()),
            word
        );
        assert!(
            original
                .operations()
                .contains(&expected(dst, 4, bits, negate_src))
        );
    }
    assert_eq!(
        u64::from_le_bytes(VERTEX[0x68..0x70].try_into().unwrap()),
        0x1c08000000070000
    );
    for (word, negate_src) in [(0x1c089abcdef70400u64, false), (0x1d089abcdef70400, true)] {
        let mut bytes = BIN.to_vec();
        change(&mut bytes, 0x60, |_| word);
        let p = FragmentProgram::decode(&bytes).unwrap();
        assert_eq!(p.operations()[1], expected(0, 4, 0x89abcdef, negate_src));
        assert_eq!(p.required_registers(), 5);
        validate(&p.wgsl());
        assert!(VertexProgram::decode(&bytes).is_err());
    }
    for (base, negate_src) in FORMS {
        let mut bytes = VERTEX.to_vec();
        change(&mut bytes, 0x68, |_| instruction(base, 0, 0, 0x80000000));
        let p = VertexProgram::decode(&bytes).unwrap();
        assert_eq!(p.required_registers(), 6);
        assert!(
            p.operations()
                .contains(&expected(0, 0, 0x80000000, negate_src))
        );
        validate(&p.wgsl());
        assert!(FragmentProgram::decode(&bytes).is_err());
    }
}

#[test]
fn all_immediate_bits_remain_raw_data_without_signed_extension_or_predicate_confusion() {
    let mut values = vec![
        0,
        1,
        0x7fffffff,
        0x80000000,
        u32::MAX,
        0x89abcdef,
        0x70000000,
    ];
    for bit in 0..32 {
        values.push(1 << bit);
        values.push(!(1 << bit));
    }
    for (base, negate_src) in FORMS {
        for &bits in &values {
            let mut bytes = BIN.to_vec();
            change(&mut bytes, 0x60, |_| instruction(base, 0, 4, bits));
            let p = FragmentProgram::decode(&bytes).unwrap();
            assert_eq!(p.operations()[1], expected(0, 4, bits, negate_src));
            assert_eq!(p.required_registers(), 5);
            // Immediate bits28..30 occupy instruction bits48..50. All eight
            // combinations are valid data, without a predicate-output meaning.
            validate(&p.wgsl());
        }
        for bits in [0, 1, 0x7fffffff, 0x80000000, u32::MAX] {
            let mut bytes = VERTEX.to_vec();
            change(&mut bytes, 0x68, |_| instruction(base, 0, 0, bits));
            let p = VertexProgram::decode(&bytes).unwrap();
            assert!(p.operations().contains(&expected(0, 0, bits, negate_src)));
            validate(&p.wgsl());
        }
    }
}

#[test]
fn source_rz_in_place_and_register_requirements_remain_distinct_from_immediate_data() {
    for (base, negate_src) in FORMS {
        let mut bytes = BIN.to_vec();
        change(&mut bytes, 0x60, |_| instruction(base, 0, 5, u32::MAX));
        error(&bytes, "unready");
        change(&mut bytes, 0x60, |_| instruction(base, 0, 255, u32::MAX));
        let p = FragmentProgram::decode(&bytes).unwrap();
        assert_eq!(p.operations()[1], expected(0, 255, u32::MAX, negate_src));
        assert_eq!(p.required_registers(), 5);
        validate(&p.wgsl());
        change(&mut bytes, 0x60, |_| instruction(base, 255, 4, u32::MAX));
        error(&bytes, "RZ destination");
        for (dst, src) in [(0, 0), (4, 4), (254, 4), (0, 254)] {
            let mut code = initialized();
            if src == 254 {
                code.push(mov32i(254, u32::MAX));
            }
            code.extend([(instruction(base, dst, src, 0x80000000), ALU), (EXIT, ALU)]);
            let p = FragmentProgram::decode(&program(BIN, &code)).unwrap();
            assert_eq!(
                p.required_registers(),
                if src == 254 || dst == 254 { 255 } else { 5 }
            );
            assert!(
                p.operations()
                    .contains(&expected(dst, src, 0x80000000, negate_src))
            );
            validate(&p.wgsl());
        }
    }
}

#[test]
fn fixed_alu_scheduling_cannot_bypass_pending_source_or_destination() {
    // Six cycles is the translator's conservative supported schedule subset,
    // not a claim of exact hardware minimum latency.
    for (base, _) in FORMS {
        let mut bytes = BIN.to_vec();
        change(&mut bytes, 0x60, |_| instruction(base, 0, 4, 0));
        change(&mut bytes, 0x50, |c| c & !(1 << (21 + 11)));
        error(&bytes, "unready");
        let mut bytes = BIN.to_vec();
        change(&mut bytes, 0x60, |_| instruction(base, 0, 4, 0));
        change(&mut bytes, 0x50, |c| c & !(7 << (21 + 5)));
        error(&bytes, "unready immediate addition operand/control");
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
fn predicates_cc_extended_saturating_and_unverified_bit55_are_rejected() {
    for (base, _) in FORMS {
        for bit in [16, 17, 18, 19, 52, 53, 54, 55, 57, 60, 61, 63] {
            let mut bytes = BIN.to_vec();
            change(&mut bytes, 0x60, |_| {
                instruction(base, 0, 4, 0) ^ (1 << bit)
            });
            if base == 0x1c00000000070000 && bit == 57 {
                // This bit belongs to opcode space: IADD32I becomes the newly
                // supported FMUL32I. Require its distinct floating semantics.
                let p = FragmentProgram::decode(&bytes).unwrap();
                assert_eq!(
                    p.operations()[1],
                    Operation::FloatImmediate {
                        dst: 0,
                        src: 4,
                        bits: 0,
                        operator: wl_gpu::shader_program::FloatOperator::Multiply,
                    }
                );
                validate(&p.wgsl());
            } else {
                error(&bytes, "unsupported instruction");
            }
        }
    }
}

#[test]
fn arithmetic_identity_does_not_preserve_handle_or_perspective_proof() {
    for (base, _) in FORMS {
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
            code.insert(at, (instruction(base, register, register, 0), READY));
            error(&program(bin, &code), message);
        }
    }
}

#[test]
fn add_immediate_retains_float_rounding_hazards_until_real_untainted_overwrite() {
    for (base, _) in FORMS {
        for dst in [1, 2] {
            let mut code = items(include_bytes!("fixtures/maxwell/alu.bin"));
            code[5] = (instruction(base, dst, 1, 0), ALU);
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

const SHORT: &[u8] = include_bytes!("fixtures/maxwell/iadd20.bin");
const SHORT_VERTEX: &[u8] = include_bytes!("fixtures/maxwell/vertex-iadd20.bin");
fn short_instruction(dst: u8, src: u8, low: u32, sign: bool, negate_src: bool) -> u64 {
    assert!(low < 524288);
    0x3810000000070000
        | u64::from(dst)
        | (u64::from(src) << 8)
        | (u64::from(low) << 20)
        | (u64::from(sign) << 56)
        | (u64::from(negate_src) << 49)
}
fn short_value(low: u32, sign: bool) -> u32 {
    // Independent mathematical signed20 interpretation, rather than the
    // production high-bit fill expression.
    (i64::from(low) - if sign { 524288 } else { 0 }) as u32
}

#[test]
fn signed_twenty_bit_literals_and_each_payload_bit_separate_sign_from_source_negation() {
    assert_eq!((SHORT.len(), SHORT_VERTEX.len()), (144, 144));
    let original = FragmentProgram::decode(SHORT).unwrap();
    for (offset, word, dst, bits, negate_src) in [
        (0x60, 0x3810007ffff70400u64, 0, 0x0007ffff, false),
        (0x68, 0x3912000000070401, 1, 0xfff80000, true),
        (0x78, 0x3910007ffff70402, 2, 0xffffffff, false),
        (0x80, 0x3812000000170403, 3, 1, true),
    ] {
        assert_eq!(
            u64::from_le_bytes(SHORT[offset..offset + 8].try_into().unwrap()),
            word
        );
        assert!(
            original
                .operations()
                .contains(&expected(dst, 4, bits, negate_src))
        );
    }
    assert_eq!(
        u64::from_le_bytes(SHORT_VERTEX[0x68..0x70].try_into().unwrap()),
        0x3910000000070000
    );
    let mut lows = vec![0, 1, 0x3ffff, 0x40000, 0x7fffe, 0x7ffff];
    for bit in 0..19 {
        lows.push(1 << bit);
        lows.push(0x7ffff ^ (1 << bit));
    }
    for sign in [false, true] {
        for negate_src in [false, true] {
            for &low in &lows {
                let bits = short_value(low, sign);
                // These are explicitly mutated binary encodings. In particular,
                // textual zero may assemble as a GPR-RZ operand instead.
                let mut bytes = SHORT.to_vec();
                change(&mut bytes, 0x60, |_| {
                    short_instruction(0, 4, low, sign, negate_src)
                });
                let p = FragmentProgram::decode(&bytes).unwrap();
                assert_eq!(p.operations()[1], expected(0, 4, bits, negate_src));
                assert_eq!(p.required_registers(), 5);
                validate(&p.wgsl());
                let mut bytes = SHORT_VERTEX.to_vec();
                change(&mut bytes, 0x68, |_| {
                    short_instruction(0, 0, low, sign, negate_src)
                });
                let p = VertexProgram::decode(&bytes).unwrap();
                assert!(p.operations().contains(&expected(0, 0, bits, negate_src)));
                assert_eq!(p.required_registers(), 6);
                validate(&p.wgsl());
            }
        }
    }
}

#[test]
fn signed_twenty_bit_fields_register_bounds_and_scoreboard_are_strict() {
    for sign in [false, true] {
        for negate_src in [false, true] {
            let valid = short_instruction(0, 4, 0x7ffff, sign, negate_src);
            for bit in [
                16, 17, 18, 19, 39, 40, 41, 42, 43, 44, 45, 46, 47, 48, 50, 51, 52, 53, 54, 55, 57,
            ] {
                let mut bytes = SHORT.to_vec();
                change(&mut bytes, 0x60, |_| valid ^ (1 << bit));
                error(&bytes, "unsupported instruction");
            }
            let mut bytes = SHORT.to_vec();
            change(&mut bytes, 0x60, |_| {
                short_instruction(0, 5, 0, sign, negate_src)
            });
            error(&bytes, "unready");
            change(&mut bytes, 0x60, |_| {
                short_instruction(0, 255, 0, sign, negate_src)
            });
            validate(&FragmentProgram::decode(&bytes).unwrap().wgsl());
            change(&mut bytes, 0x60, |_| {
                short_instruction(255, 4, 0, sign, negate_src)
            });
            error(&bytes, "RZ destination");
            for (dst, src) in [(4, 4), (254, 4), (0, 254)] {
                let mut code = initialized();
                if src == 254 {
                    code.push(mov32i(254, 0x80000000));
                }
                code.extend([
                    (short_instruction(dst, src, 0, sign, negate_src), ALU),
                    (EXIT, ALU),
                ]);
                let p = FragmentProgram::decode(&program(SHORT, &code)).unwrap();
                assert_eq!(
                    p.required_registers(),
                    if src == 254 || dst == 254 { 255 } else { 5 }
                );
                validate(&p.wgsl());
            }
            for (field, mask, replacement, message) in [
                (21, 1 << 11, 0, "unready"),
                (21, 7 << 5, 0, "unready immediate addition operand/control"),
                (21, 31, 13, "fixed-latency schedule"),
            ] {
                let mut bytes = SHORT.to_vec();
                change(&mut bytes, 0x60, |_| valid);
                change(&mut bytes, 0x50, |c| {
                    (c & !(mask << field)) | (replacement << field)
                });
                error(&bytes, message);
            }
            let mut code = initialized();
            code.extend([
                (0xef9400000107ff06, WRITE),
                (short_instruction(6, 255, 0, sign, negate_src), ALU),
                (EXIT, READY),
            ]);
            error(&program(SHORT, &code), "write to pending register");
            code[6] = (short_instruction(0, 6, 0, sign, negate_src), ALU);
            error(&program(SHORT, &code), "unready");
            code[6].1 = READY;
            validate(
                &FragmentProgram::decode(&program(SHORT, &code))
                    .unwrap()
                    .wgsl(),
            );
        }
    }
}

#[test]
fn signed_twenty_bit_addition_erases_proofs_but_keeps_source_float_hazards() {
    for sign in [false, true] {
        for negate_src in [false, true] {
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
                code.insert(
                    at,
                    (
                        short_instruction(register, register, 0, sign, negate_src),
                        READY,
                    ),
                );
                error(&program(bin, &code), message);
            }
            for dst in [1, 2] {
                let mut code = items(include_bytes!("fixtures/maxwell/alu.bin"));
                code[5] = (short_instruction(dst, 1, 0, sign, negate_src), ALU);
                code[6] = (0x5c5800000ff70003 | (u64::from(dst) << 8), ALU);
                error(&program(SHORT, &code), "unfused rounding");
            }
            let mut code = initialized();
            code.extend([
                (0x5c68000000170004, ALU),
                (short_instruction(4, 2, 0, sign, negate_src), ALU),
                (0x5c5800000ff70400, ALU),
                (EXIT, ALU),
            ]);
            validate(
                &FragmentProgram::decode(&program(SHORT, &code))
                    .unwrap()
                    .wgsl(),
            );
        }
    }
}
