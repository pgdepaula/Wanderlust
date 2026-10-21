#![cfg(feature = "wgpu-backend")]
mod common;
use common::{maxwell::*, *};
use stats_alloc::{INSTRUMENTED_SYSTEM, Region, StatsAlloc};
use std::alloc::System;
#[global_allocator]
static ALLOCATOR: &StatsAlloc<System> = &INSTRUMENTED_SYSTEM;
use wl_gpu::{
    backend::Renderer,
    gpfifo::*,
    maxwell::{Channel, TextureImage, TextureSampler},
    memory::*,
    texture::*,
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
fn fixture() -> (Vec<u8>, AddressMap, Vec<u32>, Vec<u8>) {
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
    let mut map = AddressMap::default();
    for (g, a, n) in [
        (0x10000, 0x1000, surface.len() as u64),
        (0x30000, 0, 32),
        (0x31000, 32, 32),
        (0x40000, 64, 32),
        (0x50000, 0x4000, 0x1000),
        (0x70000, 0x3000, 72),
        (0x80000, 0x6000, 6),
        (0x90000, RT as u64, SIZE as u64),
        (0xb0000, 0x10000, 0x4000),
        (0xc0000, 0x10000, 0x4000),
    ] {
        map.map(GpuAddress(g), GuestAddress(a), n).unwrap();
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
    (data, map, w, images[0].clone())
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
fn submit(
    r: &mut Renderer,
    c: &mut Channel,
    e: &[u8],
    m: &AddressMap,
    d: &mut [u8],
    t: &mut TextureChanges,
) -> Result<Progress, SubmissionFault> {
    r.submit_gpfifo_deferred(
        c,
        e,
        m,
        &mut SliceMemoryMut {
            base: GuestAddress(0),
            bytes: d,
        },
        t,
    )
}
fn sync(r: &mut Renderer, d: &mut [u8], t: &mut TextureChanges) -> wl_gpu::backend::Frame {
    r.synchronize_target(
        &mut SliceMemoryMut {
            base: GuestAddress(0),
            bytes: d,
        },
        t,
    )
    .unwrap()
    .unwrap()
}
#[test]
#[ignore = "requires actual GPU; independently assembled MME changes color mask"]
fn gpfifo_uploaded_mme_changes_real_raster_state_and_reuses_shader() {
    mme_raster_state(false);
}
#[test]
#[ignore = "requires actual GPU; MME reads guest-programmed scratch state"]
fn gpfifo_mme_scratch_read_changes_pixels_without_shader_recompilation() {
    mme_raster_state(true);
}
fn mme_raster_state(scratch: bool) {
    let (mut data, map, mut words, image) = fixture();
    let draw = words.split_off(words.len() - 8);
    for (method_id, value) in [(0x11c, 0), (0x120, 0), (0x114, 0)] {
        method(&mut words, method_id, value);
    }
    // Independent envyas: maddr 0x680; read $r2 0xd1a;
    // exit send (add $r2 0); nop;
    let code = if scratch {
        vec![0x01a00021, 0x03468215, 0x10c0, 0x11]
    } else {
        include_bytes!("fixtures/mme/state-mask.bin")
            .chunks_exact(4)
            .map(|b| u32::from_le_bytes(b.try_into().unwrap()))
            .collect()
    };
    for word in code {
        method(&mut words, 0x118, word);
    }
    if scratch {
        method(&mut words, 0x3468, 1);
    }
    method(&mut words, 0x3800, 1); // Red-only writes, produced by guest MME.
    words.extend_from_slice(&draw);
    put(&mut data, 0x10000, &words);
    let mut renderer = Renderer::new().unwrap();
    let mut channel = Channel::default();
    let mut changes = TextureChanges::new(1);
    let first = submit(
        &mut renderer,
        &mut channel,
        &entry(0xb0000, words.len()),
        &map,
        &mut data,
        &mut changes,
    )
    .unwrap();
    assert_eq!(first.draws_submitted, 1);
    let frame = sync(&mut renderer, &mut data, &mut changes);
    let mut masked = image.clone();
    for pixel in masked.chunks_exact_mut(4) {
        pixel[1..].copy_from_slice(&BG[1..]);
    }
    check(&frame.rgba, &masked, None, BG);
    let before = renderer.counters;
    let mut second = Vec::new();
    if scratch {
        method(&mut second, 0x3468, 0x1111);
    }
    method(&mut second, 0x3800, 0x1111); // Same uploaded code, different input.
    second.extend_from_slice(&draw);
    put(&mut data, 0x10000, &second);
    submit(
        &mut renderer,
        &mut channel,
        &entry(0xb0000, second.len()),
        &map,
        &mut data,
        &mut changes,
    )
    .unwrap();
    check(
        &sync(&mut renderer, &mut data, &mut changes).rgba,
        &image,
        None,
        BG,
    );
    assert_eq!(renderer.counters.shader_decodes, before.shader_decodes);
    assert_eq!(
        renderer.counters.shader_module_allocations,
        before.shader_module_allocations
    );
}
#[test]
#[ignore = "requires actual GPU; run explicitly"]
fn mapped_indexed_gpfifo_split_order_warm_alias_and_remap() {
    let (mut d, m, mut w, image) = fixture();
    let mut bindings = Vec::new();
    for (subchannel, class) in [(1, 0xb1c0), (2, 0xb0b5), (3, 0x902d), (4, 0xa140)] {
        bindings.extend([0x20010000 | (subchannel << 13), class]);
        bindings.extend([0x20010040 | (subchannel << 13), u32::MAX]);
    }
    w.splice(2..2, bindings);
    put(&mut d, 0x10000, &w);
    let mut r = Renderer::new().unwrap();
    println!("adapter={:?}", r.capabilities.adapter);
    let mut c = Channel::default();
    let mut t = TextureChanges::new(1);
    // Literal low/high words: one command word at B0000. Split SET_OBJECT data
    // across entries AND submission calls, not merely at packet boundaries.
    let first = [0, 0, 0x0b, 0, 0, 4, 0, 0];
    assert_eq!(
        submit(&mut r, &mut c, &first, &m, &mut d, &mut t)
            .unwrap()
            .words_consumed,
        1
    );
    assert!(c.fifo.finish().is_err());
    let mut entries = entry(0xb0004, 2).to_vec();
    entries.extend(entry(0xb000c, w.len() - 3));
    let p = submit(&mut r, &mut c, &entries, &m, &mut d, &mut t).unwrap();
    assert_eq!(p.draws_submitted, 1);
    assert_eq!(p.entries_completed, 2);
    c.finish().unwrap();
    assert!(d[RT..RT + SIZE].chunks_exact(4).all(|p| p == BG));
    assert_eq!(
        (
            r.counters.readback_operations,
            r.counters.synchronization_points
        ),
        (0, 0)
    );
    let f = sync(&mut r, &mut d, &mut t);
    check(&f.rgba, &image, None, BG);
    assert_eq!(f.rgba, d[RT..RT + SIZE]);
    let draw = [0x20010586, 4, 0x200105f7, 0, 0x200105f8, 3, 0x20010585, 0];
    put(&mut d, 0x11000, &draw);
    let warm = entry(0xc1000, draw.len()); // Same backing, another GPU VA.
    let before = r.counters;
    let region = Region::new(ALLOCATOR);
    let time = std::time::Instant::now();
    for _ in 0..32 {
        assert_eq!(
            submit(&mut r, &mut c, &warm, &m, &mut d, &mut t)
                .unwrap()
                .draws_submitted,
            1
        );
    }
    let elapsed = time.elapsed();
    let a = region.change();
    assert_eq!(
        (
            r.counters.upload_bytes,
            r.counters.pipeline_allocations,
            r.counters.shader_binary_bytes,
            r.counters.descriptor_reads,
            r.counters.readback_operations,
            r.counters.synchronization_points
        ),
        (
            before.upload_bytes,
            before.pipeline_allocations,
            before.shader_binary_bytes,
            before.descriptor_reads,
            before.readback_operations,
            before.synchronization_points
        )
    );
    assert_eq!(r.counters.gpfifo_read_calls - before.gpfifo_read_calls, 64); // draw + trailing END.
    assert_eq!(
        r.counters.gpfifo_command_bytes - before.gpfifo_command_bytes,
        1024
    );
    sync(&mut r, &mut d, &mut t);
    assert_eq!(
        r.counters.readback_operations - before.readback_operations,
        1
    );
    println!(
        "warm32 submit={elapsed:?},allocations={},reallocations={},command_bytes={},borrowed_span_bytes={},reads={},entry_decodes={},submissions={},readbacks={}",
        a.allocations,
        a.reallocations,
        r.counters.gpfifo_command_bytes - before.gpfifo_command_bytes,
        r.counters.gpfifo_borrowed_bytes - before.gpfifo_borrowed_bytes,
        r.counters.gpfifo_read_calls - before.gpfifo_read_calls,
        r.counters.gpfifo_entry_decodes - before.gpfifo_entry_decodes,
        r.counters.submissions - before.submissions,
        r.counters.readback_operations - before.readback_operations
    );
    // Mutable guest texture bytes change sampling without recompiling the shader.
    let mut green = image.clone();
    for pixel in green.chunks_exact_mut(4) {
        pixel.copy_from_slice(&[7, 211, 31, 255]);
    }
    let tiled = tile_mip(&green, 37, 19, wl_gpu::format::Format::Rgba8Unorm, 2);
    d[0x1000..0x1000 + tiled.len()].copy_from_slice(&tiled);
    t.mark_write(GuestAddress(0x1000), tiled.len()).unwrap();
    let pipeline_count = r.counters.pipeline_allocations;
    submit(&mut r, &mut c, &warm, &m, &mut d, &mut t).unwrap();
    check(&sync(&mut r, &mut d, &mut t).rgba, &green, None, BG);
    assert_eq!(r.counters.pipeline_allocations, pipeline_count);
    // Restore the independent original pattern for the ordered test.
    let tiled = tile_mip(&image, 37, 19, wl_gpu::format::Format::Rgba8Unorm, 2);
    d[0x1000..0x1000 + tiled.len()].copy_from_slice(&tiled);
    t.mark_write(GuestAddress(0x1000), tiled.len()).unwrap();
    // Actual ordered red then textured draws in a SINGLE entry, no caller shaders.
    let mut two = Vec::new();
    method(&mut two, 0x2144, 0x400);
    two.extend(draw);
    method(&mut two, 0x2144, 0);
    two.extend(draw);
    put(&mut d, 0x12000, &two);
    assert_eq!(
        submit(
            &mut r,
            &mut c,
            &entry(0xb2000, two.len()),
            &m,
            &mut d,
            &mut t
        )
        .unwrap()
        .draws_submitted,
        2
    );
    check(&sync(&mut r, &mut d, &mut t).rgba, &image, None, BG);
    // Replace SAME command VA mapping on next call: new backing selects quarters.
    let mut map = AddressMap::default();
    for (g, a, n) in [
        (0x50000, 0x4000, 0x1000),
        (0x70000, 0x3000, 72),
        (0x80000, 0x6000, 6),
        (0x90000, RT as u64, SIZE as u64),
        (0xb0000, 0x13000, 0x1000),
    ] {
        map.map(GpuAddress(g), GuestAddress(a), n).unwrap();
    }
    let mut quarters = Vec::new();
    method(&mut quarters, 0x2144, 0x100);
    quarters.extend(draw);
    put(&mut d, 0x13000, &quarters);
    submit(
        &mut r,
        &mut c,
        &entry(0xb0000, quarters.len()),
        &map,
        &mut d,
        &mut t,
    )
    .unwrap();
    check(
        &sync(&mut r, &mut d, &mut t).rgba,
        &image,
        Some([64, 128, 191, 255]),
        BG,
    );
}
struct RejectWrite<'a>(&'a mut [u8]);
impl GuestMemory for RejectWrite<'_> {
    fn read(&self, a: GuestAddress, n: usize) -> wl_gpu::Result<&[u8]> {
        let start =
            usize::try_from(a.0).map_err(|_| wl_gpu::Error("test address overflow".into()))?;
        let end = start
            .checked_add(n)
            .ok_or_else(|| wl_gpu::Error("test range overflow".into()))?;
        self.0
            .get(start..end)
            .ok_or_else(|| wl_gpu::Error("test short read".into()))
    }
}
impl GuestMemoryWrite for RejectWrite<'_> {
    fn validate_write(&self, a: GuestAddress, n: usize) -> wl_gpu::Result<()> {
        self.read(a, n).map(|_| ())
    }
    fn write(&mut self, _: GuestAddress, _: &[u8]) -> wl_gpu::Result<()> {
        Err(wl_gpu::Error("injected atomic rejection".into()))
    }
}
#[test]
#[ignore = "requires actual GPU; run explicitly"]
fn gpfifo_partial_fault_never_replays_draw_and_writeback_is_retryable() {
    let (mut d, m, w, image) = fixture();
    put(&mut d, 0x10000, &w);
    put(&mut d, 0x11000, &[0xc0000000]);
    let mut entries = entry(0xb0000, w.len()).to_vec();
    entries.extend(entry(0xb1000, 1));
    let mut r = Renderer::new().unwrap();
    let mut c = Channel::default();
    let mut t = TextureChanges::new(1);
    let e = submit(&mut r, &mut c, &entries, &m, &mut d, &mut t).unwrap_err();
    assert_eq!(e.phase, FaultPhase::Execute);
    assert_eq!(
        (e.progress.entries_completed, e.progress.draws_submitted),
        (1, 1)
    );
    assert!(c.is_faulted());
    let before = r.counters;
    assert!(submit(&mut r, &mut c, &entries, &m, &mut d, &mut t).is_err());
    assert_eq!(r.counters, before);
    assert!(
        r.synchronize_target(&mut RejectWrite(&mut d), &mut t)
            .is_err()
    );
    assert!(d[RT..RT + SIZE].chunks_exact(4).all(|p| p == BG));
    check(&sync(&mut r, &mut d, &mut t).rgba, &image, None, BG);
    assert!(
        r.synchronize_target(
            &mut SliceMemoryMut {
                base: GuestAddress(0),
                bytes: &mut d
            },
            &mut t
        )
        .unwrap()
        .is_none()
    );
    // Fetch failure in a later entry has the same prefix-preservation contract.
    let mut entries = entry(0xb0000, w.len()).to_vec();
    entries.extend(entry(0xdead000, 1));
    let e = submit(
        &mut r,
        &mut Channel::default(),
        &entries,
        &m,
        &mut d,
        &mut t,
    )
    .unwrap_err();
    assert_eq!(e.phase, FaultPhase::Fetch);
    assert_eq!(
        (e.progress.entries_completed, e.progress.draws_submitted),
        (1, 1)
    );
    check(&sync(&mut r, &mut d, &mut t).rgba, &image, None, BG);
}
#[test]
#[ignore = "requires actual GPU; run explicitly"]
fn gpfifo_command_alias_of_gpu_output_requires_visibility_before_fetch() {
    for high_flags in [[0, 0], [0x80000200, 0x80000200], [0x80000200, 0]] {
        let (mut d, m, mut w, image) = fixture();
        for b in &mut d[RT..RT + SIZE] {
            *b = 0;
        }
        method(&mut w, 0x2144, 0x100);
        w.extend([0x20010485, 0x40030000]);
        put(&mut d, 0x10000, &w);
        let mut entries = entry(0xb0000, w.len()).to_vec();
        entries.extend(entry(0x90000 + ((14 * 52 + 20) * 4), 1));
        // USER/SUBROUTINE/WAIT: neither this payload nor the next one may be
        // snapshotted before earlier GPU output has become coherent.
        for (descriptor, flags) in entries.chunks_exact_mut(8).zip(high_flags) {
            let high = u32::from_le_bytes(descriptor[4..].try_into().unwrap()) | flags;
            descriptor[4..].copy_from_slice(&high.to_le_bytes());
        }
        let mut r = Renderer::new().unwrap();
        let mut c = Channel::default();
        let mut t = TextureChanges::new(1);
        let e = submit(&mut r, &mut c, &entries, &m, &mut d, &mut t).unwrap_err();
        assert_eq!(e.phase, FaultPhase::Execute);
        assert_eq!(e.progress.draws_submitted, 2);
        assert!(e.cause.0.contains("FIFO opcode 7")); // GPU color FF BF 80 40, not stale RAM NOP.
        assert_eq!(
            (
                r.counters.coherence_barriers,
                r.counters.readback_operations
            ),
            (1, 1)
        );
        check(&d[RT..RT + SIZE], &image, Some([64, 128, 191, 255]), [0; 4]);
    }
}
#[test]
#[ignore = "requires actual GPU; run explicitly"]
fn gpfifo_preflight_fetch_and_engine_faults_are_distinct() {
    let (mut d, m, w, _) = fixture();
    put(&mut d, 0x10000, &w);
    let mut r = Renderer::new().unwrap();
    let mut t = TextureChanges::new(1);
    let mut bad = entry(0xb0000, w.len()).to_vec();
    bad.extend([0; 8]);
    let before = r.counters;
    let e = submit(&mut r, &mut Channel::default(), &bad, &m, &mut d, &mut t).unwrap_err();
    assert_eq!(e.phase, FaultPhase::Preflight);
    assert_eq!(e.progress, Progress::default());
    assert_eq!(r.counters.gpfifo_read_calls, before.gpfifo_read_calls);
    let e = submit(
        &mut r,
        &mut Channel::default(),
        &entry(0xdead000, 1),
        &m,
        &mut d,
        &mut t,
    )
    .unwrap_err();
    assert_eq!(e.phase, FaultPhase::Fetch);
    assert_eq!(e.progress.words_consumed, 0);
    let mut adjacent = AddressMap::default();
    adjacent
        .map(GpuAddress(0xb0000), GuestAddress(0x10000), 4)
        .unwrap();
    adjacent
        .map(GpuAddress(0xb0004), GuestAddress(0x10004), 4)
        .unwrap();
    let e = submit(
        &mut r,
        &mut Channel::default(),
        &entry(0xb0000, 2),
        &adjacent,
        &mut d,
        &mut t,
    )
    .unwrap_err();
    assert_eq!(e.phase, FaultPhase::Fetch);
    assert!(e.cause.0.contains("noncontiguous"));
    put(&mut d, 0x10000, &[0x20010000, 0xffff]);
    let e = submit(
        &mut r,
        &mut Channel::default(),
        &entry(0xb0000, 2),
        &m,
        &mut d,
        &mut t,
    )
    .unwrap_err();
    assert_eq!(e.phase, FaultPhase::Execute);
    assert_eq!(e.progress.words_consumed, 2);
    assert!(e.cause.0.contains("UnsupportedClass"));
    assert_eq!(r.counters.submissions, 0);
    let e = r
        .submit_gpfifo_deferred(
            &mut Channel::default(),
            &entry(0xb0000, 2),
            &m,
            &mut ShortRead(&mut d),
            &mut t,
        )
        .unwrap_err();
    assert_eq!(e.phase, FaultPhase::Fetch);
    assert_eq!(e.progress.words_consumed, 0);
    assert!(e.cause.0.contains("short guest GPFIFO"));
    // Zero heap allocations in the command acquisition/dispatch hot path,
    // excluding draws, error strings and driver/device setup.
    put(&mut d, 0x10000, &[0, 0]);
    let entries = entry(0xb0000, 2);
    let mut channel = Channel::default();
    let region = Region::new(ALLOCATOR);
    let start = std::time::Instant::now();
    for _ in 0..1000 {
        submit(&mut r, &mut channel, &entries, &m, &mut d, &mut t).unwrap();
    }
    let elapsed = start.elapsed();
    let a = region.change();
    assert_eq!((a.allocations, a.reallocations), (0, 0));
    println!("1000 NOP GPFIFO submissions: {elapsed:?},zero Rust allocations,8000 consumed bytes");
}
struct ShortRead<'a>(&'a mut [u8]);
impl GuestMemory for ShortRead<'_> {
    fn read(&self, a: GuestAddress, n: usize) -> wl_gpu::Result<&[u8]> {
        let start = a.0 as usize;
        self.0
            .get(start..start + n.saturating_sub(1))
            .ok_or_else(|| wl_gpu::Error("test range".into()))
    }
}
impl GuestMemoryWrite for ShortRead<'_> {
    fn validate_write(&self, _: GuestAddress, _: usize) -> wl_gpu::Result<()> {
        Ok(())
    }
    fn write(&mut self, _: GuestAddress, _: &[u8]) -> wl_gpu::Result<()> {
        Err(wl_gpu::Error("unexpected test write".into()))
    }
}

#[test]
#[ignore = "requires actual GPU; run explicitly"]
fn gpfifo_draw_overwrites_unread_suffix_of_its_own_entry() {
    let (mut data, mut map, mut words, _) = fixture();
    words.truncate(words.len() - 8); // Remove the indexed draw; retain guest state.
    method(&mut words, 0x2144, 0x100); // Select genuine quarters binary.
    words.extend([0x20010485, 0x40030000]); // Supported nonindexed B197 trigger.
    let start = 0x7800;
    let pixel = RT + (14 * 52 + 20) * 4;
    let end = pixel + 4;
    assert!(start + words.len() * 4 < RT);
    data[start..end].fill(0); // Unread suffix is initially all valid FIFO NOPs.
    put(&mut data, start, &words);
    map.map(
        GpuAddress(0xd0000),
        GuestAddress(start as u64),
        (end - start) as u64,
    )
    .unwrap();
    let mut renderer = Renderer::new().unwrap();
    let mut channel = Channel::default();
    let mut changes = TextureChanges::new(1);
    let fault = submit(
        &mut renderer,
        &mut channel,
        &entry(0xd0000, (end - start) / 4),
        &map,
        &mut data,
        &mut changes,
    )
    .unwrap_err();
    assert_eq!(fault.phase, FaultPhase::Execute);
    assert_eq!(
        (
            fault.progress.entries_completed,
            fault.progress.draws_submitted
        ),
        (0, 1)
    );
    assert!(fault.cause.0.contains("FIFO opcode 7"));
    assert_eq!(&data[pixel..pixel + 4], &[64, 128, 191, 255]);
    assert_eq!(
        (
            renderer.counters.coherence_barriers,
            renderer.counters.readback_operations
        ),
        (1, 1)
    );
    assert_eq!(renderer.counters.gpfifo_read_calls, 2);
    assert!(channel.is_faulted());
}
