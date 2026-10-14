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

fn color(n: usize) -> [u8; 4] {
    [
        (n * 3 + 1) as u8,
        (255 - n * 2) as u8,
        (n * 4 + 2) as u8,
        255,
    ]
}
fn constants(d: &mut GpuDriver, b: BackingId, data: &mut [u8], n: usize) {
    let bytes: Vec<u8> = color(n)
        .into_iter()
        .flat_map(|x| (f32::from(x) / 255.).to_le_bytes())
        .collect();
    d.write_backing(b, 0x50, &bytes, &mut mem(data)).unwrap();
}
fn tile(w: &mut Vec<u32>, n: usize) {
    let x = 10 + ((n / 2) % 8) as u32;
    let y = 10 + (n / 16) as u32;
    method(w, 0xe04, ((x + 1) << 16) | x);
    method(w, 0xe08, ((y + 1) << 16) | y);
}
#[test]
#[ignore = "requires actual GPU;64 mutable CB/scissor submissions include32 ordered overlapping pairs with one final readback"]
fn ordered_mutable_constants_survive_journal_rotation_and_completion_capacity() {
    let (mut data, mut w, _) = fixture();
    // Explicit guest vertex data describes a large clipped triangle covering the
    // viewport; every test tile is well away from its primitive boundaries.
    // This still executes the independently assembled guest ALD/AST vertex shader.
    for (n, p) in [[-1f32, -1.], [3., -1.], [-1., 3.]].iter().enumerate() {
        for (k, value) in [p[0], p[1], 0., 1., 0., 0.].into_iter().enumerate() {
            data[0x3000 + n * 24 + k * 4..0x3004 + n * 24 + k * 4]
                .copy_from_slice(&value.to_le_bytes());
        }
    }
    let end = w.split_off(w.len() - 8);
    for (m, v) in [(0x2144, 0x500), (0x214c, 5), (0x2380, 32)] {
        method(&mut w, m, v);
    }
    tile(&mut w, 0);
    w.extend(end);
    let (mut d, c, _, b, _) = driver();
    constants(&mut d, b, &mut data, 0);
    let cold = submit(&mut d, c, b, &mut data, &w);
    assert!(cold.fault.is_none(), "{:?}", cold.fault);
    let mut tokens = Vec::with_capacity(64);
    tokens.push(cold.token);
    let before = d.graphics_counters();
    let driver_before = d.counters;
    let start = std::time::Instant::now();
    let region = Region::new(ALLOCATOR);
    for n in 1..64 {
        constants(&mut d, b, &mut data, n);
        let mut w = Vec::with_capacity(12);
        tile(&mut w, n);
        w.extend(draw());
        let r = submit(&mut d, c, b, &mut data, &w);
        assert!(r.fault.is_none(), "tile{n}: {:?}", r.fault);
        tokens.push(r.token);
    }
    let allocations = region.change();
    let elapsed = start.elapsed();
    let after = d.graphics_counters();
    assert_eq!(after.draw_commands - before.draw_commands, 63);
    assert_eq!(
        after.constant_guest_bytes - before.constant_guest_bytes,
        1008
    );
    assert_eq!(
        after.constant_upload_bytes - before.constant_upload_bytes,
        756
    );
    assert_eq!(after.submissions - before.submissions, 63);
    assert_eq!(after.render_target_reuses - before.render_target_reuses, 63);
    assert_eq!(after.render_target_allocations, 1);
    assert_eq!(
        after.render_target_upload_bytes,
        before.render_target_upload_bytes
    );
    assert_eq!(after.shader_decodes, before.shader_decodes);
    assert_eq!(after.shader_binary_bytes, before.shader_binary_bytes);
    assert_eq!(after.vertex_shader_bytes, before.vertex_shader_bytes);
    assert_eq!(after.pipeline_allocations, before.pipeline_allocations);
    assert_eq!(
        after.shader_module_allocations,
        before.shader_module_allocations
    );
    assert_eq!(after.vertex_upload_bytes, before.vertex_upload_bytes);
    assert_eq!(after.index_upload_bytes, before.index_upload_bytes);
    assert_eq!(after.texture_uploads, before.texture_uploads);
    assert_eq!(after.readback_operations, 0);
    assert_eq!(after.synchronization_points, 0);
    assert_eq!(d.counters.completion_waits, driver_before.completion_waits);
    assert_eq!(d.counters.accepted, 64);
    assert!(d.has_pending_output());
    for p in data[RT..RT + SIZE].chunks_exact(4) {
        assert_eq!(p, BG);
    }
    //64 outstanding channel records is a real separate ownership limit. The
    //65th request fails preflight, preserving all previously accepted GPU work.
    let mut h = [0u8; 24];
    h[8..12].copy_from_slice(&1u32.to_le_bytes());
    h[12..16].copy_from_slice(&4u32.to_le_bytes());
    let reject = d.submit(
        c,
        KICKOFF_PB,
        &h,
        Some(&entry(0xb0000, 12)),
        &mut mem(&mut data),
    );
    assert!(
        reject
            .unwrap_err()
            .to_string()
            .contains("completion capacity")
    );
    for p in data[RT..RT + SIZE].chunks_exact(4) {
        assert_eq!(p, BG);
    }
    assert_eq!(d.graphics_counters(), after);
    assert_eq!(d.counters.accepted, 64);
    assert_eq!(d.counters.rejected, driver_before.rejected + 1);
    let frame = d.publish(c, &mut mem(&mut data)).unwrap().unwrap();
    // Each successive pair targets one pixel: odd color must overwrite even
    // color. Reversing pair order or reusing later CB data changes final output.
    // Every other byte remains background, including unused pitch columns.
    let mut expected = BG.repeat(W * H);
    for cell in 0..32 {
        let x = 10 + cell % 8;
        let y = 10 + cell / 8;
        let a = (y * W + x) * 4;
        expected[a..a + 4].copy_from_slice(&color(cell * 2 + 1));
    }
    assert_eq!(frame.rgba, expected);
    assert_eq!(&data[RT..RT + SIZE], expected);
    let done = d.graphics_counters();
    assert_eq!(done.draw_commands, 64);
    assert_eq!(done.readback_operations, 1);
    assert_eq!(done.synchronization_points, 1);
    assert_eq!(done.guest_framebuffer_write_bytes, SIZE as u64);
    for token in tokens {
        let status = d.query(token).unwrap();
        assert!(status.host_completed && status.guest_visible && !status.faulted);
        d.retire(token).unwrap();
    }
    eprintln!(
        "Mutable63 warm submission {elapsed:?}, Rust {allocations:?}; CB guest={} upload={}, queues={}, final readback={} wait=1; before {before:?}; after {done:?}",
        after.constant_guest_bytes - before.constant_guest_bytes,
        after.constant_upload_bytes - before.constant_upload_bytes,
        after.submissions - before.submissions,
        done.readback_bytes - after.readback_bytes
    );
    d.close(c, &mut mem(&mut data)).unwrap();
}
