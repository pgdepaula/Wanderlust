use wl_gpu::shader_program::{FragmentProgram, Operation, VertexProgram};
const BIN: &[u8] = include_bytes!("fixtures/maxwell/integer-add-three.bin");
const VERTEX: &[u8] = include_bytes!("fixtures/maxwell/vertex-integer-add-three.bin");
const ALU: u64 = 0x7e6;
const EXIT: u64 = 0xe30000000007000f;
const NOP: u64 = 0x50b0000000070f00;
fn add3(dst: u8, left: u8, middle: u8, right: u8) -> (u64, u64) {
    (
        0x5cc0000000070000
            | u64::from(dst)
            | (u64::from(left) << 8)
            | (u64::from(middle) << 20)
            | (u64::from(right) << 39),
        ALU,
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
fn independent_programs_use_three_sources_and_preserve_predicates() {
    assert_eq!(
        u64::from_le_bytes(BIN[0x80..0x88].try_into().unwrap()),
        0x5cc0030000570404
    );
    assert_eq!(
        u64::from_le_bytes(BIN[0x98..0xa0].try_into().unwrap()),
        0x5cc07f800ff7ff04
    );
    let p = FragmentProgram::decode(BIN).unwrap();
    assert!(p.operations().contains(&Operation::AddThree {
        dst: 4,
        left: 4,
        middle: 5,
        right: 6
    }));
    assert_eq!(p.required_registers(), 8);
    assert_eq!(p.wgsl().matches("p0=").count(), 1);
    validate(&p.wgsl());
    assert!(VertexProgram::decode(BIN).is_err());
    assert_eq!(
        u64::from_le_bytes(VERTEX[0x80..0x88].try_into().unwrap()),
        0x5cc0038000670006
    );
    let v = VertexProgram::decode(VERTEX).unwrap();
    assert!(v.operations().contains(&Operation::AddThree {
        dst: 6,
        left: 0,
        middle: 6,
        right: 7
    }));
    assert_eq!(v.required_registers(), 9);
    validate(&v.wgsl());
    assert!(FragmentProgram::decode(VERTEX).is_err());
}
#[test]
fn all_source_aliases_and_rz_combinations_are_ordinary_wrapping_additions() {
    for left in [1, 255] {
        for middle in [2, 255] {
            for right in [3, 255] {
                for dst in 0..4 {
                    let mut code = initialized();
                    code.extend([add3(dst, left, middle, right), (EXIT, ALU)]);
                    let p = FragmentProgram::decode(&program(BIN, &code)).unwrap();
                    assert_eq!(
                        p.operations()[4],
                        Operation::AddThree {
                            dst,
                            left,
                            middle,
                            right
                        }
                    );
                    assert_eq!(p.required_registers(), 4);
                    validate(&p.wgsl());
                }
            }
        }
    }
    assert_eq!(add3(0, 255, 255, 255).0, 0x5cc07f800ff7ff00);
    assert_eq!(add3(254, 253, 252, 254).0, 0x5cc07f000fc7fdfe);
}
#[test]
fn high_registers_and_c_bits_43_through_46_are_data() {
    for high in [4, 16, 32, 64, 128, 254, 255] {
        for slot in 0..3 {
            let mut regs = [1, 2, 3];
            regs[slot] = high;
            let mut code = initialized();
            if high != 255 {
                code.push(immediate(high));
            }
            code.extend([add3(0, regs[0], regs[1], regs[2]), (EXIT, ALU)]);
            let p = FragmentProgram::decode(&program(BIN, &code)).unwrap();
            assert_eq!(
                p.required_registers(),
                if high == 255 { 4 } else { u16::from(high) + 1 }
            );
            validate(&p.wgsl());
        }
    }
    let mut code = initialized();
    code.push(immediate(254));
    code.extend([add3(254, 1, 2, 3), (EXIT, ALU)]);
    let p = FragmentProgram::decode(&program(BIN, &code)).unwrap();
    assert_eq!(p.required_registers(), 255);
    validate(&p.wgsl());
}
#[test]
fn halfwords_shifts_modifiers_and_non_gpr_forms_are_rejected() {
    for bit in [
        16, 17, 18, 19, 28, 29, 30, 31, 32, 33, 34, 35, 36, 37, 38, 47, 48, 49, 50, 51, 56,
    ] {
        let mut code = initialized();
        let mut op = add3(0, 1, 2, 3);
        op.0 ^= 1 << bit;
        code.extend([op, (EXIT, ALU)]);
        error(&code, "unsupported instruction");
    }
    for word in [0x4cc0018000270100, 0x38c0018000270100] {
        let mut code = initialized();
        code.extend([(word, ALU), (EXIT, ALU)]);
        error(&code, "unsupported instruction");
    }
    let mut code = initialized();
    code.extend([add3(255, 1, 2, 3), (EXIT, ALU)]);
    error(&code, "RZ destination unsupported");
}
#[test]
fn each_source_pending_destination_and_fixed_schedule_are_checked() {
    for slot in 0..3 {
        let mut regs = [1, 2, 3];
        regs[slot] = 4;
        let mut code = initialized();
        code.extend([add3(0, regs[0], regs[1], regs[2]), (EXIT, ALU)]);
        error(&code, "unready IADD3 operand");
        regs = [1, 2, 3];
        let mut code = initialized();
        code.extend([
            (0xef9400000107ff00 | u64::from(regs[slot]), 0x706),
            add3(0, regs[0], regs[1], regs[2]),
            (EXIT, ALU),
        ]);
        error(&code, "unready IADD3 operand");
        code[5].1 |= 1 << 11;
        validate(
            &FragmentProgram::decode(&program(BIN, &code))
                .unwrap()
                .wgsl(),
        );
    }
    let mut code = initialized();
    code.extend([(0xef9400000107ff00, 0x706), add3(0, 1, 2, 3), (EXIT, ALU)]);
    error(&code, "write to pending register");
    for schedule in [
        0x706,
        0x7ed,
        ALU | (1 << 17),
        ALU | (2 << 11),
        ALU ^ (1 << 8),
    ] {
        let mut code = initialized();
        code.extend([(add3(0, 1, 2, 3).0, schedule), (EXIT, ALU)]);
        assert!(FragmentProgram::decode(&program(BIN, &code)).is_err());
    }
}
#[test]
fn all_three_sources_propagate_rounding_taint_without_initializing_p0() {
    for source in [1u8, 2, 3] {
        let mut code = initialized();
        code.extend([
            (0x5c68000000270100 | u64::from(source), ALU),
            add3(0, 1, 2, 3),
            (0x5c5800000ff70002, ALU),
            (EXIT, ALU),
        ]);
        error(&code, "unfused rounding");
    }
    let mut code = initialized();
    code.extend([add3(0, 1, 2, 3), (0x5ca0000000370100, ALU), (EXIT, ALU)]);
    error(&code, "uninitialized selector predicate P0");
}
#[test]
fn adding_two_zero_registers_still_clears_every_resource_proof() {
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
        for slot in 0..3 {
            let mut regs = [255; 3];
            regs[slot] = reg;
            let mut code = items(bin);
            let mut op = add3(reg, regs[0], regs[1], regs[2]);
            op.1 |= 1 << 11;
            code.insert(at, op);
            let e = FragmentProgram::decode(&program(bin, &code))
                .unwrap_err()
                .to_string();
            assert!(e.contains(text), "expected {text}: {e}");
        }
    }
}
