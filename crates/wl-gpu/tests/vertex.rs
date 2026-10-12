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
#[test]
fn b197_vertex_stream_fields_and_bounds() {
    use wl_gpu::{
        fifo::{Decoder, MethodWrite},
        maxwell::*,
        memory::*,
    };
    let mut e = Maxwell3d::default();
    let mut f = Decoder::default();
    for w in [
        0x20010000, CLASS, 0x20020582, 0, 0x50000, 0x20020810, 0x11, 0x200, 0x20020813, 8, 0,
        0x20040700, 0x1018, 0, 0x70000, 1, 0x200207c0, 0, 0x70047, 0x20010620, 0, 0x20020458,
        0x38200000, 0x38800800,
    ] {
        if let Some(m) = f.push_word(w).unwrap() {
            e.apply(m).unwrap();
        }
    }
    f.finish().unwrap();
    assert_eq!(e.vertex_program().unwrap().address, GpuAddress(0x50200));
    let a = e.vertex_array().unwrap();
    assert_eq!(a.offsets, [0, 16]);
    assert_eq!(a.size(0, 3).unwrap(), 72);
    for (first, count) in [(1, 3), (0, 0), (0, 2), (u32::MAX, 3)] {
        assert!(a.size(first, count).is_err());
    }
    for (method, value) in [
        (0x2040, 0x51),
        (0x204c, 0),
        (0x2050, 5),
        (0x1c04, 256),
        (0x1160, 0x38200001),
        (0x1164, 0x38200800),
        (0x1880, 1),
        (0x1c0c, 2),
    ] {
        assert!(
            e.apply(MethodWrite {
                subchannel: 0,
                method,
                value
            })
            .is_err()
        );
    }
}
