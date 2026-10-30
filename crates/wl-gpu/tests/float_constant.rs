use wl_gpu::shader_program::{
    ConstantRef, FloatOperator as F, FragmentProgram, Operation, VertexProgram,
};
const BIN: &[u8] = include_bytes!("fixtures/maxwell/float-constant.bin");
const ALU: u64 = 0x7e6;
const EXIT: u64 = 0xe30000000007000f;
const NOP: u64 = 0x50b0000000070f00;
fn movc(dst: u8, slot: u8, offset: u32) -> (u64, u64) {
    (
        0x4c98078000070000
            | u64::from(dst)
            | (u64::from(slot) << 34)
            | (u64::from(offset / 4) << 20),
        ALU,
    )
}
fn ldc(dst: u8, slot: u8, offset: u32) -> (u64, u64) {
    (
        0xef9400000007ff00 | u64::from(dst) | (u64::from(slot) << 36) | (u64::from(offset) << 20),
        0x706,
    )
}
fn floatc(dst: u8, src: u8, slot: u8, offset: u32, operator: F) -> (u64, u64) {
    assert!(slot < 32 && offset <= 0x7ffc && offset % 4 == 0);
    let base = match operator {
        F::Add => 0x4c58000000070000,
        F::Multiply => 0x4c68000000070000,
    };
    (
        base | u64::from(dst)
            | (u64::from(src) << 8)
            | (u64::from(slot) << 34)
            | (u64::from(offset / 4) << 20),
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
fn independently_assembled_constant_arithmetic_keeps_guest_references() {
    assert_eq!(
        u64::from_le_bytes(BIN[0x60..0x68].try_into().unwrap()),
        0x4c58000c00570400
    );
    assert_eq!(
        u64::from_le_bytes(BIN[0x68..0x70].try_into().unwrap()),
        0x4c68000c00670401
    );
    for (_, control) in items(BIN) {
        assert_eq!(control, ALU);
    }
    let p = FragmentProgram::decode(BIN).unwrap();
    assert_eq!(p.required_registers(), 5);
    assert_eq!(
        p.constants(),
        &[
            ConstantRef {
                slot: 3,
                offset: 16
            },
            ConstantRef {
                slot: 3,
                offset: 20
            },
            ConstantRef {
                slot: 3,
                offset: 24
            },
            ConstantRef {
                slot: 3,
                offset: 28
            }
        ]
    );
    assert_eq!(
        p.operations()[1],
        Operation::FloatConstant {
            dst: 0,
            src: 4,
            input: 1,
            operator: F::Add
        }
    );
    assert_eq!(
        p.operations()[2],
        Operation::FloatConstant {
            dst: 1,
            src: 4,
            input: 2,
            operator: F::Multiply
        }
    );
    validate(&p.wgsl());
    assert!(VertexProgram::decode(BIN).is_err());
}
#[test]
fn both_operators_cover_slots_offset_boundaries_aliases_rz_and_high_registers() {
    for operator in [F::Add, F::Multiply] {
        for slot in 0..32 {
            for offset in [0, 4, 0x10, 0x7ffc] {
                let mut code = initialized();
                code.extend([floatc(0, 1, slot, offset, operator), (EXIT, ALU)]);
                let p = FragmentProgram::decode(&program(BIN, &code)).unwrap();
                assert_eq!(p.constants(), &[ConstantRef { slot, offset }]);
                assert_eq!(
                    p.operations()[4],
                    Operation::FloatConstant {
                        dst: 0,
                        src: 1,
                        input: 0,
                        operator
                    }
                );
                assert_eq!(p.required_registers(), 4);
                validate(&p.wgsl());
            }
        }
        for src in [0u8, 1, 2, 3, 4, 16, 32, 64, 128, 254, 255] {
            for dst in [0, src.min(254)] {
                let mut code = initialized();
                if src != 255 && src > 3 {
                    code.push(immediate(src));
                }
                code.extend([floatc(dst, src, 31, 0x7ffc, operator), (EXIT, ALU)]);
                let p = FragmentProgram::decode(&program(BIN, &code)).unwrap();
                let expected =
                    4.max(u16::from(dst) + 1)
                        .max(if src == 255 { 0 } else { u16::from(src) + 1 });
                assert_eq!(p.required_registers(), expected);
                validate(&p.wgsl());
            }
        }
    }
    assert_eq!(floatc(254, 255, 31, 0x7ffc, F::Add).0, 0x4c58007dfff7fffe);
    assert_eq!(
        floatc(254, 255, 31, 0x7ffc, F::Multiply).0,
        0x4c68007dfff7fffe
    );
}
#[test]
fn only_unmodified_rn_positive_direct_constant_forms_are_accepted() {
    for operator in [F::Add, F::Multiply] {
        // Bits39..50 are rounding/scales/FTZ/DNZ/ABS/NEG/CC/SAT, not offset payload.
        for bit in [
            16, 17, 18, 19, 33, 39, 40, 41, 42, 43, 44, 45, 46, 47, 48, 49, 50, 56,
        ] {
            let mut code = initialized();
            let mut op = floatc(0, 1, 0, 0, operator);
            op.0 ^= 1 << bit;
            code.extend([op, (EXIT, ALU)]);
            error(&code, "unsupported instruction");
        }
        let mut code = initialized();
        code.extend([floatc(255, 1, 0, 0, operator), (EXIT, ALU)]);
        error(&code, "RZ destination unsupported");
        let vertex = include_bytes!("fixtures/maxwell/vertex.bin");
        let code = [immediate(1), floatc(0, 1, 3, 16, operator), (EXIT, ALU)];
        assert!(
            VertexProgram::decode(&program(vertex, &code))
                .unwrap_err()
                .to_string()
                .contains("unready fragment exports")
        );
    }
}
#[test]
fn ready_source_and_destination_use_fixed_latency_not_a_memory_barrier() {
    for operator in [F::Add, F::Multiply] {
        let mut code = initialized();
        code.extend([floatc(0, 4, 3, 16, operator), (EXIT, ALU)]);
        error(&code, "unready floating ALU");
        let mut code = initialized();
        code.extend([ldc(4, 3, 16), floatc(0, 4, 3, 16, operator), (EXIT, ALU)]);
        error(&code, "unready floating ALU");
        code[5].1 |= 1 << 11;
        validate(
            &FragmentProgram::decode(&program(BIN, &code))
                .unwrap()
                .wgsl(),
        );
        let mut code = initialized();
        code.extend([ldc(0, 3, 16), floatc(0, 1, 3, 16, operator), (EXIT, ALU)]);
        assert!(FragmentProgram::decode(&program(BIN, &code)).is_err());
        code[5].1 |= 1 << 11;
        validate(
            &FragmentProgram::decode(&program(BIN, &code))
                .unwrap()
                .wgsl(),
        );
        let mut code = initialized();
        code.extend([ldc(4, 3, 16), floatc(0, 1, 3, 20, operator), (EXIT, ALU)]);
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
            code.extend([(floatc(0, 1, 0, 0, operator).0, schedule), (EXIT, ALU)]);
            assert!(FragmentProgram::decode(&program(BIN, &code)).is_err());
        }
    }
}
#[test]
fn movc_ldc_and_alu_share_a_bounded_uniform_reference_domain() {
    let mut code = initialized();
    code.push(movc(4, 3, 16));
    code.push(ldc(5, 3, 16));
    let mut add = floatc(0, 1, 3, 16, F::Add);
    add.1 |= 1 << 11;
    code.push(add);
    code.extend([floatc(2, 1, 3, 16, F::Multiply), (EXIT, ALU)]);
    let p = FragmentProgram::decode(&program(BIN, &code)).unwrap();
    assert_eq!(
        p.constants(),
        &[ConstantRef {
            slot: 3,
            offset: 16
        }]
    );
    assert_eq!(
        p.operations()[6],
        Operation::FloatConstant {
            dst: 0,
            src: 1,
            input: 0,
            operator: F::Add
        }
    );
    assert_eq!(
        p.operations()[7],
        Operation::FloatConstant {
            dst: 2,
            src: 1,
            input: 0,
            operator: F::Multiply
        }
    );
    validate(&p.wgsl());
    let mut code = initialized();
    for i in 0..63 {
        code.push(movc(0, 3, i * 4));
    }
    code.extend([
        floatc(0, 1, 3, 252, F::Add),
        floatc(2, 1, 3, 252, F::Multiply),
        (EXIT, ALU),
    ]);
    let p = FragmentProgram::decode(&program(BIN, &code)).unwrap();
    assert_eq!(p.constants().len(), 64);
    validate(&p.wgsl());
    code.pop();
    code.extend([floatc(0, 1, 3, 256, F::Add), (EXIT, ALU)]);
    error(&code, "too many constant references");
}
#[test]
fn constant_products_mark_and_constant_adds_respect_unfused_rounding_hazards() {
    // Every accepted FADD source representation must reject a new constant product.
    for consumer in [
        (0x5c5800000ff70002, ALU),
        (0x0800000000070002 | (0x3f800000u64 << 20), ALU),
        (0x3858000000070002 | (0x3f800u64 << 20), ALU),
        floatc(2, 0, 3, 20, F::Add),
    ] {
        let mut code = initialized();
        code.extend([floatc(0, 1, 3, 16, F::Multiply), consumer, (EXIT, ALU)]);
        error(&code, "unfused rounding");
    }
    // Existing GPR and immediate FMUL origins must also be visible to FADD.C.
    for producer in [
        (0x5c68000000270100, ALU),
        (
            0x1e00000000070000 | (0x3f800000u64 << 20) | (1u64 << 8),
            ALU,
        ),
        (0x3868000000070100 | (0x3f800u64 << 20), ALU),
    ] {
        let mut code = initialized();
        code.extend([producer, floatc(2, 0, 3, 16, F::Add), (EXIT, ALU)]);
        error(&code, "unfused rounding");
    }
    // A later fresh raw constant overwrites old product data and its hazard.
    let mut code = initialized();
    code.extend([
        floatc(0, 1, 3, 16, F::Multiply),
        movc(0, 3, 20),
        floatc(2, 0, 3, 24, F::Add),
        (EXIT, ALU),
    ]);
    validate(
        &FragmentProgram::decode(&program(BIN, &code))
            .unwrap()
            .wgsl(),
    );
}
#[test]
fn arithmetic_neither_initializes_nor_overwrites_p0() {
    for operator in [F::Add, F::Multiply] {
        let mut code = initialized();
        code.extend([
            floatc(0, 1, 3, 16, operator),
            (0x5ca0000000270100, ALU),
            (EXIT, ALU),
        ]);
        error(&code, "uninitialized selector predicate P0");
        code.insert(4, (0x5b64038000270107, 0x7ed));
        let p = FragmentProgram::decode(&program(BIN, &code)).unwrap();
        assert_eq!(p.wgsl().matches("p0=").count(), 1);
        validate(&p.wgsl());
    }
}
#[test]
fn both_arithmetic_operators_erase_handle_and_position_proofs() {
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
        for operator in [F::Add, F::Multiply] {
            let mut code = items(bin);
            let mut op = floatc(reg, reg, 3, 16, operator);
            op.1 |= 1 << 11;
            code.insert(at, op);
            let e = FragmentProgram::decode(&program(bin, &code))
                .unwrap_err()
                .to_string();
            assert!(e.contains(text), "expected {text}: {e}");
        }
    }
}
