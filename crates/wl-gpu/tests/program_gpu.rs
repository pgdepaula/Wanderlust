#![cfg(feature = "wgpu-backend")]
mod common;
use common::{maxwell::*, *};
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
    texture::*,
};
fn packets(engine: &mut Maxwell3d, words: &[u32]) {
    let mut decoder = Decoder::default();
    for word in words {
        if let Some(w) = decoder.push_word(*word).unwrap() {
            engine.apply(w).unwrap();
        }
    }
    decoder.finish().unwrap();
}
fn write(engine: &mut Maxwell3d, method: u16, value: u32) {
    engine
        .apply(MethodWrite {
            subchannel: 0,
            method,
            value,
        })
        .unwrap();
}
fn mappings(shader_backing: u64, surface: usize) -> AddressMap {
    let mut m = AddressMap::default();
    for (gpu, guest, size) in [
        (0x10000, 0x1000, surface as u64),
        (0x30000, 0, 32),
        (0x31000, 32, 32),
        (0x40000, 64, 32),
        (0x50000, shader_backing, 0x1000),
        (0x60000, shader_backing, 0x1000),
    ] {
        m.map(GpuAddress(gpu), GuestAddress(guest), size).unwrap();
    }
    m
}
#[test]
#[ignore = "requires actual GPU; explicitly run --ignored --nocapture"]
fn literal_b197_program_state_selects_guest_binaries_and_invalidates_code() {
    let mut r = Renderer::new().expect("actual GPU requested");
    println!("adapter={:?}", r.capabilities);
    let mut engine = Maxwell3d::default();
    packets(&mut engine, &POOL_PACKETS);
    // Literal byte-method definitions, divided by four only in packet headers.
    packets(
        &mut engine,
        &[
            0x20020582, 0, 0x50000, 0x20020850, 0x51, 0, 0x20020853, 8, 4, 0x200308e0, 32, 0,
            0x40000, 0x20010924, 1,
        ],
    );
    let desc = TextureImage::decode(TIC_RGBA)
        .unwrap()
        .with_sampler(TextureSampler::decode(TSC_NEAREST).unwrap());
    let (surface, expected) = rgba_surface(desc);
    let mut data = vec![0u8; 0x6000];
    data[..64].copy_from_slice(&descriptor_bytes(TIC_RGBA, TSC_NEAREST));
    data[0x1000..0x1000 + surface.len()].copy_from_slice(&surface);
    let tex = include_bytes!("fixtures/maxwell/tex.bin");
    let ldc = include_bytes!("fixtures/maxwell/ldc.bin");
    let quarters = include_bytes!("fixtures/maxwell/quarters.bin");
    let red = include_bytes!("fixtures/maxwell/red.bin");
    data[0x4000..0x4000 + tex.len()].copy_from_slice(tex);
    data[0x4100..0x4100 + ldc.len()].copy_from_slice(ldc);
    data[0x5000..0x5000 + red.len()].copy_from_slice(red);
    for (n, v) in [0f32, 0., 1., 1.].iter().enumerate() {
        data[80 + n * 4..84 + n * 4].copy_from_slice(&v.to_bits().to_le_bytes());
    }
    let mut changes = TextureChanges::new(31);
    let map = mappings(0x4000, surface.len());
    r.sync_selected_fragment(
        &engine,
        &map,
        &SliceMemory {
            base: GuestAddress(0),
            bytes: &data,
        },
        &changes,
    )
    .unwrap();
    assert_pixels(
        &r.execute(37, 19, &[Command::DrawFragment]).unwrap().rgba,
        &expected[0],
        1,
    );
    let before = r.counters;
    let region = Region::new(ALLOCATOR);
    let start = Instant::now();
    for _ in 0..32 {
        r.sync_selected_fragment(
            &engine,
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
    let alloc = region.change();
    assert_eq!(alloc.allocations, 0);
    assert_eq!(alloc.reallocations, 0);
    assert_eq!(before.shader_binary_bytes, r.counters.shader_binary_bytes);
    assert_eq!(before.shader_decodes, r.counters.shader_decodes);
    assert_eq!(before.upload_bytes, r.counters.upload_bytes);
    assert_eq!(before.descriptor_reads, r.counters.descriptor_reads);
    println!("32 unchanged program/resource syncs {elapsed:?}, allocations={alloc:?}");
    write(&mut engine, 0x214c, 4);
    assert!(
        r.sync_selected_fragment(
            &engine,
            &map,
            &SliceMemory {
                base: GuestAddress(0),
                bytes: &data
            },
            &changes
        )
        .is_err()
    );
    assert!(r.execute(37, 19, &[Command::DrawFragment]).is_err());
    write(&mut engine, 0x214c, 8);
    r.sync_selected_fragment(
        &engine,
        &map,
        &SliceMemory {
            base: GuestAddress(0),
            bytes: &data,
        },
        &changes,
    )
    .unwrap();
    // Data writes do not invalidate the mapped shader range.
    data[0x1000..0x1004].copy_from_slice(&[255, 31, 63, 255]);
    changes.mark_write(GuestAddress(0x1000), 4).unwrap();
    r.sync_selected_fragment(
        &engine,
        &map,
        &SliceMemory {
            base: GuestAddress(0),
            bytes: &data,
        },
        &changes,
    )
    .unwrap();
    assert_eq!(
        &r.execute(37, 19, &[Command::DrawFragment]).unwrap().rgba[..4],
        &[255, 31, 63, 255]
    );
    assert_eq!(r.counters.shader_decodes, before.shader_decodes);
    // Guest program offset selects LDC, with no host shader argument.
    packets(&mut engine, &[0x20010851, 0x100]);
    r.sync_selected_fragment(
        &engine,
        &map,
        &SliceMemory {
            base: GuestAddress(0),
            bytes: &data,
        },
        &changes,
    )
    .unwrap();
    assert!(
        r.execute(34, 13, &[Command::DrawFragment])
            .unwrap()
            .rgba
            .chunks_exact(4)
            .all(|p| p == [0, 0, 255, 255])
    );
    // Same address, different binary: explicit code write must invalidate compilation.
    data[0x4100..0x4100 + quarters.len()].copy_from_slice(quarters);
    changes
        .mark_write(GuestAddress(0x4100), quarters.len())
        .unwrap();
    r.sync_selected_fragment(
        &engine,
        &map,
        &SliceMemory {
            base: GuestAddress(0),
            bytes: &data,
        },
        &changes,
    )
    .unwrap();
    assert!(
        r.execute(34, 13, &[Command::DrawFragment])
            .unwrap()
            .rgba
            .chunks_exact(4)
            .all(|p| p == [64, 128, 191, 255])
    );
    let before = r.counters;
    write(&mut engine, 0x160c, 0x60000);
    r.sync_selected_fragment(
        &engine,
        &map,
        &SliceMemory {
            base: GuestAddress(0),
            bytes: &data,
        },
        &changes,
    )
    .unwrap();
    assert_eq!(before.shader_binary_bytes, r.counters.shader_binary_bytes);
    assert_eq!(before.shader_decodes, r.counters.shader_decodes);
    // Same GPU address is mapped to different guest backing.
    write(&mut engine, 0x160c, 0x50000);
    write(&mut engine, 0x2144, 0);
    let remapped = mappings(0x5000, surface.len());
    r.sync_selected_fragment(
        &engine,
        &remapped,
        &SliceMemory {
            base: GuestAddress(0),
            bytes: &data,
        },
        &changes,
    )
    .unwrap();
    assert!(
        r.execute(34, 13, &[Command::DrawFragment])
            .unwrap()
            .rgba
            .chunks_exact(4)
            .all(|p| p == [255, 0, 0, 255])
    );
    // Invalid selected code never continues with the last good pipeline.
    data[0x5000] ^= 1;
    changes.mark_write(GuestAddress(0x5000), 1).unwrap();
    assert!(
        r.sync_selected_fragment(
            &engine,
            &remapped,
            &SliceMemory {
                base: GuestAddress(0),
                bytes: &data
            },
            &changes
        )
        .is_err()
    );
    assert!(r.execute(34, 13, &[Command::DrawFragment]).is_err());
    write(&mut engine, 0x2140, 0x50);
    assert!(
        r.sync_selected_fragment(
            &engine,
            &map,
            &SliceMemory {
                base: GuestAddress(0),
                bytes: &data
            },
            &changes
        )
        .is_err()
    );
    write(&mut engine, 0x2140, 0x51);
    write(&mut engine, 0x160c, 0x70000);
    assert!(
        r.sync_selected_fragment(
            &engine,
            &map,
            &SliceMemory {
                base: GuestAddress(0),
                bytes: &data
            },
            &changes
        )
        .is_err()
    );
    write(&mut engine, 0x160c, 0x50000);
    write(&mut engine, 0x2144, 0);
    let mut short = AddressMap::default();
    short
        .map(GpuAddress(0x50000), GuestAddress(0x4000), 112)
        .unwrap();
    assert!(
        r.sync_selected_fragment(
            &engine,
            &short,
            &SliceMemory {
                base: GuestAddress(0),
                bytes: &data
            },
            &changes
        )
        .is_err()
    );
    assert!(r.execute(34, 13, &[Command::DrawFragment]).is_err());
    println!(
        "methods={}, counters={:?}",
        engine.method_writes(),
        r.counters
    );
}
