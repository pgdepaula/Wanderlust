use wl_gpu::shader_program::{FloatOperand, FragmentProgram, Operation, VertexProgram};
const BIN: &[u8] = include_bytes!("fixtures/maxwell/ftz-arithmetic.bin");
const VS: &[u8] = include_bytes!("fixtures/maxwell/vertex-matrix-ftz.bin");
fn validate(source: &str) {
    let m = naga::front::wgsl::parse_str(source).unwrap();
    naga::valid::Validator::new(
        naga::valid::ValidationFlags::all(),
        naga::valid::Capabilities::empty(),
    )
    .validate(&m)
    .unwrap();
}
#[test]
fn independent_fused_arithmetic_and_xyz_stage_programs_decode() {
    let p = FragmentProgram::decode(BIN).unwrap();
    assert!(p.operations().contains(&Operation::FmaFtz {
        dst: 4,
        left: 4,
        right: FloatOperand::Register(5),
        addend: 6
    }));
    validate(&p.wgsl());
    let v = VertexProgram::decode(VS).unwrap();
    assert_eq!(v.constants().len(), 16);
    assert_eq!(v.required_registers(), 14);
    assert!(!v.needs_uv_input());
    validate(&v.wgsl());
    assert!(v.wgsl().contains("wl_fma_ftz"));
}
#[test]
fn fused_instruction_modifiers_predication_and_readiness_are_strict() {
    // Fifth live instruction: FFMA.FTZ register. Fields are independently
    // assembled; arbitrary arithmetic modifiers cannot become operand payload.
    for bit in [13, 14, 16, 19, 47, 48, 49, 50, 51, 54] {
        let mut b = BIN.to_vec();
        let w = u64::from_le_bytes(b[128..136].try_into().unwrap()) ^ (1 << bit);
        b[128..136].copy_from_slice(&w.to_le_bytes());
        assert!(FragmentProgram::decode(&b).is_err(), "modifier {bit}");
    }
    for (byte, value) in [(128, 255), (129, 42)] {
        let mut b = BIN.to_vec();
        b[byte] = value;
        assert!(FragmentProgram::decode(&b).is_err());
    }
}
#[test]
fn xyz_profile_requires_live_barrier_waits_exact_maps_and_complete_exports() {
    let mut b = VS.to_vec();
    // Remove wait of all three ALDs at the first multiply.
    let c = u64::from_le_bytes(b[112..120].try_into().unwrap()) & !(7u64 << 11);
    b[112..120].copy_from_slice(&c.to_le_bytes());
    assert!(
        VertexProgram::decode(&b)
            .unwrap_err()
            .to_string()
            .contains("unready FTZ")
    );
    for byte in [1, 4, 8, 12, 20, 25, 53, 56, 72] {
        let mut b = VS.to_vec();
        b[byte] ^= 0x80;
        assert!(VertexProgram::decode(&b).is_err(), "SPH byte {byte}");
    }
    let mut b = VS.to_vec();
    b[3] &= !2;
    let v = VertexProgram::decode(&b).unwrap();
    assert_ne!(
        v,
        VertexProgram::decode(VS).unwrap(),
        "compiler flag retained in identity"
    );
    let mut b = VS.to_vec();
    // Input W isn't declared by the XYZ map.
    let w = u64::from_le_bytes(b[88..96].try_into().unwrap());
    b[88..96].copy_from_slice(&((w & !(0x3ff << 20)) | (0x8cu64 << 20)).to_le_bytes());
    assert!(VertexProgram::decode(&b).is_err());
}
