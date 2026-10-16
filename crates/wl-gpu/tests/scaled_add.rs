use wl_gpu::shader_program::{FragmentProgram, Operation, VertexProgram};
const BIN: &[u8] = include_bytes!("fixtures/maxwell/scaled-add.bin");
const VERTEX: &[u8] = include_bytes!("fixtures/maxwell/vertex-scaled-add.bin");
const ALU: u64 = 0x7e6;
const LOAD: u64 = 0x706;
const EXIT: u64 = 0xe30000000007000f;
const NOP: u64 = 0x50b0000000070f00;
fn immediate(dst: u8) -> (u64, u64) {
    (0x0103f8000007f000 | u64::from(dst), ALU)
}
fn iscadd(dst: u8, left: u8, right: u8, shift: u8) -> (u64, u64) {
    assert!(shift < 32);
    (
        0x5c18000000070000
            | u64::from(dst)
            | (u64::from(left) << 8)
            | (u64::from(right) << 20)
            | (u64::from(shift) << 39),
        ALU,
    )
}
fn initialized() -> Vec<(u64, u64)> {
    (0..4).map(immediate).collect()
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
fn original_assembler_scaled_add_programs_use_stage_headers_and_preserve_p0() {
    assert_eq!(
        u64::from_le_bytes(BIN[0x78..0x80].try_into().unwrap()),
        0x5c18018000570404
    );
    assert_eq!(
        u64::from_le_bytes(BIN[0x88..0x90].try_into().unwrap()),
        0x5c180f800ff7ff04
    );
    let p = FragmentProgram::decode(BIN).unwrap();
    for op in [
        Operation::ScaledAdd {
            dst: 4,
            left: 4,
            right: 5,
            shift: 3,
        },
        Operation::ScaledAdd {
            dst: 4,
            left: 255,
            right: 255,
            shift: 31,
        },
    ] {
        assert!(p.operations().contains(&op));
    }
    assert_eq!(p.required_registers(), 8);
    assert_eq!(p.wgsl().matches("p0=").count(), 1);
    validate(&p.wgsl());
    assert!(VertexProgram::decode(BIN).is_err());
    assert_eq!(
        u64::from_le_bytes(VERTEX[0x78..0x80].try_into().unwrap()),
        0x5c1800800ff70006
    );
    let v = VertexProgram::decode(VERTEX).unwrap();
    assert!(v.operations().contains(&Operation::ScaledAdd {
        dst: 6,
        left: 0,
        right: 255,
        shift: 1,
    }));
    assert_eq!(v.required_registers(), 9);
    validate(&v.wgsl());
    assert!(FragmentProgram::decode(VERTEX).is_err());
}
#[test]
fn all_32_shifts_and_both_aliases_validate_including_bit43() {
    for shift in 0..32 {
        for dst in [0, 1, 2] {
            let mut code = initialized();
            code.extend([iscadd(dst, 1, 2, shift), (EXIT, ALU)]);
            let p = FragmentProgram::decode(&program(BIN, &code)).unwrap();
            assert_eq!(
                p.operations()[4],
                Operation::ScaledAdd {
                    dst,
                    left: 1,
                    right: 2,
                    shift,
                }
            );
            assert_eq!(p.required_registers(), 4);
            validate(&p.wgsl());
        }
        for (left, right) in [(255, 2), (1, 255), (255, 255), (1, 1)] {
            let mut code = initialized();
            code.extend([iscadd(0, left, right, shift), (EXIT, ALU)]);
            let p = FragmentProgram::decode(&program(BIN, &code)).unwrap();
            assert_eq!(p.required_registers(), 4);
            validate(&p.wgsl());
        }
    }
    // ISCADD bit43 is shift[4], even though IADD uses that bit for .X.
    assert_eq!(iscadd(0, 1, 2, 16).0 ^ iscadd(0, 1, 2, 0).0, 1 << 43);
}
#[test]
fn both_source_register_fields_retain_high_bits_and_exclude_rz_metadata() {
    for high in [4, 16, 32, 64, 128, 254, 255] {
        for (left, right) in [(high, 1), (1, high)] {
            let mut code = initialized();
            if high != 255 {
                code.push(immediate(high));
            }
            code.extend([iscadd(0, left, right, 31), (EXIT, ALU)]);
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
fn modifiers_reserved_fields_other_sources_and_rz_destination_are_rejected() {
    // 44..46 are outside the independently verified five-bit shift field.
    // In particular MaxAs's permissive i39w8 grammar does not establish them.
    for bit in [16, 17, 18, 19, 28, 32, 38, 44, 45, 46, 47, 48, 49, 50, 56] {
        let mut code = initialized();
        let mut op = iscadd(0, 1, 2, 3);
        op.0 ^= 1 << bit;
        code.extend([op, (EXIT, ALU)]);
        error(&code, "unsupported instruction");
    }
    for word in [0x3818018000170100, 0x4c18018000170100, 0x1400000000170100] {
        let mut code = initialized();
        code.extend([(word, ALU), (EXIT, ALU)]);
        error(&code, "unsupported instruction");
    }
    let mut code = initialized();
    code.extend([iscadd(255, 1, 2, 0), (EXIT, ALU)]);
    error(&code, "RZ destination unsupported");
}
#[test]
fn both_sources_and_destination_require_readiness_with_fixed_six_schedule() {
    for shift in [0, 1, 31] {
        for (left, right) in [(4, 2), (1, 4)] {
            let mut code = initialized();
            code.extend([iscadd(0, left, right, shift), (EXIT, ALU)]);
            error(&code, "unready ISCADD operand");
        }
        for pending in [1u8, 2] {
            let mut code = initialized();
            code.extend([
                (0xef9400000107ff00 | u64::from(pending), LOAD),
                iscadd(0, 1, 2, shift),
                (EXIT, ALU),
            ]);
            error(&code, "unready ISCADD operand");
            code[5].1 |= 1 << 11;
            validate(
                &FragmentProgram::decode(&program(BIN, &code))
                    .unwrap()
                    .wgsl(),
            );
        }
    }
    let mut code = initialized();
    code.extend([(0xef9400000107ff00, LOAD), iscadd(0, 1, 2, 0), (EXIT, ALU)]);
    error(&code, "write to pending register");
    for schedule in [
        LOAD,
        0x7ed,
        ALU | (1 << 17),
        ALU | (2 << 11),
        ALU ^ (1 << 8),
    ] {
        let mut code = initialized();
        code.extend([(iscadd(0, 1, 2, 0).0, schedule), (EXIT, ALU)]);
        assert!(FragmentProgram::decode(&program(BIN, &code)).is_err());
    }
}
#[test]
fn either_source_keeps_float_rounding_hazards_and_scaled_add_cannot_create_p0() {
    for source in [1u8, 2] {
        for shift in [0, 1, 31] {
            let mut code = initialized();
            code.extend([
                (0x5c68000000270100 | u64::from(source), ALU),
                iscadd(0, 1, 2, shift),
                (0x5c5800000ff70003, ALU),
                (EXIT, ALU),
            ]);
            error(&code, "unfused rounding");
        }
    }
    let mut code = initialized();
    code.extend([iscadd(0, 1, 2, 0), (0x5ca0000000370100, ALU), (EXIT, ALU)]);
    error(&code, "uninitialized selector predicate P0");
}
#[test]
fn both_identity_sides_erase_texture_and_perspective_proofs() {
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
            let mut op = iscadd(reg, left, right, 0);
            op.1 |= 1 << 11;
            code.insert(at, op);
            let e = FragmentProgram::decode(&program(bin, &code))
                .unwrap_err()
                .to_string();
            assert!(e.contains(text), "expected {text}: {e}");
        }
    }
}
