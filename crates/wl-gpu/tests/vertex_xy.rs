use wl_gpu::shader_program::{Operation, VertexProgram};
const BIN: &[u8] = include_bytes!("fixtures/maxwell/vertex-xy.bin");
#[test]
fn xy_position_only_interface_retains_constant_addend_fma() {
    let p = VertexProgram::decode(BIN).unwrap();
    assert!(!p.needs_uv_input());
    assert!(!p.exports_uv());
    assert_eq!(p.constants().len(), 4);
    assert!(p.operations().iter().any(|op| matches!(
        op,
        Operation::FmaConstantAddend {
            negate_right: true,
            ..
        }
    )));
    let m = naga::front::wgsl::parse_str(&p.wgsl()).unwrap();
    naga::valid::Validator::new(
        naga::valid::ValidationFlags::all(),
        naga::valid::Capabilities::empty(),
    )
    .validate(&m)
    .unwrap();
}
#[test]
fn xy_vertex_requires_declared_inputs_complete_position_and_ready_loads() {
    for (offset, bit) in [(24, 2), (24, 3), (54, 0), (23, 7), (1, 7)] {
        let mut b = BIN.to_vec();
        b[offset] ^= 1 << bit;
        assert!(VertexProgram::decode(&b).is_err(), "SPH {offset}/{bit}");
    }
    // First FMA must wait for both words of the ALD.
    let mut b = BIN.to_vec();
    let c = u64::from_le_bytes(b[112..120].try_into().unwrap());
    b[112..120].copy_from_slice(&(c & !(1u64 << 11)).to_le_bytes());
    assert!(VertexProgram::decode(&b).is_err());
    for bit in [16, 19, 47, 49, 50] {
        let mut b = BIN.to_vec();
        let w = u64::from_le_bytes(b[120..128].try_into().unwrap()) ^ (1 << bit);
        b[120..128].copy_from_slice(&w.to_le_bytes());
        assert!(VertexProgram::decode(&b).is_err(), "FMA modifier {bit}");
    }
    // A b64 load starting at Y would read undeclared Z.
    let mut b = BIN.to_vec();
    let w = u64::from_le_bytes(b[96..104].try_into().unwrap());
    b[96..104].copy_from_slice(&((w & !(0x3ffu64 << 20)) | (0x84u64 << 20)).to_le_bytes());
    assert!(VertexProgram::decode(&b).is_err());
    // Reducing the position store to b64 must leave EXIT incomplete.
    let mut b = BIN.to_vec();
    let w = u64::from_le_bytes(b[160..168].try_into().unwrap());
    b[160..168].copy_from_slice(&(w & !(2u64 << 47)).to_le_bytes());
    assert!(VertexProgram::decode(&b).is_err());
}
