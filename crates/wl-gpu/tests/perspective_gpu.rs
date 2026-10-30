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
    check_texels(data, clip_w, perspective, |tx, ty| {
        [7 + tx * 60, 11 + ty * 50, 31 + tx * 9 + ty * 5, 255]
    });
}
fn check_texels(
    data: &[u8],
    clip_w: [f32; 3],
    perspective: bool,
    texel: impl Fn(u8, u8) -> [u8; 4],
) {
    check_texels_tolerance(data, clip_w, perspective, texel, 0);
}
fn check_texels_tolerance(
    data: &[u8],
    clip_w: [f32; 3],
    perspective: bool,
    texel: impl Fn(u8, u8) -> [u8; 4],
    tolerance: u8,
) {
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
                texel(tx, ty)
            } else {
                BG
            };
            let actual = &data[RT + (y * W + x) * 4..RT + (y * W + x) * 4 + 4];
            for component in 0..4 {
                let allowed = if component == 3 { 0 } else { tolerance };
                assert!(
                    actual[component].abs_diff(expected[component]) <= allowed,
                    "{x},{y} component{component} actual={actual:?} expected={expected:?}"
                );
            }
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

#[test]
#[ignore = "requires actual GPU; bound TEXS reads B197-selected CB handles and independently checked perspective pixels"]
fn bound_texs_multibarrier_perspective_runtime_handle_and_cache() {
    let (mut data, mut words, _) = fixture();
    // B197 natively interprets Maxwell TICs; the B097 selection write is absent.
    assert_eq!(&words[2..4], &[0x200103c4, 1]);
    words.drain(2..4);
    let (mut d, c, _, b, _) = driver();
    let bin = include_bytes!("fixtures/maxwell/texs-perspective.bin");
    data[0x4600..0x4600 + bin.len()].copy_from_slice(bin);
    let clip_w = [1., 2., 4.];
    data[0x3000..0x3048].copy_from_slice(&vertex_bytes(clip_w));
    let mut tic = TIC_RGBA;
    tic[4] = (tic[4] & !0xffff) | 3;
    tic[5] = (tic[5] & !0xffff) | 3;
    // Distinct nonzero TIC/TSC indices, with index zero deliberately unusable.
    data[..32].fill(0);
    let descriptors = descriptor_bytes(tic, TSC_NEAREST);
    data[0x20..0x40].copy_from_slice(&descriptors[..32]);
    data[0x60..0x80].copy_from_slice(&descriptors[32..]);
    data[0x50..0x54].copy_from_slice(&0x00200001u32.to_le_bytes());
    data[0x90..0x94].copy_from_slice(&0x00200001u32.to_le_bytes());
    for y in 0..4 {
        for x in 0..4 {
            for (k, value) in [7 + x * 60, 11 + y * 50, 31 + x * 9 + y * 5, 255]
                .into_iter()
                .enumerate()
            {
                data[0x1000 + gob_address(x as usize * 4 + k, y as usize, 16, 1)] = value;
            }
        }
    }
    let end = words.split_off(words.len() - 8);
    method(&mut words, 0x2144, 0x600);
    method(&mut words, 0x214c, 12);
    method(&mut words, 0x157c, 1);
    method(&mut words, 0x1564, 2);
    method(&mut words, 0x2490, 0x21); // snapshot current selector into pixel CB slot 2
    method(&mut words, 0x2608, 2); // runtime bound-texture selector, not hidden slot zero
    words.extend(end);
    let report = submit(&mut d, c, b, &mut data, &words);
    assert!(report.fault.is_none(), "{:?}", report.fault);
    assert_eq!(
        &data[RT + (16 * W + 19) * 4..RT + (16 * W + 19) * 4 + 4],
        &BG
    );
    d.publish(c, &mut mem(&mut data)).unwrap();
    check(&data, clip_w, true);
    d.retire(report.token).unwrap();
    let before = d.graphics_counters();
    let region = Region::new(&INSTRUMENTED_SYSTEM);
    let mut tokens = Vec::with_capacity(32);
    for _ in 0..32 {
        let report = submit(&mut d, c, b, &mut data, &draw());
        assert!(report.fault.is_none(), "{:?}", report.fault);
        tokens.push(report.token);
    }
    let allocations = region.change();
    let warm = d.graphics_counters();
    for (a, z) in [
        (warm.upload_bytes, before.upload_bytes),
        (warm.readback_operations, before.readback_operations),
        (warm.synchronization_points, before.synchronization_points),
        (
            warm.shader_module_allocations,
            before.shader_module_allocations,
        ),
        (warm.pipeline_allocations, before.pipeline_allocations),
        (warm.constant_guest_bytes, before.constant_guest_bytes),
        (warm.descriptor_reads, before.descriptor_reads),
    ] {
        assert_eq!(a, z);
    }
    assert_eq!(warm.draw_commands, before.draw_commands + 32);
    d.publish(c, &mut mem(&mut data)).unwrap();
    check(&data, clip_w, true);
    for t in tokens {
        d.retire(t).unwrap();
    }
    // Rebind to a different CB slot/address through B197, preserving the binary.
    let mut rebind = Vec::new();
    method(&mut rebind, 0x2388, 0x40080);
    method(&mut rebind, 0x2490, 0x31);
    method(&mut rebind, 0x2608, 3);
    rebind.extend(draw());
    let report = submit(&mut d, c, b, &mut data, &rebind);
    assert!(report.fault.is_none(), "{:?}", report.fault);
    d.publish(c, &mut mem(&mut data)).unwrap();
    check(&data, clip_w, true);
    d.retire(report.token).unwrap();
    assert_eq!(d.graphics_counters().shader_decodes, before.shader_decodes);
    assert_eq!(
        d.graphics_counters().pipeline_allocations,
        before.pipeline_allocations
    );
    // The selected guest word is live: an invalid TIC/TSC handle must fault,
    // rather than draw using the prior resident texture or a default index.
    d.write_backing(b, 0x90, &u32::MAX.to_le_bytes(), &mut mem(&mut data))
        .unwrap();
    let draws = d.graphics_counters().draw_commands;
    let saved = data[RT..RT + SIZE].to_vec();
    let report = submit(&mut d, c, b, &mut data, &draw());
    assert!(report.fault.is_some());
    assert_eq!(d.graphics_counters().draw_commands, draws);
    assert_eq!(&data[RT..RT + SIZE], saved);
    eprintln!(
        "Bound TEXS warm32 Rust {allocations:?}; queue submissions {}, readbacks {}, waits {}",
        warm.submissions - before.submissions,
        warm.readback_operations - before.readback_operations,
        warm.synchronization_points - before.synchronization_points
    );
    d.close(c, &mut mem(&mut data)).unwrap();
}

#[test]
#[ignore = "requires actual GPU; missing bound-TEXS selector, CB binding and range fail before draw"]
fn bound_texs_missing_selector_binding_and_short_range_are_not_defaults() {
    for mode in 0..3 {
        let (mut data, mut words, _) = fixture();
        let (mut d, c, _, b, _) = driver();
        let bin = include_bytes!("fixtures/maxwell/texs-perspective.bin");
        data[0x4600..0x4600 + bin.len()].copy_from_slice(bin);
        let end = words.split_off(words.len() - 8);
        method(&mut words, 0x2144, 0x600);
        method(&mut words, 0x214c, 12);
        if mode != 0 {
            method(&mut words, 0x2608, 2);
        }
        if mode == 2 {
            method(&mut words, 0x2380, 16); // handle read starts exactly at end
            method(&mut words, 0x2490, 0x21);
        }
        words.extend(end);
        let saved = data[RT..RT + SIZE].to_vec();
        let r = submit(&mut d, c, b, &mut data, &words);
        assert!(r.fault.is_some(), "mode {mode}");
        assert_eq!(d.graphics_counters().draw_commands, 0);
        assert_eq!(d.graphics_counters().submissions, 0);
        assert_eq!(&data[RT..RT + SIZE], saved);
        assert!(d.publish(c, &mut mem(&mut data)).unwrap().is_none());
        d.close(c, &mut mem(&mut data)).unwrap();
    }
}

#[test]
#[ignore = "requires actual GPU; native block-linear packed HDR sampled through bound Maxwell TEXS"]
fn packed_hdr_bound_texs_pixels_padding_mutation_and_warm_resources() {
    let (mut data, mut words, _) = fixture();
    words.drain(2..4); // no Maxwell A compatibility selection on B197
    // Source-verified driver context enables vertex B; guest still supplies code.
    let start = words.iter().position(|w| *w == 0x20020810).unwrap();
    assert_eq!(words[start + 1], 0x11);
    words[start] = 0x20010811;
    words.remove(start + 1);
    let (mut d, c, _, b, _) = driver();
    let bin = include_bytes!("fixtures/maxwell/texs-perspective.bin");
    data[0x4600..0x4600 + bin.len()].copy_from_slice(bin);
    let clip_w = [1., 2., 4.];
    data[0x3000..0x3048].copy_from_slice(&vertex_bytes(clip_w));
    // Synthetic 4x4 RGB unsigned packed float, alpha=one, no commercial data.
    let tic = [
        0x78d7ffa1, 0x10000, 0x00600000, 0x00070000, 0xe8800003, 0x80000003, 0x03000000, 0,
    ];
    let descriptors = descriptor_bytes(tic, TSC_NEAREST);
    data[..32].fill(0);
    data[0x20..0x40].copy_from_slice(&descriptors[..32]);
    data[0x60..0x80].copy_from_slice(&descriptors[32..]);
    data[0x50..0x54].copy_from_slice(&0x00200001u32.to_le_bytes());
    data[0x1000..0x1200].fill(0xa5);
    // Independent exact binary encodings for .25, .5, .75 and 2.0.
    let f11 = [13u32 << 6, 14 << 6, (14 << 6) | 32, 16 << 6];
    for y in 0..4 {
        for x in 0..4 {
            let packed = f11[x] | (f11[y] << 11) | ((14 << 5) << 22);
            let at = 0x1000 + gob_address(x * 4, y, 16, 1);
            data[at..at + 4].copy_from_slice(&packed.to_le_bytes());
        }
    }
    let mut source_before = data[0x1000..0x1200].to_vec();
    let end = words.split_off(words.len() - 8);
    for (m, v) in [
        (0x2144, 0x600),
        (0x214c, 12),
        (0x2490, 0x21),
        (0x2608, 2),
        (0x157c, 1),
        (0x1564, 2),
    ] {
        method(&mut words, m, v);
    }
    words.extend(end);
    let r = submit(&mut d, c, b, &mut data, &words);
    assert!(r.fault.is_none(), "{:?}", r.fault);
    d.publish(c, &mut mem(&mut data)).unwrap();
    let expected = |x: u8, y: u8| {
        [
            [64, 128, 191, 255][x as usize],
            [64, 128, 191, 255][y as usize],
            128,
            255,
        ]
    };
    check_texels(&data, clip_w, true, expected);
    assert_eq!(&data[0x1000..0x1200], source_before);
    d.retire(r.token).unwrap();
    let before = d.graphics_counters();
    let mut tokens = Vec::with_capacity(32);
    for _ in 0..32 {
        let r = submit(&mut d, c, b, &mut data, &draw());
        assert!(r.fault.is_none());
        tokens.push(r.token);
    }
    let warm = d.graphics_counters();
    for (after, before) in [
        (warm.upload_bytes, before.upload_bytes),
        (warm.texture_conversions, before.texture_conversions),
        (warm.readback_operations, before.readback_operations),
        (warm.synchronization_points, before.synchronization_points),
        (warm.pipeline_allocations, before.pipeline_allocations),
        (
            warm.shader_module_allocations,
            before.shader_module_allocations,
        ),
    ] {
        assert_eq!(after, before);
    }
    d.publish(c, &mut mem(&mut data)).unwrap();
    check_texels(&data, clip_w, true, expected);
    for t in tokens {
        d.retire(t).unwrap();
    }
    // Mutate exactly one packed guest texel. Native texture values stay float
    // until the RGBA8 target performs its independently checked quantization.
    let at = 0x1000 + gob_address(4, 0, 16, 1);
    let packed = f11[3] | (f11[0] << 11) | ((14 << 5) << 22);
    d.write_backing(b, at as u64, &packed.to_le_bytes(), &mut mem(&mut data))
        .unwrap();
    source_before[at - 0x1000..at - 0x1000 + 4].copy_from_slice(&packed.to_le_bytes());
    let r = submit(&mut d, c, b, &mut data, &draw());
    assert!(r.fault.is_none());
    d.publish(c, &mut mem(&mut data)).unwrap();
    check_texels(&data, clip_w, true, |x, y| {
        let mut rgba = expected(x, y);
        if x == 1 && y == 0 {
            rgba[0] = 255;
        }
        rgba
    });
    assert_eq!(&data[0x1000..0x1200], source_before);
    assert_eq!(
        d.graphics_counters().pipeline_allocations,
        before.pipeline_allocations
    );
    assert_eq!(d.graphics_counters().shader_decodes, before.shader_decodes);
    d.retire(r.token).unwrap();
    eprintln!(
        "Packed HDR: cold guest bytes {}, deswizzle {}, uploads {}; warm32 queues {}, readbacks {}, waits {}; final counters {:?}",
        before.guest_texture_bytes,
        before.deswizzle_bytes,
        before.upload_bytes,
        warm.submissions - before.submissions,
        warm.readback_operations - before.readback_operations,
        warm.synchronization_points - before.synchronization_points,
        d.graphics_counters()
    );
    d.close(c, &mut mem(&mut data)).unwrap();
}

#[test]
#[ignore = "requires actual GPU; native single-level sampler bilinear/clamp/border and sRGB conversion gating"]
fn native_single_level_sampler_linear_clamp_and_srgb_gate() {
    let (mut data, mut words, _) = fixture();
    words.drain(2..4);
    let (mut d, c, _, b, _) = driver();
    let program = include_bytes!("fixtures/maxwell/texs-perspective.bin");
    data[0x4600..0x4600 + program.len()].copy_from_slice(program);
    let tic = [
        0x78d7ffa1, 0x10000, 0x00600000, 0x00070000, 0xe8800001, 0x80000001, 0x03000000, 0,
    ];
    let sampler = [
        0x92 | (1 << 13) | (1 << 14) | (1 << 17),
        2 | (2 << 4) | (3 << 6) | (3 << 8),
        15 << 20,
        0,
        0x3f800000,
        0,
        0x3f800000,
        0x3e800000,
    ];
    let desc = descriptor_bytes(tic, sampler);
    data[0x20..0x40].copy_from_slice(&desc[..32]);
    data[0x60..0x80].copy_from_slice(&desc[32..]);
    data[0x50..0x54].copy_from_slice(&0x00200001u32.to_le_bytes());
    let f11 = [13u32 << 6, (14 << 6) | 32]; // exact .25 and .75
    for y in 0..2 {
        for x in 0..2 {
            let packed = f11[x] | (f11[y] << 11) | ((14 << 5) << 22);
            let at = 0x1000 + gob_address(x * 4, y, 8, 1);
            data[at..at + 4].copy_from_slice(&packed.to_le_bytes());
        }
    }
    let end = words.split_off(words.len() - 8);
    for (m, v) in [
        (0x2144, 0x600),
        (0x214c, 12),
        (0x2490, 0x21),
        (0x2608, 2),
        (0x157c, 1),
        (0x1564, 2),
    ] {
        method(&mut words, m, v);
    }
    words.extend(end);
    let mut pipelines = None;
    for (i, (uv, color)) in [
        ([0.25f32, 0.25], [64, 64, 128, 255]),
        ([0.75, 0.25], [191, 64, 128, 255]),
        ([0.5, 0.5], [128, 128, 128, 255]),
        ([-0.5, 1.5], [64, 191, 128, 255]),
    ]
    .into_iter()
    .enumerate()
    {
        let mut vertices = vertex_bytes([1.; 3]);
        for vertex in vertices.chunks_exact_mut(24) {
            vertex[16..20].copy_from_slice(&uv[0].to_le_bytes());
            vertex[20..24].copy_from_slice(&uv[1].to_le_bytes());
        }
        d.write_backing(b, 0x3000, &vertices, &mut mem(&mut data))
            .unwrap();
        let commands = if i == 0 { words.clone() } else { draw() };
        let r = submit(&mut d, c, b, &mut data, &commands);
        assert!(r.fault.is_none(), "{:?}", r.fault);
        d.publish(c, &mut mem(&mut data)).unwrap();
        check_texels_tolerance(&data, [1.; 3], true, |_, _| color, 1);
        d.retire(r.token).unwrap();
        if let Some((p, m)) = pipelines {
            assert_eq!(d.graphics_counters().pipeline_allocations, p);
            assert_eq!(d.graphics_counters().shader_module_allocations, m);
        } else {
            pipelines = Some((
                d.graphics_counters().pipeline_allocations,
                d.graphics_counters().shader_module_allocations,
            ));
        }
    }
    // A one-texel sRGB image independently distinguishes texture conversion from
    // alpha handling and from the sampler's inactive border conversion fields.
    let srgb = [
        0x58d24908, 0x10000, 0x00600000, 0, 0x00c00000, 0x80000000, 0, 0,
    ];
    let bytes: Vec<_> = srgb.into_iter().flat_map(u32::to_le_bytes).collect();
    d.write_backing(b, 0x20, &bytes, &mut mem(&mut data))
        .unwrap();
    d.write_backing(b, 0x1000, &[128, 64, 192, 123], &mut mem(&mut data))
        .unwrap();
    for (enabled, color) in [(true, [55, 13, 134, 123]), (false, [128, 64, 192, 123])] {
        let word = if enabled {
            sampler[0]
        } else {
            sampler[0] & !(1 << 13)
        };
        d.write_backing(b, 0x60, &word.to_le_bytes(), &mut mem(&mut data))
            .unwrap();
        let r = submit(&mut d, c, b, &mut data, &draw());
        assert!(r.fault.is_none(), "{:?}", r.fault);
        d.publish(c, &mut mem(&mut data)).unwrap();
        check_texels_tolerance(&data, [1.; 3], true, |_, _| color, 1);
        d.retire(r.token).unwrap();
        assert_eq!(
            d.graphics_counters().pipeline_allocations,
            pipelines.unwrap().0
        );
        assert_eq!(
            d.graphics_counters().shader_module_allocations,
            pipelines.unwrap().1
        );
    }
    // The host view currently exposes one mip. Never silently replace implicit
    // LOD semantics by sampling base zero for a multilevel guest texture.
    let multilevel = [
        0x58d24908,
        0x10000,
        0x00600000,
        1 << 28,
        0x00800003,
        0x80000003,
        0,
        0x10,
    ];
    let bytes: Vec<_> = multilevel.into_iter().flat_map(u32::to_le_bytes).collect();
    d.write_backing(b, 0x20, &bytes, &mut mem(&mut data))
        .unwrap();
    let bytes: Vec<_> = TSC_NEAREST.into_iter().flat_map(u32::to_le_bytes).collect();
    d.write_backing(b, 0x60, &bytes, &mut mem(&mut data))
        .unwrap();
    let before = d.graphics_counters().draw_commands;
    let saved = data[RT..RT + SIZE].to_vec();
    let r = submit(&mut d, c, b, &mut data, &draw());
    assert!(r.fault.is_some());
    assert_eq!(d.graphics_counters().draw_commands, before);
    assert_eq!(&data[RT..RT + SIZE], saved);
    d.close(c, &mut mem(&mut data)).unwrap();
}

// Independent coverage and affine varying oracle. No production shader or
// resource conversion code computes the expected geometry or sample address.
fn check_vertex_constants(data: &[u8], image: &[u8], translation: [f32; 2], u_scale: f32) {
    let v = POS.map(|p| {
        [
            f64::from(p[0] + translation[0]),
            f64::from(p[1] + translation[1]),
        ]
    });
    let area = edge(v[0], v[1], v[2]);
    let mut inside = 0;
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
            if b.iter().any(|v| v.abs() < 0.006) {
                continue;
            }
            let expected = if x < 47 && b.iter().all(|v| *v >= 0.) {
                let u = (0..3)
                    .map(|n| b[n] * f64::from(UV[n][0] * u_scale))
                    .sum::<f64>()
                    * 37.;
                let v = (0..3).map(|n| b[n] * f64::from(UV[n][1])).sum::<f64>() * 19.;
                if (u > 0. && u < 37. && (u - u.round()).abs() < 0.025)
                    || (v > 0. && v < 19. && (v - v.round()).abs() < 0.025)
                {
                    continue;
                }
                inside += 1;
                let n = (v.floor().clamp(0., 18.) as usize * 37
                    + u.floor().clamp(0., 36.) as usize)
                    * 4;
                <[u8; 4]>::try_from(&image[n..n + 4]).unwrap()
            } else {
                BG
            };
            let n = RT + (y * W + x) * 4;
            assert_eq!(&data[n..n + 4], &expected, "vertex CB pixel {x},{y}");
        }
    }
    assert!(inside > 100);
}
#[test]
#[ignore = "requires actual GPU; stage-isolated guest vertex constants and buffer replacement"]
fn guest_vertex_constants_mutation_stage_isolation_and_uniform_replacement() {
    let (mut data, mut words, image) = fixture();
    let (mut d, c, _, b, _) = driver();
    let bin = include_bytes!("fixtures/maxwell/vertex-constant-alu.bin");
    data[0x4700..0x4700 + bin.len()].copy_from_slice(bin);
    data[0x98..0x9c].copy_from_slice(&1f32.to_le_bytes());
    let end = words.split_off(words.len() - 8);
    // Pixel CB0 stays at GPU40040; vertex group0 CB0 points at GPU40080.
    for (m, v) in [(0x2044, 0x700), (0x2388, 0x40080), (0x2410, 1)] {
        method(&mut words, m, v);
    }
    words.extend(end);
    let report = submit(&mut d, c, b, &mut data, &words);
    assert!(report.fault.is_none(), "{:?}", report.fault);
    d.publish(c, &mut mem(&mut data)).unwrap();
    check_vertex_constants(&data, &image, [0., 0.], 1.);
    d.retire(report.token).unwrap();
    let cold = d.graphics_counters();
    let mut tokens = Vec::new();
    for _ in 0..32 {
        let r = submit(&mut d, c, b, &mut data, &draw());
        assert!(r.fault.is_none(), "{:?}", r.fault);
        tokens.push(r.token);
    }
    let warm = d.graphics_counters();
    assert_eq!(warm.constant_guest_bytes, cold.constant_guest_bytes);
    assert_eq!(warm.upload_bytes, cold.upload_bytes);
    assert_eq!(warm.readback_operations, cold.readback_operations);
    assert_eq!(warm.synchronization_points, cold.synchronization_points);
    d.publish(c, &mut mem(&mut data)).unwrap();
    for t in tokens {
        d.retire(t).unwrap();
    }
    let patch: Vec<_> = [0.25f32, -0.125, 0.5]
        .into_iter()
        .flat_map(f32::to_le_bytes)
        .collect();
    d.write_backing(b, 0x90, &patch, &mut mem(&mut data))
        .unwrap();
    let bg: Vec<_> = (0..W * H).flat_map(|_| BG).collect();
    d.write_backing(b, RT as u64, &bg, &mut mem(&mut data))
        .unwrap();
    let r = submit(&mut d, c, b, &mut data, &draw());
    assert!(r.fault.is_none(), "{:?}", r.fault);
    d.publish(c, &mut mem(&mut data)).unwrap();
    check_vertex_constants(&data, &image, [0.25, -0.125], 0.5);
    d.retire(r.token).unwrap();
    let changed = d.graphics_counters();
    assert_eq!(changed.shader_decodes, cold.shader_decodes);
    assert_eq!(changed.vertex_shader_decodes, cold.vertex_shader_decodes);
    assert_eq!(changed.pipeline_allocations, cold.pipeline_allocations);
    assert_eq!(changed.constant_guest_bytes, warm.constant_guest_bytes + 12);
    // Replace PS + its shared uniform allocation, then return to textured PS.
    // Unchanged vertex constants must be restored into the replacement buffer.
    for offset in [0x400, 0] {
        d.write_backing(b, RT as u64, &bg, &mut mem(&mut data))
            .unwrap();
        let mut commands = Vec::new();
        method(&mut commands, 0x2144, offset);
        commands.extend(draw());
        let r = submit(&mut d, c, b, &mut data, &commands);
        assert!(r.fault.is_none(), "{:?}", r.fault);
        d.publish(c, &mut mem(&mut data)).unwrap();
        if offset == 0 {
            check_vertex_constants(&data, &image, [0.25, -0.125], 0.5);
        }
        d.retire(r.token).unwrap();
    }
    // A changed vertex binary now references zero-filled words. The new
    // resident shadow is zero too: buffer identity still must force the upload.
    let mut replacement = bin.to_vec();
    for bundle in replacement[80..].chunks_exact_mut(32) {
        for instruction in bundle[8..].chunks_exact_mut(8) {
            let word = u64::from_le_bytes(instruction.try_into().unwrap());
            if matches!(word >> 48, 0x4c58 | 0x4c68) {
                instruction.copy_from_slice(&(word + (16u64 << 20)).to_le_bytes());
            }
        }
    }
    d.write_backing(b, 0x4700, &replacement, &mut mem(&mut data))
        .unwrap();
    d.write_backing(b, RT as u64, &bg, &mut mem(&mut data))
        .unwrap();
    let mut rebound = Vec::new();
    method(&mut rebound, 0x2380, 128);
    method(&mut rebound, 0x2410, 1);
    rebound.extend(draw());
    let r = submit(&mut d, c, b, &mut data, &rebound);
    assert!(r.fault.is_none(), "{:?}", r.fault);
    d.publish(c, &mut mem(&mut data)).unwrap();
    check_vertex_constants(&data, &image, [0., 0.], 0.);
    d.retire(r.token).unwrap();
    // Failed new binding must not draw with previously resident constants.
    let prior = d.graphics_counters().draw_commands;
    let saved = data[RT..RT + SIZE].to_vec();
    let mut unbound = Vec::new();
    method(&mut unbound, 0x2410, 0);
    unbound.extend(draw());
    let r = submit(&mut d, c, b, &mut data, &unbound);
    assert!(r.fault.is_some());
    assert_eq!(d.graphics_counters().draw_commands, prior);
    assert_eq!(&data[RT..RT + SIZE], &saved);
    eprintln!(
        "Vertex CB warm32: guest {} upload {} queues {} readbacks {} waits {}; mutation {} bytes",
        warm.constant_guest_bytes - cold.constant_guest_bytes,
        warm.upload_bytes - cold.upload_bytes,
        warm.submissions - cold.submissions,
        warm.readback_operations - cold.readback_operations,
        warm.synchronization_points - cold.synchronization_points,
        changed.constant_guest_bytes - warm.constant_guest_bytes
    );
    d.close(c, &mut mem(&mut data)).unwrap();
}
