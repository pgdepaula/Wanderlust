//! Reproducible synthetic texture coherency/copy workload, with real GPU readback.
#[path = "../tests/common/mod.rs"]
mod common;
use stats_alloc::{INSTRUMENTED_SYSTEM, Region, StatsAlloc};
use std::{alloc::System, time::Instant};
use wl_gpu::{
    TextureUploadPolicy, backend::Renderer, command::Command, format::Format, memory::*, texture::*,
};
#[global_allocator]
static GLOBAL: &StatsAlloc<System> = &INSTRUMENTED_SYSTEM;

fn sample(
    r: &mut Renderer,
    d: TextureDesc,
    source: &[u8],
    map: &AddressMap,
    changes: &TextureChanges,
    label: &str,
    repeats: u32,
) {
    let before = r.counters;
    let times = r.texture_timings;
    let start = Instant::now();
    let region = Region::new(GLOBAL);
    for _ in 0..repeats {
        r.sync_texture_with_policy(
            TextureId(1),
            d,
            map,
            &SliceMemory {
                base: GuestAddress(0x80000),
                bytes: source,
            },
            changes,
            TextureUploadPolicy::Software,
        )
        .unwrap();
    }
    let allocations = region.change();
    let sync = start.elapsed();
    let start = Instant::now();
    let mut gpu_ns = Vec::new();
    for _ in 0..repeats {
        let frame = r
            .execute(
                d.width,
                d.height,
                &[Command::DrawTexture {
                    id: TextureId(1),
                    mip: 0,
                }],
            )
            .unwrap();
        if let Some(ns) = frame.gpu_nanoseconds {
            gpu_ns.push(ns);
        }
        assert_eq!(frame.rgba.len(), (d.width * d.height * 4) as usize);
    }
    println!(
        "gpu_samples={} gpu_mean_ns={:?}",
        gpu_ns.len(),
        (!gpu_ns.is_empty()).then(|| gpu_ns.iter().sum::<f64>() / gpu_ns.len() as f64)
    );
    println!(
        "{label}: repeats={repeats} sync={sync:?} draw_readback={:?} upload={} guest={} deswizzle={} decompress={} textures={} uploads={} hits={} image_temps={} rust_allocs={} rust_alloc_bytes={} source_time={:?} untile_time={:?} decode_time={:?} upload_time={:?}",
        start.elapsed(),
        r.counters.upload_bytes - before.upload_bytes,
        r.counters.guest_texture_bytes - before.guest_texture_bytes,
        r.counters.deswizzle_bytes - before.deswizzle_bytes,
        r.counters.decompress_bytes - before.decompress_bytes,
        r.counters.texture_allocations - before.texture_allocations,
        r.counters.texture_uploads - before.texture_uploads,
        r.counters.texture_cache_hits - before.texture_cache_hits,
        r.counters.temporary_allocations - before.temporary_allocations,
        allocations.allocations,
        allocations.bytes_allocated,
        r.texture_timings.source - times.source,
        r.texture_timings.deswizzle - times.deswizzle,
        r.texture_timings.decompress - times.decompress,
        r.texture_timings.upload - times.upload
    );
}
fn main() {
    let mut r = Renderer::new().unwrap();
    println!("adapter={:?}", r.capabilities);
    let d = common::desc(37, 35, 6);
    let (mut source, expected) = common::rgba_surface(d);
    let mut map = AddressMap::default();
    map.map(d.address, GuestAddress(0x80000), source.len() as u64)
        .unwrap();
    let mut changes = TextureChanges::new(1);
    sample(&mut r, d, &source, &map, &changes, "initial", 1);
    common::assert_pixels(
        &r.execute(
            37,
            35,
            &[Command::DrawTexture {
                id: TextureId(1),
                mip: 0,
            }],
        )
        .unwrap()
        .rgba,
        &expected[0],
        0,
    );
    sample(&mut r, d, &source, &map, &changes, "unchanged", 32);
    let at = common::gob_address(64, 9, 37 * 4, 2);
    source[at..at + 4].copy_from_slice(&[9, 87, 231, 113]);
    changes
        .mark_write(GuestAddress(0x80000 + at as u64), 4)
        .unwrap();
    sample(&mut r, d, &source, &map, &changes, "dirty_4_bytes", 1);
    let frame = r
        .execute(
            37,
            35,
            &[Command::DrawTexture {
                id: TextureId(1),
                mip: 0,
            }],
        )
        .unwrap();
    let pixel = (9 * 37 + 16) * 4;
    assert_eq!(&frame.rgba[pixel..pixel + 4], &[9, 87, 231, 113]);
    changes
        .mark_write(GuestAddress(0x80000 + at as u64), 4)
        .unwrap();
    changes
        .mark_write(GuestAddress(0x80000 + at as u64 + 1), 2)
        .unwrap();
    sample(&mut r, d, &source, &map, &changes, "overlapping_writes", 1);
    let mip = d.levels().unwrap()[1];
    changes
        .mark_write(GuestAddress(0x80000 + mip.offset as u64), 4)
        .unwrap();
    sample(&mut r, d, &source, &map, &changes, "mip1", 1);
    r.remove_texture(TextureId(1)).unwrap();
    sample(&mut r, d, &source, &map, &changes, "recreate", 1);
    let d = TextureDesc {
        format: Format::Astc4x4,
        ..d
    };
    let (source, expected) = common::compressed::surface(d);
    let mut map = AddressMap::default();
    map.map(d.address, GuestAddress(0x80000), source.len() as u64)
        .unwrap();
    sample(
        &mut r,
        d,
        &source,
        &map,
        &TextureChanges::new(2),
        "astc_fallback",
        1,
    );
    common::assert_pixels(
        &r.execute(
            37,
            35,
            &[Command::DrawTexture {
                id: TextureId(1),
                mip: 0,
            }],
        )
        .unwrap()
        .rgba,
        &expected[0],
        1,
    );
    println!("total={:?}", r.counters);
}
