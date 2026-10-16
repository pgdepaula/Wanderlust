use wl_gpu::shader_program::{FragmentProgram, Operation, VertexProgram};
const BIN: &[u8] = include_bytes!("fixtures/maxwell/ternary.bin");
const VERTEX: &[u8] = include_bytes!("fixtures/maxwell/vertex-ternary.bin");
const ALU: u64 = 0x7e6;
const EXIT: u64 = 0xe30000000007000f;
const NOP: u64 = 0x50b0000000070f00;
fn mov(dst: u8) -> (u64, u64) {
    (0x0103f8000007f000 | u64::from(dst), ALU)
}
fn lop(dst: u8, first: u8, second: u8, third: u8, lut: u8) -> (u64, u64) {
    (
        0x5be7000000070000
            | u64::from(dst)
            | (u64::from(first) << 8)
            | (u64::from(second) << 20)
            | (u64::from(lut) << 28)
            | (u64::from(third) << 39),
        ALU,
    )
}
fn program(header: &[u8], code: &[(u64, u64)]) -> Vec<u8> {
    let mut out = header[..80].to_vec();
    for group in code.chunks(3) {
        let control = (0..3).fold(0u64, |v, n| {
            v | (group.get(n).map_or(ALU, |x| x.1) << (n * 21))
        });
        out.extend(control.to_le_bytes());
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
fn initialized() -> Vec<(u64, u64)> {
    (0..4).map(mov).collect()
}
fn error(code: &[(u64, u64)], message: &str) {
    let e = FragmentProgram::decode(&program(BIN, code))
        .unwrap_err()
        .to_string();
    assert!(e.contains(message), "expected {message}: {e}");
}
fn validate(s: &str) {
    let m = naga::front::wgsl::parse_str(s).unwrap();
    naga::valid::Validator::new(
        naga::valid::ValidationFlags::all(),
        naga::valid::Capabilities::empty(),
    )
    .validate(&m)
    .unwrap();
}
#[test]
fn independent_maxwell_three_source_fixtures_and_stage_headers() {
    let p = FragmentProgram::decode(BIN).unwrap();
    assert_eq!(
        u64::from_le_bytes(BIN[0x88..0x90].try_into().unwrap()),
        0x5be7030e80570404
    );
    assert!(p.operations().contains(&Operation::TernaryLogic {
        dst: 4,
        first: 4,
        second: 5,
        third: 6,
        lut: 0xe8
    }));
    assert_eq!(p.required_registers(), 9);
    validate(&p.wgsl());
    let v = VertexProgram::decode(VERTEX).unwrap();
    assert_eq!(
        u64::from_le_bytes(VERTEX[0x78..0x80].try_into().unwrap()),
        0x5be77f8c00670000
    );
    assert!(v.operations().contains(&Operation::TernaryLogic {
        dst: 0,
        first: 0,
        second: 6,
        third: 255,
        lut: 0xc0
    }));
    assert_eq!(v.required_registers(), 7);
    validate(&v.wgsl());
    assert!(VertexProgram::decode(BIN).is_err());
    assert!(FragmentProgram::decode(VERTEX).is_err());
}
#[test]
fn all_256_tables_and_all_destination_aliases_validate() {
    for lut in 0..=255u8 {
        for dst in [0, 1, 2, 3] {
            let mut code = initialized();
            code.extend([lop(dst, 1, 2, 3, lut), (EXIT, ALU)]);
            let p = FragmentProgram::decode(&program(BIN, &code)).unwrap();
            assert_eq!(
                p.operations()[4],
                Operation::TernaryLogic {
                    dst,
                    first: 1,
                    second: 2,
                    third: 3,
                    lut
                }
            );
            assert_eq!(p.required_registers(), 4);
            validate(&p.wgsl());
        }
    }
    for (first, second, third) in [(255, 2, 3), (1, 255, 3), (1, 2, 255), (255, 255, 255)] {
        let mut code = initialized();
        code.extend([lop(0, first, second, third, 0xe8), (EXIT, ALU)]);
        let p = FragmentProgram::decode(&program(BIN, &code)).unwrap();
        assert_eq!(p.required_registers(), 4);
        validate(&p.wgsl());
    }
}
#[test]
fn third_register_high_bits_are_operand_data_and_count_as_registers() {
    for third in [4, 16, 32, 64, 128, 254] {
        let mut code = initialized();
        code.push(mov(third));
        code.extend([lop(0, 1, 2, third, 0xaa), (EXIT, ALU)]);
        let p = FragmentProgram::decode(&program(BIN, &code)).unwrap();
        assert_eq!(p.required_registers(), u16::from(third) + 1);
        validate(&p.wgsl());
    }
}
#[test]
fn unsupported_output_modes_and_predication_are_not_source_modifiers() {
    for bit in [16, 17, 18, 19, 36, 37, 38, 47, 48, 49, 50] {
        let mut code = initialized();
        let mut op = lop(0, 1, 2, 3, 0xe8);
        op.0 ^= 1 << bit;
        code.extend([op, (EXIT, ALU)]);
        error(&code, "unsupported instruction");
    }
    let mut code = initialized();
    code.extend([lop(255, 1, 2, 3, 0xe8), (EXIT, ALU)]);
    error(&code, "RZ destination unsupported");
    for word in [0x3c00000000170100, 0x0200000000070100] {
        let mut code = initialized();
        code.extend([(word, ALU), (EXIT, ALU)]);
        error(&code, "unsupported instruction");
    }
}
#[test]
fn every_source_remains_ready_even_for_constant_or_identity_tables() {
    for lut in [0, 0xff, 0xf0, 0xcc, 0xaa] {
        for index in 0..3 {
            let mut src = [1, 2, 3];
            src[index] = 4;
            let mut code = initialized();
            code.extend([lop(0, src[0], src[1], src[2], lut), (EXIT, ALU)]);
            error(&code, "unready LOP3 operand");
            src = [1, 2, 3];
            let mut code = initialized();
            code.extend([
                (0xef9400000107ff00 | u64::from(src[index]), 0x706),
                lop(0, src[0], src[1], src[2], lut),
                (EXIT, ALU),
            ]);
            error(&code, "unready LOP3 operand");
            code[5].1 |= 1 << 11;
            validate(
                &FragmentProgram::decode(&program(BIN, &code))
                    .unwrap()
                    .wgsl(),
            );
        }
    }
    let mut code = initialized();
    code.extend([
        (0xef9400000107ff00, 0x706),
        lop(0, 1, 2, 3, 0xe8),
        (EXIT, ALU),
    ]);
    error(&code, "write to pending register");
    for schedule in [0x706, 0x7ed, ALU | (1 << 17)] {
        let mut code = initialized();
        code.extend([(lop(0, 1, 2, 3, 0xe8).0, schedule), (EXIT, ALU)]);
        assert!(FragmentProgram::decode(&program(BIN, &code)).is_err());
    }
}
#[test]
fn discarded_pt_output_cannot_initialize_p0_or_erase_source_multiply_hazards() {
    let mut code = initialized();
    code.extend([
        lop(0, 1, 2, 3, 0xe8),
        (0x5ca0000000370100, ALU),
        (EXIT, ALU),
    ]);
    error(&code, "uninitialized selector predicate P0");
    for source in [1u8, 2, 3] {
        for lut in [0, 0xff, 0xf0, 0xcc, 0xaa, 0xe8] {
            let mut code = initialized();
            code.extend([
                (0x5c68000000270100 | u64::from(source), ALU),
                lop(0, 1, 2, 3, lut),
                (0x5c5800000ff70002, ALU),
                (EXIT, ALU),
            ]);
            error(&code, "unfused rounding");
        }
    }
}
#[test]
fn identity_tables_clear_texture_and_perspective_proofs() {
    for (bin, at, reg, message) in [
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
        let mut op = lop(reg, reg, 255, 255, 0xf0);
        op.1 |= 1 << 11;
        code.insert(at, op);
        let e = FragmentProgram::decode(&program(bin, &code))
            .unwrap_err()
            .to_string();
        assert!(e.contains(message), "expected {message}: {e}");
    }
}
