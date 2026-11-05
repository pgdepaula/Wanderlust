use wl_gpu::shader_program::{FloatOperand, FloatOperator, FragmentProgram, Operation};

const BIN: &[u8] = include_bytes!("fixtures/maxwell/saturated-constant.bin");

#[test]
fn assembled_saturated_constant_exports_have_independent_operands() {
    let program = FragmentProgram::decode(BIN).unwrap();
    for dst in 0..4 {
        assert_eq!(
            program.operations()[usize::from(dst)],
            Operation::FloatFtz {
                dst,
                left: 255,
                right: FloatOperand::Constant(dst),
                operator: FloatOperator::Add,
                negate_left: true,
                saturate: true,
            }
        );
        assert_eq!(program.constants()[usize::from(dst)].slot, 3);
        assert_eq!(
            program.constants()[usize::from(dst)].offset,
            16 + u32::from(dst) * 4
        );
    }
    let module = naga::front::wgsl::parse_str(&program.wgsl()).unwrap();
    naga::valid::Validator::new(
        naga::valid::ValidationFlags::all(),
        naga::valid::Capabilities::empty(),
    )
    .validate(&module)
    .unwrap();
}

#[test]
fn short_controls_do_not_admit_unready_register_arithmetic_or_barriers() {
    for bit in [8, 16, 19, 45, 46, 47, 49, 51] {
        let mut binary = BIN.to_vec();
        let word = u64::from_le_bytes(binary[88..96].try_into().unwrap()) ^ (1 << bit);
        binary[88..96].copy_from_slice(&word.to_le_bytes());
        assert!(
            FragmentProgram::decode(&binary).is_err(),
            "instruction bit {bit}"
        );
    }
    for bit in [5, 8, 11, 17, 63] {
        let mut binary = BIN.to_vec();
        let control = u64::from_le_bytes(binary[80..88].try_into().unwrap()) ^ (1 << bit);
        binary[80..88].copy_from_slice(&control.to_le_bytes());
        assert!(
            FragmentProgram::decode(&binary).is_err(),
            "control bit {bit}"
        );
    }
}
