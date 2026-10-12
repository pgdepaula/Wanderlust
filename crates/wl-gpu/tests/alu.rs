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
