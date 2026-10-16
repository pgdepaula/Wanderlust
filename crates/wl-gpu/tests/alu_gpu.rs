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
