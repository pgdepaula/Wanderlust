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
fn tiled_setup(words: &mut Vec<u32>, log: u8) {
    // Override actual B197 target methods after the baseline, before draw.
    let end = words.split_off(words.len() - 8);
    for (m, v) in [
        (0x808, W as u32),
        (0x814, u32::from(log) << 4),
        (0x81c, (gob::size(W, H, log) / 4) as u32),
        (0xe04, 35 << 16),
        (0xe08, 20 << 16),
    ] {
        method(words, m, v);
    }
    words.extend(end);
}
fn background() -> Vec<u8> {
    common::rgba_mip(W as u32, H as u32, 2)
}
fn storage(log: u8) -> Vec<u8> {
    (0..gob::size(W, H, log))
        .map(|n| (n * 23 + n / 31 + 0x71) as u8)
        .collect()
}
fn edge(a: [f64; 2], b: [f64; 2], p: [f64; 2]) -> f64 {
    (b[0] - a[0]) * (p[1] - a[1]) - (b[1] - a[1]) * (p[0] - a[0])
}
// Independent screen-linear barycentric/nearest oracle. Boundary tie pixels are
// excluded only from color assertions; exact layout/padding checks still cover them.
fn check_image(actual: &[u8], initial: &[u8], texture: &[u8], blend: bool) {
    let v = POS.map(|p| p.map(f64::from));
    let area = edge(v[0], v[1], v[2]);
    let mut covered = 0;
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
            let inside = x < 35 && y < 20 && weights.iter().all(|b| *b >= 0.);
            if x < 35 && y < 20 && weights.iter().any(|b| b.abs() < 0.005) {
                continue;
            }
            let off = (y * W + x) * 4;
            let mut expected: [u8; 4] = initial[off..off + 4].try_into().unwrap();
            if inside {
                covered += 1;
                if blend {
                    // src=(.25,1,.5,.5), SRC_ALPHA / ONE_MINUS_SRC_ALPHA,
                    // separate alpha ONE / ONE_MINUS_SRC_ALPHA, mask R/B only.
                    for (k, c) in [(0, 0.25), (2, 0.5)] {
                        expected[k] = (c * 255. * 0.5 + f64::from(expected[k]) * 0.5).round() as u8;
                    }
                } else {
                    let u = (0..3)
                        .map(|n| weights[n] * f64::from(UV[n][0]))
                        .sum::<f64>()
                        * 37.;
                    let vv = (0..3)
                        .map(|n| weights[n] * f64::from(UV[n][1]))
                        .sum::<f64>()
                        * 19.;
                    if (u - u.round()).abs() < 0.02 || (vv - vv.round()).abs() < 0.02 {
                        continue;
                    }
                    let a = (vv.floor().clamp(0., 18.) as usize * 37
                        + u.floor().clamp(0., 36.) as usize)
                        * 4;
                    expected.copy_from_slice(&texture[a..a + 4]);
                }
            }
            for k in 0..4 {
                assert!(
                    actual[off + k].abs_diff(expected[k]) <= u8::from(blend),
                    "pixel{x},{y}: {:?} != {:?}",
                    &actual[off..off + 4],
                    expected
                );
            }
        }
    }
    assert!(covered > 100);
}
fn check_storage(tiled: &[u8], pixels: &[u8], original: &[u8], log: u8) {
    assert_eq!(tiled, gob::encode(pixels, W, H, log, original));
    assert_eq!(gob::decode(tiled, W, H, log), pixels);
}
#[test]
#[ignore = "requires real GPU; block-linear target GPFIFO execution"]
fn tiled_driver_draw_warm_residency_blending_masks_and_padding() {
    let log = 1;
    let (mut data, mut words, texture) = fixture();
    let (d0, c, s, b, m) = driver();
    let mut d = d0;
    let initial = background();
    let original = gob::encode(&initial, W, H, log, &storage(log));
    let size = original.len();
    data[RT..RT + size].copy_from_slice(&original);
    tiled_setup(&mut words, log);
    let first = submit(&mut d, c, b, &mut data, &words);
    assert!(first.fault.is_none());
    assert_eq!(&data[RT..RT + size], original);
    assert!(d.has_pending_output());
    assert!(d.unmap(s, m).is_err());
    let frame = d.publish(c, &mut mem(&mut data)).unwrap().unwrap();
    check_image(&frame.rgba, &initial, &texture, false);
    check_storage(&data[RT..RT + size], &frame.rgba, &original, log);
    // Warm repeated draws: real commands, target resident, no conversions/readbacks.
    let commands = draw();
    let prime = submit(&mut d, c, b, &mut data, &commands);
    assert!(prime.fault.is_none());
    let before = d.graphics_counters();
    let region = Region::new(ALLOCATOR);
    let start = std::time::Instant::now();
    for _ in 0..32 {
        assert!(
            submit_existing(&mut d, c, &mut data, commands.len())
                .fault
                .is_none()
        );
    }
    let elapsed = start.elapsed();
    let alloc = region.change();
    let warm = d.graphics_counters();
    assert_eq!(warm.upload_bytes, before.upload_bytes);
    assert_eq!(warm.target_deswizzle_bytes, before.target_deswizzle_bytes);
    assert_eq!(warm.target_swizzle_bytes, before.target_swizzle_bytes);
    assert_eq!(warm.pipeline_allocations, before.pipeline_allocations);
    assert_eq!(
        warm.shader_module_allocations,
        before.shader_module_allocations
    );
    assert_eq!(warm.readback_operations, before.readback_operations);
    assert_eq!(warm.synchronization_points, before.synchronization_points);
    assert_eq!(warm.submissions - before.submissions, 32);
    let old_storage = data[RT..RT + size].to_vec();
    let frame = d.publish(c, &mut mem(&mut data)).unwrap().unwrap();
    check_image(&frame.rgba, &initial, &texture, false);
    check_storage(&data[RT..RT + size], &frame.rgba, &old_storage, log);
    // Real guest LDC shader and blend/mask registers operate on decoded old colors.
    let previous = frame.rgba;
    let previous_storage = data[RT..RT + size].to_vec();
    for (k, f) in [0.25f32, 1., 0.5, 0.5].into_iter().enumerate() {
        d.write_backing(b, 80 + k as u64 * 4, &f.to_le_bytes(), &mut mem(&mut data))
            .unwrap();
    }
    let mut w = Vec::new();
    for (m, v) in [
        (0x2144, 0x500),
        (0x1360, 1),
        (0x133c, 1),
        (0x1340, 1),
        (0x1344, 5),
        (0x1348, 6),
        (0x134c, 1),
        (0x1350, 2),
        (0x1354, 0),
        (0x1358, 6),
        (0x1a00, 0x101),
    ] {
        method(&mut w, m, v);
    }
    w.extend(draw());
    let r = submit(&mut d, c, b, &mut data, &w);
    assert!(r.fault.is_none());
    let f = d.publish(c, &mut mem(&mut data)).unwrap().unwrap();
    check_image(&f.rgba, &previous, &texture, true);
    check_storage(&data[RT..RT + size], &f.rgba, &previous_storage, log);
    // CPU write before the next draw refreshes this same tiled target, without
    // touching pipeline identity. This pixel lies beyond scissor coverage.
    let mut changed = f.rgba;
    let x = 50;
    let y = 25;
    changed[(y * W + x) * 4..(y * W + x + 1) * 4].copy_from_slice(&[99, 100, 101, 255]);
    let offset = gob::address(x * 4, y, W, log);
    d.write_backing(
        b,
        (RT + offset) as u64,
        &[99, 100, 101, 255],
        &mut mem(&mut data),
    )
    .unwrap();
    let old_storage = data[RT..RT + size].to_vec();
    let before = d.graphics_counters();
    assert!(submit(&mut d, c, b, &mut data, &draw()).fault.is_none());
    let f = d.publish(c, &mut mem(&mut data)).unwrap().unwrap();
    check_image(&f.rgba, &changed, &texture, true);
    check_storage(&data[RT..RT + size], &f.rgba, &old_storage, log);
    assert_eq!(
        d.graphics_counters().render_target_upload_bytes - before.render_target_upload_bytes,
        SIZE as u64
    );
    assert_eq!(
        d.graphics_counters().pipeline_allocations,
        before.pipeline_allocations
    );
    d.close(c, &mut mem(&mut data)).unwrap();
    eprintln!(
        "32 tiled warm draws {elapsed:?}, allocations {}, reallocations {}; counters {:?}",
        alloc.allocations,
        alloc.reallocations,
        d.graphics_counters()
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
        Err(wl_gpu::Error("injected atomic publication failure".into()))
    }
}
fn padding_offsets(log: u8) -> Vec<usize> {
    let mut used = vec![false; gob::size(W, H, log)];
    for y in 0..H {
        for x in 0..W * 4 {
            used[gob::address(x, y, W, log)] = true;
        }
    }
    used.iter()
        .enumerate()
        .filter_map(|(i, u)| (!u).then_some(i))
        .collect()
}
#[test]
#[ignore = "requires real GPU; tiled CPU patches, channel fault and retry"]
fn tiled_raw_cpu_patches_padding_fault_and_atomic_publication_retry() {
    let log = 1;
    let (mut data, mut words, texture) = fixture();
    let (mut d, c, s, b, m) = driver();
    let initial = background();
    let original = gob::encode(&initial, W, H, log, &storage(log));
    let size = original.len();
    data[RT..RT + size].copy_from_slice(&original);
    tiled_setup(&mut words, log);
    words.push(0xe0000000); // Fault after a successful real draw, never replay it.
    let r = submit(&mut d, c, b, &mut data, &words);
    assert!(r.fault.is_some());
    assert_eq!(r.progress.draws_submitted, 1);
    assert!(d.wait(r.token).unwrap().host_completed);
    assert!(submit_existing_error(&mut d, c, &mut data, words.len()));
    assert!(d.unmap(s, m).is_err());
    // Deliberately raw, unaligned and crossing the 16-byte GOB microtile boundary.
    // Neither interval is a row-major pixel offset.
    let raw = gob::address(3 * 4 + 3, 11, W, log);
    assert_ne!(raw, (11 * W + 3) * 4 + 3);
    let padding = *padding_offsets(log).iter().find(|n| **n > SIZE).unwrap();
    let patches = [(raw, vec![91, 92, 93, 94, 95, 96]), (padding, vec![0xe7])];
    for (off, bytes) in &patches {
        d.write_backing(b, (RT + off) as u64, bytes, &mut mem(&mut data))
            .unwrap();
    }
    let before = data.clone();
    let draws = d.graphics_counters().gpu_target_writes;
    assert!(d.publish(c, &mut FailWrite(&mut data)).is_err());
    assert_eq!(data, before);
    assert!(d.has_pending_output());
    assert!(!d.query(r.token).unwrap().guest_visible);
    let frame = d.publish(c, &mut mem(&mut data)).unwrap().unwrap();
    assert_eq!(draws, d.graphics_counters().gpu_target_writes);
    assert!(d.query(r.token).unwrap().guest_visible);
    for (off, bytes) in &patches {
        assert_eq!(&data[RT + off..RT + off + bytes.len()], bytes);
    }
    let mut restored = frame.rgba.clone();
    // Restore only the patched logical components for the independent raster oracle.
    // Pixel (3,11) and neighboring GOB bytes are outside this clipped triangle.
    for y in 0..H {
        for x in 0..W * 4 {
            let a = gob::address(x, y, W, log);
            if patches
                .iter()
                .any(|(off, v)| (*off..*off + v.len()).contains(&a))
            {
                restored[y * W * 4 + x] = initial[y * W * 4 + x];
            }
        }
    }
    check_image(&restored, &initial, &texture, false);
    check_storage(
        &data[RT..RT + size],
        &frame.rgba,
        &before[RT..RT + size],
        log,
    );
    assert_eq!(d.graphics_counters().target_patch_bytes, 14); // two attempts, seven bytes each
    d.close(c, &mut mem(&mut data)).unwrap();
}
fn submit_existing_error(d: &mut GpuDriver, c: ChannelId, data: &mut [u8], length: usize) -> bool {
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
    .is_err()
}
#[test]
#[ignore = "requires real GPU; full padded footprint command barrier and alias identity"]
fn tiled_padding_command_alias_and_target_layout_transitions() {
    let log = 1;
    let (mut data, mut words, texture) = fixture();
    let (mut d, c, _s, b, _) = driver();
    let initial = background();
    let mut original = gob::encode(&initial, W, H, log, &storage(log));
    // Put a NOP entirely in padding, beyond the logical linear byte count.
    let pads = padding_offsets(log);
    let nop = *pads
        .iter()
        .find(|n| **n > SIZE && **n % 4 == 0 && (0..4).all(|k| pads.contains(&(**n + k))))
        .unwrap();
    original[nop..nop + 4].fill(0);
    data[RT..RT + original.len()].copy_from_slice(&original);
    tiled_setup(&mut words, log);
    assert!(submit(&mut d, c, b, &mut data, &words).fault.is_none());
    // Numerical GPU address changes, canonical surface and layout do not.
    let mut alias = Vec::new();
    method(&mut alias, 0x804, 0xa0000);
    alias.extend(draw());
    let before = d.graphics_counters();
    assert!(submit(&mut d, c, b, &mut data, &alias).fault.is_none());
    let after = d.graphics_counters();
    assert_eq!(
        before.render_target_upload_bytes,
        after.render_target_upload_bytes
    );
    assert_eq!(before.readback_operations, after.readback_operations);
    // Command acquisition must see the full tiled footprint, including padding.
    let mut h = [0; 24];
    h[8..12].copy_from_slice(&1u32.to_le_bytes());
    h[12..16].copy_from_slice(&4u32.to_le_bytes());
    let r = d
        .submit(
            c,
            KICKOFF_PB,
            &h,
            Some(&entry(0xa0000 + nop as u64, 1)),
            &mut mem(&mut data),
        )
        .unwrap();
    assert!(r.fault.is_none());
    assert!(!d.has_pending_output());
    assert_eq!(
        d.graphics_counters().readback_operations,
        after.readback_operations + 1
    );
    check_image(
        &gob::decode(&data[RT..RT + original.len()], W, H, log),
        &initial,
        &texture,
        false,
    );
    assert_eq!(&data[RT + nop..RT + nop + 4], &[0; 4]);
    // BH1 and BH2 have IDENTICAL dimensions and footprint here (height29).
    // No CPU target write: layout alone must barrier pending output and reupload.
    assert!(submit(&mut d, c, b, &mut data, &draw()).fault.is_none());
    let old_pixels = gob::decode(&data[RT..RT + original.len()], W, H, log);
    let old_storage = gob::encode(&old_pixels, W, H, log, &original);
    let reinterpreted = gob::decode(&old_storage, W, H, 0);
    let mut next = Vec::new();
    method(&mut next, 0x814, 0);
    next.extend(draw());
    let before = d.graphics_counters();
    assert!(submit(&mut d, c, b, &mut data, &next).fault.is_none());
    assert_eq!(
        d.graphics_counters().readback_operations,
        before.readback_operations + 1
    );
    let frame = d.publish(c, &mut mem(&mut data)).unwrap().unwrap();
    assert_eq!(
        d.graphics_counters().render_target_upload_bytes - before.render_target_upload_bytes,
        SIZE as u64
    );
    assert_eq!(
        d.graphics_counters().pipeline_allocations,
        before.pipeline_allocations
    );
    check_image(&frame.rgba, &reinterpreted, &texture, false);
    check_storage(
        &data[RT..RT + old_storage.len()],
        &frame.rgba,
        &old_storage,
        0,
    );
    let other = data[RT..RT + old_storage.len()].to_vec();
    // Same host dimensions, new pitch-linear backing, then return to tiled.
    let tiled_pixels = frame.rgba.clone();
    let pitch = initial.clone();
    d.write_backing(b, 0x12000, &pitch, &mut mem(&mut data))
        .unwrap();
    let mut next = Vec::new();
    for (m, v) in [
        (0x804, 0xd0000),
        (0x808, (W * 4) as u32),
        (0x814, 0x1000),
        (0x81c, 0),
    ] {
        method(&mut next, m, v);
    }
    next.extend(draw());
    let before = d.graphics_counters();
    assert!(submit(&mut d, c, b, &mut data, &next).fault.is_none());
    let frame = d.publish(c, &mut mem(&mut data)).unwrap().unwrap();
    assert_eq!(&data[0x12000..0x12000 + SIZE], frame.rgba);
    check_image(&frame.rgba, &initial, &texture, false);
    assert_eq!(
        d.graphics_counters().pipeline_allocations,
        before.pipeline_allocations
    );
    assert_eq!(d.graphics_counters().render_target_allocations, 1);
    let mut next = Vec::new();
    for (m, v) in [
        (0x804, 0x90000),
        (0x808, W as u32),
        (0x814, 0),
        (0x81c, (other.len() / 4) as u32),
    ] {
        method(&mut next, m, v);
    }
    next.extend(draw());
    assert!(submit(&mut d, c, b, &mut data, &next).fault.is_none());
    d.close(c, &mut mem(&mut data)).unwrap(); // close publishes tiled output
    check_storage(&data[RT..RT + other.len()], &tiled_pixels, &other, 0);
}

#[test]
#[ignore = "requires real GPU; tiled backing remap and missing final GOB"]
fn tiled_remap_and_unmapped_storage_tail_fail_without_guest_corruption() {
    let log = 1;
    let (mut data, mut words, texture) = fixture();
    let (mut d, c, s, b, _) = driver();
    let initial = background();
    let original = gob::encode(&initial, W, H, log, &storage(log));
    let r = d.reserve_fixed(s, GpuAddress(0xf0000), 0x4000).unwrap();
    let m = d.map(s, r, b, 0x16000).unwrap();
    data[0x16000..0x16000 + original.len()].copy_from_slice(&original);
    tiled_setup(&mut words, log);
    let end = words.split_off(words.len() - 8);
    method(&mut words, 0x804, 0xf0000);
    words.extend(end);
    assert!(submit(&mut d, c, b, &mut data, &words).fault.is_none());
    assert!(d.unmap(s, m).is_err());
    let f = d.publish(c, &mut mem(&mut data)).unwrap().unwrap();
    check_image(&f.rgba, &initial, &texture, false);
    let old = data[0x16000..0x16000 + original.len()].to_vec();
    d.unmap(s, m).unwrap();
    d.map(s, r, b, 0x1a000).unwrap();
    let changed = common::rgba_mip(W as u32, H as u32, 4);
    let new = gob::encode(&changed, W, H, log, &storage(log));
    d.write_backing(b, 0x1a000, &new, &mut mem(&mut data))
        .unwrap();
    let before = d.graphics_counters();
    assert!(submit(&mut d, c, b, &mut data, &draw()).fault.is_none());
    let f = d.publish(c, &mut mem(&mut data)).unwrap().unwrap();
    check_image(&f.rgba, &changed, &texture, false);
    check_storage(&data[0x1a000..0x1a000 + new.len()], &f.rgba, &new, log);
    assert_eq!(&data[0x16000..0x16000 + old.len()], old);
    assert_eq!(
        d.graphics_counters().pipeline_allocations,
        before.pipeline_allocations
    );
    assert_eq!(
        d.graphics_counters().render_target_upload_bytes - before.render_target_upload_bytes,
        SIZE as u64
    );
    // One mapped page cannot cover the final GOBs of this 8192-byte surface.
    let r = d.reserve_fixed(s, GpuAddress(0xe0000), 0x1000).unwrap();
    d.map(s, r, b, 0x16000).unwrap();
    let mut invalid = Vec::new();
    method(&mut invalid, 0x804, 0xe0000);
    invalid.extend(draw());
    let before = d.graphics_counters().gpu_target_writes;
    let old = data[0x16000..0x1c000].to_vec();
    let report = submit(&mut d, c, b, &mut data, &invalid);
    assert!(report.fault.is_some());
    assert_eq!(report.progress.draws_submitted, 0);
    assert_eq!(d.graphics_counters().gpu_target_writes, before);
    assert_eq!(&data[0x16000..0x1c000], old);
    d.close(c, &mut mem(&mut data)).unwrap();
}
