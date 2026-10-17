use wl_gpu::shader_program::{FragmentProgram, Operation, PermutationSelector as S, VertexProgram};
const BIN: &[u8] = include_bytes!("fixtures/maxwell/byte-permute.bin");
const IMM: &[u8] = include_bytes!("fixtures/maxwell/byte-permute-immediate.bin");
const VERTEX: &[u8] = include_bytes!("fixtures/maxwell/vertex-byte-permute.bin");
const ALU: u64 = 0x7e6;
const EXIT: u64 = 0xe30000000007000f;
const NOP: u64 = 0x50b0000000070f00;
fn prmt(dst: u8, left: u8, right: u8, selector: S) -> (u64, u64) {
    let (base, selection) = match selector {
        S::Immediate(v) => (0x36c0000000070000, u64::from(v) << 20),
        S::Register(r) => (0x5bc0000000070000, u64::from(r) << 20),
    };
    (
        base | u64::from(dst) | (u64::from(left) << 8) | (u64::from(right) << 39) | selection,
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
fn independent_programs_keep_both_selector_kinds_and_stage_domains() {
    for (bin, pc, word, selector) in [
        (BIN, 0x80, 0x5bc0028000670404, S::Register(6)),
        (IMM, 0x78, 0x36c0028541070404, S::Immediate(0x5410)),
    ] {
        assert_eq!(
            u64::from_le_bytes(bin[pc..pc + 8].try_into().unwrap()),
            word
        );
        let p = FragmentProgram::decode(bin).unwrap();
        assert!(p.operations().contains(&Operation::PermuteBytes {
            dst: 4,
            left: 4,
            right: 5,
            selector
        }));
        assert_eq!(p.required_registers(), 8);
        assert_eq!(p.wgsl().matches("p0=").count(), 1); // auxiliary PRMT preserves P0
        validate(&p.wgsl());
        assert!(VertexProgram::decode(bin).is_err());
    }
    assert_eq!(
        u64::from_le_bytes(VERTEX[0x80..0x88].try_into().unwrap()),
        0x5bc07f8000670006
    );
    let v = VertexProgram::decode(VERTEX).unwrap();
    assert!(v.operations().contains(&Operation::PermuteBytes {
        dst: 6,
        left: 0,
        right: 255,
        selector: S::Register(6)
    }));
    assert_eq!(v.required_registers(), 9);
    validate(&v.wgsl());
    assert!(FragmentProgram::decode(VERTEX).is_err());
}
#[test]
fn selectors_and_every_destination_alias_validate_including_immediate_zero() {
    let mut selectors = vec![
        S::Register(3),
        S::Register(255),
        S::Immediate(0),
        S::Immediate(0xffff),
    ];
    for lane in 0..4 {
        for nibble in 0..16u16 {
            selectors.push(S::Immediate(
                (0x3210 & !(15 << (lane * 4))) | (nibble << (lane * 4)),
            ));
        }
    }
    for selector in selectors {
        for dst in [0, 1, 2, 3] {
            let mut code = initialized();
            code.extend([prmt(dst, 1, 2, selector), (EXIT, ALU)]);
            let p = FragmentProgram::decode(&program(BIN, &code)).unwrap();
            assert_eq!(
                p.operations()[4],
                Operation::PermuteBytes {
                    dst,
                    left: 1,
                    right: 2,
                    selector
                }
            );
            assert_eq!(p.required_registers(), 4);
            validate(&p.wgsl());
        }
    }
    // Original MaxAs emits canonical immediate0; original envyas literal0
    // independently chooses selector RZ. They must remain distinct typed forms.
    assert_eq!(prmt(4, 4, 5, S::Immediate(0)).0, 0x36c0028000070404);
    assert_eq!(prmt(4, 4, 5, S::Register(255)).0, 0x5bc002800ff70404);
}
#[test]
fn high_source_fields_and_rz_metadata_are_not_modes_or_modifiers() {
    for high in [4, 16, 32, 64, 128, 254, 255] {
        for slot in 0..3 {
            let mut regs = [1, 2, 3];
            regs[slot] = high;
            let mut code = initialized();
            if high != 255 {
                code.push(immediate(high));
            }
            code.extend([prmt(0, regs[0], regs[1], S::Register(regs[2])), (EXIT, ALU)]);
            let p = FragmentProgram::decode(&program(BIN, &code)).unwrap();
            assert_eq!(
                p.required_registers(),
                if high == 255 { 4 } else { u16::from(high) + 1 }
            );
            validate(&p.wgsl());
        }
    }
    for selector in [S::Immediate(0xfedc), S::Register(255)] {
        let mut code = initialized();
        code.extend([prmt(0, 255, 255, selector), (EXIT, ALU)]);
        let p = FragmentProgram::decode(&program(BIN, &code)).unwrap();
        assert_eq!(p.required_registers(), 4);
        validate(&p.wgsl());
    }
}
#[test]
fn only_mode_zero_canonical_immediates_and_ordinary_gpr_forms_are_accepted() {
    for selector in [S::Immediate(0x3210), S::Register(3)] {
        for bit in [16, 17, 18, 19, 36, 37, 38, 47, 48, 49, 50, 51, 56] {
            let mut code = initialized();
            let mut op = prmt(0, 1, 2, selector);
            op.0 ^= 1 << bit;
            code.extend([op, (EXIT, ALU)]);
            error(&code, "unsupported instruction");
        }
        let mut code = initialized();
        code.extend([prmt(255, 1, 2, selector), (EXIT, ALU)]);
        error(&code, "RZ destination unsupported");
    }
    for bit in [28, 29, 30, 31, 32, 33, 34, 35] {
        let mut code = initialized();
        let mut op = prmt(0, 1, 2, S::Register(3));
        op.0 ^= 1 << bit;
        code.extend([op, (EXIT, ALU)]);
        error(&code, "unsupported instruction");
    }
    // Constant selector and constant B are separate unsupported instruction forms.
    for word in [0x4bc0028000070100, 0x53c0028000070100] {
        let mut code = initialized();
        code.extend([(word, ALU), (EXIT, ALU)]);
        error(&code, "unsupported instruction");
    }
}
#[test]
fn all_three_register_sources_and_pending_destination_require_readiness() {
    for slot in 0..3 {
        let mut regs = [1, 2, 3];
        regs[slot] = 4;
        let mut code = initialized();
        code.extend([prmt(0, regs[0], regs[1], S::Register(regs[2])), (EXIT, ALU)]);
        error(&code, "unready PRMT operand");
        regs = [1, 2, 3];
        let mut code = initialized();
        code.extend([
            (0xef9400000107ff00 | u64::from(regs[slot]), 0x706),
            prmt(0, regs[0], regs[1], S::Register(regs[2])),
            (EXIT, ALU),
        ]);
        error(&code, "unready PRMT operand");
        code[5].1 |= 1 << 11;
        validate(
            &FragmentProgram::decode(&program(BIN, &code))
                .unwrap()
                .wgsl(),
        );
    }
    // The identity selector still requires its otherwise unused B register.
    let mut code = initialized();
    code.extend([prmt(0, 1, 4, S::Immediate(0x3210)), (EXIT, ALU)]);
    error(&code, "unready PRMT operand");
    let mut code = initialized();
    code.extend([
        (0xef9400000107ff00, 0x706),
        prmt(0, 1, 2, S::Register(3)),
        (EXIT, ALU),
    ]);
    error(&code, "write to pending register");
    for schedule in [
        0x706,
        0x7ed,
        ALU | (1 << 17),
        ALU | (2 << 11),
        ALU ^ (1 << 8),
    ] {
        let mut code = initialized();
        code.extend([(prmt(0, 1, 2, S::Register(3)).0, schedule), (EXIT, ALU)]);
        assert!(FragmentProgram::decode(&program(BIN, &code)).is_err());
    }
}
#[test]
fn all_sources_keep_rounding_taint_and_permute_cannot_initialize_p0() {
    for source in [1u8, 2, 3] {
        let mut code = initialized();
        code.extend([
            (0x5c68000000270100 | u64::from(source), ALU),
            prmt(0, 1, 2, S::Register(3)),
            (0x5c5800000ff70002, ALU),
            (EXIT, ALU),
        ]);
        error(&code, "unfused rounding");
    }
    for source in [1u8, 2] {
        for selector in [0, 0x3210, 0x7654, 0xffff] {
            let mut code = initialized();
            code.extend([
                (0x5c68000000270100 | u64::from(source), ALU),
                prmt(0, 1, 2, S::Immediate(selector)),
                (0x5c5800000ff70002, ALU),
                (EXIT, ALU),
            ]);
            error(&code, "unfused rounding");
        }
    }
    for selector in [S::Register(3), S::Immediate(0x3210)] {
        let mut code = initialized();
        code.extend([
            prmt(0, 1, 2, selector),
            (0x5ca0000000370100, ALU),
            (EXIT, ALU),
        ]);
        error(&code, "uninitialized selector predicate P0");
    }
}
#[test]
fn identity_permutations_and_register_controls_clear_all_resource_proofs() {
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
        for (left, right, selector) in [
            (reg, 255, S::Immediate(0x3210)),
            (255, reg, S::Immediate(0x7654)),
            (255, 255, S::Register(reg)),
        ] {
            let mut code = items(bin);
            let mut op = prmt(reg, left, right, selector);
            op.1 |= 1 << 11;
            code.insert(at, op);
            let e = FragmentProgram::decode(&program(bin, &code))
                .unwrap_err()
                .to_string();
            assert!(e.contains(text), "expected {text}: {e}");
        }
    }
}
