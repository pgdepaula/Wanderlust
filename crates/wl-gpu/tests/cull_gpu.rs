#![cfg(feature = "wgpu-backend")]
mod common;
use common::{maxwell::*, *};
use stats_alloc::{INSTRUMENTED_SYSTEM, Region, StatsAlloc};
use std::{alloc::System, time::Instant};
#[global_allocator]
static ALLOCATOR: &StatsAlloc<System> = &INSTRUMENTED_SYSTEM;
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
fn check(data: &[u8], sy: f64, reversed: bool, ccw: bool, mode: u32) {
    let v = POS.map(|p| [23.5 + 23.5 * f64::from(p[0]), 14.5 + sy * f64::from(p[1])]);
    let area = edge(v[0], v[1], v[2]);
    let signed = if reversed { -area } else { area };
    let front = (signed < 0.) == ccw;
    let culled = match mode {
        0 => false,
        0x404 => front,
        0x405 => !front,
        _ => panic!("oracle mode"),
    };
    let mut inside = 0;
    for y in 0..H {
        for x in 0..W {
            let p = [x as f64 + 0.5, y as f64 + 0.5];
            let b = [
                edge(v[1], v[2], p) / area,
                edge(v[2], v[0], p) / area,
                edge(v[0], v[1], p) / area,
            ];
            if b.iter().any(|v| v.abs() < 0.005) {
                continue;
            }
            let color = if x < 47 && b.iter().all(|v| *v >= 0.) && !culled {
                inside += 1;
                [255, 0, 0, 255]
            } else {
                BG
            };
            assert_eq!(
                &data[RT + (y * W + x) * 4..RT + (y * W + x) * 4 + 4],
                &color,
                "{x},{y} sy{sy} reversed{reversed} ccw{ccw} mode{mode:x}"
            );
        }
    }
    assert_eq!(inside == 0, culled);
}
fn cleared_draw(sy: f32, ccw: bool, mode: u32) -> Vec<u32> {
    let mut w = Vec::new();
    for (m, v) in [(0x10f8, 0), (0xff4, 52 << 16), (0xff8, 29 << 16)] {
        method(&mut w, m, v);
    }
    w.push(0x20040360);
    w.extend(BG.map(|v| (f32::from(v) / 255.).to_bits()));
    method(&mut w, 0x19d0, 0x3c);
    method(&mut w, 0xff4, 47 << 16);
    method(&mut w, 0xa04, sy.to_bits());
    method(&mut w, 0x1918, u32::from(mode != 0));
    method(&mut w, 0x191c, if ccw { 0x901 } else { 0x900 });
    method(&mut w, 0x1920, if mode == 0 { 0x405 } else { mode });
    w.extend(draw());
    w
}
#[test]
#[ignore = "requires actual GPU; signed-area cull oracle, both viewport-Y signs and guest index windings"]
fn guest_cull_winding_viewport_and_pipeline_identity() {
    let (mut data, mut setup, _) = fixture();
    setup.truncate(setup.len() - 8);
    method(&mut setup, 0x2144, 0x400);
    let (mut d, c, _, b, _) = driver();
    let r = submit(&mut d, c, b, &mut data, &setup);
    assert!(r.fault.is_none());
    d.retire(r.token).unwrap();
    let mut prior = None;
    for sy in [-14.5f32, 14.5] {
        for reversed in [false, true] {
            for ccw in [false, true] {
                for mode in [0, 0x404, 0x405] {
                    let ids = if reversed { [0u16, 2, 1] } else { [0, 1, 2] };
                    let bytes: Vec<u8> = ids.into_iter().flat_map(u16::to_le_bytes).collect();
                    d.write_backing(b, 0x6000, &bytes, &mut mem(&mut data))
                        .unwrap();
                    let before = d.graphics_counters();
                    let r = submit(&mut d, c, b, &mut data, &cleared_draw(sy, ccw, mode));
                    assert!(r.fault.is_none(), "{:?}", r.fault);
                    assert_eq!(r.progress.draws_submitted, 1);
                    d.publish(c, &mut mem(&mut data)).unwrap();
                    check(&data, f64::from(sy), reversed, ccw, mode);
                    if prior == Some(sy) {
                        assert_eq!(
                            d.graphics_counters().shader_module_allocations,
                            before.shader_module_allocations
                        );
                    }
                    prior = Some(sy);
                    d.retire(r.token).unwrap();
                }
            }
        }
    }
    // An unchanged culling pipeline and buffers remain resident across deferred draws.
    let bytes: Vec<u8> = draw().iter().flat_map(|v| v.to_le_bytes()).collect();
    d.write_backing(b, 0x10000, &bytes, &mut mem(&mut data))
        .unwrap();
    let before = d.graphics_counters();
    let mut tokens = Vec::with_capacity(32);
    let region = Region::new(&INSTRUMENTED_SYSTEM);
    let start = Instant::now();
    for _ in 0..32 {
        let r = submit_existing(&mut d, c, &mut data, 8);
        assert!(r.fault.is_none());
        tokens.push(r.token);
    }
    let elapsed = start.elapsed();
    let stats = region.change();
    let after = d.graphics_counters();
    eprintln!("cull warm32 CPU {elapsed:?}, Rust {stats:?}");
    eprintln!(
        "cull warm32: vertex {} index {} uploads {} queues {} readbacks {} waits {} pipelines {} modules {}",
        after.vertex_upload_bytes - before.vertex_upload_bytes,
        after.index_upload_bytes - before.index_upload_bytes,
        after.upload_bytes - before.upload_bytes,
        after.submissions - before.submissions,
        after.readback_operations - before.readback_operations,
        after.synchronization_points - before.synchronization_points,
        after.pipeline_allocations - before.pipeline_allocations,
        after.shader_module_allocations - before.shader_module_allocations
    );
    assert_eq!(after.submissions - before.submissions, 32);
    for (a, b) in [
        (after.pipeline_allocations, before.pipeline_allocations),
        (
            after.shader_module_allocations,
            before.shader_module_allocations,
        ),
        (after.upload_bytes, before.upload_bytes),
        (after.index_upload_bytes, before.index_upload_bytes),
        (after.vertex_upload_bytes, before.vertex_upload_bytes),
        (after.readback_operations, before.readback_operations),
        (after.synchronization_points, before.synchronization_points),
    ] {
        assert_eq!(a, b);
    }
    d.publish(c, &mut mem(&mut data)).unwrap();
    check(&data, 14.5, true, true, 0x405);
    for t in tokens {
        d.retire(t).unwrap();
    }
    // Degenerate indexed triangles cover no pixels even with culling disabled.
    d.write_backing(b, 0x6000, &[0; 6], &mut mem(&mut data))
        .unwrap();
    let r = submit(&mut d, c, b, &mut data, &cleared_draw(-14.5, true, 0));
    assert!(r.fault.is_none());
    d.publish(c, &mut mem(&mut data)).unwrap();
    assert_eq!(&data[RT..RT + SIZE], &BG.repeat(W * H));
    d.close(c, &mut mem(&mut data)).unwrap();
}
#[test]
#[ignore = "requires actual GPU; incomplete new cull state cannot execute a stale earlier pipeline"]
fn invalid_cull_state_preserves_prior_draw_without_replay() {
    let (mut data, mut w, _) = fixture();
    let end = w.split_off(w.len() - 8);
    method(&mut w, 0x2144, 0x400);
    w.extend(end);
    let (mut d, c, _, b, _) = driver();
    let r = submit(&mut d, c, b, &mut data, &w);
    assert!(r.fault.is_none());
    let before = d.graphics_counters();
    let mut w = Vec::new();
    method(&mut w, 0x1918, 1);
    w.extend(draw());
    let r = submit(&mut d, c, b, &mut data, &w);
    assert!(r.fault.is_some());
    assert_eq!(r.progress.draws_submitted, 0);
    assert_eq!(d.graphics_counters().draw_commands, before.draw_commands);
    assert_eq!(
        d.graphics_counters().pipeline_allocations,
        before.pipeline_allocations
    );
    d.publish(c, &mut mem(&mut data)).unwrap();
    check(&data, -14.5, false, true, 0);
    d.close(c, &mut mem(&mut data)).unwrap();
}

#[test]
#[ignore = "requires actual GPU; frequent geometry observations survive bounded dirty-journal rollover"]
fn geometry_journal_rollover_keeps_residency_and_rejects_lost_history() {
    let (mut data, mut setup, _) = fixture();
    setup.truncate(setup.len() - 8);
    method(&mut setup, 0x2144, 0x400);
    setup.extend(cleared_draw(-14.5, true, 0x405));
    let (mut d, c, _, b, _) = driver();
    let first = submit(&mut d, c, b, &mut data, &setup);
    assert!(first.fault.is_none());
    d.publish(c, &mut mem(&mut data)).unwrap();
    d.retire(first.token).unwrap();
    let before = d.graphics_counters();
    for _ in 0..128 {
        // Disjoint command writes, GPU invalidations and publication roll over
        // the 64-write journal repeatedly. Geometry is observed on every draw.
        let r = submit(&mut d, c, b, &mut data, &cleared_draw(-14.5, true, 0x405));
        assert!(r.fault.is_none(), "{:?}", r.fault);
        d.publish(c, &mut mem(&mut data)).unwrap();
        check(&data, -14.5, false, true, 0x405);
        d.retire(r.token).unwrap();
    }
    let after = d.graphics_counters();
    assert_eq!(after.vertex_upload_bytes, before.vertex_upload_bytes);
    assert_eq!(after.index_upload_bytes, before.index_upload_bytes);
    assert_eq!(after.vertex_shader_decodes, before.vertex_shader_decodes);
    assert_eq!(after.pipeline_allocations, before.pipeline_allocations);
    // Overlapping index writes still invalidate min/max and the GPU index data.
    d.write_backing(b, 0x6000, &[0, 0, 2, 0, 1, 0], &mut mem(&mut data))
        .unwrap();
    let r = submit(&mut d, c, b, &mut data, &cleared_draw(-14.5, true, 0x405));
    assert!(r.fault.is_none());
    d.publish(c, &mut mem(&mut data)).unwrap();
    check(&data, -14.5, true, true, 0x405);
    d.retire(r.token).unwrap();
    assert_eq!(
        d.graphics_counters().index_upload_bytes - after.index_upload_bytes,
        8
    );
    // A cache not observed during >64 writes cannot prove that an evicted write
    // was disjoint. It must reread, rescan and upload, even if contents match.
    let before = d.graphics_counters();
    d.write_backing(b, 0x6000, &[0, 0, 1, 0, 2, 0], &mut mem(&mut data))
        .unwrap();
    for n in 0..65 {
        d.write_backing(b, 0x1f000 + n, &[0], &mut mem(&mut data))
            .unwrap();
    }
    let r = submit(&mut d, c, b, &mut data, &cleared_draw(-14.5, true, 0x405));
    assert!(r.fault.is_none());
    d.publish(c, &mut mem(&mut data)).unwrap();
    check(&data, -14.5, false, true, 0x405);
    d.retire(r.token).unwrap();
    let after = d.graphics_counters();
    assert_eq!(after.vertex_upload_bytes - before.vertex_upload_bytes, 72);
    assert_eq!(after.index_upload_bytes - before.index_upload_bytes, 8);
    // CPU vertex writes cannot be hidden by advancing the observation cursor.
    d.write_backing(b, 0x3000, &[0; 72], &mut mem(&mut data))
        .unwrap();
    let r = submit(&mut d, c, b, &mut data, &cleared_draw(-14.5, true, 0));
    assert!(r.fault.is_none());
    d.publish(c, &mut mem(&mut data)).unwrap();
    assert_eq!(&data[RT..RT + SIZE], &BG.repeat(W * H));
    assert_eq!(
        d.graphics_counters().vertex_upload_bytes - after.vertex_upload_bytes,
        72
    );
    d.close(c, &mut mem(&mut data)).unwrap();
}
