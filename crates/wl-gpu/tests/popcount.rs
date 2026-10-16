use wl_gpu::shader_program::{FragmentProgram, Operation, VertexProgram};
const BIN: &[u8] = include_bytes!("fixtures/maxwell/popcount.bin");
const VERTEX: &[u8] = include_bytes!("fixtures/maxwell/vertex-popcount.bin");
const ALU: u64 = 0x7e6;
const WRITE: u64 = 0x706;
const WAIT_WRITE: u64 = 0xf06;
const READY: u64 = 0xfe6;
const EXIT: u64 = 0xe30000000007000f;
const NOP: u64 = 0x50b0000000070f00;
fn pop(dst: u8, src: u8, invert: bool) -> u64 {
    0x5c08000000070000 | u64::from(dst) | (u64::from(src) << 20) | (u64::from(invert) << 40)
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
fn independently_assembled_variable_latency_popc_in_both_stages() {
    assert_eq!(
        u64::from_le_bytes(BIN[0x68..0x70].try_into().unwrap()),
        pop(4, 4, false)
    );
    let p = FragmentProgram::decode(BIN).unwrap();
    assert!(p.operations().contains(&Operation::CountBits {
        dst: 4,
        src: 4,
        invert: false
    }));
    assert_eq!(p.required_registers(), 7);
    validate(&p.wgsl());
    assert_eq!(
        u64::from_le_bytes(VERTEX[0x78..0x80].try_into().unwrap()),
        pop(6, 0, false)
    );
    let v = VertexProgram::decode(VERTEX).unwrap();
    assert_eq!(v.required_registers(), 9);
    assert!(v.operations().contains(&Operation::CountBits {
        dst: 6,
        src: 0,
        invert: false
    }));
    validate(&v.wgsl());
    assert!(VertexProgram::decode(BIN).is_err());
    assert!(FragmentProgram::decode(VERTEX).is_err());
}
#[test]
fn inversion_aliases_zero_and_high_register_metadata() {
    for src in [0, 1, 2, 3, 4, 16, 32, 64, 128, 254, 255u8] {
        for invert in [false, true] {
            let mut code = initialized();
            if src > 3 && src != 255 {
                code.push(immediate(src));
            }
            code.extend([
                (pop(0, src, invert), WRITE),
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
            assert!(p.wgsl().contains(&format!(
                "r[0]=countOneBits({}r[{src}]);",
                if invert { "~" } else { "" }
            )));
            validate(&p.wgsl());
        }
    }
}
#[test]
fn results_need_an_explicit_wait_even_for_zero_source() {
    for src in [1, 255] {
        let mut code = initialized();
        code.extend([(pop(0, src, false), WRITE), (mov(1, 0), ALU), (EXIT, ALU)]);
        error(&code, "unready MOV source");
        code[5].1 = READY;
        validate(
            &FragmentProgram::decode(&program(BIN, &code))
                .unwrap()
                .wgsl(),
        );
        let mut code = initialized();
        code.extend([(pop(0, src, false), WRITE), (EXIT, ALU)]);
        error(&code, "unready fragment exports");
    }
}
#[test]
fn barrier_reuse_pending_destination_and_uninitialized_source_fail() {
    let mut code = initialized();
    code.extend([(pop(0, 4, false), WRITE), (EXIT, READY)]);
    error(&code, "POPC requires ready source");
    // Pending R1 is independent of the RZ source and destination R2.
    let mut code = initialized();
    code.extend([
        (pop(1, 255, false), WRITE),
        (pop(2, 255, true), WRITE),
        (EXIT, READY),
    ]);
    error(&code, "unused write barrier zero");
    code[5].1 = WAIT_WRITE;
    validate(
        &FragmentProgram::decode(&program(BIN, &code))
            .unwrap()
            .wgsl(),
    );
    // A pending destination cannot be overwritten merely because the new
    // source is ready. Waiting first makes that same alias valid.
    let mut code = initialized();
    code.extend([
        (0xef9400000107ff00, WRITE),
        (pop(0, 255, false), WRITE),
        (EXIT, READY),
    ]);
    error(&code, "unused write barrier zero");
    code[5].1 = WAIT_WRITE;
    validate(
        &FragmentProgram::decode(&program(BIN, &code))
            .unwrap()
            .wgsl(),
    );
    let mut code = initialized();
    code.extend([(pop(0, 1, false), WRITE), immediate(0), (EXIT, READY)]);
    error(&code, "write to pending register");
}
#[test]
fn only_verified_gpr_forms_and_barrier_zero_are_accepted() {
    for bit in [8, 9, 15, 16, 17, 18, 19, 28, 32, 39, 41, 47, 48, 50, 56] {
        let mut code = initialized();
        code.extend([(pop(0, 1, false) ^ (1 << bit), WRITE), (EXIT, READY)]);
        error(&code, "unsupported instruction");
    }
    for word in [0x3808000000170000, 0x4c08000001070000] {
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
        code.extend([(pop(0, 1, false), schedule), (EXIT, READY)]);
        assert!(FragmentProgram::decode(&program(BIN, &code)).is_err());
    }
    let mut code = initialized();
    code.extend([(pop(255, 1, false), WRITE), (EXIT, READY)]);
    error(&code, "RZ destination unsupported");
}
#[test]
fn bit_count_keeps_multiply_hazards_and_cannot_initialize_predicate() {
    for invert in [false, true] {
        let mut code = initialized();
        code.extend([
            (0x5c68000000270101, ALU),
            (pop(0, 1, invert), WRITE),
            (0x5c5800000ff70002, READY),
            (EXIT, ALU),
        ]);
        error(&code, "unfused rounding");
    }
    let mut code = initialized();
    code.extend([
        (pop(0, 1, false), WRITE),
        (0x5ca0000000370100, READY),
        (EXIT, ALU),
    ]);
    error(&code, "uninitialized selector predicate P0");
}
#[test]
fn count_does_not_inherit_texture_or_perspective_resource_proofs() {
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
        code.insert(at, (pop(reg, reg, false), WAIT_WRITE));
        code[at + 1].1 |= 1 << 11;
        let e = FragmentProgram::decode(&program(bin, &code))
            .unwrap_err()
            .to_string();
        assert!(e.contains(text), "expected {text}: {e}");
    }
}
