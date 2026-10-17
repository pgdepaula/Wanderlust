#![cfg(feature = "wgpu-backend")]
mod common;
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
            [f64::from(p[0]) * 0.5, f64::from(p[1]) + 0.5]
        } else {
            p.map(f64::from)
        }
    });
    check_positions(data, color, v);
}
fn check_positions(data: &[u8], color: [u8; 4], v: [[f64; 2]; 3]) {
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
                BG
            };
            assert_eq!(
                &data[RT + (y * W + x) * 4..RT + (y * W + x) * 4 + 4],
                &expected,
                "{x},{y}"
            );
        }
    }
}
#[test]
#[ignore = "requires actual GPU; guest selected FADD/FMUL, mutable LDC data and code invalidation"]
fn driver_float_alu_resources_and_code_coherence() {
    let (mut data, mut w, _) = fixture();
    let (mut d, c, _, b, _) = driver();
    let bin = include_bytes!("fixtures/maxwell/alu.bin");
    data[0x4600..0x4600 + bin.len()].copy_from_slice(bin);
    let end = w.split_off(w.len() - 8);
    for (m, v) in [
        (0x2144, 0x600),
        (0x2380, 32),
        (0x2384, 0),
        (0x2388, 0x40040),
        (0x2490, 1),
    ] {
        method(&mut w, m, v);
    }
    w.extend(end);
    let mut cached = None;
    for (input, expected) in [
        ([0.25f32, 0.5, 1.], [191, 128, 191, 255]),
        ([0.125, 0.125, 0.5], [64, 16, 64, 64]),
        ([0.25, 0.5, 1.], [191, 128, 191, 255]),
    ] {
        let bytes: Vec<u8> = input.iter().flat_map(|v| v.to_le_bytes()).collect();
        d.write_backing(b, 0x50, &bytes, &mut mem(&mut data))
            .unwrap();
        let report = submit(&mut d, c, b, &mut data, &w);
        assert!(report.fault.is_none(), "{:?}", report.fault);
        d.publish(c, &mut mem(&mut data)).unwrap();
        check(&data, expected, false);
        let counters = d.graphics_counters();
        if let Some((modules, pipelines, decodes)) = cached {
            assert_eq!(
                (
                    counters.shader_module_allocations,
                    counters.pipeline_allocations,
                    counters.shader_decodes
                ),
                (modules, pipelines, decodes)
            );
        }
        cached = Some((
            counters.shader_module_allocations,
            counters.pipeline_allocations,
            counters.shader_decodes,
        ));
        d.retire(report.token).unwrap();
    }
    // The independently assembled FADD opcode replaces the independent FMUL. Binary
    // semantics change while the same guest constants remain runtime data.
    let mut code = bin.to_vec();
    let off = 0x80;
    let original = u64::from_le_bytes(code[off..off + 8].try_into().unwrap());
    let changed = (original & 0x0000ffffffffffff) | 0x5c58000000000000;
    code[off..off + 8].copy_from_slice(&changed.to_le_bytes());
    d.write_backing(b, 0x4600, &code, &mut mem(&mut data))
        .unwrap();
    let before = d.graphics_counters();
    let report = submit(&mut d, c, b, &mut data, &draw());
    assert!(report.fault.is_none(), "{:?}", report.fault);
    d.publish(c, &mut mem(&mut data)).unwrap();
    check(&data, [191, 255, 191, 255], false);
    assert_eq!(
        d.graphics_counters().shader_decodes,
        before.shader_decodes + 1
    );
    eprintln!("ALU final counters {:?}", d.graphics_counters());
    d.close(c, &mut mem(&mut data)).unwrap();
}

#[test]
#[ignore = "requires real GPU; Maxwell vertex FADD/FMUL changes independently predicted coverage"]
fn driver_vertex_float_alu_moves_guest_geometry() {
    let (mut data, mut w, _) = fixture();
    let (mut d, c, _, b, _) = driver();
    let bin = include_bytes!("fixtures/maxwell/vertex-alu.bin");
    data[0x4800..0x4800 + bin.len()].copy_from_slice(bin);
    let end = w.split_off(w.len() - 8);
    method(&mut w, 0x2044, 0x800);
    method(&mut w, 0x2144, 0x400);
    w.extend(end);
    let report = submit(&mut d, c, b, &mut data, &w);
    assert!(report.fault.is_none(), "{:?}", report.fault);
    d.publish(c, &mut mem(&mut data)).unwrap();
    check(&data, [255, 0, 0, 255], true);
    let before = d.graphics_counters();
    let report = submit(&mut d, c, b, &mut data, &draw());
    assert!(report.fault.is_none());
    d.publish(c, &mut mem(&mut data)).unwrap();
    check(&data, [255, 0, 0, 255], true);
    assert_eq!(
        d.graphics_counters().shader_module_allocations,
        before.shader_module_allocations
    );
    assert_eq!(
        d.graphics_counters().pipeline_allocations,
        before.pipeline_allocations
    );
    d.close(c, &mut mem(&mut data)).unwrap();
}

#[test]
#[ignore = "requires actual GPU; short-float immediates, mutable guest constants and binary invalidation"]
fn driver_float_immediate_resources_and_code_coherence() {
    let (mut data, mut words, _) = fixture();
    let (mut d, c, _, b, _) = driver();
    let bin = include_bytes!("fixtures/maxwell/float-immediate.bin");
    data[0x4600..0x4600 + bin.len()].copy_from_slice(bin);
    let end = words.split_off(words.len() - 8);
    for (m, v) in [
        (0x2144, 0x600),
        (0x2380, 32),
        (0x2384, 0),
        (0x2388, 0x40040),
        (0x2490, 1),
    ] {
        method(&mut words, m, v);
    }
    words.extend(end);
    let mut identity = None;
    for (input, expected) in [
        ([0.125f32, 0.75, 0.75], [64, 96, 128, 255]),
        ([0.375, 0.25, 0.5], [128, 32, 64, 255]),
        ([0.125, 0.75, 0.75], [64, 96, 128, 255]),
    ] {
        let bytes: Vec<_> = input.iter().flat_map(|v| v.to_le_bytes()).collect();
        d.write_backing(b, 0x50, &bytes, &mut mem(&mut data))
            .unwrap();
        d.write_backing(b, RT as u64, &BG.repeat(W * H), &mut mem(&mut data))
            .unwrap();
        let r = submit(&mut d, c, b, &mut data, &words);
        assert!(r.fault.is_none(), "{:?}", r.fault);
        d.publish(c, &mut mem(&mut data)).unwrap();
        check(&data, expected, false);
        d.retire(r.token).unwrap();
        let n = d.graphics_counters();
        let now = [
            n.shader_decodes,
            n.shader_module_allocations,
            n.pipeline_allocations,
        ];
        if let Some(old) = identity {
            assert_eq!(now, old);
        }
        identity = Some(now);
    }
    // Independently decoded raw sign/exponent/mantissa field: change -0.25 to -0.5.
    let mut code = bin.to_vec();
    let off = 0x88;
    let w = u64::from_le_bytes(code[off..off + 8].try_into().unwrap());
    let w = (w & !(0x7ffffu64 << 20)) | (0x3f000u64 << 20);
    code[off..off + 8].copy_from_slice(&w.to_le_bytes());
    d.write_backing(b, 0x4600, &code, &mut mem(&mut data))
        .unwrap();
    let before = d.graphics_counters();
    let r = submit(&mut d, c, b, &mut data, &draw());
    assert!(r.fault.is_none(), "{:?}", r.fault);
    assert!(d.has_pending_output());
    // Unsupported SAT must fault without reusing the previously resident shader.
    let w = u64::from_le_bytes(code[0x78..0x80].try_into().unwrap()) | (1 << 50);
    code[0x78..0x80].copy_from_slice(&w.to_le_bytes());
    d.write_backing(b, 0x4600, &code, &mut mem(&mut data))
        .unwrap();
    let accepted = d.graphics_counters();
    let fault = submit(&mut d, c, b, &mut data, &draw());
    assert!(fault.fault.is_some());
    assert_eq!(d.graphics_counters().draw_commands, accepted.draw_commands);
    assert_eq!(d.graphics_counters().submissions, accepted.submissions);
    d.publish(c, &mut mem(&mut data)).unwrap();
    check(&data, [64, 96, 64, 255], false);
    assert_eq!(accepted.shader_decodes, before.shader_decodes + 1);
    assert_eq!(
        d.graphics_counters().draw_commands,
        before.draw_commands + 1
    );
    eprintln!("shortfloat resources {:?}", d.graphics_counters());
    d.close(c, &mut mem(&mut data)).unwrap();
}
#[test]
#[ignore = "requires actual GPU; Maxwell immediate vertex arithmetic and warm resource identity"]
fn driver_vertex_float_immediate_moves_guest_geometry() {
    let (mut data, mut words, _) = fixture();
    let (mut d, c, _, b, _) = driver();
    let bin = include_bytes!("fixtures/maxwell/vertex-float-immediate.bin");
    data[0x4800..0x4800 + bin.len()].copy_from_slice(bin);
    let end = words.split_off(words.len() - 8);
    method(&mut words, 0x2044, 0x800);
    method(&mut words, 0x2144, 0x400);
    words.extend(end);
    let r = submit(&mut d, c, b, &mut data, &words);
    assert!(r.fault.is_none(), "{:?}", r.fault);
    d.publish(c, &mut mem(&mut data)).unwrap();
    check(&data, [255, 0, 0, 255], true);
    d.retire(r.token).unwrap();
    let bytes: Vec<_> = draw().iter().flat_map(|v| v.to_le_bytes()).collect();
    d.write_backing(b, 0x10000, &bytes, &mut mem(&mut data))
        .unwrap();
    let before = d.graphics_counters();
    let start = std::time::Instant::now();
    for _ in 0..32 {
        assert!(submit_existing(&mut d, c, &mut data, 8).fault.is_none());
    }
    let elapsed = start.elapsed();
    let after = d.graphics_counters();
    assert_eq!(after.submissions - before.submissions, 32);
    assert_eq!(
        [
            after.shader_decodes,
            after.vertex_shader_decodes,
            after.shader_binary_bytes,
            after.vertex_shader_bytes,
            after.vertex_guest_bytes,
            after.index_guest_bytes,
            after.constant_guest_bytes,
            after.guest_texture_bytes,
            after.descriptor_reads,
            after.render_target_guest_bytes
        ],
        [
            before.shader_decodes,
            before.vertex_shader_decodes,
            before.shader_binary_bytes,
            before.vertex_shader_bytes,
            before.vertex_guest_bytes,
            before.index_guest_bytes,
            before.constant_guest_bytes,
            before.guest_texture_bytes,
            before.descriptor_reads,
            before.render_target_guest_bytes
        ]
    );
    assert_eq!(
        [
            after.upload_bytes,
            after.upload_calls,
            after.shader_module_allocations,
            after.pipeline_allocations,
            after.readback_operations,
            after.synchronization_points
        ],
        [
            before.upload_bytes,
            before.upload_calls,
            before.shader_module_allocations,
            before.pipeline_allocations,
            before.readback_operations,
            before.synchronization_points
        ]
    );
    assert!(d.has_pending_output());
    d.publish(c, &mut mem(&mut data)).unwrap();
    check(&data, [255, 0, 0, 255], true);
    eprintln!("shortfloat warm32 CPU={elapsed:?} before={before:?} after={after:?}");
    d.close(c, &mut mem(&mut data)).unwrap();
}

#[test]
#[ignore = "requires actual GPU; raw-bit oracle exposes complete FADD32I/FMUL32I mantissas"]
fn driver_float32_immediate_raw_bits_and_code_coherence() {
    let (mut data, mut words, _) = fixture();
    let (mut d, c, _, b, _) = driver();
    let bin = include_bytes!("fixtures/maxwell/float32-immediate.bin");
    data[0x4600..0x4600 + bin.len()].copy_from_slice(bin);
    let end = words.split_off(words.len() - 8);
    for (m, v) in [
        (0x2144, 0x600),
        (0x214c, 11),
        (0x2380, 32),
        (0x2384, 0),
        (0x2388, 0x40040),
        (0x2490, 1),
    ] {
        method(&mut words, m, v);
    }
    words.extend(end);
    let mut identity = None;
    // The guest shader compares actual raw arithmetic outputs with independent
    // expected words supplied as runtime data, then SEL exports the two answers.
    // One missing low mantissa bit changes yellow into black.
    for (input, color) in [
        (
            [0xbf800000u32, 0x3f800000, 0x34000000, 0x3f800001],
            [255, 255, 0, 255],
        ),
        ([0xbf800000, 0x3f800000, 0, 0x3f800000], [0, 0, 0, 255]),
        ([0, 0x40000000, 0x3f800001, 0x40000001], [255, 255, 0, 255]),
        (
            [0xbf800000, 0x3f800000, 0x34000000, 0x3f800001],
            [255, 255, 0, 255],
        ),
    ] {
        let bytes: Vec<_> = input.iter().flat_map(|v| v.to_le_bytes()).collect();
        d.write_backing(b, 0x50, &bytes, &mut mem(&mut data))
            .unwrap();
        let r = submit(&mut d, c, b, &mut data, &words);
        assert!(r.fault.is_none(), "{:?}", r.fault);
        d.publish(c, &mut mem(&mut data)).unwrap();
        check(&data, color, false);
        d.retire(r.token).unwrap();
        let n = d.graphics_counters();
        let now = [
            n.shader_decodes,
            n.shader_module_allocations,
            n.pipeline_allocations,
        ];
        if let Some(old) = identity {
            assert_eq!(now, old);
        }
        identity = Some(now);
    }
    let mut code = bin.to_vec();
    let off = 0x88;
    let old = u64::from_le_bytes(code[off..off + 8].try_into().unwrap());
    let changed = (old & !(0xffffffffu64 << 20)) | (0x3f800002u64 << 20);
    code[off..off + 8].copy_from_slice(&changed.to_le_bytes());
    d.write_backing(b, 0x4600, &code, &mut mem(&mut data))
        .unwrap();
    let before = d.graphics_counters();
    let r = submit(&mut d, c, b, &mut data, &draw());
    assert!(r.fault.is_none(), "{:?}", r.fault);
    let accepted = d.graphics_counters();
    assert_eq!(accepted.shader_decodes, before.shader_decodes + 1);
    let off = 0x98;
    let old = u64::from_le_bytes(code[off..off + 8].try_into().unwrap());
    code[off..off + 8].copy_from_slice(&(old | (1 << 52)).to_le_bytes());
    d.write_backing(b, 0x4600, &code, &mut mem(&mut data))
        .unwrap();
    let fault = submit(&mut d, c, b, &mut data, &draw());
    assert!(fault.fault.is_some());
    assert_eq!(d.graphics_counters().draw_commands, accepted.draw_commands);
    assert_eq!(d.graphics_counters().submissions, accepted.submissions);
    d.publish(c, &mut mem(&mut data)).unwrap();
    check(&data, [0, 255, 0, 255], false);
    assert_eq!(
        d.graphics_counters().draw_commands,
        before.draw_commands + 1
    );
    eprintln!("fullfloat rawbit counters {:?}", d.graphics_counters());
    d.close(c, &mut mem(&mut data)).unwrap();
}
fn check_full32_geometry(data: &[u8]) {
    let scale = f64::from(f32::from_bits(0x3f000001));
    let v = POS.map(|p| [f64::from(p[0]) * scale, f64::from(p[1]) + scale]);
    let area = edge(v[0], v[1], v[2]);
    let mut inside = 0;
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
            if weights.iter().any(|w| w.abs() < 0.005) {
                continue;
            }
            let expected = if x < 47 && weights.iter().all(|w| *w > 0.) {
                inside += 1;
                [255, 0, 0, 255]
            } else {
                outside += 1;
                BG
            };
            let off = RT + (y * W + x) * 4;
            assert_eq!(&data[off..off + 4], &expected, "full32 VS {x},{y}");
        }
    }
    assert!(inside > 80 && outside > 1000);
}
#[test]
#[ignore = "requires actual GPU; full32 vertex constants and unchanged resource residency"]
fn driver_vertex_float32_immediate_geometry_and_warm_cache() {
    let (mut data, mut words, _) = fixture();
    let (mut d, c, _, b, _) = driver();
    let bin = include_bytes!("fixtures/maxwell/vertex-float32-immediate.bin");
    data[0x4800..0x4800 + bin.len()].copy_from_slice(bin);
    let end = words.split_off(words.len() - 8);
    method(&mut words, 0x2044, 0x800);
    method(&mut words, 0x2144, 0x400);
    words.extend(end);
    let r = submit(&mut d, c, b, &mut data, &words);
    assert!(r.fault.is_none(), "{:?}", r.fault);
    d.publish(c, &mut mem(&mut data)).unwrap();
    check_full32_geometry(&data);
    d.retire(r.token).unwrap();
    let bytes: Vec<_> = draw().iter().flat_map(|v| v.to_le_bytes()).collect();
    d.write_backing(b, 0x10000, &bytes, &mut mem(&mut data))
        .unwrap();
    let before = d.graphics_counters();
    let start = std::time::Instant::now();
    for _ in 0..32 {
        assert!(submit_existing(&mut d, c, &mut data, 8).fault.is_none());
    }
    let elapsed = start.elapsed();
    let after = d.graphics_counters();
    assert_eq!(after.submissions - before.submissions, 32);
    assert_eq!(
        [
            after.shader_decodes,
            after.vertex_shader_decodes,
            after.shader_binary_bytes,
            after.vertex_shader_bytes,
            after.vertex_guest_bytes,
            after.index_guest_bytes,
            after.constant_guest_bytes,
            after.guest_texture_bytes,
            after.descriptor_reads,
            after.render_target_guest_bytes
        ],
        [
            before.shader_decodes,
            before.vertex_shader_decodes,
            before.shader_binary_bytes,
            before.vertex_shader_bytes,
            before.vertex_guest_bytes,
            before.index_guest_bytes,
            before.constant_guest_bytes,
            before.guest_texture_bytes,
            before.descriptor_reads,
            before.render_target_guest_bytes
        ]
    );
    assert_eq!(
        [
            after.upload_bytes,
            after.upload_calls,
            after.shader_module_allocations,
            after.pipeline_allocations,
            after.readback_operations,
            after.synchronization_points
        ],
        [
            before.upload_bytes,
            before.upload_calls,
            before.shader_module_allocations,
            before.pipeline_allocations,
            before.readback_operations,
            before.synchronization_points
        ]
    );
    d.publish(c, &mut mem(&mut data)).unwrap();
    check_full32_geometry(&data);
    eprintln!("fullfloat warm32 CPU={elapsed:?} before={before:?} after={after:?}");
    d.close(c, &mut mem(&mut data)).unwrap();
}

#[test]
#[ignore = "requires actual GPU; conditional MOV preserves false destinations and runtime predicate data"]
fn driver_conditional_move_runtime_predicates_and_code_coherence() {
    let (mut data, mut words, _) = fixture();
    let (mut d, c, _, b, _) = driver();
    let bin = include_bytes!("fixtures/maxwell/conditional-move.bin");
    data[0x4600..0x4600 + bin.len()].copy_from_slice(bin);
    let end = words.split_off(words.len() - 8);
    for (m, v) in [
        (0x2144, 0x600),
        (0x214c, 8),
        (0x2380, 32),
        (0x2384, 0),
        (0x2388, 0x40040),
        (0x2490, 1),
    ] {
        method(&mut words, m, v);
    }
    words.extend(end);
    let mut identity = None;
    // Independently specified unsigned comparisons include pairs that disagree
    // with signed-integer or floating comparisons. False MOV retains .25/.75.
    for (input, color) in [
        ([1u32, 2], [255, 128, 191, 255]),
        ([2, 1], [64, 128, 0, 255]),
        ([7, 7], [64, 128, 0, 255]),
        ([0, 0xffffffff], [255, 128, 191, 255]),
        ([0x80000000, 0x7fffffff], [64, 128, 0, 255]),
        ([1, 2], [255, 128, 191, 255]),
    ] {
        let bytes: Vec<_> = input.iter().flat_map(|v| v.to_le_bytes()).collect();
        d.write_backing(b, 0x50, &bytes, &mut mem(&mut data))
            .unwrap();
        let r = submit(&mut d, c, b, &mut data, &words);
        assert!(r.fault.is_none(), "{:?}", r.fault);
        d.publish(c, &mut mem(&mut data)).unwrap();
        check(&data, color, false);
        d.retire(r.token).unwrap();
        let n = d.graphics_counters();
        let now = [
            n.shader_decodes,
            n.shader_module_allocations,
            n.pipeline_allocations,
        ];
        if let Some(old) = identity {
            assert_eq!(now, old);
        }
        identity = Some(now);
    }
    // Invert only the first MOV's execution predicate. With true P0 both MOVs
    // now preserve their old destination. This is code, not mutable CB data.
    let mut code = bin.to_vec();
    let off = 0xb8;
    let old = u64::from_le_bytes(code[off..off + 8].try_into().unwrap());
    code[off..off + 8].copy_from_slice(&(old ^ (1 << 19)).to_le_bytes());
    d.write_backing(b, 0x4600, &code, &mut mem(&mut data))
        .unwrap();
    let before = d.graphics_counters();
    let accepted = submit(&mut d, c, b, &mut data, &draw());
    assert!(accepted.fault.is_none(), "{:?}", accepted.fault);
    let after = d.graphics_counters();
    assert_eq!(after.shader_decodes, before.shader_decodes + 1);
    assert_eq!(after.draw_commands, before.draw_commands + 1);
    // Unsupported P1 must not execute a stale cached program or repeat the
    // already accepted draw. Publication is still possible after this fault.
    let old = u64::from_le_bytes(code[off..off + 8].try_into().unwrap());
    code[off..off + 8].copy_from_slice(&(old | (1 << 16)).to_le_bytes());
    d.write_backing(b, 0x4600, &code, &mut mem(&mut data))
        .unwrap();
    assert!(submit(&mut d, c, b, &mut data, &draw()).fault.is_some());
    assert_eq!(d.graphics_counters().draw_commands, after.draw_commands);
    assert_eq!(d.graphics_counters().submissions, after.submissions);
    d.publish(c, &mut mem(&mut data)).unwrap();
    check(&data, [64, 128, 191, 255], false);
    eprintln!(
        "conditional MOV fragment counters {:?}",
        d.graphics_counters()
    );
    d.close(c, &mut mem(&mut data)).unwrap();
}

fn check_conditional_geometry(data: &[u8], first_x: f64) {
    let v = [
        [first_x, f64::from(POS[0][1])],
        POS[1].map(f64::from),
        POS[2].map(f64::from),
    ];
    let area = edge(v[0], v[1], v[2]);
    let mut inside = 0;
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
            if weights.iter().any(|w| w.abs() < 0.005) {
                continue;
            }
            let expected = if x < 47 && weights.iter().all(|w| *w > 0.) {
                inside += 1;
                [255, 0, 0, 255]
            } else {
                outside += 1;
                BG
            };
            let off = RT + (y * W + x) * 4;
            assert_eq!(&data[off..off + 4], &expected, "conditional VS {x},{y}");
        }
    }
    assert!(inside > 20 && outside > 1000);
}

#[test]
#[ignore = "requires actual GPU; per-vertex conditional MOV and guest vertex mutations"]
fn driver_vertex_conditional_move_geometry_and_warm_cache() {
    let (mut data, mut words, _) = fixture();
    let (mut d, c, _, b, _) = driver();
    let bin = include_bytes!("fixtures/maxwell/vertex-conditional-move.bin");
    data[0x4800..0x4800 + bin.len()].copy_from_slice(bin);
    let end = words.split_off(words.len() - 8);
    method(&mut words, 0x2044, 0x800);
    method(&mut words, 0x204c, 8);
    method(&mut words, 0x2144, 0x400);
    words.extend(end);
    let r = submit(&mut d, c, b, &mut data, &words);
    assert!(r.fault.is_none(), "{:?}", r.fault);
    d.publish(c, &mut mem(&mut data)).unwrap();
    check_conditional_geometry(&data, 0.5);
    d.retire(r.token).unwrap();
    // Only guest X changes: raw equality to -.8 becomes false for vertex zero.
    // Explicit canonical target reset removes prior coverage before the oracle.
    d.write_backing(b, 0x3000, &(-0.6f32).to_le_bytes(), &mut mem(&mut data))
        .unwrap();
    let background: Vec<_> = (0..W * H).flat_map(|_| BG).collect();
    d.write_backing(b, RT as u64, &background, &mut mem(&mut data))
        .unwrap();
    let before = d.graphics_counters();
    let r = submit(&mut d, c, b, &mut data, &draw());
    assert!(r.fault.is_none(), "{:?}", r.fault);
    d.publish(c, &mut mem(&mut data)).unwrap();
    check_conditional_geometry(&data, f64::from(-0.6f32));
    d.retire(r.token).unwrap();
    let after = d.graphics_counters();
    assert_eq!(
        [
            before.vertex_shader_decodes,
            before.shader_decodes,
            before.shader_module_allocations,
            before.pipeline_allocations
        ],
        [
            after.vertex_shader_decodes,
            after.shader_decodes,
            after.shader_module_allocations,
            after.pipeline_allocations
        ]
    );
    assert_eq!(after.vertex_upload_bytes - before.vertex_upload_bytes, 72);
    let bytes: Vec<_> = draw().iter().flat_map(|v| v.to_le_bytes()).collect();
    d.write_backing(b, 0x10000, &bytes, &mut mem(&mut data))
        .unwrap();
    let before = d.graphics_counters();
    let start = std::time::Instant::now();
    for _ in 0..32 {
        let report = submit_existing(&mut d, c, &mut data, 8);
        assert!(report.fault.is_none());
        assert_eq!(report.progress.draws_submitted, 1);
    }
    let elapsed = start.elapsed();
    let after = d.graphics_counters();
    assert_eq!(after.submissions - before.submissions, 32);
    assert_eq!(after.draw_commands - before.draw_commands, 32);
    assert_eq!(after.shader_decodes, before.shader_decodes);
    assert_eq!(after.vertex_shader_decodes, before.vertex_shader_decodes);
    assert_eq!(
        [
            after.shader_binary_bytes,
            after.vertex_shader_bytes,
            after.vertex_guest_bytes,
            after.index_guest_bytes,
            after.constant_guest_bytes,
            after.guest_texture_bytes,
            after.descriptor_reads,
            after.render_target_guest_bytes
        ],
        [
            before.shader_binary_bytes,
            before.vertex_shader_bytes,
            before.vertex_guest_bytes,
            before.index_guest_bytes,
            before.constant_guest_bytes,
            before.guest_texture_bytes,
            before.descriptor_reads,
            before.render_target_guest_bytes
        ]
    );
    assert_eq!(
        [
            after.upload_bytes,
            after.upload_calls,
            after.shader_module_allocations,
            after.pipeline_allocations,
            after.readback_operations,
            after.synchronization_points
        ],
        [
            before.upload_bytes,
            before.upload_calls,
            before.shader_module_allocations,
            before.pipeline_allocations,
            before.readback_operations,
            before.synchronization_points
        ]
    );
    d.publish(c, &mut mem(&mut data)).unwrap();
    check_conditional_geometry(&data, f64::from(-0.6f32));
    eprintln!("conditional MOV warm32 CPU={elapsed:?} before={before:?} after={after:?}");
    d.close(c, &mut mem(&mut data)).unwrap();
}
fn float_predicate_color(a: u32, b: u32) -> [u8; 4] {
    // Binary32 values embed exactly in binary64. This oracle uses host numeric
    // relations, independently of production's integer-key WGSL comparison.
    let a = f64::from(f32::from_bits(a));
    let b = f64::from(f32::from_bits(b));
    let red = if a.is_nan() || b.is_nan() {
        0 // All six ORDERED relations, including NE, must be false.
    } else if a < b {
        84 // LT | LE | NE = (1+4+16)/64.
    } else if a == b {
        151 // EQ | LE | GE = (2+4+32)/64.
    } else {
        223 // GT | NE | GE = (8+16+32)/64.
    };
    [red, 64, 191, 255]
}
fn all_float_predicate_color(a: u32, b: u32) -> [u8; 4] {
    // Independent numeric oracle: truth masks for F, six ordered predicates,
    // NUM, NAN, six unordered predicates, T are AAAA/CCCC/F0F0/FF00.
    // Four nibble channels use exact dyadic weights 1/16,1/8,1/4,1/2.
    let a = f64::from(f32::from_bits(a));
    let b = f64::from(f32::from_bits(b));
    if a.is_nan() || b.is_nan() {
        [0, 0, 239, 239]
    } else if a < b {
        [159, 159, 159, 159]
    } else if a == b {
        [191, 191, 191, 191]
    } else {
        [0, 239, 0, 239]
    }
}

#[test]
#[ignore = "requires actual GPU; sixteen floating conditions with ordered/unordered NaNs and exact raw data"]
fn driver_all_float_predicate_modes_and_nan_coherence() {
    let (mut data, mut words, _) = fixture();
    let (mut d, c, _, b, _) = driver();
    let bin = include_bytes!("fixtures/maxwell/float-predicate-all.bin");
    data[0x4600..0x4600 + bin.len()].copy_from_slice(bin);
    let end = words.split_off(words.len() - 8);
    for (m, v) in [
        (0x2144, 0x600),
        (0x214c, 8),
        (0x2380, 32),
        (0x2384, 0),
        (0x2388, 0x40040),
        (0x2490, 1),
    ] {
        method(&mut words, m, v);
    }
    words.extend(end);
    let mut identity = None;
    let pairs = ieee_predicate_pairs();
    let count = pairs.len();
    for [a, z] in pairs {
        let bytes: Vec<_> = [a, z].into_iter().flat_map(u32::to_le_bytes).collect();
        d.write_backing(b, 0x50, &bytes, &mut mem(&mut data))
            .unwrap();
        let r = submit(&mut d, c, b, &mut data, &words);
        assert!(r.fault.is_none(), "{:?}", r.fault);
        assert_eq!(r.progress.draws_submitted, 1);
        d.publish(c, &mut mem(&mut data)).unwrap();
        check(&data, all_float_predicate_color(a, z), false);
        d.retire(r.token).unwrap();
        let n = d.graphics_counters();
        let now = [
            n.shader_decodes,
            n.shader_module_allocations,
            n.pipeline_allocations,
        ];
        if let Some(old) = identity {
            assert_eq!(now, old);
        }
        identity = Some(now);
    }
    // The first condition is F, with weight1/16 in red. Change only F→T:
    // final -1<1 maskAAAA becomesAAAB, red increases from159 to175.
    let mut code = bin.to_vec();
    let off = 0xa0;
    let op = u64::from_le_bytes(code[off..off + 8].try_into().unwrap());
    assert_eq!(op, 0x5bb0038000570407);
    code[off..off + 8].copy_from_slice(&(op | (15 << 48)).to_le_bytes());
    d.write_backing(b, 0x4600, &code, &mut mem(&mut data))
        .unwrap();
    let before = d.graphics_counters();
    let r = submit(&mut d, c, b, &mut data, &draw());
    assert!(r.fault.is_none(), "{:?}", r.fault);
    assert_eq!(r.progress.draws_submitted, 1);
    assert_eq!(
        d.graphics_counters().shader_decodes,
        before.shader_decodes + 1
    );
    d.publish(c, &mut mem(&mut data)).unwrap();
    check(&data, [175, 159, 159, 159], false);
    d.retire(r.token).unwrap();

    let bytes: Vec<_> = draw().iter().flat_map(|v| v.to_le_bytes()).collect();
    d.write_backing(b, 0x10000, &bytes, &mut mem(&mut data))
        .unwrap();
    let before = d.graphics_counters();
    let start = std::time::Instant::now();
    for _ in 0..32 {
        let r = submit_existing(&mut d, c, &mut data, 8);
        assert!(r.fault.is_none());
        assert_eq!(r.progress.draws_submitted, 1);
        // Completion records remain live until the explicit final publication;
        // queue acceptance alone does not permit retirement.
    }
    let elapsed = start.elapsed();
    let after = d.graphics_counters();
    assert_eq!(after.draw_commands - before.draw_commands, 32);
    assert_eq!(after.submissions - before.submissions, 32);
    assert_eq!(
        [
            after.shader_binary_bytes,
            after.vertex_shader_bytes,
            after.shader_decodes,
            after.vertex_shader_decodes,
            after.descriptor_reads,
            after.vertex_guest_bytes,
            after.index_guest_bytes,
            after.constant_guest_bytes,
            after.guest_texture_bytes,
            after.render_target_guest_bytes,
            after.upload_bytes,
            after.upload_calls,
            after.shader_module_allocations,
            after.pipeline_allocations,
            after.readback_operations,
            after.synchronization_points
        ],
        [
            before.shader_binary_bytes,
            before.vertex_shader_bytes,
            before.shader_decodes,
            before.vertex_shader_decodes,
            before.descriptor_reads,
            before.vertex_guest_bytes,
            before.index_guest_bytes,
            before.constant_guest_bytes,
            before.guest_texture_bytes,
            before.render_target_guest_bytes,
            before.upload_bytes,
            before.upload_calls,
            before.shader_module_allocations,
            before.pipeline_allocations,
            before.readback_operations,
            before.synchronization_points
        ]
    );
    d.publish(c, &mut mem(&mut data)).unwrap();
    check(&data, [175, 159, 159, 159], false);
    eprintln!(
        "FSETP all16 {count} IEEE pairs, warm32 CPU={elapsed:?} before={before:?} after={after:?}"
    );
    d.close(c, &mut mem(&mut data)).unwrap();
}
fn ieee_predicate_pairs() -> Vec<[u32; 2]> {
    let mut pairs = vec![
        [0u32, 0],
        [0, 0x80000000],
        [0x80000000, 0],
        [0x80000000, 0x80000000],
        [1, 0],
        [0x80000001, 0],
        [1, 0x80000001],
        [0x80000000, 0x80000001],
        [0x007fffff, 0x00800000],
        [0x807fffff, 0x80800000],
        [0x7f7fffff, 0x7f800000],
        [0xff800000, 0xff7fffff],
        [0x7f800000, 0x7f800000],
        [0xff800000, 0xff800000],
        [0x7f800000, 0xff800000],
        [0x3f800000, 0xbf800000],
    ];
    for nan in [
        0x7f800001, 0x7fc00000, 0x7fffffff, 0xff800001, 0xffc12345, 0xffffffff,
    ] {
        pairs.extend([[nan, 0x3f800000], [0xbf800000, nan], [nan, nan]]);
    }
    // Bounded deterministic general payloads supplement the named boundaries.
    let mut seed = 0x516e3a29u32;
    for _ in 0..32 {
        seed ^= seed << 13;
        seed ^= seed >> 17;
        seed ^= seed << 5;
        let a = seed;
        seed ^= seed << 13;
        seed ^= seed >> 17;
        seed ^= seed << 5;
        pairs.push([a, seed]);
    }
    pairs.push([0xbf800000, 0x3f800000]);
    pairs
}
#[test]
#[ignore = "requires actual GPU; six ordered float relations with NaNs, signed zeros and subnormals"]
fn driver_float_predicate_ieee_relations_and_code_coherence() {
    let (mut data, mut words, _) = fixture();
    let (mut d, c, _, b, _) = driver();
    let bin = include_bytes!("fixtures/maxwell/float-predicate.bin");
    data[0x4600..0x4600 + bin.len()].copy_from_slice(bin);
    let end = words.split_off(words.len() - 8);
    for (m, v) in [
        (0x2144, 0x600),
        (0x214c, 8),
        (0x2380, 32),
        (0x2384, 0),
        (0x2388, 0x40040),
        (0x2490, 1),
    ] {
        method(&mut words, m, v);
    }
    words.extend(end);
    let pairs = ieee_predicate_pairs();
    let count = pairs.len();
    let mut identity = None;
    for [a, z] in pairs {
        let bytes: Vec<_> = [a, z].into_iter().flat_map(u32::to_le_bytes).collect();
        d.write_backing(b, 0x50, &bytes, &mut mem(&mut data))
            .unwrap();
        let r = submit(&mut d, c, b, &mut data, &words);
        assert!(r.fault.is_none(), "{:?}", r.fault);
        assert_eq!(r.progress.draws_submitted, 1);
        d.publish(c, &mut mem(&mut data)).unwrap();
        check(&data, float_predicate_color(a, z), false);
        d.retire(r.token).unwrap();
        let n = d.graphics_counters();
        let now = [
            n.shader_decodes,
            n.shader_module_allocations,
            n.pipeline_allocations,
        ];
        if let Some(old) = identity {
            assert_eq!(now, old);
        }
        identity = Some(now);
    }
    // Replace just the first ordered LT with GT. For -1<1, its bit1/64
    // disappears: remaining LE|NE=20/64, independently quantized to80.
    let mut code = bin.to_vec();
    let off = 0xa0;
    let old = u64::from_le_bytes(code[off..off + 8].try_into().unwrap());
    assert_eq!(old, 0x5bb1038000570407);
    code[off..off + 8].copy_from_slice(&((old & !(15 << 48)) | (4 << 48)).to_le_bytes());
    d.write_backing(b, 0x4600, &code, &mut mem(&mut data))
        .unwrap();
    let before = d.graphics_counters();
    let r = submit(&mut d, c, b, &mut data, &draw());
    assert!(r.fault.is_none(), "{:?}", r.fault);
    let accepted = d.graphics_counters();
    assert_eq!(accepted.shader_decodes, before.shader_decodes + 1);
    let old = u64::from_le_bytes(code[off..off + 8].try_into().unwrap());
    code[off..off + 8].copy_from_slice(&(old | (1 << 47)).to_le_bytes());
    d.write_backing(b, 0x4600, &code, &mut mem(&mut data))
        .unwrap();
    assert!(submit(&mut d, c, b, &mut data, &draw()).fault.is_some());
    assert_eq!(d.graphics_counters().draw_commands, accepted.draw_commands);
    assert_eq!(d.graphics_counters().submissions, accepted.submissions);
    d.publish(c, &mut mem(&mut data)).unwrap();
    check(&data, [80, 64, 191, 255], false);
    eprintln!(
        "FSETP {count} IEEE pairs counters {:?}",
        d.graphics_counters()
    );
    d.close(c, &mut mem(&mut data)).unwrap();
}
#[test]
#[ignore = "requires actual GPU; floating predicate changes vertex geometry and preserves warm residency"]
fn driver_vertex_float_predicate_geometry_and_warm_cache() {
    let (mut data, mut words, _) = fixture();
    let (mut d, c, _, b, _) = driver();
    let bin = include_bytes!("fixtures/maxwell/vertex-float-predicate.bin");
    data[0x4800..0x4800 + bin.len()].copy_from_slice(bin);
    let end = words.split_off(words.len() - 8);
    method(&mut words, 0x2044, 0x800);
    method(&mut words, 0x204c, 8);
    method(&mut words, 0x2144, 0x400);
    words.extend(end);
    let r = submit(&mut d, c, b, &mut data, &words);
    assert!(r.fault.is_none(), "{:?}", r.fault);
    d.publish(c, &mut mem(&mut data)).unwrap();
    check_conditional_geometry(&data, 0.5);
    d.retire(r.token).unwrap();
    d.write_backing(b, 0x3000, &(-0.4f32).to_le_bytes(), &mut mem(&mut data))
        .unwrap();
    let background: Vec<_> = (0..W * H).flat_map(|_| BG).collect();
    d.write_backing(b, RT as u64, &background, &mut mem(&mut data))
        .unwrap();
    let before = d.graphics_counters();
    let r = submit(&mut d, c, b, &mut data, &draw());
    assert!(r.fault.is_none(), "{:?}", r.fault);
    d.publish(c, &mut mem(&mut data)).unwrap();
    check_conditional_geometry(&data, f64::from(-0.4f32));
    d.retire(r.token).unwrap();
    let after = d.graphics_counters();
    assert_eq!(
        [
            before.vertex_shader_decodes,
            before.shader_decodes,
            before.shader_module_allocations,
            before.pipeline_allocations
        ],
        [
            after.vertex_shader_decodes,
            after.shader_decodes,
            after.shader_module_allocations,
            after.pipeline_allocations
        ]
    );
    assert_eq!(after.vertex_upload_bytes - before.vertex_upload_bytes, 72);
    let bytes: Vec<_> = draw().iter().flat_map(|v| v.to_le_bytes()).collect();
    d.write_backing(b, 0x10000, &bytes, &mut mem(&mut data))
        .unwrap();
    let before = d.graphics_counters();
    let start = std::time::Instant::now();
    for _ in 0..32 {
        let r = submit_existing(&mut d, c, &mut data, 8);
        assert!(r.fault.is_none());
        assert_eq!(r.progress.draws_submitted, 1);
    }
    let elapsed = start.elapsed();
    let after = d.graphics_counters();
    assert_eq!(after.submissions - before.submissions, 32);
    assert_eq!(after.draw_commands - before.draw_commands, 32);
    assert_eq!(
        [
            after.shader_decodes,
            after.vertex_shader_decodes,
            after.shader_binary_bytes,
            after.vertex_shader_bytes,
            after.vertex_guest_bytes,
            after.index_guest_bytes,
            after.constant_guest_bytes,
            after.guest_texture_bytes,
            after.descriptor_reads,
            after.render_target_guest_bytes
        ],
        [
            before.shader_decodes,
            before.vertex_shader_decodes,
            before.shader_binary_bytes,
            before.vertex_shader_bytes,
            before.vertex_guest_bytes,
            before.index_guest_bytes,
            before.constant_guest_bytes,
            before.guest_texture_bytes,
            before.descriptor_reads,
            before.render_target_guest_bytes
        ]
    );
    assert_eq!(
        [
            after.upload_bytes,
            after.upload_calls,
            after.shader_module_allocations,
            after.pipeline_allocations,
            after.readback_operations,
            after.synchronization_points
        ],
        [
            before.upload_bytes,
            before.upload_calls,
            before.shader_module_allocations,
            before.pipeline_allocations,
            before.readback_operations,
            before.synchronization_points
        ]
    );
    d.publish(c, &mut mem(&mut data)).unwrap();
    check_conditional_geometry(&data, f64::from(-0.4f32));
    eprintln!("FSETP warm32 CPU={elapsed:?} before={before:?} after={after:?}");
    d.close(c, &mut mem(&mut data)).unwrap();
}

#[test]
#[ignore = "requires actual GPU; NAN predicate controls guest vertex exports without exporting NaN"]
fn driver_vertex_nan_predicate_controls_primitive_coverage() {
    let (mut data, mut words, _) = fixture();
    let (mut d, c, _, b, _) = driver();
    let mut bin = include_bytes!("fixtures/maxwell/vertex-float-predicate.bin").to_vec();
    let word = u64::from_le_bytes(bin[0x80..0x88].try_into().unwrap());
    assert_eq!(word, 0x5bb1038000670007);
    bin[0x80..0x88].copy_from_slice(&((word & !(15 << 48)) | (8 << 48)).to_le_bytes());
    data[0x4800..0x4800 + bin.len()].copy_from_slice(&bin);
    let end = words.split_off(words.len() - 8);
    method(&mut words, 0x2044, 0x800);
    method(&mut words, 0x204c, 8);
    method(&mut words, 0x2144, 0x400);
    words.extend(end);
    let r = submit(&mut d, c, b, &mut data, &words);
    assert!(r.fault.is_none(), "{:?}", r.fault);
    assert_eq!(r.progress.draws_submitted, 1);
    d.publish(c, &mut mem(&mut data)).unwrap();
    check_conditional_geometry(&data, f64::from(POS[0][0]));
    d.retire(r.token).unwrap();
    let background: Vec<_> = (0..W * H).flat_map(|_| BG).collect();
    for bits in [0x7f800001u32, 0xffc12345, 1, 0x80000000] {
        // Only the comparison's immediate changes. Vertex positions always
        // remain finite. NAN=true moves ALL X to .5, producing zero-area
        // geometry; numeric values leave the asymmetric original triangle.
        let word = u64::from_le_bytes(bin[0x68..0x70].try_into().unwrap());
        bin[0x68..0x70].copy_from_slice(
            &((word & !(u64::from(u32::MAX) << 20)) | (u64::from(bits) << 20)).to_le_bytes(),
        );
        d.write_backing(b, 0x4800, &bin, &mut mem(&mut data))
            .unwrap();
        d.write_backing(b, RT as u64, &background, &mut mem(&mut data))
            .unwrap();
        let before = d.graphics_counters();
        let r = submit(&mut d, c, b, &mut data, &draw());
        assert!(r.fault.is_none(), "{:?}", r.fault);
        assert_eq!(r.progress.draws_submitted, 1);
        let after = d.graphics_counters();
        assert_eq!(
            after.vertex_shader_decodes,
            before.vertex_shader_decodes + 1
        );
        assert_eq!(after.shader_decodes, before.shader_decodes);
        assert_eq!(after.vertex_upload_bytes, before.vertex_upload_bytes);
        d.publish(c, &mut mem(&mut data)).unwrap();
        if f32::from_bits(bits).is_nan() {
            assert_eq!(&data[RT..RT + SIZE], &background);
        } else {
            check_conditional_geometry(&data, f64::from(POS[0][0]));
        }
        d.retire(r.token).unwrap();
    }
    eprintln!("VS NAN finite-output coverage {:?}", d.graphics_counters());
    d.close(c, &mut mem(&mut data)).unwrap();
}

fn reference_field(value: u32, offset: u8, width: u8, signed: bool) -> u32 {
    // Independent scalar oracle builds each destination bit, instead of using
    // production's paired shift/sign-extension or extraction mask.
    let sign = signed && width != 0 && (value >> (offset + width - 1)) & 1 != 0;
    (0..32).fold(0u32, |out, bit| {
        let set = if bit < u32::from(width) {
            (value >> (u32::from(offset) + bit)) & 1 != 0
        } else {
            sign
        };
        out | (u32::from(set) << bit)
    })
}
fn field_inputs(value: u32) -> Vec<u8> {
    [
        value,
        reference_field(value, 5, 11, false),
        reference_field(value, 5, 11, true),
        value,
        reference_field(value, 31, 1, true),
    ]
    .into_iter()
    .flat_map(u32::to_le_bytes)
    .collect()
}
#[test]
#[ignore = "requires actual GPU; signed/unsigned BFE with guest runtime words and independent bit oracle"]
fn driver_bitfield_extraction_guest_data_and_code_coherence() {
    let (mut data, mut words, _) = fixture();
    let (mut d, c, _, b, _) = driver();
    let bin = include_bytes!("fixtures/maxwell/bitfield.bin");
    data[0x4600..0x4600 + bin.len()].copy_from_slice(bin);
    let end = words.split_off(words.len() - 8);
    for (m, v) in [
        (0x2144, 0x600),
        (0x214c, 11),
        (0x2380, 48),
        (0x2384, 0),
        (0x2388, 0x40040),
        (0x2490, 1),
    ] {
        method(&mut words, m, v);
    }
    words.extend(end);
    let mut values = vec![
        0, 1, 0x80000000, 0xffffffff, 0x7fffffff, 0x89abcdef, 0xaaaaaaaa, 0x55555555,
    ];
    let mut seed = 0x42464531u32;
    for _ in 0..32 {
        seed ^= seed << 13;
        seed ^= seed >> 17;
        seed ^= seed << 5;
        values.push(seed);
    }
    values.push(0x89abcdef);
    let count = values.len();
    let mut identity = None;
    for value in values {
        d.write_backing(b, 0x50, &field_inputs(value), &mut mem(&mut data))
            .unwrap();
        let r = submit(&mut d, c, b, &mut data, &words);
        assert!(r.fault.is_none(), "{:?}", r.fault);
        assert_eq!(r.progress.draws_submitted, 1);
        d.publish(c, &mut mem(&mut data)).unwrap();
        check(&data, [64, 128, 191, 255], false);
        d.retire(r.token).unwrap();
        let n = d.graphics_counters();
        let now = [
            n.shader_decodes,
            n.shader_module_allocations,
            n.pipeline_allocations,
        ];
        if let Some(old) = identity {
            assert_eq!(now, old);
        }
        identity = Some(now);
    }
    // A deliberately incorrect expected word proves the guest comparison path
    // can report failure; it cannot simply render an unconditional pass color.
    let mut input = field_inputs(0x89abcdef);
    input[4] ^= 1;
    d.write_backing(b, 0x50, &input, &mut mem(&mut data))
        .unwrap();
    let before = d.graphics_counters();
    let r = submit(&mut d, c, b, &mut data, &draw());
    assert!(r.fault.is_none());
    assert_eq!(r.progress.draws_submitted, 1);
    d.publish(c, &mut mem(&mut data)).unwrap();
    check(&data, [0, 128, 191, 255], false);
    d.retire(r.token).unwrap();
    assert_eq!(d.graphics_counters().shader_decodes, before.shader_decodes);
    let mut code = bin.to_vec();
    let off = 0x98;
    let word = u64::from_le_bytes(code[off..off + 8].try_into().unwrap());
    assert_eq!(word, 0x38000000b057040a);
    code[off..off + 8].copy_from_slice(&((word & !(0xffff << 20)) | (0x1f << 20)).to_le_bytes());
    input[4..8].copy_from_slice(&0u32.to_le_bytes());
    d.write_backing(b, 0x50, &input, &mut mem(&mut data))
        .unwrap();
    d.write_backing(b, 0x4600, &code, &mut mem(&mut data))
        .unwrap();
    let before = d.graphics_counters();
    let r = submit(&mut d, c, b, &mut data, &draw());
    assert!(r.fault.is_none());
    assert_eq!(r.progress.draws_submitted, 1);
    assert_eq!(
        d.graphics_counters().shader_decodes,
        before.shader_decodes + 1
    );
    d.publish(c, &mut mem(&mut data)).unwrap();
    check(&data, [64, 128, 191, 255], false);
    d.retire(r.token).unwrap();
    let bytes: Vec<_> = draw().iter().flat_map(|v| v.to_le_bytes()).collect();
    d.write_backing(b, 0x10000, &bytes, &mut mem(&mut data))
        .unwrap();
    let before = d.graphics_counters();
    let start = std::time::Instant::now();
    for _ in 0..32 {
        let r = submit_existing(&mut d, c, &mut data, 8);
        assert!(r.fault.is_none());
        assert_eq!(r.progress.draws_submitted, 1);
    }
    let elapsed = start.elapsed();
    let after = d.graphics_counters();
    assert_eq!(after.draw_commands - before.draw_commands, 32);
    assert_eq!(after.submissions - before.submissions, 32);
    assert_eq!(
        [
            after.shader_binary_bytes,
            after.vertex_shader_bytes,
            after.shader_decodes,
            after.vertex_shader_decodes,
            after.descriptor_reads,
            after.vertex_guest_bytes,
            after.index_guest_bytes,
            after.constant_guest_bytes,
            after.guest_texture_bytes,
            after.render_target_guest_bytes,
            after.upload_bytes,
            after.upload_calls,
            after.shader_module_allocations,
            after.pipeline_allocations,
            after.readback_operations,
            after.synchronization_points
        ],
        [
            before.shader_binary_bytes,
            before.vertex_shader_bytes,
            before.shader_decodes,
            before.vertex_shader_decodes,
            before.descriptor_reads,
            before.vertex_guest_bytes,
            before.index_guest_bytes,
            before.constant_guest_bytes,
            before.guest_texture_bytes,
            before.render_target_guest_bytes,
            before.upload_bytes,
            before.upload_calls,
            before.shader_module_allocations,
            before.pipeline_allocations,
            before.readback_operations,
            before.synchronization_points
        ]
    );
    let word = u64::from_le_bytes(code[off..off + 8].try_into().unwrap());
    code[off..off + 8].copy_from_slice(&(word | (1 << 40)).to_le_bytes());
    d.write_backing(b, 0x4600, &code, &mut mem(&mut data))
        .unwrap();
    assert!(submit(&mut d, c, b, &mut data, &draw()).fault.is_some());
    assert_eq!(d.graphics_counters().draw_commands, after.draw_commands);
    assert_eq!(d.graphics_counters().submissions, after.submissions);
    d.publish(c, &mut mem(&mut data)).unwrap();
    check(&data, [64, 128, 191, 255], false);
    eprintln!("BFE {count} raw words, warm32 CPU={elapsed:?} before={before:?} after={after:?}");
    d.close(c, &mut mem(&mut data)).unwrap();
}

#[test]
#[ignore = "requires actual GPU; guest vertex BFE clears the sign bit and moves asymmetric geometry"]
fn driver_vertex_bitfield_controls_finite_position_bits() {
    let (mut data, mut words, _) = fixture();
    let (mut d, c, _, b, _) = driver();
    let bin = include_bytes!("fixtures/maxwell/vertex-bitfield.bin");
    data[0x4800..0x4800 + bin.len()].copy_from_slice(bin);
    let end = words.split_off(words.len() - 8);
    for (m, v) in [(0x2044, 0x800), (0x204c, 6), (0x2144, 0x400)] {
        method(&mut words, m, v);
    }
    words.extend(end);
    let r = submit(&mut d, c, b, &mut data, &words);
    assert!(r.fault.is_none(), "{:?}", r.fault);
    assert_eq!(r.progress.draws_submitted, 1);
    d.publish(c, &mut mem(&mut data)).unwrap();
    check_positions(
        &data,
        [255, 0, 0, 255],
        POS.map(|p| [f64::from(p[0]).abs(), f64::from(p[1])]),
    );
    d.retire(r.token).unwrap();
    let before = d.graphics_counters();
    d.write_backing(b, 0x3000, &(-0.4f32).to_le_bytes(), &mut mem(&mut data))
        .unwrap();
    let background: Vec<_> = (0..W * H).flat_map(|_| BG).collect();
    d.write_backing(b, RT as u64, &background, &mut mem(&mut data))
        .unwrap();
    let r = submit(&mut d, c, b, &mut data, &draw());
    assert!(r.fault.is_none());
    assert_eq!(r.progress.draws_submitted, 1);
    let after = d.graphics_counters();
    assert_eq!(after.vertex_shader_decodes, before.vertex_shader_decodes);
    assert_eq!(after.pipeline_allocations, before.pipeline_allocations);
    assert_eq!(after.vertex_upload_bytes - before.vertex_upload_bytes, 72);
    d.publish(c, &mut mem(&mut data)).unwrap();
    check_positions(
        &data,
        [255, 0, 0, 255],
        [
            [f64::from(0.4f32), f64::from(POS[0][1])],
            [f64::from(POS[1][0]), f64::from(POS[1][1])],
            [f64::from(POS[2][0]).abs(), f64::from(POS[2][1])],
        ],
    );
    eprintln!("VS BFE geometry {:?}", d.graphics_counters());
    d.close(c, &mut mem(&mut data)).unwrap();
}

fn ternary_reference(a: u32, b: u32, c: u32, lut: u8) -> u32 {
    // Independent scalar table lookup, not production's vector minterms.
    (0..32).fold(0u32, |out, bit| {
        let index = (((a >> bit) & 1) << 2) | (((b >> bit) & 1) << 1) | ((c >> bit) & 1);
        out | (u32::from((lut >> index) & 1) << bit)
    })
}
fn ternary_draw(
    d: &mut GpuDriver,
    c: ChannelId,
    b: BackingId,
    data: &mut [u8],
    words: &[u32],
    input: [u32; 4],
    color: [u8; 4],
) {
    let mut bytes = [0u8; 16];
    for (chunk, word) in bytes.chunks_exact_mut(4).zip(input) {
        chunk.copy_from_slice(&word.to_le_bytes());
    }
    d.write_backing(b, 0x50, &bytes, &mut mem(data)).unwrap();
    let r = submit(d, c, b, data, words);
    assert!(r.fault.is_none(), "{:?}", r.fault);
    assert_eq!(r.progress.draws_submitted, 1);
    d.publish(c, &mut mem(data)).unwrap();
    check(data, color, false);
    d.retire(r.token).unwrap();
}
#[test]
#[ignore = "requires actual GPU; all256 Maxwell LOP3 LUTs, dynamic operands, aliasing and pending faults"]
fn driver_ternary_all_tables_aliases_and_runtime_coherence() {
    let (mut data, mut words, _) = fixture();
    let (mut d, c, _, b, _) = driver();
    let bin = include_bytes!("fixtures/maxwell/ternary.bin");
    data[0x4600..0x4600 + bin.len()].copy_from_slice(bin);
    let end = words.split_off(words.len() - 8);
    for (m, v) in [
        (0x2144, 0x600),
        (0x214c, 9),
        (0x2380, 32),
        (0x2384, 0),
        (0x2388, 0x40040),
        (0x2490, 1),
    ] {
        method(&mut words, m, v);
    }
    words.extend(end);
    let original = u64::from_le_bytes(bin[0x88..0x90].try_into().unwrap());
    assert_eq!(original, 0x5be7030e80570404);
    let draw_words = draw();
    let start = std::time::Instant::now();
    for lut in 0..=255u8 {
        let op = (original & !(255 << 28)) | (u64::from(lut) << 28);
        d.write_backing(b, 0x4688, &op.to_le_bytes(), &mut mem(&mut data))
            .unwrap();
        // These three words enumerate ALL eight truth-table indices in each
        // byte. Expected repeated LUT bytes follow from that independent fact.
        let input = [
            0xf0f0f0f0,
            0xcccccccc,
            0xaaaaaaaa,
            u32::from(lut) * 0x01010101,
        ];
        let before = d.graphics_counters();
        ternary_draw(
            &mut d,
            c,
            b,
            &mut data,
            if lut == 0 { &words } else { &draw_words },
            input,
            [64, 128, 191, 255],
        );
        assert_eq!(
            d.graphics_counters().shader_decodes,
            before.shader_decodes + 1
        );
    }
    let table_time = start.elapsed();
    d.write_backing(b, 0x4600, bin, &mut mem(&mut data))
        .unwrap();
    let mut triples = vec![
        [0, 0, 0],
        [u32::MAX, u32::MAX, u32::MAX],
        [0x80000000, 0x7fffffff, 1],
        [0xaaaaaaaa, 0x55555555, 0xf0f0f0f0],
    ];
    let mut seed = 0x4c4f5033u32;
    for _ in 0..32 {
        let mut t = [0; 3];
        for v in &mut t {
            seed ^= seed << 13;
            seed ^= seed >> 17;
            seed ^= seed << 5;
            *v = seed;
        }
        triples.push(t);
    }
    let mut identity = None;
    for [a, z, q] in triples {
        ternary_draw(
            &mut d,
            c,
            b,
            &mut data,
            &draw_words,
            [a, z, q, ternary_reference(a, z, q, 0xe8)],
            [64, 128, 191, 255],
        );
        let n = d.graphics_counters();
        let now = [
            n.shader_decodes,
            n.shader_module_allocations,
            n.pipeline_allocations,
        ];
        if let Some(old) = identity {
            assert_eq!(now, old);
        }
        identity = Some(now);
    }
    let a = 0x89abcdef;
    let z = 0xaaaaaaaa;
    let q = 0x55555555;
    let before = d.graphics_counters();
    ternary_draw(
        &mut d,
        c,
        b,
        &mut data,
        &draw_words,
        [a, z, q, ternary_reference(a, z, q, 0xe8) ^ 1],
        [0, 128, 191, 255],
    );
    assert_eq!(d.graphics_counters().shader_decodes, before.shader_decodes);
    let mut code = bin.to_vec();
    let compare = u64::from_le_bytes(code[0x98..0xa0].try_into().unwrap());
    for lut in [0x96u8, 0xe8, 0xca] {
        for dst in [5u8, 6] {
            let op = (original & !(255 | (255 << 28))) | u64::from(dst) | (u64::from(lut) << 28);
            code[0x88..0x90].copy_from_slice(&op.to_le_bytes());
            code[0x98..0xa0]
                .copy_from_slice(&((compare & !(255 << 8)) | (u64::from(dst) << 8)).to_le_bytes());
            d.write_backing(b, 0x4600, &code, &mut mem(&mut data))
                .unwrap();
            ternary_draw(
                &mut d,
                c,
                b,
                &mut data,
                &draw_words,
                [a, z, q, ternary_reference(a, z, q, lut)],
                [64, 128, 191, 255],
            );
        }
    }
    d.write_backing(b, 0x4600, bin, &mut mem(&mut data))
        .unwrap();
    ternary_draw(
        &mut d,
        c,
        b,
        &mut data,
        &draw_words,
        [a, z, q, ternary_reference(a, z, q, 0xe8)],
        [64, 128, 191, 255],
    );
    let bytes: Vec<_> = draw_words.iter().flat_map(|v| v.to_le_bytes()).collect();
    d.write_backing(b, 0x10000, &bytes, &mut mem(&mut data))
        .unwrap();
    let before = d.graphics_counters();
    let start = std::time::Instant::now();
    for _ in 0..32 {
        let r = submit_existing(&mut d, c, &mut data, 8);
        assert!(r.fault.is_none());
        assert_eq!(r.progress.draws_submitted, 1);
    }
    let elapsed = start.elapsed();
    let after = d.graphics_counters();
    assert_eq!(after.draw_commands - before.draw_commands, 32);
    assert_eq!(after.submissions - before.submissions, 32);
    assert_eq!(
        [
            after.shader_binary_bytes,
            after.vertex_shader_bytes,
            after.shader_decodes,
            after.vertex_shader_decodes,
            after.descriptor_reads,
            after.vertex_guest_bytes,
            after.index_guest_bytes,
            after.constant_guest_bytes,
            after.guest_texture_bytes,
            after.render_target_guest_bytes,
            after.upload_bytes,
            after.upload_calls,
            after.shader_module_allocations,
            after.pipeline_allocations,
            after.readback_operations,
            after.synchronization_points
        ],
        [
            before.shader_binary_bytes,
            before.vertex_shader_bytes,
            before.shader_decodes,
            before.vertex_shader_decodes,
            before.descriptor_reads,
            before.vertex_guest_bytes,
            before.index_guest_bytes,
            before.constant_guest_bytes,
            before.guest_texture_bytes,
            before.render_target_guest_bytes,
            before.upload_bytes,
            before.upload_calls,
            before.shader_module_allocations,
            before.pipeline_allocations,
            before.readback_operations,
            before.synchronization_points
        ]
    );
    d.write_backing(
        b,
        0x4688,
        &(original | (1 << 38)).to_le_bytes(),
        &mut mem(&mut data),
    )
    .unwrap();
    assert!(submit(&mut d, c, b, &mut data, &draw_words).fault.is_some());
    assert_eq!(d.graphics_counters().draw_commands, after.draw_commands);
    assert_eq!(d.graphics_counters().submissions, after.submissions);
    d.publish(c, &mut mem(&mut data)).unwrap();
    check(&data, [64, 128, 191, 255], false);
    eprintln!(
        "LOP3 all256 CPU={table_time:?}, warm32 CPU={elapsed:?} before={before:?} after={after:?}"
    );
    d.close(c, &mut mem(&mut data)).unwrap();
}

#[test]
#[ignore = "requires actual GPU; three-source Maxwell LUT controls guest vertex clip positions"]
fn driver_vertex_ternary_alias_controls_geometry() {
    let (mut data, mut words, _) = fixture();
    let (mut d, c, _, b, _) = driver();
    let bin = include_bytes!("fixtures/maxwell/vertex-ternary.bin");
    data[0x4800..0x4800 + bin.len()].copy_from_slice(bin);
    let end = words.split_off(words.len() - 8);
    for (m, v) in [(0x2044, 0x800), (0x204c, 7), (0x2144, 0x400)] {
        method(&mut words, m, v);
    }
    words.extend(end);
    let r = submit(&mut d, c, b, &mut data, &words);
    assert!(r.fault.is_none(), "{:?}", r.fault);
    assert_eq!(r.progress.draws_submitted, 1);
    d.publish(c, &mut mem(&mut data)).unwrap();
    check_positions(
        &data,
        [255, 0, 0, 255],
        POS.map(|p| [f64::from(p[0]).abs(), f64::from(p[1])]),
    );
    d.retire(r.token).unwrap();
    d.write_backing(b, 0x3000, &(-0.4f32).to_le_bytes(), &mut mem(&mut data))
        .unwrap();
    let background: Vec<_> = (0..W * H).flat_map(|_| BG).collect();
    d.write_backing(b, RT as u64, &background, &mut mem(&mut data))
        .unwrap();
    let before = d.graphics_counters();
    let r = submit(&mut d, c, b, &mut data, &draw());
    assert!(r.fault.is_none());
    assert_eq!(r.progress.draws_submitted, 1);
    let after = d.graphics_counters();
    assert_eq!(after.vertex_shader_decodes, before.vertex_shader_decodes);
    assert_eq!(after.pipeline_allocations, before.pipeline_allocations);
    assert_eq!(after.vertex_upload_bytes - before.vertex_upload_bytes, 72);
    d.publish(c, &mut mem(&mut data)).unwrap();
    check_positions(
        &data,
        [255, 0, 0, 255],
        [
            [f64::from(0.4f32), f64::from(POS[0][1])],
            [f64::from(POS[1][0]), f64::from(POS[1][1])],
            [f64::from(POS[2][0]).abs(), f64::from(POS[2][1])],
        ],
    );
    eprintln!("VS LOP3 geometry {:?}", d.graphics_counters());
    d.close(c, &mut mem(&mut data)).unwrap();
}

fn popcount_reference(word: u32, invert: bool) -> u32 {
    // Independent scalar bit summation, rather than WGSL's population-count
    // intrinsic or a production decoder/translator invocation.
    (0..32)
        .map(|bit| ((word >> bit) & 1) ^ u32::from(invert))
        .sum()
}
fn popcount_draw(
    d: &mut GpuDriver,
    c: ChannelId,
    b: BackingId,
    data: &mut [u8],
    words: &[u32],
    value: u32,
    expected: u32,
) {
    let mut bytes = [0u8; 8];
    bytes[..4].copy_from_slice(&value.to_le_bytes());
    bytes[4..].copy_from_slice(&expected.to_le_bytes());
    d.write_backing(b, 0x50, &bytes, &mut mem(data)).unwrap();
    let r = submit(d, c, b, data, words);
    assert!(r.fault.is_none(), "{:?}", r.fault);
    assert_eq!(r.progress.draws_submitted, 1);
    d.publish(c, &mut mem(data)).unwrap();
    check(
        data,
        if expected == popcount_reference(value, false) {
            [64, 128, 191, 255]
        } else {
            [0, 128, 191, 255]
        },
        false,
    );
    d.retire(r.token).unwrap();
}

#[test]
#[ignore = "requires actual GPU; variable-latency POPC, inverted sources, RZ, aliases and runtime mutations"]
fn driver_popcount_dynamic_words_and_wait_coherence() {
    let (mut data, mut words, _) = fixture();
    let (mut d, c, _, b, _) = driver();
    let bin = include_bytes!("fixtures/maxwell/popcount.bin");
    data[0x4600..0x4600 + bin.len()].copy_from_slice(bin);
    let end = words.split_off(words.len() - 8);
    for (m, v) in [
        (0x2144, 0x600),
        (0x214c, 7),
        (0x2380, 24),
        (0x2384, 0),
        (0x2388, 0x40040),
        (0x2490, 1),
    ] {
        method(&mut words, m, v);
    }
    words.extend(end);
    let original = u64::from_le_bytes(bin[0x68..0x70].try_into().unwrap());
    assert_eq!(original, 0x5c08000000470004);
    let draw_words = draw();
    let mut values: Vec<u32> = (0..=32).map(|n| ((1u64 << n) - 1) as u32).collect();
    for bit in 0..32 {
        values.push(1 << bit);
        values.push(!(1 << bit));
    }
    let mut seed = 0x504f5043u32;
    for _ in 0..32 {
        seed ^= seed << 13;
        seed ^= seed >> 17;
        seed ^= seed << 5;
        values.push(seed);
    }
    let mut first = true;
    for invert in [false, true] {
        let op = original | (u64::from(invert) << 40);
        d.write_backing(b, 0x4668, &op.to_le_bytes(), &mut mem(&mut data))
            .unwrap();
        let mut identity = None;
        for &value in &values {
            let expected = popcount_reference(value, invert);
            let mut bytes = [0u8; 8];
            bytes[..4].copy_from_slice(&value.to_le_bytes());
            bytes[4..].copy_from_slice(&expected.to_le_bytes());
            d.write_backing(b, 0x50, &bytes, &mut mem(&mut data))
                .unwrap();
            let r = submit(
                &mut d,
                c,
                b,
                &mut data,
                if first { &words } else { &draw_words },
            );
            first = false;
            assert!(
                r.fault.is_none(),
                "value={value:#x} invert={invert}: {:?}",
                r.fault
            );
            assert_eq!(r.progress.draws_submitted, 1);
            d.publish(c, &mut mem(&mut data)).unwrap();
            check(&data, [64, 128, 191, 255], false);
            d.retire(r.token).unwrap();
            let n = d.graphics_counters();
            let now = [
                n.shader_decodes,
                n.shader_module_allocations,
                n.pipeline_allocations,
            ];
            if let Some(old) = identity {
                assert_eq!(now, old);
            }
            identity = Some(now);
        }
    }
    // Restore plain POPC and deliberately provide an incorrect reference.
    d.write_backing(b, 0x4668, &original.to_le_bytes(), &mut mem(&mut data))
        .unwrap();
    popcount_draw(&mut d, c, b, &mut data, &draw_words, 0x89abcdef, 0);
    let before = d.graphics_counters();
    popcount_draw(
        &mut d,
        c,
        b,
        &mut data,
        &draw_words,
        0x89abcdef,
        popcount_reference(0x89abcdef, false),
    );
    assert_eq!(d.graphics_counters().shader_decodes, before.shader_decodes);
    // RZ still requires the independently validated scoreboard wait. Both
    // zero and inverted-zero produce exact counts, not input-buffer values.
    for invert in [false, true] {
        let op = (original & !(255 << 20)) | (255 << 20) | (u64::from(invert) << 40);
        d.write_backing(b, 0x4668, &op.to_le_bytes(), &mut mem(&mut data))
            .unwrap();
        let mut bytes = [0u8; 8];
        bytes[..4].copy_from_slice(&0x89abcdefu32.to_le_bytes());
        bytes[4..].copy_from_slice(&(if invert { 32u32 } else { 0 }).to_le_bytes());
        d.write_backing(b, 0x50, &bytes, &mut mem(&mut data))
            .unwrap();
        let r = submit(&mut d, c, b, &mut data, &draw_words);
        assert!(r.fault.is_none(), "{:?}", r.fault);
        assert_eq!(r.progress.draws_submitted, 1);
        d.publish(c, &mut mem(&mut data)).unwrap();
        check(&data, [64, 128, 191, 255], false);
        d.retire(r.token).unwrap();
    }
    let bytes: Vec<_> = draw_words.iter().flat_map(|v| v.to_le_bytes()).collect();
    d.write_backing(b, 0x10000, &bytes, &mut mem(&mut data))
        .unwrap();
    let before = d.graphics_counters();
    let start = std::time::Instant::now();
    for _ in 0..32 {
        let r = submit_existing(&mut d, c, &mut data, 8);
        assert!(r.fault.is_none());
        assert_eq!(r.progress.draws_submitted, 1);
    }
    let elapsed = start.elapsed();
    let after = d.graphics_counters();
    assert_eq!(after.draw_commands - before.draw_commands, 32);
    assert_eq!(after.submissions - before.submissions, 32);
    assert_eq!(
        [
            after.shader_binary_bytes,
            after.vertex_shader_bytes,
            after.shader_decodes,
            after.vertex_shader_decodes,
            after.descriptor_reads,
            after.vertex_guest_bytes,
            after.index_guest_bytes,
            after.constant_guest_bytes,
            after.guest_texture_bytes,
            after.render_target_guest_bytes,
            after.upload_bytes,
            after.upload_calls,
            after.shader_module_allocations,
            after.pipeline_allocations,
            after.readback_operations,
            after.synchronization_points
        ],
        [
            before.shader_binary_bytes,
            before.vertex_shader_bytes,
            before.shader_decodes,
            before.vertex_shader_decodes,
            before.descriptor_reads,
            before.vertex_guest_bytes,
            before.index_guest_bytes,
            before.constant_guest_bytes,
            before.guest_texture_bytes,
            before.render_target_guest_bytes,
            before.upload_bytes,
            before.upload_calls,
            before.shader_module_allocations,
            before.pipeline_allocations,
            before.readback_operations,
            before.synchronization_points
        ]
    );
    // Remove only the consumer's wait. A fault must not replay/append work and
    // must leave all 32 previously accepted GPU draws available for publication.
    let ctl = u64::from_le_bytes(bin[0x70..0x78].try_into().unwrap());
    d.write_backing(
        b,
        0x4670,
        &(ctl & !(1 << 11)).to_le_bytes(),
        &mut mem(&mut data),
    )
    .unwrap();
    let failed = submit(&mut d, c, b, &mut data, &draw_words);
    assert!(failed.fault.is_some());
    assert_eq!(d.graphics_counters().draw_commands, after.draw_commands);
    assert_eq!(d.graphics_counters().submissions, after.submissions);
    d.publish(c, &mut mem(&mut data)).unwrap();
    check(&data, [64, 128, 191, 255], false);
    eprintln!(
        "POPC {} raw words per inversion, warm32 CPU={elapsed:?} before={before:?} after={after:?}",
        values.len()
    );
    d.close(c, &mut mem(&mut data)).unwrap();
}

#[test]
#[ignore = "requires actual GPU; guest vertex POPC predicates finite geometry using raw guest X bits"]
fn driver_vertex_popcount_controls_geometry() {
    let (mut data, mut words, _) = fixture();
    let (mut d, c, _, b, _) = driver();
    let bin = include_bytes!("fixtures/maxwell/vertex-popcount.bin");
    data[0x4800..0x4800 + bin.len()].copy_from_slice(bin);
    let end = words.split_off(words.len() - 8);
    for (m, v) in [(0x2044, 0x800), (0x204c, 9), (0x2144, 0x400)] {
        method(&mut words, m, v);
    }
    words.extend(end);
    let r = submit(&mut d, c, b, &mut data, &words);
    assert!(r.fault.is_none(), "{:?}", r.fault);
    assert_eq!(r.progress.draws_submitted, 1);
    d.publish(c, &mut mem(&mut data)).unwrap();
    check_conditional_geometry(&data, 0.5);
    d.retire(r.token).unwrap();
    assert_eq!(popcount_reference((-0.8f32).to_bits(), false), 19);
    assert_eq!(popcount_reference((-0.6f32).to_bits(), false), 18);
    d.write_backing(b, 0x3000, &(-0.6f32).to_le_bytes(), &mut mem(&mut data))
        .unwrap();
    let background: Vec<_> = (0..W * H).flat_map(|_| BG).collect();
    d.write_backing(b, RT as u64, &background, &mut mem(&mut data))
        .unwrap();
    let before = d.graphics_counters();
    let r = submit(&mut d, c, b, &mut data, &draw());
    assert!(r.fault.is_none());
    assert_eq!(r.progress.draws_submitted, 1);
    let after = d.graphics_counters();
    assert_eq!(after.vertex_shader_decodes, before.vertex_shader_decodes);
    assert_eq!(after.pipeline_allocations, before.pipeline_allocations);
    assert_eq!(after.vertex_upload_bytes - before.vertex_upload_bytes, 72);
    d.publish(c, &mut mem(&mut data)).unwrap();
    check_conditional_geometry(&data, f64::from(-0.6f32));
    eprintln!("VS POPC geometry {:?}", d.graphics_counters());
    d.close(c, &mut mem(&mut data)).unwrap();
}

fn leading_bit_reference(word: u32, signed: bool) -> u32 {
    // Independent scalar scan for a bit different from the sign. No host
    // leading-bit/count-zero intrinsic or production shader invocation.
    let wanted = if signed && word & 0x80000000 != 0 {
        0
    } else {
        1
    };
    for bit in (0..32).rev() {
        if (word >> bit) & 1 == wanted {
            return bit;
        }
    }
    u32::MAX
}

fn leading_bit_draw(
    d: &mut GpuDriver,
    c: ChannelId,
    b: BackingId,
    data: &mut [u8],
    words: &[u32],
    value: u32,
    expected: u32,
) {
    let mut bytes = [0u8; 8];
    bytes[..4].copy_from_slice(&value.to_le_bytes());
    bytes[4..].copy_from_slice(&expected.to_le_bytes());
    d.write_backing(b, 0x50, &bytes, &mut mem(data)).unwrap();
    let r = submit(d, c, b, data, words);
    assert!(r.fault.is_none(), "{:?}", r.fault);
    assert_eq!(r.progress.draws_submitted, 1);
    d.publish(c, &mut mem(data)).unwrap();
    check(
        data,
        if expected == leading_bit_reference(value, false) {
            [64, 128, 191, 255]
        } else {
            [0, 128, 191, 255]
        },
        false,
    );
    d.retire(r.token).unwrap();
}

#[test]
#[ignore = "requires actual GPU; variable-latency FLO signed/unsigned indices, sentinel, RZ and aliases"]
fn driver_leading_bit_dynamic_words_and_wait_coherence() {
    let (mut data, mut words, _) = fixture();
    let (mut d, c, _, b, _) = driver();
    let bin = include_bytes!("fixtures/maxwell/leading-bit.bin");
    data[0x4600..0x4600 + bin.len()].copy_from_slice(bin);
    let end = words.split_off(words.len() - 8);
    for (m, v) in [
        (0x2144, 0x600),
        (0x214c, 7),
        (0x2380, 24),
        (0x2384, 0),
        (0x2388, 0x40040),
        (0x2490, 1),
    ] {
        method(&mut words, m, v);
    }
    words.extend(end);
    let original = u64::from_le_bytes(bin[0x68..0x70].try_into().unwrap());
    assert_eq!(original, 0x5c30000000470004);
    let draw_words = draw();
    let mut values = vec![0, u32::MAX, 0x80000000, 0x7fffffff];
    for bit in 0..32 {
        values.push(1 << bit);
        values.push(!(1 << bit));
        values.push(((1u64 << (bit + 1)) - 1) as u32);
    }
    let mut seed = 0x464c4f31u32;
    for _ in 0..32 {
        seed ^= seed << 13;
        seed ^= seed >> 17;
        seed ^= seed << 5;
        values.push(seed);
    }
    let mut first = true;
    for signed in [false, true] {
        let op = original | (u64::from(signed) << 48);
        d.write_backing(b, 0x4668, &op.to_le_bytes(), &mut mem(&mut data))
            .unwrap();
        let mut identity = None;
        for &value in &values {
            let expected = leading_bit_reference(value, signed);
            let mut bytes = [0u8; 8];
            bytes[..4].copy_from_slice(&value.to_le_bytes());
            bytes[4..].copy_from_slice(&expected.to_le_bytes());
            d.write_backing(b, 0x50, &bytes, &mut mem(&mut data))
                .unwrap();
            let r = submit(
                &mut d,
                c,
                b,
                &mut data,
                if first { &words } else { &draw_words },
            );
            first = false;
            assert!(
                r.fault.is_none(),
                "value={value:#x} signed={signed}: {:?}",
                r.fault
            );
            assert_eq!(r.progress.draws_submitted, 1);
            d.publish(c, &mut mem(&mut data)).unwrap();
            check(&data, [64, 128, 191, 255], false);
            d.retire(r.token).unwrap();
            let n = d.graphics_counters();
            let now = [
                n.shader_decodes,
                n.shader_module_allocations,
                n.pipeline_allocations,
            ];
            if let Some(old) = identity {
                assert_eq!(now, old);
            }
            identity = Some(now);
        }
    }
    // Restore unsigned FLO and deliberately provide an incorrect reference.
    d.write_backing(b, 0x4668, &original.to_le_bytes(), &mut mem(&mut data))
        .unwrap();
    leading_bit_draw(&mut d, c, b, &mut data, &draw_words, 0x89abcdef, 0);
    let before = d.graphics_counters();
    leading_bit_draw(
        &mut d,
        c,
        b,
        &mut data,
        &draw_words,
        0x89abcdef,
        leading_bit_reference(0x89abcdef, false),
    );
    assert_eq!(d.graphics_counters().shader_decodes, before.shader_decodes);
    // RZ still requires the independently validated scoreboard wait. Both
    // signed modes produce the raw FFFFFFFF sentinel, not input-buffer values.
    for signed in [false, true] {
        let op = (original & !(255 << 20)) | (255 << 20) | (u64::from(signed) << 48);
        d.write_backing(b, 0x4668, &op.to_le_bytes(), &mut mem(&mut data))
            .unwrap();
        let mut bytes = [0u8; 8];
        bytes[..4].copy_from_slice(&0x89abcdefu32.to_le_bytes());
        bytes[4..].copy_from_slice(&u32::MAX.to_le_bytes());
        d.write_backing(b, 0x50, &bytes, &mut mem(&mut data))
            .unwrap();
        let r = submit(&mut d, c, b, &mut data, &draw_words);
        assert!(r.fault.is_none(), "{:?}", r.fault);
        assert_eq!(r.progress.draws_submitted, 1);
        d.publish(c, &mut mem(&mut data)).unwrap();
        check(&data, [64, 128, 191, 255], false);
        d.retire(r.token).unwrap();
    }
    let bytes: Vec<_> = draw_words.iter().flat_map(|v| v.to_le_bytes()).collect();
    d.write_backing(b, 0x10000, &bytes, &mut mem(&mut data))
        .unwrap();
    let before = d.graphics_counters();
    let start = std::time::Instant::now();
    for _ in 0..32 {
        let r = submit_existing(&mut d, c, &mut data, 8);
        assert!(r.fault.is_none());
        assert_eq!(r.progress.draws_submitted, 1);
    }
    let elapsed = start.elapsed();
    let after = d.graphics_counters();
    assert_eq!(after.draw_commands - before.draw_commands, 32);
    assert_eq!(after.submissions - before.submissions, 32);
    assert_eq!(
        [
            after.shader_binary_bytes,
            after.vertex_shader_bytes,
            after.shader_decodes,
            after.vertex_shader_decodes,
            after.descriptor_reads,
            after.vertex_guest_bytes,
            after.index_guest_bytes,
            after.constant_guest_bytes,
            after.guest_texture_bytes,
            after.render_target_guest_bytes,
            after.upload_bytes,
            after.upload_calls,
            after.shader_module_allocations,
            after.pipeline_allocations,
            after.readback_operations,
            after.synchronization_points
        ],
        [
            before.shader_binary_bytes,
            before.vertex_shader_bytes,
            before.shader_decodes,
            before.vertex_shader_decodes,
            before.descriptor_reads,
            before.vertex_guest_bytes,
            before.index_guest_bytes,
            before.constant_guest_bytes,
            before.guest_texture_bytes,
            before.render_target_guest_bytes,
            before.upload_bytes,
            before.upload_calls,
            before.shader_module_allocations,
            before.pipeline_allocations,
            before.readback_operations,
            before.synchronization_points
        ]
    );
    // Remove only the consumer's wait. A fault must not replay/append work and
    // must leave all 32 previously accepted GPU draws available for publication.
    let ctl = u64::from_le_bytes(bin[0x70..0x78].try_into().unwrap());
    d.write_backing(
        b,
        0x4670,
        &(ctl & !(1 << 11)).to_le_bytes(),
        &mut mem(&mut data),
    )
    .unwrap();
    let failed = submit(&mut d, c, b, &mut data, &draw_words);
    assert!(failed.fault.is_some());
    assert_eq!(d.graphics_counters().draw_commands, after.draw_commands);
    assert_eq!(d.graphics_counters().submissions, after.submissions);
    d.publish(c, &mut mem(&mut data)).unwrap();
    check(&data, [64, 128, 191, 255], false);
    eprintln!(
        "FLO {} raw words per signed mode, warm32 CPU={elapsed:?} before={before:?} after={after:?}",
        values.len()
    );
    d.close(c, &mut mem(&mut data)).unwrap();
}

#[test]
#[ignore = "requires actual GPU; signed FLO of raw vertex X selects geometry through guest predicates"]
fn driver_vertex_leading_bit_controls_geometry() {
    let (mut data, mut words, _) = fixture();
    let (mut d, c, _, b, _) = driver();
    let bin = include_bytes!("fixtures/maxwell/vertex-leading-bit.bin");
    data[0x4800..0x4800 + bin.len()].copy_from_slice(bin);
    let end = words.split_off(words.len() - 8);
    for (m, v) in [(0x2044, 0x800), (0x204c, 9), (0x2144, 0x400)] {
        method(&mut words, m, v);
    }
    words.extend(end);
    assert_eq!(leading_bit_reference(POS[0][0].to_bits(), true), 30);
    assert_eq!(leading_bit_reference(POS[1][0].to_bits(), true), 29);
    assert_eq!(leading_bit_reference(POS[2][0].to_bits(), true), 30);
    let r = submit(&mut d, c, b, &mut data, &words);
    assert!(r.fault.is_none(), "{:?}", r.fault);
    assert_eq!(r.progress.draws_submitted, 1);
    d.publish(c, &mut mem(&mut data)).unwrap();
    check_positions(
        &data,
        [255, 0, 0, 255],
        [
            POS[0].map(f64::from),
            [0.5, f64::from(POS[1][1])],
            POS[2].map(f64::from),
        ],
    );
    d.retire(r.token).unwrap();
    assert_eq!(leading_bit_reference((-0.6f32).to_bits(), true), 30);
    d.write_backing(b, 0x3018, &(-0.6f32).to_le_bytes(), &mut mem(&mut data))
        .unwrap();
    let background: Vec<_> = (0..W * H).flat_map(|_| BG).collect();
    d.write_backing(b, RT as u64, &background, &mut mem(&mut data))
        .unwrap();
    let before = d.graphics_counters();
    let r = submit(&mut d, c, b, &mut data, &draw());
    assert!(r.fault.is_none());
    assert_eq!(r.progress.draws_submitted, 1);
    let after = d.graphics_counters();
    assert_eq!(after.vertex_shader_decodes, before.vertex_shader_decodes);
    assert_eq!(after.pipeline_allocations, before.pipeline_allocations);
    assert_eq!(after.vertex_upload_bytes - before.vertex_upload_bytes, 72);
    d.publish(c, &mut mem(&mut data)).unwrap();
    check_positions(
        &data,
        [255, 0, 0, 255],
        [
            POS[0].map(f64::from),
            [f64::from(-0.6f32), f64::from(POS[1][1])],
            POS[2].map(f64::from),
        ],
    );
    eprintln!("VS FLO geometry {:?}", d.graphics_counters());
    d.close(c, &mut mem(&mut data)).unwrap();
}

fn int_to_float_reference(word: u32, signed: bool) -> u32 {
    // Independent oracle: binary-search adjacent positive binary32 numbers.
    // Every u32/i32 and every candidate f32 is exact in f64. Choose the nearer
    // candidate, then even encoding on ties. No production bit-packing steps
    // or integer-to-f32 conversion are used to derive the expected result.
    let value = if signed {
        f64::from(word as i32)
    } else {
        f64::from(word)
    };
    let magnitude = value.abs();
    let mut low = 0u32;
    let mut high = 0x4f800000u32; // 2^32, includes rounded UINT_MAX.
    while low < high {
        let mid = low + (high - low) / 2;
        if f64::from(f32::from_bits(mid)) < magnitude {
            low = mid + 1;
        } else {
            high = mid;
        }
    }
    let upper = low;
    let bits = if upper == 0 || f64::from(f32::from_bits(upper)) == magnitude {
        upper
    } else {
        let lower = upper - 1;
        let down = magnitude - f64::from(f32::from_bits(lower));
        let up = f64::from(f32::from_bits(upper)) - magnitude;
        if down < up || (down == up && lower & 1 == 0) {
            lower
        } else {
            upper
        }
    };
    bits | if value < 0.0 { 0x80000000 } else { 0 }
}

fn int_to_float_draw(
    d: &mut GpuDriver,
    c: ChannelId,
    b: BackingId,
    data: &mut [u8],
    words: &[u32],
    value: u32,
    expected: u32,
) {
    let mut bytes = [0u8; 8];
    bytes[..4].copy_from_slice(&value.to_le_bytes());
    bytes[4..].copy_from_slice(&expected.to_le_bytes());
    d.write_backing(b, 0x50, &bytes, &mut mem(data)).unwrap();
    let r = submit(d, c, b, data, words);
    assert!(r.fault.is_none(), "{:?}", r.fault);
    assert_eq!(r.progress.draws_submitted, 1);
    d.publish(c, &mut mem(data)).unwrap();
    check(
        data,
        if expected == int_to_float_reference(value, false) {
            [64, 128, 191, 255]
        } else {
            [0, 128, 191, 255]
        },
        false,
    );
    d.retire(r.token).unwrap();
}

#[test]
#[ignore = "requires actual GPU; variable-latency I2F RN32 exact signed/unsigned rounding, RZ and aliases"]
fn driver_int_to_float_dynamic_words_and_wait_coherence() {
    let (mut data, mut words, _) = fixture();
    let (mut d, c, _, b, _) = driver();
    let bin = include_bytes!("fixtures/maxwell/int-to-float.bin");
    data[0x4600..0x4600 + bin.len()].copy_from_slice(bin);
    let end = words.split_off(words.len() - 8);
    for (m, v) in [
        (0x2144, 0x600),
        (0x214c, 7),
        (0x2380, 24),
        (0x2384, 0),
        (0x2388, 0x40040),
        (0x2490, 1),
    ] {
        method(&mut words, m, v);
    }
    words.extend(end);
    let original = u64::from_le_bytes(bin[0x68..0x70].try_into().unwrap());
    assert_eq!(original, 0x5cb8000000470a04);
    let draw_words = draw();
    let mut values = vec![
        0,
        u32::MAX,
        0x80000000,
        0x7fffffff,
        0xfeffffff,
        0xfefffffd,
        0xfdfffffe,
        0xfdfffffa,
    ];
    for bit in 0..32 {
        values.push(1 << bit);
        values.push(!(1 << bit));
        values.push(((1u64 << bit) - 1) as u32);
    }
    // Half-ULP ties above 2^24: even, odd and significand carry. Include the
    // last adjacent integer on each side; u64 prevents generating overflow.
    for exponent in 24..32 {
        let unit = 1u64 << (exponent - 23);
        let base = 1u64 << exponent;
        for candidate in [
            base + unit / 2,
            base + unit + unit / 2,
            (1u64 << (exponent + 1)) - unit / 2,
        ] {
            for value in [candidate - 1, candidate, candidate + 1] {
                if let Ok(word) = u32::try_from(value) {
                    values.push(word);
                }
            }
        }
    }
    let mut seed = 0x49324631u32;
    for _ in 0..32 {
        seed ^= seed << 13;
        seed ^= seed >> 17;
        seed ^= seed << 5;
        values.push(seed);
    }
    let mut first = true;
    for signed in [false, true] {
        let op = original | (u64::from(signed) << 13);
        d.write_backing(b, 0x4668, &op.to_le_bytes(), &mut mem(&mut data))
            .unwrap();
        let mut identity = None;
        for &value in &values {
            let expected = int_to_float_reference(value, signed);
            let mut bytes = [0u8; 8];
            bytes[..4].copy_from_slice(&value.to_le_bytes());
            bytes[4..].copy_from_slice(&expected.to_le_bytes());
            d.write_backing(b, 0x50, &bytes, &mut mem(&mut data))
                .unwrap();
            let r = submit(
                &mut d,
                c,
                b,
                &mut data,
                if first { &words } else { &draw_words },
            );
            first = false;
            assert!(
                r.fault.is_none(),
                "value={value:#x} signed={signed}: {:?}",
                r.fault
            );
            assert_eq!(r.progress.draws_submitted, 1);
            d.publish(c, &mut mem(&mut data)).unwrap();
            check(&data, [64, 128, 191, 255], false);
            d.retire(r.token).unwrap();
            let n = d.graphics_counters();
            let now = [
                n.shader_decodes,
                n.shader_module_allocations,
                n.pipeline_allocations,
            ];
            if let Some(old) = identity {
                assert_eq!(now, old);
            }
            identity = Some(now);
        }
    }
    // Restore unsigned I2F and deliberately provide an incorrect reference.
    d.write_backing(b, 0x4668, &original.to_le_bytes(), &mut mem(&mut data))
        .unwrap();
    int_to_float_draw(&mut d, c, b, &mut data, &draw_words, 0x89abcdef, 0);
    let before = d.graphics_counters();
    int_to_float_draw(
        &mut d,
        c,
        b,
        &mut data,
        &draw_words,
        0x89abcdef,
        int_to_float_reference(0x89abcdef, false),
    );
    assert_eq!(d.graphics_counters().shader_decodes, before.shader_decodes);
    // RZ still requires the validated scoreboard wait. Both signed modes
    // produce positive zero, independently of the nonzero guest input.
    for signed in [false, true] {
        let op = (original & !(255 << 20)) | (255 << 20) | (u64::from(signed) << 13);
        d.write_backing(b, 0x4668, &op.to_le_bytes(), &mut mem(&mut data))
            .unwrap();
        let mut bytes = [0u8; 8];
        bytes[..4].copy_from_slice(&0x89abcdefu32.to_le_bytes());
        bytes[4..].copy_from_slice(&0u32.to_le_bytes());
        d.write_backing(b, 0x50, &bytes, &mut mem(&mut data))
            .unwrap();
        let r = submit(&mut d, c, b, &mut data, &draw_words);
        assert!(r.fault.is_none(), "{:?}", r.fault);
        assert_eq!(r.progress.draws_submitted, 1);
        d.publish(c, &mut mem(&mut data)).unwrap();
        check(&data, [64, 128, 191, 255], false);
        d.retire(r.token).unwrap();
    }
    let bytes: Vec<_> = draw_words.iter().flat_map(|v| v.to_le_bytes()).collect();
    d.write_backing(b, 0x10000, &bytes, &mut mem(&mut data))
        .unwrap();
    let before = d.graphics_counters();
    let start = std::time::Instant::now();
    for _ in 0..32 {
        let r = submit_existing(&mut d, c, &mut data, 8);
        assert!(r.fault.is_none());
        assert_eq!(r.progress.draws_submitted, 1);
    }
    let elapsed = start.elapsed();
    let after = d.graphics_counters();
    assert_eq!(after.draw_commands - before.draw_commands, 32);
    assert_eq!(after.submissions - before.submissions, 32);
    assert_eq!(
        [
            after.shader_binary_bytes,
            after.vertex_shader_bytes,
            after.shader_decodes,
            after.vertex_shader_decodes,
            after.descriptor_reads,
            after.vertex_guest_bytes,
            after.index_guest_bytes,
            after.constant_guest_bytes,
            after.guest_texture_bytes,
            after.render_target_guest_bytes,
            after.upload_bytes,
            after.upload_calls,
            after.shader_module_allocations,
            after.pipeline_allocations,
            after.readback_operations,
            after.synchronization_points
        ],
        [
            before.shader_binary_bytes,
            before.vertex_shader_bytes,
            before.shader_decodes,
            before.vertex_shader_decodes,
            before.descriptor_reads,
            before.vertex_guest_bytes,
            before.index_guest_bytes,
            before.constant_guest_bytes,
            before.guest_texture_bytes,
            before.render_target_guest_bytes,
            before.upload_bytes,
            before.upload_calls,
            before.shader_module_allocations,
            before.pipeline_allocations,
            before.readback_operations,
            before.synchronization_points
        ]
    );
    // Remove only the consumer's wait. A fault must not replay/append work and
    // must leave all 32 previously accepted GPU draws available for publication.
    let ctl = u64::from_le_bytes(bin[0x70..0x78].try_into().unwrap());
    d.write_backing(
        b,
        0x4670,
        &(ctl & !(1 << 11)).to_le_bytes(),
        &mut mem(&mut data),
    )
    .unwrap();
    let failed = submit(&mut d, c, b, &mut data, &draw_words);
    assert!(failed.fault.is_some());
    assert_eq!(d.graphics_counters().draw_commands, after.draw_commands);
    assert_eq!(d.graphics_counters().submissions, after.submissions);
    d.publish(c, &mut mem(&mut data)).unwrap();
    check(&data, [64, 128, 191, 255], false);
    eprintln!(
        "I2F {} raw words per signed mode, warm32 CPU={elapsed:?} before={before:?} after={after:?}",
        values.len()
    );
    d.close(c, &mut mem(&mut data)).unwrap();
}

#[test]
#[ignore = "requires actual GPU; signed I2F of raw vertex X selects finite geometry through guest predicates"]
fn driver_vertex_int_to_float_controls_geometry() {
    let (mut data, mut words, _) = fixture();
    let (mut d, c, _, b, _) = driver();
    let bin = include_bytes!("fixtures/maxwell/vertex-int-to-float.bin");
    data[0x4800..0x4800 + bin.len()].copy_from_slice(bin);
    let end = words.split_off(words.len() - 8);
    for (m, v) in [(0x2044, 0x800), (0x204c, 9), (0x2144, 0x400)] {
        method(&mut words, m, v);
    }
    words.extend(end);
    assert_eq!(
        int_to_float_reference(POS[0][0].to_bits(), true),
        0xce816666
    );
    assert_eq!(
        int_to_float_reference(POS[1][0].to_bits(), true),
        0x4e7c6666
    );
    assert_eq!(
        int_to_float_reference(POS[2][0].to_bits(), true),
        0xce836666
    );
    let r = submit(&mut d, c, b, &mut data, &words);
    assert!(r.fault.is_none(), "{:?}", r.fault);
    assert_eq!(r.progress.draws_submitted, 1);
    d.publish(c, &mut mem(&mut data)).unwrap();
    check_conditional_geometry(&data, 0.5);
    d.retire(r.token).unwrap();
    assert_eq!(
        int_to_float_reference((-0.6f32).to_bits(), true),
        0xce81cccd
    );
    d.write_backing(b, 0x3000, &(-0.6f32).to_le_bytes(), &mut mem(&mut data))
        .unwrap();
    let background: Vec<_> = (0..W * H).flat_map(|_| BG).collect();
    d.write_backing(b, RT as u64, &background, &mut mem(&mut data))
        .unwrap();
    let before = d.graphics_counters();
    let r = submit(&mut d, c, b, &mut data, &draw());
    assert!(r.fault.is_none());
    assert_eq!(r.progress.draws_submitted, 1);
    let after = d.graphics_counters();
    assert_eq!(after.vertex_shader_decodes, before.vertex_shader_decodes);
    assert_eq!(after.pipeline_allocations, before.pipeline_allocations);
    assert_eq!(after.vertex_upload_bytes - before.vertex_upload_bytes, 72);
    d.publish(c, &mut mem(&mut data)).unwrap();
    check_conditional_geometry(&data, f64::from(-0.6f32));
    eprintln!("VS I2F geometry {:?}", d.graphics_counters());
    d.close(c, &mut mem(&mut data)).unwrap();
}

#[test]
fn integer_conversion_reference_has_exact_rn_even_boundaries() {
    for (word, signed, bits) in [
        (0, false, 0),
        (0, true, 0),
        (0xffffffff, false, 0x4f800000),
        (0xffffffff, true, 0xbf800000),
        (0x7fffffff, true, 0x4f000000),
        (0x80000000, true, 0xcf000000),
        (0x01000001, false, 0x4b800000),
        (0x01000003, false, 0x4b800002),
        (0xfeffffff, true, 0xcb800000),
        (0xfefffffd, true, 0xcb800002),
    ] {
        assert_eq!(int_to_float_reference(word, signed), bits);
    }
}
fn float_set_reference(a: u32, b: u32, mode: u8, float_result: bool) -> u32 {
    // Independent numeric relations in binary64, which embeds every finite
    // binary32 value exactly; production uses raw integer classification/order.
    let a = f64::from(f32::from_bits(a));
    let b = f64::from(f32::from_bits(b));
    let nan = a.is_nan() || b.is_nan();
    let yes = match mode {
        0 => false,
        1 => !nan && a < b,
        2 => !nan && a == b,
        3 => !nan && a <= b,
        4 => !nan && a > b,
        5 => !nan && a != b,
        6 => !nan && a >= b,
        7 => !nan,
        8 => nan,
        9 => nan || a < b,
        10 => nan || a == b,
        11 => nan || a <= b,
        12 => nan || a > b,
        13 => nan || a != b,
        14 => nan || a >= b,
        15 => true,
        _ => panic!("invalid test condition"),
    };
    if !yes {
        0
    } else if float_result {
        0x3f800000
    } else {
        u32::MAX
    }
}
fn float_set_draw(
    d: &mut GpuDriver,
    c: ChannelId,
    b: BackingId,
    data: &mut [u8],
    words: &[u32],
    payload: [u32; 3],
    matches: bool,
) {
    let mut bytes = [0u8; 12];
    for (slot, word) in bytes.chunks_exact_mut(4).zip(payload) {
        slot.copy_from_slice(&word.to_le_bytes());
    }
    d.write_backing(b, 0x50, &bytes, &mut mem(data)).unwrap();
    let r = submit(d, c, b, data, words);
    assert!(r.fault.is_none(), "{:?}", r.fault);
    assert_eq!(r.progress.draws_submitted, 1);
    d.publish(c, &mut mem(data)).unwrap();
    check(data, [if matches { 64 } else { 0 }, 128, 191, 255], false);
    d.retire(r.token).unwrap();
}
#[test]
#[ignore = "requires actual GPU; all16 FSET conditions, exact BOOL/BF bits and independent P0 preservation"]
fn driver_float_set_all_conditions_and_raw_results() {
    let (mut data, mut words, _) = fixture();
    let (mut d, c, _, b, _) = driver();
    let bin = include_bytes!("fixtures/maxwell/float-set.bin");
    data[0x4600..0x4600 + bin.len()].copy_from_slice(bin);
    let end = words.split_off(words.len() - 8);
    for (m, v) in [
        (0x2144, 0x600),
        (0x214c, 8),
        (0x2380, 28),
        (0x2384, 0),
        (0x2388, 0x40040),
        (0x2490, 1),
    ] {
        method(&mut words, m, v);
    }
    words.extend(end);
    let original = u64::from_le_bytes(bin[0x78..0x80].try_into().unwrap());
    assert_eq!(original, 0x5801038000570404);
    let draw_words = draw();
    let pairs = ieee_predicate_pairs();
    let mut first = true;
    for float_result in [false, true] {
        for mode in 0..16u8 {
            let op = (original & !(31u64 << 48))
                | (u64::from(mode) << 48)
                | (u64::from(float_result) << 52);
            d.write_backing(b, 0x4678, &op.to_le_bytes(), &mut mem(&mut data))
                .unwrap();
            let mut identity = None;
            for &[a, z] in &pairs {
                float_set_draw(
                    &mut d,
                    c,
                    b,
                    &mut data,
                    if first { &words } else { &draw_words },
                    [a, z, float_set_reference(a, z, mode, float_result)],
                    true,
                );
                first = false;
                let n = d.graphics_counters();
                let now = [
                    n.shader_decodes,
                    n.shader_module_allocations,
                    n.pipeline_allocations,
                ];
                if let Some(old) = identity {
                    assert_eq!(now, old);
                }
                identity = Some(now);
            }
        }
    }
    d.write_backing(b, 0x4678, &original.to_le_bytes(), &mut mem(&mut data))
        .unwrap();
    float_set_draw(
        &mut d,
        c,
        b,
        &mut data,
        &draw_words,
        [0xbf800000, 0x3f800000, 0],
        false,
    );
    let before = d.graphics_counters();
    float_set_draw(
        &mut d,
        c,
        b,
        &mut data,
        &draw_words,
        [0xbf800000, 0x3f800000, u32::MAX],
        true,
    );
    assert_eq!(d.graphics_counters().shader_decodes, before.shader_decodes);
    // Destination aliases the other operand too; adjust the guest integer
    // comparator to observe R5, rather than hiding incorrect output in R4.
    let compare = u64::from_le_bytes(bin[0x80..0x88].try_into().unwrap());
    let right_alias = (original & !255) | 5;
    d.write_backing(b, 0x4678, &right_alias.to_le_bytes(), &mut mem(&mut data))
        .unwrap();
    d.write_backing(
        b,
        0x4680,
        &((compare & !(255 << 8)) | (5 << 8)).to_le_bytes(),
        &mut mem(&mut data),
    )
    .unwrap();
    float_set_draw(
        &mut d,
        c,
        b,
        &mut data,
        &draw_words,
        [0xbf800000, 0x3f800000, u32::MAX],
        true,
    );
    d.write_backing(b, 0x4680, &compare.to_le_bytes(), &mut mem(&mut data))
        .unwrap();
    // Both operands are RZ: EQ is true regardless of poisoned guest values.
    for float_result in [false, true] {
        let op = (original & !((255 << 8) | (255 << 20) | (31u64 << 48)))
            | (255 << 8)
            | (255 << 20)
            | (2 << 48)
            | (u64::from(float_result) << 52);
        d.write_backing(b, 0x4678, &op.to_le_bytes(), &mut mem(&mut data))
            .unwrap();
        let bits = if float_result { 0x3f800000 } else { u32::MAX };
        float_set_draw(
            &mut d,
            c,
            b,
            &mut data,
            &draw_words,
            [0x7f800001, 0x3f800000, bits],
            true,
        );
    }
    d.write_backing(b, 0x4678, &original.to_le_bytes(), &mut mem(&mut data))
        .unwrap();
    float_set_draw(
        &mut d,
        c,
        b,
        &mut data,
        &draw_words,
        [0xbf800000, 0x3f800000, u32::MAX],
        true,
    );
    let bytes: Vec<_> = draw_words.iter().flat_map(|v| v.to_le_bytes()).collect();
    d.write_backing(b, 0x10000, &bytes, &mut mem(&mut data))
        .unwrap();
    let before = d.graphics_counters();
    let start = std::time::Instant::now();
    for _ in 0..32 {
        let r = submit_existing(&mut d, c, &mut data, 8);
        assert!(r.fault.is_none());
        assert_eq!(r.progress.draws_submitted, 1);
    }
    let elapsed = start.elapsed();
    let after = d.graphics_counters();
    assert_eq!(after.draw_commands - before.draw_commands, 32);
    assert_eq!(after.submissions - before.submissions, 32);
    assert_eq!(
        [
            after.shader_binary_bytes,
            after.vertex_shader_bytes,
            after.shader_decodes,
            after.vertex_shader_decodes,
            after.descriptor_reads,
            after.vertex_guest_bytes,
            after.index_guest_bytes,
            after.constant_guest_bytes,
            after.guest_texture_bytes,
            after.render_target_guest_bytes,
            after.upload_bytes,
            after.upload_calls,
            after.shader_module_allocations,
            after.pipeline_allocations,
            after.readback_operations,
            after.synchronization_points
        ],
        [
            before.shader_binary_bytes,
            before.vertex_shader_bytes,
            before.shader_decodes,
            before.vertex_shader_decodes,
            before.descriptor_reads,
            before.vertex_guest_bytes,
            before.index_guest_bytes,
            before.constant_guest_bytes,
            before.guest_texture_bytes,
            before.render_target_guest_bytes,
            before.upload_bytes,
            before.upload_calls,
            before.shader_module_allocations,
            before.pipeline_allocations,
            before.readback_operations,
            before.synchronization_points
        ]
    );
    // Unsupported CC faults without appending or replaying GPU work, retaining
    // the earlier pending output for an explicit visibility boundary.
    d.write_backing(
        b,
        0x4678,
        &(original | (1 << 47)).to_le_bytes(),
        &mut mem(&mut data),
    )
    .unwrap();
    let failed = submit(&mut d, c, b, &mut data, &draw_words);
    assert!(failed.fault.is_some());
    assert_eq!(d.graphics_counters().draw_commands, after.draw_commands);
    assert_eq!(d.graphics_counters().submissions, after.submissions);
    d.publish(c, &mut mem(&mut data)).unwrap();
    check(&data, [64, 128, 191, 255], false);
    eprintln!(
        "FSET all32 modes, {} IEEE pairs each, warm32 CPU={elapsed:?} before={before:?} after={after:?}",
        pairs.len()
    );
    d.close(c, &mut mem(&mut data)).unwrap();
}
#[test]
#[ignore = "requires actual GPU; FSET BF/BOOL GPR comparison changes genuine vertex coverage"]
fn driver_vertex_float_set_controls_geometry() {
    let (mut data, mut words, _) = fixture();
    let (mut d, c, _, b, _) = driver();
    let bin = include_bytes!("fixtures/maxwell/vertex-float-set.bin");
    data[0x4800..0x4800 + bin.len()].copy_from_slice(bin);
    let end = words.split_off(words.len() - 8);
    for (m, v) in [(0x2044, 0x800), (0x204c, 10), (0x2144, 0x400)] {
        method(&mut words, m, v);
    }
    words.extend(end);
    let r = submit(&mut d, c, b, &mut data, &words);
    assert!(r.fault.is_none(), "{:?}", r.fault);
    assert_eq!(r.progress.draws_submitted, 1);
    d.publish(c, &mut mem(&mut data)).unwrap();
    check_conditional_geometry(&data, 0.5);
    d.retire(r.token).unwrap();
    d.write_backing(b, 0x3000, &(-0.4f32).to_le_bytes(), &mut mem(&mut data))
        .unwrap();
    let bg: Vec<_> = (0..W * H).flat_map(|_| BG).collect();
    d.write_backing(b, RT as u64, &bg, &mut mem(&mut data))
        .unwrap();
    let before = d.graphics_counters();
    let r = submit(&mut d, c, b, &mut data, &draw());
    assert!(r.fault.is_none());
    assert_eq!(r.progress.draws_submitted, 1);
    let after = d.graphics_counters();
    assert_eq!(after.vertex_shader_decodes, before.vertex_shader_decodes);
    assert_eq!(after.pipeline_allocations, before.pipeline_allocations);
    assert_eq!(after.vertex_upload_bytes - before.vertex_upload_bytes, 72);
    d.publish(c, &mut mem(&mut data)).unwrap();
    check_conditional_geometry(&data, f64::from(-0.4f32));
    d.retire(r.token).unwrap();
    // Switch BF to mask output and adjust only the raw expected value. Equal
    // observable geometry establishes the result-mode linkage in the VS too.
    let op = u64::from_le_bytes(bin[0x78..0x80].try_into().unwrap());
    d.write_backing(
        b,
        0x4878,
        &(op & !(1 << 52)).to_le_bytes(),
        &mut mem(&mut data),
    )
    .unwrap();
    let expected = u64::from_le_bytes(bin[0x80..0x88].try_into().unwrap());
    let mask = (expected & !(0xffffffffu64 << 20)) | (u64::from(u32::MAX) << 20);
    d.write_backing(b, 0x4880, &mask.to_le_bytes(), &mut mem(&mut data))
        .unwrap();
    d.write_backing(b, 0x3000, &(-0.8f32).to_le_bytes(), &mut mem(&mut data))
        .unwrap();
    d.write_backing(b, RT as u64, &bg, &mut mem(&mut data))
        .unwrap();
    let before = d.graphics_counters();
    let r = submit(&mut d, c, b, &mut data, &draw());
    assert!(r.fault.is_none(), "{:?}", r.fault);
    assert_eq!(r.progress.draws_submitted, 1);
    let changed = d.graphics_counters();
    assert_eq!(
        changed.vertex_shader_decodes,
        before.vertex_shader_decodes + 1
    );
    assert_eq!(changed.shader_decodes, before.shader_decodes);
    assert_eq!(
        changed.pipeline_allocations,
        before.pipeline_allocations + 1
    );
    d.publish(c, &mut mem(&mut data)).unwrap();
    check_conditional_geometry(&data, 0.5);
    eprintln!("VS FSET modes/mutation {:?}", d.graphics_counters());
    d.close(c, &mut mem(&mut data)).unwrap();
}

fn scaled_add_reference(left: u32, right: u32, shift: u8) -> u32 {
    assert!(shift < 32);
    // Independent oracle: multiply in u64, add, then retain the low 32 bits.
    // Largest intermediate is (2^32-1)*2^31+(2^32-1), below 2^64.
    (u64::from(left) * (1u64 << shift) + u64::from(right)) as u32
}
fn scaled_add_draw(
    d: &mut GpuDriver,
    c: ChannelId,
    b: BackingId,
    data: &mut [u8],
    words: &[u32],
    payload: [u32; 3],
    matches: bool,
) {
    let [left, right, expected] = payload;
    let mut bytes = [0u8; 12];
    bytes[..4].copy_from_slice(&left.to_le_bytes());
    bytes[4..8].copy_from_slice(&right.to_le_bytes());
    bytes[8..].copy_from_slice(&expected.to_le_bytes());
    d.write_backing(b, 0x50, &bytes, &mut mem(data)).unwrap();
    let r = submit(d, c, b, data, words);
    assert!(r.fault.is_none(), "{:?}", r.fault);
    assert_eq!(r.progress.draws_submitted, 1);
    d.publish(c, &mut mem(data)).unwrap();
    check(
        data,
        if matches {
            [64, 128, 191, 255]
        } else {
            [0, 128, 191, 255]
        },
        false,
    );
    d.retire(r.token).unwrap();
}
struct ScaledAddFailWrite<'a> {
    bytes: &'a mut [u8],
    calls: usize,
}
impl GuestMemory for ScaledAddFailWrite<'_> {
    fn read(&self, address: GuestAddress, length: usize) -> wl_gpu::Result<&[u8]> {
        let start = usize::try_from(address.0).map_err(|_| wl_gpu::Error("offset".into()))?;
        let end = start
            .checked_add(length)
            .ok_or_else(|| wl_gpu::Error("overflow".into()))?;
        self.bytes
            .get(start..end)
            .ok_or_else(|| wl_gpu::Error("unmapped".into()))
    }
}
impl GuestMemoryWrite for ScaledAddFailWrite<'_> {
    fn validate_write(&self, address: GuestAddress, length: usize) -> wl_gpu::Result<()> {
        self.read(address, length).map(|_| ())
    }
    fn write(&mut self, _: GuestAddress, _: &[u8]) -> wl_gpu::Result<()> {
        self.calls += 1;
        Err(wl_gpu::Error(
            "injected scaled-add publication failure".into(),
        ))
    }
}
#[test]
#[ignore = "requires actual GPU; all32 ISCADD shifts, wrapping arithmetic, aliases and retry-safe deferred output"]
fn driver_scaled_add_all_shifts_aliases_and_fault_publication() {
    let (mut data, mut words, _) = fixture();
    let (mut d, c, _, b, _) = driver();
    let bin = include_bytes!("fixtures/maxwell/scaled-add.bin");
    data[0x4600..0x4600 + bin.len()].copy_from_slice(bin);
    let end = words.split_off(words.len() - 8);
    for (m, v) in [
        (0x2144, 0x600),
        (0x214c, 8),
        (0x2380, 28),
        (0x2384, 0),
        (0x2388, 0x40040),
        (0x2490, 1),
    ] {
        method(&mut words, m, v);
    }
    words.extend(end);
    let original = u64::from_le_bytes(bin[0x78..0x80].try_into().unwrap());
    let compare = u64::from_le_bytes(bin[0x80..0x88].try_into().unwrap());
    assert_eq!(original, 0x5c18018000570404);
    assert_eq!(compare, 0x5b64038000770407);
    assert_eq!(
        u64::from_le_bytes(bin[0x88..0x90].try_into().unwrap()),
        0x5c180f800ff7ff04
    );
    let draw_words = draw();
    let mut pairs = vec![
        (0, 0),
        (0, u32::MAX),
        (u32::MAX, 0),
        (u32::MAX, 1),
        (u32::MAX, u32::MAX),
        (1, u32::MAX),
        (0x80000000, 0),
        (0x80000000, 1),
        (0x7fffffff, 1),
        (0x7fffffff, 0x80000000),
        (0x80000001, 0x7fffffff),
        (0x55555555, 0xaaaaaaaa),
        (0xaaaaaaaa, 0x55555555),
        (0x12345678, 0x89abcdef),
        (1, 1),
        (0x10000, 0xffff0000),
    ];
    let mut seed = 0x49534341u32;
    for _ in 0..32 {
        seed ^= seed << 13;
        seed ^= seed >> 17;
        seed ^= seed << 5;
        let left = seed;
        seed ^= seed << 13;
        seed ^= seed >> 17;
        seed ^= seed << 5;
        pairs.push((left, seed));
    }
    let mut first = true;
    for shift in 0..32u8 {
        let op = (original & !(31u64 << 39)) | (u64::from(shift) << 39);
        d.write_backing(b, 0x4678, &op.to_le_bytes(), &mut mem(&mut data))
            .unwrap();
        let mut identity = None;
        for &(left, right) in &pairs {
            scaled_add_draw(
                &mut d,
                c,
                b,
                &mut data,
                if first { &words } else { &draw_words },
                [left, right, scaled_add_reference(left, right, shift)],
                true,
            );
            first = false;
            let n = d.graphics_counters();
            let current = [
                n.shader_decodes,
                n.shader_module_allocations,
                n.pipeline_allocations,
            ];
            if let Some(previous) = identity {
                assert_eq!(current, previous);
            }
            identity = Some(current);
        }
    }
    // Destination aliases the addend. Patch the guest comparison input too;
    // source A remains R4 and the independently assembled auxiliary ISCADD
    // zeroes R4 after ISETP, proving P0 is preserved before SEL.
    let compare_right = (compare & !(255u64 << 8)) | (5u64 << 8);
    d.write_backing(b, 0x4680, &compare_right.to_le_bytes(), &mut mem(&mut data))
        .unwrap();
    for shift in [0, 3, 31u8] {
        let op = (original & !(255u64 | (31u64 << 39))) | 5 | (u64::from(shift) << 39);
        d.write_backing(b, 0x4678, &op.to_le_bytes(), &mut mem(&mut data))
            .unwrap();
        for (left, right) in [(0x12345678, 0x89abcdef), (u32::MAX, 1), (1, u32::MAX)] {
            scaled_add_draw(
                &mut d,
                c,
                b,
                &mut data,
                &draw_words,
                [left, right, scaled_add_reference(left, right, shift)],
                true,
            );
        }
    }
    d.write_backing(b, 0x4680, &compare.to_le_bytes(), &mut mem(&mut data))
        .unwrap();
    // Poison both guest source words: replacing either/both source fields with
    // RZ must read architectural zero, never an accidentally adjacent GPR.
    for shift in [0, 3, 31u8] {
        for (left_reg, right_reg) in [(255u8, 5u8), (4, 255), (255, 255)] {
            let op = (original & !((255u64 << 8) | (255u64 << 20) | (31u64 << 39)))
                | (u64::from(left_reg) << 8)
                | (u64::from(right_reg) << 20)
                | (u64::from(shift) << 39);
            d.write_backing(b, 0x4678, &op.to_le_bytes(), &mut mem(&mut data))
                .unwrap();
            let left = 0x89abcdef;
            let right = 0x76543210;
            let expected = scaled_add_reference(
                if left_reg == 255 { 0 } else { left },
                if right_reg == 255 { 0 } else { right },
                shift,
            );
            scaled_add_draw(
                &mut d,
                c,
                b,
                &mut data,
                &draw_words,
                [left, right, expected],
                true,
            );
        }
    }
    d.write_backing(b, 0x4678, &original.to_le_bytes(), &mut mem(&mut data))
        .unwrap();
    let expected = scaled_add_reference(0x12345678, 0x89abcdef, 3);
    assert_eq!(expected, 0x1b4e81af);
    scaled_add_draw(
        &mut d,
        c,
        b,
        &mut data,
        &draw_words,
        [0x12345678, 0x89abcdef, expected ^ 1],
        false,
    );
    let before_correct = d.graphics_counters();
    scaled_add_draw(
        &mut d,
        c,
        b,
        &mut data,
        &draw_words,
        [0x12345678, 0x89abcdef, expected],
        true,
    );
    assert_eq!(
        d.graphics_counters().shader_decodes,
        before_correct.shader_decodes
    );
    // Publish a visibly different image before pending warm work. Losing
    // pending output or treating failed publication as success must therefore
    // leave red0 and fail the final expected red64 verification.
    scaled_add_draw(
        &mut d,
        c,
        b,
        &mut data,
        &draw_words,
        [0x12345678, 0x89abcdef, expected ^ 1],
        false,
    );
    let mut payload = [0u8; 12];
    payload[..4].copy_from_slice(&0x12345678u32.to_le_bytes());
    payload[4..8].copy_from_slice(&0x89abcdefu32.to_le_bytes());
    payload[8..].copy_from_slice(&expected.to_le_bytes());
    d.write_backing(b, 0x50, &payload, &mut mem(&mut data))
        .unwrap();
    // Prime the correct GPU-only image and its resource uploads BEFORE the
    // warm baseline; guest RAM must retain the previously published red0.
    let priming = submit(&mut d, c, b, &mut data, &draw_words);
    assert!(priming.fault.is_none(), "{:?}", priming.fault);
    assert_eq!(priming.progress.draws_submitted, 1);
    assert!(!d.query(priming.token).unwrap().guest_visible);
    check(&data, [0, 128, 191, 255], false);
    let before = d.graphics_counters();
    let start = std::time::Instant::now();
    let mut last = None;
    for _ in 0..32 {
        let r = submit_existing(&mut d, c, &mut data, 8);
        assert!(r.fault.is_none(), "{:?}", r.fault);
        assert_eq!(r.progress.draws_submitted, 1);
        last = Some(r.token);
    }
    let elapsed = start.elapsed();
    let after = d.graphics_counters();
    assert_eq!(after.draw_commands - before.draw_commands, 32);
    assert_eq!(after.submissions - before.submissions, 32);
    assert_eq!(
        [
            after.shader_binary_bytes,
            after.vertex_shader_bytes,
            after.shader_decodes,
            after.vertex_shader_decodes,
            after.descriptor_reads,
            after.vertex_guest_bytes,
            after.index_guest_bytes,
            after.constant_guest_bytes,
            after.guest_texture_bytes,
            after.render_target_guest_bytes,
            after.upload_bytes,
            after.upload_calls,
            after.shader_module_allocations,
            after.pipeline_allocations,
            after.render_target_allocations,
            after.readback_operations,
            after.synchronization_points
        ],
        [
            before.shader_binary_bytes,
            before.vertex_shader_bytes,
            before.shader_decodes,
            before.vertex_shader_decodes,
            before.descriptor_reads,
            before.vertex_guest_bytes,
            before.index_guest_bytes,
            before.constant_guest_bytes,
            before.guest_texture_bytes,
            before.render_target_guest_bytes,
            before.upload_bytes,
            before.upload_calls,
            before.shader_module_allocations,
            before.pipeline_allocations,
            before.render_target_allocations,
            before.readback_operations,
            before.synchronization_points
        ]
    );
    let last = last.unwrap();
    assert!(!d.query(last).unwrap().guest_visible);
    assert!(!d.query(priming.token).unwrap().guest_visible);
    check(&data, [0, 128, 191, 255], false);
    // Unsupported CC must not append a draw or replay any accepted draw.
    d.write_backing(
        b,
        0x4678,
        &(original | (1 << 47)).to_le_bytes(),
        &mut mem(&mut data),
    )
    .unwrap();
    let failed = submit(&mut d, c, b, &mut data, &draw_words);
    assert!(failed.fault.is_some());
    assert_eq!(failed.progress.draws_submitted, 0);
    assert_eq!(d.graphics_counters().draw_commands, after.draw_commands);
    assert_eq!(d.graphics_counters().submissions, after.submissions);
    let old_guest = data[RT..RT + SIZE].to_vec();
    let guest_bytes = d.graphics_counters().guest_framebuffer_write_bytes;
    {
        let mut rejecting = ScaledAddFailWrite {
            bytes: &mut data,
            calls: 0,
        };
        assert!(d.publish(c, &mut rejecting).is_err());
        assert_eq!(rejecting.calls, 1);
    }
    assert_eq!(&data[RT..RT + SIZE], old_guest.as_slice());
    assert_eq!(
        d.graphics_counters().guest_framebuffer_write_bytes,
        guest_bytes
    );
    assert_eq!(d.graphics_counters().draw_commands, after.draw_commands);
    assert!(!d.query(last).unwrap().guest_visible);
    assert!(!d.query(priming.token).unwrap().guest_visible);
    check(&data, [0, 128, 191, 255], false);
    // Retry visibility, never command submission. Readback may repeat on a
    // failed atomic write; no claim of readback caching is made here.
    d.publish(c, &mut mem(&mut data)).unwrap();
    check(&data, [64, 128, 191, 255], false);
    assert!(d.query(last).unwrap().guest_visible);
    assert!(d.query(priming.token).unwrap().guest_visible);
    assert_eq!(d.graphics_counters().draw_commands, after.draw_commands);
    eprintln!(
        "ISCADD {} pairs x32 shifts, warm32 CPU={elapsed:?} before={before:?} after={after:?}",
        pairs.len()
    );
    d.close(c, &mut mem(&mut data)).unwrap();
}
#[test]
#[ignore = "requires actual GPU; scaled-add of raw guest vertex X controls finite geometry and code changes"]
fn driver_vertex_scaled_add_controls_geometry_and_code_mutation() {
    let (mut data, mut words, _) = fixture();
    let (mut d, c, _, b, _) = driver();
    let bin = include_bytes!("fixtures/maxwell/vertex-scaled-add.bin");
    data[0x4800..0x4800 + bin.len()].copy_from_slice(bin);
    let end = words.split_off(words.len() - 8);
    for (m, v) in [(0x2044, 0x800), (0x204c, 9), (0x2144, 0x400)] {
        method(&mut words, m, v);
    }
    words.extend(end);
    assert_eq!(scaled_add_reference(POS[0][0].to_bits(), 0, 1), 0x7e99999a);
    assert_eq!(scaled_add_reference(POS[1][0].to_bits(), 0, 1), 0x7e333334);
    assert_eq!(scaled_add_reference(POS[2][0].to_bits(), 0, 1), 0x7c99999a);
    let r = submit(&mut d, c, b, &mut data, &words);
    assert!(r.fault.is_none(), "{:?}", r.fault);
    assert_eq!(r.progress.draws_submitted, 1);
    d.publish(c, &mut mem(&mut data)).unwrap();
    check_conditional_geometry(&data, 0.5);
    d.retire(r.token).unwrap();
    assert_eq!(scaled_add_reference((-0.6f32).to_bits(), 0, 1), 0x7e333334);
    d.write_backing(b, 0x3000, &(-0.6f32).to_le_bytes(), &mut mem(&mut data))
        .unwrap();
    let background: Vec<_> = (0..W * H).flat_map(|_| BG).collect();
    d.write_backing(b, RT as u64, &background, &mut mem(&mut data))
        .unwrap();
    let before = d.graphics_counters();
    let r = submit(&mut d, c, b, &mut data, &draw());
    assert!(r.fault.is_none(), "{:?}", r.fault);
    assert_eq!(r.progress.draws_submitted, 1);
    let after = d.graphics_counters();
    assert_eq!(after.vertex_shader_decodes, before.vertex_shader_decodes);
    assert_eq!(after.pipeline_allocations, before.pipeline_allocations);
    assert_eq!(after.vertex_upload_bytes - before.vertex_upload_bytes, 72);
    d.publish(c, &mut mem(&mut data)).unwrap();
    check_conditional_geometry(&data, f64::from(-0.6f32));
    d.retire(r.token).unwrap();
    // Change the real vertex program to shift0 and compare against first
    // vertex's unchanged raw -.6 bits. It now matches and moves to finite .5.
    // Both opcode and expected MOV32I are guest code writes, not host geometry.
    let op = u64::from_le_bytes(bin[0x78..0x80].try_into().unwrap()) & !(31u64 << 39);
    let reference = 0x010000000007f007u64 | (u64::from((-0.6f32).to_bits()) << 20);
    d.write_backing(b, 0x4878, &op.to_le_bytes(), &mut mem(&mut data))
        .unwrap();
    d.write_backing(b, 0x4868, &reference.to_le_bytes(), &mut mem(&mut data))
        .unwrap();
    d.write_backing(b, RT as u64, &background, &mut mem(&mut data))
        .unwrap();
    let before = d.graphics_counters();
    let r = submit(&mut d, c, b, &mut data, &draw());
    assert!(r.fault.is_none(), "{:?}", r.fault);
    assert_eq!(r.progress.draws_submitted, 1);
    let after = d.graphics_counters();
    assert_eq!(
        after.vertex_shader_decodes,
        before.vertex_shader_decodes + 1
    );
    assert_eq!(after.shader_decodes, before.shader_decodes);
    assert_eq!(after.vertex_upload_bytes, before.vertex_upload_bytes);
    assert!(after.pipeline_allocations > before.pipeline_allocations);
    d.publish(c, &mut mem(&mut data)).unwrap();
    check_conditional_geometry(&data, 0.5);
    d.retire(r.token).unwrap();
    eprintln!("VS ISCADD geometry {:?}", d.graphics_counters());
    d.close(c, &mut mem(&mut data)).unwrap();
}

fn widen_reference(word: u32, bits: u8, signed: bool) -> u32 {
    // Independent byte interpretation, not the production mask/shift lowering.
    let b = word.to_le_bytes();
    match (bits, signed) {
        (8, false) => u32::from(b[0]),
        (8, true) => i32::from(i8::from_le_bytes([b[0]])) as u32,
        (16, false) => u32::from(u16::from_le_bytes([b[0], b[1]])),
        (16, true) => i32::from(i16::from_le_bytes([b[0], b[1]])) as u32,
        _ => panic!("unsupported oracle mode"),
    }
}
fn widen_draw(
    d: &mut GpuDriver,
    c: ChannelId,
    b: BackingId,
    data: &mut [u8],
    words: &[u32],
    payload: [u32; 2],
    matches: bool,
) {
    let mut bytes = [0u8; 8];
    bytes[..4].copy_from_slice(&payload[0].to_le_bytes());
    bytes[4..].copy_from_slice(&payload[1].to_le_bytes());
    d.write_backing(b, 0x50, &bytes, &mut mem(data)).unwrap();
    let r = submit(d, c, b, data, words);
    assert!(r.fault.is_none(), "{:?}", r.fault);
    assert_eq!(r.progress.draws_submitted, 1);
    d.publish(c, &mut mem(data)).unwrap();
    check(
        data,
        if matches {
            [64, 128, 191, 255]
        } else {
            [0, 128, 191, 255]
        },
        false,
    );
    d.retire(r.token).unwrap();
}
#[test]
#[ignore = "requires actual GPU; four scoreboarded I2I widening forms, raw sign/zero extension and deferred visibility"]
fn driver_integer_widening_types_data_and_wait_coherence() {
    let (mut data, mut words, _) = fixture();
    let (mut d, c, _, b, _) = driver();
    let variants: [(u8, bool, &[u8]); 4] = [
        (
            8,
            false,
            include_bytes!("fixtures/maxwell/integer-widen-u8.bin"),
        ),
        (
            16,
            false,
            include_bytes!("fixtures/maxwell/integer-widen-u16.bin"),
        ),
        (
            8,
            true,
            include_bytes!("fixtures/maxwell/integer-widen-s8.bin"),
        ),
        (
            16,
            true,
            include_bytes!("fixtures/maxwell/integer-widen-s16.bin"),
        ),
    ];
    let bin = variants[0].2;
    data[0x4600..0x4600 + bin.len()].copy_from_slice(bin);
    let end = words.split_off(words.len() - 8);
    for (m, v) in [
        (0x2144, 0x600),
        (0x214c, 7),
        (0x2380, 24),
        (0x2384, 0),
        (0x2388, 0x40040),
        (0x2490, 1),
    ] {
        method(&mut words, m, v);
    }
    words.extend(end);
    let draw_words = draw();
    let mut values = vec![
        0,
        u32::MAX,
        0x80000000,
        0x7fffffff,
        0x7f,
        0x80,
        0xff,
        0x100,
        0x7fff,
        0x8000,
        0xffff,
        0x10000,
        0x12345678,
        0x89abcdef,
        0xffffff7f,
        0xffffff80,
        0xffff7fff,
        0xffff8000,
        0x12345680,
        0xdead8000,
        0x000080ff,
        0x80000080,
    ];
    let mut seed = 0x49324931u32;
    for _ in 0..32 {
        seed ^= seed << 13;
        seed ^= seed >> 17;
        seed ^= seed << 5;
        values.push(seed);
    }
    let mut first = true;
    for (bits, signed, fixture) in variants {
        let op = u64::from_le_bytes(fixture[0x68..0x70].try_into().unwrap());
        d.write_backing(b, 0x4668, &op.to_le_bytes(), &mut mem(&mut data))
            .unwrap();
        let mut identity = None;
        for &raw in &values {
            widen_draw(
                &mut d,
                c,
                b,
                &mut data,
                if first { &words } else { &draw_words },
                [raw, widen_reference(raw, bits, signed)],
                true,
            );
            first = false;
            let n = d.graphics_counters();
            let current = [
                n.shader_decodes,
                n.shader_module_allocations,
                n.pipeline_allocations,
            ];
            if let Some(old) = identity {
                assert_eq!(current, old);
            }
            identity = Some(current);
        }
    }
    // Both raw source storage and signedness are poisoned independently of RZ.
    for (_, _, fixture) in variants {
        let op = (u64::from_le_bytes(fixture[0x68..0x70].try_into().unwrap()) & !(255u64 << 20))
            | (255u64 << 20);
        d.write_backing(b, 0x4668, &op.to_le_bytes(), &mut mem(&mut data))
            .unwrap();
        widen_draw(&mut d, c, b, &mut data, &draw_words, [0x89abcdef, 0], true);
    }
    let signed = variants[3].2;
    let op = u64::from_le_bytes(signed[0x68..0x70].try_into().unwrap());
    d.write_backing(b, 0x4668, &op.to_le_bytes(), &mut mem(&mut data))
        .unwrap();
    let expected = widen_reference(0x89abcdef, 16, true);
    assert_eq!(expected, 0xffffcdef);
    // Wrong expected bits must visibly fail; they also establish different RAM
    // contents for the subsequent pending-output publication proof.
    widen_draw(
        &mut d,
        c,
        b,
        &mut data,
        &draw_words,
        [0x89abcdef, expected ^ 1],
        false,
    );
    let mut bytes = [0u8; 8];
    bytes[..4].copy_from_slice(&0x89abcdefu32.to_le_bytes());
    bytes[4..].copy_from_slice(&expected.to_le_bytes());
    d.write_backing(b, 0x50, &bytes, &mut mem(&mut data))
        .unwrap();
    let priming = submit(&mut d, c, b, &mut data, &draw_words);
    assert!(priming.fault.is_none(), "{:?}", priming.fault);
    assert_eq!(priming.progress.draws_submitted, 1);
    assert!(!d.query(priming.token).unwrap().guest_visible);
    check(&data, [0, 128, 191, 255], false);
    let unchanged = |n: wl_gpu::Counters| {
        [
            n.shader_binary_bytes,
            n.vertex_shader_bytes,
            n.shader_decodes,
            n.vertex_shader_decodes,
            n.descriptor_reads,
            n.vertex_guest_bytes,
            n.index_guest_bytes,
            n.constant_guest_bytes,
            n.guest_texture_bytes,
            n.render_target_guest_bytes,
            n.upload_bytes,
            n.upload_calls,
            n.shader_module_allocations,
            n.pipeline_allocations,
            n.render_target_allocations,
            n.readback_operations,
            n.synchronization_points,
        ]
    };
    let before = d.graphics_counters();
    let start = std::time::Instant::now();
    let mut last = None;
    for _ in 0..32 {
        let r = submit_existing(&mut d, c, &mut data, 8);
        assert!(r.fault.is_none(), "{:?}", r.fault);
        assert_eq!(r.progress.draws_submitted, 1);
        last = Some(r.token);
    }
    let elapsed = start.elapsed();
    let after = d.graphics_counters();
    assert_eq!(after.draw_commands - before.draw_commands, 32);
    assert_eq!(after.submissions - before.submissions, 32);
    assert_eq!(unchanged(after), unchanged(before));
    check(&data, [0, 128, 191, 255], false);
    let last = last.unwrap();
    assert!(!d.query(last).unwrap().guest_visible);
    // Remove the first I2I consumer wait, leaving the auxiliary's independent
    // WB0/wait1 intact. This must fault before drawing and retain red64 GPU output.
    let control = u64::from_le_bytes(signed[0x70..0x78].try_into().unwrap());
    d.write_backing(
        b,
        0x4670,
        &(control & !(1 << 11)).to_le_bytes(),
        &mut mem(&mut data),
    )
    .unwrap();
    let failed = submit(&mut d, c, b, &mut data, &draw_words);
    assert!(failed.fault.is_some());
    assert_eq!(failed.progress.draws_submitted, 0);
    assert_eq!(d.graphics_counters().draw_commands, after.draw_commands);
    assert_eq!(d.graphics_counters().submissions, after.submissions);
    d.publish(c, &mut mem(&mut data)).unwrap();
    check(&data, [64, 128, 191, 255], false);
    assert!(d.query(last).unwrap().guest_visible);
    assert!(d.query(priming.token).unwrap().guest_visible);
    assert_eq!(d.graphics_counters().draw_commands, after.draw_commands);
    eprintln!(
        "I2I {} raw words x4 types, warm32 CPU={elapsed:?} before={before:?} after={after:?}",
        values.len()
    );
    d.close(c, &mut mem(&mut data)).unwrap();
}
#[test]
#[ignore = "requires actual GPU; signed low-halfword widening predicates finite guest geometry"]
fn driver_vertex_integer_widening_controls_geometry() {
    let (mut data, mut words, _) = fixture();
    let (mut d, c, _, b, _) = driver();
    let bin = include_bytes!("fixtures/maxwell/vertex-integer-widen.bin");
    data[0x4800..0x4800 + bin.len()].copy_from_slice(bin);
    let end = words.split_off(words.len() - 8);
    for (m, v) in [(0x2044, 0x800), (0x204c, 9), (0x2144, 0x400)] {
        method(&mut words, m, v);
    }
    words.extend(end);
    for (x, expected) in [
        (-0.8f32, 0xffffcccd),
        (0.6f32, 0xffff999a),
        (-0.2f32, 0xffffcccd),
        (0.57f32, 0xffffeb85),
    ] {
        assert_eq!(widen_reference(x.to_bits(), 16, true), expected);
    }
    let r = submit(&mut d, c, b, &mut data, &words);
    assert!(r.fault.is_none(), "{:?}", r.fault);
    assert_eq!(r.progress.draws_submitted, 1);
    d.publish(c, &mut mem(&mut data)).unwrap();
    check_positions(
        &data,
        [255, 0, 0, 255],
        [
            POS[0].map(f64::from),
            [0.5, f64::from(POS[1][1])],
            POS[2].map(f64::from),
        ],
    );
    d.retire(r.token).unwrap();
    // The unique low EB85 field is negative under S16 and positive under U16.
    // These literal checks make both later code changes necessary for restoration.
    assert_eq!(0.57f32.to_bits(), 0x3f11eb85);
    assert_eq!(widen_reference(0x3f11eb85, 16, true), 0xffffeb85);
    assert_eq!(widen_reference(0x3f11eb85, 16, false), 0x0000eb85);
    d.write_backing(b, 0x3018, &0.57f32.to_le_bytes(), &mut mem(&mut data))
        .unwrap();
    let background: Vec<_> = (0..W * H).flat_map(|_| BG).collect();
    d.write_backing(b, RT as u64, &background, &mut mem(&mut data))
        .unwrap();
    let before = d.graphics_counters();
    let r = submit(&mut d, c, b, &mut data, &draw());
    assert!(r.fault.is_none(), "{:?}", r.fault);
    assert_eq!(r.progress.draws_submitted, 1);
    let after = d.graphics_counters();
    assert_eq!(after.vertex_shader_decodes, before.vertex_shader_decodes);
    assert_eq!(after.pipeline_allocations, before.pipeline_allocations);
    assert_eq!(after.vertex_upload_bytes - before.vertex_upload_bytes, 72);
    d.publish(c, &mut mem(&mut data)).unwrap();
    check_positions(
        &data,
        [255, 0, 0, 255],
        [
            POS[0].map(f64::from),
            [f64::from(0.57f32), f64::from(POS[1][1])],
            POS[2].map(f64::from),
        ],
    );
    d.retire(r.token).unwrap();
    // U16 plus expected0000EB85 selects only the second vertex again. Changing
    // the expected literal alone leaves FFFFEB85 unmatched; changing the type
    // alone leaves old FFFF999A unmatched. No vertex bytes change in this step.
    let unsigned = u64::from_le_bytes(bin[0x78..0x80].try_into().unwrap()) & !0x3000;
    assert_eq!(unsigned, 0x5ce0000000070606);
    let expected = 0x010000000007f007u64 | (0xeb85u64 << 20);
    d.write_backing(b, 0x4878, &unsigned.to_le_bytes(), &mut mem(&mut data))
        .unwrap();
    d.write_backing(b, 0x4868, &expected.to_le_bytes(), &mut mem(&mut data))
        .unwrap();
    d.write_backing(b, RT as u64, &background, &mut mem(&mut data))
        .unwrap();
    let before = d.graphics_counters();
    let r = submit(&mut d, c, b, &mut data, &draw());
    assert!(r.fault.is_none(), "{:?}", r.fault);
    assert_eq!(r.progress.draws_submitted, 1);
    let after = d.graphics_counters();
    assert_eq!(
        after.vertex_shader_decodes,
        before.vertex_shader_decodes + 1
    );
    assert_eq!(after.shader_decodes, before.shader_decodes);
    assert_eq!(after.vertex_upload_bytes, before.vertex_upload_bytes);
    assert!(after.pipeline_allocations > before.pipeline_allocations);
    d.publish(c, &mut mem(&mut data)).unwrap();
    check_positions(
        &data,
        [255, 0, 0, 255],
        [
            POS[0].map(f64::from),
            [0.5, f64::from(POS[1][1])],
            POS[2].map(f64::from),
        ],
    );
    d.retire(r.token).unwrap();
    eprintln!("VS I2I geometry {:?}", d.graphics_counters());
    d.close(c, &mut mem(&mut data)).unwrap();
}

fn integer_set_reference(a: u32, b: u32, relation: u8, signed: bool, bf: bool) -> u32 {
    use std::cmp::Ordering::{Equal, Greater, Less};
    let left = if signed {
        i64::from(a as i32)
    } else {
        i64::from(a)
    };
    let right = if signed {
        i64::from(b as i32)
    } else {
        i64::from(b)
    };
    let order = left.cmp(&right);
    let result = match relation {
        1 => order == Less,
        2 => order == Equal,
        3 => order != Greater,
        4 => order == Greater,
        5 => order != Equal,
        6 => order != Less,
        _ => panic!("unsupported oracle relation"),
    };
    if !result {
        0
    } else if bf {
        0x3f800000
    } else {
        u32::MAX
    }
}
fn integer_set_draw(
    d: &mut GpuDriver,
    c: ChannelId,
    b: BackingId,
    data: &mut [u8],
    words: &[u32],
    payload: [u32; 3],
    matches: bool,
) {
    let mut bytes = [0u8; 12];
    for (dst, raw) in bytes.chunks_exact_mut(4).zip(payload) {
        dst.copy_from_slice(&raw.to_le_bytes());
    }
    d.write_backing(b, 0x50, &bytes, &mut mem(data)).unwrap();
    let r = submit(d, c, b, data, words);
    assert!(r.fault.is_none(), "{:?}", r.fault);
    assert_eq!(r.progress.draws_submitted, 1);
    d.publish(c, &mut mem(data)).unwrap();
    check(
        data,
        if matches {
            [64, 128, 191, 255]
        } else {
            [0, 128, 191, 255]
        },
        false,
    );
    d.retire(r.token).unwrap();
}
#[test]
#[ignore = "requires actual GPU; ISET24 signed/unsigned mask/BF modes, aliases and fixed6 fault preservation"]
fn driver_integer_set_modes_aliases_and_fixed_latency() {
    let (mut data, mut words, _) = fixture();
    let (mut d, c, _, b, _) = driver();
    let bin = include_bytes!("fixtures/maxwell/integer-set-u-mask.bin");
    data[0x4600..0x4600 + bin.len()].copy_from_slice(bin);
    let end = words.split_off(words.len() - 8);
    for (m, v) in [
        (0x2144, 0x600),
        (0x214c, 8),
        (0x2380, 28),
        (0x2384, 0),
        (0x2388, 0x40040),
        (0x2490, 1),
    ] {
        method(&mut words, m, v);
    }
    words.extend(end);
    let draw_words = draw();
    let original = u64::from_le_bytes(bin[0x78..0x80].try_into().unwrap());
    let compare = u64::from_le_bytes(bin[0x80..0x88].try_into().unwrap());
    assert_eq!(original, 0x5b52038000570404);
    assert_eq!(compare, 0x5b64038000770407);
    let mut pairs = vec![
        (0, 0),
        (0, 1),
        (1, 0),
        (u32::MAX, 0),
        (0, u32::MAX),
        (u32::MAX, u32::MAX),
        (0x80000000, 0x7fffffff),
        (0x7fffffff, 0x80000000),
        (0x80000000, 0x80000000),
        (0x80000001, 0x80000000),
        (0x7fffffff, 0x7ffffffe),
        (0x12345678, 0x12345678),
        (0xffff0000, 0x0000ffff),
        (0x55555555, 0xaaaaaaaa),
        (0xaaaaaaaa, 0x55555555),
        (0xdeadbeef, 0xdeadbef0),
    ];
    let mut seed = 0x49534554u32;
    for _ in 0..32 {
        seed ^= seed << 13;
        seed ^= seed >> 17;
        seed ^= seed << 5;
        let left = seed;
        seed ^= seed << 13;
        seed ^= seed >> 17;
        seed ^= seed << 5;
        pairs.push((left, seed));
    }
    let mut first = true;
    for relation in 1..=6u8 {
        for signed in [false, true] {
            for bf in [false, true] {
                let op = (original & !((15u64 << 48) | (1u64 << 44)))
                    | (u64::from(relation) << 49)
                    | (u64::from(signed) << 48)
                    | (u64::from(bf) << 44);
                d.write_backing(b, 0x4678, &op.to_le_bytes(), &mut mem(&mut data))
                    .unwrap();
                let mut identity = None;
                for &(left, right) in &pairs {
                    integer_set_draw(
                        &mut d,
                        c,
                        b,
                        &mut data,
                        if first { &words } else { &draw_words },
                        [
                            left,
                            right,
                            integer_set_reference(left, right, relation, signed, bf),
                        ],
                        true,
                    );
                    first = false;
                    let n = d.graphics_counters();
                    let current = [
                        n.shader_decodes,
                        n.shader_module_allocations,
                        n.pipeline_allocations,
                    ];
                    if let Some(old) = identity {
                        assert_eq!(current, old);
                    }
                    identity = Some(current);
                }
            }
        }
    }
    // Right destination alias: ISETP reads the result from R5, while auxiliary
    // ISET zeroes R4 after P0 was produced and must leave that P0 untouched.
    let compare_right = (compare & !(255u64 << 8)) | (5u64 << 8);
    d.write_backing(b, 0x4680, &compare_right.to_le_bytes(), &mut mem(&mut data))
        .unwrap();
    for bf in [false, true] {
        let op = (original & !255u64) | 5 | (u64::from(bf) << 44);
        d.write_backing(b, 0x4678, &op.to_le_bytes(), &mut mem(&mut data))
            .unwrap();
        for (left, right) in [(0, 1), (u32::MAX, 0), (u32::MAX, u32::MAX)] {
            integer_set_draw(
                &mut d,
                c,
                b,
                &mut data,
                &draw_words,
                [
                    left,
                    right,
                    integer_set_reference(left, right, 1, false, bf),
                ],
                true,
            );
        }
    }
    d.write_backing(b, 0x4680, &compare.to_le_bytes(), &mut mem(&mut data))
        .unwrap();
    for relation in [2u8, 4] {
        for signed in [false, true] {
            for bf in [false, true] {
                for (left_reg, right_reg) in [(255u8, 5u8), (4, 255), (255, 255)] {
                    let op = (original
                        & !((255u64 << 8) | (255u64 << 20) | (15u64 << 48) | (1u64 << 44)))
                        | (u64::from(left_reg) << 8)
                        | (u64::from(right_reg) << 20)
                        | (u64::from(relation) << 49)
                        | (u64::from(signed) << 48)
                        | (u64::from(bf) << 44);
                    d.write_backing(b, 0x4678, &op.to_le_bytes(), &mut mem(&mut data))
                        .unwrap();
                    let left = 0x89abcdef;
                    let right = 0x76543210;
                    let expected = integer_set_reference(
                        if left_reg == 255 { 0 } else { left },
                        if right_reg == 255 { 0 } else { right },
                        relation,
                        signed,
                        bf,
                    );
                    integer_set_draw(
                        &mut d,
                        c,
                        b,
                        &mut data,
                        &draw_words,
                        [left, right, expected],
                        true,
                    );
                }
            }
        }
    }
    d.write_backing(b, 0x4678, &original.to_le_bytes(), &mut mem(&mut data))
        .unwrap();
    let expected = integer_set_reference(0x12345678, 0x89abcdef, 1, false, false);
    assert_eq!(expected, u32::MAX);
    integer_set_draw(
        &mut d,
        c,
        b,
        &mut data,
        &draw_words,
        [0x12345678, 0x89abcdef, expected ^ 1],
        false,
    );
    let mut payload = [0u8; 12];
    for (dst, raw) in payload
        .chunks_exact_mut(4)
        .zip([0x12345678u32, 0x89abcdef, expected])
    {
        dst.copy_from_slice(&raw.to_le_bytes());
    }
    d.write_backing(b, 0x50, &payload, &mut mem(&mut data))
        .unwrap();
    let priming = submit(&mut d, c, b, &mut data, &draw_words);
    assert!(priming.fault.is_none(), "{:?}", priming.fault);
    assert_eq!(priming.progress.draws_submitted, 1);
    assert!(!d.query(priming.token).unwrap().guest_visible);
    check(&data, [0, 128, 191, 255], false);
    let unchanged = |n: wl_gpu::Counters| {
        [
            n.shader_binary_bytes,
            n.vertex_shader_bytes,
            n.shader_decodes,
            n.vertex_shader_decodes,
            n.descriptor_reads,
            n.vertex_guest_bytes,
            n.index_guest_bytes,
            n.constant_guest_bytes,
            n.guest_texture_bytes,
            n.render_target_guest_bytes,
            n.upload_bytes,
            n.upload_calls,
            n.shader_module_allocations,
            n.pipeline_allocations,
            n.render_target_allocations,
            n.readback_operations,
            n.synchronization_points,
        ]
    };
    let before = d.graphics_counters();
    let start = std::time::Instant::now();
    let mut last = None;
    for _ in 0..32 {
        let r = submit_existing(&mut d, c, &mut data, 8);
        assert!(r.fault.is_none(), "{:?}", r.fault);
        assert_eq!(r.progress.draws_submitted, 1);
        last = Some(r.token);
    }
    let elapsed = start.elapsed();
    let after = d.graphics_counters();
    assert_eq!(after.draw_commands - before.draw_commands, 32);
    assert_eq!(after.submissions - before.submissions, 32);
    assert_eq!(unchanged(after), unchanged(before));
    check(&data, [0, 128, 191, 255], false);
    let last = last.unwrap();
    assert!(!d.query(last).unwrap().guest_visible);
    // ISET is fixed6, whereas the adjacent ISETP is fixed13. Change only the
    // ISET stall to13; the accepted GPU-only image must survive this fault.
    let control = u64::from_le_bytes(bin[0x70..0x78].try_into().unwrap());
    d.write_backing(
        b,
        0x4670,
        &((control & !31) | 13).to_le_bytes(),
        &mut mem(&mut data),
    )
    .unwrap();
    let failed = submit(&mut d, c, b, &mut data, &draw_words);
    assert!(failed.fault.is_some());
    assert_eq!(failed.progress.draws_submitted, 0);
    assert_eq!(d.graphics_counters().draw_commands, after.draw_commands);
    assert_eq!(d.graphics_counters().submissions, after.submissions);
    check(&data, [0, 128, 191, 255], false);
    d.publish(c, &mut mem(&mut data)).unwrap();
    check(&data, [64, 128, 191, 255], false);
    assert!(d.query(priming.token).unwrap().guest_visible);
    assert!(d.query(last).unwrap().guest_visible);
    assert_eq!(d.graphics_counters().draw_commands, after.draw_commands);
    eprintln!(
        "ISET {} raw pairs x24 modes, warm32 CPU={elapsed:?} before={before:?} after={after:?}",
        pairs.len()
    );
    d.close(c, &mut mem(&mut data)).unwrap();
}
#[test]
#[ignore = "requires actual GPU; integer comparison BF/mask code mutation and guest vertex geometry"]
fn driver_vertex_integer_set_results_control_geometry() {
    let (mut data, mut words, _) = fixture();
    let (mut d, c, _, b, _) = driver();
    let bin = include_bytes!("fixtures/maxwell/vertex-integer-set.bin");
    data[0x4800..0x4800 + bin.len()].copy_from_slice(bin);
    let end = words.split_off(words.len() - 8);
    for (m, v) in [(0x2044, 0x800), (0x204c, 9), (0x2144, 0x400)] {
        method(&mut words, m, v);
    }
    words.extend(end);
    for (vertex, expected) in [(0, 0), (1, 0x3f800000), (2, 0)] {
        assert_eq!(
            integer_set_reference(POS[vertex][0].to_bits(), 0, 4, true, true),
            expected
        );
    }
    let positions = [
        POS[0].map(f64::from),
        [0.5, f64::from(POS[1][1])],
        POS[2].map(f64::from),
    ];
    let r = submit(&mut d, c, b, &mut data, &words);
    assert!(r.fault.is_none(), "{:?}", r.fault);
    assert_eq!(r.progress.draws_submitted, 1);
    d.publish(c, &mut mem(&mut data)).unwrap();
    check_positions(&data, [255, 0, 0, 255], positions);
    d.retire(r.token).unwrap();
    // Both guest code edits are necessary: BF result3F800000 must become mask
    // FFFFFFFF, and ISETP must compare against the matching literal. With only
    // either edit the position substitution would fail for the second vertex.
    let mask = u64::from_le_bytes(bin[0x78..0x80].try_into().unwrap()) & !(1u64 << 44);
    assert_eq!(mask, 0x5b5903800ff70006);
    let literal = 0x010000000007f007u64 | (u64::from(u32::MAX) << 20);
    d.write_backing(b, 0x4878, &mask.to_le_bytes(), &mut mem(&mut data))
        .unwrap();
    d.write_backing(b, 0x4868, &literal.to_le_bytes(), &mut mem(&mut data))
        .unwrap();
    let background: Vec<_> = (0..W * H).flat_map(|_| BG).collect();
    d.write_backing(b, RT as u64, &background, &mut mem(&mut data))
        .unwrap();
    let before = d.graphics_counters();
    let r = submit(&mut d, c, b, &mut data, &draw());
    assert!(r.fault.is_none(), "{:?}", r.fault);
    assert_eq!(r.progress.draws_submitted, 1);
    let after = d.graphics_counters();
    assert_eq!(
        after.vertex_shader_decodes,
        before.vertex_shader_decodes + 1
    );
    assert_eq!(after.shader_decodes, before.shader_decodes);
    assert_eq!(after.vertex_upload_bytes, before.vertex_upload_bytes);
    assert!(after.pipeline_allocations > before.pipeline_allocations);
    d.publish(c, &mut mem(&mut data)).unwrap();
    check_positions(&data, [255, 0, 0, 255], positions);
    d.retire(r.token).unwrap();
    assert_eq!(
        integer_set_reference((-0.6f32).to_bits(), 0, 4, true, false),
        0
    );
    d.write_backing(b, 0x3018, &(-0.6f32).to_le_bytes(), &mut mem(&mut data))
        .unwrap();
    d.write_backing(b, RT as u64, &background, &mut mem(&mut data))
        .unwrap();
    let before = d.graphics_counters();
    let r = submit(&mut d, c, b, &mut data, &draw());
    assert!(r.fault.is_none(), "{:?}", r.fault);
    assert_eq!(r.progress.draws_submitted, 1);
    let after = d.graphics_counters();
    assert_eq!(after.vertex_shader_decodes, before.vertex_shader_decodes);
    assert_eq!(after.pipeline_allocations, before.pipeline_allocations);
    assert_eq!(after.vertex_upload_bytes - before.vertex_upload_bytes, 72);
    d.publish(c, &mut mem(&mut data)).unwrap();
    check_positions(
        &data,
        [255, 0, 0, 255],
        [
            POS[0].map(f64::from),
            [f64::from(-0.6f32), f64::from(POS[1][1])],
            POS[2].map(f64::from),
        ],
    );
    d.retire(r.token).unwrap();
    eprintln!("VS ISET geometry {:?}", d.graphics_counters());
    d.close(c, &mut mem(&mut data)).unwrap();
}

fn byte_permute_reference(a: u32, b: u32, control: u32) -> u32 {
    let a = a.to_le_bytes();
    let b = b.to_le_bytes();
    let c = control.to_le_bytes();
    let bytes = [a[0], a[1], a[2], a[3], b[0], b[1], b[2], b[3]];
    let mut out = [0u8; 4];
    for (lane, target) in out.iter_mut().enumerate() {
        let nibble = (c[lane / 2] >> ((lane % 2) * 4)) & 15;
        let value = bytes[usize::from(nibble % 8)];
        *target = if nibble < 8 {
            value
        } else if i8::from_le_bytes([value]) < 0 {
            255
        } else {
            0
        };
    }
    u32::from_le_bytes(out)
}
fn byte_permute_draw(
    d: &mut GpuDriver,
    c: ChannelId,
    b: BackingId,
    data: &mut [u8],
    words: &[u32],
    payload: [u32; 4],
    matches: bool,
) {
    let mut bytes = [0u8; 16];
    for (dst, raw) in bytes.chunks_exact_mut(4).zip(payload) {
        dst.copy_from_slice(&raw.to_le_bytes());
    }
    d.write_backing(b, 0x50, &bytes, &mut mem(data)).unwrap();
    let r = submit(d, c, b, data, words);
    assert!(r.fault.is_none(), "{:?}", r.fault);
    assert_eq!(r.progress.draws_submitted, 1);
    d.publish(c, &mut mem(data)).unwrap();
    check(
        data,
        if matches {
            [64, 128, 191, 255]
        } else {
            [0, 128, 191, 255]
        },
        false,
    );
    d.retire(r.token).unwrap();
}
#[test]
#[ignore = "requires actual GPU; dynamic and immediate PRMT selectors, sign replication, aliases and deferred fault visibility"]
fn driver_byte_permutation_selectors_resources_and_aliases() {
    let (mut data, mut words, _) = fixture();
    let (mut d, c, _, b, _) = driver();
    let bin = include_bytes!("fixtures/maxwell/byte-permute.bin");
    let imm = include_bytes!("fixtures/maxwell/byte-permute-immediate.bin");
    data[0x4600..0x4600 + bin.len()].copy_from_slice(bin);
    data[0x4a00..0x4a00 + imm.len()].copy_from_slice(imm);
    let end = words.split_off(words.len() - 8);
    for (m, v) in [
        (0x2144, 0x600),
        (0x214c, 8),
        (0x2380, 32),
        (0x2384, 0),
        (0x2388, 0x40040),
        (0x2490, 1),
    ] {
        method(&mut words, m, v);
    }
    words.extend(end);
    let draw_words = draw();
    let original = u64::from_le_bytes(bin[0x80..0x88].try_into().unwrap());
    let comparison = u64::from_le_bytes(bin[0x88..0x90].try_into().unwrap());
    assert_eq!(original, 0x5bc0028000670404);
    assert_eq!(comparison, 0x5b64038000770407);
    for (selector, expected) in [
        (0x3210, 0x807fff00),
        (0x7654, 0x01807ffe),
        (0x0123, 0x00ff7f80),
        (0x4567, 0xfe7f8001),
        (0xba98, 0xff00ff00),
        (0xfedc, 0x00ff00ff),
        (0x5410, 0x7ffeff00),
        (0x7632, 0x0180807f),
    ] {
        assert_eq!(
            byte_permute_reference(0x807fff00, 0x01807ffe, selector),
            expected
        );
    }
    let inputs = [(0x807fff00u32, 0x01807ffeu32), (0x81fe7f01, 0x7e0080ff)];
    let mut selectors = Vec::new();
    for lane in 0..4 {
        for nibble in 0..16u32 {
            selectors.push((0x3210 & !(15 << (lane * 4))) | (nibble << (lane * 4)));
        }
    }
    selectors.extend([
        0, 0xffff, 0x3210, 0x7654, 0x0123, 0x4567, 0xba98, 0xfedc, 0x5410, 0x7632,
    ]);
    let mut cases = Vec::new();
    for selector in selectors {
        for (left, right) in inputs {
            for high in [0, 0xdead0000, 0xffff0000] {
                cases.push((left, right, selector | high));
            }
        }
    }
    let mut seed = 0x50524d54u32;
    for _ in 0..32 {
        let mut values = [0u32; 3];
        for value in &mut values {
            seed ^= seed << 13;
            seed ^= seed >> 17;
            seed ^= seed << 5;
            *value = seed;
        }
        cases.push((values[0], values[1], values[2]));
    }
    assert_eq!(cases.len(), 476);
    let mut identity = None;
    for (index, &(left, right, selector)) in cases.iter().enumerate() {
        byte_permute_draw(
            &mut d,
            c,
            b,
            &mut data,
            if index == 0 { &words } else { &draw_words },
            [
                left,
                right,
                selector,
                byte_permute_reference(left, right, selector),
            ],
            true,
        );
        let n = d.graphics_counters();
        let current = [
            n.shader_decodes,
            n.shader_module_allocations,
            n.pipeline_allocations,
        ];
        if let Some(old) = identity {
            assert_eq!(current, old);
        }
        identity = Some(current);
    }
    // The main GPR selector is R6. Exercise destination=B and destination=control
    // with independently patched ISETP input; auxiliary PRMT must preserve P0.
    for dst in [5u8, 6] {
        let op = (original & !255u64) | u64::from(dst);
        let compare = (comparison & !(255u64 << 8)) | (u64::from(dst) << 8);
        d.write_backing(b, 0x4680, &op.to_le_bytes(), &mut mem(&mut data))
            .unwrap();
        d.write_backing(b, 0x4688, &compare.to_le_bytes(), &mut mem(&mut data))
            .unwrap();
        for selector in [0x0123, 0x5410, 0xfedc] {
            for (left, right) in inputs {
                byte_permute_draw(
                    &mut d,
                    c,
                    b,
                    &mut data,
                    &draw_words,
                    [
                        left,
                        right,
                        selector,
                        byte_permute_reference(left, right, selector),
                    ],
                    true,
                );
            }
        }
    }
    d.write_backing(b, 0x4688, &comparison.to_le_bytes(), &mut mem(&mut data))
        .unwrap();
    for (left_reg, right_reg, control_reg) in [
        (255u8, 5u8, 6u8),
        (4, 255, 6),
        (4, 5, 255),
        (255, 255, 6),
        (255, 255, 255),
    ] {
        let op = (original & !((255u64 << 8) | (255u64 << 39) | (255u64 << 20)))
            | (u64::from(left_reg) << 8)
            | (u64::from(right_reg) << 39)
            | (u64::from(control_reg) << 20);
        d.write_backing(b, 0x4680, &op.to_le_bytes(), &mut mem(&mut data))
            .unwrap();
        let (left, right, selector) = (0x81fe7f01, 0x7e0080ff, 0xdead7654);
        let expected = byte_permute_reference(
            if left_reg == 255 { 0 } else { left },
            if right_reg == 255 { 0 } else { right },
            if control_reg == 255 { 0 } else { selector },
        );
        byte_permute_draw(
            &mut d,
            c,
            b,
            &mut data,
            &draw_words,
            [left, right, selector, expected],
            true,
        );
    }
    // Separate guest-selected program uses an immediate selector at PC78.
    // Its expected word is c0[18], whereas the dynamic program uses c0[1c].
    let immediate = u64::from_le_bytes(imm[0x78..0x80].try_into().unwrap());
    assert_eq!(immediate, 0x36c0028541070404);
    let mut select_immediate = Vec::new();
    method(&mut select_immediate, 0x2144, 0xa00);
    select_immediate.extend(&draw_words);
    for (index, selector) in [
        0u16, 0xffff, 0x3210, 0x7654, 0x0123, 0x4567, 0xba98, 0xfedc, 0x5410, 0x7632,
    ]
    .into_iter()
    .enumerate()
    {
        let op = (immediate & !(0xffffu64 << 20)) | (u64::from(selector) << 20);
        // Selector0 is the independently MaxAs-verified canonical36c form,
        // not envyas's alternative5bc GPR-control-RZ encoding.
        d.write_backing(b, 0x4a78, &op.to_le_bytes(), &mut mem(&mut data))
            .unwrap();
        for (pair, &(left, right)) in inputs.iter().enumerate() {
            let expected = byte_permute_reference(left, right, u32::from(selector));
            byte_permute_draw(
                &mut d,
                c,
                b,
                &mut data,
                if index == 0 && pair == 0 {
                    &select_immediate
                } else {
                    &draw_words
                },
                [left, right, expected, 0],
                true,
            );
        }
    }
    d.write_backing(b, 0x4680, &original.to_le_bytes(), &mut mem(&mut data))
        .unwrap();
    let mut select_dynamic = Vec::new();
    method(&mut select_dynamic, 0x2144, 0x600);
    select_dynamic.extend(&draw_words);
    let (left, right, selector) = (0x807fff00, 0x01807ffe, 0xdead3210);
    let expected = byte_permute_reference(left, right, selector);
    assert_eq!(expected, left);
    byte_permute_draw(
        &mut d,
        c,
        b,
        &mut data,
        &select_dynamic,
        [left, right, selector, expected ^ 1],
        false,
    );
    let mut payload = [0u8; 16];
    for (dst, raw) in payload
        .chunks_exact_mut(4)
        .zip([left, right, selector, expected])
    {
        dst.copy_from_slice(&raw.to_le_bytes());
    }
    d.write_backing(b, 0x50, &payload, &mut mem(&mut data))
        .unwrap();
    let priming = submit(&mut d, c, b, &mut data, &draw_words);
    assert!(priming.fault.is_none(), "{:?}", priming.fault);
    assert_eq!(priming.progress.draws_submitted, 1);
    assert!(!d.query(priming.token).unwrap().guest_visible);
    check(&data, [0, 128, 191, 255], false);
    let unchanged = |n: wl_gpu::Counters| {
        [
            n.shader_binary_bytes,
            n.vertex_shader_bytes,
            n.shader_decodes,
            n.vertex_shader_decodes,
            n.descriptor_reads,
            n.vertex_guest_bytes,
            n.index_guest_bytes,
            n.constant_guest_bytes,
            n.guest_texture_bytes,
            n.render_target_guest_bytes,
            n.upload_bytes,
            n.upload_calls,
            n.shader_module_allocations,
            n.pipeline_allocations,
            n.render_target_allocations,
            n.readback_operations,
            n.synchronization_points,
        ]
    };
    let before = d.graphics_counters();
    let start = std::time::Instant::now();
    let mut last = None;
    for _ in 0..32 {
        let r = submit_existing(&mut d, c, &mut data, 8);
        assert!(r.fault.is_none(), "{:?}", r.fault);
        assert_eq!(r.progress.draws_submitted, 1);
        last = Some(r.token);
    }
    let elapsed = start.elapsed();
    let after = d.graphics_counters();
    assert_eq!(after.draw_commands - before.draw_commands, 32);
    assert_eq!(after.submissions - before.submissions, 32);
    assert_eq!(unchanged(after), unchanged(before));
    check(&data, [0, 128, 191, 255], false);
    let last = last.unwrap();
    assert!(!d.query(last).unwrap().guest_visible);
    d.write_backing(
        b,
        0x4680,
        &(original | (1 << 48)).to_le_bytes(),
        &mut mem(&mut data),
    )
    .unwrap();
    let failed = submit(&mut d, c, b, &mut data, &draw_words);
    assert!(failed.fault.is_some());
    assert_eq!(failed.progress.draws_submitted, 0);
    assert_eq!(d.graphics_counters().draw_commands, after.draw_commands);
    assert_eq!(d.graphics_counters().submissions, after.submissions);
    check(&data, [0, 128, 191, 255], false);
    d.publish(c, &mut mem(&mut data)).unwrap();
    check(&data, [64, 128, 191, 255], false);
    assert!(d.query(priming.token).unwrap().guest_visible);
    assert!(d.query(last).unwrap().guest_visible);
    assert_eq!(d.graphics_counters().draw_commands, after.draw_commands);
    eprintln!(
        "PRMT {} dynamic cases, warm32 CPU={elapsed:?} before={before:?} after={after:?}",
        cases.len()
    );
    d.close(c, &mut mem(&mut data)).unwrap();
}
#[test]
#[ignore = "requires actual GPU; PRMT selector/destination alias controls finite guest vertex geometry"]
fn driver_vertex_byte_permutation_controls_geometry_and_code() {
    let (mut data, mut words, _) = fixture();
    let (mut d, c, _, b, _) = driver();
    let bin = include_bytes!("fixtures/maxwell/vertex-byte-permute.bin");
    data[0x4800..0x4800 + bin.len()].copy_from_slice(bin);
    let end = words.split_off(words.len() - 8);
    for (m, v) in [(0x2044, 0x800), (0x204c, 9), (0x2144, 0x400)] {
        method(&mut words, m, v);
    }
    words.extend(end);
    assert_eq!(
        u64::from_le_bytes(bin[0x80..0x88].try_into().unwrap()),
        0x5bc07f8000670006
    );
    for (vertex, expected) in [(0, 0xcdcc4cbf), (1, 0x9a99193f), (2, 0xcdcc4cbe)] {
        assert_eq!(
            byte_permute_reference(POS[vertex][0].to_bits(), 0, 0xdead0123),
            expected
        );
    }
    let r = submit(&mut d, c, b, &mut data, &words);
    assert!(r.fault.is_none(), "{:?}", r.fault);
    assert_eq!(r.progress.draws_submitted, 1);
    d.publish(c, &mut mem(&mut data)).unwrap();
    check_conditional_geometry(&data, 0.5);
    d.retire(r.token).unwrap();
    assert_eq!(
        byte_permute_reference((-0.6f32).to_bits(), 0, 0xdead0123),
        0x9a9919bf
    );
    d.write_backing(b, 0x3000, &(-0.6f32).to_le_bytes(), &mut mem(&mut data))
        .unwrap();
    let background: Vec<_> = (0..W * H).flat_map(|_| BG).collect();
    d.write_backing(b, RT as u64, &background, &mut mem(&mut data))
        .unwrap();
    let before = d.graphics_counters();
    let r = submit(&mut d, c, b, &mut data, &draw());
    assert!(r.fault.is_none(), "{:?}", r.fault);
    assert_eq!(r.progress.draws_submitted, 1);
    let after = d.graphics_counters();
    assert_eq!(after.vertex_shader_decodes, before.vertex_shader_decodes);
    assert_eq!(after.pipeline_allocations, before.pipeline_allocations);
    assert_eq!(after.vertex_upload_bytes - before.vertex_upload_bytes, 72);
    d.publish(c, &mut mem(&mut data)).unwrap();
    check_conditional_geometry(&data, f64::from(-0.6f32));
    d.retire(r.token).unwrap();
    // Actual PCs: selector MOV is68, expected MOV is78, PRMT is80.
    // Identity3210 plus expected rawBF19999A restore only the first vertex.
    assert_eq!(
        byte_permute_reference((-0.6f32).to_bits(), 0, 0xdead3210),
        0xbf19999a
    );
    let control = 0x010000000007f006u64 | (0xdead3210u64 << 20);
    let expected = 0x010000000007f007u64 | (u64::from((-0.6f32).to_bits()) << 20);
    d.write_backing(b, 0x4868, &control.to_le_bytes(), &mut mem(&mut data))
        .unwrap();
    d.write_backing(b, 0x4878, &expected.to_le_bytes(), &mut mem(&mut data))
        .unwrap();
    d.write_backing(b, RT as u64, &background, &mut mem(&mut data))
        .unwrap();
    let before = d.graphics_counters();
    let r = submit(&mut d, c, b, &mut data, &draw());
    assert!(r.fault.is_none(), "{:?}", r.fault);
    assert_eq!(r.progress.draws_submitted, 1);
    let after = d.graphics_counters();
    assert_eq!(
        after.vertex_shader_decodes,
        before.vertex_shader_decodes + 1
    );
    assert_eq!(after.shader_decodes, before.shader_decodes);
    assert_eq!(after.vertex_upload_bytes, before.vertex_upload_bytes);
    assert!(after.pipeline_allocations > before.pipeline_allocations);
    d.publish(c, &mut mem(&mut data)).unwrap();
    check_conditional_geometry(&data, 0.5);
    d.retire(r.token).unwrap();
    eprintln!("VS PRMT geometry {:?}", d.graphics_counters());
    d.close(c, &mut mem(&mut data)).unwrap();
}

fn add_three_reference(a: u32, b: u32, c: u32) -> u32 {
    (u64::from(a) + u64::from(b) + u64::from(c)) as u32
}
fn add_three_draw(
    d: &mut GpuDriver,
    c: ChannelId,
    b: BackingId,
    data: &mut [u8],
    words: &[u32],
    payload: [u32; 4],
    matches: bool,
) {
    let mut bytes = [0u8; 16];
    for (dst, raw) in bytes.chunks_exact_mut(4).zip(payload) {
        dst.copy_from_slice(&raw.to_le_bytes());
    }
    d.write_backing(b, 0x50, &bytes, &mut mem(data)).unwrap();
    let r = submit(d, c, b, data, words);
    assert!(r.fault.is_none(), "{:?}", r.fault);
    assert_eq!(r.progress.draws_submitted, 1);
    d.publish(c, &mut mem(data)).unwrap();
    check(
        data,
        if matches {
            [64, 128, 191, 255]
        } else {
            [0, 128, 191, 255]
        },
        false,
    );
    d.retire(r.token).unwrap();
}
#[test]
#[ignore = "requires actual GPU; three-source IADD3 wrapping, aliases, RZ, cache and deferred fault visibility"]
fn driver_three_source_addition_wraps_and_preserves_deferred_output() {
    let (mut data, mut words, _) = fixture();
    let (mut d, c, _, b, _) = driver();
    let bin = include_bytes!("fixtures/maxwell/integer-add-three.bin");
    data[0x4600..0x4600 + bin.len()].copy_from_slice(bin);
    let end = words.split_off(words.len() - 8);
    for (m, v) in [
        (0x2144, 0x600),
        (0x214c, 8),
        (0x2380, 32),
        (0x2384, 0),
        (0x2388, 0x40040),
        (0x2490, 1),
    ] {
        method(&mut words, m, v);
    }
    words.extend(end);
    let draw_words = draw();
    let original = u64::from_le_bytes(bin[0x80..0x88].try_into().unwrap());
    let comparison = u64::from_le_bytes(bin[0x88..0x90].try_into().unwrap());
    assert_eq!(original, 0x5cc0030000570404);
    assert_eq!(comparison, 0x5b64038000770407);
    let mut cases = vec![
        (0, 0, 0),
        (1, 0, 0),
        (0, 1, 0),
        (0, 0, 1),
        (u32::MAX, 0, 0),
        (0, u32::MAX, 0),
        (0, 0, u32::MAX),
        (u32::MAX, 1, 0),
        (u32::MAX, 0, 1),
        (0, u32::MAX, 1),
        (u32::MAX, u32::MAX, 0),
        (u32::MAX, u32::MAX, 1),
        (u32::MAX, u32::MAX, u32::MAX),
        (0x7fffffff, 1, 0),
        (0x7fffffff, 0, 1),
        (0x80000000, 0x80000000, 0),
        (0x80000000, 0, 0x80000000),
        (0, 0x80000000, 0x80000000),
        (0x80000000, 0x80000000, 0x80000000),
        (0x7fffffff, 0x7fffffff, 0x7fffffff),
        (0xffff, 1, 0),
        (0xffff, 0xffff, 2),
        (0x00ffffff, 1, 0),
        (0x55555555, 0xaaaaaaaa, 1),
        (0x80000001, 0x7ffffffe, 1),
        (0x01020304, 0x10203040, 0x807fff00),
        (0xdeadbeef, 0x01010101, 0x10203040),
        (0xffff0000, 0x0000ffff, 1),
        (0x12345678, 0x87654321, 0xa5a5a5a5),
        (0x7fffffff, 0x80000000, 1),
        (0x80000000, 0x7fffffff, 0),
        (0xffffffff, 0xffffffff, 2),
    ];
    // Literal checks independently expose carry loss and accidental signed saturation.
    assert_eq!(
        add_three_reference(u32::MAX, u32::MAX, u32::MAX),
        0xfffffffd
    );
    assert_eq!(
        add_three_reference(0x7fffffff, 0x7fffffff, 0x7fffffff),
        0x7ffffffd
    );
    assert_eq!(
        add_three_reference(0x80000000, 0x80000000, 0x80000000),
        0x80000000
    );
    assert_eq!(add_three_reference(u32::MAX, 1, 0), 0);
    let mut seed = 0x49414433u32;
    for _ in 0..32 {
        let mut raw = [0u32; 3];
        for value in &mut raw {
            seed ^= seed << 13;
            seed ^= seed >> 17;
            seed ^= seed << 5;
            *value = seed;
        }
        cases.push((raw[0], raw[1], raw[2]));
    }
    assert_eq!(cases.len(), 64);
    let mut identity = None;
    for (index, &(a, bits_b, bits_c)) in cases.iter().enumerate() {
        add_three_draw(
            &mut d,
            c,
            b,
            &mut data,
            if index == 0 { &words } else { &draw_words },
            [a, bits_b, bits_c, add_three_reference(a, bits_b, bits_c)],
            true,
        );
        let n = d.graphics_counters();
        let now = [
            n.shader_decodes,
            n.shader_module_allocations,
            n.pipeline_allocations,
        ];
        if let Some(old) = identity {
            assert_eq!(now, old);
        }
        identity = Some(now);
    }
    // A aliases by construction. B and C also must read their old values before writing.
    for dst in [5u8, 6] {
        let op = (original & !255u64) | u64::from(dst);
        let cmp = (comparison & !(255u64 << 8)) | (u64::from(dst) << 8);
        d.write_backing(b, 0x4680, &op.to_le_bytes(), &mut mem(&mut data))
            .unwrap();
        d.write_backing(b, 0x4688, &cmp.to_le_bytes(), &mut mem(&mut data))
            .unwrap();
        for (a, bits_b, bits_c) in [
            (u32::MAX, 1, 0),
            (0x10203040, 0x807fff00, 0x01020304),
            (u32::MAX, u32::MAX, u32::MAX),
        ] {
            add_three_draw(
                &mut d,
                c,
                b,
                &mut data,
                &draw_words,
                [a, bits_b, bits_c, add_three_reference(a, bits_b, bits_c)],
                true,
            );
        }
    }
    d.write_backing(b, 0x4688, &comparison.to_le_bytes(), &mut mem(&mut data))
        .unwrap();
    // Nonzero poisoned constant data must never leak through an RZ operand.
    for zero_mask in 1..8u8 {
        let regs = std::array::from_fn::<_, 3, _>(|slot| {
            if zero_mask & (1 << slot) != 0 {
                255u8
            } else {
                4 + slot as u8
            }
        });
        let op = (original & !((255u64 << 8) | (255u64 << 20) | (255u64 << 39)))
            | (u64::from(regs[0]) << 8)
            | (u64::from(regs[1]) << 20)
            | (u64::from(regs[2]) << 39);
        d.write_backing(b, 0x4680, &op.to_le_bytes(), &mut mem(&mut data))
            .unwrap();
        let poison = [0x10203040, 0x807fff00, 0x01020304];
        let actual =
            std::array::from_fn::<_, 3, _>(|slot| if regs[slot] == 255 { 0 } else { poison[slot] });
        add_three_draw(
            &mut d,
            c,
            b,
            &mut data,
            &draw_words,
            [
                poison[0],
                poison[1],
                poison[2],
                add_three_reference(actual[0], actual[1], actual[2]),
            ],
            true,
        );
    }
    d.write_backing(b, 0x4680, &original.to_le_bytes(), &mut mem(&mut data))
        .unwrap();
    let raw = [0xfffffff0, 0x01020304, 0x807fff00];
    let expected = add_three_reference(raw[0], raw[1], raw[2]);
    add_three_draw(
        &mut d,
        c,
        b,
        &mut data,
        &draw_words,
        [raw[0], raw[1], raw[2], expected ^ 1],
        false,
    );
    let mut payload = [0u8; 16];
    for (dst, v) in payload
        .chunks_exact_mut(4)
        .zip([raw[0], raw[1], raw[2], expected])
    {
        dst.copy_from_slice(&v.to_le_bytes());
    }
    d.write_backing(b, 0x50, &payload, &mut mem(&mut data))
        .unwrap();
    let priming = submit(&mut d, c, b, &mut data, &draw_words);
    assert!(priming.fault.is_none(), "{:?}", priming.fault);
    assert_eq!(priming.progress.draws_submitted, 1);
    assert!(!d.query(priming.token).unwrap().guest_visible);
    check(&data, [0, 128, 191, 255], false);
    let unchanged = |n: wl_gpu::Counters| {
        [
            n.shader_binary_bytes,
            n.vertex_shader_bytes,
            n.shader_decodes,
            n.vertex_shader_decodes,
            n.descriptor_reads,
            n.vertex_guest_bytes,
            n.index_guest_bytes,
            n.constant_guest_bytes,
            n.guest_texture_bytes,
            n.render_target_guest_bytes,
            n.upload_bytes,
            n.upload_calls,
            n.shader_module_allocations,
            n.pipeline_allocations,
            n.render_target_allocations,
            n.readback_operations,
            n.synchronization_points,
        ]
    };
    let before = d.graphics_counters();
    let start = std::time::Instant::now();
    let mut last = None;
    for _ in 0..32 {
        let r = submit_existing(&mut d, c, &mut data, 8);
        assert!(r.fault.is_none(), "{:?}", r.fault);
        assert_eq!(r.progress.draws_submitted, 1);
        last = Some(r.token);
    }
    let elapsed = start.elapsed();
    let after = d.graphics_counters();
    assert_eq!(after.draw_commands - before.draw_commands, 32);
    assert_eq!(after.submissions - before.submissions, 32);
    assert_eq!(unchanged(after), unchanged(before));
    let last = last.unwrap();
    assert!(!d.query(last).unwrap().guest_visible);
    check(&data, [0, 128, 191, 255], false);
    d.write_backing(
        b,
        0x4680,
        &(original | (1 << 47)).to_le_bytes(),
        &mut mem(&mut data),
    )
    .unwrap();
    let failed = submit(&mut d, c, b, &mut data, &draw_words);
    assert!(failed.fault.is_some());
    assert_eq!(failed.progress.draws_submitted, 0);
    assert_eq!(d.graphics_counters().draw_commands, after.draw_commands);
    assert_eq!(d.graphics_counters().submissions, after.submissions);
    check(&data, [0, 128, 191, 255], false);
    d.publish(c, &mut mem(&mut data)).unwrap();
    check(&data, [64, 128, 191, 255], false);
    assert!(d.query(priming.token).unwrap().guest_visible);
    assert!(d.query(last).unwrap().guest_visible);
    assert_eq!(d.graphics_counters().draw_commands, after.draw_commands);
    eprintln!(
        "IADD3 {} raw tuples, warm32 CPU={elapsed:?} before={before:?} after={after:?}",
        cases.len()
    );
    d.close(c, &mut mem(&mut data)).unwrap();
}
#[test]
#[ignore = "requires actual GPU; IADD3 old B input and guest shader code select finite asymmetric geometry"]
fn driver_vertex_three_source_addition_controls_geometry_and_code() {
    let (mut data, mut words, _) = fixture();
    let (mut d, c, _, b, _) = driver();
    let bin = include_bytes!("fixtures/maxwell/vertex-integer-add-three.bin");
    data[0x4800..0x4800 + bin.len()].copy_from_slice(bin);
    let end = words.split_off(words.len() - 8);
    for (m, v) in [(0x2044, 0x800), (0x204c, 9), (0x2144, 0x400)] {
        method(&mut words, m, v);
    }
    words.extend(end);
    assert_eq!(
        u64::from_le_bytes(bin[0x80..0x88].try_into().unwrap()),
        0x5cc0038000670006
    );
    for (i, expected) in [(0, 0x3f4ccccd), (1, 0xbf19999a), (2, 0x3e4ccccd)] {
        assert_eq!(
            add_three_reference(POS[i][0].to_bits(), 0x7fffffff, 1),
            expected
        );
    }
    let r = submit(&mut d, c, b, &mut data, &words);
    assert!(r.fault.is_none(), "{:?}", r.fault);
    assert_eq!(r.progress.draws_submitted, 1);
    d.publish(c, &mut mem(&mut data)).unwrap();
    check_conditional_geometry(&data, 0.5);
    d.retire(r.token).unwrap();
    assert_eq!(
        add_three_reference((-0.6f32).to_bits(), 0x7fffffff, 1),
        0x3f19999a
    );
    d.write_backing(b, 0x3000, &(-0.6f32).to_le_bytes(), &mut mem(&mut data))
        .unwrap();
    let background: Vec<_> = (0..W * H).flat_map(|_| BG).collect();
    d.write_backing(b, RT as u64, &background, &mut mem(&mut data))
        .unwrap();
    let before = d.graphics_counters();
    let r = submit(&mut d, c, b, &mut data, &draw());
    assert!(r.fault.is_none(), "{:?}", r.fault);
    assert_eq!(r.progress.draws_submitted, 1);
    let after = d.graphics_counters();
    assert_eq!(after.vertex_shader_decodes, before.vertex_shader_decodes);
    assert_eq!(after.pipeline_allocations, before.pipeline_allocations);
    assert_eq!(after.vertex_upload_bytes - before.vertex_upload_bytes, 72);
    d.publish(c, &mut mem(&mut data)).unwrap();
    check_conditional_geometry(&data, f64::from(-0.6f32));
    d.retire(r.token).unwrap();
    // Change actual source C from1 to2 and expected to3f19999b. Only first matches.
    assert_eq!(
        add_three_reference((-0.6f32).to_bits(), 0x7fffffff, 2),
        0x3f19999b
    );
    assert_eq!(
        add_three_reference(POS[1][0].to_bits(), 0x7fffffff, 2),
        0xbf19999b
    );
    assert_eq!(
        add_three_reference(POS[2][0].to_bits(), 0x7fffffff, 2),
        0x3e4cccce
    );
    let source = 0x010000000007f007u64 | (2u64 << 20);
    let expected = 0x010000000007f007u64 | (0x3f19999bu64 << 20);
    d.write_backing(b, 0x4878, &source.to_le_bytes(), &mut mem(&mut data))
        .unwrap();
    d.write_backing(b, 0x4888, &expected.to_le_bytes(), &mut mem(&mut data))
        .unwrap();
    d.write_backing(b, RT as u64, &background, &mut mem(&mut data))
        .unwrap();
    let before = d.graphics_counters();
    let r = submit(&mut d, c, b, &mut data, &draw());
    assert!(r.fault.is_none(), "{:?}", r.fault);
    assert_eq!(r.progress.draws_submitted, 1);
    let after = d.graphics_counters();
    assert_eq!(
        after.vertex_shader_decodes,
        before.vertex_shader_decodes + 1
    );
    assert_eq!(after.shader_decodes, before.shader_decodes);
    assert_eq!(after.vertex_upload_bytes, before.vertex_upload_bytes);
    assert!(after.pipeline_allocations > before.pipeline_allocations);
    d.publish(c, &mut mem(&mut data)).unwrap();
    check_conditional_geometry(&data, 0.5);
    d.retire(r.token).unwrap();
    eprintln!("VS IADD3 geometry {:?}", d.graphics_counters());
    d.close(c, &mut mem(&mut data)).unwrap();
}

fn multiply_add16_reference(a: u32, b: u32, c: u32, signed: bool) -> u32 {
    let aa = a.to_le_bytes();
    let bb = b.to_le_bytes();
    if signed {
        let x = i16::from_le_bytes([aa[0], aa[1]]);
        let y = i16::from_le_bytes([bb[0], bb[1]]);
        (i64::from(x) * i64::from(y) + i64::from(c)) as u32
    } else {
        let x = u16::from_le_bytes([aa[0], aa[1]]);
        let y = u16::from_le_bytes([bb[0], bb[1]]);
        (u64::from(x) * u64::from(y) + u64::from(c)) as u32
    }
}
fn multiply_add16_draw(
    d: &mut GpuDriver,
    c: ChannelId,
    b: BackingId,
    data: &mut [u8],
    words: &[u32],
    payload: [u32; 4],
    matches: bool,
) {
    let mut bytes = [0u8; 16];
    for (dst, raw) in bytes.chunks_exact_mut(4).zip(payload) {
        dst.copy_from_slice(&raw.to_le_bytes());
    }
    d.write_backing(b, 0x50, &bytes, &mut mem(data)).unwrap();
    let r = submit(d, c, b, data, words);
    assert!(r.fault.is_none(), "{:?}", r.fault);
    assert_eq!(r.progress.draws_submitted, 1);
    d.publish(c, &mut mem(data)).unwrap();
    check(
        data,
        if matches {
            [64, 128, 191, 255]
        } else {
            [0, 128, 191, 255]
        },
        false,
    );
    d.retire(r.token).unwrap();
}
#[test]
#[ignore = "requires actual GPU; like-signed low16 XMAD full32 addend, aliases, RZ, cache and deferred fault visibility"]
fn driver_halfword_multiply_add_wraps_and_preserves_deferred_output() {
    for signed in [false, true] {
        let (mut data, mut words, _) = fixture();
        let (mut d, c, _, b, _) = driver();
        let bin = if signed {
            include_bytes!("fixtures/maxwell/multiply-add16-signed.bin").as_slice()
        } else {
            include_bytes!("fixtures/maxwell/multiply-add16.bin").as_slice()
        };
        data[0x4600..0x4600 + bin.len()].copy_from_slice(bin);
        let end = words.split_off(words.len() - 8);
        for (m, v) in [
            (0x2144, 0x600),
            (0x214c, 8),
            (0x2380, 32),
            (0x2384, 0),
            (0x2388, 0x40040),
            (0x2490, 1),
        ] {
            method(&mut words, m, v);
        }
        words.extend(end);
        let draw_words = draw();
        let original = u64::from_le_bytes(bin[0x80..0x88].try_into().unwrap());
        let comparison = u64::from_le_bytes(bin[0x88..0x90].try_into().unwrap());
        assert_eq!(
            original,
            if signed {
                0x5b03030000570404
            } else {
                0x5b00030000570404
            }
        );
        assert_eq!(comparison, 0x5b64038000770407);
        let mut cases = vec![
            (0, 0, 0),
            (1, 0, 1),
            (0, 1, 2),
            (1, 1, u32::MAX),
            (0xffff, 0xffff, 0),
            (0x8000, 0x8000, 0),
            (0x8000, 0x7fff, 0),
            (0xffff, 1, 0),
            (0x7fff, 0x7fff, 0),
            (0xdeadffff, 0xbeefffff, 0),
            (0xffff0000, 0xffff0000, 0xdeadbeef),
            (0x80000000, 0xffffffff, 0x12345678),
            (0, 0, u32::MAX),
            (1, 1, 0x7fffffff),
            (1, 1, 0x80000000),
            (2, 2, 0xfffffffc),
            (0x8000, 0xffff, u32::MAX),
            (0x7fff, 0xffff, 0x80000000),
            (0xffff, 0xffff, 0xffffffff),
            (0xffff, 0xffff, 0x0001ffff),
            (0x7fff8000, 0x80007fff, 0x80000001),
            (0x80007fff, 0x7fff8000, 0x7fffffff),
            (0x80008000, 0x7fff8000, 0xc0000000),
            (0x55555555, 0xaaaaaaaa, 0x01234567),
            (0x00010001, 0xffff0001, 0x01020304),
            (0x123400ff, 0x56780100, 0xffffff01),
            (0x8080ff80, 0x7f7f0080, 0x11111111),
            (0x0000ffff, 0xffff0000, 0xa5a5a5a5),
            (0xffff0000, 0x0000ffff, 0x5a5a5a5a),
            (0x12345678, 0x87654321, 0xa5a5a5a5),
            (0x01230002, 0x45670003, 0x80000000),
            (0xffffffff, 0xffffffff, 2),
        ];
        // Independent literal checks distinguish low-half extraction, signedness and full C.
        assert_eq!(
            multiply_add16_reference(0xffff, 0xffff, 0, false),
            0xfffe0001
        );
        assert_eq!(multiply_add16_reference(0xffff, 0xffff, 0, true), 1);
        assert_eq!(
            multiply_add16_reference(0x8000, 0x7fff, 0, true),
            0xc0008000
        );
        assert_eq!(multiply_add16_reference(0xffff, 1, 0, true), u32::MAX);
        assert_eq!(
            multiply_add16_reference(0xffff0000, 0xffff0000, 0xdeadbeef, signed),
            0xdeadbeef
        );
        assert_eq!(multiply_add16_reference(2, 2, 0xfffffffc, signed), 0);
        assert_eq!(
            multiply_add16_reference(0xdeadffff, 0xbeefffff, 0, signed),
            multiply_add16_reference(0xffff, 0xffff, 0, signed)
        );
        let mut seed = 0x584d4144u32;
        for _ in 0..32 {
            let mut raw = [0u32; 3];
            for value in &mut raw {
                seed ^= seed << 13;
                seed ^= seed >> 17;
                seed ^= seed << 5;
                *value = seed;
            }
            cases.push((raw[0], raw[1], raw[2]));
        }
        assert_eq!(cases.len(), 64);
        let mut identity = None;
        for (index, &(a, bits_b, bits_c)) in cases.iter().enumerate() {
            multiply_add16_draw(
                &mut d,
                c,
                b,
                &mut data,
                if index == 0 { &words } else { &draw_words },
                [
                    a,
                    bits_b,
                    bits_c,
                    multiply_add16_reference(a, bits_b, bits_c, signed),
                ],
                true,
            );
            let n = d.graphics_counters();
            let now = [
                n.shader_decodes,
                n.shader_module_allocations,
                n.pipeline_allocations,
            ];
            if let Some(old) = identity {
                assert_eq!(now, old);
            }
            identity = Some(now);
        }
        // A aliases by construction. B and C also must read their old values before writing.
        for dst in [5u8, 6] {
            let op = (original & !255u64) | u64::from(dst);
            let cmp = (comparison & !(255u64 << 8)) | (u64::from(dst) << 8);
            d.write_backing(b, 0x4680, &op.to_le_bytes(), &mut mem(&mut data))
                .unwrap();
            d.write_backing(b, 0x4688, &cmp.to_le_bytes(), &mut mem(&mut data))
                .unwrap();
            for (a, bits_b, bits_c) in [
                (u32::MAX, 1, 0),
                (0x10203040, 0x807fff00, 0x01020304),
                (u32::MAX, u32::MAX, u32::MAX),
            ] {
                multiply_add16_draw(
                    &mut d,
                    c,
                    b,
                    &mut data,
                    &draw_words,
                    [
                        a,
                        bits_b,
                        bits_c,
                        multiply_add16_reference(a, bits_b, bits_c, signed),
                    ],
                    true,
                );
            }
        }
        d.write_backing(b, 0x4688, &comparison.to_le_bytes(), &mut mem(&mut data))
            .unwrap();
        // Nonzero poisoned constant data must never leak through an RZ operand.
        for zero_mask in 1..8u8 {
            let regs = std::array::from_fn::<_, 3, _>(|slot| {
                if zero_mask & (1 << slot) != 0 {
                    255u8
                } else {
                    4 + slot as u8
                }
            });
            let op = (original & !((255u64 << 8) | (255u64 << 20) | (255u64 << 39)))
                | (u64::from(regs[0]) << 8)
                | (u64::from(regs[1]) << 20)
                | (u64::from(regs[2]) << 39);
            d.write_backing(b, 0x4680, &op.to_le_bytes(), &mut mem(&mut data))
                .unwrap();
            let poison = [0x10203040, 0x807fff00, 0x01020304];
            let actual =
                std::array::from_fn::<_, 3, _>(
                    |slot| if regs[slot] == 255 { 0 } else { poison[slot] },
                );
            multiply_add16_draw(
                &mut d,
                c,
                b,
                &mut data,
                &draw_words,
                [
                    poison[0],
                    poison[1],
                    poison[2],
                    multiply_add16_reference(actual[0], actual[1], actual[2], signed),
                ],
                true,
            );
        }
        d.write_backing(b, 0x4680, &original.to_le_bytes(), &mut mem(&mut data))
            .unwrap();
        let raw = [0xfffffff0, 0x01020304, 0x807fff00];
        let expected = multiply_add16_reference(raw[0], raw[1], raw[2], signed);
        multiply_add16_draw(
            &mut d,
            c,
            b,
            &mut data,
            &draw_words,
            [raw[0], raw[1], raw[2], expected ^ 1],
            false,
        );
        let mut payload = [0u8; 16];
        for (dst, v) in payload
            .chunks_exact_mut(4)
            .zip([raw[0], raw[1], raw[2], expected])
        {
            dst.copy_from_slice(&v.to_le_bytes());
        }
        d.write_backing(b, 0x50, &payload, &mut mem(&mut data))
            .unwrap();
        let priming = submit(&mut d, c, b, &mut data, &draw_words);
        assert!(priming.fault.is_none(), "{:?}", priming.fault);
        assert_eq!(priming.progress.draws_submitted, 1);
        assert!(!d.query(priming.token).unwrap().guest_visible);
        check(&data, [0, 128, 191, 255], false);
        let unchanged = |n: wl_gpu::Counters| {
            [
                n.shader_binary_bytes,
                n.vertex_shader_bytes,
                n.shader_decodes,
                n.vertex_shader_decodes,
                n.descriptor_reads,
                n.vertex_guest_bytes,
                n.index_guest_bytes,
                n.constant_guest_bytes,
                n.guest_texture_bytes,
                n.render_target_guest_bytes,
                n.upload_bytes,
                n.upload_calls,
                n.shader_module_allocations,
                n.pipeline_allocations,
                n.render_target_allocations,
                n.readback_operations,
                n.synchronization_points,
            ]
        };
        let before = d.graphics_counters();
        let start = std::time::Instant::now();
        let mut last = None;
        for _ in 0..32 {
            let r = submit_existing(&mut d, c, &mut data, 8);
            assert!(r.fault.is_none(), "{:?}", r.fault);
            assert_eq!(r.progress.draws_submitted, 1);
            last = Some(r.token);
        }
        let elapsed = start.elapsed();
        let after = d.graphics_counters();
        assert_eq!(after.draw_commands - before.draw_commands, 32);
        assert_eq!(after.submissions - before.submissions, 32);
        assert_eq!(unchanged(after), unchanged(before));
        let last = last.unwrap();
        assert!(!d.query(last).unwrap().guest_visible);
        check(&data, [0, 128, 191, 255], false);
        d.write_backing(
            b,
            0x4680,
            &(original ^ (1 << 48)).to_le_bytes(),
            &mut mem(&mut data),
        )
        .unwrap();
        let failed = submit(&mut d, c, b, &mut data, &draw_words);
        assert!(failed.fault.is_some());
        assert_eq!(failed.progress.draws_submitted, 0);
        assert_eq!(d.graphics_counters().draw_commands, after.draw_commands);
        assert_eq!(d.graphics_counters().submissions, after.submissions);
        check(&data, [0, 128, 191, 255], false);
        d.publish(c, &mut mem(&mut data)).unwrap();
        check(&data, [64, 128, 191, 255], false);
        assert!(d.query(priming.token).unwrap().guest_visible);
        assert!(d.query(last).unwrap().guest_visible);
        assert_eq!(d.graphics_counters().draw_commands, after.draw_commands);
        eprintln!(
            "XMAD signed={signed} {} raw tuples, warm32 CPU={elapsed:?} before={before:?} after={after:?}",
            cases.len()
        );
        d.close(c, &mut mem(&mut data)).unwrap();
    }
}
#[test]
#[ignore = "requires actual GPU; XMAD guest position/UV low halves select finite geometry in both signed modes"]
fn driver_vertex_halfword_multiply_add_controls_geometry_and_code() {
    let (mut data, mut words, _) = fixture();
    let (mut d, c, _, b, _) = driver();
    let bin = include_bytes!("fixtures/maxwell/vertex-multiply-add16.bin");
    data[0x4800..0x4800 + bin.len()].copy_from_slice(bin);
    let end = words.split_off(words.len() - 8);
    for (m, v) in [(0x2044, 0x800), (0x204c, 9), (0x2144, 0x400)] {
        method(&mut words, m, v);
    }
    words.extend(end);
    let original = u64::from_le_bytes(bin[0x78..0x80].try_into().unwrap());
    assert_eq!(original, 0x5b037f8000470006);
    for (i, expected) in [(0, 0x0a3d5c29), (1, 0xd70a8f5c), (2, 0)] {
        assert_eq!(
            multiply_add16_reference(POS[i][0].to_bits(), UV[i][0].to_bits(), 0, true),
            expected
        );
    }
    let r = submit(&mut d, c, b, &mut data, &words);
    assert!(r.fault.is_none(), "{:?}", r.fault);
    assert_eq!(r.progress.draws_submitted, 1);
    d.publish(c, &mut mem(&mut data)).unwrap();
    check_conditional_geometry(&data, 0.5);
    d.retire(r.token).unwrap();
    assert_eq!(
        multiply_add16_reference((-0.6f32).to_bits(), UV[0][0].to_bits(), 0, true),
        0x147ab852
    );
    d.write_backing(b, 0x3000, &(-0.6f32).to_le_bytes(), &mut mem(&mut data))
        .unwrap();
    let background: Vec<_> = (0..W * H).flat_map(|_| BG).collect();
    d.write_backing(b, RT as u64, &background, &mut mem(&mut data))
        .unwrap();
    let before = d.graphics_counters();
    let r = submit(&mut d, c, b, &mut data, &draw());
    assert!(r.fault.is_none(), "{:?}", r.fault);
    assert_eq!(r.progress.draws_submitted, 1);
    let after = d.graphics_counters();
    assert_eq!(after.vertex_shader_decodes, before.vertex_shader_decodes);
    assert_eq!(after.pipeline_allocations, before.pipeline_allocations);
    assert_eq!(after.vertex_upload_bytes - before.vertex_upload_bytes, 72);
    d.publish(c, &mut mem(&mut data)).unwrap();
    check_conditional_geometry(&data, f64::from(-0.6f32));
    d.retire(r.token).unwrap();
    // Actual mode and expected-code mutation restores only first vertex with unchanged input buffer.
    assert_eq!(
        multiply_add16_reference((-0.6f32).to_bits(), UV[0][0].to_bits(), 0, false),
        0x7ae1b852
    );
    assert_eq!(
        multiply_add16_reference(POS[1][0].to_bits(), UV[1][0].to_bits(), 0, false),
        0x3d708f5c
    );
    assert_eq!(
        multiply_add16_reference(POS[2][0].to_bits(), UV[2][0].to_bits(), 0, false),
        0
    );
    let unsigned = original & !(3u64 << 48);
    let expected = 0x010000000007f007u64 | (0x7ae1b852u64 << 20);
    d.write_backing(b, 0x4878, &unsigned.to_le_bytes(), &mut mem(&mut data))
        .unwrap();
    d.write_backing(b, 0x4868, &expected.to_le_bytes(), &mut mem(&mut data))
        .unwrap();
    d.write_backing(b, RT as u64, &background, &mut mem(&mut data))
        .unwrap();
    let before = d.graphics_counters();
    let r = submit(&mut d, c, b, &mut data, &draw());
    assert!(r.fault.is_none(), "{:?}", r.fault);
    assert_eq!(r.progress.draws_submitted, 1);
    let after = d.graphics_counters();
    assert_eq!(
        after.vertex_shader_decodes,
        before.vertex_shader_decodes + 1
    );
    assert_eq!(after.shader_decodes, before.shader_decodes);
    assert_eq!(after.vertex_upload_bytes, before.vertex_upload_bytes);
    assert!(after.pipeline_allocations > before.pipeline_allocations);
    d.publish(c, &mut mem(&mut data)).unwrap();
    check_conditional_geometry(&data, 0.5);
    d.retire(r.token).unwrap();
    eprintln!("VS XMAD geometry {:?}", d.graphics_counters());
    d.close(c, &mut mem(&mut data)).unwrap();
}

#[test]
#[ignore = "requires actual GPU; MOV c3 data, code, ready scheduling and fault-safe publication"]
fn driver_move_constant_slot_data_code_and_visibility() {
    let (mut data, mut words, _) = fixture();
    let (mut d, c, _, b, _) = driver();
    let bin = include_bytes!("fixtures/maxwell/move-constant.bin");
    data[0x4600..0x4600 + bin.len()].copy_from_slice(bin);
    let end = words.split_off(words.len() - 8);
    for (m, v) in [
        (0x2144, 0x600),
        (0x214c, 4),
        (0x2380, 32),
        (0x2384, 0),
        (0x2388, 0x40040),
        (0x2490, 0x31),
    ] {
        method(&mut words, m, v);
    }
    words.extend(end);
    let draw_words = draw();
    let mut identity = None;
    for (index, (input, color)) in [
        ([0.25f32, 0.5, 0.75, 1.], [64, 128, 191, 255]),
        ([0.75, 0.25, 0.5, 0.5], [191, 64, 128, 128]),
        ([0.25, 0.5, 0.75, 1.], [64, 128, 191, 255]),
    ]
    .into_iter()
    .enumerate()
    {
        let bytes: Vec<_> = input.into_iter().flat_map(f32::to_le_bytes).collect();
        d.write_backing(b, 0x50, &bytes, &mut mem(&mut data))
            .unwrap();
        let r = submit(
            &mut d,
            c,
            b,
            &mut data,
            if index == 0 { &words } else { &draw_words },
        );
        assert!(r.fault.is_none(), "{:?}", r.fault);
        assert_eq!(r.progress.draws_submitted, 1);
        d.publish(c, &mut mem(&mut data)).unwrap();
        check(&data, color, false);
        d.retire(r.token).unwrap();
        let n = d.graphics_counters();
        let now = [
            n.shader_decodes,
            n.shader_module_allocations,
            n.pipeline_allocations,
        ];
        if let Some(old) = identity {
            assert_eq!(now, old);
        }
        identity = Some(now);
    }
    let bytes: Vec<_> = [0.75f32, 0.25, 0.5, 1.]
        .into_iter()
        .flat_map(f32::to_le_bytes)
        .collect();
    d.write_backing(b, 0x50, &bytes, &mut mem(&mut data))
        .unwrap();
    let pending = submit(&mut d, c, b, &mut data, &draw_words);
    assert!(pending.fault.is_none(), "{:?}", pending.fault);
    assert_eq!(pending.progress.draws_submitted, 1);
    check(&data, [64, 128, 191, 255], false);
    let before = d.graphics_counters();
    let start = std::time::Instant::now();
    let mut last = pending.token;
    for _ in 0..32 {
        let r = submit_existing(&mut d, c, &mut data, 8);
        assert!(r.fault.is_none(), "{:?}", r.fault);
        assert_eq!(r.progress.draws_submitted, 1);
        last = r.token;
    }
    let elapsed = start.elapsed();
    let after = d.graphics_counters();
    assert_eq!(after.draw_commands - before.draw_commands, 32);
    assert_eq!(after.submissions - before.submissions, 32);
    assert_eq!(
        move_constant_resource_work(after),
        move_constant_resource_work(before)
    );
    assert!(!d.query(last).unwrap().guest_visible);
    check(&data, [64, 128, 191, 255], false);
    d.publish(c, &mut mem(&mut data)).unwrap();
    check(&data, [191, 64, 128, 255], false);
    assert!(d.query(pending.token).unwrap().guest_visible);
    assert!(d.query(last).unwrap().guest_visible);
    // Change only the binary offset: red now reads c3[0x14], the green word.
    let old = u64::from_le_bytes(bin[0x58..0x60].try_into().unwrap());
    assert_eq!(old, 0x4c98078c00470000);
    let changed = (old & !(0x1fffu64 << 20)) | (5u64 << 20);
    d.write_backing(b, 0x4658, &changed.to_le_bytes(), &mut mem(&mut data))
        .unwrap();
    let before_code = d.graphics_counters();
    let changed = submit(&mut d, c, b, &mut data, &draw_words);
    assert!(changed.fault.is_none(), "{:?}", changed.fault);
    assert_eq!(changed.progress.draws_submitted, 1);
    assert_eq!(
        d.graphics_counters().shader_decodes,
        before_code.shader_decodes + 1
    );
    check(&data, [191, 64, 128, 255], false);
    let accepted = d.graphics_counters();
    let mut invalid = Vec::new();
    method(&mut invalid, 0x2490, 0x30); // unbind actual referenced slot three
    invalid.extend(draw_words);
    let failed = submit(&mut d, c, b, &mut data, &invalid);
    assert!(failed.fault.is_some());
    assert_eq!(failed.progress.draws_submitted, 0);
    assert_eq!(d.graphics_counters().draw_commands, accepted.draw_commands);
    assert_eq!(d.graphics_counters().submissions, accepted.submissions);
    d.publish(c, &mut mem(&mut data)).unwrap();
    check(&data, [64, 64, 128, 255], false);
    assert!(d.query(changed.token).unwrap().guest_visible);
    assert_eq!(d.graphics_counters().draw_commands, accepted.draw_commands);
    eprintln!("MOVC color warm32 CPU={elapsed:?} before={before:?} after={after:?}");
    d.close(c, &mut mem(&mut data)).unwrap();
}

fn move_constant_resource_work(n: wl_gpu::Counters) -> [u64; 17] {
    [
        n.shader_binary_bytes,
        n.vertex_shader_bytes,
        n.shader_decodes,
        n.vertex_shader_decodes,
        n.descriptor_reads,
        n.vertex_guest_bytes,
        n.index_guest_bytes,
        n.constant_guest_bytes,
        n.guest_texture_bytes,
        n.render_target_guest_bytes,
        n.upload_bytes,
        n.upload_calls,
        n.shader_module_allocations,
        n.pipeline_allocations,
        n.render_target_allocations,
        n.readback_operations,
        n.synchronization_points,
    ]
}

fn check_move_constant_texture(data: &[u8], image: &[u8]) {
    // Independent f64 barycentric interpolation and nearest texel selection.
    let v = POS.map(|p| p.map(f64::from));
    let area = edge(v[0], v[1], v[2]);
    let mut covered = 0;
    let mut outside = 0;
    for y in 0..H {
        for x in 0..W {
            let p = [
                (x as f64 + 0.5) * 2. / 47. - 1.,
                1. - (y as f64 + 0.5) * 2. / 29.,
            ];
            let bary = [
                edge(v[1], v[2], p) / area,
                edge(v[2], v[0], p) / area,
                edge(v[0], v[1], p) / area,
            ];
            let actual = &data[RT + (y * W + x) * 4..RT + (y * W + x + 1) * 4];
            if x < 47 && bary.iter().any(|v| v.abs() < 0.005) {
                continue;
            }
            if x >= 47 || bary.iter().any(|v| *v < 0.) {
                assert_eq!(actual, BG, "outside{x},{y}");
                outside += 1;
                continue;
            }
            let u = (0..3).map(|n| bary[n] * f64::from(UV[n][0])).sum::<f64>() * 37.;
            let v = (0..3).map(|n| bary[n] * f64::from(UV[n][1])).sum::<f64>() * 19.;
            // Exclude only interpolation/texel-boundary ambiguity, never interior samples.
            if (u - u.round()).abs() < 0.02 || (v - v.round()).abs() < 0.02 {
                continue;
            }
            let at =
                (v.floor().clamp(0., 18.) as usize * 37 + u.floor().clamp(0., 36.) as usize) * 4;
            for channel in 0..4 {
                assert!(
                    actual[channel].abs_diff(image[at + channel]) <= 1,
                    "texture{x},{y}: {actual:?} != {:?}",
                    &image[at..at + 4]
                );
            }
            covered += 1;
        }
    }
    assert!(covered > 100);
    assert!(outside > 600);
}

#[test]
#[ignore = "requires actual GPU; MOV constant handle selects actual independent TIC/TSC through guest state"]
fn driver_move_constant_texture_handles_and_pending_visibility() {
    let (mut data, mut words, image0) = fixture();
    let (mut d, c, _, b, _) = driver();
    let bin = include_bytes!("fixtures/maxwell/tex-move-constant.bin");
    data[0x4a00..0x4a00 + bin.len()].copy_from_slice(bin);
    let mut image1 = image0.clone();
    for p in image1.chunks_exact_mut(4) {
        p[0] = 255 - p[0];
        p[1] ^= 0x5a;
        p[2] ^= 0xc3;
    }
    let tile = common::tile_mip(&image1, 37, 19, wl_gpu::format::Format::Rgba8Unorm, 2);
    assert!(tile.len() <= 0x4000);
    data[0x12000..0x12000 + tile.len()].copy_from_slice(&tile);
    let mut tic1 = TIC_RGBA;
    tic1[1] = 0xd0000; // existing mapping resolves to guest canonical offset12000
    for (at, descriptor) in [(0x100, TIC_RGBA), (0x120, tic1), (0x180, TSC_NEAREST)] {
        for (dst, raw) in data[at..at + 32].chunks_exact_mut(4).zip(descriptor) {
            dst.copy_from_slice(&raw.to_le_bytes());
        }
    }
    let end = words.split_off(words.len() - 8);
    for (m, v) in [
        (0x2144, 0xa00),
        (0x214c, 8),
        (0x1578, 0x30100),
        (0x157c, 1),
        (0x1560, 0x31180),
        (0x1564, 0),
        (0x2380, 32),
        (0x2384, 0),
        (0x2388, 0x40200),
        (0x2490, 1),
    ] {
        method(&mut words, m, v);
    }
    words.extend(end);
    let draw_words = draw();
    let mut identity = None;
    for (index, handle) in [1u32, 0, 1].into_iter().enumerate() {
        d.write_backing(b, 0x200, &handle.to_le_bytes(), &mut mem(&mut data))
            .unwrap();
        let r = submit(
            &mut d,
            c,
            b,
            &mut data,
            if index == 0 { &words } else { &draw_words },
        );
        assert!(r.fault.is_none(), "{:?}", r.fault);
        assert_eq!(r.progress.draws_submitted, 1);
        d.publish(c, &mut mem(&mut data)).unwrap();
        check_move_constant_texture(&data, if handle == 0 { &image0 } else { &image1 });
        d.retire(r.token).unwrap();
        let n = d.graphics_counters();
        let now = [
            n.shader_decodes,
            n.shader_module_allocations,
            n.pipeline_allocations,
        ];
        if let Some(old) = identity {
            assert_eq!(now, old);
        }
        identity = Some(now);
    }
    let mut image2 = image1.clone();
    for p in image2.chunks_exact_mut(4) {
        p[0] ^= 0x40;
        p[1] ^= 0x20;
    }
    let tile = common::tile_mip(&image2, 37, 19, wl_gpu::format::Format::Rgba8Unorm, 2);
    d.write_backing(b, 0x12000, &tile, &mut mem(&mut data))
        .unwrap();
    let before_data = d.graphics_counters();
    let pending = submit(&mut d, c, b, &mut data, &draw_words);
    assert!(pending.fault.is_none(), "{:?}", pending.fault);
    assert_eq!(pending.progress.draws_submitted, 1);
    assert_eq!(
        d.graphics_counters().shader_decodes,
        before_data.shader_decodes
    );
    assert_eq!(
        d.graphics_counters().pipeline_allocations,
        before_data.pipeline_allocations
    );
    assert!(d.graphics_counters().guest_texture_bytes > before_data.guest_texture_bytes);
    check_move_constant_texture(&data, &image1);
    let before = d.graphics_counters();
    let start = std::time::Instant::now();
    let mut last = pending.token;
    for _ in 0..32 {
        let r = submit_existing(&mut d, c, &mut data, 8);
        assert!(r.fault.is_none(), "{:?}", r.fault);
        assert_eq!(r.progress.draws_submitted, 1);
        last = r.token;
    }
    let elapsed = start.elapsed();
    let after = d.graphics_counters();
    assert_eq!(after.draw_commands - before.draw_commands, 32);
    assert_eq!(after.submissions - before.submissions, 32);
    assert_eq!(
        move_constant_resource_work(after),
        move_constant_resource_work(before)
    );
    assert!(!d.query(last).unwrap().guest_visible);
    check_move_constant_texture(&data, &image1);
    d.write_backing(b, 0x200, &2u32.to_le_bytes(), &mut mem(&mut data))
        .unwrap(); // TIC2 exceeds literal pool limit1
    let failed = submit(&mut d, c, b, &mut data, &draw_words);
    assert!(failed.fault.is_some());
    assert_eq!(failed.progress.draws_submitted, 0);
    assert_eq!(d.graphics_counters().draw_commands, after.draw_commands);
    assert_eq!(d.graphics_counters().submissions, after.submissions);
    check_move_constant_texture(&data, &image1);
    d.publish(c, &mut mem(&mut data)).unwrap();
    check_move_constant_texture(&data, &image2);
    assert!(d.query(pending.token).unwrap().guest_visible);
    assert!(d.query(last).unwrap().guest_visible);
    assert_eq!(d.graphics_counters().draw_commands, after.draw_commands);
    eprintln!("MOVC texture warm32 CPU={elapsed:?} before={before:?} after={after:?}");
    d.close(c, &mut mem(&mut data)).unwrap();
}
