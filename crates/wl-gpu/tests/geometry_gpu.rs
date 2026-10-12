#![cfg(feature = "wgpu-backend")]
mod common;
use common::{maxwell::*, *};
use stats_alloc::{INSTRUMENTED_SYSTEM, Region, StatsAlloc};
use std::alloc::System;
#[global_allocator]
static ALLOCATOR: &StatsAlloc<System> = &INSTRUMENTED_SYSTEM;
use wl_gpu::{
    backend::Renderer, command::Command, fifo::Decoder, maxwell::*, memory::*, texture::*,
};
const POS: [[f32; 2]; 3] = [[-0.8, -0.6], [0.6, -0.4], [-0.2, 0.7]];
const UV: [[f32; 2]; 3] = [[0.1, 0.2], [0.9, 0.1], [0.25, 0.85]];
fn packets(e: &mut Maxwell3d, w: &[u32]) {
    let mut f = Decoder::default();
    for n in w {
        if let Some(m) = f.push_word(*n).unwrap() {
            e.apply(m).unwrap();
        }
    }
    f.finish().unwrap();
}
fn set(e: &mut Maxwell3d, method: u16, value: u32) {
    packets(e, &[0x20010000 | u32::from(method / 4), value]);
}
fn map(guest: u64, surface: usize) -> AddressMap {
    let mut m = AddressMap::default();
    for (g, a, n) in [
        (0x10000, 0x1000, surface as u64),
        (0x30000, 0, 32),
        (0x31000, 32, 32),
        (0x40000, 64, 32),
        (0x50000, 0x4000, 0x1000),
        (0x70000, guest, 72),
        (0x71000, guest, 72),
    ] {
        m.map(GpuAddress(g), GuestAddress(a), n).unwrap();
    }
    m
}
fn vertices(data: &mut [u8], offset: usize, pos: [[f32; 2]; 3], uv: [[f32; 2]; 3]) {
    for n in 0..3 {
        for (c, v) in [pos[n][0], pos[n][1], 0., 1., uv[n][0], uv[n][1]]
            .iter()
            .enumerate()
        {
            data[offset + n * 24 + c * 4..offset + n * 24 + c * 4 + 4]
                .copy_from_slice(&v.to_le_bytes());
        }
    }
}
fn edge(a: [f64; 2], b: [f64; 2], p: [f64; 2]) -> f64 {
    (b[0] - a[0]) * (p[1] - a[1]) - (b[1] - a[1]) * (p[0] - a[0])
}
fn check(pixels: &[u8], pos: [[f32; 2]; 3], uv: [[f32; 2]; 3], image: &[u8]) {
    let v = pos.map(|p| p.map(f64::from));
    let area = edge(v[0], v[1], v[2]);
    let mut inside = 0;
    for y in 0..29 {
        for x in 0..47 {
            let p = [
                (x as f64 + 0.5) * 2. / 47. - 1.,
                1. - (y as f64 + 0.5) * 2. / 29.,
            ];
            let b = [
                edge(v[1], v[2], p) / area,
                edge(v[2], v[0], p) / area,
                edge(v[0], v[1], p) / area,
            ];
            if b.iter().any(|v| v.abs() < 0.005) {
                continue;
            }
            let actual = &pixels[(y * 47 + x) * 4..(y * 47 + x + 1) * 4];
            if b.iter().any(|v| *v < 0.) {
                assert_eq!(actual, &[0, 0, 0, 255]);
                continue;
            }
            let u = (0..3).map(|n| b[n] * f64::from(uv[n][0])).sum::<f64>() * 37.;
            let v = (0..3).map(|n| b[n] * f64::from(uv[n][1])).sum::<f64>() * 19.;
            if (u - u.round()).abs() < 0.02 || (v - v.round()).abs() < 0.02 {
                continue;
            }
            let n =
                (v.floor().clamp(0., 18.) as usize * 37 + u.floor().clamp(0., 36.) as usize) * 4;
            for c in 0..4 {
                assert!(
                    actual[c].abs_diff(image[n + c]) <= 1,
                    "pixel {x},{y} uv texel {u},{v} channel{c}: actual {actual:?} expected {:?}",
                    &image[n..n + 4]
                );
            }
            inside += 1;
        }
    }
    assert!(inside > 150);
}
#[test]
#[ignore = "requires actual GPU; run explicitly with --ignored --nocapture"]
fn guest_vertex_arrays_and_interstage_texture_varyings() {
    let mut e = Maxwell3d::default();
    packets(&mut e, &POOL_PACKETS);
    packets(
        &mut e,
        &[
            0x20020582, 0, 0x50000, 0x20020850, 0x51, 0, 0x20020853, 8, 4, 0x200308e0, 32, 0,
            0x40000, 0x20010924, 1, 0x20020810, 0x11, 0x200, 0x20020813, 8, 0, 0x20040700, 0x1018,
            0, 0x70000, 1, 0x200207c0, 0, 0x70047, 0x20010620, 0, 0x20020458, 0x38200000,
            0x38800800,
        ],
    );
    let desc = TextureImage::decode(TIC_RGBA)
        .unwrap()
        .with_sampler(TextureSampler::decode(TSC_NEAREST).unwrap());
    let (surface, image) = rgba_surface(desc);
    let mut data = vec![0u8; 0x6000];
    data[..64].copy_from_slice(&descriptor_bytes(TIC_RGBA, TSC_NEAREST));
    data[0x1000..0x1000 + surface.len()].copy_from_slice(&surface);
    for (off, b) in [
        (
            0x4000,
            include_bytes!("fixtures/maxwell/tex.bin").as_slice(),
        ),
        (
            0x4200,
            include_bytes!("fixtures/maxwell/vertex.bin").as_slice(),
        ),
        (
            0x4300,
            include_bytes!("fixtures/maxwell/vertex-swap.bin").as_slice(),
        ),
    ] {
        data[off..off + b.len()].copy_from_slice(b);
    }
    vertices(&mut data, 0x3000, POS, UV);
    let moved = POS.map(|p| [p[0] + 0.2, p[1] - 0.1]);
    vertices(&mut data, 0x3100, moved, UV);
    let mut changes = TextureChanges::new(45);
    let m = map(0x3000, surface.len());
    let mut r = Renderer::new().unwrap();
    println!("adapter={:?}", r.capabilities.adapter);
    let sync =
        |r: &mut Renderer, e: &Maxwell3d, m: &AddressMap, data: &[u8], changes: &TextureChanges| {
            let memory = SliceMemory {
                base: GuestAddress(0),
                bytes: data,
            };
            r.sync_selected_fragment(e, m, &memory, changes).unwrap();
            r.sync_selected_vertices(e, m, &memory, changes, 0, 3)
                .unwrap();
        };
    sync(&mut r, &e, &m, &data, &changes);
    check(
        &r.execute(47, 29, &[Command::DrawVertices { first: 0, count: 3 }])
            .unwrap()
            .rgba,
        POS,
        UV,
        &image[0],
    );
    let before = r.counters;
    let region = Region::new(ALLOCATOR);
    for _ in 0..32 {
        sync(&mut r, &e, &m, &data, &changes);
    }
    let alloc = region.change();
    assert_eq!(alloc.allocations, 0);
    assert_eq!(alloc.reallocations, 0);
    assert_eq!(before.vertex_shader_bytes, r.counters.vertex_shader_bytes);
    assert_eq!(before.vertex_guest_bytes, r.counters.vertex_guest_bytes);
    assert_eq!(before.upload_bytes, r.counters.upload_bytes);
    vertices(&mut data, 0x3000, moved, UV);
    changes.mark_write(GuestAddress(0x3000), 72).unwrap();
    sync(&mut r, &e, &m, &data, &changes);
    check(
        &r.execute(47, 29, &[Command::DrawVertices { first: 0, count: 3 }])
            .unwrap()
            .rgba,
        moved,
        UV,
        &image[0],
    );
    assert_eq!(before.pipeline_allocations, r.counters.pipeline_allocations);
    let fixed = [[0.23, 0.31]; 3];
    vertices(&mut data, 0x3000, moved, fixed);
    changes.mark_write(GuestAddress(0x3000), 72).unwrap();
    sync(&mut r, &e, &m, &data, &changes);
    check(
        &r.execute(47, 29, &[Command::DrawVertices { first: 0, count: 3 }])
            .unwrap()
            .rgba,
        moved,
        fixed,
        &image[0],
    );
    set(&mut e, 0x1c08, 0x71000);
    set(&mut e, 0x1f04, 0x71047);
    let before = r.counters;
    sync(&mut r, &e, &m, &data, &changes);
    assert_eq!(before.vertex_guest_bytes, r.counters.vertex_guest_bytes);
    let remap = map(0x3100, surface.len());
    sync(&mut r, &e, &remap, &data, &changes);
    check(
        &r.execute(47, 29, &[Command::DrawVertices { first: 0, count: 3 }])
            .unwrap()
            .rgba,
        moved,
        UV,
        &image[0],
    );
    set(&mut e, 0x2044, 0x300);
    sync(&mut r, &e, &remap, &data, &changes);
    check(
        &r.execute(47, 29, &[Command::DrawVertices { first: 0, count: 3 }])
            .unwrap()
            .rgba,
        moved.map(|p| [p[1], p[0]]),
        UV,
        &image[0],
    );
    set(&mut e, 0x1f04, 0x71040);
    assert!(
        r.sync_selected_vertices(
            &e,
            &remap,
            &SliceMemory {
                base: GuestAddress(0),
                bytes: &data
            },
            &changes,
            0,
            3
        )
        .is_err()
    );
    assert!(
        r.execute(47, 29, &[Command::DrawVertices { first: 0, count: 3 }])
            .is_err()
    );
    println!(
        "methods={}, allocations={alloc:?}, counters={:?}",
        e.method_writes(),
        r.counters
    );
}
