#![cfg(feature = "wgpu-backend")]
use stats_alloc::{INSTRUMENTED_SYSTEM, Region, StatsAlloc};
use std::{alloc::System, time::Instant};
#[global_allocator]
static ALLOCATOR: &StatsAlloc<System> = &INSTRUMENTED_SYSTEM;
use wl_gpu::{
    backend::Renderer,
    command::Command,
    fifo::{Decoder, MethodWrite},
    maxwell::*,
    memory::*,
    shader_program::FragmentProgram,
    texture::TextureChanges,
};
fn bind(engine: &mut Maxwell3d, address: u32, size: u32) {
    let mut fifo = Decoder::default();
    for word in [
        0x20010000, CLASS, 0x200308e0, size, 0, address, 0x20010924, 1,
    ] {
        if let Some(w) = fifo.push_word(word).unwrap() {
            engine.apply(w).unwrap();
        }
    }
    fifo.finish().unwrap();
}
#[test]
#[ignore = "requires actual GPU; explicitly run --ignored --nocapture"]
fn guest_ldc_changes_pixels_without_shader_recompile() {
    let mut r = Renderer::new().expect("actual GPU requested");
    println!("adapter={:?}", r.capabilities);
    let program = FragmentProgram::decode(include_bytes!("fixtures/maxwell/ldc.bin")).unwrap();
    let mut engine = Maxwell3d::default();
    bind(&mut engine, 0x10000, 32);
    let mut map = AddressMap::default();
    map.map(GpuAddress(0x10000), GuestAddress(0), 64).unwrap();
    map.map(GpuAddress(0x20000), GuestAddress(0), 64).unwrap();
    map.map(GpuAddress(0x30000), GuestAddress(64), 64).unwrap();
    let mut data = [0u8; 128];
    let mut changes = TextureChanges::new(1);
    r.set_resource_fragment(&program).unwrap();
    let modules = r.counters.shader_module_allocations;
    let pipelines = r.counters.pipeline_allocations;
    for (color, expected) in [
        ([1.0f32, 0., 0., 1.], [255, 0, 0, 255]),
        ([0., 0., 1., 1.], [0, 0, 255, 255]),
        ([0.25, 0.5, 0.75, 1.], [64, 128, 191, 255]),
    ] {
        for (n, v) in color.iter().enumerate() {
            data[16 + n * 4..20 + n * 4].copy_from_slice(&v.to_bits().to_le_bytes());
        }
        changes.mark_write(GuestAddress(16), 16).unwrap();
        r.sync_fragment_resources(
            &engine,
            4,
            &map,
            &SliceMemory {
                base: GuestAddress(0),
                bytes: &data,
            },
            &changes,
        )
        .unwrap();
        let frame = r.execute(37, 19, &[Command::DrawFragment]).unwrap();
        for p in frame.rgba.chunks_exact(4) {
            assert_eq!(p, expected);
        }
        r.set_resource_fragment(&program).unwrap();
        assert_eq!(r.counters.shader_module_allocations, modules);
        assert_eq!(r.counters.pipeline_allocations, pipelines);
    }
    let before = r.counters;
    let region = Region::new(ALLOCATOR);
    let start = Instant::now();
    for _ in 0..32 {
        r.sync_fragment_resources(
            &engine,
            4,
            &map,
            &SliceMemory {
                base: GuestAddress(0),
                bytes: &data,
            },
            &changes,
        )
        .unwrap();
    }
    let elapsed = start.elapsed();
    let allocations = region.change();
    assert_eq!(allocations.allocations, 0);
    assert_eq!(allocations.reallocations, 0);
    println!("32 unchanged constant syncs: {elapsed:?}, allocations={allocations:?}");
    assert_eq!(before.constant_guest_bytes, r.counters.constant_guest_bytes);
    assert_eq!(
        before.constant_upload_bytes,
        r.counters.constant_upload_bytes
    );
    bind(&mut engine, 0x20000, 32);
    r.sync_fragment_resources(
        &engine,
        4,
        &map,
        &SliceMemory {
            base: GuestAddress(0),
            bytes: &data,
        },
        &changes,
    )
    .unwrap();
    assert_eq!(before.constant_guest_bytes, r.counters.constant_guest_bytes);
    data[16..20].copy_from_slice(&1f32.to_bits().to_le_bytes());
    changes.mark_write(GuestAddress(16), 4).unwrap();
    let uploaded = r.counters.constant_upload_bytes;
    r.sync_fragment_resources(
        &engine,
        4,
        &map,
        &SliceMemory {
            base: GuestAddress(0),
            bytes: &data,
        },
        &changes,
    )
    .unwrap();
    assert_eq!(r.counters.constant_upload_bytes - uploaded, 4);
    // A different guest backing and zero alpha must replace old values without compilation.
    bind(&mut engine, 0x30000, 32);
    r.sync_fragment_resources(
        &engine,
        4,
        &map,
        &SliceMemory {
            base: GuestAddress(0),
            bytes: &data,
        },
        &changes,
    )
    .unwrap();
    let frame = r.execute(34, 13, &[Command::DrawFragment]).unwrap();
    assert!(frame.rgba.iter().all(|b| *b == 0));
    for (address, size) in [(0x40000, 32), (0x10000, 16)] {
        bind(&mut engine, address, size);
        assert!(
            r.sync_fragment_resources(
                &engine,
                4,
                &map,
                &SliceMemory {
                    base: GuestAddress(0),
                    bytes: &data
                },
                &changes
            )
            .is_err()
        );
        assert!(r.execute(34, 13, &[Command::DrawFragment]).is_err());
    }
    engine
        .apply(MethodWrite {
            subchannel: 0,
            method: 0x2490,
            value: 0,
        })
        .unwrap();
    assert!(
        r.sync_fragment_resources(
            &engine,
            4,
            &map,
            &SliceMemory {
                base: GuestAddress(0),
                bytes: &data
            },
            &changes
        )
        .is_err()
    );
    println!("counters={:?}", r.counters);
}
