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
fn edge(a: [f64; 2], b: [f64; 2], p: [f64; 2]) -> f64 {
    (b[0] - a[0]) * (p[1] - a[1]) - (b[1] - a[1]) * (p[0] - a[0])
}
fn weights(x: usize, y: usize) -> [f64; 3] {
    let v = POS.map(|p| p.map(f64::from));
    let p = [
        (x as f64 + 0.5) * 2. / 47. - 1.,
        1. - (y as f64 + 0.5) * 2. / 29.,
    ];
    let a = edge(v[0], v[1], v[2]);
    [
        edge(v[1], v[2], p) / a,
        edge(v[2], v[0], p) / a,
        edge(v[0], v[1], p) / a,
    ]
}
fn accepts(f: usize, d: f32) -> bool {
    match f {
        0 => false,
        1 => 0.5 < d,
        2 => 0.5 == d,
        3 => 0.5 <= d,
        4 => 0.5 > d,
        5 => 0.5 != d,
        6 => 0.5 >= d,
        _ => true,
    }
}
fn verify(data: &[u8], initial: &[u8], texture: &[u8], f: usize, write: bool) {
    let mut used = vec![false; initial.len()];
    let mut tested = 0;
    for y in 0..H {
        for x in 0..W {
            let b = weights(x, y);
            let inside = x < 47 && b.iter().all(|v| *v > 0.);
            let initial_z = [0.25f32, 0.5, 0.75][(x + y) % 3];
            let pass = inside && accepts(f, initial_z);
            let z = if pass && write { 0.5f32 } else { initial_z };
            for k in 0..4 {
                used[gob::address(x * 4 + k, y, W, 1)] = true;
            }
            if x < 47 && b.iter().any(|v| v.abs() < 0.005) {
                continue;
            }
            let off = gob::address(x * 4, y, W, 1);
            assert_eq!(
                &data[Z + off..Z + off + 4],
                &z.to_le_bytes(),
                "depth {x},{y} f={f} write={write}"
            );
            let mut expected = BG;
            if pass {
                let u = (0..3).map(|n| b[n] * f64::from(UV[n][0])).sum::<f64>() * 37.;
                let v = (0..3).map(|n| b[n] * f64::from(UV[n][1])).sum::<f64>() * 19.;
                if (u - u.round()).abs() < 0.02 || (v - v.round()).abs() < 0.02 {
                    continue;
                }
                let off = (v.floor().clamp(0., 18.) as usize * 37
                    + u.floor().clamp(0., 36.) as usize)
                    * 4;
                expected.copy_from_slice(&texture[off..off + 4]);
            }
            let p = RT + (y * W + x) * 4;
            assert_eq!(&data[p..p + 4], &expected, "color {x},{y} f={f}");
            tested += 1;
        }
    }
    assert!(tested > 1300);
    for (n, u) in used.into_iter().enumerate() {
        if !u {
            assert_eq!(data[Z + n], initial[n], "padding{n}");
        }
    }
}
#[test]
#[ignore = "requires real GPU; source-derived ZF32 compares, writes and byte publication"]
fn driver_depth_comparisons_write_masks_and_order() {
    let (mut data, setup, texture) = fixture();
    let (mut d, c, _, b, _) = driver();
    let initial = initial_depth();
    for write in [false, true] {
        for f in 0..8 {
            d.write_backing(b, Z as u64, &initial, &mut mem(&mut data))
                .unwrap();
            let bg = BG.repeat(W * H);
            d.write_backing(b, RT as u64, &bg, &mut mem(&mut data))
                .unwrap();
            let mut w = setup.clone();
            depth_setup(&mut w, 0x200 + f as u32, write);
            let report = submit(&mut d, c, b, &mut data, &w);
            assert!(report.fault.is_none(), "{:?}", report.fault);
            assert_eq!(&data[Z..Z + initial.len()], &initial);
            assert_eq!(&data[RT..RT + SIZE], &bg);
            assert!(!d.query(report.token).unwrap().guest_visible);
            d.publish(c, &mut mem(&mut data)).unwrap();
            verify(&data, &initial, &texture, f, write);
            d.retire(report.token).unwrap();
        }
    }
    d.close(c, &mut mem(&mut data)).unwrap();
}
#[test]
#[ignore = "requires real GPU; resident depth, CPU patches and independently retryable attachments"]
fn driver_depth_warm_cpu_patches_partial_publication_and_fault() {
    let (mut data, mut w, _) = fixture();
    let (mut d, c, s, b, _) = driver();
    let initial = initial_depth();
    data[Z..Z + initial.len()].copy_from_slice(&initial);
    depth_setup(&mut w, 0x203, true);
    assert!(submit(&mut d, c, b, &mut data, &w).fault.is_none());
    let commands = draw();
    let bytes: Vec<_> = commands.iter().flat_map(|w| w.to_le_bytes()).collect();
    d.write_backing(b, 0x10000, &bytes, &mut mem(&mut data))
        .unwrap();
    let before = d.graphics_counters();
    let allocations = Region::new(ALLOCATOR);
    let start = std::time::Instant::now();
    for _ in 0..32 {
        assert!(
            submit_existing(&mut d, c, &mut data, commands.len())
                .fault
                .is_none()
        );
    }
    let elapsed = start.elapsed();
    let a = allocations.change();
    let after = d.graphics_counters();
    assert_eq!(after.submissions - before.submissions, 32);
    assert_eq!(after.upload_bytes, before.upload_bytes);
    assert_eq!(after.depth_allocations, before.depth_allocations);
    assert_eq!(after.readback_operations, before.readback_operations);
    assert_eq!(after.synchronization_points, before.synchronization_points);
    assert_eq!(after.pipeline_allocations, before.pipeline_allocations);
    assert_eq!(
        after.shader_module_allocations,
        before.shader_module_allocations
    );
    eprintln!(
        "depth warm32 {elapsed:?}; allocations={} reallocations={}; before={before:?} after={after:?}",
        a.allocations, a.reallocations
    );
    // CPU-after-GPU modifies one visible depth and one padding byte in raw GOB coordinates.
    let off = gob::address(20 * 4, 14, W, 1);
    d.write_backing(
        b,
        (Z + off) as u64,
        &0.125f32.to_le_bytes(),
        &mut mem(&mut data),
    )
    .unwrap();
    let padding = (0..initial.len())
        .find(|n| !(0..H).any(|y| (0..W * 4).any(|x| gob::address(x, y, W, 1) == *n)))
        .unwrap();
    d.write_backing(b, (Z + padding) as u64, &[0xe3], &mut mem(&mut data))
        .unwrap();
    assert!(d.reserve_fixed(s, GpuAddress(0xf0000), 0x1000).is_err());
    // Failed atomic depth write preserves both pending attachments and raw patches.
    {
        let mut fail = FailAt(&mut data, Z as u64);
        assert!(d.publish(c, &mut fail).is_err());
    }
    assert_eq!(d.graphics_counters().depth_write_bytes, 0);
    assert_eq!(&data[Z + off..Z + off + 4], &0.125f32.to_le_bytes());
    // Fault after earlier valid draws, then publish depth successfully but fail color.
    assert!(
        submit(&mut d, c, b, &mut data, &[0x20010041, 0])
            .fault
            .is_some()
    );
    let draws = d.graphics_counters().draw_commands;
    {
        let mut fail = FailAt(&mut data, RT as u64);
        assert!(d.publish(c, &mut fail).is_err());
    }
    assert!(d.has_pending_output());
    assert_eq!(&data[Z + off..Z + off + 4], &0.125f32.to_le_bytes());
    assert_eq!(data[Z + padding], 0xe3);
    let done = d.graphics_counters();
    d.publish(c, &mut mem(&mut data)).unwrap();
    assert_eq!(d.graphics_counters().draw_commands, draws);
    assert_eq!(
        d.graphics_counters().depth_write_bytes,
        done.depth_write_bytes
    );
    assert_eq!(
        d.graphics_counters().readback_operations,
        done.readback_operations + 1
    );
    assert!(!d.has_pending_output());
    d.close(c, &mut mem(&mut data)).unwrap();
}
struct FailAt<'a>(&'a mut [u8], u64);
impl GuestMemory for FailAt<'_> {
    fn read(&self, a: GuestAddress, n: usize) -> wl_gpu::Result<&[u8]> {
        self.0
            .get(a.0 as usize..a.0 as usize + n)
            .ok_or_else(|| wl_gpu::Error("read".into()))
    }
}
impl GuestMemoryWrite for FailAt<'_> {
    fn validate_write(&self, a: GuestAddress, n: usize) -> wl_gpu::Result<()> {
        self.read(a, n).map(|_| ())
    }
    fn write(&mut self, a: GuestAddress, v: &[u8]) -> wl_gpu::Result<()> {
        if a.0 == self.1 {
            return Err(wl_gpu::Error("injected color failure".into()));
        }
        self.0[a.0 as usize..a.0 as usize + v.len()].copy_from_slice(v);
        Ok(())
    }
}

#[test]
#[ignore = "requires real GPU; depth alias, CPU writes before draw and pending source guard"]
fn driver_depth_alias_rebind_cpu_refresh_and_source_fault() {
    let (mut data, mut w, _) = fixture();
    let (mut d, c, s, b, _) = driver();
    let initial = initial_depth();
    data[Z..Z + initial.len()].copy_from_slice(&initial);
    let reservation = d.reserve_fixed(s, GpuAddress(0xe0000), 0x4000).unwrap();
    let mapping = d.map(s, reservation, b, Z as u64).unwrap();
    depth_setup(&mut w, 0x203, true);
    assert!(submit(&mut d, c, b, &mut data, &w).fault.is_none());
    let before = d.graphics_counters();
    let mut w = Vec::new();
    method(&mut w, 0xfe4, 0xe0000);
    w.extend(draw());
    assert!(submit(&mut d, c, b, &mut data, &w).fault.is_none());
    assert_eq!(
        d.graphics_counters().depth_upload_bytes,
        before.depth_upload_bytes
    );
    assert_eq!(
        d.graphics_counters().readback_operations,
        before.readback_operations
    );
    // CPU-after-GPU then next draw must publish/merge and refresh actual depth.
    let off = gob::address(20 * 4, 14, W, 1);
    d.write_backing(
        b,
        (Z + off) as u64,
        &0.125f32.to_le_bytes(),
        &mut mem(&mut data),
    )
    .unwrap();
    assert!(submit(&mut d, c, b, &mut data, &draw()).fault.is_none());
    assert!(d.graphics_counters().coherence_barriers > before.coherence_barriers);
    d.publish(c, &mut mem(&mut data)).unwrap();
    assert_eq!(&data[Z + off..Z + off + 4], &0.125f32.to_le_bytes());
    d.unmap(s, mapping).unwrap();
    d.map(s, reservation, b, 0x18000).unwrap();
    let mut new = initial_depth();
    for y in 0..H {
        for x in 0..W {
            let a = gob::address(x * 4, y, W, 1);
            new[a..a + 4].copy_from_slice(&1f32.to_le_bytes());
        }
    }
    d.write_backing(b, 0x18000, &new, &mut mem(&mut data))
        .unwrap();
    let before = d.graphics_counters();
    assert!(submit(&mut d, c, b, &mut data, &draw()).fault.is_none());
    assert_eq!(
        d.graphics_counters().depth_upload_bytes,
        before.depth_upload_bytes + SIZE as u64
    );
    // A new constant buffer selected from pending depth triggers the shared source
    // barrier before rejecting its invalid handle, preserving earlier submitted draws.
    let mut alias = Vec::new();
    for (m, v) in [(0x2380, 256), (0x2384, 0), (0x2388, 0xe0000), (0x2490, 1)] {
        method(&mut alias, m, v);
    }
    alias.extend(draw());
    let result = submit(&mut d, c, b, &mut data, &alias);
    assert!(result.fault.is_some());
    assert!(d.graphics_counters().coherence_barriers > before.coherence_barriers);
    assert!(!d.has_pending_output());
    d.close(c, &mut mem(&mut data)).unwrap();
}

#[test]
#[ignore = "requires real GPU; invalid depth states fail before any draw or guest publication"]
fn driver_depth_invalid_attachment_and_storage_are_atomic() {
    for case in 0..4 {
        let (mut data, mut w, _) = fixture();
        let (mut d, c, _, _, _) = driver();
        let initial = initial_depth();
        data[Z..Z + initial.len()].copy_from_slice(&initial);
        depth_setup(&mut w, 0x201, true);
        let end = w.split_off(w.len() - 8);
        match case {
            0 => method(&mut w, 0xfe4, 0x90000),
            1 => method(&mut w, 0x1228, 51),
            2 => method(&mut w, 0xfe4, 0x40000),
            _ => {
                let off = gob::address(0, 0, W, 1);
                data[Z + off..Z + off + 4].copy_from_slice(&f32::NAN.to_le_bytes());
            }
        }
        w.extend(end);
        let before = data.clone();
        let bytes: Vec<u8> = w.iter().flat_map(|v| v.to_le_bytes()).collect();
        data[0x10000..0x10000 + bytes.len()].copy_from_slice(&bytes);
        let result = submit_existing(&mut d, c, &mut data, w.len());
        assert!(result.fault.is_some(), "case{case}");
        assert_eq!(d.graphics_counters().draw_commands, 0);
        assert!(!d.has_pending_output());
        assert_eq!(&data[RT..RT + SIZE], &before[RT..RT + SIZE]);
        assert_eq!(&data[Z..Z + initial.len()], &before[Z..Z + initial.len()]);
        d.close(c, &mut mem(&mut data)).unwrap();
    }
}
#[test]
#[ignore = "requires real GPU; ordered overlapping geometry and blending gated by real depth"]
fn driver_depth_overlapping_geometry_and_blending() {
    let (mut data, mut w, texture) = fixture();
    let (mut d, c, _, b, _) = driver();
    let initial = initial_depth();
    data[Z..Z + initial.len()].copy_from_slice(&initial);
    depth_setup(&mut w, 0x207, true);
    assert!(submit(&mut d, c, b, &mut data, &w).fault.is_none());
    // Farther red geometry must be rejected even though submitted later.
    for n in 0..3 {
        d.write_backing(
            b,
            0x3008 + n * 24,
            &0.625f32.to_le_bytes(),
            &mut mem(&mut data),
        )
        .unwrap();
    }
    let mut w = Vec::new();
    method(&mut w, 0x2144, 0x400);
    method(&mut w, 0x130c, 0x201);
    w.extend(draw());
    assert!(submit(&mut d, c, b, &mut data, &w).fault.is_none());
    d.publish(c, &mut mem(&mut data)).unwrap();
    verify(&data, &initial, &texture, 7, true);
    // Nearer red passes with depth writes disabled. Separate-alpha blending and
    // an R-only mask change only R; depth still contains the preceding .5.
    for n in 0..3 {
        d.write_backing(
            b,
            0x3008 + n * 24,
            &0.25f32.to_le_bytes(),
            &mut mem(&mut data),
        )
        .unwrap();
    }
    let previous = data[RT..RT + SIZE].to_vec();
    let before = d.graphics_counters();
    let mut w = Vec::new();
    for (m, v) in [
        (0x12e8, 0),
        (0x133c, 1),
        (0x1340, 0x8006),
        (0x1344, 0x4001),
        (0x1348, 0x4001),
        (0x134c, 0x8006),
        (0x1350, 0x4001),
        (0x1358, 0x4000),
        (0x1360, 1),
        (0x1a00, 1),
    ] {
        method(&mut w, m, v);
    }
    w.extend(draw());
    assert!(submit(&mut d, c, b, &mut data, &w).fault.is_none());
    d.publish(c, &mut mem(&mut data)).unwrap();
    assert_eq!(
        d.graphics_counters().depth_write_bytes,
        before.depth_write_bytes
    );
    for y in 0..H {
        for x in 0..W {
            let a = weights(x, y);
            if a.iter().any(|v| v.abs() < 0.005) {
                continue;
            }
            let off = RT + (y * W + x) * 4;
            let mut expected: [u8; 4] = previous[(y * W + x) * 4..(y * W + x) * 4 + 4]
                .try_into()
                .unwrap();
            if x < 47 && a.iter().all(|v| *v > 0.) {
                expected[0] = 255;
            }
            assert_eq!(
                &data[off..off + 4],
                &expected,
                "blend gated by depth {x},{y}"
            );
        }
    }
    d.close(c, &mut mem(&mut data)).unwrap();
}
#[test]
#[ignore = "requires real GPU; arbitrary normalized ZF32 bit patterns survive actual GPU transfers"]
fn driver_depth_normalized_bit_transfers() {
    let (mut data, mut w, _) = fixture();
    let (mut d, c, _, b, _) = driver();
    let mut initial = initial_depth();
    let values = [
        0u32,
        1f32.to_bits(),
        0x3eaaaaab,
        0x3f000001,
        0x00800000,
        0x3dcccccd,
        0x3f7fffff,
        0x3a83126f,
    ];
    for y in 0..H {
        for x in 0..W {
            let a = gob::address(x * 4, y, W, 1);
            initial[a..a + 4].copy_from_slice(&values[(x + 3 * y) % values.len()].to_le_bytes());
        }
    }
    data[Z..Z + initial.len()].copy_from_slice(&initial);
    depth_setup(&mut w, 0x200, true);
    let r = submit(&mut d, c, b, &mut data, &w);
    assert!(r.fault.is_none(), "{:?}", r.fault);
    d.publish(c, &mut mem(&mut data)).unwrap();
    assert_eq!(&data[Z..Z + initial.len()], &initial);
    assert_eq!(&data[RT..RT + SIZE], &BG.repeat(W * H));
    d.close(c, &mut mem(&mut data)).unwrap();
}

// Analytic homogeneous-depth oracle, independent of host clipping/raster code.
// Dedicated binary-exact NDC positions project through the guest 32x16 viewport
// to [(2,15),(31,13),(8,1)]. Near/far clipping below intersects edge midpoints,
// also exactly on the raster subpixel grid. D3D snaps post-clipped XY to n.8
// before interpolator setup (D3D11.3 spec 3.4.1); this fixture avoids attributing
// legitimate snapping error to the guest depth translation. Keep the original
// 47x29/POS depth regressions above unchanged.
const CLIP_POS: [[f32; 2]; 3] = [[-0.875, -0.875], [0.9375, -0.625], [-0.5, 0.875]];
fn clip_weights(x: usize, y: usize) -> [f64; 3] {
    // Independently authored window coordinates, not production viewport math.
    let v = [[2., 15.], [31., 13.], [8., 1.]];
    let p = [x as f64 + 0.5, y as f64 + 0.5];
    let a = edge(v[0], v[1], v[2]);
    [
        edge(v[1], v[2], p) / a,
        edge(v[2], v[0], p) / a,
        edge(v[0], v[1], p) / a,
    ]
}
fn clip_vertices(z: [f32; 3], w: [f32; 3]) -> Vec<u8> {
    (0..3)
        .flat_map(|n| {
            [
                CLIP_POS[n][0] * w[n],
                CLIP_POS[n][1] * w[n],
                z[n] * w[n],
                w[n],
                UV[n][0],
                UV[n][1],
            ]
            .into_iter()
            .flat_map(f32::to_le_bytes)
        })
        .collect()
}
fn verify_clip_depth(data: &[u8], initial: &[u8], z: [f32; 3], textured: bool) {
    let mut checked = 0;
    let mut covered = 0;
    let mut rejected_inside = 0;
    for y in 0..H {
        for x in 0..W {
            let b = clip_weights(x, y);
            let ndc_z = if z[0] == z[1] && z[1] == z[2] {
                f64::from(z[0])
            } else {
                (0..3).map(|n| b[n] * f64::from(z[n])).sum::<f64>()
            };
            let inside = x < 32 && y < 16 && b.iter().all(|v| *v > 0.);
            if b.iter().any(|v| v.abs() < 0.005)
                || (z[0] != z[1] || z[1] != z[2])
                    && (ndc_z.abs() < 0.003 || (ndc_z - 1.).abs() < 0.003)
            {
                continue;
            }
            let pass = inside && (0.0..=1.0).contains(&ndc_z);
            let off = gob::address(x * 4, y, W, 1);
            let expected_color = if pass && textured {
                // NoPerspective clipping preserves the original screen-affine
                // UV plane, even when homogeneous W differs across vertices.
                let u = (0..3).map(|n| b[n] * f64::from(UV[n][0])).sum::<f64>() * 37.;
                let v = (0..3).map(|n| b[n] * f64::from(UV[n][1])).sum::<f64>() * 19.;
                if (u - u.round()).abs() < 0.02 || (v - v.round()).abs() < 0.02 {
                    continue;
                }
                let x = u.floor().clamp(0., 36.) as usize;
                let y = v.floor().clamp(0., 18.) as usize;
                [
                    (x * 17 + y * 3) as u8,
                    (y * 29 + 7) as u8,
                    (x * 5 + y * 11 + 41) as u8,
                    (64 + (x * 3 + y * 7) % 192) as u8,
                ]
            } else if pass {
                [255, 0, 0, 255]
            } else {
                BG
            };
            assert_eq!(
                &data[RT + (y * W + x) * 4..RT + (y * W + x + 1) * 4],
                expected_color,
                "color {x},{y} z={z:?}, interpolated={ndc_z}"
            );
            if pass {
                covered += 1;
                let actual = f32::from_le_bytes(data[Z + off..Z + off + 4].try_into().unwrap());
                if z[0] == z[1] && z[1] == z[2] {
                    assert_eq!(actual.to_bits(), z[0].to_bits());
                } else {
                    assert!(
                        (f64::from(actual) - ndc_z).abs() <= 0.000002,
                        "depth {x},{y} actual={actual}, expected={ndc_z}"
                    );
                }
            } else {
                assert_eq!(&data[Z + off..Z + off + 4], &initial[off..off + 4]);
                rejected_inside += usize::from(inside);
            }
            checked += 1;
        }
    }
    assert!(checked > 1300, "{checked}");
    if z[0] == z[1] && z[1] == z[2] && !(0.0..=1.0).contains(&z[0]) {
        assert_eq!(covered, 0);
        assert!(rejected_inside > 150);
        assert_eq!(&data[Z..Z + initial.len()], initial);
        assert!(data[RT..RT + SIZE].chunks_exact(4).all(|p| p == BG));
    } else {
        assert!(covered > 20);
        if z[0] != z[1] || z[1] != z[2] {
            assert!(rejected_inside > 20);
        }
    }
    let mut used = vec![false; initial.len()];
    for y in 0..H {
        for x in 0..W * 4 {
            used[gob::address(x, y, W, 1)] = true;
        }
    }
    for (i, used) in used.into_iter().enumerate() {
        if !used {
            assert_eq!(data[Z + i], initial[i], "depth padding {i}");
        }
    }
}
#[test]
#[ignore = "requires actual GPU; explicit homogeneous zero-to-W clipping and depth publication"]
fn driver_explicit_z_clip_planes_partial_geometry_and_unequal_w() {
    let (mut data, mut setup, _) = fixture();
    let (mut d, c, space, backing, _) = driver();
    let initial = initial_depth();
    data[Z..Z + initial.len()].copy_from_slice(&initial);
    let end = setup.split_off(setup.len() - 8);
    method(&mut setup, 0x2144, 0x400); // Guest-selected independently assembled red PS.
    // Explicit guest viewport, surface clip, viewport clip and scissor.
    for (m, v) in [
        (0x0a00, 16f32.to_bits()),
        (0x0a04, (-8f32).to_bits()),
        (0x0a0c, 16f32.to_bits()),
        (0x0a10, 8f32.to_bits()),
        (0x0ff4, 32 << 16),
        (0x0ff8, 16 << 16),
        (0x0c00, 32 << 16),
        (0x0c04, 16 << 16),
        (0x0e04, 32 << 16),
        (0x0e08, 16 << 16),
    ] {
        method(&mut setup, m, v);
    }
    setup.extend(end);
    depth_setup(&mut setup, 0x207, true); // ALWAYS must not bypass homogeneous clipping.
    let mut missing = setup.clone();
    let at = missing.iter().position(|w| *w == 0x2001035f).unwrap();
    assert_eq!(missing[at + 1], 1);
    missing.drain(at..at + 2);
    let r = submit(&mut d, c, backing, &mut data, &missing);
    assert!(
        r.fault.as_ref().is_some_and(|e| e
            .to_string()
            .contains("incomplete or unsupported fixed raster state")),
        "{:?}",
        r.fault
    );
    assert_eq!(d.graphics_counters().draw_commands, 0);
    assert_eq!(d.graphics_counters().submissions, 0);
    assert!(data[RT..RT + SIZE].chunks_exact(4).all(|p| p == BG));
    d.close(c, &mut mem(&mut data)).unwrap();
    let c = d.create_channel(space).unwrap();
    let cases = [
        ([-0.25; 3], [1.; 3]),
        ([0.; 3], [1.; 3]),
        ([0.25; 3], [1.; 3]),
        ([0.5; 3], [1.; 3]),
        ([0.75; 3], [1.; 3]),
        ([1.; 3], [1.; 3]),
        ([1.25; 3], [1.; 3]),
        ([-0.5, 0.5, 0.5], [1.; 3]),
        ([0.5, 1.5, 0.5], [1.; 3]),
        ([-0.5, 0.5, 0.5], [0.5, 1., 2.]),
        ([0.5, 1.5, 0.5], [0.5, 1., 2.]),
        ([-0.5, 0.5, 0.5], [0.5, 1., 2.]),
        ([0.5, 1.5, 0.5], [0.5, 1., 2.]),
    ];
    let mut program_identity = None;
    for (case, (z, w)) in cases.into_iter().enumerate() {
        let textured = case >= 11;
        d.write_backing(backing, RT as u64, &BG.repeat(W * H), &mut mem(&mut data))
            .unwrap();
        d.write_backing(backing, Z as u64, &initial, &mut mem(&mut data))
            .unwrap();
        d.write_backing(backing, 0x3000, &clip_vertices(z, w), &mut mem(&mut data))
            .unwrap();
        let mut commands = setup.clone();
        if textured {
            let end = commands.split_off(commands.len() - 8);
            method(&mut commands, 0x2144, 0); // Real guest TEX/IPA pixel program.
            commands.extend(end);
        }
        let r = submit(&mut d, c, backing, &mut data, &commands);
        assert!(r.fault.is_none(), "{:?}", r.fault);
        assert!(data[RT..RT + SIZE].chunks_exact(4).all(|p| p == BG));
        assert_eq!(&data[Z..Z + initial.len()], initial);
        d.publish(c, &mut mem(&mut data)).unwrap();
        verify_clip_depth(&data, &initial, z, textured);
        d.retire(r.token).unwrap();
        let n = d.graphics_counters();
        let identity = (
            n.shader_decodes,
            n.vertex_shader_decodes,
            n.shader_module_allocations,
            n.pipeline_allocations,
        );
        if let Some((old_mode, old)) = program_identity {
            if old_mode == textured {
                assert_eq!(identity, old);
            }
        }
        program_identity = Some((textured, identity));
    }
    // Changing to an unsupported clip interval faults without replay or an
    // implicit publication of the already accepted last image.
    d.write_backing(backing, RT as u64, &BG.repeat(W * H), &mut mem(&mut data))
        .unwrap();
    d.write_backing(backing, Z as u64, &initial, &mut mem(&mut data))
        .unwrap();
    d.write_backing(
        backing,
        0x3000,
        &clip_vertices([0.5; 3], [1.; 3]),
        &mut mem(&mut data),
    )
    .unwrap();
    let mut commands = Vec::new();
    method(&mut commands, 0x2144, 0x400);
    commands.extend(draw());
    assert!(
        submit(&mut d, c, backing, &mut data, &commands)
            .fault
            .is_none()
    );
    let before = d.graphics_counters();
    let r = submit(&mut d, c, backing, &mut data, &[0x2001035f, 0]);
    assert!(
        r.fault.as_ref().is_some_and(|e| e
            .to_string()
            .contains("UnsupportedValue { method: 3452, value: 0 }")),
        "{:?}",
        r.fault
    );
    assert_eq!(d.graphics_counters().draw_commands, before.draw_commands);
    assert_eq!(d.graphics_counters().submissions, before.submissions);
    assert!(data[RT..RT + SIZE].chunks_exact(4).all(|p| p == BG));
    d.publish(c, &mut mem(&mut data)).unwrap();
    verify_clip_depth(&data, &initial, [0.5; 3], false);
    assert_eq!(d.graphics_counters().draw_commands, before.draw_commands);
    eprintln!("explicit Z clipping counters {:?}", d.graphics_counters());
    d.close(c, &mut mem(&mut data)).unwrap();
}

fn z16_initial() -> Vec<u8> {
    let row = W * 2;
    let mut tiled = vec![0x6d; row.div_ceil(64) * H.div_ceil(16) * 1024];
    for y in 0..H {
        for x in 0..W {
            let code = [8192u16, 24575, 40959, 57343][(x + y) % 4];
            for (k, b) in code.to_le_bytes().into_iter().enumerate() {
                tiled[gob::address_bytes(x * 2 + k, y, row, 1)] = b;
            }
        }
    }
    tiled
}
fn z16_setup(words: &mut Vec<u32>, comparison: u32, write: bool) {
    depth_setup(words, comparison, write);
    let end = words.split_off(words.len() - 8);
    method(words, 0xfe8, 0x13);
    method(words, 0xff0, 1024);
    words.extend(end);
}
fn z16_verify(data: &[u8], initial: &[u8], texture: &[u8], f: usize, write: bool) {
    let mut used = vec![false; initial.len()];
    let mut checked = 0;
    for y in 0..H {
        for x in 0..W {
            let off = gob::address_bytes(x * 2, y, W * 2, 1);
            used[off] = true;
            used[off + 1] = true;
            let code = u16::from_le_bytes(initial[off..off + 2].try_into().unwrap());
            let b = weights(x, y);
            if x < 47 && b.iter().any(|v| v.abs() < 0.005) {
                continue;
            }
            let inside = x < 47 && b.iter().all(|v| *v > 0.);
            // Vertex Z=0.375 has an unambiguous nearest D16 code 24576.
            // Stored test values avoid the incoming code and quantization ties.
            // Thus float-versus-quantized comparison precision cannot change this oracle.
            let stored = f64::from(code) / 65535.;
            let pass = inside
                && match f {
                    0 => false,
                    1 => 0.375 < stored,
                    2 => 0.375 == stored,
                    3 => 0.375 <= stored,
                    4 => 0.375 > stored,
                    5 => 0.375 != stored,
                    6 => 0.375 >= stored,
                    _ => true,
                };
            let expected = if pass && write { 24576u16 } else { code };
            assert_eq!(
                &data[Z + off..Z + off + 2],
                &expected.to_le_bytes(),
                "Z16 {x},{y} compare={f} write={write}"
            );
            let mut color = BG;
            if pass {
                let u = (0..3).map(|n| b[n] * f64::from(UV[n][0])).sum::<f64>() * 37.;
                let v = (0..3).map(|n| b[n] * f64::from(UV[n][1])).sum::<f64>() * 19.;
                if (u - u.round()).abs() < 0.02 || (v - v.round()).abs() < 0.02 {
                    continue;
                }
                let p = (v.floor().clamp(0., 18.) as usize * 37
                    + u.floor().clamp(0., 36.) as usize)
                    * 4;
                color.copy_from_slice(&texture[p..p + 4]);
            }
            let p = RT + (y * W + x) * 4;
            assert_eq!(&data[p..p + 4], &color, "Z16 color {x},{y} compare={f}");
            checked += 1;
        }
    }
    assert!(checked > 1300);
    for (n, visible) in used.into_iter().enumerate() {
        if !visible {
            assert_eq!(data[Z + n], initial[n], "Z16 padding {n}");
        }
    }
}
#[test]
#[ignore = "requires real GPU; native Z16 tests/writes and exact guest block-linear bytes"]
fn driver_z16_comparisons_and_publication() {
    let (mut data, setup, texture) = fixture();
    for n in 0..3 {
        data[0x3008 + n * 24..0x300c + n * 24].copy_from_slice(&0.375f32.to_le_bytes());
    }
    let (mut d, c, _, b, _) = driver();
    let initial = z16_initial();
    for write in [false, true] {
        for f in 0..8 {
            d.write_backing(b, Z as u64, &initial, &mut mem(&mut data))
                .unwrap();
            d.write_backing(b, RT as u64, &BG.repeat(W * H), &mut mem(&mut data))
                .unwrap();
            let mut words = setup.clone();
            z16_setup(&mut words, 0x200 + f as u32, write);
            let report = submit(&mut d, c, b, &mut data, &words);
            assert!(report.fault.is_none(), "{:?}", report.fault);
            assert_eq!(&data[Z..Z + initial.len()], &initial);
            assert!(!d.query(report.token).unwrap().guest_visible);
            d.publish(c, &mut mem(&mut data)).unwrap();
            z16_verify(&data, &initial, &texture, f, write);
            d.retire(report.token).unwrap();
        }
    }
    eprintln!("Z16 compare counters {:?}", d.graphics_counters());
    d.close(c, &mut mem(&mut data)).unwrap();
}
#[test]
#[ignore = "requires real GPU; Z16 raw byte patches, alias residency and publication retry"]
fn driver_z16_alias_cpu_patches_retry_and_warm_residency() {
    let (mut data, mut words, _) = fixture();
    for n in 0..3 {
        data[0x3008 + n * 24..0x300c + n * 24].copy_from_slice(&0.375f32.to_le_bytes());
    }
    let initial = z16_initial();
    data[Z..Z + initial.len()].copy_from_slice(&initial);
    let (mut d, c, s, b, _) = driver();
    let r = d.reserve_fixed(s, GpuAddress(0xe0000), 0x1000).unwrap();
    let mapping = d.map(s, r, b, Z as u64).unwrap();
    z16_setup(&mut words, 0x207, true);
    assert!(submit(&mut d, c, b, &mut data, &words).fault.is_none());
    let mut alias = Vec::new();
    method(&mut alias, 0xfe4, 0xe0000);
    alias.extend(draw());
    let before = d.graphics_counters();
    assert!(submit(&mut d, c, b, &mut data, &alias).fault.is_none());
    assert_eq!(
        d.graphics_counters().depth_upload_bytes,
        before.depth_upload_bytes
    );
    assert_eq!(
        d.graphics_counters().depth_allocations,
        before.depth_allocations
    );
    assert!(d.unmap(s, mapping).is_err());
    let bytes: Vec<_> = draw().iter().flat_map(|v| v.to_le_bytes()).collect();
    d.write_backing(b, 0x10000, &bytes, &mut mem(&mut data))
        .unwrap();
    let before = d.graphics_counters();
    for _ in 0..32 {
        assert!(submit_existing(&mut d, c, &mut data, 8).fault.is_none());
    }
    let after = d.graphics_counters();
    assert_eq!(after.submissions - before.submissions, 32);
    assert_eq!(after.depth_upload_bytes, before.depth_upload_bytes);
    assert_eq!(after.upload_bytes, before.upload_bytes);
    assert_eq!(after.depth_allocations, before.depth_allocations);
    assert_eq!(after.pipeline_allocations, before.pipeline_allocations);
    assert_eq!(
        after.shader_module_allocations,
        before.shader_module_allocations
    );
    assert_eq!(after.readback_operations, before.readback_operations);
    assert_eq!(after.synchronization_points, before.synchronization_points);
    let off = gob::address_bytes(20 * 2, 14, W * 2, 1);
    let padding = (0..initial.len())
        .find(|n| !(0..H).any(|y| (0..W * 2).any(|x| gob::address_bytes(x, y, W * 2, 1) == *n)))
        .unwrap();
    d.write_backing(b, (Z + off) as u64, &[0x21], &mut mem(&mut data))
        .unwrap();
    d.write_backing(b, (Z + off + 1) as u64, &[0x43], &mut mem(&mut data))
        .unwrap();
    d.write_backing(b, (Z + padding) as u64, &[0xe3], &mut mem(&mut data))
        .unwrap();
    let draws = after.draw_commands;
    let unchanged = data.clone();
    assert!(d.publish(c, &mut FailAt(&mut data, Z as u64)).is_err());
    assert_eq!(
        data, unchanged,
        "failed depth write leaves all guest RAM intact"
    );
    assert_eq!(d.graphics_counters().depth_write_bytes, 0);
    assert!(d.has_pending_output());
    d.publish(c, &mut mem(&mut data)).unwrap();
    assert_eq!(&data[Z + off..Z + off + 2], &[0x21, 0x43]);
    assert_eq!(data[Z + padding], 0xe3);
    let inside = gob::address_bytes(21 * 2, 14, W * 2, 1);
    assert_eq!(&data[Z + inside..Z + inside + 2], &24576u16.to_le_bytes());
    let outside = gob::address_bytes(0, 0, W * 2, 1);
    assert_eq!(
        &data[Z + outside..Z + outside + 2],
        &initial[outside..outside + 2]
    );
    assert_eq!(
        d.graphics_counters().depth_write_bytes,
        initial.len() as u64
    );
    assert_eq!(d.graphics_counters().draw_commands, draws);
    assert!(!d.has_pending_output());
    eprintln!(
        "Z16 warm32 before={before:?} after={after:?} published={:?}",
        d.graphics_counters()
    );
    d.close(c, &mut mem(&mut data)).unwrap();
}

#[test]
#[ignore = "requires real GPU; every Z16 code survives native attachment initialization/export"]
fn driver_z16_all_codes_roundtrip_and_exact_endpoint_equality() {
    let (mut data, mut setup, _) = fixture();
    data.resize(0x100000, 0);
    let width = 32usize;
    let height = 2048usize;
    let color = 0x20000usize;
    let depth = 0x80000usize;
    let initial_color = BG.repeat(width * height);
    data[color..color + initial_color.len()].copy_from_slice(&initial_color);
    let mut tiled = vec![0; 65536 * 2];
    for code in 0..65536usize {
        let x = code % width;
        let y = code / width;
        let off = gob::address_bytes(x * 2, y, width * 2, 0);
        tiled[off..off + 2].copy_from_slice(&(code as u16).to_le_bytes());
    }
    data[depth..depth + tiled.len()].copy_from_slice(&tiled);
    // An entirely clipped guest triangle initializes depth without changing codes.
    for n in 0..3 {
        data[0x3000 + n * 24..0x3004 + n * 24].copy_from_slice(&2f32.to_le_bytes());
    }
    let mut d = GpuDriver::new().unwrap();
    eprintln!("adapter {:?}", d.capabilities().adapter);
    let b = d
        .register_backing(GuestAddress(0), data.len() as u64)
        .unwrap();
    let s = d.create_space().unwrap();
    let c = d.create_channel(s).unwrap();
    for (gpu, offset, size) in [
        (0x10000, 0x1000, 0x2000),
        (0x30000, 0, 0x1000),
        (0x31000, 0, 0x1000),
        (0x40000, 0, 0x1000),
        (0x50000, 0x4000, 0x1000),
        (0x70000, 0x3000, 0x1000),
        (0x80000, 0x6000, 0x1000),
        (0xb0000, 0x10000, 0x2000),
        (0x100000, color as u64, 0x40000),
        (0x200000, depth as u64, 0x20000),
    ] {
        let r = d.reserve_fixed(s, GpuAddress(gpu), size).unwrap();
        d.map(s, r, b, offset).unwrap();
    }
    let end = setup.split_off(setup.len() - 8);
    for (m, v) in [
        (0x2144, 0x400),
        (0x800, 0),
        (0x804, 0x100000),
        (0x808, (width * 4) as u32),
        (0x80c, height as u32),
        (0xa00, 16f32.to_bits()),
        (0xa04, (-1024f32).to_bits()),
        (0xa0c, 16f32.to_bits()),
        (0xa10, 1024f32.to_bits()),
        (0xff4, (width as u32) << 16),
        (0xff8, (height as u32) << 16),
        (0xc00, (width as u32) << 16),
        (0xc04, (height as u32) << 16),
        (0xe04, (width as u32) << 16),
        (0xe08, (height as u32) << 16),
        (0xfe0, 0),
        (0xfe4, 0x200000),
        (0xfe8, 0x13),
        (0xfec, 0),
        (0xff0, 0),
        (0x1228, width as u32),
        (0x122c, height as u32),
        (0x1230, 0x10001),
        (0x179c, 0),
        (0x12cc, 1),
        (0x12e8, 1),
        (0x1538, 1),
        (0x130c, 0x207),
    ] {
        method(&mut setup, m, v);
    }
    setup.extend(end);
    let report = submit(&mut d, c, b, &mut data, &setup);
    assert!(report.fault.is_none(), "{:?}", report.fault);
    assert_eq!(&data[depth..depth + tiled.len()], &tiled);
    assert!(!d.query(report.token).unwrap().guest_visible);
    d.publish(c, &mut mem(&mut data)).unwrap();
    assert_eq!(
        &data[depth..depth + tiled.len()],
        &tiled,
        "all65536 Z16 codes"
    );
    assert_eq!(&data[color..color + initial_color.len()], &initial_color);
    let counters = d.graphics_counters();
    assert_eq!(counters.depth_guest_bytes, tiled.len() as u64 * 2); // initialization and padding-preserving publication
    assert_eq!(counters.depth_upload_bytes, 65536 * 4);
    assert_eq!(counters.depth_write_bytes, 65536 * 2);
    assert_eq!(counters.draw_commands, 1);
    d.retire(report.token).unwrap();
    // Exactly representable endpoints exercise EQUAL/NOT_EQUAL without a D16 tie.
    for z in [0f32, 1.] {
        let code = if z == 0. { 0u16 } else { u16::MAX };
        for y in 0..height {
            for x in 0..width {
                let off = gob::address_bytes(x * 2, y, width * 2, 0);
                tiled[off..off + 2].copy_from_slice(&code.to_le_bytes());
            }
        }
        let vertices: Vec<u8> = (0..3)
            .flat_map(|n| [POS[n][0], POS[n][1], z, 1., UV[n][0], UV[n][1]])
            .flat_map(f32::to_le_bytes)
            .collect();
        d.write_backing(b, 0x3000, &vertices, &mut mem(&mut data))
            .unwrap();
        for equal in [true, false] {
            d.write_backing(b, depth as u64, &tiled, &mut mem(&mut data))
                .unwrap();
            d.write_backing(b, color as u64, &initial_color, &mut mem(&mut data))
                .unwrap();
            let mut words = Vec::new();
            method(&mut words, 0x130c, if equal { 0x202 } else { 0x205 });
            words.extend(draw());
            let r = submit(&mut d, c, b, &mut data, &words);
            assert!(r.fault.is_none(), "{:?}", r.fault);
            d.publish(c, &mut mem(&mut data)).unwrap();
            assert_eq!(&data[depth..depth + tiled.len()], &tiled);
            // At (12,1024), NDC(-0.21875,-0.00048828125) is strictly inside.
            let off = color + (1024 * width + 12) * 4;
            assert_eq!(
                &data[off..off + 4],
                if equal { &[255, 0, 0, 255] } else { &BG }
            );
            assert_eq!(&data[color..color + 4], &BG);
            d.retire(r.token).unwrap();
        }
    }
    eprintln!("Z16 exhaustive counters {:?}", d.graphics_counters());
    d.close(c, &mut mem(&mut data)).unwrap();
}

#[test]
#[ignore = "requires real GPU; pending depth format switches publish old backing before host replacement"]
fn driver_z16_zf32_format_switch_and_fault_prefix() {
    let (mut data, mut setup, _) = fixture();
    let initial = initial_depth();
    data[Z..Z + initial.len()].copy_from_slice(&initial);
    let (mut d, c, _, b, _) = driver();
    depth_setup(&mut setup, 0x207, true);
    assert!(submit(&mut d, c, b, &mut data, &setup).fault.is_none());
    let first = d.graphics_counters();
    let z16 = z16_initial();
    d.write_backing(b, Z as u64, &z16, &mut mem(&mut data))
        .unwrap();
    let mut words = Vec::new();
    method(&mut words, 0xfe8, 0x13);
    method(&mut words, 0xff0, 1024);
    words.extend(draw());
    assert!(submit(&mut d, c, b, &mut data, &words).fault.is_none());
    assert_eq!(
        &data[Z..Z + z16.len()],
        &z16,
        "old ZF32 publication preserves CPU Z16 patch"
    );
    let second = d.graphics_counters();
    assert_eq!(second.depth_allocations, first.depth_allocations + 1);
    assert_eq!(second.coherence_barriers, first.coherence_barriers + 1);
    assert_eq!(second.depth_write_bytes - first.depth_write_bytes, 8192);
    assert_eq!(second.vertex_shader_decodes, first.vertex_shader_decodes);
    assert_eq!(second.shader_decodes, first.shader_decodes);
    assert_eq!(second.shader_binary_bytes, first.shader_binary_bytes);
    d.write_backing(b, Z as u64, &initial, &mut mem(&mut data))
        .unwrap();
    let mut words = Vec::new();
    method(&mut words, 0xfe8, 0xa);
    method(&mut words, 0xff0, 2048);
    words.extend(draw());
    assert!(submit(&mut d, c, b, &mut data, &words).fault.is_none());
    assert_eq!(
        &data[Z..Z + initial.len()],
        &initial,
        "old Z16 publication preserves CPU ZF32 patch"
    );
    let third = d.graphics_counters();
    assert_eq!(third.depth_allocations, second.depth_allocations + 1);
    assert_eq!(third.depth_write_bytes - second.depth_write_bytes, 4096);
    assert_eq!(third.vertex_shader_decodes, first.vertex_shader_decodes);
    assert_eq!(third.shader_decodes, first.shader_decodes);
    let fault = submit(&mut d, c, b, &mut data, &[0x200103fa, 0x14]);
    assert!(fault.fault.is_some());
    assert_eq!(d.graphics_counters().draw_commands, third.draw_commands);
    assert_eq!(d.graphics_counters().submissions, third.submissions);
    assert!(d.has_pending_output());
    d.publish(c, &mut mem(&mut data)).unwrap();
    let off = gob::address(20 * 4, 14, W, 1);
    assert_eq!(&data[Z + off..Z + off + 4], &0.5f32.to_le_bytes());
    assert_eq!(d.graphics_counters().draw_commands, third.draw_commands);
    d.close(c, &mut mem(&mut data)).unwrap();
}
