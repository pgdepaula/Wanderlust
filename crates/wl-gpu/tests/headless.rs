#![cfg(feature = "wgpu-backend")]
use wl_gpu::memory::{AddressMap, GpuAddress, GuestAddress, SliceMemory};
use wl_gpu::{backend::Renderer, command::*};

#[test]
#[ignore = "requires a headless GPU adapter; run explicitly with --ignored --nocapture"]
fn indexed_draw_readback_and_resource_updates() {
    let mut r = Renderer::new().expect("GPU execution requested: adapter absence is a failure");
    println!("capabilities: {:?}", r.capabilities);
    let (v, i) = test_geometry();
    let memory = SliceMemory {
        base: GuestAddress(0x1000),
        bytes: &v,
    };
    let mut mappings = AddressMap::default();
    mappings
        .map(GpuAddress(0x2000), GuestAddress(0x1000), v.len() as u64)
        .unwrap();
    let initial = [
        Command::CreateBuffer {
            id: BufferId(1),
            size: v.len() as u64,
            kind: BufferKind::Vertex,
        },
        Command::CreateBuffer {
            id: BufferId(2),
            size: i.len() as u64,
            kind: BufferKind::Index,
        },
        Command::Upload {
            id: BufferId(1),
            offset: 0,
            data: mappings.read(&memory, GpuAddress(0x2000), v.len()).unwrap(),
        },
        Command::Upload {
            id: BufferId(2),
            offset: 0,
            data: &i,
        },
        Command::DrawIndexed {
            vertex: BufferId(1),
            index: BufferId(2),
            index_count: 12,
        },
    ];
    let frame = r.execute(66, 19, &initial).unwrap();
    frame.validate_test_pattern().unwrap();
    assert_eq!(r.counters.upload_bytes, 216);
    let before = r.counters;
    let draw = Command::DrawIndexed {
        vertex: BufferId(1),
        index: BufferId(2),
        index_count: 12,
    };
    r.execute(66, 19, std::slice::from_ref(&draw))
        .unwrap()
        .validate_test_pattern()
        .unwrap();
    assert_eq!(r.counters.upload_bytes, before.upload_bytes);
    assert_eq!(r.counters.buffer_allocations, before.buffer_allocations);
    assert_eq!(r.counters.texture_allocations, 1);
    // Multiple draws share one timestamp interval and preserve prior color attachment data.
    r.execute(66, 19, &[draw.clone(), draw.clone()])
        .unwrap()
        .validate_test_pattern()
        .unwrap();
    // A four-byte dirty update changes the red channel of one left vertex to zero.
    let changed = r
        .execute(
            66,
            19,
            &[
                Command::Upload {
                    id: BufferId(1),
                    offset: 8,
                    data: &0_f32.to_le_bytes(),
                },
                draw.clone(),
            ],
        )
        .unwrap();
    assert_ne!(changed.rgba, frame.rgba);
    assert_eq!(r.counters.upload_bytes, before.upload_bytes + 4);
    r.execute(
        66,
        19,
        &[
            Command::Upload {
                id: BufferId(1),
                offset: 8,
                data: &1_f32.to_le_bytes(),
            },
            draw.clone(),
        ],
    )
    .unwrap()
    .validate_test_pattern()
    .unwrap();
    r.execute(
        34,
        13,
        &[
            draw,
            Command::DestroyBuffer { id: BufferId(1) },
            Command::DestroyBuffer { id: BufferId(2) },
        ],
    )
    .unwrap()
    .validate_test_pattern()
    .unwrap();
    assert!(
        r.execute(
            34,
            13,
            &[Command::DrawIndexed {
                vertex: BufferId(1),
                index: BufferId(2),
                index_count: 12
            }]
        )
        .is_err()
    );
    assert!(r.execute(0, 1, &[]).is_err());
    println!(
        "validated 66x19 and 34x13 RGBA8 patterns; counters: {:?}; GPU ns: {:?}",
        r.counters, frame.gpu_nanoseconds
    );
}
