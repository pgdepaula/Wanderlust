use wl_gpu::shader_program::{ConstantRef, FragmentProgram, Operation, VertexProgram};
const BIN: &[u8] = include_bytes!("fixtures/maxwell/move-constant.bin");
const TEX: &[u8] = include_bytes!("fixtures/maxwell/tex-move-constant.bin");
const ALU: u64 = 0x7e6;
const EXIT: u64 = 0xe30000000007000f;
const NOP: u64 = 0x50b0000000070f00;
fn movc(dst: u8, slot: u8, offset: u32) -> (u64, u64) {
    assert!(slot < 32 && offset <= 0x7ffc && offset % 4 == 0);
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
fn independently_assembled_four_colors_are_ready_without_a_memory_barrier() {
    let expected = [
        0x4c98078c00470000,
        0x4c98078c00570001,
        0x4c98078c00670002,
        0x4c98078c00770003,
    ];
    for (pc, word) in [0x58, 0x60, 0x68, 0x78].into_iter().zip(expected) {
        assert_eq!(
            u64::from_le_bytes(BIN[pc..pc + 8].try_into().unwrap()),
            word
        );
    }
    for (_, schedule) in items(BIN) {
        assert_eq!(schedule, ALU);
    }
    let p = FragmentProgram::decode(BIN).unwrap();
    assert_eq!(p.required_registers(), 4);
    for n in 0..4u8 {
        assert_eq!(
            p.constants()[usize::from(n)],
            ConstantRef {
                slot: 3,
                offset: 16 + 4 * u32::from(n)
            }
        );
        assert_eq!(
            p.operations()[usize::from(n)],
            Operation::Constant { dst: n, input: n }
        );
    }
    validate(&p.wgsl());
    assert!(VertexProgram::decode(BIN).is_err());
}
#[test]
fn all_slots_positive_offset_boundaries_and_full_destination_fields_decode() {
    for slot in 0..32 {
        for offset in [0, 4, 0x10, 0x7ffc] {
            let mut code = initialized();
            code.extend([movc(0, slot, offset), (EXIT, ALU)]);
            let p = FragmentProgram::decode(&program(BIN, &code)).unwrap();
            assert_eq!(p.constants(), &[ConstantRef { slot, offset }]);
            assert_eq!(p.operations()[4], Operation::Constant { dst: 0, input: 0 });
            validate(&p.wgsl());
        }
    }
    for dst in [0u8, 1, 2, 3, 4, 16, 32, 64, 128, 254] {
        let mut code = initialized();
        code.extend([movc(dst, 31, 0x7ffc), (EXIT, ALU)]);
        let p = FragmentProgram::decode(&program(BIN, &code)).unwrap();
        assert_eq!(p.required_registers(), 4.max(u16::from(dst) + 1));
        validate(&p.wgsl());
    }
    assert_eq!(movc(254, 31, 0x7ffc).0, 0x4c9807fdfff700fe);
    // A newly written uninitialized R4 is a legitimate destination.
    let mut code = initialized();
    code.extend([movc(4, 0, 0), (0x5c98078000470000, ALU), (EXIT, ALU)]);
    validate(
        &FragmentProgram::decode(&program(BIN, &code))
            .unwrap()
            .wgsl(),
    );
}
#[test]
fn negative_offsets_partial_quad_lanes_and_predicates_are_rejected() {
    for bit in [
        8, 9, 10, 11, 12, 13, 14, 15, 16, 17, 18, 19, 33, 39, 40, 41, 42, 43, 47, 48, 49, 50, 51,
        56,
    ] {
        let mut code = initialized();
        let mut op = movc(0, 0, 0);
        op.0 ^= 1 << bit;
        code.extend([op, (EXIT, ALU)]);
        error(&code, "unsupported instruction");
    }
    let mut code = initialized();
    code.extend([movc(255, 0, 0), (EXIT, ALU)]);
    error(&code, "RZ destination unsupported");
    for schedule in [
        0x706,
        0x7ed,
        ALU | (1 << 17),
        ALU | (2 << 11),
        ALU ^ (1 << 8),
    ] {
        let mut code = initialized();
        code.extend([(movc(0, 0, 0).0, schedule), (EXIT, ALU)]);
        assert!(FragmentProgram::decode(&program(BIN, &code)).is_err());
    }
    // Valid VTG header still does not authorize vertex-stage CB resources.
    let vertex = include_bytes!("fixtures/maxwell/vertex.bin");
    assert!(
        VertexProgram::decode(&program(vertex, &[movc(0, 0, 0), (EXIT, ALU)]))
            .unwrap_err()
            .to_string()
            .contains("unsupported instruction")
    );
}
#[test]
fn pending_destination_is_checked_but_unrelated_memory_work_is_permitted() {
    let mut code = initialized();
    code.extend([ldc(0, 3, 16), movc(0, 3, 16), (EXIT, ALU)]);
    error(&code, "MOV constant requires ready destination");
    code[5].1 |= 1 << 11;
    let p = FragmentProgram::decode(&program(BIN, &code)).unwrap();
    assert_eq!(
        p.constants(),
        &[ConstantRef {
            slot: 3,
            offset: 16
        }]
    );
    validate(&p.wgsl());
    // MOVC must not acquire an artificial global LDC-style barrier restriction.
    let mut code = initialized();
    code.extend([ldc(4, 3, 16), movc(0, 3, 20), (EXIT, ALU)]);
    let p = FragmentProgram::decode(&program(BIN, &code)).unwrap();
    validate(&p.wgsl());
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
            }
        ]
    );
}
#[test]
fn ldc_and_movc_share_references_and_the_uniform_limit_without_duplicate_slots() {
    let mut code = initialized();
    code.extend([ldc(4, 3, 16)]);
    let mut load = movc(0, 3, 16);
    load.1 |= 1 << 11;
    code.push(load);
    code.extend([movc(1, 3, 20), movc(2, 3, 16), (EXIT, ALU)]);
    let p = FragmentProgram::decode(&program(BIN, &code)).unwrap();
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
            }
        ]
    );
    assert_eq!(p.operations()[4], Operation::Constant { dst: 4, input: 0 });
    assert_eq!(p.operations()[5], Operation::Constant { dst: 0, input: 0 });
    assert_eq!(p.operations()[7], Operation::Constant { dst: 2, input: 0 });
    validate(&p.wgsl());
    let mut code = initialized();
    for i in 0..64 {
        code.push(movc(0, 3, i * 4));
    }
    // A repeated reference remains legal at the capacity boundary.
    code.extend([movc(0, 3, 0), (EXIT, ALU)]);
    let p = FragmentProgram::decode(&program(BIN, &code)).unwrap();
    assert_eq!(p.constants().len(), 64);
    validate(&p.wgsl());
    code.pop();
    code.extend([movc(0, 3, 256), (EXIT, ALU)]);
    error(&code, "too many constant references");
}
#[test]
fn constant_move_preserves_p0_and_replaces_old_multiply_rounding_taint() {
    let mut code = initialized();
    code.extend([
        (0x5b64038000270107, 0x7ed),
        movc(0, 3, 16),
        (0x5ca0000000270100, ALU),
        (EXIT, ALU),
    ]);
    let p = FragmentProgram::decode(&program(BIN, &code)).unwrap();
    assert_eq!(p.wgsl().matches("p0=").count(), 1);
    validate(&p.wgsl());
    let mut code = initialized();
    code.extend([movc(0, 3, 16), (0x5ca0000000270100, ALU), (EXIT, ALU)]);
    error(&code, "uninitialized selector predicate P0");
    let mut code = initialized();
    code.extend([
        (0x5c68000000270100, ALU),
        (0x5c5800000ff70002, ALU),
        (EXIT, ALU),
    ]);
    error(&code, "unfused rounding");
    code.insert(5, movc(0, 3, 16));
    validate(
        &FragmentProgram::decode(&program(BIN, &code))
            .unwrap()
            .wgsl(),
    );
}
#[test]
fn texture_handle_origin_reuses_constant_data_but_other_proofs_are_erased() {
    let p = FragmentProgram::decode(TEX).unwrap();
    assert_eq!(p.constants(), &[ConstantRef { slot: 0, offset: 0 }]);
    assert_eq!(p.texture_input(), Some(0));
    assert_eq!(p.operations()[0], Operation::Constant { dst: 6, input: 0 });
    validate(&p.wgsl());
    // A prior unused MOVC and an equivalent LDC deduplicate the handle's input index.
    let old = include_bytes!("fixtures/maxwell/tex.bin");
    let mut code = items(old);
    code.insert(0, movc(7, 0, 0));
    let p = FragmentProgram::decode(&program(old, &code)).unwrap();
    assert_eq!(p.constants().len(), 1);
    assert_eq!(p.texture_input(), Some(0));
    validate(&p.wgsl());
    // A genuinely fresh CB load can legitimately replace an old handle source.
    let mut code = items(TEX);
    code.insert(1, movc(6, 3, 16));
    let p = FragmentProgram::decode(&program(TEX, &code)).unwrap();
    assert_eq!(p.texture_input(), Some(1));
    validate(&p.wgsl());
    for (at, text) in [(2, "PositionW"), (3, "PositionW reciprocal")] {
        let bin = include_bytes!("fixtures/maxwell/perspective.bin");
        let mut code = items(bin);
        let mut load = movc(7, 3, 16);
        load.1 |= 1 << 11;
        code.insert(at, load);
        let e = FragmentProgram::decode(&program(bin, &code))
            .unwrap_err()
            .to_string();
        assert!(e.contains(text), "expected {text}: {e}");
    }
}
