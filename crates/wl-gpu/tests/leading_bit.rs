use wl_gpu::shader_program::{FragmentProgram, Operation, VertexProgram};
const BIN: &[u8] = include_bytes!("fixtures/maxwell/leading-bit.bin");
const SIGNED: &[u8] = include_bytes!("fixtures/maxwell/leading-bit-signed.bin");
const VERTEX: &[u8] = include_bytes!("fixtures/maxwell/vertex-leading-bit.bin");
const ALU: u64 = 0x7e6;
const WRITE: u64 = 0x706;
const WAIT_WRITE: u64 = 0xf06;
const READY: u64 = 0xfe6;
const EXIT: u64 = 0xe30000000007000f;
const NOP: u64 = 0x50b0000000070f00;
fn flo(dst: u8, src: u8, signed: bool) -> u64 {
    0x5c30000000070000 | u64::from(dst) | (u64::from(src) << 20) | (u64::from(signed) << 48)
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
fn independently_assembled_nonsh_flo_stages_and_signed_modes() {
    for (bin, signed) in [(BIN, false), (SIGNED, true)] {
        assert_eq!(
            u64::from_le_bytes(bin[0x68..0x70].try_into().unwrap()),
            flo(4, 4, signed)
        );
        let p = FragmentProgram::decode(bin).unwrap();
        assert!(p.operations().contains(&Operation::LeadingBit {
            dst: 4,
            src: 4,
            signed
        }));
        assert_eq!(p.required_registers(), 7);
        validate(&p.wgsl());
        assert!(VertexProgram::decode(bin).is_err());
    }
    assert_eq!(
        u64::from_le_bytes(VERTEX[0x78..0x80].try_into().unwrap()),
        flo(6, 0, true)
    );
    let v = VertexProgram::decode(VERTEX).unwrap();
    assert!(v.operations().contains(&Operation::LeadingBit {
        dst: 6,
        src: 0,
        signed: true
    }));
    assert_eq!(v.required_registers(), 9);
    validate(&v.wgsl());
    assert!(FragmentProgram::decode(VERTEX).is_err());
}
#[test]
fn alias_zero_and_high_register_metadata_in_both_modes() {
    for src in [0, 1, 2, 3, 4, 16, 32, 64, 128, 254, 255u8] {
        for signed in [false, true] {
            let mut code = initialized();
            if src > 3 && src != 255 {
                code.push(immediate(src));
            }
            code.extend([
                (flo(0, src, signed), WRITE),
                (mov(1, 0), READY),
                (EXIT, ALU),
            ]);
            let p = FragmentProgram::decode(&program(BIN, &code)).unwrap();
            assert_eq!(
                p.required_registers(),
                if src == 255 {
                    4
                } else {
                    u16::from(src).max(3) + 1
                }
            );
            let expression = if signed {
                format!("r[0]=bitcast<u32>(firstLeadingBit(bitcast<i32>(r[{src}])));")
            } else {
                format!("r[0]=firstLeadingBit(r[{src}]);")
            };
            assert!(p.wgsl().contains(&expression));
            validate(&p.wgsl());
        }
    }
}
#[test]
fn no_qualifying_bit_still_requires_an_explicit_wait() {
    for signed in [false, true] {
        let mut code = initialized();
        code.extend([(flo(0, 255, signed), WRITE), (mov(1, 0), ALU), (EXIT, ALU)]);
        error(&code, "unready MOV source");
        code[5].1 = READY;
        validate(
            &FragmentProgram::decode(&program(BIN, &code))
                .unwrap()
                .wgsl(),
        );
        let mut code = initialized();
        code.extend([(flo(0, 255, signed), WRITE), (EXIT, ALU)]);
        error(&code, "unready fragment exports");
    }
}
#[test]
fn outstanding_loads_and_other_variable_operations_cannot_reuse_barrier() {
    let mut code = initialized();
    code.extend([(flo(0, 4, false), WRITE), (EXIT, READY)]);
    error(&code, "FLO requires ready source");
    for producer in [flo(1, 255, false), 0x5c0800000ff70001, 0xef9400000107ff00] {
        let mut code = initialized();
        code.extend([(producer, WRITE), (flo(0, 255, true), WRITE), (EXIT, READY)]);
        error(&code, "unused write barrier zero");
        code[5].1 = WAIT_WRITE;
        validate(
            &FragmentProgram::decode(&program(BIN, &code))
                .unwrap()
                .wgsl(),
        );
    }
    let mut code = initialized();
    code.extend([(flo(0, 1, false), WRITE), immediate(0), (EXIT, READY)]);
    error(&code, "write to pending register");
}
#[test]
fn sh_inversion_cc_predication_and_other_source_forms_stay_unsupported() {
    for bit in [
        8, 15, 16, 17, 18, 19, 28, 32, 39, 40, 41, 42, 47, 49, 50, 56,
    ] {
        let mut code = initialized();
        code.extend([(flo(0, 1, false) ^ (1 << bit), WRITE), (EXIT, READY)]);
        error(&code, "unsupported instruction");
    }
    for word in [0x3830000000170000, 0x4c30000001070000] {
        let mut code = initialized();
        code.extend([(word, WRITE), (EXIT, READY)]);
        error(&code, "unsupported instruction");
    }
    for schedule in [
        ALU,
        0x726,
        0x70d,
        WRITE | (1 << 17),
        WRITE | (2 << 11),
        WRITE ^ (1 << 8),
    ] {
        let mut code = initialized();
        code.extend([(flo(0, 1, false), schedule), (EXIT, READY)]);
        assert!(FragmentProgram::decode(&program(BIN, &code)).is_err());
    }
    let mut code = initialized();
    code.extend([(flo(255, 1, false), WRITE), (EXIT, READY)]);
    error(&code, "RZ destination unsupported");
}
#[test]
fn leading_bit_keeps_rounding_hazards_and_cannot_initialize_p0() {
    for signed in [false, true] {
        let mut code = initialized();
        code.extend([
            (0x5c68000000270101, ALU),
            (flo(0, 1, signed), WRITE),
            (0x5c5800000ff70002, READY),
            (EXIT, ALU),
        ]);
        error(&code, "unfused rounding");
    }
    let mut code = initialized();
    code.extend([
        (flo(0, 1, false), WRITE),
        (0x5ca0000000370100, READY),
        (EXIT, ALU),
    ]);
    error(&code, "uninitialized selector predicate P0");
}
#[test]
fn leading_index_does_not_inherit_resource_proofs_after_a_valid_wait() {
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
        code.insert(at, (flo(reg, reg, false), WAIT_WRITE));
        code[at + 1].1 |= 1 << 11;
        let e = FragmentProgram::decode(&program(bin, &code))
            .unwrap_err()
            .to_string();
        assert!(e.contains(text), "expected {text}: {e}");
    }
}
