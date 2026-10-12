#![cfg(feature = "wgpu-backend")]
use wl_gpu::{
    backend::Renderer, command::Command, maxwell::Maxwell3d, memory::*, shader_program::*,
    texture::TextureChanges,
};
const POS: [[f32; 2]; 3] = [[-0.8, -0.6], [0.6, -0.4], [-0.2, 0.7]];
fn edge(a: [f32; 2], b: [f32; 2], p: [f32; 2]) -> f32 {
    (b[0] - a[0]) * (p[1] - a[1]) - (b[1] - a[1]) * (p[0] - a[0])
}
fn check(pixels: &[u8], positions: [[f32; 2]; 3]) {
    let area = edge(positions[0], positions[1], positions[2]);
    let mut inside = 0;
    let mut outside = 0;
    for y in 0..29 {
        for x in 0..47 {
            let p = [
                (x as f32 + 0.5) * 2. / 47. - 1.,
                1. - (y as f32 + 0.5) * 2. / 29.,
            ];
            let es = [
                edge(positions[1], positions[2], p) / area,
                edge(positions[2], positions[0], p) / area,
                edge(positions[0], positions[1], p) / area,
            ];
            if es.iter().any(|e| e.abs() < 0.005) {
                continue;
            }
            let expected = if es.iter().all(|e| *e > 0.) {
                inside += 1;
                [255, 0, 0, 255]
            } else {
                outside += 1;
                [0, 0, 0, 255]
            };
            assert_eq!(
                &pixels[(y * 47 + x) * 4..(y * 47 + x + 1) * 4],
                &expected,
                "{x},{y}"
            );
        }
    }
    assert!(inside > 200 && outside > 500);
}
#[test]
#[ignore = "requires actual GPU; run explicitly with --ignored --nocapture"]
fn decoded_vertex_program_controls_rasterized_geometry() {
    let mut r = Renderer::new().unwrap();
    println!("adapter={:?}", r.capabilities.adapter);
    r.set_resource_fragment(
        &FragmentProgram::decode(include_bytes!("fixtures/maxwell/red.bin")).unwrap(),
    )
    .unwrap();
    r.sync_fragment_resources(
        &Maxwell3d::default(),
        4,
        &AddressMap::default(),
        &SliceMemory {
            base: GuestAddress(0),
            bytes: &[],
        },
        &TextureChanges::new(1),
    )
    .unwrap();
    let vertices: Vec<u8> = POS
        .iter()
        .flat_map(|p| [p[0], p[1], 0., 1., 0., 0.])
        .flat_map(f32::to_le_bytes)
        .collect();
    for (n, binary) in [
        include_bytes!("fixtures/maxwell/vertex.bin").as_slice(),
        include_bytes!("fixtures/maxwell/vertex-swap.bin").as_slice(),
    ]
    .iter()
    .enumerate()
    {
        let v = VertexProgram::decode(binary).unwrap();
        r.set_vertex_program(&v, 24, [0, 16]).unwrap();
        r.upload_vertices(&vertices).unwrap();
        let pixels = r
            .execute(47, 29, &[Command::DrawVertices { first: 0, count: 3 }])
            .unwrap();
        check(
            &pixels.rgba,
            if n == 0 {
                POS
            } else {
                POS.map(|p| [p[1], p[0]])
            },
        );
        let before = r.counters;
        r.set_vertex_program(&v, 24, [0, 16]).unwrap();
        assert_eq!(before.pipeline_allocations, r.counters.pipeline_allocations);
    }
    assert!(
        r.execute(47, 29, &[Command::DrawVertices { first: 1, count: 3 }])
            .is_err()
    );
    assert!(
        r.set_vertex_program(
            &VertexProgram::decode(include_bytes!("fixtures/maxwell/vertex.bin")).unwrap(),
            20,
            [0, 16]
        )
        .is_err()
    );
    println!("counters={:?}", r.counters);
}
