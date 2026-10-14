#![cfg(feature = "wgpu-backend")]
mod common;
#[global_allocator]
static ALLOCATOR: &stats_alloc::StatsAlloc<std::alloc::System> = &stats_alloc::INSTRUMENTED_SYSTEM;
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
fn check(data: &[u8], color: [u8; 4], transformed: bool) {
    let v = POS.map(|p| {
        if transformed {
            [f64::from(p[0]).min(0.), f64::from(p[1])]
        } else {
            p.map(f64::from)
        }
    });
    let area = edge(v[0], v[1], v[2]);
    let mut covered = 0;
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
            if weights.iter().any(|v| v.abs() < 0.005) {
                continue;
            }
            let expected = if x < 47 && weights.iter().all(|v| *v >= 0.) {
                covered += 1;
                color
            } else {
                outside += 1;
                BG
            };
            assert_eq!(
                &data[RT + (y * W + x) * 4..RT + (y * W + x) * 4 + 4],
                &expected,
                "{x},{y}"
            );
        }
    }
    assert!(covered > 40);
    assert!(outside > 600);
}

fn select(w: &mut Vec<u32>, offset: u32, registers: u32) {
    let end = w.split_off(w.len() - 8);
    for (m, v) in [
        (0x2144, offset),
        (0x214c, registers),
        (0x2380, 32),
        (0x2384, 0),
        (0x2388, 0x40040),
        (0x2490, 1),
    ] {
        method(w, m, v);
    }
    w.extend(end);
}
fn prepare(bytes: bool) -> (Vec<u8>, Vec<u32>) {
    let (mut data, mut w, _) = fixture();
    let bin = if bytes {
        include_bytes!("fixtures/maxwell/signed-select-bytes.bin").as_slice()
    } else {
        include_bytes!("fixtures/maxwell/signed-predicate.bin").as_slice()
    };
    data[0x4600..0x4600 + bin.len()].copy_from_slice(bin);
    let v = include_bytes!("fixtures/maxwell/vertex-signed-predicate.bin");
    data[0x4a00..0x4a00 + v.len()].copy_from_slice(v);
    select(&mut w, 0x600, if bytes { 16 } else { 8 });
    (data, w)
}
fn input(d: &mut GpuDriver, b: BackingId, data: &mut [u8], words: [u32; 4]) {
    let bytes: Vec<u8> = words.into_iter().flat_map(u32::to_le_bytes).collect();
    d.write_backing(b, 0x50, &bytes, &mut mem(data)).unwrap();
}
fn signed(x: u32) -> i64 {
    i64::from(x) - if x & 0x80000000 != 0 { 1i64 << 32 } else { 0 }
}
fn compare(mode: u64, left: u32, right: u32) -> bool {
    let (a, b) = (signed(left), signed(right));
    match mode {
        1 => a < b,
        2 => a == b,
        3 => a <= b,
        4 => a > b,
        5 => a != b,
        6 => a >= b,
        _ => panic!("bad oracle mode"),
    }
}
#[test]
#[ignore = "requires actual GPU; signed comparisons select every bit of runtime candidate words"]
fn signed_all_comparisons_preserve_full_selected_words_and_data_cache() {
    let (mut data, w) = prepare(true);
    let (mut d, c, _, b, _) = driver();
    let mut pairs = vec![
        (0, 0),
        (0xffffffff, 0),
        (0, 0xffffffff),
        (0x80000000, 0),
        (0, 0x80000000),
        (0x7fffffff, 0x80000000),
        (0x80000000, 0x7fffffff),
        (0xffffffff, 0x80000000),
        (0x80000001, 0x80000000),
        (0xffffffff, 0xffffffff),
        (0x80000000, 0x80000000),
    ];
    let mut seed = 0x79642513u32;
    for _ in 0..32 {
        seed = seed.wrapping_mul(1664525).wrapping_add(1013904223);
        let a = seed;
        seed = seed.wrapping_mul(1664525).wrapping_add(1013904223);
        pairs.push((a, seed));
    }
    for mode in 1..=6 {
        // Guest code mutation selects independently assembled mode field words.
        let word = 0x5b61038000570407u64 | (mode << 49);
        d.write_backing(b, 0x46a0, &word.to_le_bytes(), &mut mem(&mut data))
            .unwrap();
        let mut cached = None;
        for &(left, right) in &pairs {
            let yes = left.rotate_left(9) ^ right.rotate_right(7) ^ 0x01234567;
            let no = !yes;
            assert_ne!(yes, no);
            input(&mut d, b, &mut data, [left, right, yes, no]);
            let r = submit(&mut d, c, b, &mut data, &w);
            assert!(r.fault.is_none(), "{:?}", r.fault);
            d.publish(c, &mut mem(&mut data)).unwrap();
            let expected = if compare(mode, left, right) { yes } else { no };
            check(&data, expected.to_le_bytes(), false);
            let now = d.graphics_counters();
            let key = (
                now.shader_decodes,
                now.shader_module_allocations,
                now.pipeline_allocations,
            );
            if let Some(old) = cached {
                assert_eq!(key, old, "CB data must not recompile mode{mode}");
            }
            cached = Some(key);
            d.retire(r.token).unwrap();
        }
    }
    assert_eq!(
        d.graphics_counters().draw_commands,
        (pairs.len() * 6) as u64
    );
    // Accept one earlier draw, then reject .X without publishing or replaying it.
    input(
        &mut d,
        b,
        &mut data,
        [0xffffffff, 0, 0x01234567, 0xfedcba98],
    );
    let saved = data[RT..RT + SIZE].to_vec();
    let good = submit(&mut d, c, b, &mut data, &draw());
    assert!(good.fault.is_none());
    assert_eq!(&data[RT..RT + SIZE], saved);
    d.write_backing(
        b,
        0x46a0,
        &0x5b6d0b8000570407u64.to_le_bytes(),
        &mut mem(&mut data),
    )
    .unwrap();
    let before = d.graphics_counters();
    let bad = submit(&mut d, c, b, &mut data, &draw());
    assert!(
        bad.fault
            .as_ref()
            .is_some_and(|f| f.to_string().contains("unsupported instruction")),
        "{:?}",
        bad.fault
    );
    assert_eq!(d.graphics_counters().draw_commands, before.draw_commands);
    assert_eq!(&data[RT..RT + SIZE], saved);
    d.publish(c, &mut mem(&mut data)).unwrap();
    check(&data, [152, 186, 220, 254], false); // -1>=0 false, selects raw FEDCBA98.
    assert_eq!(d.graphics_counters().draw_commands, before.draw_commands);
    eprintln!("signed ISETP all-word counters {:?}", d.graphics_counters());
    d.close(c, &mut mem(&mut data)).unwrap();
}
#[test]
#[ignore = "requires actual GPU; signedness belongs to each comparison, not the complete shader"]
fn signed_and_unsigned_comparisons_mix_and_reuse_resident_resources() {
    let (mut data, w) = prepare(false);
    let (mut d, c, _, b, _) = driver();
    input(
        &mut d,
        b,
        &mut data,
        [0xffffffff, 0, 0.25f32.to_bits(), 0.75f32.to_bits()],
    );
    let r = submit(&mut d, c, b, &mut data, &w);
    assert!(r.fault.is_none(), "{:?}", r.fault);
    d.publish(c, &mut mem(&mut data)).unwrap();
    check(&data, [64, 191, 191, 255], false);
    d.retire(r.token).unwrap();
    // Signed LT remains true, but unsigned GE is now true rather than false.
    let old = u64::from_le_bytes(data[0x46a0..0x46a8].try_into().unwrap());
    d.write_backing(
        b,
        0x46a0,
        &(old & !(1 << 48)).to_le_bytes(),
        &mut mem(&mut data),
    )
    .unwrap();
    let r = submit(&mut d, c, b, &mut data, &draw());
    assert!(r.fault.is_none(), "{:?}", r.fault);
    d.publish(c, &mut mem(&mut data)).unwrap();
    check(&data, [64, 191, 64, 255], false);
    d.retire(r.token).unwrap();
    let before = d.graphics_counters();
    let started = std::time::Instant::now();
    let region = stats_alloc::Region::new(ALLOCATOR);
    for _ in 0..32 {
        assert!(submit_existing(&mut d, c, &mut data, 8).fault.is_none());
    }
    let allocations = region.change();
    let elapsed = started.elapsed();
    let after = d.graphics_counters();
    assert_eq!(after.submissions - before.submissions, 32);
    assert_eq!(after.upload_bytes, before.upload_bytes);
    assert_eq!(after.shader_decodes, before.shader_decodes);
    assert_eq!(after.constant_guest_bytes, before.constant_guest_bytes);
    assert_eq!(after.pipeline_allocations, before.pipeline_allocations);
    assert_eq!(
        after.shader_module_allocations,
        before.shader_module_allocations
    );
    assert_eq!(after.readback_operations, before.readback_operations);
    assert_eq!(after.synchronization_points, before.synchronization_points);
    d.publish(c, &mut mem(&mut data)).unwrap();
    check(&data, [64, 191, 64, 255], false);
    assert_eq!(
        d.graphics_counters().readback_operations,
        after.readback_operations + 1
    );
    assert_eq!(
        d.graphics_counters().synchronization_points,
        after.synchronization_points + 1
    );
    eprintln!(
        "signed ISETP warm32 {elapsed:?}, Rust {allocations:?}; queues32, uploads0, waits0, readbacks0 before final visibility"
    );
    d.close(c, &mut mem(&mut data)).unwrap();
}
#[test]
#[ignore = "requires actual GPU; signed vertex predicate determines actual triangle coverage"]
fn signed_vertex_comparison_and_in_place_unsigned_mutation_change_coverage() {
    let (mut data, mut w) = prepare(false);
    let end = w.split_off(w.len() - 8);
    for (m, v) in [(0x2144, 0x400), (0x214c, 4), (0x2044, 0xa00), (0x204c, 7)] {
        method(&mut w, m, v);
    }
    w.extend(end);
    let (mut d, c, _, b, _) = driver();
    let r = submit(&mut d, c, b, &mut data, &w);
    assert!(r.fault.is_none(), "{:?}", r.fault);
    d.publish(c, &mut mem(&mut data)).unwrap();
    check(&data, [255, 0, 0, 255], true); // signed X<0 keeps negative X, otherwise zero.
    d.retire(r.token).unwrap();
    let before = d.graphics_counters();
    assert!(submit(&mut d, c, b, &mut data, &draw()).fault.is_none());
    let warm = d.graphics_counters();
    assert_eq!(warm.shader_binary_bytes, before.shader_binary_bytes);
    assert_eq!(warm.vertex_shader_bytes, before.vertex_shader_bytes);
    assert_eq!(warm.upload_bytes, before.upload_bytes);
    assert_eq!(warm.pipeline_allocations, before.pipeline_allocations);
    d.publish(c, &mut mem(&mut data)).unwrap();
    // Clear background using documented B197 clear before the degenerate draw;
    // otherwise earlier resident color would hide a missing/incorrect primitive.
    let mut clear = vec![0x20040360];
    clear.extend(BG.map(|x| (f32::from(x) / 255.).to_bits()));
    for (m, v) in [
        (0xff4, (W as u32) << 16),
        (0xff8, (H as u32) << 16),
        (0x10f8, 0),
        (0x19d0, 0x3c),
    ] {
        method(&mut clear, m, v);
    }
    assert!(submit(&mut d, c, b, &mut data, &clear).fault.is_none());
    // Unsigned X<0 is always false; every vertex X becomes zero, no coverage.
    let old = u64::from_le_bytes(data[0x4a78..0x4a80].try_into().unwrap());
    d.write_backing(
        b,
        0x4a78,
        &(old & !(1 << 48)).to_le_bytes(),
        &mut mem(&mut data),
    )
    .unwrap();
    assert!(submit(&mut d, c, b, &mut data, &draw()).fault.is_none());
    d.publish(c, &mut mem(&mut data)).unwrap();
    for p in data[RT..RT + SIZE].chunks_exact(4) {
        assert_eq!(p, BG);
    }
    assert_eq!(
        d.graphics_counters().vertex_shader_decodes,
        before.vertex_shader_decodes + 1
    );
    d.close(c, &mut mem(&mut data)).unwrap();
}
