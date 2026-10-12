use wl_gpu::shader_program::{FragmentProgram, VertexProgram};
const VERTEX: &[u8] = include_bytes!("fixtures/maxwell/vertex.bin");
#[test]
fn independent_ald_ast_vertex_stages_validate() {
    for b in [VERTEX, include_bytes!("fixtures/maxwell/vertex-swap.bin")] {
        let v = VertexProgram::decode(b).unwrap();
        let m = naga::front::wgsl::parse_str(&v.wgsl()).unwrap();
        naga::valid::Validator::new(
            naga::valid::ValidationFlags::all(),
            naga::valid::Capabilities::empty(),
        )
        .validate(&m)
        .unwrap();
        assert!(FragmentProgram::decode(b).is_err());
    }
    assert_eq!(
        VertexProgram::decode(VERTEX).unwrap().required_registers(),
        6
    );
    assert!(VertexProgram::decode(include_bytes!("fixtures/maxwell/tex.bin")).is_err());
}
#[test]
fn invalid_vertex_header_instruction_and_scoreboard() {
    for n in 0..VERTEX.len() {
        assert!(VertexProgram::decode(&VERTEX[..n]).is_err());
    }
    for (offset, bit) in [
        (0, 1u64),
        (48, 1 << 16),
        (48, 1 << 56),
        (88, 1 << 16),
        (88, 1 << 8),
        (88, 1 << 39),
        (88, 1 << 31),
        (88, 1 << 48),
        (80, 1 << 32),
        (104, 1 << 20),
        (104, 1 << 3),
        (80, 1 << 53),
        (120, 1 << 48),
        (128, 1 << 16),
    ] {
        let mut b = VERTEX.to_vec();
        let w = u64::from_le_bytes(b[offset..offset + 8].try_into().unwrap()) ^ bit;
        b[offset..offset + 8].copy_from_slice(&w.to_le_bytes());
        assert!(
            VertexProgram::decode(&b).is_err(),
            "offset {offset} bit {bit:x}"
        );
    }
    let mut b = VERTEX.to_vec();
    b[104] = 255;
    assert!(VertexProgram::decode(&b).is_err());
}
