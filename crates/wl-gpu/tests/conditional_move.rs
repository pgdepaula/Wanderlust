use wl_gpu::shader_program::{FragmentProgram, Operation, VertexProgram};

const BIN: &[u8] = include_bytes!("fixtures/maxwell/conditional-move.bin");
const VERTEX: &[u8] = include_bytes!("fixtures/maxwell/vertex-conditional-move.bin");
const ALU: u64 = 0x7e6;
const WAIT: u64 = 0xfe6;
const WRITE: u64 = 0x706;
const COMPARE: u64 = 0x7ed;
const EXIT: u64 = 0xe30000000007000f;
const NOP: u64 = 0x50b0000000070f00;

fn mov(dst: u8, bits: u32) -> (u64, u64) {
    (
        0x010000000007f000 | (u64::from(bits) << 20) | u64::from(dst),
        ALU,
    )
}
fn conditional(dst: u8, src: u8, invert: bool) -> (u64, u64) {
    (
        0x5c98078000000000 | (u64::from(src) << 20) | (u64::from(invert) << 19) | u64::from(dst),
        ALU,
    )
}
fn compare(left: u8, right: u8) -> (u64, u64) {
    (
        0x5b62038000070007 | (u64::from(left) << 8) | (u64::from(right) << 20),
        COMPARE,
    )
}
fn initialized() -> Vec<(u64, u64)> {
    (0..8).map(|r| mov(r, 0x3f800000)).collect()
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
fn independently_assembled_p0_and_inverted_moves_link_both_stages() {
    let p = FragmentProgram::decode(BIN).unwrap();
    assert_eq!(p.required_registers(), 8);
    assert_eq!(
        p.operations()[9],
        Operation::ConditionalMove {
            dst: 0,
            src: 6,
            invert: false,
        }
    );
    assert_eq!(
        p.operations()[10],
        Operation::ConditionalMove {
            dst: 2,
            src: 7,
            invert: true,
        }
    );
    assert_eq!(
        u64::from_le_bytes(BIN[0xb8..0xc0].try_into().unwrap()),
        0x5c98078000600000
    );
    assert_eq!(
        u64::from_le_bytes(BIN[0xc0..0xc8].try_into().unwrap()),
        0x5c98078000780002
    );
    let source = p.wgsl();
    assert!(source.contains("if p0 { r[0]=r[6]; }"));
    assert!(source.contains("if !p0 { r[2]=r[7]; }"));
    validate(&source);
    let v = VertexProgram::decode(VERTEX).unwrap();
    assert_eq!(v.required_registers(), 8);
    assert!(v.operations().contains(&Operation::ConditionalMove {
        dst: 0,
        src: 7,
        invert: false,
    }));
    validate(&v.wgsl());
    assert!(FragmentProgram::decode(VERTEX).is_err());
    assert!(VertexProgram::decode(BIN).is_err());
}

#[test]
fn conditional_move_requires_both_possible_values_even_with_a_false_compare() {
    for invert in [false, true] {
        for (dst, src) in [(8, 4), (0, 8)] {
            let mut code = initialized();
            code.extend([
                compare(255, 255),
                conditional(dst, src, invert),
                (EXIT, ALU),
            ]);
            error(&program(BIN, &code), "unready conditional MOV operand");
        }
        for (dst, src) in [(0, 255), (0, 0), (7, 4)] {
            let mut code = initialized();
            code.extend([
                compare(255, 255),
                conditional(dst, src, invert),
                (EXIT, ALU),
            ]);
            let p = FragmentProgram::decode(&program(BIN, &code)).unwrap();
            assert_eq!(p.required_registers(), 8);
            assert!(
                p.operations()
                    .contains(&Operation::ConditionalMove { dst, src, invert })
            );
            validate(&p.wgsl());
        }
    }
}

#[test]
fn conditional_move_rejects_partial_lanes_other_predicates_and_rz_destination() {
    for invert in [false, true] {
        for lanes in 0u64..15 {
            let mut code = initialized();
            let mut instruction = conditional(0, 4, invert);
            instruction.0 = (instruction.0 & !(15 << 39)) | (lanes << 39);
            code.extend([compare(255, 255), instruction, (EXIT, ALU)]);
            error(&program(BIN, &code), "unsupported instruction");
        }
        for predicate in 1u64..7 {
            let mut code = initialized();
            let mut instruction = conditional(0, 4, invert);
            instruction.0 |= predicate << 16;
            code.extend([compare(255, 255), instruction, (EXIT, ALU)]);
            error(&program(BIN, &code), "unsupported instruction");
        }
        let mut code = initialized();
        code.extend([compare(255, 255), conditional(255, 4, invert), (EXIT, ALU)]);
        error(&program(BIN, &code), "RZ destination");
    }
}

#[test]
fn pt_remains_unconditional_and_inverted_pt_remains_unsupported() {
    let mut code = initialized();
    let mut instruction = conditional(8, 4, false);
    instruction.0 |= 7 << 16;
    code.extend([instruction, (EXIT, ALU)]);
    let p = FragmentProgram::decode(&program(BIN, &code)).unwrap();
    assert_eq!(p.required_registers(), 9);
    assert!(p.operations().contains(&Operation::Move { dst: 8, src: 4 }));
    validate(&p.wgsl());
    code[8].0 |= 1 << 19;
    error(&program(BIN, &code), "unsupported instruction");
}

#[test]
fn missing_predicate_and_noncanonical_controls_cannot_initialize_a_move() {
    let mut code = initialized();
    code.extend([conditional(0, 4, false), (EXIT, ALU)]);
    error(
        &program(BIN, &code),
        "uninitialized conditional MOV predicate P0",
    );
    for schedule in [WRITE, ALU - 6 + 13, ALU & !(7 << 8), ALU | (1 << 17)] {
        let mut code = initialized();
        code.extend([
            compare(255, 255),
            (conditional(0, 4, false).0, schedule),
            (EXIT, ALU),
        ]);
        assert!(
            FragmentProgram::decode(&program(BIN, &code)).is_err(),
            "schedule {schedule:x}"
        );
    }
}

#[test]
fn pending_old_destination_and_source_require_real_waits() {
    for invert in [false, true] {
        for (dst, src) in [(0, 6), (6, 4), (6, 255)] {
            let mut code = initialized();
            code.extend([
                compare(255, 255),
                (0xef9400000107ff06, WRITE),
                conditional(dst, src, invert),
                (EXIT, WAIT),
            ]);
            error(&program(BIN, &code), "unready conditional MOV operand");
            code[10].1 = WAIT;
            validate(
                &FragmentProgram::decode(&program(BIN, &code))
                    .unwrap()
                    .wgsl(),
            );
        }
    }
}

#[test]
fn conditional_move_retains_float_hazards_from_old_source_and_predicate_paths() {
    for invert in [false, true] {
        // R0 is the old value; R6 is the new candidate. Only the final case
        // compares an FMUL-derived R4 while both candidate values are untainted.
        for multiply_dst in [0u8, 6, 4] {
            let mut code = initialized();
            code.push((0x5c68000000570400 | u64::from(multiply_dst), ALU));
            code.push(if multiply_dst == 4 {
                compare(4, 5)
            } else {
                compare(255, 255)
            });
            code.push(conditional(0, 6, invert));
            code.extend([(0x5c5800000ff70002, ALU), (EXIT, ALU)]);
            error(&program(BIN, &code), "unfused rounding");
            // A real unguarded overwrite discards every possible conditional
            // value; it must allow later arithmetic even with a tainted P0.
            code.insert(11, mov(0, 0));
            validate(
                &FragmentProgram::decode(&program(BIN, &code))
                    .unwrap()
                    .wgsl(),
            );
        }
    }
}

fn inject_same_register_move(bin: &[u8], at: usize, register: u8, invert: bool) -> Vec<u8> {
    let mut code = items(bin);
    let mut predicate = compare(255, 255);
    predicate.1 |= 1 << 11; // Wait for the existing resource/attribute producer.
    code.insert(at, predicate);
    code.insert(at + 1, conditional(register, register, invert));
    program(bin, &code)
}

#[test]
fn even_conditional_identity_moves_erase_resource_and_position_proofs() {
    for invert in [false, true] {
        error(
            &inject_same_register_move(include_bytes!("fixtures/maxwell/tex.bin"), 3, 6, invert),
            "TEX handle",
        );
        for (at, message) in [(2, "PositionW"), (3, "reciprocal")] {
            error(
                &inject_same_register_move(
                    include_bytes!("fixtures/maxwell/perspective.bin"),
                    at,
                    7,
                    invert,
                ),
                message,
            );
        }
    }
}
