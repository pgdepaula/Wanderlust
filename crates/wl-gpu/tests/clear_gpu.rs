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
fn check(data: &[u8], color: [u8; 4], background: [u8; 4]) {
    let v = POS.map(|p| p.map(f64::from));
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
                background
            };
            assert_eq!(
                &data[RT + (y * W + x) * 4..RT + (y * W + x) * 4 + 4],
                &expected,
                "{x},{y}"
            );
        }
    }
}
const Z: usize = 0x12000;
fn clear_setup(width: u32, height: u32, bh: Option<u8>) -> Vec<u32> {
    let mut w = vec![0x20010000, 0xb197];
    for (m, v) in [
        (0x138, 1),
        (0x15d0, 0),
        (0x121c, 1),
        (0x10f8, 0),
        (0xff4, width << 16),
        (0xff8, height << 16),
        (0x1a00, 0),
        (0x1360, 1),
    ] {
        method(&mut w, m, v);
    }
    for (n, v) in [
        0,
        0x90000,
        if bh.is_some() { width } else { width * 4 },
        height,
        0xd5,
        bh.map_or(0x1000, |n| u32::from(n) << 4),
        1,
        0,
        0,
    ]
    .into_iter()
    .enumerate()
    {
        method(&mut w, 0x800 + n as u32 * 4, v);
    }
    w
}
fn clear_values(w: &mut Vec<u32>, v: [f32; 4]) {
    w.push(0x20040360);
    w.extend(v.map(f32::to_bits));
}
fn z_setup(w: &mut Vec<u32>, width: u32, height: u32, value: f32) {
    for (m, v) in [
        (0xfe0, 0),
        (0xfe4, 0xd0000),
        (0xfe8, 0xa),
        (0xfec, 0x10),
        (0xff0, 0),
        (0x1228, width),
        (0x122c, height),
        (0x1230, 1),
        (0x179c, 0),
        (0x1538, 1),
        (0x12cc, 0),
        (0x12e8, 0),
        (0xd90, value.to_bits()),
    ] {
        method(w, m, v);
    }
}
fn storage(width: usize, height: usize, bh: Option<u8>) -> usize {
    bh.map_or(width * height * 4, |n| {
        (width * 4).div_ceil(64) * height.div_ceil(8 * (1 << n)) * 512 * (1 << n)
    })
}
fn texel(width: usize, x: usize, y: usize, bh: Option<u8>) -> usize {
    bh.map_or((y * width + x) * 4, |n| {
        gob_address(x * 4, y, width * 4, 1 << n)
    })
}
fn expected_color(width: usize, height: usize, bh: Option<u8>, v: [u8; 4]) -> Vec<u8> {
    let mut out = vec![0xa5; storage(width, height, bh)];
    for y in 0..height {
        for x in 0..width {
            let a = texel(width, x, y, bh);
            out[a..a + 4].copy_from_slice(&v);
        }
    }
    out
}
fn expected_depth(width: usize, height: usize, value: f32) -> Vec<u8> {
    let mut out = vec![0xa5; storage(width, height, Some(1))];
    for y in 0..height {
        for x in 0..width {
            let a = texel(width, x, y, Some(1));
            out[a..a + 4].copy_from_slice(&value.to_le_bytes());
        }
    }
    out
}
struct FailAt<'a>(&'a mut [u8], u64);
impl GuestMemory for FailAt<'_> {
    fn read(&self, a: GuestAddress, n: usize) -> wl_gpu::Result<&[u8]> {
        let start = usize::try_from(a.0).map_err(|_| wl_gpu::Error("address".into()))?;
        self.0
            .get(
                start
                    ..start
                        .checked_add(n)
                        .ok_or_else(|| wl_gpu::Error("size".into()))?,
            )
            .ok_or_else(|| wl_gpu::Error("range".into()))
    }
}
impl GuestMemoryWrite for FailAt<'_> {
    fn validate_write(&self, a: GuestAddress, n: usize) -> wl_gpu::Result<()> {
        self.read(a, n).map(|_| ())
    }
    fn write(&mut self, a: GuestAddress, v: &[u8]) -> wl_gpu::Result<()> {
        self.validate_write(a, v.len())?;
        if a.0 == self.1 {
            return Err(wl_gpu::Error("injected atomic publication failure".into()));
        }
        self.0[a.0 as usize..a.0 as usize + v.len()].copy_from_slice(v);
        Ok(())
    }
}
#[test]
#[ignore = "requires actual GPU; hardware clear without guest shaders, storage padding and retry"]
fn color_clear_storage_cpu_ordering_warm_and_atomic_retry() {
    for (width, height, bh) in [(7usize, 9usize, None), (7, 9, Some(0)), (52, 29, Some(1))] {
        let mut data = vec![0xa5; 0x20000];
        let (mut d, c, _, b, _) = driver();
        let before = d.graphics_counters();
        let prior = texel(width, 1, 1, bh);
        d.write_backing(b, (RT + prior) as u64, &[1, 2, 3, 4], &mut mem(&mut data))
            .unwrap();
        let mut w = clear_setup(width as u32, height as u32, bh);
        clear_values(&mut w, [0.25, 0.5, 0.75, 1.]);
        method(&mut w, 0x19d0, 0x3c);
        let report = submit(&mut d, c, b, &mut data, &w);
        assert!(report.fault.is_none(), "{:?}", report.fault);
        assert_eq!(report.progress.draws_submitted, 0);
        assert_eq!(&data[RT + prior..RT + prior + 4], &[1, 2, 3, 4]);
        let mut tokens = Vec::with_capacity(33);
        tokens.push(report.token);
        let warm = d.graphics_counters();
        let mut again = Vec::new();
        method(&mut again, 0x19d0, 0x3c);
        let bytes: Vec<u8> = again.iter().flat_map(|v| v.to_le_bytes()).collect();
        d.write_backing(b, 0x10000, &bytes, &mut mem(&mut data))
            .unwrap();
        let region = Region::new(&INSTRUMENTED_SYSTEM);
        let start = Instant::now();
        for _ in 0..32 {
            let r = submit_existing(&mut d, c, &mut data, again.len());
            assert!(r.fault.is_none());
            tokens.push(r.token);
        }
        eprintln!(
            "Clear warm32 {width}x{height} BH{bh:?} {:?}, Rust {:?}",
            start.elapsed(),
            region.change()
        );
        let after = d.graphics_counters();
        assert_eq!(after.submissions - warm.submissions, 32);
        assert_eq!(after.clear_commands - warm.clear_commands, 32);
        assert_eq!(after.upload_bytes, before.upload_bytes);
        assert_eq!(after.readback_operations, before.readback_operations);
        assert_eq!(after.synchronization_points, before.synchronization_points);
        assert_eq!(
            after.shader_module_allocations,
            before.shader_module_allocations
        );
        assert_eq!(after.pipeline_allocations, before.pipeline_allocations);
        d.write_backing(b, (RT + prior) as u64, &[9, 8, 7, 6], &mut mem(&mut data))
            .unwrap();
        let mut expected = expected_color(width, height, bh, [64, 128, 191, 255]);
        expected[prior..prior + 4].copy_from_slice(&[9, 8, 7, 6]);
        if bh.is_some() {
            let pad = expected.len() - 1;
            d.write_backing(b, (RT + pad) as u64, &[0x5c], &mut mem(&mut data))
                .unwrap();
            expected[pad] = 0x5c;
        }
        let saved = data[RT..RT + expected.len()].to_vec();
        assert!(d.publish(c, &mut FailAt(&mut data, RT as u64)).is_err());
        assert_eq!(&data[RT..RT + saved.len()], &saved);
        assert_eq!(d.graphics_counters().clear_commands, 33);
        d.publish(c, &mut mem(&mut data)).unwrap();
        assert_eq!(&data[RT..RT + expected.len()], &expected);
        assert_eq!(d.graphics_counters().clear_commands, 33);
        for t in tokens {
            d.retire(t).unwrap();
        }
        eprintln!(
            "Clear {width}x{height} BH{bh:?} counters {:?}",
            d.graphics_counters()
        );
        d.close(c, &mut mem(&mut data)).unwrap();
    }
}
#[test]
#[ignore = "requires actual GPU; depth-only clear and preservation of the other pending attachment"]
fn depth_color_clear_order_and_independent_pending_visibility() {
    let mut data = vec![0xa5; 0x20000];
    let (mut d, c, _, b, _) = driver();
    let mut w = vec![0x20010000, 0xb197];
    for (m, v) in [
        (0x138, 1),
        (0x15d0, 0),
        (0x10f8, 0),
        (0xff4, 7 << 16),
        (0xff8, 9 << 16),
    ] {
        method(&mut w, m, v);
    }
    z_setup(&mut w, 7, 9, 0.25);
    method(&mut w, 0x19d0, 1);
    let r = submit(&mut d, c, b, &mut data, &w);
    assert!(r.fault.is_none(), "{:?}", r.fault);
    assert_eq!(&data[Z..Z + 4], &[0xa5; 4]);
    let reads = d.graphics_counters().readback_operations;
    let status = d.wait(r.token).unwrap();
    assert!(status.host_completed && !status.guest_visible);
    assert_eq!(d.graphics_counters().readback_operations, reads);
    assert_eq!(&data[Z..Z + 4], &[0xa5; 4]);
    // CPU-after-depth patch must survive a color-only clear and later publication.
    let a = texel(7, 2, 3, Some(1));
    d.write_backing(
        b,
        (Z + a) as u64,
        &0.5f32.to_le_bytes(),
        &mut mem(&mut data),
    )
    .unwrap();
    let mut w = clear_setup(7, 9, Some(1));
    clear_values(&mut w, [0., 1., 0., 1.]);
    method(&mut w, 0x19d0, 0x3c);
    assert!(submit(&mut d, c, b, &mut data, &w).fault.is_none());
    let size = storage(7, 9, Some(1));
    let saved = data[RT..RT + size].to_vec();
    assert!(d.publish(c, &mut FailAt(&mut data, RT as u64)).is_err());
    let mut expected = expected_depth(7, 9, 0.25);
    expected[a..a + 4].copy_from_slice(&0.5f32.to_le_bytes());
    assert_eq!(&data[Z..Z + size], &expected);
    assert_eq!(&data[RT..RT + size], &saved);
    let bytes = d.graphics_counters().depth_write_bytes;
    d.publish(c, &mut mem(&mut data)).unwrap();
    assert_eq!(d.graphics_counters().depth_write_bytes, bytes);
    assert_eq!(
        &data[RT..RT + size],
        &expected_color(7, 9, Some(1), [0, 255, 0, 255])
    );
    // The inverse selection preserves color output pending while depth is replaced.
    let mut w = Vec::new();
    clear_values(&mut w, [0., 0., 1., 1.]);
    method(&mut w, 0x19d0, 0x3c);
    method(&mut w, 0xd90, 0.75f32.to_bits());
    method(&mut w, 0x19d0, 1);
    assert!(submit(&mut d, c, b, &mut data, &w).fault.is_none());
    d.publish(c, &mut mem(&mut data)).unwrap();
    assert_eq!(
        &data[RT..RT + size],
        &expected_color(7, 9, Some(1), [0, 0, 255, 255])
    );
    let mut expected = expected_depth(7, 9, 0.75);
    expected[a..a + 4].copy_from_slice(&0.75f32.to_le_bytes());
    assert_eq!(&data[Z..Z + size], &expected);
    // Depth-only visibility does not produce a color Frame.
    let mut w = Vec::new();
    method(&mut w, 0xd90, 0.25f32.to_bits());
    method(&mut w, 0x19d0, 1);
    assert!(submit(&mut d, c, b, &mut data, &w).fault.is_none());
    let barriers = d.counters.visibility_barriers;
    assert!(d.publish(c, &mut mem(&mut data)).unwrap().is_none());
    assert_eq!(d.counters.visibility_barriers, barriers + 1);
    assert!(d.publish(c, &mut mem(&mut data)).unwrap().is_none());
    assert_eq!(d.counters.visibility_barriers, barriers + 1);
    assert_eq!(&data[Z..Z + size], &expected_depth(7, 9, 0.25));
    // Aliased color selection and pending depth survive channel-close publication.
    let allocations = d.graphics_counters().render_target_allocations;
    let mut w = Vec::new();
    method(&mut w, 0x804, 0xa0000);
    clear_values(&mut w, [0., 1., 0., 1.]);
    method(&mut w, 0x19d0, 0x3c);
    method(&mut w, 0xd90, 0.75f32.to_bits());
    method(&mut w, 0x19d0, 1);
    assert!(submit(&mut d, c, b, &mut data, &w).fault.is_none());
    assert_eq!(d.graphics_counters().render_target_allocations, allocations);
    d.close(c, &mut mem(&mut data)).unwrap();
    assert_eq!(
        &data[RT..RT + size],
        &expected_color(7, 9, Some(1), [0, 255, 0, 255])
    );
    assert_eq!(&data[Z..Z + size], &expected_depth(7, 9, 0.75));
}
#[test]
#[ignore = "requires actual GPU; clear/draw ordering on real persistent color and ZF32 attachments"]
fn clear_draw_depth_testing_and_clear_selection_order() {
    let (mut data, mut setup, _) = fixture();
    data[Z..Z + 8192].fill(0xa5);
    let (mut d, c, _, b, _) = driver();
    let mut w = clear_setup(52, 29, None);
    clear_values(&mut w, BG.map(|v| f32::from(v) / 255.));
    z_setup(&mut w, 52, 29, 0.25);
    method(&mut w, 0x19d0, 0x3d);
    assert!(submit(&mut d, c, b, &mut data, &w).fault.is_none());
    let end = setup.split_off(setup.len() - 8);
    method(&mut setup, 0x2144, 0x400);
    z_setup(&mut setup, 52, 29, 0.25);
    for (m, v) in [(0x12cc, 1), (0x12e8, 1), (0x130c, 0x201)] {
        method(&mut setup, m, v);
    }
    setup.extend(end);
    let report = submit(&mut d, c, b, &mut data, &setup);
    assert!(report.fault.is_none(), "{:?}", report.fault);
    d.publish(c, &mut mem(&mut data)).unwrap();
    assert_eq!(&data[RT..RT + SIZE], &expected_color(52, 29, None, BG));
    assert_eq!(&data[Z..Z + 8192], &expected_depth(52, 29, 0.25));
    assert_eq!(d.graphics_counters().render_target_upload_bytes, 0);
    assert_eq!(d.graphics_counters().depth_upload_bytes, 0);
    // A draw writes Z=.5, then color-only clear supersedes an intervening CPU patch.
    let mut w = Vec::new();
    method(&mut w, 0x130c, 0x207);
    w.extend(draw());
    assert!(submit(&mut d, c, b, &mut data, &w).fault.is_none());
    let inside = RT + (16 * W + 19) * 4;
    d.write_backing(b, inside as u64, &[1, 2, 3, 4], &mut mem(&mut data))
        .unwrap();
    let mut w = Vec::new();
    method(&mut w, 0xff4, 52 << 16);
    method(&mut w, 0x1a00, 0);
    method(&mut w, 0x1360, 1);
    clear_values(&mut w, [0., 0., 1., 1.]);
    method(&mut w, 0x19d0, 0x3c);
    assert!(submit(&mut d, c, b, &mut data, &w).fault.is_none());
    d.publish(c, &mut mem(&mut data)).unwrap();
    assert_eq!(
        &data[RT..RT + SIZE],
        &expected_color(52, 29, None, [0, 0, 255, 255])
    );
    let zi = Z + texel(52, 19, 16, Some(1));
    assert_eq!(&data[zi..zi + 4], &0.5f32.to_le_bytes());
    assert_eq!(&data[Z..Z + 4], &0.25f32.to_le_bytes());
    // Depth-only clear changes no pending color pixels and ignores depth-write state.
    let mut w = Vec::new();
    method(&mut w, 0xff4, 47 << 16);
    method(&mut w, 0x1a00, 0x1111);
    method(&mut w, 0x1360, 0);
    w.extend(draw());
    method(&mut w, 0xff4, 52 << 16);
    method(&mut w, 0x12cc, 0);
    method(&mut w, 0x12e8, 0);
    method(&mut w, 0xd90, 0.75f32.to_bits());
    method(&mut w, 0x19d0, 1);
    assert!(submit(&mut d, c, b, &mut data, &w).fault.is_none());
    d.publish(c, &mut mem(&mut data)).unwrap();
    check(&data, [255, 0, 0, 255], [0, 0, 255, 255]);
    assert_eq!(&data[Z..Z + 8192], &expected_depth(52, 29, 0.75));
    assert_eq!(d.graphics_counters().draw_commands, 3);
    assert_eq!(d.graphics_counters().clear_commands, 3);
    d.close(c, &mut mem(&mut data)).unwrap();
}
#[test]
#[ignore = "requires actual GPU; later channel faults preserve earlier clears without replay"]
fn clear_faults_bounds_and_close_preserve_pending_output() {
    let mut data = vec![0xa5; 0x20000];
    let (mut d, c, space, b, mapping) = driver();
    let mut w = clear_setup(7, 9, Some(1));
    clear_values(&mut w, [1., 0., 0., 1.]);
    method(&mut w, 0x19d0, 0x3c);
    method(&mut w, 0x19d0, 4);
    let r = submit(&mut d, c, b, &mut data, &w);
    assert!(r.fault.is_some());
    assert_eq!(r.progress.draws_submitted, 0);
    assert_eq!(d.graphics_counters().clear_commands, 1);
    assert!(d.unmap(space, mapping).is_err());
    let mut h = [0u8; 24];
    h[8..12].copy_from_slice(&1u32.to_le_bytes());
    h[12..16].copy_from_slice(&4u32.to_le_bytes());
    assert!(
        d.submit(
            c,
            KICKOFF_PB,
            &h,
            Some(&entry(0xb0000, w.len())),
            &mut mem(&mut data)
        )
        .is_err()
    );
    assert_eq!(d.graphics_counters().clear_commands, 1);
    // close drains the earlier clear even though the channel fault is latched.
    d.close(c, &mut mem(&mut data)).unwrap();
    let expected = expected_color(7, 9, Some(1), [255, 0, 0, 255]);
    assert_eq!(&data[RT..RT + expected.len()], &expected);
    for (m, v) in [
        (0x0804, 0xf0000),
        (0x0808, 0x10000),
        (0x0ff4, 6 << 16),
        (0x0d80, 0x7fc00000),
        (0x10f8, 0x100),
    ] {
        let mut data = vec![0xa5; 0x20000];
        let (mut d, c, _, b, _) = driver();
        let mut w = clear_setup(7, 9, None);
        clear_values(&mut w, [1., 0., 0., 1.]);
        method(&mut w, m, v);
        method(&mut w, 0x19d0, 0x3c);
        assert!(submit(&mut d, c, b, &mut data, &w).fault.is_some());
        assert_eq!(d.graphics_counters().clear_commands, 0);
        assert!(data[RT..RT + 252].iter().all(|v| *v == 0xa5));
        d.close(c, &mut mem(&mut data)).unwrap();
    }
}
#[test]
#[ignore = "requires actual GPU; unread command suffix aliases a pending clear and must be reacquired"]
fn clear_overwrites_unread_gpfifo_suffix_before_reacquisition() {
    let mut data = vec![0xa5; 0x20000];
    let (mut d, c, _, b, _) = driver();
    let mut w = clear_setup(7, 9, None);
    clear_values(&mut w, [0.; 4]);
    method(&mut w, 0x19d0, 0x3c);
    method(&mut w, 0x19d0, 4);
    let bytes: Vec<u8> = w.iter().flat_map(|v| v.to_le_bytes()).collect();
    assert!(bytes.len() < 252);
    d.write_backing(b, RT as u64, &bytes, &mut mem(&mut data))
        .unwrap();
    let mut h = [0u8; 24];
    h[8..12].copy_from_slice(&1u32.to_le_bytes());
    h[12..16].copy_from_slice(&4u32.to_le_bytes());
    let report = d
        .submit(
            c,
            KICKOFF_PB,
            &h,
            Some(&entry(0x90000, w.len())),
            &mut mem(&mut data),
        )
        .unwrap();
    assert!(report.fault.is_none(), "{:?}", report.fault);
    assert_eq!(report.progress.words_consumed, w.len() as u64);
    assert_eq!(d.graphics_counters().clear_commands, 1);
    assert_eq!(d.graphics_counters().coherence_barriers, 1);
    assert!(data[RT..RT + 252].iter().all(|v| *v == 0));
    d.close(c, &mut mem(&mut data)).unwrap();
}

#[test]
#[ignore = "requires actual GPU; clear target rebinding drains prior output and rejects pending attachment aliases"]
fn clear_rebind_and_pending_attachment_overlap_are_safe() {
    let mut data = vec![0xa5; 0x20000];
    let (mut d, c, _, b, _) = driver();
    let mut w = clear_setup(7, 9, Some(1));
    clear_values(&mut w, [1., 0., 0., 1.]);
    method(&mut w, 0x19d0, 0x3c);
    method(&mut w, 0x804, 0xd0000);
    clear_values(&mut w, [0., 0., 1., 1.]);
    method(&mut w, 0x19d0, 0x3c);
    let r = submit(&mut d, c, b, &mut data, &w);
    assert!(r.fault.is_none(), "{:?}", r.fault);
    let red = expected_color(7, 9, Some(1), [255, 0, 0, 255]);
    let blue = expected_color(7, 9, Some(1), [0, 0, 255, 255]);
    assert_eq!(&data[RT..RT + red.len()], &red);
    assert!(data[Z..Z + blue.len()].iter().all(|v| *v == 0xa5));
    assert_eq!(d.graphics_counters().coherence_barriers, 1);
    d.publish(c, &mut mem(&mut data)).unwrap();
    assert_eq!(&data[Z..Z + blue.len()], &blue);
    assert_eq!(&data[RT..RT + red.len()], &red);
    d.close(c, &mut mem(&mut data)).unwrap();
    let mut data = vec![0xa5; 0x20000];
    let (mut d, c, _, b, _) = driver();
    let mut w = clear_setup(7, 9, Some(1));
    z_setup(&mut w, 7, 9, 0.25);
    method(&mut w, 0x19d0, 1);
    method(&mut w, 0x804, 0xd0000);
    clear_values(&mut w, [1., 0., 0., 1.]);
    method(&mut w, 0x19d0, 0x3c);
    let r = submit(&mut d, c, b, &mut data, &w);
    assert!(r.fault.is_some());
    assert_eq!(d.graphics_counters().clear_commands, 1);
    d.publish(c, &mut mem(&mut data)).unwrap();
    assert_eq!(&data[Z..Z + 1024], &expected_depth(7, 9, 0.25));
    assert!(data[RT..RT + 1024].iter().all(|v| *v == 0xa5));
    d.close(c, &mut mem(&mut data)).unwrap();
    // A complete, valid descriptor must resolve the final padded GOB, not just pixels.
    let mut data = vec![0xa5; 0x20000];
    let (mut d, c, _, b, _) = driver();
    let mut w = clear_setup(52, 65, Some(1));
    clear_values(&mut w, [1., 0., 0., 1.]);
    method(&mut w, 0x19d0, 0x3c);
    let r = submit(&mut d, c, b, &mut data, &w);
    assert!(r.fault.is_some());
    assert_eq!(d.graphics_counters().clear_commands, 0);
    assert!(data[RT..RT + 0x4000].iter().all(|v| *v == 0xa5));
    d.close(c, &mut mem(&mut data)).unwrap();
}
