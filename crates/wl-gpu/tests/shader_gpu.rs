#![cfg(feature = "wgpu-backend")]
use wl_gpu::{backend::Renderer, command::*, shader::ConstantFragment};
#[test]
#[ignore = "requires actual GPU; explicitly run --ignored --nocapture"]
fn assembled_maxwell_fragment_compiles_and_executes() {
    let mut r = Renderer::new().expect("GPU execution requested");
    println!("adapter={:?}", r.capabilities);
    let (vertices, indices) = test_geometry();
    // The source geometry deliberately contains red and green vertex colors.
    // Translated fragment output must override those colors over both rectangles.
    r.execute(
        66,
        19,
        &[
            Command::CreateBuffer {
                id: BufferId(1),
                size: vertices.len() as u64,
                kind: BufferKind::Vertex,
            },
            Command::CreateBuffer {
                id: BufferId(2),
                size: indices.len() as u64,
                kind: BufferKind::Index,
            },
            Command::Upload {
                id: BufferId(1),
                offset: 0,
                data: &vertices,
            },
            Command::Upload {
                id: BufferId(2),
                offset: 0,
                data: &indices,
            },
            Command::DrawIndexed {
                vertex: BufferId(1),
                index: BufferId(2),
                index_count: 12,
            },
        ],
    )
    .unwrap()
    .validate_test_pattern()
    .unwrap();
    let draw = Command::DrawIndexed {
        vertex: BufferId(1),
        index: BufferId(2),
        index_count: 12,
    };
    let before = r.counters;
    for (binary, expected) in [
        (
            include_bytes!("fixtures/maxwell/red.bin"),
            [255_u8, 0, 0, 255],
        ),
        (
            include_bytes!("fixtures/maxwell/quarters.bin"),
            [64, 128, 191, 255],
        ),
    ] {
        let fragment = ConstantFragment::decode(binary).unwrap();
        r.set_constant_fragment(fragment).unwrap();
        let pipelines = r.counters.pipeline_allocations;
        for (w, h) in [(66, 19), (34, 13)] {
            let frame = r.execute(w, h, std::slice::from_ref(&draw)).unwrap();
            for (n, pixel) in frame.rgba.chunks_exact(4).enumerate() {
                assert_eq!(pixel, expected, "pixel {n}");
            }
            r.set_constant_fragment(fragment).unwrap();
            assert_eq!(r.counters.pipeline_allocations, pipelines);
        }
        r.restore_default_fragment().unwrap();
        r.execute(66, 19, std::slice::from_ref(&draw))
            .unwrap()
            .validate_test_pattern()
            .unwrap();
        // Restoring the default does not evict the most recent translated pipeline.
        r.set_constant_fragment(fragment).unwrap();
        assert_eq!(r.counters.pipeline_allocations, pipelines);
    }
    assert_eq!(
        r.counters.pipeline_allocations - before.pipeline_allocations,
        2
    );
    assert_eq!(
        r.counters.shader_module_allocations - before.shader_module_allocations,
        2
    );
    assert_eq!(r.counters.shader_cache_hits - before.shader_cache_hits, 6);
    assert_eq!(r.counters.upload_bytes, before.upload_bytes);
    println!("counters={:?}", r.counters);
}
