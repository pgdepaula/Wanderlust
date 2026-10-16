use wl_gpu::shader_program::{FragmentProgram, Operation, VertexProgram};
const U8_BIN: &[u8] = include_bytes!("fixtures/maxwell/integer-widen-u8.bin");
const U16_BIN: &[u8] = include_bytes!("fixtures/maxwell/integer-widen-u16.bin");
const S8_BIN: &[u8] = include_bytes!("fixtures/maxwell/integer-widen-s8.bin");
const S16_BIN: &[u8] = include_bytes!("fixtures/maxwell/integer-widen-s16.bin");
const VERTEX: &[u8] = include_bytes!("fixtures/maxwell/vertex-integer-widen.bin");
const ALU: u64 = 0x7e6;
const WRITE: u64 = 0x706;
const WAIT_WRITE: u64 = 0xf06;
const READY: u64 = 0xfe6;
const EXIT: u64 = 0xe30000000007000f;
const NOP: u64 = 0x50b0000000070f00;
fn i2i(dst: u8, src: u8, bits: u8, signed: bool) -> u64 {
    assert!(matches!(bits, 8 | 16));
    0x5ce0000000070200
        | u64::from(dst)
        | (u64::from(src) << 20)
        | if bits == 16 { 0x400 } else { 0 }
        | if signed { 0x3000 } else { 0 }
}
fn mov(dst: u8, src: u8) -> u64 {
    0x5c98078000070000 | u64::from(dst) | (u64::from(src) << 20)
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
fn error(code: &[(u64, u64)], text: &str) {
    let e = FragmentProgram::decode(&program(U8_BIN, code))
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
fn independently_assembled_widening_types_and_vertex_stage_preserve_p0() {
    for (bin, bits, signed, word) in [
        (U8_BIN, 8, false, 0x5ce0000000470204),
        (U16_BIN, 16, false, 0x5ce0000000470604),
        (S8_BIN, 8, true, 0x5ce0000000473204),
        (S16_BIN, 16, true, 0x5ce0000000473604),
    ] {
        assert_eq!(
            u64::from_le_bytes(bin[0x68..0x70].try_into().unwrap()),
            word
        );
        assert_eq!(
            u64::from_le_bytes(bin[0x80..0x88].try_into().unwrap()),
            0x5ce000000ff70204
        );
        let p = FragmentProgram::decode(bin).unwrap();
        assert!(p.operations().contains(&Operation::IntWiden {
            dst: 4,
            src: 4,
            bits,
            signed
        }));
        assert!(p.operations().contains(&Operation::IntWiden {
            dst: 4,
            src: 255,
            bits: 8,
            signed: false
        }));
        assert_eq!(p.required_registers(), 7);
        assert_eq!(p.wgsl().matches("p0=").count(), 1);
        validate(&p.wgsl());
        assert!(VertexProgram::decode(bin).is_err());
    }
    assert_eq!(
        u64::from_le_bytes(VERTEX[0x78..0x80].try_into().unwrap()),
        0x5ce0000000073606
    );
    let v = VertexProgram::decode(VERTEX).unwrap();
    assert!(v.operations().contains(&Operation::IntWiden {
        dst: 6,
        src: 0,
        bits: 16,
        signed: true
    }));
    assert_eq!(v.required_registers(), 9);
    validate(&v.wgsl());
    assert!(FragmentProgram::decode(VERTEX).is_err());
}
#[test]
fn register_aliases_high_source_fields_and_rz_metadata_keep_their_domains() {
    for bits in [8, 16] {
        for signed in [false, true] {
            for src in [0, 1, 4, 16, 32, 64, 128, 254, 255] {
                for dst in [0, if src == 255 { 1 } else { src }] {
                    let mut code = initialized();
                    if src > 3 && src != 255 {
                        code.push(immediate(src));
                    }
                    code.extend([(i2i(dst, src, bits, signed), WRITE), (EXIT, READY)]);
                    let p = FragmentProgram::decode(&program(U8_BIN, &code)).unwrap();
                    assert_eq!(
                        p.required_registers(),
                        if src == 255 {
                            4
                        } else {
                            u16::from(src).max(3) + 1
                        }
                    );
                    assert!(p.operations().contains(&Operation::IntWiden {
                        dst,
                        src,
                        bits,
                        signed
                    }));
                    validate(&p.wgsl());
                }
            }
        }
    }
}
#[test]
fn wrong_width_cross_signedness_selectors_and_modifiers_are_rejected() {
    for word in [
        0x5ce0000000171200, // signed destination, unsigned source
        0x5ce0000000172200, // unsigned destination, signed source
        0x5ce0000000170a00, // U32 source, not a widening byte/halfword
        0x5ce0000000173a00, // S32 source
        0x5ce0000000170000, // U8 destination
        0x5ce0000000173500, // S16 destination
        0x5ce0000000170e00, // 64-bit source
        0x38e0000000170200, // immediate source
        0x4ce0000000170200, // cbuf source
    ] {
        let mut code = initialized();
        code.extend([(word, WRITE), (EXIT, READY)]);
        error(&code, "unsupported instruction");
    }
    for bit in [
        8, 9, 11, 12, 13, 14, 15, 16, 17, 18, 19, 28, 32, 39, 40, 41, 42, 45, 47, 49, 50, 56,
    ] {
        let mut code = initialized();
        code.extend([(i2i(0, 1, 8, false) ^ (1 << bit), WRITE), (EXIT, READY)]);
        error(&code, "unsupported instruction");
    }
    let mut code = initialized();
    code.extend([(i2i(255, 1, 8, false), WRITE), (EXIT, READY)]);
    error(&code, "RZ destination unsupported");
}
#[test]
fn even_zero_widening_is_scoreboarded_and_requires_a_consumer_wait() {
    for bits in [8, 16] {
        for signed in [false, true] {
            let mut code = initialized();
            code.extend([
                (i2i(0, 255, bits, signed), WRITE),
                (mov(1, 0), ALU),
                (EXIT, ALU),
            ]);
            error(&code, "unready MOV source");
            code[5].1 = READY;
            validate(
                &FragmentProgram::decode(&program(U8_BIN, &code))
                    .unwrap()
                    .wgsl(),
            );
            let mut code = initialized();
            code.extend([(i2i(0, 255, bits, signed), WRITE), (EXIT, ALU)]);
            error(&code, "unready fragment exports");
        }
    }
}
#[test]
fn uninitialized_sources_old_pending_destinations_and_barrier_reuse_stay_invalid() {
    let mut code = initialized();
    code.extend([(i2i(0, 4, 8, false), WRITE), (EXIT, READY)]);
    error(&code, "I2I requires ready source");
    for producer in [
        i2i(1, 255, 8, false),
        0x5c0800000ff70001,
        0xef9400000107ff00,
    ] {
        let mut code = initialized();
        code.extend([
            (producer, WRITE),
            (i2i(0, 255, 16, true), WRITE),
            (EXIT, READY),
        ]);
        error(&code, "unused write barrier zero");
        code[5].1 = WAIT_WRITE;
        validate(
            &FragmentProgram::decode(&program(U8_BIN, &code))
                .unwrap()
                .wgsl(),
        );
    }
    let mut code = initialized();
    code.extend([(i2i(0, 1, 8, false), WRITE), immediate(0), (EXIT, READY)]);
    error(&code, "write to pending register");
    for control in [
        ALU,
        0x726,
        0x70d,
        WRITE | (1 << 17),
        WRITE | (2 << 11),
        WRITE ^ (1 << 8),
    ] {
        let mut code = initialized();
        code.extend([(i2i(0, 1, 8, false), control), (EXIT, READY)]);
        assert!(FragmentProgram::decode(&program(U8_BIN, &code)).is_err());
    }
}
#[test]
fn conversion_retains_float_rounding_taint_and_cannot_create_p0() {
    for bits in [8, 16] {
        for signed in [false, true] {
            let mut code = initialized();
            code.extend([
                (0x5c68000000270101, ALU),
                (i2i(0, 1, bits, signed), WRITE),
                (0x5c5800000ff70002, READY),
                (EXIT, ALU),
            ]);
            error(&code, "unfused rounding");
        }
    }
    let mut code = initialized();
    code.extend([
        (i2i(0, 1, 8, false), WRITE),
        (0x5ca0000000370100, READY),
        (EXIT, ALU),
    ]);
    error(&code, "uninitialized selector predicate P0");
}
#[test]
fn widening_cannot_retain_texture_or_perspective_proofs_after_wait() {
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
        let mut code = items(bin);
        code.insert(at, (i2i(reg, reg, 16, false), WAIT_WRITE));
        code[at + 1].1 |= 1 << 11;
        let e = FragmentProgram::decode(&program(bin, &code))
            .unwrap_err()
            .to_string();
        assert!(e.contains(text), "expected {text}: {e}");
    }
}
