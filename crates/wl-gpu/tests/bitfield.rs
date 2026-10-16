use wl_gpu::shader_program::{FragmentProgram, Operation, VertexProgram};

const BIN: &[u8] = include_bytes!("fixtures/maxwell/bitfield.bin");
const VERTEX: &[u8] = include_bytes!("fixtures/maxwell/vertex-bitfield.bin");
const ALU: u64 = 0x7e6;
const EXIT: u64 = 0xe30000000007000f;
const NOP: u64 = 0x50b0000000070f00;
fn mov(dst: u8) -> (u64, u64) {
    (0x0103f8000007f000 | u64::from(dst), ALU)
}
fn bfe(dst: u8, src: u8, offset: u8, width: u8, signed: bool) -> (u64, u64) {
    (
        0x3800000000070000
            | u64::from(dst)
            | (u64::from(src) << 8)
            | (u64::from(offset) << 20)
            | (u64::from(width) << 28)
            | (u64::from(signed) << 48),
        ALU,
    )
}
fn program(code: &[(u64, u64)]) -> Vec<u8> {
    with_header(BIN, code)
}
fn with_header(header: &[u8], code: &[(u64, u64)]) -> Vec<u8> {
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
fn initialized() -> Vec<(u64, u64)> {
    (0..4).map(mov).collect()
}
fn error(code: &[(u64, u64)], message: &str) {
    let e = FragmentProgram::decode(&program(code))
        .unwrap_err()
        .to_string();
    assert!(e.contains(message), "expected {message}: {e}");
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
fn independently_assembled_signed_unsigned_and_vertex_fields() {
    let p = FragmentProgram::decode(BIN).unwrap();
    let decoded: Vec<_> = p
        .operations()
        .iter()
        .filter_map(|o| {
            if let Operation::BitfieldExtract {
                dst,
                src,
                offset,
                width,
                signed,
            } = *o
            {
                Some((dst, src, offset, width, signed))
            } else {
                None
            }
        })
        .collect();
    assert_eq!(
        decoded,
        [
            (10, 4, 5, 11, false),
            (10, 4, 5, 11, true),
            (10, 4, 0, 32, false),
            (10, 4, 31, 1, true)
        ]
    );
    assert_eq!(p.required_registers(), 11);
    validate(&p.wgsl());
    let v = VertexProgram::decode(VERTEX).unwrap();
    assert!(v.operations().contains(&Operation::BitfieldExtract {
        dst: 0,
        src: 0,
        offset: 0,
        width: 31,
        signed: false
    }));
    assert_eq!(v.required_registers(), 6);
    validate(&v.wgsl());
    assert!(VertexProgram::decode(BIN).is_err());
    assert!(FragmentProgram::decode(VERTEX).is_err());
}
#[test]
fn every_contained_field_and_zero_register_validates() {
    for offset in 0..32u8 {
        for width in 0..=32 - offset {
            for signed in [false, true] {
                let mut code = initialized();
                code.extend([bfe(0, 1, offset, width, signed), (EXIT, ALU)]);
                let p = FragmentProgram::decode(&program(&code)).unwrap();
                assert_eq!(
                    p.operations()[4],
                    Operation::BitfieldExtract {
                        dst: 0,
                        src: 1,
                        offset,
                        width,
                        signed
                    }
                );
                assert_eq!(p.required_registers(), 4);
                validate(&p.wgsl());
            }
        }
    }
    for src in [0, 255] {
        for (offset, width) in [(0, 32), (31, 0), (31, 1)] {
            let mut code = initialized();
            code.extend([bfe(0, src, offset, width, true), (EXIT, ALU)]);
            validate(&FragmentProgram::decode(&program(&code)).unwrap().wgsl());
        }
    }
}
#[test]
fn unverified_ranges_modifiers_and_control_forms_fail_explicitly() {
    for (offset, width) in [(32, 0), (32, 1), (31, 2), (0, 33), (255, 255)] {
        let mut code = initialized();
        code.extend([bfe(0, 1, offset, width, false), (EXIT, ALU)]);
        error(&code, "unsupported BFE field outside source word");
    }
    for bit in [
        16, 17, 18, 19, 36, 37, 38, 39, 40, 41, 42, 43, 44, 45, 46, 47, 49, 50, 56,
    ] {
        let mut code = initialized();
        let mut op = bfe(0, 1, 5, 11, false);
        op.0 ^= 1 << bit;
        code.extend([op, (EXIT, ALU)]);
        error(&code, "unsupported instruction");
    }
    // GPR control and cbuf control are distinct unimplemented encodings.
    for word in [0x5c00000000270100, 0x4c00000000270100] {
        let mut code = initialized();
        code.extend([(word, ALU), (EXIT, ALU)]);
        error(&code, "unsupported instruction");
    }
}
#[test]
fn readiness_pending_destination_and_fixed_schedule_are_preserved() {
    let mut code = initialized();
    code.extend([bfe(0, 4, 0, 32, false), (EXIT, ALU)]);
    error(&code, "unready BFE source");
    let mut code = initialized();
    code.extend([
        (0xef9400000107ff01, 0x706),
        bfe(0, 1, 0, 32, false),
        (EXIT, ALU),
    ]);
    error(&code, "unready BFE source");
    code[5].1 |= 1 << 11;
    validate(&FragmentProgram::decode(&program(&code)).unwrap().wgsl());
    let mut code = initialized();
    code.extend([
        (0xef9400000107ff00, 0x706),
        bfe(0, 1, 0, 32, false),
        (EXIT, ALU),
    ]);
    error(&code, "write to pending register");
    for schedule in [0x706, 0x7ed, ALU | (1 << 17)] {
        let mut code = initialized();
        code.extend([(bfe(0, 1, 0, 32, false).0, schedule), (EXIT, ALU)]);
        assert!(FragmentProgram::decode(&program(&code)).is_err());
    }
    let mut code = initialized();
    code.extend([bfe(255, 1, 0, 32, false), (EXIT, ALU)]);
    error(&code, "RZ destination unsupported");
}
#[test]
fn extraction_cannot_erase_multiplication_rounding_hazards() {
    for width in [0, 1, 31, 32] {
        let mut code = initialized();
        code.extend([
            (0x5c68000000270101, ALU),
            bfe(0, 1, 0, width, false),
            (0x5c5800000ff70002, ALU),
            (EXIT, ALU),
        ]);
        error(&code, "unfused rounding");
    }
}

#[test]
fn even_full_width_extraction_clears_resource_and_perspective_proofs() {
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
        let mut code: Vec<_> = bin[80..]
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
            .collect();
        let mut op = bfe(reg, reg, 0, 32, false);
        op.1 |= 1 << 11;
        code.insert(at, op);
        let e = FragmentProgram::decode(&with_header(bin, &code))
            .unwrap_err()
            .to_string();
        assert!(e.contains(message), "expected {message}: {e}");
    }
}
