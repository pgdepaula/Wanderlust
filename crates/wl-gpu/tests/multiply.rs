use wl_gpu::shader_program::{FragmentProgram, IntegerOperator, Operation, VertexProgram};

const BIN: &[u8] = include_bytes!("fixtures/maxwell/imul.bin");
const BYTES: &[u8] = include_bytes!("fixtures/maxwell/imul-bytes.bin");
const VERTEX: &[u8] = include_bytes!("fixtures/maxwell/vertex-imul.bin");
const EXIT: u64 = 0xe30000000007000f;
const NOP: u64 = 0x50b0000000070f00;
const WRITE: u64 = 0x706;
const WAIT_WRITE: u64 = 0xf06;
const READY: u64 = 0xfe6;
const ALU: u64 = 0x7e6;
// Independently assembled GPR low-product forms: U32/U32, S32/U32,
// U32/S32 and S32/S32. Signedness does not change the low 32 product bits.
const WORDS: [u64; 4] = [
    0x5c38000000570400,
    0x5c38010000570400,
    0x5c38020000570400,
    0x5c38030000570400,
];

fn change(bytes: &mut [u8], offset: usize, f: impl FnOnce(u64) -> u64) {
    let word = u64::from_le_bytes(bytes[offset..offset + 8].try_into().unwrap());
    bytes[offset..offset + 8].copy_from_slice(&f(word).to_le_bytes());
}
fn validate(wgsl: &str) {
    let module = naga::front::wgsl::parse_str(wgsl).unwrap();
    naga::valid::Validator::new(
        naga::valid::ValidationFlags::all(),
        naga::valid::Capabilities::all(),
    )
    .validate(&module)
    .unwrap();
}
fn error(bin: &[u8], text: &str) {
    let e = FragmentProgram::decode(bin).unwrap_err().to_string();
    assert!(e.contains(text), "{e}");
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
fn program(header: &[u8], items: &[(u64, u64)]) -> Vec<u8> {
    let mut out = header[..80].to_vec();
    for group in items.chunks(3) {
        let control = (0..3).fold(0u64, |value, n| {
            value | (group.get(n).map_or(ALU, |item| item.1) << (n * 21))
        });
        out.extend(control.to_le_bytes());
        for n in 0..3 {
            out.extend(group.get(n).map_or(NOP, |item| item.0).to_le_bytes());
        }
    }
    out
}
fn imul(dst: u8, left: u8, right: u8) -> u64 {
    0x5c38000000070000 | u64::from(dst) | (u64::from(left) << 8) | (u64::from(right) << 20)
}
fn mov(dst: u8, src: u8) -> u64 {
    0x5c98078000070000 | u64::from(dst) | (u64::from(src) << 20)
}
fn initialized_outputs() -> Vec<(u64, u64)> {
    [0x3f800000u32, 0x3f000000, 0x3e800000, 0x3f800000]
        .into_iter()
        .enumerate()
        .map(|(dst, bits)| {
            (
                0x010000000007f000 | (u64::from(bits) << 20) | dst as u64,
                ALU,
            )
        })
        .collect()
}

#[test]
fn independently_assembled_low_products_share_raw_u32_lowering_in_both_stages() {
    assert_eq!(
        u64::from_le_bytes(BIN[0x60..0x68].try_into().unwrap()),
        WORDS[0]
    );
    for word in WORDS {
        let mut bin = BIN.to_vec();
        change(&mut bin, 0x60, |_| word);
        let p = FragmentProgram::decode(&bin).unwrap();
        assert_eq!(p.required_registers(), 6);
        assert_eq!(
            p.operations()[2],
            Operation::Integer {
                dst: 0,
                left: 4,
                right: 5,
                operator: IntegerOperator::Multiply,
            }
        );
        assert!(p.wgsl().contains("r[0]=r[4] * r[5];"));
        validate(&p.wgsl());
        assert!(VertexProgram::decode(&bin).is_err());
    }
    let p = FragmentProgram::decode(BYTES).unwrap();
    assert_eq!(p.required_registers(), 13);
    validate(&p.wgsl());
    for signedness in 0u64..4 {
        let mut bin = VERTEX.to_vec();
        change(&mut bin, 0x98, |word| word | (signedness << 40));
        let v = VertexProgram::decode(&bin).unwrap();
        assert_eq!(v.required_registers(), 8);
        assert!(v.operations().contains(&Operation::Integer {
            dst: 0,
            left: 0,
            right: 6,
            operator: IntegerOperator::Multiply,
        }));
        assert!(v.wgsl().contains("r[0]=r[0] * r[6];"));
        validate(&v.wgsl());
        assert!(FragmentProgram::decode(&bin).is_err());
    }
}

#[test]
fn variable_latency_product_requires_producer_barrier_and_consumer_wait() {
    let mut bin = BIN.to_vec();
    change(&mut bin, 0x50, |control| control | (7 << (21 + 5)));
    error(&bin, "unused write barrier zero");
    let mut code = initialized_outputs();
    code.extend([(imul(4, 0, 1), WRITE), (mov(2, 4), ALU), (EXIT, READY)]);
    error(&program(BIN, &code), "unready MOV");
    let mut code = initialized_outputs();
    code.extend([(imul(0, 1, 2), WRITE), (EXIT, ALU)]);
    error(&program(BIN, &code), "unready fragment exports");
    for dst in [0, 4] {
        let mut code = initialized_outputs();
        code.extend([
            (imul(dst, 1, 2), WRITE),
            (mov(dst, 255), ALU),
            (EXIT, READY),
        ]);
        error(&program(BIN, &code), "write to pending register");
    }
    let mut vertex = VERTEX.to_vec();
    change(&mut vertex, 0x90, |control| control & !(1 << (21 + 11)));
    let e = VertexProgram::decode(&vertex).unwrap_err().to_string();
    assert!(e.contains("unready AST"), "{e}");
}

#[test]
fn barrier_zero_cannot_be_reused_even_by_an_independent_product() {
    let mut code = initialized_outputs();
    code.extend([
        (imul(4, 0, 1), WRITE),
        (imul(5, 2, 3), WRITE),
        (EXIT, READY),
    ]);
    error(&program(BIN, &code), "unused write barrier zero");
    code[5].1 = WAIT_WRITE;
    validate(
        &FragmentProgram::decode(&program(BIN, &code))
            .unwrap()
            .wgsl(),
    );
    // A pending CB load blocks barrier reuse even when IMUL reads only RZ.
    let bin = program(
        BIN,
        &[
            (0xef9500000107ff00, WRITE),
            (imul(4, 255, 255), WRITE),
            (EXIT, READY),
        ],
    );
    error(&bin, "unused write barrier zero");
}

#[test]
fn products_validate_sources_rz_and_in_place_destination_reads() {
    for word in WORDS {
        for shift in [8, 20] {
            let mut bin = BIN.to_vec();
            change(&mut bin, 0x60, |_| (word & !(255 << shift)) | (9 << shift));
            error(&bin, "ready operands");
            change(&mut bin, 0x60, |w| w | (255 << shift));
            validate(&FragmentProgram::decode(&bin).unwrap().wgsl());
        }
        let mut bin = BIN.to_vec();
        change(&mut bin, 0x60, |_| word | 255);
        error(&bin, "RZ destination");
        // Waiting is necessary even if the value operands themselves are ready.
        let mut bin = BIN.to_vec();
        change(&mut bin, 0x60, |_| {
            (word & !((255 << 8) | (255 << 20))) | (255 << 8) | (255 << 20)
        });
        change(&mut bin, 0x50, |control| control & !(1 << (21 + 11)));
        error(&bin, "unused write barrier zero");
        for dst in [0, 1, 254] {
            let mut code = initialized_outputs();
            let signedness = word & (3 << 40);
            code.extend([(imul(dst, 0, 1) | signedness, WRITE), (EXIT, READY)]);
            let p = FragmentProgram::decode(&program(BIN, &code)).unwrap();
            assert_eq!(p.required_registers(), if dst == 254 { 255 } else { 4 });
            validate(&p.wgsl());
        }
    }
}

#[test]
fn high_product_cc_predicates_and_alternate_operand_forms_are_rejected() {
    for word in [
        0x5c38008000570400, // Independently assembled U32/U32 HI.
        0x5c38038000570400, // S32/S32 HI.
        0x5c38800000570400, // CC.
        0x5c38000000500400, // Execution P0.
        0x5c380000005f0400, // Execution !PT.
        0x3838000000570400, // Immediate source encoding, unimplemented.
        0x4c38000000570400, // Constant source encoding, unimplemented.
    ] {
        let mut bin = BIN.to_vec();
        change(&mut bin, 0x60, |_| word);
        error(&bin, "unsupported instruction");
    }
}

#[test]
fn multiplication_erases_each_special_resource_origin() {
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
        code.insert(at, (imul(register, register, 255), WAIT_WRITE));
        error(&program(bin, &code), message);
    }
}

#[test]
fn multiplication_propagates_float_hazards_but_real_overwrite_clears_them() {
    for (left, right) in [(1, 255), (255, 1)] {
        for dst in [1, 2] {
            let bin = include_bytes!("fixtures/maxwell/alu.bin");
            let mut code = items(bin);
            code[5] = (imul(dst, left, right), WRITE);
            code[6] = (0x5c5800000ff70003 | (u64::from(dst) << 8), READY);
            error(&program(bin, &code), "unfused rounding");
        }
    }
    let mut code = initialized_outputs();
    code.extend([
        (0x5c68000000170004, ALU),   // FMUL R4,R0,R1.
        (imul(4, 2, 3), WRITE),      // An actual overwrite from untainted values.
        (0x5c5800000ff70400, READY), // FADD R0,R4,RZ.
        (EXIT, ALU),
    ]);
    validate(
        &FragmentProgram::decode(&program(BIN, &code))
            .unwrap()
            .wgsl(),
    );
}
