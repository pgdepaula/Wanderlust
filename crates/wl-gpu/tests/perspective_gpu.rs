#![cfg(feature = "wgpu-backend")]
mod common;
use common::{maxwell::*, *};
use stats_alloc::{INSTRUMENTED_SYSTEM, Region, StatsAlloc};
use std::{alloc::System, time::Instant};
#[global_allocator]
static ALLOCATOR: &StatsAlloc<System> = &INSTRUMENTED_SYSTEM;
use wl_gpu::{
    driver::*,
    driver_backend::*,
    maxwell::{TextureImage, TextureSampler},
    memory::*,
};
const RT: usize = 0x8000;
const SIZE: usize = 208 * 29;
const BG: [u8; 4] = [11, 17, 23, 255];
const POS: [[f32; 2]; 3] = [[-0.8, -0.6], [0.6, -0.4], [-0.2, 0.7]];
const UV: [[f32; 2]; 3] = [[0.1, 0.2], [0.9, 0.1], [0.25, 0.85]];
fn method(w: &mut Vec<u32>, m: u32, v: u32) {
    w.extend([0x20010000 | (m / 4), v]);
}
fn entry(address: u64, words: usize) -> [u8; 8] {
    // Independent libnx start | (num_cmds<<42).
    (address | ((words as u64) << 42)).to_le_bytes()
}
fn fixture() -> (Vec<u8>, Vec<u32>, Vec<u8>) {
    let desc = TextureImage::decode(TIC_RGBA)
        .unwrap()
        .with_sampler(TextureSampler::decode(TSC_NEAREST).unwrap());
    let (surface, images) = rgba_surface(desc);
    let mut data = vec![0u8; 0x20000];
    data[..64].copy_from_slice(&descriptor_bytes(TIC_RGBA, TSC_NEAREST));
    data[0x1000..0x1000 + surface.len()].copy_from_slice(&surface);
    for (off, bin) in [
        (
            0x4000,
            include_bytes!("fixtures/maxwell/tex.bin").as_slice(),
        ),
        (
            0x4100,
            include_bytes!("fixtures/maxwell/quarters.bin").as_slice(),
        ),
        (
            0x4200,
            include_bytes!("fixtures/maxwell/vertex.bin").as_slice(),
        ),
        (
            0x4400,
            include_bytes!("fixtures/maxwell/red.bin").as_slice(),
        ),
    ] {
        data[off..off + bin.len()].copy_from_slice(bin);
    }
    let ldc = include_bytes!("fixtures/maxwell/ldc.bin");
    data[0x4500..0x4500 + ldc.len()].copy_from_slice(ldc);
    for n in 0..3 {
        for (c, v) in [POS[n][0], POS[n][1], 0.5, 1., UV[n][0], UV[n][1]]
            .into_iter()
            .enumerate()
        {
            data[0x3000 + n * 24 + c * 4..0x3004 + n * 24 + c * 4]
                .copy_from_slice(&v.to_le_bytes());
        }
    }
    for (n, i) in [0u16, 1, 2].into_iter().enumerate() {
        data[0x6000 + n * 2..0x6002 + n * 2].copy_from_slice(&i.to_le_bytes());
    }
    for p in data[RT..RT + SIZE].chunks_exact_mut(4) {
        p.copy_from_slice(&BG);
    }
    let mut w = common::hardware::setup();
    for (m, v) in [
        (0x17c8, 0),
        (0x17cc, 0x80000),
        (0x17d0, 0),
        (0x17d4, 0x80005),
        (0x17d8, 1),
        (0x1434, 0),
        (0x1438, 0),
        (0x1644, 0),
    ] {
        method(&mut w, m, v);
    }
    w.extend([0x20010586, 4, 0x200105f7, 0, 0x200105f8, 3, 0x20010585, 0]);
    // TSC and CB are subregions of a page-aligned canonical allocation.
    // Change their actual B197 addresses, not the texture handle semantics.
    for word in &mut w {
        if *word == 0x31000 {
            *word = 0x31020;
        } else if *word == 0x40000 {
            *word = 0x40040;
        }
    }
    (data, w, images[0].clone())
}

const W: usize = 52;
const H: usize = 29;
fn mem(data: &mut [u8]) -> SliceMemoryMut<'_> {
    SliceMemoryMut {
        base: GuestAddress(0),
        bytes: data,
    }
}
fn driver() -> (GpuDriver, ChannelId, AddressSpaceId, BackingId, MappingId) {
    let mut d = GpuDriver::new().unwrap();
    eprintln!("adapter {:?}", d.capabilities().adapter);
    let b = d.register_backing(GuestAddress(0), 0x20000).unwrap();
    let s = d.create_space().unwrap();
    let c = d.create_channel(s).unwrap();
    let mut target = None;
    for (gpu, offset, size) in [
        (0x10000, 0x1000, 0x2000),
        (0x30000, 0, 0x1000),
        (0x31000, 0, 0x1000),
        (0x40000, 0, 0x1000),
        (0x50000, 0x4000, 0x1000),
        (0x70000, 0x3000, 0x1000),
        (0x80000, 0x6000, 0x1000),
        (0x90000, 0x8000, 0x4000),
        (0xa0000, 0x8000, 0x4000),
        (0xb0000, 0x10000, 0x2000),
        (0xd0000, 0x12000, 0x4000),
    ] {
        let r = d.reserve_fixed(s, GpuAddress(gpu), size).unwrap();
        let m = d.map(s, r, b, offset).unwrap();
        if gpu == 0x90000 {
            target = Some(m);
        }
    }
    (d, c, s, b, target.unwrap())
}
fn submit(
    d: &mut GpuDriver,
    c: ChannelId,
    b: BackingId,
    data: &mut [u8],
    words: &[u32],
) -> SubmissionReport {
    let bytes: Vec<u8> = words.iter().flat_map(|w| w.to_le_bytes()).collect();
    d.write_backing(b, 0x10000, &bytes, &mut mem(data)).unwrap();
    submit_existing(d, c, data, words.len())
}
fn submit_existing(
    d: &mut GpuDriver,
    c: ChannelId,
    data: &mut [u8],
    length: usize,
) -> SubmissionReport {
    let mut h = [0; 24];
    h[8..12].copy_from_slice(&1u32.to_le_bytes());
    h[12..16].copy_from_slice(&4u32.to_le_bytes());
    d.submit(
        c,
        KICKOFF_PB,
        &h,
        Some(&entry(0xb0000, length)),
        &mut mem(data),
    )
    .unwrap()
}
fn draw() -> Vec<u32> {
    vec![0x20010586, 4, 0x200105f7, 0, 0x200105f8, 3, 0x20010585, 0]
}
fn edge(a: [f64; 2], b: [f64; 2], p: [f64; 2]) -> f64 {
    (b[0] - a[0]) * (p[1] - a[1]) - (b[1] - a[1]) * (p[0] - a[0])
}
fn check(data: &[u8], clip_w: [f32; 3], perspective: bool) {
    let v = POS.map(|p| p.map(f64::from));
    let area = edge(v[0], v[1], v[2]);
    let mut checked = 0;
    for y in 0..H {
        for x in 0..W {
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
            let expected = if x < 47 && b.iter().all(|v| *v >= 0.) {
                let a = if perspective {
                    std::array::from_fn(|n| b[n] / f64::from(clip_w[n]))
                } else {
                    b
                };
                let sum: f64 = a.iter().sum();
                let uv = [a[1] / sum, a[2] / sum];
                // D3D point addressing snaps texel coordinates to at least 16.8
                // fixed point (functional spec 7.18.7), with snapped raster
                // positions too. Exclude one fractional step at a texel boundary;
                // the three literal samples are deliberately far from these ties.
                if uv
                    .iter()
                    .any(|v| ((*v * 4.).fract() - 0.5).abs() > 0.5 - 1. / 256.)
                {
                    continue;
                }
                let tx = (uv[0] * 4.).floor().clamp(0., 3.) as u8;
                let ty = (uv[1] * 4.).floor().clamp(0., 3.) as u8;
                checked += 1;
                [7 + tx * 60, 11 + ty * 50, 31 + tx * 9 + ty * 5, 255]
            } else {
                BG
            };
            assert_eq!(
                &data[RT + (y * W + x) * 4..RT + (y * W + x) * 4 + 4],
                &expected,
                "{x},{y} perspective={perspective}"
            );
        }
    }
    assert!(checked > 200);
}
fn vertex_bytes(clip_w: [f32; 3]) -> Vec<u8> {
    let uv = [[0., 0.], [1., 0.], [0., 1.]];
    (0..3)
        .flat_map(|n| {
            [
                POS[n][0] * clip_w[n],
                POS[n][1] * clip_w[n],
                0.5 * clip_w[n],
                clip_w[n],
                uv[n][0],
                uv[n][1],
            ]
        })
        .flat_map(f32::to_le_bytes)
        .collect()
}
#[test]
#[ignore = "requires actual GPU; guest IPA/RCP and unequal clip W through GpuDriver"]
fn guest_perspective_interpolation_and_stage_cache_identity() {
    let (mut data, mut words, _) = fixture();
    let (mut d, c, _, b, _) = driver();
    let bin = include_bytes!("fixtures/maxwell/perspective.bin");
    data[0x4600..0x4600 + bin.len()].copy_from_slice(bin);
    let clip_w = [1., 2., 4.];
    data[0x3000..0x3048].copy_from_slice(&vertex_bytes(clip_w));
    let mut tic = TIC_RGBA;
    tic[4] = (tic[4] & !0xffff) | 3;
    tic[5] = (tic[5] & !0xffff) | 3;
    data[..64].copy_from_slice(&descriptor_bytes(tic, TSC_NEAREST));
    // Literal 4x4 RGBA texels placed with an independent scalar GOB oracle.
    for y in 0..4 {
        for x in 0..4 {
            for (k, value) in [7 + x * 60, 11 + y * 50, 31 + x * 9 + y * 5, 255]
                .into_iter()
                .enumerate()
            {
                let off = gob_address(x as usize * 4 + k, y as usize, 16, 1);
                data[0x1000 + off] = value;
            }
        }
    }
    let end = words.split_off(words.len() - 8);
    method(&mut words, 0x2144, 0x600);
    words.extend(end);
    let report = submit(&mut d, c, b, &mut data, &words);
    assert!(report.fault.is_none(), "{:?}", report.fault);
    // An independently known covered pixel remains old RAM until publication.
    let covered = RT + (16 * W + 19) * 4;
    assert_eq!(&data[covered..covered + 4], &BG);
    d.publish(c, &mut mem(&mut data)).unwrap();
    check(&data, clip_w, true);
    for (x, y, color) in [
        (19, 16, [67, 11, 40, 255]),
        (18, 11, [7, 61, 36, 255]),
        (24, 12, [67, 61, 45, 255]),
    ] {
        assert_eq!(
            &data[RT + (y * W + x) * 4..RT + (y * W + x) * 4 + 4],
            &color
        );
    }
    d.retire(report.token).unwrap();
    let before = d.graphics_counters();
    let mut tokens = Vec::with_capacity(32);
    let region = Region::new(&INSTRUMENTED_SYSTEM);
    let start = Instant::now();
    for _ in 0..32 {
        let report = submit(&mut d, c, b, &mut data, &draw());
        assert!(report.fault.is_none());
        tokens.push(report.token);
    }
    eprintln!(
        "Perspective warm32 CPU {:?}, Rust {:?}",
        start.elapsed(),
        region.change()
    );
    let warm = d.graphics_counters();
    for (after, before) in [
        (warm.upload_bytes, before.upload_bytes),
        (warm.readback_operations, before.readback_operations),
        (warm.synchronization_points, before.synchronization_points),
        (
            warm.shader_module_allocations,
            before.shader_module_allocations,
        ),
        (warm.pipeline_allocations, before.pipeline_allocations),
        (warm.shader_binary_bytes, before.shader_binary_bytes),
        (warm.vertex_shader_bytes, before.vertex_shader_bytes),
        (warm.vertex_upload_bytes, before.vertex_upload_bytes),
        (warm.index_upload_bytes, before.index_upload_bytes),
        (warm.texture_uploads, before.texture_uploads),
        (warm.descriptor_reads, before.descriptor_reads),
    ] {
        assert_eq!(after, before);
    }
    assert_eq!(warm.submissions - before.submissions, 32);
    d.publish(c, &mut mem(&mut data)).unwrap();
    for token in tokens {
        d.retire(token).unwrap();
    }
    check(&data, clip_w, true);
    // Same vertex program, different guest FS interpolation metadata: the linked
    // VS module must change its qualifier, rather than reusing the linear module.
    let mut w = Vec::new();
    method(&mut w, 0x2144, 0);
    w.extend(draw());
    let before = d.graphics_counters();
    let report = submit(&mut d, c, b, &mut data, &w);
    assert!(report.fault.is_none(), "{:?}", report.fault);
    d.publish(c, &mut mem(&mut data)).unwrap();
    check(&data, clip_w, false);
    assert_eq!(
        d.graphics_counters().shader_module_allocations,
        before.shader_module_allocations + 2
    );
    let mut w = Vec::new();
    method(&mut w, 0x2144, 0x600);
    w.extend(draw());
    let report = submit(&mut d, c, b, &mut data, &w);
    assert!(report.fault.is_none());
    d.publish(c, &mut mem(&mut data)).unwrap();
    check(&data, clip_w, true);
    // Only guest vertex W/XY changes; shaders and pipeline remain resident.
    let next = [4., 1., 2.];
    let before = d.graphics_counters();
    d.write_backing(b, 0x3000, &vertex_bytes(next), &mut mem(&mut data))
        .unwrap();
    let report = submit(&mut d, c, b, &mut data, &draw());
    assert!(report.fault.is_none());
    d.publish(c, &mut mem(&mut data)).unwrap();
    check(&data, next, true);
    assert_eq!(
        d.graphics_counters().shader_module_allocations,
        before.shader_module_allocations
    );
    assert_eq!(
        d.graphics_counters().pipeline_allocations,
        before.pipeline_allocations
    );
    eprintln!("Perspective counters {:?}", d.graphics_counters());
    d.close(c, &mut mem(&mut data)).unwrap();
}
