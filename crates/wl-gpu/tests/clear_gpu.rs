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

#[test]
#[ignore = "requires actual GPU; native Z16 clear, whole-allocation CPU patch and indexed depth reuse"]
fn z16_clear_full_cpu_patch_retry_and_indexed_depth_test() {
    // Independent Tegra GOB geometry: two 64-byte columns, two 16-row
    // blocks, each occupying 1024 bytes. Logical Z16 pixels occupy 3016 bytes.
    const DEPTH_SIZE: usize = 4096;
    let (mut data, mut setup, _) = fixture();
    data[Z..Z + DEPTH_SIZE].fill(0xa5);
    for n in 0..3 {
        data[0x3008 + n * 24..0x300c + n * 24].copy_from_slice(&0.375f32.to_le_bytes());
    }
    let unrelated = data[Z + DEPTH_SIZE..Z + 0x4000].to_vec();
    let color_initial = data[RT..RT + SIZE].to_vec();
    let (mut d, c, _, b, _) = driver();
    let before = d.graphics_counters();
    let mut clear = clear_setup(W as u32, H as u32, None);
    z_setup(&mut clear, W as u32, H as u32, 0.375);
    method(&mut clear, 0xfe8, 0x13); // SET_ZT_FORMAT.Z16
    method(&mut clear, 0xff0, 1024); // single-layer footprint in words
    method(&mut clear, 0x19d0, 1); // CLEAR_SURFACE.Z only
    let first = submit(&mut d, c, b, &mut data, &clear);
    assert!(first.fault.is_none(), "{:?}", first.fault);
    assert_eq!(first.progress.draws_submitted, 0);
    let cleared = d.graphics_counters();
    assert_eq!(cleared.clear_commands - before.clear_commands, 1);
    assert_eq!(cleared.depth_guest_bytes, before.depth_guest_bytes);
    assert_eq!(cleared.depth_upload_bytes, before.depth_upload_bytes);
    assert_eq!(
        cleared.render_target_guest_bytes,
        before.render_target_guest_bytes
    );
    assert_eq!(cleared.upload_calls, before.upload_calls);
    assert_eq!(cleared.upload_bytes, before.upload_bytes);
    assert_eq!(cleared.deswizzle_bytes, before.deswizzle_bytes);
    assert_eq!(
        cleared.target_deswizzle_bytes,
        before.target_deswizzle_bytes
    );
    assert_eq!(cleared.target_swizzle_bytes, before.target_swizzle_bytes);
    assert_eq!(cleared.readback_operations, before.readback_operations);
    assert_eq!(cleared.depth_write_bytes, before.depth_write_bytes);
    assert!(data[Z..Z + DEPTH_SIZE].iter().all(|v| *v == 0xa5));
    assert_eq!(&data[RT..RT + SIZE], &color_initial);
    let completed = d.wait(first.token).unwrap();
    assert!(completed.host_completed && !completed.guest_visible);
    assert_eq!(
        d.graphics_counters().readback_operations,
        cleared.readback_operations
    );
    assert!(data[Z..Z + DEPTH_SIZE].iter().all(|v| *v == 0xa5));
    assert!(d.publish(c, &mut mem(&mut data)).unwrap().is_none());
    let mut expected = vec![0xa5; DEPTH_SIZE];
    let mut visible = vec![false; DEPTH_SIZE];
    for y in 0..H {
        for x in 0..W {
            // .375 * 65535 = 24575.625, away from a rounding tie.
            for (lane, byte) in 24576u16.to_le_bytes().into_iter().enumerate() {
                let a = gob_address(x * 2 + lane, y, W * 2, 2);
                visible[a] = true;
                expected[a] = byte;
            }
        }
    }
    assert_eq!(&data[Z..Z + DEPTH_SIZE], &expected);
    assert_eq!(&data[Z + DEPTH_SIZE..Z + 0x4000], &unrelated);
    assert_eq!(&data[RT..RT + SIZE], &color_initial);
    let published = d.graphics_counters();
    assert_eq!(
        published.depth_write_bytes - cleared.depth_write_bytes,
        DEPTH_SIZE as u64
    );
    assert_eq!(
        published.readback_operations - cleared.readback_operations,
        1
    );
    assert!(d.query(first.token).unwrap().guest_visible);
    d.retire(first.token).unwrap();

    // A CPU write covering the complete canonical storage allocation occurs
    // after another GPU clear. It replaces visible depth bytes AND padding.
    let mut again = Vec::new();
    method(&mut again, 0x19d0, 1);
    let second = submit(&mut d, c, b, &mut data, &again);
    assert!(second.fault.is_none(), "{:?}", second.fault);
    assert_eq!(&data[Z..Z + DEPTH_SIZE], &expected);
    let mut patch = vec![0x6d; DEPTH_SIZE];
    for y in 0..H {
        for x in 0..W {
            let code = if (x + y) % 2 == 0 { 8192u16 } else { 57343u16 };
            for (lane, byte) in code.to_le_bytes().into_iter().enumerate() {
                patch[gob_address(x * 2 + lane, y, W * 2, 2)] = byte;
            }
        }
    }
    d.write_backing(b, Z as u64, &patch, &mut mem(&mut data))
        .unwrap();
    let prior_failure = d.graphics_counters();
    assert!(d.publish(c, &mut FailAt(&mut data, Z as u64)).is_err());
    assert_eq!(&data[Z..Z + DEPTH_SIZE], &patch);
    assert_eq!(
        d.graphics_counters().depth_write_bytes,
        prior_failure.depth_write_bytes
    );
    assert!(!d.query(second.token).unwrap().guest_visible);
    assert_eq!(d.graphics_counters().clear_commands, 2);
    assert_eq!(d.graphics_counters().draw_commands, 0);
    assert!(d.publish(c, &mut mem(&mut data)).unwrap().is_none());
    assert_eq!(&data[Z..Z + DEPTH_SIZE], &patch);
    assert_eq!(&data[Z + DEPTH_SIZE..Z + 0x4000], &unrelated);
    assert_eq!(&data[RT..RT + SIZE], &color_initial);
    assert_eq!(d.graphics_counters().clear_commands, 2);
    assert_eq!(d.graphics_counters().draw_commands, 0);
    assert!(d.wait(second.token).unwrap().guest_visible);
    d.retire(second.token).unwrap();

    // The next genuine indexed draw must initialize native depth from the CPU
    // checker, rather than reuse the stale resident clear. Incoming .375 is
    // safely between the two stored depths, avoiding comparison precision ties.
    let end = setup.split_off(setup.len() - 8);
    method(&mut setup, 0x2144, 0x400); // independently assembled red pixel program
    z_setup(&mut setup, W as u32, H as u32, 0.375);
    for (m, v) in [
        (0xfe8, 0x13),
        (0xff0, 1024),
        (0x12cc, 1),
        (0x12e8, 1),
        (0x130c, 0x201),
    ] {
        method(&mut setup, m, v);
    }
    setup.extend(end);
    let before_draw = d.graphics_counters();
    let third = submit(&mut d, c, b, &mut data, &setup);
    assert!(third.fault.is_none(), "{:?}", third.fault);
    assert_eq!(third.progress.draws_submitted, 1);
    assert_eq!(&data[Z..Z + DEPTH_SIZE], &patch);
    assert_eq!(&data[RT..RT + SIZE], &color_initial);
    assert_eq!(
        d.graphics_counters().depth_upload_bytes - before_draw.depth_upload_bytes,
        (W * H * 4) as u64,
        "portable Z16 initialization stages four-byte R32 codes"
    );
    assert_eq!(
        d.graphics_counters().depth_allocations,
        before_draw.depth_allocations
    );
    assert_eq!(
        d.graphics_counters().readback_operations,
        before_draw.readback_operations
    );
    assert!(d.publish(c, &mut mem(&mut data)).unwrap().is_some());

    let v = POS.map(|p| p.map(f64::from));
    let area = edge(v[0], v[1], v[2]);
    let mut passed = 0;
    let mut rejected = 0;
    let mut outside = 0;
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
            if x < 47
                && weights.iter().any(|w| w.abs() < 0.005)
                && weights.iter().all(|w| *w >= -0.005)
            {
                // At a rasterization precision boundary, permit only the two
                // coherent coverage outcomes, never arbitrary depth/color bytes.
                let actual_depth = u16::from_le_bytes([
                    data[Z + gob_address(x * 2, y, W * 2, 2)],
                    data[Z + gob_address(x * 2 + 1, y, W * 2, 2)],
                ]);
                let a = RT + (y * W + x) * 4;
                let actual_color: [u8; 4] = data[a..a + 4].try_into().unwrap();
                let valid = if (x + y) % 2 == 0 {
                    actual_depth == 8192 && actual_color == BG
                } else {
                    (actual_depth == 57343 && actual_color == BG)
                        || (actual_depth == 24576 && actual_color == [255, 0, 0, 255])
                };
                assert!(
                    valid,
                    "Z16 raster edge {x},{y}: {actual_depth} {actual_color:?}"
                );
                continue;
            }
            let inside = x < 47 && weights.iter().all(|w| *w > 0.);
            let pass = inside && (x + y) % 2 != 0;
            if pass {
                passed += 1;
            } else if inside {
                rejected += 1;
            } else {
                outside += 1;
            }
            let depth = if pass {
                24576u16
            } else if (x + y) % 2 == 0 {
                8192u16
            } else {
                57343u16
            };
            for (lane, byte) in depth.to_le_bytes().into_iter().enumerate() {
                let a = gob_address(x * 2 + lane, y, W * 2, 2);
                assert_eq!(data[Z + a], byte, "Z16 depth {x},{y} lane {lane}");
            }
            let color = if pass { [255, 0, 0, 255] } else { BG };
            let a = RT + (y * W + x) * 4;
            assert_eq!(&data[a..a + 4], &color, "Z16-tested color {x},{y}");
        }
    }
    assert!(passed > 100 && rejected > 100 && outside > 1000);
    for (a, used) in visible.into_iter().enumerate() {
        if !used {
            assert_eq!(data[Z + a], 0x6d, "Z16 padding byte {a}");
        }
    }
    assert_eq!(&data[Z + DEPTH_SIZE..Z + 0x4000], &unrelated);
    assert_eq!(d.graphics_counters().clear_commands, 2);
    assert_eq!(d.graphics_counters().draw_commands, 1);
    assert!(d.wait(third.token).unwrap().guest_visible);
    d.retire(third.token).unwrap();
    d.close(c, &mut mem(&mut data)).unwrap();
}

#[test]
#[ignore = "requires actual GPU; full scissor-zero clears preserve pitch/tiled storage"]
fn guest_full_scissor_clear_control_renders_and_rejects_partial_clear() {
    for bh in [None, Some(1)] {
        let mut data = vec![0xa5; 0x20000];
        let (mut d, c, _, b, _) = driver();
        let mut words = clear_setup(7, 9, bh);
        clear_values(&mut words, [0.25, 0.5, 0.75, 1.]);
        for (m, v) in [
            (0x10f8, 0x101),
            (0xe00, 1),
            (0xe04, 7 << 16),
            (0xe08, 9 << 16),
        ] {
            method(&mut words, m, v);
        }
        method(&mut words, 0x19d0, 0x3c);
        let accepted = submit(&mut d, c, b, &mut data, &words);
        assert!(accepted.fault.is_none());
        assert_eq!(d.graphics_counters().clear_commands, 1);
        assert!(data[RT..RT + 252].iter().all(|v| *v == 0xa5)); // Deferred visibility.
        d.publish(c, &mut mem(&mut data)).unwrap();
        let expected = expected_color(7, 9, bh, [64, 128, 191, 255]);
        assert_eq!(&data[RT..RT + expected.len()], &expected); // Independent GOB/padding oracle.
        let before = d.graphics_counters();
        let mut partial = Vec::new();
        method(&mut partial, 0xe04, 6 << 16);
        method(&mut partial, 0x19d0, 0x3c);
        let rejected = submit(&mut d, c, b, &mut data, &partial);
        assert!(rejected.fault.is_some());
        assert_eq!(d.graphics_counters().clear_commands, before.clear_commands);
        assert_eq!(&data[RT..RT + expected.len()], &expected);
        d.close(c, &mut mem(&mut data)).unwrap();
    }
}

#[test]
#[ignore = "requires actual GPU; empty exclusive rectangles must not suppress a clear"]
fn empty_exclusive_window_rectangles_clear_and_nonempty_rectangles_fault() {
    for bh in [None, Some(1)] {
        let mut data = vec![0xa5; 0x20000];
        let (mut d, c, _, b, _) = driver();
        let mut words = clear_setup(7, 9, bh);
        clear_values(&mut words, [0.25, 0.5, 0.75, 1.]);
        method(&mut words, 0x1950, 1);
        method(&mut words, 0x194c, 1);
        for index in 0..8 {
            method(&mut words, 0xd00 + index * 8, 0);
            method(&mut words, 0xd04 + index * 8, 9 << 16);
        }
        method(&mut words, 0x19d0, 0x3c);
        let accepted = submit(&mut d, c, b, &mut data, &words);
        assert!(accepted.fault.is_none(), "{:?}", accepted.fault);
        assert_eq!(d.graphics_counters().clear_commands, 1);
        assert!(data[RT..RT + 252].iter().all(|v| *v == 0xa5));
        // A later unsupported clip cannot replay or discard the accepted clear.
        let mut clipped = Vec::new();
        method(&mut clipped, 0xd00, 7 << 16);
        method(&mut clipped, 0x19d0, 0x3c);
        assert!(submit(&mut d, c, b, &mut data, &clipped).fault.is_some());
        assert_eq!(d.graphics_counters().clear_commands, 1);
        d.publish(c, &mut mem(&mut data)).unwrap();
        let expected = expected_color(7, 9, bh, [64, 128, 191, 255]);
        assert_eq!(&data[RT..RT + expected.len()], &expected);
        assert_eq!(d.graphics_counters().clear_commands, 1);
        d.close(c, &mut mem(&mut data)).unwrap();
    }
}

#[test]
#[ignore = "requires actual GPU; B197 initialized context keeps window clipping disabled"]
fn initialized_context_allows_clear_before_window_and_multisample_writes() {
    for bh in [None, Some(1)] {
        let mut data = vec![0xa5; 0x20000];
        let (mut d, c, _, b, _) = driver();
        let mut words = clear_setup(7, 9, bh);
        // Remove the explicit single-sample write: initial B197 context supplies it.
        let index = words.windows(2).position(|v| v == [0x20010574, 0]).unwrap();
        words.drain(index..index + 2);
        // Driver-owned initialization also establishes the unconditional gate
        // and target-zero base layer. Neither is injected by the test caller.
        for pair in [[0x2001004e, 1], [0x20010208, 0]] {
            let index = words.windows(2).position(|v| v == pair).unwrap();
            words.drain(index..index + 2);
        }
        clear_values(&mut words, [0.25, 0.5, 0.75, 1.]);
        for index in 0..8 {
            method(&mut words, 0xd00 + index * 8, 0);
            method(&mut words, 0xd04 + index * 8, 0);
        }
        method(&mut words, 0x19d0, 0x3c);
        let accepted = submit(&mut d, c, b, &mut data, &words);
        assert!(accepted.fault.is_none(), "{:?}", accepted.fault);
        assert_eq!(d.graphics_counters().clear_commands, 1);
        // Unsupported global gating cannot be silently coerced to TRUE, nor
        // discard or replay the previously accepted driver-context clear.
        let mut unsupported = Vec::new();
        method(&mut unsupported, 0x0138, 0);
        method(&mut unsupported, 0x19d0, 0x3c);
        assert!(
            submit(&mut d, c, b, &mut data, &unsupported)
                .fault
                .is_some()
        );
        assert_eq!(d.graphics_counters().clear_commands, 1);
        d.publish(c, &mut mem(&mut data)).unwrap();
        let expected = expected_color(7, 9, bh, [64, 128, 191, 255]);
        assert_eq!(&data[RT..RT + expected.len()], &expected);
        d.close(c, &mut mem(&mut data)).unwrap();
    }
}
