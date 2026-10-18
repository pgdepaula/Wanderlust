use wl_gpu::shader_program::{FragmentProgram, Operation, VertexProgram};
const BIN: &[u8] = include_bytes!("fixtures/maxwell/conditional-immediate.bin");
const VERTEX: &[u8] = include_bytes!("fixtures/maxwell/vertex-conditional-immediate.bin");
const ALU: u64 = 0x7e6;
const EXIT: u64 = 0xe30000000007000f;
const NOP: u64 = 0x50b0000000070f00;
fn conditional(dst: u8, bits: u32, invert: bool) -> (u64, u64) {
    (
        0x010000000000f000 | (u64::from(bits) << 20) | (u64::from(invert) << 19) | u64::from(dst),
        ALU,
    )
}
fn compare(left: u8, right: u8) -> (u64, u64) {
    (
        0x5b62038000070007 | (u64::from(left) << 8) | (u64::from(right) << 20),
        0x7ed,
    )
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
fn independent_immediate_predication_fixture_preserves_both_stage_domains() {
    assert_eq!(
        u64::from_le_bytes(BIN[0xa0..0xa8].try_into().unwrap()),
        0x0103f8000000f000
    );
    assert_eq!(
        u64::from_le_bytes(BIN[0xa8..0xb0].try_into().unwrap()),
        0x010000000008f002
    );
    let p = FragmentProgram::decode(BIN).unwrap();
    assert_eq!(p.required_registers(), 6);
    assert!(p.operations().contains(&Operation::ConditionalImmediate {
        dst: 0,
        bits: 0x3f800000,
        invert: false
    }));
    assert!(p.operations().contains(&Operation::ConditionalImmediate {
        dst: 2,
        bits: 0,
        invert: true
    }));
    validate(&p.wgsl());
    assert!(VertexProgram::decode(BIN).is_err());
    assert_eq!(
        u64::from_le_bytes(VERTEX[0x80..0x88].try_into().unwrap()),
        0x0103f0000000f000
    );
    let v = VertexProgram::decode(VERTEX).unwrap();
    assert_eq!(v.required_registers(), 7);
    assert!(v.operations().contains(&Operation::ConditionalImmediate {
        dst: 0,
        bits: 0x3f000000,
        invert: false
    }));
    validate(&v.wgsl());
    assert!(FragmentProgram::decode(VERTEX).is_err());
}
#[test]
fn all_immediate_bits_are_data_and_only_the_old_destination_is_a_register() {
    let mut values = vec![
        0,
        1,
        u32::MAX,
        0x80000000,
        0x7fffffff,
        0xdeadbeef,
        0x7f800000,
        0xff800000,
        0x7fc00001,
        0x7f800001,
    ];
    values.extend((0..32).map(|bit| 1u32 << bit));
    for bits in values {
        for invert in [false, true] {
            for dst in [0u8, 4, 128, 254] {
                let mut code = initialized();
                if dst > 3 {
                    code.push(immediate(dst));
                }
                code.extend([compare(1, 2), conditional(dst, bits, invert), (EXIT, ALU)]);
                let p = FragmentProgram::decode(&program(BIN, &code)).unwrap();
                assert!(p.operations().contains(&Operation::ConditionalImmediate {
                    dst,
                    bits,
                    invert
                }));
                assert_eq!(p.required_registers(), 4.max(u16::from(dst) + 1));
                validate(&p.wgsl());
            }
        }
    }
    assert_eq!(conditional(254, 0xdeadbeef, false).0, 0x010deadbeef0f0fe);
    assert_eq!(conditional(254, u32::MAX, true).0, 0x010ffffffff8f0fe);
}
#[test]
fn predicate_and_old_destination_must_be_initialized_even_for_a_constant_guard() {
    for invert in [false, true] {
        let mut code = initialized();
        code.extend([conditional(0, 0, invert), (EXIT, ALU)]);
        error(&code, "uninitialized conditional MOV32I predicate P0");
        let mut code = initialized();
        code.extend([compare(255, 255), conditional(4, 0, invert), (EXIT, ALU)]);
        error(&code, "unready conditional MOV32I destination");
        // EQ RZ,RZ is true, but the supported dataflow contract still requires the false path.
        let mut code = initialized();
        code.extend([
            (0x5b6403800ff7ff07, 0x7ed),
            conditional(4, 0, invert),
            (EXIT, ALU),
        ]);
        error(&code, "unready conditional MOV32I destination");
        let mut code = initialized();
        code.extend([compare(255, 255), conditional(255, 0, invert), (EXIT, ALU)]);
        error(&code, "RZ destination unsupported");
    }
}
#[test]
fn partial_lanes_other_predicates_and_reserved_fields_remain_unsupported() {
    for invert in [false, true] {
        for lanes in 0..15u64 {
            let mut code = initialized();
            let mut op = conditional(0, 0x3f800000, invert);
            op.0 = (op.0 & !(15 << 12)) | (lanes << 12);
            code.extend([compare(1, 2), op, (EXIT, ALU)]);
            error(&code, "unsupported instruction");
        }
        for predicate in 1..7u64 {
            let mut code = initialized();
            let mut op = conditional(0, 0x3f800000, invert);
            op.0 |= predicate << 16;
            code.extend([compare(1, 2), op, (EXIT, ALU)]);
            error(&code, "unsupported instruction");
        }
        for bit in [8, 9, 10, 11, 52, 53, 54, 55] {
            let mut code = initialized();
            let mut op = conditional(0, 0x3f800000, invert);
            op.0 ^= 1 << bit;
            code.extend([compare(1, 2), op, (EXIT, ALU)]);
            error(&code, "unsupported instruction");
        }
    }
    let mut code = initialized();
    let mut op = conditional(4, 0xdeadbeef, false);
    op.0 |= 7 << 16;
    code.extend([op, (EXIT, ALU)]);
    let p = FragmentProgram::decode(&program(BIN, &code)).unwrap();
    assert!(p.operations().contains(&Operation::Immediate {
        dst: 4,
        bits: 0xdeadbeef
    }));
    validate(&p.wgsl());
    code[4].0 |= 1 << 19;
    error(&code, "unsupported instruction");
}
#[test]
fn pending_false_path_requires_a_real_wait_and_fixed_latency_is_enforced() {
    for invert in [false, true] {
        let mut code = initialized();
        code.extend([
            compare(1, 2),
            (0xef9400000107ff00, 0x706),
            conditional(0, 0, invert),
            (EXIT, ALU),
        ]);
        error(&code, "unready conditional MOV32I destination");
        code[6].1 |= 1 << 11;
        validate(
            &FragmentProgram::decode(&program(BIN, &code))
                .unwrap()
                .wgsl(),
        );
        // A different pending register is not read by immediate predication.
        let mut code = initialized();
        code.extend([
            compare(1, 2),
            (0xef9400000107ff04, 0x706),
            conditional(0, 0, invert),
            (EXIT, ALU),
        ]);
        validate(
            &FragmentProgram::decode(&program(BIN, &code))
                .unwrap()
                .wgsl(),
        );
        for schedule in [
            0x706,
            0x7ed,
            ALU | (1 << 17),
            ALU | (2 << 11),
            ALU ^ (1 << 8),
        ] {
            let mut code = initialized();
            code.extend([
                compare(1, 2),
                (conditional(0, 0, invert).0, schedule),
                (EXIT, ALU),
            ]);
            assert!(FragmentProgram::decode(&program(BIN, &code)).is_err());
        }
    }
}
#[test]
fn old_destination_and_predicate_rounding_hazards_survive_conditional_constants() {
    for invert in [false, true] {
        for source in [0u8, 4] {
            let mut code = initialized();
            code.push((0x5c68000000270100 | u64::from(source), ALU));
            code.push(if source == 4 {
                compare(4, 1)
            } else {
                compare(255, 255)
            });
            code.extend([
                conditional(0, 0, invert),
                (0x5c5800000ff70002, ALU),
                (EXIT, ALU),
            ]);
            error(&code, "unfused rounding");
            // A genuine unconditional overwrite replaces both guarded values and their taint.
            code.insert(7, (0x010000000007f000, ALU));
            validate(
                &FragmentProgram::decode(&program(BIN, &code))
                    .unwrap()
                    .wgsl(),
            );
        }
    }
}
#[test]
fn even_false_guarded_constants_clear_resource_and_position_proofs() {
    for (bin, at, reg, text) in [
        (
            include_bytes!("fixtures/maxwell/tex.bin").as_slice(),
            3,
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
            "reciprocal",
        ),
    ] {
        for invert in [false, true] {
            let mut code = items(bin);
            let mut cmp = compare(255, 255);
            cmp.1 |= 1 << 11;
            code.insert(at, cmp);
            code.insert(at + 1, conditional(reg, 0, invert));
            let e = FragmentProgram::decode(&program(bin, &code))
                .unwrap_err()
                .to_string();
            assert!(e.contains(text), "expected {text}: {e}");
        }
    }
}
