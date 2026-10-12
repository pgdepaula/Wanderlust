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
