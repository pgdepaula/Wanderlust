use wl_gpu::shader_program::{FragmentProgram, Operation, VertexProgram};
const BIN: &[u8] = include_bytes!("fixtures/maxwell/multiply-add16.bin");
const SIGNED: &[u8] = include_bytes!("fixtures/maxwell/multiply-add16-signed.bin");
const VERTEX: &[u8] = include_bytes!("fixtures/maxwell/vertex-multiply-add16.bin");
const ALU: u64 = 0x7e6;
const EXIT: u64 = 0xe30000000007000f;
const NOP: u64 = 0x50b0000000070f00;
fn xmad(dst: u8, left: u8, middle: u8, right: u8, signed: bool) -> (u64, u64) {
    (
        0x5b00000000070000
            | (u64::from(signed) * 0x0003000000000000)
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
fn independent_like_signed_programs_and_vertex_stage_keep_full_addend() {
    for (bin, signed, main, aux) in [
        (BIN, false, 0x5b00030000570404, 0x5b007f800ff7ff04),
        (SIGNED, true, 0x5b03030000570404, 0x5b037f800ff7ff04),
    ] {
        assert_eq!(
            u64::from_le_bytes(bin[0x80..0x88].try_into().unwrap()),
            main
        );
        assert_eq!(u64::from_le_bytes(bin[0x98..0xa0].try_into().unwrap()), aux);
        let p = FragmentProgram::decode(bin).unwrap();
        assert!(p.operations().contains(&Operation::MultiplyAdd16 {
            dst: 4,
            left: 4,
            middle: 5,
            right: 6,
            signed
        }));
        assert_eq!(p.required_registers(), 8);
        assert_eq!(p.wgsl().matches("p0=").count(), 1);
        validate(&p.wgsl());
        assert!(VertexProgram::decode(bin).is_err());
    }
    assert_eq!(
        u64::from_le_bytes(VERTEX[0x78..0x80].try_into().unwrap()),
        0x5b037f8000470006
    );
    let v = VertexProgram::decode(VERTEX).unwrap();
    assert!(v.operations().contains(&Operation::MultiplyAdd16 {
        dst: 6,
        left: 0,
        middle: 4,
        right: 255,
        signed: true
    }));
    assert_eq!(v.required_registers(), 9);
    validate(&v.wgsl());
    assert!(FragmentProgram::decode(VERTEX).is_err());
}
#[test]
fn each_source_alias_and_all_zero_register_combinations_validate() {
    for signed in [false, true] {
        for left in [1, 255] {
            for middle in [2, 255] {
                for right in [3, 255] {
                    for dst in 0..4 {
                        let mut code = initialized();
                        code.extend([xmad(dst, left, middle, right, signed), (EXIT, ALU)]);
                        let p = FragmentProgram::decode(&program(BIN, &code)).unwrap();
                        assert_eq!(
                            p.operations()[4],
                            Operation::MultiplyAdd16 {
                                dst,
                                left,
                                middle,
                                right,
                                signed
                            }
                        );
                        assert_eq!(p.required_registers(), 4);
                        validate(&p.wgsl());
                    }
                }
            }
        }
    }
    assert_eq!(xmad(0, 255, 255, 255, false).0, 0x5b007f800ff7ff00);
    assert_eq!(xmad(254, 253, 252, 254, false).0, 0x5b007f000fc7fdfe);
}
#[test]
fn all_three_high_register_fields_and_destination_preserve_metadata() {
    for signed in [false, true] {
        for high in [4, 16, 32, 64, 128, 254, 255] {
            for slot in 0..3 {
                let mut regs = [1, 2, 3];
                regs[slot] = high;
                let mut code = initialized();
                if high != 255 {
                    code.push(immediate(high));
                }
                code.extend([xmad(0, regs[0], regs[1], regs[2], signed), (EXIT, ALU)]);
                let p = FragmentProgram::decode(&program(BIN, &code)).unwrap();
                assert_eq!(
                    p.required_registers(),
                    if high == 255 { 4 } else { u16::from(high) + 1 }
                );
                validate(&p.wgsl());
            }
        }
        let mut code = initialized();
        code.extend([xmad(254, 1, 2, 3, signed), (EXIT, ALU)]);
        let p = FragmentProgram::decode(&program(BIN, &code)).unwrap();
        assert_eq!(p.required_registers(), 255);
        validate(&p.wgsl());
    }
}
#[test]
fn mixed_signs_halfwords_c_modes_and_other_forms_remain_unsupported() {
    for signed in [false, true] {
        for bit in [
            16, 17, 18, 19, 28, 29, 30, 31, 32, 33, 34, 35, 36, 37, 38, 47, 48, 49, 50, 51, 52, 53,
            56,
        ] {
            let mut code = initialized();
            let mut op = xmad(0, 1, 2, 3, signed);
            op.0 ^= 1 << bit;
            code.extend([op, (EXIT, ALU)]);
            error(&code, "unsupported instruction");
        }
        let mut code = initialized();
        code.extend([xmad(255, 1, 2, 3, signed), (EXIT, ALU)]);
        error(&code, "RZ destination unsupported");
    }
    for word in [0x3600018000270100, 0x4e00018000270100, 0x5100018000270100] {
        let mut code = initialized();
        code.extend([(word, ALU), (EXIT, ALU)]);
        error(&code, "unsupported instruction");
    }
}
#[test]
fn three_source_readiness_and_pending_destination_require_fixed_schedule() {
    for signed in [false, true] {
        for slot in 0..3 {
            let mut regs = [1, 2, 3];
            regs[slot] = 4;
            let mut code = initialized();
            code.extend([xmad(0, regs[0], regs[1], regs[2], signed), (EXIT, ALU)]);
            error(&code, "unready XMAD operand");
            regs = [1, 2, 3];
            let mut code = initialized();
            code.extend([
                (0xef9400000107ff00 | u64::from(regs[slot]), 0x706),
                xmad(0, regs[0], regs[1], regs[2], signed),
                (EXIT, ALU),
            ]);
            error(&code, "unready XMAD operand");
            code[5].1 |= 1 << 11;
            validate(
                &FragmentProgram::decode(&program(BIN, &code))
                    .unwrap()
                    .wgsl(),
            );
        }
        let mut code = initialized();
        code.extend([
            (0xef9400000107ff00, 0x706),
            xmad(0, 1, 2, 3, signed),
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
            code.extend([(xmad(0, 1, 2, 3, signed).0, schedule), (EXIT, ALU)]);
            assert!(FragmentProgram::decode(&program(BIN, &code)).is_err());
        }
    }
}
#[test]
fn all_sources_conservatively_preserve_rounding_taint_and_do_not_define_p0() {
    for signed in [false, true] {
        for source in [1u8, 2, 3] {
            let mut code = initialized();
            code.extend([
                (0x5c68000000270100 | u64::from(source), ALU),
                xmad(0, 1, 2, 3, signed),
                (0x5c5800000ff70002, ALU),
                (EXIT, ALU),
            ]);
            error(&code, "unfused rounding");
        }
        let mut code = initialized();
        code.extend([
            xmad(0, 1, 2, 3, signed),
            (0x5ca0000000370100, ALU),
            (EXIT, ALU),
        ]);
        error(&code, "uninitialized selector predicate P0");
    }
}
#[test]
fn every_operand_loses_resource_and_perspective_provenance() {
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
        for signed in [false, true] {
            for slot in 0..3 {
                let mut regs = [255; 3];
                regs[slot] = reg;
                let mut code = items(bin);
                let mut op = xmad(reg, regs[0], regs[1], regs[2], signed);
                op.1 |= 1 << 11;
                code.insert(at, op);
                let e = FragmentProgram::decode(&program(bin, &code))
                    .unwrap_err()
                    .to_string();
                assert!(e.contains(text), "expected {text}: {e}");
            }
        }
    }
}
