#![cfg(feature = "wgpu-backend")]
mod common;
use stats_alloc::{INSTRUMENTED_SYSTEM, Region, StatsAlloc};
use std::alloc::System;
#[path = "common/scoped_alloc.rs"]
mod measured_alloc;
#[global_allocator]
static SCOPED_ALLOCATOR: measured_alloc::Allocator = measured_alloc::Allocator;
static ALLOCATOR: &StatsAlloc<System> = &INSTRUMENTED_SYSTEM;
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
fn put(data: &mut [u8], offset: usize, words: &[u32]) {
    for (b, w) in data[offset..offset + words.len() * 4]
        .chunks_exact_mut(4)
        .zip(words)
    {
        b.copy_from_slice(&w.to_le_bytes());
    }
}
fn fixture() -> (Vec<u8>, Vec<u32>, Vec<u8>) {
    let desc = TextureImage::decode(TIC_RGBA)
        .unwrap()
        .with_sampler(TextureSampler::decode(TSC_NEAREST).unwrap());
    let (surface, images) = rgba_surface(desc);
    let mut data = vec![0u8; 0x18000];
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
    for n in 0..3 {
        for (c, v) in [POS[n][0], POS[n][1], 0., 1., UV[n][0], UV[n][1]]
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
fn edge(a: [f64; 2], b: [f64; 2], p: [f64; 2]) -> f64 {
    (b[0] - a[0]) * (p[1] - a[1]) - (b[1] - a[1]) * (p[0] - a[0])
}
fn check(pixels: &[u8], image: &[u8], solid: Option<[u8; 4]>, bg: [u8; 4]) {
    let v = POS.map(|p| p.map(f64::from));
    let area = edge(v[0], v[1], v[2]);
    let mut counts = [0, 0];
    for y in 0..29 {
        for x in 0..52 {
            let p = [
                (x as f64 + 0.5) * 2. / 47. - 1.,
                1. - (y as f64 + 0.5) * 2. / 29.,
            ];
            let b = [
                edge(v[1], v[2], p) / area,
                edge(v[2], v[0], p) / area,
                edge(v[0], v[1], p) / area,
            ];
            if x < 47 && b.iter().any(|v| v.abs() < 0.005) {
                continue;
            }
            let inside = x < 47 && b.iter().all(|v| *v >= 0.);
            let expected = if inside {
                if let Some(s) = solid {
                    s
                } else {
                    let u = (0..3).map(|n| b[n] * f64::from(UV[n][0])).sum::<f64>() * 37.;
                    let v = (0..3).map(|n| b[n] * f64::from(UV[n][1])).sum::<f64>() * 19.;
                    if (u - u.round()).abs() < 0.02 || (v - v.round()).abs() < 0.02 {
                        continue;
                    }
                    let i = (v.floor().clamp(0., 18.) as usize * 37
                        + u.floor().clamp(0., 36.) as usize)
                        * 4;
                    image[i..i + 4].try_into().unwrap()
                }
            } else {
                bg
            };
            assert_eq!(
                &pixels[(y * 52 + x) * 4..(y * 52 + x + 1) * 4],
                expected,
                "pixel{x},{y}"
            );
            counts[usize::from(inside)] += 1;
        }
    }
    assert!(counts[0] > 600 && counts[1] > 100);
}

fn header(count: u32) -> [u8; 24] {
    let mut h = [0; 24];
    h[8..12].copy_from_slice(&count.to_le_bytes());
    h[12..16].copy_from_slice(&4u32.to_le_bytes());
    h
}
fn mem(data: &mut [u8]) -> SliceMemoryMut<'_> {
    SliceMemoryMut {
        base: GuestAddress(0),
        bytes: data,
    }
}
fn setup_driver() -> (
    GpuDriver,
    AddressSpaceId,
    ChannelId,
    BackingId,
    Vec<(ReservationId, MappingId)>,
) {
    let mut d = GpuDriver::new().unwrap();
    eprintln!("adapter {:?}", d.capabilities().adapter);
    let backing = d.register_backing(GuestAddress(0), 0x18000).unwrap();
    let space = d.create_space().unwrap();
    let channel = d.create_channel(space).unwrap();
    let mut maps = Vec::new();
    for (gpu, offset, size) in [
        (0x10000, 0x1000, 0x2000),
        (0x30000, 0, 0x1000),
        (0x31000, 0, 0x1000),
        (0x40000, 0, 0x1000),
        (0x50000, 0x4000, 0x1000),
        (0x70000, 0x3000, 0x1000),
        (0x80000, 0x6000, 0x1000),
        (0x90000, 0x8000, 0x2000),
        (0xb0000, 0x10000, 0x4000),
    ] {
        let r = d.reserve_fixed(space, GpuAddress(gpu), size).unwrap();
        let m = d.map(space, r, backing, offset).unwrap();
        maps.push((r, m));
    }
    (d, space, channel, backing, maps)
}

#[test]
#[ignore = "requires actual GPU; externally written CPU data and checked cache notification"]
fn external_cpu_writes_invalidate_without_recopying_guest_memory() {
    let (mut data, words, image) = fixture();
    let (mut d, _, c, b, _) = setup_driver();
    put(&mut data, 0x10000, &words);
    d.notify_cpu_write(b, 0x10000, words.len() * 4).unwrap();
    let h = header(1);
    let e = entry(0xb0000, words.len());
    let r = d
        .submit(c, KICKOFF_PB, &h, Some(&e), &mut mem(&mut data))
        .unwrap();
    assert!(r.fault.is_none());
    let untouched = data.clone();
    let before = (
        d.counters.cpu_write_notifications,
        d.counters.cpu_notified_bytes,
    );
    assert!(d.notify_cpu_write(b, 0x1000, 4).is_err());
    assert_eq!(
        (
            d.counters.cpu_write_notifications,
            d.counters.cpu_notified_bytes
        ),
        before
    );
    assert_eq!(data, untouched);
    d.publish(c, &mut mem(&mut data)).unwrap();
    check(&data[RT..RT + SIZE], &image, None, BG);
    d.retire(r.token).unwrap();

    // Caller establishes publication BEFORE touching RAM. AOT/runtime would
    // perform this write itself; notification only advances canonical history.
    let desc = TextureImage::decode(TIC_RGBA)
        .unwrap()
        .with_sampler(TextureSampler::decode(TSC_NEAREST).unwrap());
    let (surface, _) = rgba_surface(desc);
    assert_eq!(surface.len(), 6144);
    let color = [77, 137, 199, 255];
    for pixel in data[0x1000..0x1000 + surface.len()].chunks_exact_mut(4) {
        pixel.copy_from_slice(&color);
    }
    let after_cpu_write = data.clone();
    d.notify_cpu_write(b, 0x1000, surface.len()).unwrap();
    assert_eq!(data, after_cpu_write); // The notification itself owns no RAM.
    let draw = [0x20010586, 4, 0x200105f7, 0, 0x200105f8, 3, 0x20010585, 0];
    put(&mut data, 0x10000, &draw);
    d.notify_cpu_write(b, 0x10000, draw.len() * 4).unwrap();
    let e = entry(0xb0000, draw.len());
    let before = d.graphics_counters();
    let r = d
        .submit(c, KICKOFF_PB, &h, Some(&e), &mut mem(&mut data))
        .unwrap();
    assert!(r.fault.is_none());
    assert_eq!(r.progress.draws_submitted, 1);
    let after = d.graphics_counters();
    assert_eq!(after.shader_decodes, before.shader_decodes);
    assert_eq!(after.vertex_shader_decodes, before.vertex_shader_decodes);
    assert_eq!(
        after.shader_module_allocations,
        before.shader_module_allocations
    );
    assert_eq!(after.pipeline_allocations, before.pipeline_allocations);
    assert_eq!(
        after.guest_texture_bytes - before.guest_texture_bytes,
        surface.len() as u64
    );
    assert_eq!(after.upload_bytes - before.upload_bytes, 37 * 19 * 4);
    d.publish(c, &mut mem(&mut data)).unwrap();
    check(&data[RT..RT + SIZE], &image, Some(color), BG);
    d.retire(r.token).unwrap();

    // Inject exhausted instrumentation before a valid notification overlapping
    // cached shader code. Rejection must precede dirty-history mutation.
    let bytes = d.counters.cpu_notified_bytes;
    d.counters.cpu_notified_bytes = u64::MAX;
    assert!(d.notify_cpu_write(b, 0x4000, 4).is_err());
    d.counters.cpu_notified_bytes = bytes;
    for (offset, length) in [(0u64, 0usize), (0x18000, 1), (u64::MAX, 4)] {
        assert!(d.notify_cpu_write(b, offset, length).is_err());
    }
    let before = d.graphics_counters();
    let r = d
        .submit(c, KICKOFF_PB, &h, Some(&e), &mut mem(&mut data))
        .unwrap();
    assert!(r.fault.is_none());
    let after = d.graphics_counters();
    assert_eq!(after.shader_binary_bytes, before.shader_binary_bytes);
    assert_eq!(after.vertex_shader_bytes, before.vertex_shader_bytes);
    assert_eq!(after.upload_bytes, before.upload_bytes);
    assert_eq!(after.pipeline_allocations, before.pipeline_allocations);
    assert_eq!(r.progress.draws_submitted, 1);
    assert_eq!(after.draw_commands - before.draw_commands, 1);
    assert_eq!(d.counters.cpu_write_notifications, 3);
    assert_eq!(d.counters.cpu_notified_bytes, 6812);
    d.publish(c, &mut mem(&mut data)).unwrap();
    check(&data[RT..RT + SIZE], &image, Some(color), BG);
    eprintln!(
        "external CPU write driver={:?}, graphics={:?}",
        d.counters,
        d.graphics_counters()
    );
    d.close(c, &mut mem(&mut data)).unwrap();
}

#[test]
#[ignore = "requires actual wgpu device; external notification bounds, lifetime and allocation measurements"]
fn external_cpu_notification_bounds_lifetime_and_bookkeeping() {
    let mut d = GpuDriver::new().unwrap();
    let b = d.register_backing(GuestAddress(0), PAGE_SIZE).unwrap();
    let mut foreign = MemoryManager::default();
    let f = foreign
        .register_backing(GuestAddress(0), PAGE_SIZE)
        .unwrap();
    assert!(d.notify_cpu_write(f, 0, 4).is_err());
    for (offset, length) in [(0u64, 0usize), (PAGE_SIZE, 1), (u64::MAX, 4)] {
        assert!(d.notify_cpu_write(b, offset, length).is_err());
    }
    d.notify_cpu_write(b, PAGE_SIZE - 4, 4).unwrap();
    let region = Region::new(ALLOCATOR);
    let caller = measured_alloc::Region::new();
    let start = std::time::Instant::now();
    for _ in 0..1000 {
        d.notify_cpu_write(b, 0, 4).unwrap();
    }
    let elapsed = start.elapsed();
    let caller = caller.finish();
    let stats = region.change();
    assert_eq!(d.counters.cpu_write_notifications, 1001);
    assert_eq!(d.counters.cpu_notified_bytes, 4004);
    assert_eq!(d.graphics_counters().submissions, 0);
    assert_eq!(d.graphics_counters().upload_bytes, 0);
    assert_eq!(d.graphics_counters().readback_bytes, 0);
    eprintln!(
        "1000 CPU notifications: {elapsed:?}, allocations {}, reallocations {}",
        stats.allocations, stats.reallocations
    );
    eprintln!(
        "caller-thread allocations {}, reallocations {}; process-wide values above include asynchronous backend threads",
        caller.allocations, caller.reallocations
    );
    assert_eq!(caller.allocations, 0);
    assert_eq!(caller.reallocations, 0);
    // The registry owns identities, not bytes: a huge canonical range tests
    // overflow without allocating an enormous synthetic guest memory buffer.
    let huge = d
        .register_backing(GuestAddress(PAGE_SIZE), 1u64 << 63)
        .unwrap();
    d.notify_cpu_write(huge, 0, 1usize << 63).unwrap();
    let before = (
        d.counters.cpu_write_notifications,
        d.counters.cpu_notified_bytes,
    );
    assert!(d.notify_cpu_write(huge, 0, 1usize << 63).is_err());
    assert_eq!(
        (
            d.counters.cpu_write_notifications,
            d.counters.cpu_notified_bytes
        ),
        before
    );
    d.notify_cpu_write(b, 0, 4).unwrap();
    d.release_backing(huge).unwrap();
    assert!(d.notify_cpu_write(huge, 0, 4).is_err());
    let count = d.counters.cpu_write_notifications;
    d.counters.cpu_write_notifications = u64::MAX;
    assert!(d.notify_cpu_write(b, 0, 4).is_err());
    d.counters.cpu_write_notifications = count;
    d.release_backing(b).unwrap();
    assert!(d.notify_cpu_write(b, 0, 4).is_err());
}
#[test]
#[ignore = "requires actual wgpu adapter and pixel readback"]
fn driver_completion_publication_and_mapping_lifetime() {
    let (mut data, words, image) = fixture();
    let (mut d, space, c, b, maps) = setup_driver();
    let before_configuration = d.graphics_counters();
    d.set_completion_timeout(c, 614).unwrap();
    assert_eq!(d.graphics_counters(), before_configuration);
    put(&mut data, 0x10000, &words);
    let entries = entry(0xb0000, words.len());
    let h = header(1);
    let first = d
        .submit(c, KICKOFF_PB, &h, Some(&entries), &mut mem(&mut data))
        .unwrap();
    assert!(first.fault.is_none());
    assert_eq!(first.progress.draws_submitted, 1);
    assert!(d.has_pending_output());
    // Acceptance/query make no completion promise. Wait is real queue completion,
    // and must NOT publish any framebuffer bytes or add a readback submission.
    let before = d.graphics_counters();
    let _ = d.query(first.token).unwrap();
    assert!(d.wait(first.token).unwrap().host_completed);
    assert!(!d.query(first.token).unwrap().guest_visible);
    assert_eq!(
        d.graphics_counters().readback_operations,
        before.readback_operations
    );
    assert_eq!(d.graphics_counters().submissions, before.submissions);
    assert!(data[RT..RT + SIZE].chunks_exact(4).all(|p| p == BG));
    assert!(d.retire(first.token).is_err());
    assert!(d.unmap(space, maps[7].1).is_err());
    let epoch = d.mapping_epoch(space).unwrap();
    let error = d.map_subrange(space, maps[7].0, b, 0, 0, 4096).unwrap_err();
    assert!(error.0.contains("publish pending output"));
    let error = d
        .apply_map_changes(
            space,
            &[MapChange {
                reservation: maps[7].0,
                address: GpuAddress(0x90000),
                size: 4096,
                backing: None,
            }],
        )
        .unwrap_err();
    assert!(error.0.contains("publish pending output"));
    assert_eq!(d.mapping_epoch(space).unwrap(), epoch);
    assert!(d.release_backing(b).is_err());
    let other = d.create_channel(space).unwrap();
    assert!(
        d.submit(other, KICKOFF_PB, &h, Some(&entries), &mut mem(&mut data))
            .is_err()
    );
    // Canonical CPU patch after GPU execution survives checked publication.
    for n in 0..32 {
        d.write_backing(
            b,
            RT as u64 + n * 4,
            &[81, 82, 83, 255],
            &mut mem(&mut data),
        )
        .unwrap();
    }
    let untouched = data[RT + 128..RT + 132].to_vec();
    assert!(
        d.write_backing(b, RT as u64 + 128, &[99; 4], &mut mem(&mut data))
            .is_err()
    );
    assert_eq!(&data[RT + 128..RT + 132], untouched);
    let frame = d.publish(c, &mut mem(&mut data)).unwrap().unwrap();
    let mut pixels = frame.rgba.clone();
    assert_eq!(&data[RT..RT + 4], &[81, 82, 83, 255]);
    assert!(
        data[RT..RT + 128]
            .chunks_exact(4)
            .all(|p| p == [81, 82, 83, 255])
    );
    for pixel in pixels[..128].chunks_exact_mut(4) {
        pixel.copy_from_slice(&BG);
    }
    check(&pixels, &image, None, BG);
    assert!(d.query(first.token).unwrap().guest_visible);
    d.retire(first.token).unwrap();
    assert!(d.query(first.token).is_err());
    // Warm draw changes only the index buffer, retaining compiled shader/pipeline.
    let mut draw = Vec::new();
    for (m, v) in [(0x1618, 4), (0x17dc, 0), (0x17e0, 3), (0x1614, 0)] {
        method(&mut draw, m, v);
    }
    let mut bytes = Vec::new();
    for w in &draw {
        bytes.extend(w.to_le_bytes());
    }
    d.write_backing(b, 0x10000, &bytes, &mut mem(&mut data))
        .unwrap();
    let e = entry(0xb0000, draw.len());
    // CPU patches require one correct target refresh; measure unchanged draws
    // only after that refresh has actually been submitted.
    let prime = d
        .submit(c, KICKOFF_PB, &h, Some(&e), &mut mem(&mut data))
        .unwrap();
    assert!(prime.fault.is_none());
    let before = d.graphics_counters();
    let mut last = None;
    let warm_region = Region::new(ALLOCATOR);
    let warm_start = std::time::Instant::now();
    for _ in 0..32 {
        let r = d
            .submit(c, KICKOFF_PB, &h, Some(&e), &mut mem(&mut data))
            .unwrap();
        assert!(r.fault.is_none());
        last = Some(r.token);
    }
    let warm_time = warm_start.elapsed();
    let warm_stats = warm_region.change();
    eprintln!(
        "32 warm driver draws: {warm_time:?}, allocations {}, reallocations {}",
        warm_stats.allocations, warm_stats.reallocations
    );
    let warm = d.graphics_counters();
    assert_eq!(warm.readback_operations, before.readback_operations);
    assert_eq!(warm.synchronization_points, before.synchronization_points);
    assert_eq!(warm.upload_bytes, before.upload_bytes);
    assert_eq!(warm.pipeline_allocations, before.pipeline_allocations);
    assert_eq!(warm.shader_binary_bytes, before.shader_binary_bytes);
    d.publish(c, &mut mem(&mut data)).unwrap();
    assert!(d.wait(last.unwrap()).unwrap().host_completed);
    // Remap index backing to a distinct canonical subregion with degenerate indices.
    d.unmap(space, maps[6].1).unwrap();
    let replacement = d.map(space, maps[6].0, b, 0x7000).unwrap();
    d.write_backing(b, RT as u64, &BG.repeat(SIZE / 4), &mut mem(&mut data))
        .unwrap();
    let r = d
        .submit(c, KICKOFF_PB, &h, Some(&e), &mut mem(&mut data))
        .unwrap();
    assert!(r.fault.is_none());
    let frame = d.publish(c, &mut mem(&mut data)).unwrap().unwrap();
    assert!(frame.rgba.chunks_exact(4).all(|p| p == BG));
    d.close(c, &mut mem(&mut data)).unwrap();
    assert!(d.query(r.token).is_err());
    assert!(d.set_completion_timeout(c, 614).is_err());
    assert!(
        d.submit(c, KICKOFF_PB, &h, Some(&e), &mut mem(&mut data))
            .is_err()
    );
    // Independent channel did not inherit B197 binding or registers.
    let r = d
        .submit(other, KICKOFF_PB, &h, Some(&e), &mut mem(&mut data))
        .unwrap();
    assert!(r.fault.is_some());
    assert_eq!(r.progress.draws_submitted, 0);
    d.close(other, &mut mem(&mut data)).unwrap();
    for (n, (r, m)) in maps.into_iter().enumerate() {
        d.unmap(space, if n == 6 { replacement } else { m })
            .unwrap();
        d.free_reservation(space, r).unwrap();
    }
    d.destroy_space(space).unwrap();
    d.release_backing(b).unwrap();
    eprintln!(
        "driver {:?}, graphics {:?}, warm upload delta {} readbacks {}",
        d.counters,
        d.graphics_counters(),
        warm.upload_bytes - before.upload_bytes,
        warm.readback_operations - before.readback_operations
    );
}
struct FailWrite<'a>(&'a mut [u8]);
impl GuestMemory for FailWrite<'_> {
    fn read(&self, a: GuestAddress, n: usize) -> wl_gpu::Result<&[u8]> {
        let start = usize::try_from(a.0).map_err(|_| wl_gpu::Error("offset".into()))?;
        let end = start
            .checked_add(n)
            .ok_or_else(|| wl_gpu::Error("overflow".into()))?;
        self.0
            .get(start..end)
            .ok_or_else(|| wl_gpu::Error("unmapped".into()))
    }
}
impl GuestMemoryWrite for FailWrite<'_> {
    fn validate_write(&self, a: GuestAddress, n: usize) -> wl_gpu::Result<()> {
        self.read(a, n).map(|_| ())
    }
    fn write(&mut self, _: GuestAddress, _: &[u8]) -> wl_gpu::Result<()> {
        Err(wl_gpu::Error("injected atomic write failure".into()))
    }
}
#[test]
#[ignore = "requires actual wgpu adapter and pixel readback"]
fn driver_fault_prefix_close_and_publication_retry_without_replay() {
    let (mut data, mut words, image) = fixture();
    let (mut d, space, c, backing, _maps) = setup_driver();
    d.set_completion_timeout(c, 614).unwrap();
    // Earlier accepted draw survives malformed suffix. Fault prevents request replay.
    words.push(0xe0000000);
    put(&mut data, 0x10000, &words);
    let e = entry(0xb0000, words.len());
    let h = header(1);
    // Truncated request is rejected before admission/channel mutation.
    assert!(
        d.submit(c, KICKOFF_PB, &h[..23], Some(&e), &mut mem(&mut data))
            .is_err()
    );
    let r = d
        .submit(c, KICKOFF_PB, &h, Some(&e), &mut mem(&mut data))
        .unwrap();
    assert!(r.fault.is_some());
    assert_eq!(r.progress.draws_submitted, 1);
    assert!(d.wait(r.token).unwrap().host_completed);
    assert!(
        d.submit(c, KICKOFF_PB, &h, Some(&e), &mut mem(&mut data))
            .is_err()
    );
    let before = data[RT..RT + SIZE].to_vec();
    let draws = d.graphics_counters().gpu_target_writes;
    assert!(d.close(c, &mut FailWrite(&mut data)).is_err());
    assert_eq!(&data[RT..RT + SIZE], before);
    assert!(d.has_pending_output());
    assert!(!d.query(r.token).unwrap().guest_visible);
    assert!(d.destroy_space(space).is_err());
    d.close(c, &mut mem(&mut data)).unwrap();
    check(&data[RT..RT + SIZE], &image, None, BG);
    assert_eq!(d.graphics_counters().gpu_target_writes, draws);
    assert!(d.query(r.token).is_err());
    let fresh = d.create_channel(space).unwrap();
    let (_, words, _) = fixture();
    let bytes: Vec<u8> = words.iter().flat_map(|w| w.to_le_bytes()).collect();
    d.write_backing(backing, 0x10000, &bytes, &mut mem(&mut data))
        .unwrap();
    let e = entry(0xb0000, words.len());
    let good = d
        .submit(fresh, KICKOFF_PB, &h, Some(&e), &mut mem(&mut data))
        .unwrap();
    assert!(good.fault.is_none());
    d.close(fresh, &mut mem(&mut data)).unwrap();
}

#[test]
#[ignore = "requires actual wgpu device; metadata/allocation isolation"]
fn driver_nop_bookkeeping_capacity_and_independent_packet_state() {
    let (mut d, space, a, backing, _) = setup_driver();
    let b = d.create_channel(space).unwrap();
    // Finite timeout configuration must preserve the allocation-free NOP path.
    d.set_completion_timeout(a, 614).unwrap();
    d.set_completion_timeout(b, 0).unwrap();
    let h = header(1);
    let e = entry(0xb0000, 1);
    let mut data = vec![0; 0x18000];
    let region = Region::new(ALLOCATOR);
    let caller = measured_alloc::Region::new();
    let start = std::time::Instant::now();
    for _ in 0..1000 {
        let r = d
            .submit(a, KICKOFF_PB, &h, Some(&e), &mut mem(&mut data))
            .unwrap();
        assert!(r.fault.is_none());
        d.retire(r.token).unwrap();
    }
    let elapsed = start.elapsed();
    let caller = caller.finish();
    let stats = region.change();
    eprintln!(
        "1000 NOP channel submit+retire: {elapsed:?}, allocations {}, reallocations {}, borrowed {}, queue submissions {}",
        stats.allocations,
        stats.reallocations,
        d.graphics_counters().gpfifo_borrowed_bytes,
        d.graphics_counters().submissions
    );
    eprintln!(
        "caller-thread allocations {}, reallocations {}; process-wide values above include asynchronous backend threads",
        caller.allocations, caller.reallocations
    );
    assert_eq!(caller.allocations, 0);
    assert_eq!(caller.reallocations, 0);
    assert_eq!(d.graphics_counters().submissions, 0);
    let mut tokens = Vec::new();
    for _ in 0..MAX_COMPLETIONS {
        tokens.push(
            d.submit(a, KICKOFF_PB, &h, Some(&e), &mut mem(&mut data))
                .unwrap()
                .token,
        );
    }
    let consumed = d.graphics_counters().gpfifo_command_bytes;
    assert!(
        d.submit(a, KICKOFF_PB, &h, Some(&e), &mut mem(&mut data))
            .is_err()
    );
    assert_eq!(d.graphics_counters().gpfifo_command_bytes, consumed);
    d.retire(tokens[0]).unwrap();
    let replacement = d
        .submit(a, KICKOFF_PB, &h, Some(&e), &mut mem(&mut data))
        .unwrap()
        .token;
    d.retire(replacement).unwrap();
    for t in &tokens[1..] {
        d.retire(*t).unwrap();
    }
    // A's pending method argument survives another independent channel's NOP.
    d.write_backing(
        backing,
        0x10000,
        &0x20010000u32.to_le_bytes(),
        &mut mem(&mut data),
    )
    .unwrap();
    let r = d
        .submit(a, KICKOFF_PB, &h, Some(&e), &mut mem(&mut data))
        .unwrap();
    assert!(r.fault.is_none());
    d.retire(r.token).unwrap();
    d.write_backing(backing, 0x10000, &0u32.to_le_bytes(), &mut mem(&mut data))
        .unwrap();
    let r = d
        .submit(b, KICKOFF_PB, &h, Some(&e), &mut mem(&mut data))
        .unwrap();
    assert!(r.fault.is_none());
    d.retire(r.token).unwrap();
    d.write_backing(
        backing,
        0x10000,
        &0xb197u32.to_le_bytes(),
        &mut mem(&mut data),
    )
    .unwrap();
    let r = d
        .submit(a, KICKOFF_PB, &h, Some(&e), &mut mem(&mut data))
        .unwrap();
    assert!(r.fault.is_none());
    d.retire(r.token).unwrap();
    // A is now bound; B is not. An ordinary engine write faults only B.
    let bytes = [0x20010582u32, 0]
        .into_iter()
        .flat_map(u32::to_le_bytes)
        .collect::<Vec<_>>();
    d.write_backing(backing, 0x10000, &bytes, &mut mem(&mut data))
        .unwrap();
    let e = entry(0xb0000, 2);
    let r = d
        .submit(a, KICKOFF_PB, &h, Some(&e), &mut mem(&mut data))
        .unwrap();
    assert!(r.fault.is_none());
    let r = d
        .submit(b, KICKOFF_PB, &h, Some(&e), &mut mem(&mut data))
        .unwrap();
    assert!(r.fault.is_some());
    assert!(
        d.submit(b, KICKOFF_PB, &h, Some(&e), &mut mem(&mut data))
            .is_err()
    );
    d.close(b, &mut mem(&mut data)).unwrap();
    d.close(a, &mut mem(&mut data)).unwrap();
}
#[test]
#[ignore = "requires actual GPU; channel-owned semaphore publication retains the real image without another readback"]
fn semaphore_publication_reaches_the_existing_driver_frame_boundary_once() {
    let (mut data, mut words, image) = fixture();
    let (mut d, space, c, b, _) = setup_driver();
    let reserve = d.reserve_fixed(space, GpuAddress(0xe0000), 0x1000).unwrap();
    d.map(space, reserve, b, 0xf000).unwrap();
    words.extend([0x20040004, 0, 0xe0000, 77, 0x01000002]);
    put(&mut data, 0x10000, &words);
    let r = d
        .submit(
            c,
            KICKOFF_PB,
            &header(1),
            Some(&entry(0xb0000, words.len())),
            &mut mem(&mut data),
        )
        .unwrap();
    assert!(r.fault.is_none());
    assert_eq!(r.progress.draws_submitted, 1);
    assert_eq!(&data[0xf000..0xf004], &77u32.to_le_bytes());
    check(&data[RT..RT + SIZE], &image, None, BG);
    let before = d.graphics_counters().readback_operations;
    assert_eq!(before, 1);
    let other = d.create_channel(space).unwrap();
    assert!(d.publish(other, &mut mem(&mut data)).unwrap().is_none());
    let frame = d.publish(c, &mut mem(&mut data)).unwrap().unwrap();
    check(&frame.rgba, &image, None, BG);
    assert!(d.publish(c, &mut mem(&mut data)).unwrap().is_none());
    assert_eq!(d.graphics_counters().readback_operations, before);
    assert!(d.query(r.token).unwrap().guest_visible);
}
#[test]
#[ignore = "requires actual GPU; sixteen-byte front-end release reports time without claiming completion or reading back each draw"]
fn long_frontend_release_keeps_gpu_output_deferred_and_reports_real_monotonic_time() {
    let began = std::time::Instant::now();
    let (mut data, mut words, image) = fixture();
    let (mut d, space, c, b, _) = setup_driver();
    let reserve = d.reserve_fixed(space, GpuAddress(0xe0000), 0x1000).unwrap();
    d.map(space, reserve, b, 0xf000).unwrap();
    data[0xf000..0xf020].fill(0xa5);
    words.extend([0x20040004, 0, 0xe0000, 77, 0x00100002]);
    put(&mut data, 0x10000, &words);
    let r = d
        .submit(
            c,
            KICKOFF_PB,
            &header(1),
            Some(&entry(0xb0000, words.len())),
            &mut mem(&mut data),
        )
        .unwrap();
    assert!(r.fault.is_none());
    assert!(d.has_pending_output());
    assert!(!d.query(r.token).unwrap().guest_visible);
    assert_eq!(d.graphics_counters().readback_operations, 0);
    assert_eq!(&data[0xf000..0xf004], &77u32.to_le_bytes());
    assert_eq!(&data[0xf004..0xf008], &[0; 4]);
    assert_eq!(&data[0xf010..0xf020], &[0xa5; 16]);
    let first = u64::from_le_bytes(data[0xf008..0xf010].try_into().unwrap());
    assert!(first > 0);
    assert!(u128::from(first) <= began.elapsed().as_nanos() * 384 / 625);
    let words = [0x20010006u32, 88, 0x20010007, 0x00100002];
    let bytes: Vec<_> = words.iter().flat_map(|v| v.to_le_bytes()).collect();
    d.write_backing(b, 0x10000, &bytes, &mut mem(&mut data))
        .unwrap();
    let r = d
        .submit(
            c,
            KICKOFF_PB,
            &header(1),
            Some(&entry(0xb0000, 4)),
            &mut mem(&mut data),
        )
        .unwrap();
    assert!(r.fault.is_none());
    assert_eq!(r.progress.draws_submitted, 0);
    assert!(d.has_pending_output());
    assert_eq!(d.graphics_counters().readback_operations, 0);
    assert_eq!(&data[0xf000..0xf004], &88u32.to_le_bytes());
    let second = u64::from_le_bytes(data[0xf008..0xf010].try_into().unwrap());
    assert!(second >= first);
    let frame = d.publish(c, &mut mem(&mut data)).unwrap().unwrap();
    check(&frame.rgba, &image, None, BG);
    assert_eq!(d.graphics_counters().readback_operations, 1);
}

#[test]
#[ignore = "requires actual GPU; tile cache flush publishes indexed pixels and retains one driver-visible image"]
fn tile_cache_operations_publish_real_pixels_and_preserve_driver_capture() {
    for method_offset in [0x0f7c, 0x0f80] {
        let (mut data, mut words, image) = fixture();
        let (mut d, _, c, _, _) = setup_driver();
        method(&mut words, method_offset, 0);
        put(&mut data, 0x10000, &words);
        let r = d
            .submit(
                c,
                KICKOFF_PB,
                &header(1),
                Some(&entry(0xb0000, words.len())),
                &mut mem(&mut data),
            )
            .unwrap();
        assert!(r.fault.is_none(), "{:?}", r.fault);
        assert_eq!(r.progress.draws_submitted, 1);
        assert!(!d.has_pending_output());
        check(&data[RT..RT + SIZE], &image, None, BG);
        let counters = d.graphics_counters();
        assert_eq!(counters.readback_operations, 1);
        assert_eq!(counters.coherence_barriers, 1);
        // Already-published bytes must still reach the runtime capture boundary.
        let frame = d.publish(c, &mut mem(&mut data)).unwrap().unwrap();
        check(&frame.rgba, &image, None, BG);
        assert!(d.publish(c, &mut mem(&mut data)).unwrap().is_none());
        assert_eq!(d.graphics_counters().readback_operations, 1);
        assert_eq!(d.graphics_counters().draw_commands, 1);
        d.retire(r.token).unwrap();
    }
}

#[test]
#[ignore = "requires actual GPU; B1C0 selectors stay isolated from real B197 graphics and unsupported dispatch faults preserve accepted draws"]
fn compute_resource_selectors_preserve_graphics_and_reject_dispatch() {
    let (mut data, graphics, image) = fixture();
    let (mut d, _, c, _, _) = setup_driver();
    // Subchannel one is B1C0. Register selectors persist without fetching memory
    // or being confused with the B197 pools on subchannel zero.
    let mut words = vec![0x20012000, 0xb1c0];
    for (method, value) in [
        (0x155c, 0),
        (0x1560, 0x31020),
        (0x1564, 17),
        (0x1574, 0),
        (0x1578, 0x30000),
        (0x157c, 33),
        (0x790, 255),
        (0x794, u32::MAX),
        (0x2e4, 255),
        (0x2e8, u32::MAX),
        (0x2ec, 511),
        (0x2f0, 0),
        (0x2f4, 0),
        (0x2f8, 1),
    ] {
        words.extend([0x20012000 | (method / 4), value]);
    }
    words.extend(graphics);
    // The unsupported launch must never be mistaken for a successful compute job.
    words.extend([0x200120ad, 1]);
    put(&mut data, 0x10000, &words);
    let result = d
        .submit(
            c,
            KICKOFF_PB,
            &header(1),
            Some(&entry(0xb0000, words.len())),
            &mut mem(&mut data),
        )
        .unwrap();
    assert_eq!(result.progress.draws_submitted, 1);
    let fault = result.fault.as_ref().expect("unsupported compute launch");
    assert!(fault.to_string().contains("method=0x02b4"), "{fault}");
    assert!(d.has_pending_output());
    assert_eq!(d.graphics_counters().readback_operations, 0);
    assert!(data[RT..RT + SIZE].chunks_exact(4).all(|p| p == BG));
    let frame = d.publish(c, &mut mem(&mut data)).unwrap().unwrap();
    check(&frame.rgba, &image, None, BG);
    assert_eq!(d.graphics_counters().draw_commands, 1);
    assert_eq!(d.graphics_counters().readback_operations, 1);
}

#[test]
#[ignore = "requires actual GPU; graphics scratch selectors cannot create an implicit fetch or change scratch-free shader pixels"]
fn graphics_scratch_configuration_preserves_real_pixels_and_warm_pipeline() {
    let (mut data, mut words, image) = fixture();
    let (mut d, _, c, b, _) = setup_driver();
    for (m, v) in [
        (0x790, 255),
        (0x794, u32::MAX),
        (0x798, 63),
        (0x79c, u32::MAX),
        (0x7a0, 0x03ffffff),
    ] {
        method(&mut words, m, v);
    }
    put(&mut data, 0x10000, &words);
    let first = d
        .submit(
            c,
            KICKOFF_PB,
            &header(1),
            Some(&entry(0xb0000, words.len())),
            &mut mem(&mut data),
        )
        .unwrap();
    assert!(first.fault.is_none(), "{:?}", first.fault);
    assert_eq!(first.progress.draws_submitted, 1);
    check(
        &d.publish(c, &mut mem(&mut data)).unwrap().unwrap().rgba,
        &image,
        None,
        BG,
    );
    d.retire(first.token).unwrap();
    let before = d.graphics_counters();
    let mut words = Vec::new();
    for m in [0x790, 0x794, 0x798, 0x79c, 0x7a0] {
        method(&mut words, m, 0);
    }
    words.extend([0x20010586, 4, 0x200105f7, 0, 0x200105f8, 3, 0x20010585, 0]);
    let bytes: Vec<_> = words.iter().flat_map(|w| w.to_le_bytes()).collect();
    d.write_backing(b, 0x10000, &bytes, &mut mem(&mut data))
        .unwrap();
    let second = d
        .submit(
            c,
            KICKOFF_PB,
            &header(1),
            Some(&entry(0xb0000, words.len())),
            &mut mem(&mut data),
        )
        .unwrap();
    assert!(second.fault.is_none(), "{:?}", second.fault);
    assert_eq!(second.progress.draws_submitted, 1);
    let after = d.graphics_counters();
    assert_eq!(after.pipeline_allocations, before.pipeline_allocations);
    assert_eq!(
        after.shader_module_allocations,
        before.shader_module_allocations
    );
    assert_eq!(after.shader_binary_bytes, before.shader_binary_bytes);
    check(
        &d.publish(c, &mut mem(&mut data)).unwrap().unwrap().rgba,
        &image,
        None,
        BG,
    );
}

#[test]
#[ignore = "requires actual GPU; B197 payload/timestamp reports observe prior real draws and preserve checked guest visibility"]
fn graphics_engine_reports_follow_real_pixels_and_write_exact_records() {
    for control in [0x1000f010, 0x0000f012] {
        let (mut data, mut words, image) = fixture();
        let (mut d, space, c, b, _) = setup_driver();
        let reserve = d.reserve_fixed(space, GpuAddress(0xe0000), 0x1000).unwrap();
        d.map(space, reserve, b, 0xf000).unwrap();
        data[0xf000..0xf020].fill(0xa5);
        // One incrementing literal B197 packet: A/B/C/D, unlike B06F's 0x10.
        words.extend([0x200406c0, 0, 0xe0000, 0x12345678, control]);
        put(&mut data, 0x10000, &words);
        let r = d
            .submit(
                c,
                KICKOFF_PB,
                &header(1),
                Some(&entry(0xb0000, words.len())),
                &mut mem(&mut data),
            )
            .unwrap();
        assert!(r.fault.is_none(), "{:?}", r.fault);
        assert_eq!(r.progress.draws_submitted, 1);
        assert!(!d.has_pending_output());
        assert_eq!(&data[0xf000..0xf004], &0x12345678u32.to_le_bytes());
        if control & 3 == 2 {
            assert_eq!(&data[0xf004..0xf008], &[0; 4]);
            assert!(u64::from_le_bytes(data[0xf008..0xf010].try_into().unwrap()) > 0);
            assert_eq!(&data[0xf010..0xf020], &[0xa5; 16]);
        } else {
            assert_eq!(&data[0xf004..0xf020], &[0xa5; 28]);
        }
        check(&data[RT..RT + SIZE], &image, None, BG);
        let frame = d.publish(c, &mut mem(&mut data)).unwrap().unwrap();
        check(&frame.rgba, &image, None, BG);
        assert_eq!(d.graphics_counters().readback_operations, 1);
        assert_eq!(d.graphics_counters().draw_commands, 1);
    }
}

#[test]
#[ignore = "requires actual GPU; inactive CT mappings must not block or recompile a real single-output guest draw"]
fn inactive_color_target_selectors_preserve_pixels_and_pipeline() {
    let (mut data, mut words, image) = fixture();
    let (mut d, _, c, b, _) = setup_driver();
    method(&mut words, 0x121c, 0x0fac6881);
    put(&mut data, 0x10000, &words);
    let first = d
        .submit(
            c,
            KICKOFF_PB,
            &header(1),
            Some(&entry(0xb0000, words.len())),
            &mut mem(&mut data),
        )
        .unwrap();
    assert!(first.fault.is_none(), "{:?}", first.fault);
    check(
        &d.publish(c, &mut mem(&mut data)).unwrap().unwrap().rgba,
        &image,
        None,
        BG,
    );
    d.retire(first.token).unwrap();
    let before = d.graphics_counters();
    let words = [
        0x20010487u32,
        0x0fffff81,
        0x20010586,
        4,
        0x200105f7,
        0,
        0x200105f8,
        3,
        0x20010585,
        0,
    ];
    let bytes: Vec<_> = words.iter().flat_map(|w| w.to_le_bytes()).collect();
    d.write_backing(b, 0x10000, &bytes, &mut mem(&mut data))
        .unwrap();
    let second = d
        .submit(
            c,
            KICKOFF_PB,
            &header(1),
            Some(&entry(0xb0000, words.len())),
            &mut mem(&mut data),
        )
        .unwrap();
    assert!(second.fault.is_none(), "{:?}", second.fault);
    assert_eq!(second.progress.draws_submitted, 1);
    let after = d.graphics_counters();
    assert_eq!(after.pipeline_allocations, before.pipeline_allocations);
    assert_eq!(
        after.shader_module_allocations,
        before.shader_module_allocations
    );
    assert_eq!(after.shader_binary_bytes, before.shader_binary_bytes);
    check(
        &d.publish(c, &mut mem(&mut data)).unwrap().unwrap().rgba,
        &image,
        None,
        BG,
    );
}

#[path = "common/gob.rs"]
mod hdr_gob;
#[test]
#[ignore = "requires actual GPU; native R11G11B10 attachment, exact packed guest bytes, GOB padding, deferred warm draws and RGBA diagnostic preview"]
fn native_hdr_targets_preserve_float_guest_pixels_and_deferred_residency() {
    for tiled in [false, true] {
        let (mut data, mut words, image) = fixture();
        data[0x4100..0x4100 + include_bytes!("fixtures/maxwell/hdr.bin").len()]
            .copy_from_slice(include_bytes!("fixtures/maxwell/hdr.bin"));
        let draw = words.split_off(words.len() - 8);
        method(&mut words, 0x810, 0xe0);
        method(&mut words, 0x2144, 0x100); // independently assembled HDR fragment
        if tiled {
            method(&mut words, 0x808, 52);
            method(&mut words, 0x814, 0x10);
        }
        words.extend(draw);
        let linear: Vec<_> = (0..52 * 29)
            .flat_map(|_| 0x00200000u32.to_le_bytes())
            .collect(); // HDR (0,2,0), not UNORM green
        let mut initial = if tiled {
            hdr_gob::encode(&linear, 52, 29, 1, &vec![0xa5; 8192])
        } else {
            linear
        };
        data[RT..RT + initial.len()].copy_from_slice(&initial);
        let (mut d, _, c, b, _) = setup_driver();
        put(&mut data, 0x10000, &words);
        let first = d
            .submit(
                c,
                KICKOFF_PB,
                &header(1),
                Some(&entry(0xb0000, words.len())),
                &mut mem(&mut data),
            )
            .unwrap();
        assert!(first.fault.is_none(), "{:?}", first.fault);
        assert_eq!(first.progress.draws_submitted, 1);
        assert_eq!(&data[RT..RT + initial.len()], initial);
        assert_eq!(d.graphics_counters().readback_operations, 0);
        let before = d.graphics_counters();
        let draws = [
            0x20010586u32,
            4,
            0x200105f7,
            0,
            0x200105f8,
            3,
            0x20010585,
            0,
        ];
        let bytes: Vec<_> = draws.iter().flat_map(|w| w.to_le_bytes()).collect();
        d.write_backing(b, 0x10000, &bytes, &mut mem(&mut data))
            .unwrap();
        for _ in 0..32 {
            let r = d
                .submit(
                    c,
                    KICKOFF_PB,
                    &header(1),
                    Some(&entry(0xb0000, draws.len())),
                    &mut mem(&mut data),
                )
                .unwrap();
            assert!(r.fault.is_none(), "{:?}", r.fault);
        }
        let after = d.graphics_counters();
        assert_eq!(after.readback_operations, 0);
        assert_eq!(after.synchronization_points, before.synchronization_points);
        assert_eq!(
            after.render_target_upload_bytes,
            before.render_target_upload_bytes
        );
        assert_eq!(after.target_deswizzle_bytes, before.target_deswizzle_bytes);
        assert_eq!(after.pipeline_allocations, before.pipeline_allocations);
        assert_eq!(
            after.shader_module_allocations,
            before.shader_module_allocations
        );
        // CPU-after-GPU patches are packed guest bytes, including GOB padding.
        d.write_backing(
            b,
            RT as u64,
            &0x80000000u32.to_le_bytes(),
            &mut mem(&mut data),
        )
        .unwrap();
        if tiled {
            let mut occupied = vec![false; 8192];
            for y in 0..29 {
                for x in 0..208 {
                    occupied[hdr_gob::address(x, y, 52, 1)] = true;
                }
            }
            let hole = occupied.iter().position(|used| !used).unwrap();
            d.write_backing(b, (RT + hole) as u64, &[0x5a], &mut mem(&mut data))
                .unwrap();
            initial[hole] = 0x5a;
            let old = data.clone();
            assert!(d.publish(c, &mut FailWrite(&mut data)).is_err());
            assert_eq!(data, old);
            assert!(d.has_pending_output());
            assert_eq!(d.graphics_counters().draw_commands, 33);
        }
        let frame = d.publish(c, &mut mem(&mut data)).unwrap().unwrap();
        assert_eq!(&frame.rgba[..4], &[0, 0, 255, 255]);
        let mut coverage = frame.rgba.clone();
        coverage[..4].copy_from_slice(&[0, 255, 0, 255]);
        check(
            &coverage,
            &image,
            Some([255, 128, 64, 255]),
            [0, 255, 0, 255],
        );
        let raw = if tiled {
            hdr_gob::decode(&data[RT..RT + 8192], 52, 29, 1)
        } else {
            data[RT..RT + SIZE].to_vec()
        };
        for pixel in raw.chunks_exact(4) {
            let bits = u32::from_le_bytes(pixel.try_into().unwrap());
            assert!(
                matches!(bits, 0x00200000 | 0x681c0400 | 0x80000000),
                "incorrect native HDR representation {bits:#010x}"
            );
        }
        if tiled {
            assert_eq!(
                &data[RT..RT + 8192],
                hdr_gob::encode(&raw, 52, 29, 1, &initial)
            );
        }
        assert_eq!(&raw[..4], &0x80000000u32.to_le_bytes());
        assert_eq!(
            d.graphics_counters().readback_operations,
            if tiled { 2 } else { 1 }
        );
        assert_eq!(d.graphics_counters().draw_commands, 33);
        // Same backing/dimensions, incompatible storage interpretation: recreate
        // only the native target/pipeline, keeping the translated stage modules.
        let baseline = d.graphics_counters();
        let ldr: Vec<_> = (0..52 * 29).flat_map(|_| BG).collect();
        let ldr = if tiled {
            hdr_gob::encode(&ldr, 52, 29, 1, &initial)
        } else {
            ldr
        };
        d.write_backing(b, RT as u64, &ldr, &mut mem(&mut data))
            .unwrap();
        let mut words = Vec::new();
        method(&mut words, 0x810, 0xd5);
        words.extend(draws);
        let bytes: Vec<_> = words.iter().flat_map(|w| w.to_le_bytes()).collect();
        d.write_backing(b, 0x10000, &bytes, &mut mem(&mut data))
            .unwrap();
        let r = d
            .submit(
                c,
                KICKOFF_PB,
                &header(1),
                Some(&entry(0xb0000, words.len())),
                &mut mem(&mut data),
            )
            .unwrap();
        assert!(r.fault.is_none(), "{:?}", r.fault);
        let after = d.graphics_counters();
        assert_eq!(
            after.pipeline_allocations,
            baseline.pipeline_allocations + 1
        );
        assert_eq!(
            after.shader_module_allocations,
            baseline.shader_module_allocations
        );
        assert_eq!(
            after.render_target_allocations,
            baseline.render_target_allocations + 1
        );
        check(
            &d.publish(c, &mut mem(&mut data)).unwrap().unwrap().rgba,
            &image,
            Some([255, 128, 64, 255]),
            BG,
        );
    }
}

#[test]
#[ignore = "requires actual GPU; secondary disabled target formats permit real single-target rendering, but disabling the active target cannot reuse old pixels"]
fn disabled_color_bindings_do_not_activate_mrt_or_reuse_a_stale_active_target() {
    let (mut data, mut words, image) = fixture();
    let draw = words.split_off(words.len() - 8);
    for slot in 1..8 {
        method(&mut words, 0x810 + slot * 0x40, 0);
    }
    words.extend(draw);
    let (mut d, _, c, b, _) = setup_driver();
    put(&mut data, 0x10000, &words);
    let first = d
        .submit(
            c,
            KICKOFF_PB,
            &header(1),
            Some(&entry(0xb0000, words.len())),
            &mut mem(&mut data),
        )
        .unwrap();
    assert!(first.fault.is_none(), "{:?}", first.fault);
    assert_eq!(first.progress.draws_submitted, 1);
    assert_eq!(d.graphics_counters().readback_operations, 0);
    check(
        &d.publish(c, &mut mem(&mut data)).unwrap().unwrap().rgba,
        &image,
        None,
        BG,
    );
    d.retire(first.token).unwrap();
    let words = [
        0x20010204u32,
        0,
        0x20010586,
        4,
        0x200105f7,
        0,
        0x200105f8,
        3,
        0x20010585,
        0,
    ];
    let bytes: Vec<_> = words.iter().flat_map(|w| w.to_le_bytes()).collect();
    d.write_backing(b, 0x10000, &bytes, &mut mem(&mut data))
        .unwrap();
    let old = data.clone();
    let second = d
        .submit(
            c,
            KICKOFF_PB,
            &header(1),
            Some(&entry(0xb0000, words.len())),
            &mut mem(&mut data),
        )
        .unwrap();
    assert!(
        second
            .fault
            .as_ref()
            .unwrap()
            .to_string()
            .contains("selected color target is disabled")
    );
    assert_eq!(second.progress.draws_submitted, 0);
    assert_eq!(data, old);
    assert!(!d.has_pending_output());
    assert_eq!(d.graphics_counters().draw_commands, 1);
    assert_eq!(d.graphics_counters().readback_operations, 1);
    assert!(
        d.submit(
            c,
            KICKOFF_PB,
            &header(1),
            Some(&entry(0xb0000, words.len())),
            &mut mem(&mut data)
        )
        .is_err()
    );
}

#[test]
#[ignore = "requires actual GPU; MME reads live target dimensions and emits color mask; an uninitialized later read preserves the prior draw"]
fn mme_live_color_state_controls_pixels_and_faults_without_default_registers() {
    let (mut data, mut words, image) = fixture();
    let draw = words.split_off(words.len() - 8);
    let code: Vec<u32> = include_str!("fixtures/mme/color-state-read.words")
        .split_ascii_whitespace()
        .map(|w| u32::from_str_radix(w, 16).unwrap())
        .collect();
    method(&mut words, 0x114, 0);
    words.push(0x60000046 | ((code.len() as u32) << 16));
    words.extend_from_slice(&code);
    method(&mut words, 0x11c, 0);
    method(&mut words, 0x120, 0);
    method(&mut words, 0x2144, 0x400); // independent red fragment
    method(&mut words, 0x3800, 0x206); // word offset of explicitly written 0x818=1
    words.extend(draw);
    let (mut d, _, c, b, _) = setup_driver();
    put(&mut data, 0x10000, &words);
    let first = d
        .submit(
            c,
            KICKOFF_PB,
            &header(1),
            Some(&entry(0xb0000, words.len())),
            &mut mem(&mut data),
        )
        .unwrap();
    assert!(first.fault.is_none(), "{:?}", first.fault);
    assert_eq!(first.progress.draws_submitted, 1);
    assert_eq!(d.graphics_counters().readback_operations, 0);
    let bytes = [0x20010e00u32, 0x210]
        .into_iter()
        .flat_map(|w| w.to_le_bytes())
        .collect::<Vec<_>>();
    d.write_backing(b, 0x10000, &bytes, &mut mem(&mut data))
        .unwrap();
    let second = d
        .submit(
            c,
            KICKOFF_PB,
            &header(1),
            Some(&entry(0xb0000, 2)),
            &mut mem(&mut data),
        )
        .unwrap();
    assert!(
        second
            .fault
            .as_ref()
            .unwrap()
            .to_string()
            .contains("UninitializedMethod")
    );
    assert_eq!(second.progress.draws_submitted, 0);
    assert!(d.has_pending_output());
    let frame = d.publish(c, &mut mem(&mut data)).unwrap().unwrap();
    check(&frame.rgba, &image, Some([255, 17, 23, 255]), BG);
    assert_eq!(d.graphics_counters().draw_commands, 1);
    assert_eq!(d.graphics_counters().readback_operations, 1);
}

#[test]
#[ignore = "requires actual GPU; literal shadow replay of validated raster state controls indexed pixels"]
fn pure_raster_shadow_replay_renders_and_missing_capture_preserves_output() {
    let (mut data, mut words, image) = fixture();
    let draw = words.split_off(words.len() - 8);
    let mut pure: Vec<_> = wl_gpu::raster::FIXED_RASTER_STATE
        .iter()
        .copied()
        .filter(|v| v.0 >= 0x200)
        .collect();
    pure.extend([0x02ec, 0x0fb4, 0x0fd8, 0x1148, 0x11f8].map(|m| (m, 0)));
    // Capture the entire supported pure-state subset in TRACK_WITH_FILTER.
    method(&mut words, 0x124, 1);
    for &(m, v) in &pure {
        method(&mut words, u32::from(m), v);
    }
    method(&mut words, 0x1a00, 1); // Capture red-only export.
    method(&mut words, 0x124, 2);
    method(&mut words, 0x1a00, 0x1111); // Passthrough must not replace capture.
    method(&mut words, 0x124, 3);
    for &(m, _) in &pure {
        method(&mut words, u32::from(m), u32::MAX);
    }
    method(&mut words, 0x124, 2); // Actions/resource selection remain outside replay.
    method(&mut words, 0x2144, 0x400);
    words.extend(draw);
    let (mut d, _, c, b, _) = setup_driver();
    put(&mut data, 0x10000, &words);
    let first = d
        .submit(
            c,
            KICKOFF_PB,
            &header(1),
            Some(&entry(0xb0000, words.len())),
            &mut mem(&mut data),
        )
        .unwrap();
    assert!(first.fault.is_none(), "{:?}", first.fault);
    assert_eq!(first.progress.draws_submitted, 1);
    assert_eq!(d.graphics_counters().readback_operations, 0);
    let mut filtered = Vec::new();
    for (m, v) in [
        (0x124, 1),
        (0x1a00, 0x1111), // Changed filtered capture must be retained.
        (0x124, 2),
        (0x1a00, 1),
        (0x124, 3),
        (0x1a00, 0),
        (0xa00, 0), // Viewport scale was never captured in this channel.
    ] {
        method(&mut filtered, m, v);
    }
    let bytes: Vec<_> = filtered.iter().flat_map(|v| v.to_le_bytes()).collect();
    d.write_backing(b, 0x10000, &bytes, &mut mem(&mut data))
        .unwrap();
    let second = d
        .submit(
            c,
            KICKOFF_PB,
            &header(1),
            Some(&entry(0xb0000, filtered.len())),
            &mut mem(&mut data),
        )
        .unwrap();
    assert!(
        second
            .fault
            .as_ref()
            .unwrap()
            .to_string()
            .contains("MissingShadowValue")
    );
    assert_eq!(second.progress.draws_submitted, 0);
    let frame = d.publish(c, &mut mem(&mut data)).unwrap().unwrap();
    check(&frame.rgba, &image, Some([255, 17, 23, 255]), BG);
    assert_eq!(d.graphics_counters().draw_commands, 1);
    assert_eq!(d.graphics_counters().readback_operations, 1);
    d.close(c, &mut mem(&mut data)).unwrap();
}

#[test]
#[ignore = "requires actual GPU; disabled iterated blending cannot multiply draws or change textured pixels"]
fn disabled_iterated_blend_passes_keep_pixels_cached_and_reject_active_modes() {
    let (mut data, mut words, image) = fixture();
    let draw = words.split_off(words.len() - 8);
    method(&mut words, 0xdd0, 0);
    method(&mut words, 0xdd4, 255);
    words.extend_from_slice(&draw);
    let (mut d, _, c, b, _) = setup_driver();
    put(&mut data, 0x10000, &words);
    let first = d
        .submit(
            c,
            KICKOFF_PB,
            &header(1),
            Some(&entry(0xb0000, words.len())),
            &mut mem(&mut data),
        )
        .unwrap();
    assert!(first.fault.is_none(), "{:?}", first.fault);
    assert_eq!(first.progress.draws_submitted, 1);
    let frame = d.publish(c, &mut mem(&mut data)).unwrap().unwrap();
    check(&frame.rgba, &image, None, BG);
    d.retire(first.token).unwrap();
    let before = d.graphics_counters();
    let mut next = Vec::new();
    method(&mut next, 0xdd4, 1);
    next.extend_from_slice(&draw);
    let bytes: Vec<_> = next.iter().flat_map(|v| v.to_le_bytes()).collect();
    d.write_backing(b, 0x10000, &bytes, &mut mem(&mut data))
        .unwrap();
    let second = d
        .submit(
            c,
            KICKOFF_PB,
            &header(1),
            Some(&entry(0xb0000, next.len())),
            &mut mem(&mut data),
        )
        .unwrap();
    assert!(second.fault.is_none(), "{:?}", second.fault);
    assert_eq!(second.progress.draws_submitted, 1);
    let after = d.graphics_counters();
    assert_eq!(after.draw_commands, before.draw_commands + 1);
    assert_eq!(
        after.shader_module_allocations,
        before.shader_module_allocations
    );
    assert_eq!(after.pipeline_allocations, before.pipeline_allocations);
    assert_eq!(after.texture_uploads, before.texture_uploads);
    let invalid = [0x20010374u32, 1];
    let bytes: Vec<_> = invalid.iter().flat_map(|v| v.to_le_bytes()).collect();
    d.write_backing(b, 0x10000, &bytes, &mut mem(&mut data))
        .unwrap();
    let failed = d
        .submit(
            c,
            KICKOFF_PB,
            &header(1),
            Some(&entry(0xb0000, 2)),
            &mut mem(&mut data),
        )
        .unwrap();
    assert!(failed.fault.is_some());
    assert_eq!(d.graphics_counters().draw_commands, after.draw_commands);
    let frame = d.publish(c, &mut mem(&mut data)).unwrap().unwrap();
    check(&frame.rgba, &image, None, BG);
    d.close(c, &mut mem(&mut data)).unwrap();
}

#[test]
#[ignore = "requires actual GPU; independent MME macros program seven inactive target banks without changing RT0 pixels"]
fn mme_inactive_target_banks_preserve_pixels_pipeline_and_pending_output() {
    let (mut data, mut words, image) = fixture();
    let draw = words.split_off(words.len() - 8);
    let code: Vec<u32> = include_str!("fixtures/mme/inactive-targets.words")
        .split_ascii_whitespace()
        .map(|v| u32::from_str_radix(v, 16).unwrap())
        .collect();
    assert_eq!(code.len(), 18);
    method(&mut words, 0x114, 0);
    words.push(0x60120046); // Nonincrementing upload of eighteen independently assembled words.
    words.extend(code);
    for (m, v) in [
        (0x11c, 0),
        (0x120, 0),
        (0x120, 9),
        (0x124, 1),
        (0x3800, 1),
        (0x3808, 0x1011),
        (0x124, 3),
        (0x3800, u32::MAX),
        (0x3808, u32::MAX),
        (0x124, 2),
    ] {
        method(&mut words, m, v);
    }
    words.extend_from_slice(&draw);
    let (mut d, _, c, b, _) = setup_driver();
    put(&mut data, 0x10000, &words);
    let first = d
        .submit(
            c,
            KICKOFF_PB,
            &header(1),
            Some(&entry(0xb0000, words.len())),
            &mut mem(&mut data),
        )
        .unwrap();
    assert!(first.fault.is_none(), "{:?}", first.fault);
    assert_eq!(first.progress.draws_submitted, 1);
    check(
        &d.publish(c, &mut mem(&mut data)).unwrap().unwrap().rgba,
        &image,
        None,
        BG,
    );
    d.retire(first.token).unwrap();
    let before = d.graphics_counters();
    let mut next = Vec::new();
    for (m, v) in [(0x124, 1), (0x3800, 0), (0x3808, 0), (0x124, 2)] {
        method(&mut next, m, v);
    }
    next.extend(draw);
    let bytes: Vec<_> = next.iter().flat_map(|v| v.to_le_bytes()).collect();
    d.write_backing(b, 0x10000, &bytes, &mut mem(&mut data))
        .unwrap();
    let second = d
        .submit(
            c,
            KICKOFF_PB,
            &header(1),
            Some(&entry(0xb0000, next.len())),
            &mut mem(&mut data),
        )
        .unwrap();
    assert!(second.fault.is_none(), "{:?}", second.fault);
    assert_eq!(second.progress.draws_submitted, 1);
    let after = d.graphics_counters();
    assert_eq!(after.pipeline_allocations, before.pipeline_allocations);
    assert_eq!(
        after.shader_module_allocations,
        before.shader_module_allocations
    );
    assert_eq!(after.texture_uploads, before.texture_uploads);
    let bad = [0x20010e02u32, 2]; // Macro entry one, invalid RGBA mask bit at inactive RT1.
    let bytes: Vec<_> = bad.iter().flat_map(|v| v.to_le_bytes()).collect();
    d.write_backing(b, 0x10000, &bytes, &mut mem(&mut data))
        .unwrap();
    let fault = d
        .submit(
            c,
            KICKOFF_PB,
            &header(1),
            Some(&entry(0xb0000, 2)),
            &mut mem(&mut data),
        )
        .unwrap();
    assert!(fault.fault.as_ref().unwrap().to_string().contains("0x1a04"));
    assert_eq!(fault.progress.draws_submitted, 0);
    assert_eq!(d.graphics_counters().draw_commands, after.draw_commands);
    check(
        &d.publish(c, &mut mem(&mut data)).unwrap().unwrap().rgba,
        &image,
        None,
        BG,
    );
    d.close(c, &mut mem(&mut data)).unwrap();
}

#[test]
#[ignore = "requires actual GPU; unused fixed blend constants remain data and do not specialize textured draws"]
fn unused_blend_constants_keep_pixels_and_warm_pipeline_data_independent() {
    let (mut data, mut words, image) = fixture();
    let draw = words.split_off(words.len() - 8);
    for (m, v) in [
        (0x1360, 1),
        (0x133c, 1),
        (0x1340, 1),
        (0x1344, 2),
        (0x1348, 1),
        (0x134c, 1),
        (0x1350, 2),
        (0x1358, 1),
    ] {
        method(&mut words, m, v);
    }
    words.extend([0x200404c7, 0x3f800000, 0x7fc00001, 0x7f800000, 0x80000000]);
    words.extend_from_slice(&draw);
    let (mut d, _, c, b, _) = setup_driver();
    put(&mut data, 0x10000, &words);
    let first = d
        .submit(
            c,
            KICKOFF_PB,
            &header(1),
            Some(&entry(0xb0000, words.len())),
            &mut mem(&mut data),
        )
        .unwrap();
    assert!(first.fault.is_none(), "{:?}", first.fault);
    check(
        &d.publish(c, &mut mem(&mut data)).unwrap().unwrap().rgba,
        &image,
        None,
        BG,
    );
    d.retire(first.token).unwrap();
    let before = d.graphics_counters();
    for i in 0..32u32 {
        let mut next = vec![0x200404c7, i, 0x3e800000, 0x3f000000, 0x3f800000];
        next.extend_from_slice(&draw);
        let bytes: Vec<_> = next.iter().flat_map(|v| v.to_le_bytes()).collect();
        d.write_backing(b, 0x10000, &bytes, &mut mem(&mut data))
            .unwrap();
        let report = d
            .submit(
                c,
                KICKOFF_PB,
                &header(1),
                Some(&entry(0xb0000, next.len())),
                &mut mem(&mut data),
            )
            .unwrap();
        assert!(report.fault.is_none(), "{:?}", report.fault);
        assert_eq!(report.progress.draws_submitted, 1);
    }
    let after = d.graphics_counters();
    assert_eq!(after.draw_commands, before.draw_commands + 32);
    assert_eq!(after.pipeline_allocations, before.pipeline_allocations);
    assert_eq!(
        after.shader_module_allocations,
        before.shader_module_allocations
    );
    assert_eq!(after.texture_uploads, before.texture_uploads);
    assert_eq!(after.readback_operations, before.readback_operations);
    assert_eq!(after.synchronization_points, before.synchronization_points);
    check(
        &d.publish(c, &mut mem(&mut data)).unwrap().unwrap().rgba,
        &image,
        None,
        BG,
    );
    d.close(c, &mut mem(&mut data)).unwrap();
}

#[test]
#[ignore = "requires actual GPU; neutral depth bias preserves indexed textured output and warm pipeline"]
fn neutral_depth_bias_preserves_indexed_pixels_and_rejects_active_slope() {
    let (mut data, mut words, image) = fixture();
    let draw = words.split_off(words.len() - 8);
    for method_offset in [0x156c, 0x15bc, 0x187c] {
        method(&mut words, method_offset, 0);
    }
    words.extend_from_slice(&draw);
    let (mut d, _, c, b, _) = setup_driver();
    put(&mut data, 0x10000, &words);
    let first = d
        .submit(
            c,
            KICKOFF_PB,
            &header(1),
            Some(&entry(0xb0000, words.len())),
            &mut mem(&mut data),
        )
        .unwrap();
    assert!(first.fault.is_none(), "{:?}", first.fault);
    let before = d.graphics_counters();
    let mut next = Vec::new();
    for method_offset in [0x156c, 0x15bc, 0x187c] {
        method(&mut next, method_offset, 0x80000000);
    }
    next.extend_from_slice(&draw);
    let bytes: Vec<_> = next.iter().flat_map(|v| v.to_le_bytes()).collect();
    d.write_backing(b, 0x10000, &bytes, &mut mem(&mut data))
        .unwrap();
    let second = d
        .submit(
            c,
            KICKOFF_PB,
            &header(1),
            Some(&entry(0xb0000, next.len())),
            &mut mem(&mut data),
        )
        .unwrap();
    assert!(second.fault.is_none());
    assert_eq!(
        d.graphics_counters().pipeline_allocations,
        before.pipeline_allocations
    );
    assert_eq!(
        d.graphics_counters().shader_module_allocations,
        before.shader_module_allocations
    );
    assert_eq!(
        d.graphics_counters().readback_operations,
        before.readback_operations
    );
    // An active coefficient cannot be silently accepted, nor discard either draw.
    let bad = [0x2001055bu32, 1f32.to_bits()];
    let bytes: Vec<_> = bad.iter().flat_map(|v| v.to_le_bytes()).collect();
    d.write_backing(b, 0x10000, &bytes, &mut mem(&mut data))
        .unwrap();
    let rejected = d
        .submit(
            c,
            KICKOFF_PB,
            &header(1),
            Some(&entry(0xb0000, 2)),
            &mut mem(&mut data),
        )
        .unwrap();
    assert!(rejected.fault.unwrap().to_string().contains("0x156c"));
    assert_eq!(d.graphics_counters().draw_commands, 2);
    check(
        &d.publish(c, &mut mem(&mut data)).unwrap().unwrap().rgba,
        &image,
        None,
        BG,
    );
    d.close(c, &mut mem(&mut data)).unwrap();
}

#[test]
#[ignore = "requires actual GPU; fragment completion and WFI texture invalidation retain unpublished output"]
fn pixel_barrier_waits_without_publication_and_texture_wfi_invalidates() {
    let (mut data, mut words, image) = fixture();
    let draw = words.split_off(words.len() - 8);
    for (m, v) in [
        (0x210, 0),
        (0xf1c, 0),
        (0xf40, 0),
        (0x1224, 0),
        (0x11f0, 0),
        (0x754, 1),
        (0x2000, 0),
        (0x2080, 0x20),
        (0x20c0, 0x30),
        (0x2100, 0x40),
    ] {
        method(&mut words, m, v);
    }
    words.extend_from_slice(&draw);
    let (mut d, _, c, b, _) = setup_driver();
    put(&mut data, 0x10000, &words);
    let first = d
        .submit(
            c,
            KICKOFF_PB,
            &header(1),
            Some(&entry(0xb0000, words.len())),
            &mut mem(&mut data),
        )
        .unwrap();
    assert!(first.fault.is_none());
    assert!(d.has_pending_output());
    assert!(data[RT..RT + SIZE].chunks_exact(4).all(|p| p == BG));
    let before = d.graphics_counters();
    // Pixel barrier and all-lines texture invalidate are two ordered actions.
    let barriers = [0x20010378u32, 0, 0x200103dd, 0];
    let bytes: Vec<_> = barriers.iter().flat_map(|v| v.to_le_bytes()).collect();
    d.write_backing(b, 0x10000, &bytes, &mut mem(&mut data))
        .unwrap();
    let report = d
        .submit(
            c,
            KICKOFF_PB,
            &header(1),
            Some(&entry(0xb0000, 4)),
            &mut mem(&mut data),
        )
        .unwrap();
    assert!(report.fault.is_none(), "{:?}", report.fault);
    let after = d.graphics_counters();
    assert_eq!(
        after.synchronization_points,
        before.synchronization_points + 2
    );
    assert_eq!(after.submissions, before.submissions);
    assert_eq!(after.readback_operations, before.readback_operations);
    assert_eq!(after.draw_commands, before.draw_commands);
    assert!(d.has_pending_output());
    assert!(d.wait(report.token).unwrap().host_completed);
    assert!(!d.query(report.token).unwrap().guest_visible);
    assert!(data[RT..RT + SIZE].chunks_exact(4).all(|p| p == BG));
    let bytes: Vec<_> = draw.iter().flat_map(|v| v.to_le_bytes()).collect();
    d.write_backing(b, 0x10000, &bytes, &mut mem(&mut data))
        .unwrap();
    let next = d
        .submit(
            c,
            KICKOFF_PB,
            &header(1),
            Some(&entry(0xb0000, draw.len())),
            &mut mem(&mut data),
        )
        .unwrap();
    assert!(next.fault.is_none());
    assert!(d.graphics_counters().texture_uploads > after.texture_uploads);
    assert_eq!(
        d.graphics_counters().shader_module_allocations,
        after.shader_module_allocations
    );
    check(
        &d.publish(c, &mut mem(&mut data)).unwrap().unwrap().rgba,
        &image,
        None,
        BG,
    );
    d.close(c, &mut mem(&mut data)).unwrap();
}
