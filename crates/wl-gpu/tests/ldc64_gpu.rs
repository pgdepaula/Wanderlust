#![cfg(feature = "wgpu-backend")]
mod common;
#[global_allocator]
static ALLOCATOR: &stats_alloc::StatsAlloc<std::alloc::System> = &stats_alloc::INSTRUMENTED_SYSTEM;
use common::{maxwell::*, *};
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
fn check(data: &[u8], color: [u8; 4], transformed: bool) {
    let v = POS.map(|p| {
        if transformed {
            [f64::from(p[0]).max(0.), f64::from(p[1])]
        } else {
            p.map(f64::from)
        }
    });
    let area = edge(v[0], v[1], v[2]);
    for y in 0..H {
        for x in 0..W {
            let p = [
                (x as f64 + 0.5) * 2. / 47. - 1.,
                1. - (y as f64 + 0.5) * 2. / 29.,
            ];
            let weights = [
                edge(v[1], v[2], p) / area,
                edge(v[2], v[0], p) / area,
                edge(v[0], v[1], p) / area,
            ];
            if weights.iter().any(|v| v.abs() < 0.005) {
                continue;
            }
            let expected = if x < 47 && weights.iter().all(|v| *v >= 0.) {
                color
            } else {
                BG
            };
            assert_eq!(
                &data[RT + (y * W + x) * 4..RT + (y * W + x) * 4 + 4],
                &expected,
                "{x},{y}"
            );
        }
    }
}

#[test]
#[ignore = "requires actual GPU; B64 guest CB words, residency and full-pair faults"]
fn guest_ldc64_colors_mutation_and_deferred_reuse() {
    let (mut data, mut w, _) = fixture();
    let (mut d, c, _, b, _) = driver();
    let bin = include_bytes!("fixtures/maxwell/ldc64.bin");
    data[0x4600..0x4600 + bin.len()].copy_from_slice(bin);
    let end = w.split_off(w.len() - 8);
    for (m, v) in [
        (0x2144, 0x600),
        (0x214c, 4),
        (0x2380, 32),
        (0x2384, 0),
        (0x2388, 0x40040),
        (0x2490, 1),
    ] {
        method(&mut w, m, v);
    }
    w.extend(end);
    let mut cached = None;
    for (values, expected) in [
        ([0.25f32, 0.5, 0.75, 1.], [64, 128, 191, 255]),
        ([1., 0., 0.5, 0.25], [255, 0, 128, 64]),
        ([0., 1., 0.25, 0.75], [0, 255, 64, 191]),
    ] {
        let bytes: Vec<u8> = values.into_iter().flat_map(f32::to_le_bytes).collect();
        d.write_backing(b, 0x50, &bytes, &mut mem(&mut data))
            .unwrap();
        let r = submit(&mut d, c, b, &mut data, &w);
        assert!(r.fault.is_none(), "{:?}", r.fault);
        d.publish(c, &mut mem(&mut data)).unwrap();
        check(&data, expected, false);
        let now = d.graphics_counters();
        let key = (
            now.shader_decodes,
            now.shader_module_allocations,
            now.pipeline_allocations,
        );
        if let Some(prior) = cached {
            assert_eq!(key, prior);
        }
        cached = Some(key);
        d.retire(r.token).unwrap();
    }
    let commands: Vec<u8> = draw().into_iter().flat_map(u32::to_le_bytes).collect();
    d.write_backing(b, 0x10000, &commands, &mut mem(&mut data))
        .unwrap();
    let before = d.graphics_counters();
    let mut tokens = Vec::with_capacity(32);
    let started = std::time::Instant::now();
    let region = stats_alloc::Region::new(ALLOCATOR);
    for _ in 0..32 {
        let r = submit_existing(&mut d, c, &mut data, 8);
        assert!(r.fault.is_none());
        tokens.push(r.token);
    }
    let allocation = region.change();
    let warm = d.graphics_counters();
    assert_eq!(warm.draw_commands, before.draw_commands + 32);
    assert_eq!(warm.upload_bytes, before.upload_bytes);
    assert_eq!(
        warm.shader_module_allocations,
        before.shader_module_allocations
    );
    assert_eq!(warm.pipeline_allocations, before.pipeline_allocations);
    assert_eq!(warm.shader_decodes, before.shader_decodes);
    assert_eq!(warm.shader_binary_bytes, before.shader_binary_bytes);
    assert_eq!(warm.descriptor_reads, before.descriptor_reads);
    assert_eq!(warm.readback_bytes, before.readback_bytes);
    assert_eq!(warm.synchronization_points, before.synchronization_points);
    eprintln!(
        "LDC64 warm32 elapsed {:?}; before {before:?}; after {warm:?}",
        started.elapsed()
    );
    eprintln!("LDC64 warm32 allocator {allocation:?}");
    d.publish(c, &mut mem(&mut data)).unwrap();
    for token in tokens {
        d.retire(token).unwrap();
    }
    check(&data, [0, 255, 64, 191], false);
    // Second pair's high word lies just outside this newly bound CB. No stale draw.
    let mut invalid = Vec::new();
    for (m, v) in [(0x2380, 28), (0x2490, 1)] {
        method(&mut invalid, m, v);
    }
    invalid.extend(draw());
    let before = d.graphics_counters();
    let prior = data[RT..RT + SIZE].to_vec();
    let r = submit(&mut d, c, b, &mut data, &invalid);
    assert!(
        r.fault
            .as_ref()
            .is_some_and(|f| f.to_string().contains("constant pair exceeds")),
        "{:?}",
        r.fault
    );
    assert_eq!(d.graphics_counters().draw_commands, before.draw_commands);
    assert_eq!(&data[RT..RT + SIZE], prior);
    d.close(c, &mut mem(&mut data)).unwrap();
}

fn check_texture(data: &[u8], image: &[u8]) {
    let vertices = POS.map(|p| p.map(f64::from));
    let area = edge(vertices[0], vertices[1], vertices[2]);
    let mut checked = 0;
    for y in 0..H {
        for x in 0..W {
            let point = [
                (x as f64 + 0.5) * 2. / 47. - 1.,
                1. - (y as f64 + 0.5) * 2. / 29.,
            ];
            let bary = [
                edge(vertices[1], vertices[2], point) / area,
                edge(vertices[2], vertices[0], point) / area,
                edge(vertices[0], vertices[1], point) / area,
            ];
            if bary.iter().any(|v| v.abs() < 0.005) {
                continue;
            }
            let expected = if x >= 47 || bary.iter().any(|v| *v < 0.) {
                BG
            } else {
                let u = (0..3).map(|n| bary[n] * f64::from(UV[n][0])).sum::<f64>() * 37.;
                let v = (0..3).map(|n| bary[n] * f64::from(UV[n][1])).sum::<f64>() * 19.;
                if (u - u.round()).abs() < 0.02 || (v - v.round()).abs() < 0.02 {
                    continue;
                }
                let index = (v.floor().clamp(0., 18.) as usize * 37
                    + u.floor().clamp(0., 36.) as usize)
                    * 4;
                checked += 1;
                image[index..index + 4].try_into().unwrap()
            };
            for (actual, expected) in data[RT + (y * W + x) * 4..RT + (y * W + x + 1) * 4]
                .iter()
                .zip(expected)
            {
                assert!(
                    actual.abs_diff(expected) <= 1,
                    "{x},{y}: {actual} != {expected}"
                );
            }
        }
    }
    assert!(checked > 100);
}

#[test]
#[ignore = "requires actual GPU; TEX handle in B64 upper word chooses real TIC"]
fn guest_ldc64_upper_lane_selects_texture_and_rejects_unaligned_base() {
    let (mut data, mut w, image) = fixture();
    let (mut d, c, _, b, _) = driver();
    let bin = include_bytes!("fixtures/maxwell/tex-ldc64.bin");
    data[0x4600..0x4600 + bin.len()].copy_from_slice(bin);
    // Two literal TIC entries at a new pool base. TSC0 remains at guest0x20.
    let mut second = TIC_RGBA;
    second[1] = 0xd0000;
    let descriptors: Vec<u8> = TIC_RGBA
        .into_iter()
        .chain(second)
        .flat_map(u32::to_le_bytes)
        .collect();
    data[0x100..0x140].copy_from_slice(&descriptors);
    let other: Vec<u8> = image.iter().map(|v| v ^ 0x80).collect();
    let tiled = tile_mip(&other, 37, 19, wl_gpu::format::Format::Rgba8Unorm, 2);
    data[0x12000..0x12000 + tiled.len()].copy_from_slice(&tiled);
    // Deliberately invalid low-lane handle catches a decoder that uses R6 for TEX.
    data[0x40..0x44].copy_from_slice(&u32::MAX.to_le_bytes());
    let end = w.split_off(w.len() - 8);
    for (m, v) in [
        (0x2144, 0x600),
        (0x214c, 8),
        (0x1574, 0),
        (0x1578, 0x30100),
        (0x157c, 1),
        (0x2380, 8),
        (0x2384, 0),
        (0x2388, 0x40040),
        (0x2490, 1),
    ] {
        method(&mut w, m, v);
    }
    w.extend(end);
    let mut cached = None;
    for handle in [0u32, 1, 0] {
        d.write_backing(b, 0x44, &handle.to_le_bytes(), &mut mem(&mut data))
            .unwrap();
        let r = submit(&mut d, c, b, &mut data, &w);
        assert!(r.fault.is_none(), "{:?}", r.fault);
        d.publish(c, &mut mem(&mut data)).unwrap();
        check_texture(&data, if handle == 0 { &image } else { &other });
        let now = d.graphics_counters();
        let key = (
            now.shader_decodes,
            now.shader_module_allocations,
            now.pipeline_allocations,
        );
        if let Some(prior) = cached {
            assert_eq!(key, prior);
        }
        cached = Some(key);
        d.retire(r.token).unwrap();
    }
    // A genuine texture content write changes pixels without a new shader/pipeline.
    d.write_backing(b, 0x1000, &tiled, &mut mem(&mut data))
        .unwrap();
    let before = d.graphics_counters();
    let r = submit(&mut d, c, b, &mut data, &draw());
    assert!(r.fault.is_none(), "{:?}", r.fault);
    d.publish(c, &mut mem(&mut data)).unwrap();
    check_texture(&data, &other);
    let after = d.graphics_counters();
    assert_eq!(after.shader_decodes, before.shader_decodes);
    assert_eq!(
        after.shader_module_allocations,
        before.shader_module_allocations
    );
    assert_eq!(after.pipeline_allocations, before.pipeline_allocations);
    eprintln!("LDC64 TEX counters {after:?}");
    d.retire(r.token).unwrap();
    let mut invalid = Vec::new();
    method(&mut invalid, 0x2388, 0x40044);
    method(&mut invalid, 0x2490, 1);
    invalid.extend(draw());
    let r = submit(&mut d, c, b, &mut data, &invalid);
    assert!(
        r.fault
            .as_ref()
            .is_some_and(|f| f.to_string().contains("constant pair address or alignment")),
        "{:?}",
        r.fault
    );
    assert_eq!(d.graphics_counters().draw_commands, after.draw_commands);
    d.close(c, &mut mem(&mut data)).unwrap();
}

#[test]
#[ignore = "requires GPU resource objects; full B64 span validation before scalar uploads"]
fn ldc64_binding_preflight_rejects_fragmented_or_short_pair_without_stale_execution() {
    use wl_gpu::{
        backend::Renderer, command::Command, fifo::MethodWrite, maxwell::Maxwell3d,
        shader_program::FragmentProgram, texture::TextureChanges,
    };
    let mut r = Renderer::new().unwrap();
    r.set_resource_fragment(
        &FragmentProgram::decode(include_bytes!("fixtures/maxwell/ldc64.bin")).unwrap(),
    )
    .unwrap();
    let mut e = Maxwell3d::default();
    for (method, value) in [
        (0, 0xb197),
        (0x2380, 32),
        (0x2384, 0),
        (0x2388, 0x10000),
        (0x2490, 1),
    ] {
        e.apply(MethodWrite {
            subchannel: 0,
            method,
            value,
        })
        .unwrap();
    }
    let values: Vec<u8> = [0.0f32, 0., 0., 0., 0.25, 0.5, 0.75, 1.]
        .into_iter()
        .flat_map(f32::to_le_bytes)
        .collect();
    let memory = SliceMemory {
        base: GuestAddress(0),
        bytes: &values,
    };
    let changes = TextureChanges::new(8);
    let mut valid = AddressMap::default();
    valid.map(GpuAddress(0x10000), GuestAddress(0), 32).unwrap();
    r.sync_fragment_resources(&e, 4, &valid, &memory, &changes)
        .unwrap();
    let before = r.counters;
    let mut fragmented = AddressMap::default();
    // Every scalar word individually resolves, but a B64 pair crosses maps.
    for n in 0..8 {
        fragmented
            .map(GpuAddress(0x10000 + n * 4), GuestAddress(n * 4), 4)
            .unwrap();
    }
    assert!(
        r.sync_fragment_resources(&e, 4, &fragmented, &memory, &changes)
            .unwrap_err()
            .to_string()
            .contains("noncontiguous")
    );
    assert_eq!(
        r.counters.constant_upload_bytes,
        before.constant_upload_bytes
    );
    assert_eq!(r.counters.constant_guest_bytes, before.constant_guest_bytes);
    assert!(r.execute(4, 3, &[Command::DrawFragment]).is_err());
    let mut truncated = AddressMap::default();
    truncated
        .map(GpuAddress(0x10000), GuestAddress(0), 28)
        .unwrap();
    assert!(
        r.sync_fragment_resources(&e, 4, &truncated, &memory, &changes)
            .is_err()
    );
    let short = SliceMemory {
        base: GuestAddress(0),
        bytes: &values[..28],
    };
    assert!(
        r.sync_fragment_resources(&e, 4, &valid, &short, &TextureChanges::new(9))
            .is_err()
    );
    assert_eq!(
        r.counters.constant_upload_bytes,
        before.constant_upload_bytes
    );
    assert!(r.execute(4, 3, &[Command::DrawFragment]).is_err());
    r.sync_fragment_resources(&e, 4, &valid, &memory, &changes)
        .unwrap();
    let frame = r.execute(4, 3, &[Command::DrawFragment]).unwrap();
    for pixel in frame.rgba.chunks_exact(4) {
        assert_eq!(pixel, [64, 128, 191, 255]);
    }
}
