#![cfg(feature = "wgpu-backend")]
mod common;
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
fn check(data: &[u8], color: [u8; 4], positions: [[f32; 2]; 3], transformed: bool) {
    let v = positions.map(|p| {
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
#[ignore = "requires actual GPU; guest ISETP and SEL runtime predicate dataflow"]
fn guest_predicate_comparisons_select_colors_without_data_specialization() {
    let (mut data, mut w, _) = fixture();
    let (mut d, c, _, b, _) = driver();
    let bin = include_bytes!("fixtures/maxwell/predicate.bin");
    data[0x4600..0x4600 + bin.len()].copy_from_slice(bin);
    let end = w.split_off(w.len() - 8);
    for (m, v) in [
        (0x2144, 0x600),
        (0x214c, 8),
        (0x2380, 64),
        (0x2384, 0),
        (0x2388, 0x40040),
        (0x2490, 1),
    ] {
        method(&mut w, m, v);
    }
    w.extend(end);
    let mut cached = None;
    for (a, bits_b, color_a, color_b, expected) in [
        (
            0u32,
            1u32,
            0x3e800000u32,
            0x3f400000u32,
            [64, 191, 191, 255],
        ),
        (1, 0, 0x3e800000, 0x3f400000, [191, 64, 191, 255]),
        (0, 0, 0x3e800000, 0x3f400000, [191, 64, 64, 255]),
        (
            0x80000000,
            0x7fffffff,
            0x3e800000,
            0x3f400000,
            [191, 64, 191, 255],
        ),
        (
            0x7fffffff,
            0x80000000,
            0x3e800000,
            0x3f400000,
            [64, 191, 191, 255],
        ),
        (0xffffffff, 0, 0x3e800000, 0x3f400000, [191, 64, 191, 255]),
        (0, 0xffffffff, 0x3e800000, 0x3f400000, [64, 191, 191, 255]),
        (
            0xffffffff,
            0xffffffff,
            0x3e800000,
            0x3f400000,
            [191, 64, 64, 255],
        ),
        (0, 1, 0x3f400000, 0x3e800000, [191, 64, 64, 255]),
    ] {
        let bytes: Vec<u8> = [a, bits_b, color_a, color_b]
            .into_iter()
            .flat_map(u32::to_le_bytes)
            .collect();
        d.write_backing(b, 0x50, &bytes, &mut mem(&mut data))
            .unwrap();
        let r = submit(&mut d, c, b, &mut data, &w);
        assert!(r.fault.is_none(), "{:?}", r.fault);
        d.publish(c, &mut mem(&mut data)).unwrap();
        check(&data, expected, POS, false);
        let now = d.graphics_counters();
        let key = (
            now.shader_module_allocations,
            now.pipeline_allocations,
            now.shader_decodes,
        );
        if let Some(prior) = cached {
            assert_eq!(key, prior);
        }
        cached = Some(key);
        d.retire(r.token).unwrap();
    }
    // Six independently assembled compare codes, each with less/equal/greater inputs.
    for mode in 1u64..=6 {
        let word = 0x5b60038000570407 | (mode << 49);
        d.write_backing(b, 0x4680, &word.to_le_bytes(), &mut mem(&mut data))
            .unwrap();
        for (a, bits_b) in [(0u32, 1u32), (1, 1), (2, 1)] {
            let predicate = match mode {
                1 => a < bits_b,
                2 => a == bits_b,
                3 => a <= bits_b,
                4 => a > bits_b,
                5 => a != bits_b,
                6 => a >= bits_b,
                _ => unreachable!(),
            };
            let bytes: Vec<u8> = [a, bits_b, 0x3e800000, 0x3f400000]
                .into_iter()
                .flat_map(u32::to_le_bytes)
                .collect();
            d.write_backing(b, 0x50, &bytes, &mut mem(&mut data))
                .unwrap();
            let r = submit(&mut d, c, b, &mut data, &draw());
            assert!(r.fault.is_none(), "{:?}", r.fault);
            d.publish(c, &mut mem(&mut data)).unwrap();
            let expected = [
                if predicate { 64 } else { 191 },
                if predicate { 191 } else { 64 },
                if a == bits_b { 64 } else { 191 },
                255,
            ];
            check(&data, expected, POS, false);
            d.retire(r.token).unwrap();
        }
    }
    eprintln!("Predicate counters {:?}", d.graphics_counters());
    d.close(c, &mut mem(&mut data)).unwrap();
}
#[test]
#[ignore = "requires actual GPU; guest VS predicate selects positions from mutable vertex memory"]
fn guest_vertex_predicate_selects_geometry_and_reuses_shaders() {
    let (mut data, mut w, _) = fixture();
    let (mut d, c, _, b, _) = driver();
    let bin = include_bytes!("fixtures/maxwell/vertex-predicate.bin");
    data[0x4800..0x4800 + bin.len()].copy_from_slice(bin);
    let end = w.split_off(w.len() - 8);
    method(&mut w, 0x2044, 0x800);
    method(&mut w, 0x204c, 7);
    method(&mut w, 0x2144, 0x400);
    w.extend(end);
    let r = submit(&mut d, c, b, &mut data, &w);
    assert!(r.fault.is_none(), "{:?}", r.fault);
    d.publish(c, &mut mem(&mut data)).unwrap();
    check(&data, [255, 0, 0, 255], POS, true);
    d.retire(r.token).unwrap();
    let before = d.graphics_counters();
    let r = submit(&mut d, c, b, &mut data, &draw());
    assert!(r.fault.is_none());
    assert_eq!(d.graphics_counters().upload_bytes, before.upload_bytes);
    d.publish(c, &mut mem(&mut data)).unwrap();
    d.retire(r.token).unwrap();
    // A genuine B197 clear discards the previous image before a changed-geometry draw.
    let positions = POS.map(|p| [-p[0], p[1]]);
    for (n, position) in positions.iter().enumerate() {
        d.write_backing(
            b,
            0x3000 + n as u64 * 24,
            &position[0].to_le_bytes(),
            &mut mem(&mut data),
        )
        .unwrap();
    }
    let mut commands = Vec::new();
    for (m, v) in [(0x10f8, 0), (0xff4, 52 << 16), (0xff8, 29 << 16)] {
        method(&mut commands, m, v);
    }
    commands.push(0x20040360);
    commands.extend(BG.map(|v| (f32::from(v) / 255.).to_bits()));
    method(&mut commands, 0x19d0, 0x3c);
    method(&mut commands, 0xff4, 47 << 16);
    commands.extend(draw());
    let r = submit(&mut d, c, b, &mut data, &commands);
    assert!(r.fault.is_none(), "{:?}", r.fault);
    d.publish(c, &mut mem(&mut data)).unwrap();
    check(&data, [255, 0, 0, 255], positions, true);
    let after = d.graphics_counters();
    assert_eq!(after.pipeline_allocations, before.pipeline_allocations);
    assert_eq!(
        after.shader_module_allocations,
        before.shader_module_allocations
    );
    assert_eq!(after.vertex_shader_decodes, before.vertex_shader_decodes);
    assert_eq!(after.vertex_upload_bytes - before.vertex_upload_bytes, 72);
    d.close(c, &mut mem(&mut data)).unwrap();
}
