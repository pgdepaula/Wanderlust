use wl_gpu::shader_program::{FloatOperator, FragmentProgram, Operation};
const BIN: &[u8] = include_bytes!("fixtures/maxwell/alu.bin");
fn word(bytes: &mut [u8], offset: usize, f: impl FnOnce(u64) -> u64) {
    let old = u64::from_le_bytes(bytes[offset..offset + 8].try_into().unwrap());
    bytes[offset..offset + 8].copy_from_slice(&f(old).to_le_bytes());
}
#[test]
fn independent_fadd_fmul_register_fields_and_raw_dataflow() {
    let p = FragmentProgram::decode(BIN).unwrap();
    assert_eq!(p.required_registers(), 7);
    assert_eq!(
        p.operations()[3],
        Operation::Float {
            dst: 0,
            left: 4,
            right: 5,
            operator: FloatOperator::Add
        }
    );
    assert_eq!(
        p.operations()[4],
        Operation::Float {
            dst: 1,
            left: 5,
            right: 6,
            operator: FloatOperator::Multiply
        }
    );
    assert_eq!(
        p.operations()[5],
        Operation::Float {
            dst: 2,
            left: 0,
            right: 255,
            operator: FloatOperator::Add
        }
    );
    assert!(p.wgsl().contains("bitcast<f32>(r[4]) + bitcast<f32>(r[5])"));
    assert!(p.wgsl().contains("bitcast<f32>(r[5]) * bitcast<f32>(r[6])"));
}
#[test]
fn floating_alu_rejects_modifiers_predicates_and_unready_registers() {
    for bit in [16, 19, 39, 44, 45, 46, 47, 48, 49, 50] {
        let mut b = BIN.to_vec();
        word(&mut b, 0x78, |w| w ^ (1 << bit));
        assert!(FragmentProgram::decode(&b).is_err(), "bit{bit}");
    }
    for operand in [8, 20] {
        let mut b = BIN.to_vec();
        word(&mut b, 0x78, |w| (w & !(255 << operand)) | (7 << operand));
        assert!(FragmentProgram::decode(&b).is_err());
    }
    let mut b = BIN.to_vec();
    word(&mut b, 0x70, |w| w & !(1 << 11));
    assert!(FragmentProgram::decode(&b).is_err());
    let mut b = BIN.to_vec();
    word(&mut b, 0x78, |w| (w & !255) | 255);
    assert!(FragmentProgram::decode(&b).is_err());
}
#[test]
fn pending_load_destination_cannot_be_overwritten_without_wait() {
    let mut b = BIN.to_vec();
    word(&mut b, 0x58, |w| w & !255); // initialize output R0 independently
    word(&mut b, 0x70, |w| w & !(1 << 11));
    word(&mut b, 0x78, |_| 0x010000000007f006);
    let error = FragmentProgram::decode(&b).unwrap_err().to_string();
    assert!(error.contains("write to pending register"), "{error}");
    // The identical overwrite with a real wait is accepted; a later multiplication
    // consumes the immediate zero rather than specializing mutable buffer values.
    word(&mut b, 0x70, |w| w | (1 << 11));
    assert!(FragmentProgram::decode(&b).is_ok());
}

#[test]
fn independent_vertex_float_arithmetic_and_stage_identity() {
    let b = include_bytes!("fixtures/maxwell/vertex-alu.bin");
    let p = wl_gpu::shader_program::VertexProgram::decode(b).unwrap();
    assert_eq!(p.required_registers(), 7);
    assert!(p.operations().contains(&Operation::Float {
        dst: 0,
        left: 0,
        right: 6,
        operator: FloatOperator::Multiply
    }));
    assert!(p.operations().contains(&Operation::Float {
        dst: 1,
        left: 1,
        right: 6,
        operator: FloatOperator::Add
    }));
    assert!(FragmentProgram::decode(b).is_err());
}

#[test]
fn contraction_prone_dataflow_is_rejected_until_rounding_control_exists() {
    // Direct FMUL->FADD via R1, and MOV-mediated FMUL->FADD via R2.
    let mut direct = BIN.to_vec();
    word(&mut direct, 0x88, |w| (w & !(255 << 8)) | (1 << 8));
    assert!(
        FragmentProgram::decode(&direct)
            .unwrap_err()
            .to_string()
            .contains("unfused rounding")
    );
    let mut moved = BIN.to_vec();
    word(&mut moved, 0x88, |_| 0x5c98078000170002);
    word(&mut moved, 0x98, |_| 0x5c5800000ff70203);
    assert!(
        FragmentProgram::decode(&moved)
            .unwrap_err()
            .to_string()
            .contains("unfused rounding")
    );
    // Genuine integer-bit overwrite clears provenance before a later FADD.
    let mut overwrite = BIN.to_vec();
    word(&mut overwrite, 0x88, |_| 0x010000000007f001);
    word(&mut overwrite, 0x98, |_| 0x5c5800000ff70102);
    word(&mut overwrite, 0xa0, |_| 0x010000000007f003);
    word(&mut overwrite, 0xa8, |_| 0xe30000000007000f);
    assert!(FragmentProgram::decode(&overwrite).is_ok());
}

const FLOAT_IMM: &[u8] = include_bytes!("fixtures/maxwell/float-immediate.bin");
#[test]
fn short_float_immediate_fields_and_stage_dataflow() {
    let p = FragmentProgram::decode(FLOAT_IMM).unwrap();
    assert_eq!(p.required_registers(), 7);
    for (n, dst, src, bits, operator) in [
        (3, 0, 4, 0x3e000000, FloatOperator::Add),
        (4, 1, 5, 0x3f000000, FloatOperator::Multiply),
        (5, 2, 6, 0xbe800000, FloatOperator::Add),
    ] {
        assert_eq!(
            p.operations()[n],
            Operation::FloatImmediate {
                dst,
                src,
                bits,
                operator
            }
        );
    }
    let v = wl_gpu::shader_program::VertexProgram::decode(include_bytes!(
        "fixtures/maxwell/vertex-float-immediate.bin"
    ))
    .unwrap();
    assert!(v.operations().contains(&Operation::FloatImmediate {
        dst: 0,
        src: 0,
        bits: 0x3f000000,
        operator: FloatOperator::Multiply
    }));
    assert!(v.operations().contains(&Operation::FloatImmediate {
        dst: 1,
        src: 1,
        bits: 0x3f000000,
        operator: FloatOperator::Add
    }));
    // Independent pinned assembler words, including the separate sign bit.
    assert_eq!(
        u64::from_le_bytes(FLOAT_IMM[0x78..0x80].try_into().unwrap()),
        0x3858003e00070400
    );
    assert_eq!(
        u64::from_le_bytes(FLOAT_IMM[0x88..0x90].try_into().unwrap()),
        0x3958003e80070602
    );
}
#[test]
fn short_float_payload_is_ieee_bits_not_signed_integer() {
    for bits in [
        0u32, 0x80000000, 0x00800000, 0x80800000, 0x3f801000, 0xbf801000, 0x7f7ff000, 0xff7ff000,
        0x7f800000, 0x7fc01000, 0x00001000,
    ] {
        let mut bytes = FLOAT_IMM.to_vec();
        word(&mut bytes, 0x78, |w| {
            (w & !((0x7ffff << 20) | (1 << 56)))
                | (u64::from((bits & 0x7fffffff) / 4096) << 20)
                | (u64::from(bits >> 31) << 56)
        });
        let p = FragmentProgram::decode(&bytes).unwrap();
        assert_eq!(
            p.operations()[3],
            Operation::FloatImmediate {
                dst: 0,
                src: 4,
                bits,
                operator: FloatOperator::Add
            }
        );
    }
}
#[test]
fn short_float_modifiers_readiness_and_fusion_hazards_fail_explicitly() {
    for offset in [0x78, 0x80] {
        for bit in [
            16, 19, 39, 40, 41, 42, 43, 44, 45, 46, 47, 48, 49, 50, 51, 52, 53, 54, 55, 57,
        ] {
            let mut bytes = FLOAT_IMM.to_vec();
            word(&mut bytes, offset, |w| w ^ (1 << bit));
            assert!(
                FragmentProgram::decode(&bytes).is_err(),
                "{offset:x} bit{bit}"
            );
        }
        let mut bytes = FLOAT_IMM.to_vec();
        word(&mut bytes, offset, |w| (w & !255) | 255);
        assert!(FragmentProgram::decode(&bytes).is_err());
        let mut bytes = FLOAT_IMM.to_vec();
        word(&mut bytes, offset, |w| (w & !(255 << 8)) | (7 << 8));
        assert!(FragmentProgram::decode(&bytes).is_err());
    }
    // Immediate FMUL followed by immediate FADD must not become implicit FMA.
    let mut bytes = FLOAT_IMM.to_vec();
    word(&mut bytes, 0x88, |w| (w & !(255 << 8)) | (1 << 8));
    assert!(
        FragmentProgram::decode(&bytes)
            .unwrap_err()
            .to_string()
            .contains("unfused")
    );
    // The same hazard is retained when the FADD uses the existing GPR form.
    word(&mut bytes, 0x88, |_| 0x5c5800000ff70102);
    assert!(
        FragmentProgram::decode(&bytes)
            .unwrap_err()
            .to_string()
            .contains("unfused")
    );
    let mut bytes = FLOAT_IMM.to_vec();
    word(&mut bytes, 0x70, |w| w & !(1 << 11));
    assert!(FragmentProgram::decode(&bytes).is_err());
}

#[test]
fn short_float_zero_sources_pending_destinations_and_mixed_hazards() {
    let mut bytes = FLOAT_IMM.to_vec();
    word(&mut bytes, 0x78, |w| (w & !(255 << 8)) | (255 << 8));
    let p = FragmentProgram::decode(&bytes).unwrap();
    assert_eq!(
        p.operations()[3],
        Operation::FloatImmediate {
            dst: 0,
            src: 255,
            bits: 0x3e000000,
            operator: FloatOperator::Add
        }
    );
    assert_eq!(p.required_registers(), 7);
    let mut bytes = FLOAT_IMM.to_vec();
    word(&mut bytes, 0x70, |w| w & !(1 << 11));
    word(&mut bytes, 0x78, |w| (w & !255) | 6);
    assert!(
        FragmentProgram::decode(&bytes)
            .unwrap_err()
            .to_string()
            .contains("write to pending register")
    );
    // Existing GPR FMUL must poison a following immediate FADD too.
    let mut bytes = FLOAT_IMM.to_vec();
    word(&mut bytes, 0x80, |_| 0x5c68000000670501);
    word(&mut bytes, 0x88, |w| (w & !(255 << 8)) | (1 << 8));
    assert!(
        FragmentProgram::decode(&bytes)
            .unwrap_err()
            .to_string()
            .contains("unfused")
    );
    // Valid schedule: LDC/IPA waits, arithmetic on the initialized handle, then TEX.
    // Arithmetic must erase resource provenance even when the same GPR is reused.
    let mut bytes = include_bytes!("fixtures/maxwell/tex.bin").to_vec();
    let control = u64::from_le_bytes(bytes[0x70..0x78].try_into().unwrap());
    let load_control = u64::from_le_bytes(bytes[0x50..0x58].try_into().unwrap());
    let field = (1u64 << 21) - 1;
    let ready = (control >> 21) & field;
    let write_zero = load_control & field;
    let adjusted = ready | (write_zero << 21) | (ready << 42);
    bytes[0x70..0x78].copy_from_slice(&adjusted.to_le_bytes());
    let tex = bytes[0x78..0x80].to_vec();
    let exit = bytes[0x80..0x88].to_vec();
    bytes[0x80..0x88].copy_from_slice(&tex);
    bytes[0x88..0x90].copy_from_slice(&exit);
    for instruction in [0x3858000000070606u64, 0x3868003f80070606] {
        // FADD +0 and FMUL 1 remain arithmetic, not resource-handle proofs.
        bytes[0x78..0x80].copy_from_slice(&instruction.to_le_bytes());
        let error = FragmentProgram::decode(&bytes).unwrap_err().to_string();
        assert!(error.contains("handle"), "{error}");
    }
}

#[test]
fn immediate_multiply_taint_survives_cross_form_identity_operations() {
    // Independently envyas-assembled MOV r1,r0; LOP.OR r1,r0,RZ;
    // IADD32I r1,r0,0. Each can disappear during host optimization.
    for identity in [
        0x5c98078000070001u64,
        0x5c4702000ff70001,
        0x1c00000000070001,
    ] {
        let mut bytes = FLOAT_IMM.to_vec();
        word(&mut bytes, 0x78, |w| {
            (w & 0xff0fffffffffffff) | 0x0060000000000000
        }); // FADD3858 -> FMUL3868
        word(&mut bytes, 0x80, |_| identity);
        word(&mut bytes, 0x88, |w| (w & !(255 << 8)) | (1 << 8));
        let error = FragmentProgram::decode(&bytes).unwrap_err().to_string();
        assert!(error.contains("unfused"), "{identity:016x}: {error}");
    }
}
