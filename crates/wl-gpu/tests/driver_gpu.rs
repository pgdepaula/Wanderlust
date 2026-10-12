#![cfg(feature = "wgpu-backend")]
mod common;
use stats_alloc::{INSTRUMENTED_SYSTEM, Region, StatsAlloc};
use std::alloc::System;
#[global_allocator]
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
#[ignore = "requires actual wgpu adapter and pixel readback"]
fn driver_completion_publication_and_mapping_lifetime() {
    let (mut data, words, image) = fixture();
    let (mut d, space, c, b, maps) = setup_driver();
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
    let h = header(1);
    let e = entry(0xb0000, 1);
    let mut data = vec![0; 0x18000];
    let region = Region::new(ALLOCATOR);
    let start = std::time::Instant::now();
    for _ in 0..1000 {
        let r = d
            .submit(a, KICKOFF_PB, &h, Some(&e), &mut mem(&mut data))
            .unwrap();
        assert!(r.fault.is_none());
        d.retire(r.token).unwrap();
    }
    let elapsed = start.elapsed();
    let stats = region.change();
    eprintln!(
        "1000 NOP channel submit+retire: {elapsed:?}, allocations {}, reallocations {}, borrowed {}, queue submissions {}",
        stats.allocations,
        stats.reallocations,
        d.graphics_counters().gpfifo_borrowed_bytes,
        d.graphics_counters().submissions
    );
    assert_eq!(stats.allocations, 0);
    assert_eq!(stats.reallocations, 0);
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
