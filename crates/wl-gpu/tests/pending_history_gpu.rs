#![cfg(feature = "wgpu-backend")]
mod common;
#[path = "common/gob.rs"]
mod gob;
use common::{maxwell::*, *};
use stats_alloc::{INSTRUMENTED_SYSTEM, Region, StatsAlloc};
use std::alloc::System;
use wl_gpu::{
    driver::*,
    driver_backend::*,
    maxwell::{TextureImage, TextureSampler},
    memory::*,
};
#[global_allocator]
static ALLOCATOR: &StatsAlloc<System> = &INSTRUMENTED_SYSTEM;
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

const Z: usize = 0x12000;
fn depth_setup(w: &mut Vec<u32>, compare: u32, write: bool) {
    let end = w.split_off(w.len() - 8);
    for (m, v) in [
        (0xfe0, 0),
        (0xfe4, 0xd0000),
        (0xfe8, 0xa),
        (0xfec, 0x10),
        (0xff0, 2048),
        (0x1228, W as u32),
        (0x122c, H as u32),
        (0x1230, 0x10001),
        (0x179c, 0),
        (0x12cc, 1),
        (0x12e8, u32::from(write)),
        (0x1538, 1),
        (0x130c, compare),
    ] {
        method(w, m, v);
    }
    w.extend(end);
}
fn initial_depth() -> Vec<u8> {
    let mut tiled = vec![0x6d; gob::size(W, H, 1)];
    for y in 0..H {
        for x in 0..W {
            let z = [0.25f32, 0.5, 0.75][(x + y) % 3];
            for (k, b) in z.to_le_bytes().into_iter().enumerate() {
                tiled[gob::address(x * 4 + k, y, W, 1)] = b;
            }
        }
    }
    tiled
}

// The command/shader fixture is shared in shape with depth_gpu, but expectations
// below are scalar GOB addresses and analytic pixel-center coverage, independent
// of production conversion, shader lowering and rasterization.
fn tiled_setup(w: &mut Vec<u32>) {
    let end = w.split_off(w.len() - 8);
    for (m, v) in [
        (0x808, W as u32),
        (0x814, 0x10),
        (0x81c, (gob::size(W, H, 1) / 4) as u32),
        (0x2144, 0x400),
    ] {
        method(w, m, v);
    }
    w.extend(end);
}
fn initial_color(data: &mut [u8]) -> Vec<u8> {
    let size = gob::size(W, H, 1);
    let padding: Vec<u8> = (0..size)
        .map(|i| (i.wrapping_mul(31) ^ (i >> 5)) as u8)
        .collect();
    let linear = BG.repeat(W * H);
    let tiled = gob::encode(&linear, W, H, 1, &padding);
    data[RT..RT + size].copy_from_slice(&tiled);
    tiled
}
fn visible_mask() -> Vec<bool> {
    let mut used = vec![false; gob::size(W, H, 1)];
    for y in 0..H {
        for byte_x in 0..W * 4 {
            used[gob::address(byte_x, y, W, 1)] = true;
        }
    }
    used
}
fn patch(d: &mut GpuDriver, b: BackingId, data: &mut [u8], a: usize, value: &[u8]) {
    d.write_backing(b, a as u64, value, &mut mem(data)).unwrap();
}
fn disjoint(d: &mut GpuDriver, b: BackingId, data: &mut [u8], count: usize) {
    for n in 0..count {
        patch(
            d,
            b,
            data,
            0x18000 + (n % 4096) * 4,
            &(n as u32).to_le_bytes(),
        );
    }
}
fn inside(x: usize, y: usize) -> Option<bool> {
    let v = POS.map(|p| p.map(f64::from));
    let p = [
        (x as f64 + 0.5) * 2. / 47. - 1.,
        1. - (y as f64 + 0.5) * 2. / 29.,
    ];
    let edge =
        |a: [f64; 2], b: [f64; 2]| (b[0] - a[0]) * (p[1] - a[1]) - (b[1] - a[1]) * (p[0] - a[0]);
    let area =
        (v[1][0] - v[0][0]) * (v[2][1] - v[0][1]) - (v[1][1] - v[0][1]) * (v[2][0] - v[0][0]);
    let b = [
        edge(v[1], v[2]) / area,
        edge(v[2], v[0]) / area,
        edge(v[0], v[1]) / area,
    ];
    if b.iter().any(|v| v.abs() < 0.005) {
        None
    } else {
        Some(x < 47 && b.iter().all(|v| *v >= 0.))
    }
}
fn verify_color(data: &[u8], initial: &[u8], patches: &[(usize, &[u8])]) {
    let mut expected = initial.to_vec();
    let mut tested = 0;
    for y in 0..H {
        for x in 0..W {
            if let Some(covered) = inside(x, y) {
                let a = gob::address(x * 4, y, W, 1);
                expected[a..a + 4].copy_from_slice(if covered { &[255, 0, 0, 255] } else { &BG });
                tested += 1;
            }
        }
    }
    for (a, value) in patches {
        expected[*a..*a + value.len()].copy_from_slice(value);
    }
    for y in 0..H {
        for x in 0..W {
            if inside(x, y).is_some() {
                let a = gob::address(x * 4, y, W, 1);
                assert_eq!(
                    &data[RT + a..RT + a + 4],
                    &expected[a..a + 4],
                    "color {x},{y}"
                );
            }
        }
    }
    for (a, used) in visible_mask().into_iter().enumerate() {
        if !used {
            assert_eq!(data[RT + a], expected[a], "padding {a}");
        }
    }
    assert!(tested > 1400);
}
fn verify_depth(data: &[u8], initial: &[u8], patches: &[(usize, &[u8])]) {
    let mut expected = initial.to_vec();
    for y in 0..H {
        for x in 0..W {
            if inside(x, y) == Some(true) {
                let a = gob::address(x * 4, y, W, 1);
                expected[a..a + 4].copy_from_slice(&0.5f32.to_le_bytes());
            }
        }
    }
    for (a, value) in patches {
        expected[*a..*a + value.len()].copy_from_slice(value);
    }
    for y in 0..H {
        for x in 0..W {
            if inside(x, y).is_some() {
                let a = gob::address(x * 4, y, W, 1);
                assert_eq!(
                    &data[Z + a..Z + a + 4],
                    &expected[a..a + 4],
                    "depth {x},{y}"
                );
            }
        }
    }
    for (a, used) in visible_mask().into_iter().enumerate() {
        if !used {
            assert_eq!(data[Z + a], expected[a], "depth padding {a}");
        }
    }
}
struct FailAt<'a>(&'a mut [u8], u64);
impl GuestMemory for FailAt<'_> {
    fn read(&self, a: GuestAddress, n: usize) -> wl_gpu::Result<&[u8]> {
        let start = usize::try_from(a.0).map_err(|_| wl_gpu::Error("read address".into()))?;
        let end = start
            .checked_add(n)
            .ok_or_else(|| wl_gpu::Error("read overflow".into()))?;
        self.0
            .get(start..end)
            .ok_or_else(|| wl_gpu::Error("read range".into()))
    }
}
impl GuestMemoryWrite for FailAt<'_> {
    fn validate_write(&self, a: GuestAddress, n: usize) -> wl_gpu::Result<()> {
        self.read(a, n).map(|_| ())
    }
    fn write(&mut self, a: GuestAddress, bytes: &[u8]) -> wl_gpu::Result<()> {
        self.validate_write(a, bytes.len())?;
        if a.0 == self.1 {
            return Err(wl_gpu::Error("injected atomic publication failure".into()));
        }
        self.0[a.0 as usize..a.0 as usize + bytes.len()].copy_from_slice(bytes);
        Ok(())
    }
}
#[test]
#[ignore = "requires actual GPU; disjoint journal rotation must retain pending tiled output"]
fn disjoint_writes_preserve_pending_target_and_warm_draws() {
    let (mut data, mut words, _) = fixture();
    let original = initial_color(&mut data);
    tiled_setup(&mut words);
    let (mut d, c, s, b, target) = driver();
    let r = submit(&mut d, c, b, &mut data, &words);
    assert!(r.fault.is_none(), "{:?}", r.fault);
    let status = d.wait(r.token).unwrap();
    assert!(status.host_completed && !status.guest_visible);
    let before = d.graphics_counters();
    let region = Region::new(ALLOCATOR);
    let start = std::time::Instant::now();
    disjoint(&mut d, b, &mut data, 4096);
    let elapsed = start.elapsed();
    let allocations = region.change();
    assert_eq!(d.graphics_counters(), before);
    assert_eq!(&data[RT..RT + original.len()], original);
    assert!(d.unmap(s, target).is_err());
    // Global cache history legitimately requires fresh source validation once;
    // it must not force publication or reinitialization of the protected target.
    let r = submit(&mut d, c, b, &mut data, &draw());
    assert!(r.fault.is_none(), "{:?}", r.fault);
    let warm = d.graphics_counters();
    assert_eq!(warm.readback_operations, before.readback_operations);
    assert_eq!(warm.synchronization_points, before.synchronization_points);
    assert_eq!(
        warm.render_target_upload_bytes,
        before.render_target_upload_bytes
    );
    for _ in 0..32 {
        assert!(submit_existing(&mut d, c, &mut data, 8).fault.is_none());
    }
    let after = d.graphics_counters();
    assert_eq!(after.submissions - warm.submissions, 32);
    assert_eq!(after.upload_bytes, warm.upload_bytes);
    assert_eq!(after.shader_binary_bytes, warm.shader_binary_bytes);
    assert_eq!(after.pipeline_allocations, warm.pipeline_allocations);
    assert_eq!(after.readback_operations, warm.readback_operations);
    assert_eq!(after.synchronization_points, warm.synchronization_points);
    assert_eq!(after.render_target_allocations, 1);
    assert_eq!(&data[RT..RT + original.len()], original);
    d.publish(c, &mut mem(&mut data)).unwrap();
    verify_color(&data, &original, &[]);
    let done = d.graphics_counters();
    assert_eq!(done.readback_operations, after.readback_operations + 1);
    assert_eq!(
        done.synchronization_points,
        after.synchronization_points + 1
    );
    eprintln!(
        "4096 disjoint RAM writes {elapsed:?}, Rust {allocations:?}; warm32 target_reuses={} uploads={} queues={} final_readback_bytes={}",
        after.render_target_reuses - warm.render_target_reuses,
        after.upload_bytes - warm.upload_bytes,
        after.submissions - warm.submissions,
        done.readback_bytes - after.readback_bytes
    );
    d.close(c, &mut mem(&mut data)).unwrap();
}
#[test]
#[ignore = "requires actual GPU; full overlap budget, atomic failure, disjoint rotation and publication retry"]
fn full_patch_budget_survives_failed_publication_and_eviction() {
    let (mut data, mut words, _) = fixture();
    let original = initial_color(&mut data);
    tiled_setup(&mut words);
    let (mut d, c, _, b, _) = driver();
    assert!(submit(&mut d, c, b, &mut data, &words).fault.is_none());
    assert_eq!(inside(20, 14), Some(true));
    let visible = gob::address(20 * 4, 14, W, 1);
    let pad = visible_mask().iter().position(|v| !*v).unwrap();
    // Two disjoint intervals; subsequent writes exercise capacity accounting,
    // rather than coalescing away notification count in the preflight.
    patch(&mut d, b, &mut data, RT + pad, &[0x99]);
    for _ in 0..31 {
        patch(&mut d, b, &mut data, RT + visible, &[91, 37, 211, 127]);
    }
    let saved = data[RT..RT + original.len()].to_vec();
    let before = d.graphics_counters();
    assert!(
        d.write_backing(b, (RT + visible) as u64, &[1, 2, 3, 4], &mut mem(&mut data))
            .is_err()
    );
    assert_eq!(&data[RT..RT + original.len()], saved);
    assert_eq!(d.graphics_counters(), before);
    disjoint(&mut d, b, &mut data, 4096);
    assert!(d.publish(c, &mut FailAt(&mut data, RT as u64)).is_err());
    assert!(d.has_pending_output());
    assert_eq!(&data[RT..RT + original.len()], saved);
    let failed = d.graphics_counters();
    disjoint(&mut d, b, &mut data, 4096);
    d.publish(c, &mut mem(&mut data)).unwrap();
    assert_eq!(d.graphics_counters().draw_commands, before.draw_commands);
    assert_eq!(
        d.graphics_counters().readback_operations,
        failed.readback_operations + 1
    );
    verify_color(
        &data,
        &original,
        &[(pad, &[0x99]), (visible, &[91, 37, 211, 127])],
    );
    d.close(c, &mut mem(&mut data)).unwrap();
}
#[test]
#[ignore = "requires actual GPU; depth publication retires independently of retryable color output"]
fn partial_depth_publication_retains_color_history_and_alias_identity() {
    let (mut data, mut words, _) = fixture();
    let original = initial_color(&mut data);
    let depth = initial_depth();
    data[Z..Z + depth.len()].copy_from_slice(&depth);
    tiled_setup(&mut words);
    depth_setup(&mut words, 0x207, true); // ALWAYS, not a host default.
    let (mut d, c, _, b, _) = driver();
    let r = submit(&mut d, c, b, &mut data, &words);
    assert!(r.fault.is_none(), "{:?}", r.fault);
    let a = gob::address(20 * 4, 14, W, 1);
    let pad = visible_mask().iter().position(|v| !*v).unwrap();
    patch(&mut d, b, &mut data, RT + a, &[1, 2, 3, 4]);
    patch(&mut d, b, &mut data, Z + a, &0.875f32.to_le_bytes());
    patch(&mut d, b, &mut data, Z + pad, &[0x42]);
    disjoint(&mut d, b, &mut data, 4096);
    let saved = data[RT..RT + original.len()].to_vec();
    assert!(d.publish(c, &mut FailAt(&mut data, RT as u64)).is_err());
    assert_eq!(&data[RT..RT + original.len()], saved);
    verify_depth(
        &data,
        &depth,
        &[(a, &0.875f32.to_le_bytes()), (pad, &[0x42])],
    );
    let published_depth = data[Z..Z + depth.len()].to_vec();
    let after_depth = d.graphics_counters();
    assert_eq!(after_depth.depth_write_bytes, depth.len() as u64);
    assert!(!d.query(r.token).unwrap().guest_visible);
    patch(&mut d, b, &mut data, RT + a, &[5, 6, 7, 8]);
    disjoint(&mut d, b, &mut data, 4096);
    d.publish(c, &mut mem(&mut data)).unwrap();
    verify_color(&data, &original, &[(a, &[5, 6, 7, 8])]);
    assert_eq!(&data[Z..Z + depth.len()], published_depth);
    let done = d.graphics_counters();
    assert_eq!(done.depth_write_bytes, after_depth.depth_write_bytes);
    assert_eq!(
        done.readback_operations,
        after_depth.readback_operations + 1
    );
    assert_eq!(done.draw_commands, 1);
    // Select another GPU VA for the same canonical color interval; then patch
    // and close with pending output. Close must drain and publish, not redirect.
    let mut alias = Vec::new();
    method(&mut alias, 0x804, 0xa0000);
    alias.extend(draw());
    assert!(submit(&mut d, c, b, &mut data, &alias).fault.is_none());
    patch(&mut d, b, &mut data, RT + a, &[9, 10, 11, 12]);
    disjoint(&mut d, b, &mut data, 4096);
    d.close(c, &mut mem(&mut data)).unwrap();
    assert_eq!(&data[RT + a..RT + a + 4], &[9, 10, 11, 12]);
    assert!(!d.has_pending_output());
}
#[test]
#[ignore = "requires actual GPU; full clears rebase only their attachment's overlapping CPU history"]
fn clear_supersedes_old_texel_patch_but_preserves_padding_and_other_attachment() {
    let (mut data, mut words, _) = fixture();
    let original = initial_color(&mut data);
    let depth = initial_depth();
    data[Z..Z + depth.len()].copy_from_slice(&depth);
    tiled_setup(&mut words);
    depth_setup(&mut words, 0x207, true);
    let (mut d, c, _, b, _) = driver();
    assert!(submit(&mut d, c, b, &mut data, &words).fault.is_none());
    let a = gob::address(20 * 4, 14, W, 1);
    let second = gob::address(21 * 4, 14, W, 1);
    let pad = visible_mask().iter().position(|v| !*v).unwrap();
    patch(&mut d, b, &mut data, RT + a, &[91, 37, 211, 127]);
    patch(&mut d, b, &mut data, RT + pad, &[0x55]);
    patch(&mut d, b, &mut data, Z + a, &0.875f32.to_le_bytes());
    disjoint(&mut d, b, &mut data, 4096);
    let before = d.graphics_counters();
    let mut clear = vec![0x20040360];
    clear.extend([0f32, 1., 0., 1.].map(f32::to_bits));
    method(&mut clear, 0xff4, (W as u32) << 16);
    method(&mut clear, 0xff8, (H as u32) << 16);
    method(&mut clear, 0x10f8, 0);
    method(&mut clear, 0x19d0, 0x3c);
    let report = submit(&mut d, c, b, &mut data, &clear);
    assert!(report.fault.is_none(), "{:?}", report.fault);
    assert_eq!(
        d.graphics_counters().readback_operations,
        before.readback_operations
    );
    patch(&mut d, b, &mut data, RT + second, &[7, 8, 9, 10]);
    disjoint(&mut d, b, &mut data, 4096);
    d.publish(c, &mut mem(&mut data)).unwrap();
    let mut expected = gob::encode(&[0, 255, 0, 255].repeat(W * H), W, H, 1, &original);
    expected[pad] = 0x55;
    expected[second..second + 4].copy_from_slice(&[7, 8, 9, 10]);
    assert_eq!(&data[RT..RT + expected.len()], expected);
    verify_depth(&data, &depth, &[(a, &0.875f32.to_le_bytes())]);
    assert_eq!(d.graphics_counters().clear_commands, 1);
    assert_eq!(d.graphics_counters().draw_commands, 1);
    d.close(c, &mut mem(&mut data)).unwrap();
}
