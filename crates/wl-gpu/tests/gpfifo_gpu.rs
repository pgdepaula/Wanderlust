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
fn pitch_inline_upload(words: &mut Vec<u32>, address: u64, length: u32) {
    // Literal B197 byte methods 180,184,188,18c followed by LAUNCH_DMA.
    words.extend_from_slice(&[
        0x20040060,
        length,
        1,
        (address >> 32) as u32,
        address as u32,
        0x2001006c,
        0x1011,
    ]);
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
    mme_raster_state(MmeFixture::ArgumentMask);
}
#[test]
#[ignore = "requires actual GPU; MME reads guest-programmed scratch state"]
fn gpfifo_mme_scratch_read_changes_pixels_without_shader_recompilation() {
    mme_raster_state(MmeFixture::ScratchRead);
}
#[test]
#[ignore = "requires actual GPU; assembled MME register ALU controls color masks"]
fn gpfifo_mme_register_alu_changes_pixels_without_shader_recompilation() {
    mme_raster_state(MmeFixture::RegisterAlu);
}
#[test]
#[ignore = "requires actual GPU; resumed MME state follows real WFI completion"]
fn gpfifo_resumable_mme_state_and_maddrsend_control_real_pixels() {
    mme_raster_state(MmeFixture::StreamingState);
}
#[test]
#[ignore = "requires actual GPU; MME arguments cross GPFIFO entries without default values"]
fn gpfifo_mme_parameters_and_incremented_scratch_writes_control_pixels() {
    mme_raster_state(MmeFixture::ParameterState);
}
#[test]
#[ignore = "requires actual GPU; assembled MME branches choose the raster mask"]
fn gpfifo_mme_conditional_branches_and_backward_loop_control_real_pixels() {
    mme_raster_state(MmeFixture::Branches);
}
#[test]
#[ignore = "requires actual GPU; bounded privileged initialization follows real WFI"]
fn gpfifo_mme_privileged_initialization_acknowledges_only_completed_context_writes() {
    mme_raster_state(MmeFixture::Privileged);
}
#[test]
#[ignore = "requires actual GPU; independent MME bitfield program controls raster state"]
fn gpfifo_mme_bitfield_replace_and_extract_shifts_control_real_pixels() {
    mme_raster_state(MmeFixture::Bitfields);
}
#[test]
#[ignore = "requires actual GPU; MME hierarchy hints must not enable fragment stencil tests"]
fn gpfifo_mme_zcull_criterion_preserves_real_pixels_and_shader_reuse() {
    mme_raster_state(MmeFixture::ZCullCriterion);
}
#[test]
#[ignore = "requires actual GPU; only MME binds the constant buffer used by LDC/TEX"]
fn gpfifo_mme_constant_selectors_bind_real_fragment_resources() {
    mme_raster_state(MmeFixture::CbSelectors);
}
enum MmeFixture {
    ArgumentMask,
    ScratchRead,
    RegisterAlu,
    StreamingState,
    ParameterState,
    Branches,
    Privileged,
    Bitfields,
    ZCullCriterion,
    CbSelectors,
}
fn mme_raster_state(program: MmeFixture) {
    let scratch = matches!(program, MmeFixture::ScratchRead);
    let alu = matches!(program, MmeFixture::RegisterAlu);
    let streaming = matches!(program, MmeFixture::StreamingState);
    let parameters = matches!(program, MmeFixture::ParameterState);
    let branches = matches!(program, MmeFixture::Branches);
    let privileged = matches!(program, MmeFixture::Privileged);
    let bitfields = matches!(program, MmeFixture::Bitfields);
    let zcull_criterion = matches!(program, MmeFixture::ZCullCriterion);
    let cb_selectors = matches!(program, MmeFixture::CbSelectors);
    let (mut data, map, mut words, image) = fixture();
    words.splice(0..0, [0x2001c002, 0x110e]); // Channel NOP on unbound subchannel six.
    let draw = words.split_off(words.len() - 8);
    // Independent literal NV902D binding on subchannel three and configuration.
    // No 2D launch is submitted; the indexed B197 draw below remains unchanged.
    words.extend_from_slice(&[
        0x20016000, 0x902d, 0x200160ab, 3, 0x200160a4, 0, 0x20016221, 0x3f,
    ]);
    method(&mut words, 0x164c, 0x1000);
    method(&mut words, 0x2608, 2);
    method(&mut words, 0x1220, 5);
    method(&mut words, 0x1450, 0x00800008);
    // Literal contiguous SET_ZCULL / SET_ZCULL_BOUNDS packet. Neither enables
    // a fragment depth/stencil test or fabricates hierarchical metadata.
    words.extend_from_slice(&[0x2002065a, 0x11, 0]);
    method(&mut words, 0x1590, 0);
    // NVIDIA's two full-u32 fields, with deko3d's exact initialization pair.
    words.extend_from_slice(&[0x20020054, 0x49000000, 0x49000001]);
    // In-bounds geometry must not dereference an unused, unmapped runout VA.
    words.extend_from_slice(&[0x200203e1, 0xa5, 0x12345678]);

    for (method_id, value) in [
        (0xf8c, 0),
        (0x1594, 1),
        (0x310, 0x503),
        (0x77c, 0x01000000),
        (0x1130, 0x13),
        (0x374, 0),
        (0x15cc, 0x10000),
    ] {
        method(&mut words, method_id, value);
    }

    for (method_id, value) in [
        (0x151c, 1),
        (0x020c, 1),
        (0x030c, 0),
        (0x0300, 3),
        (0x0fdc, 1),
        (0x19c0, 1),
    ] {
        method(&mut words, method_id, value);
    }

    for method_id in wl_gpu::maxwell::CACHE_POLICY_METHODS {
        method(&mut words, u32::from(method_id), 0x10);
    }
    for (method_id, value) in [
        (0x16a8, 0x00030003),
        (0x1794, 0x00020002),
        (0x1234, 0),
        (0x1528, 0),
        (0x0de8, 1), // Global restart is explicitly disabled by fixture setup.
        (0x165c, 0),
        (0x1684, 1),
        (0x0d64, 8),
        (0x1140, 0x10),
        (0x1610, 0x0e),
    ] {
        method(&mut words, method_id, value);
    }
    for slot in 1..16 {
        method(&mut words, 0x1880 + slot * 4, 1);
    }
    method(&mut words, 0x1534, 0); // Documented antialias disable, single sample.
    method(&mut words, 0x153c, 0); // No alpha-to-coverage or alpha-to-one.
    method(&mut words, 0x15b4, 0); // Nouveau's legacy CSAA disable.
    for index in 0..8 {
        method(&mut words, 0xd00 + index * 8, 0);
        method(&mut words, 0xd04 + index * 8, 0);
    }
    method(&mut words, 0x194c, 0);
    method(&mut words, 0x1950, 2); // CLIPALL is ignored only because disabled.
    method(&mut words, 0x10f8, 0x101); // Configure only; no clear is requested.
    for (method_id, value) in [
        (0x10cc, 4),
        (0x10e0, 0xff),
        (0x10e4, 0xff),
        (0x10e8, 0x3f),
        (0x10ec, 0xff),
        (0x10f0, 4),
        (0x074c, 0x3f),
        (0x0d9c, 0),
    ] {
        method(&mut words, method_id, value);
    }
    for slot in 1..16 {
        // Deliberately empty INACTIVE scissors must not affect viewport zero.
        method(&mut words, 0xe00 + slot * 16, 1);
        method(&mut words, 0xe04 + slot * 16, 0);
        method(&mut words, 0xe08 + slot * 16, 0);
    }
    for (method_id, value) in [(0x11c, 0), (0x120, 0), (0x114, 0)] {
        method(&mut words, method_id, value);
    }
    // Independent envyas: maddr 0x680; read $r2 0xd1a;
    // exit send (add $r2 0); nop;
    let code = if cb_selectors {
        include_str!("fixtures/mme/cb-selectors.words")
            .split_whitespace()
            .map(|w| u32::from_str_radix(w, 16).unwrap())
            .collect()
    } else if zcull_criterion {
        // envyas: maddr 0x376; send (add $r0 5); maddr 0x680;
        // exit send (add $r1 0); nop;
        vec![0x00dd8021, 0x14041, 0x01a00021, 0x8c0, 0x11]
    } else if bitfields {
        include_str!("fixtures/mme/bitfield-state.words")
            .split_whitespace()
            .map(|w| u32::from_str_radix(w, 16).unwrap())
            .collect()
    } else if privileged {
        include_str!("fixtures/mme/privileged-state.words")
            .split_whitespace()
            .map(|w| u32::from_str_radix(w, 16).unwrap())
            .collect()
    } else if branches {
        include_str!("fixtures/mme/branch-state.words")
            .split_whitespace()
            .map(|w| u32::from_str_radix(w, 16).unwrap())
            .collect()
    } else if parameters {
        include_str!("fixtures/mme/parameter-state.words")
            .split_whitespace()
            .map(|w| u32::from_str_radix(w, 16).unwrap())
            .collect()
    } else if streaming {
        include_str!("fixtures/mme/streaming-state.words")
            .split_whitespace()
            .map(|w| u32::from_str_radix(w, 16).unwrap())
            .collect()
    } else if scratch {
        vec![0x01a00021, 0x03468215, 0x10c0, 0x11]
    } else if alu {
        include_str!("fixtures/mme/register-alu.words")
            .split_whitespace()
            .map(|word| u32::from_str_radix(word, 16).unwrap())
            .collect()
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
    if cb_selectors {
        method(&mut words, 0x2490, 0); // LDC must fail unless the macro really binds it.
    }
    let macro_start = words.len();
    if cb_selectors {
        words.extend_from_slice(&[0xa0040e00, 32, 0, 0x40000, 1]);
    } else if privileged {
        words.extend_from_slice(&[0xa0040e00, 0x418800, 1, 1, 1]);
    } else if parameters {
        // Literal increment-once packet: launch, then four DATA parameters.
        words.extend_from_slice(&[0xa0050e00, 0, 0x89abcdef, 0x01234567, 1, 0xfedcba98]);
    } else {
        method(&mut words, 0x3800, 1); // Red-only writes, produced by guest MME.
    }
    words.extend_from_slice(&draw);
    let entries = if parameters || privileged || cb_selectors {
        // Stop immediately after the launch argument. DATA is fetched from the
        // next entry through a GPU VA alias of the same canonical backing.
        let cut = macro_start + 2;
        [
            entry(0xb0000, cut),
            entry(0xc0000 + cut as u64 * 4, words.len() - cut),
        ]
        .concat()
    } else {
        entry(0xb0000, words.len()).to_vec()
    };
    put(&mut data, 0x10000, &words);
    let mut renderer = Renderer::new().unwrap();
    let mut channel = Channel::default();
    let mut changes = TextureChanges::new(1);
    let first = submit(
        &mut renderer,
        &mut channel,
        &entries,
        &map,
        &mut data,
        &mut changes,
    )
    .unwrap();
    assert_eq!(first.draws_submitted, 1);
    assert_eq!(
        first.entries_completed,
        if parameters || privileged || cb_selectors {
            2
        } else {
            1
        }
    );
    let frame = sync(&mut renderer, &mut data, &mut changes);
    let mut masked = image.clone();
    for pixel in masked.chunks_exact_mut(4) {
        pixel[1..].copy_from_slice(&BG[1..]);
    }
    check(&frame.rgba, &masked, None, BG);
    let before = renderer.counters;
    let mut second = Vec::new();
    // TEX.B keeps using its LDC-derived handle even when this automatic-handle
    // selector names an unbound slot. No hidden descriptor/default is supplied.
    method(&mut second, 0x2608, 31);

    if scratch {
        method(&mut second, 0x3468, 0x1111);
    }
    if cb_selectors {
        method(&mut second, 0x2490, 0);
        second.extend_from_slice(&[0xa0040e00, 32, 0, 0x40000, 0x1111]);
    } else if privileged {
        second.extend_from_slice(&[0xa0040e00, 0x418800, 1, 1, 0x1111]);
    } else if parameters {
        second.extend_from_slice(&[0xa0050e00, 0, 0x01234567, 0x89abcdef, 0x1111, 0x76543210]);
    } else {
        method(&mut second, 0x3800, 0x1111); // Same uploaded code, different input.
    }
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
    if streaming || parameters || privileged {
        assert_eq!(renderer.counters.synchronization_points, 3);
        assert_eq!(renderer.counters.readback_operations, 2);
    }
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

#[test]
#[ignore = "requires actual GPU; guest inline constants feed translated LDC"]
fn gpfifo_inline_constants_reach_ldc_pixels_and_reuse_shader() {
    let (mut data, map, mut words, image) = fixture();
    let draw = words.split_off(words.len() - 8);
    let binary = include_bytes!("fixtures/maxwell/ldc.bin");
    data[0x4600..0x4600 + binary.len()].copy_from_slice(binary);
    method(&mut words, 0x2144, 0x600);
    for (m, v) in [(0x2380, 32), (0x2384, 0), (0x2388, 0x40000), (0x238c, 16)] {
        method(&mut words, m, v);
    }
    words.push(0x200408e4); // Incrementing four LOAD_CONSTANT_BUFFER ports.
    words.extend([0f32, 0., 1., 1.].map(f32::to_bits));
    words.extend_from_slice(&draw);
    put(&mut data, 0x10000, &words);
    let mut renderer = Renderer::new().unwrap();
    let mut channel = Channel::default();
    let mut changes = TextureChanges::new(1);
    submit(
        &mut renderer,
        &mut channel,
        &entry(0xb0000, words.len()),
        &map,
        &mut data,
        &mut changes,
    )
    .unwrap();
    check(
        &sync(&mut renderer, &mut data, &mut changes).rgba,
        &image,
        Some([0, 0, 255, 255]),
        BG,
    );
    let before = renderer.counters;
    let mut second = Vec::new();
    method(&mut second, 0x238c, 16);
    method(&mut second, 0x0d9c, 0);
    method(&mut second, 0x10cc, 0x00ff00ff);
    method(&mut second, 0x10e8, 0x003f003f);
    method(&mut second, 0x10f8, 0x1111); // Clear configuration cannot alter a draw.
    for (method_id, value) in [(0x1684, 0), (0x1140, 0), (0x1610, 0x0e), (0x0d64, 0)] {
        method(&mut second, method_id, value);
    }
    for method_id in wl_gpu::maxwell::CACHE_POLICY_METHODS {
        method(&mut second, u32::from(method_id), 0x20);
    }
    second.push(0x600408e4); // Same port, four data words.
    let color = [0.25f32, 0.5, 0.75, 1.];
    second.extend(color.map(f32::to_bits));
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
        Some([64, 128, 191, 255]),
        BG,
    );
    assert_eq!(renderer.counters.constant_buffer_write_bytes, 32);
    assert_eq!(renderer.counters.shader_decodes, before.shader_decodes);
    assert_eq!(
        renderer.counters.shader_module_allocations,
        before.shader_module_allocations
    );
    assert_eq!(
        renderer.counters.pipeline_allocations,
        before.pipeline_allocations
    );
    for (bytes, value) in data[80..96].chunks_exact(4).zip(color) {
        assert_eq!(bytes, value.to_le_bytes());
    }
    println!(
        "inline-LDC: canonical_write_bytes={},constant_guest_bytes={},uniform_upload_bytes={},shader_decodes={},pipelines={},draws={},readbacks={}",
        renderer.counters.constant_buffer_write_bytes,
        renderer.counters.constant_guest_bytes,
        renderer.counters.constant_upload_bytes,
        renderer.counters.shader_decodes,
        renderer.counters.pipeline_allocations,
        renderer.counters.draw_commands,
        renderer.counters.readback_operations
    );
}

#[test]
#[ignore = "requires GPU renderer; inline write replaces unread aliased command"]
fn gpfifo_inline_write_reacquires_its_modified_command_suffix() {
    modified_inline_suffix(false);
}
#[test]
#[ignore = "requires GPU renderer; B197 DMA word replaces unread aliased command"]
fn gpfifo_inline_dma_reacquires_its_modified_command_suffix() {
    modified_inline_suffix(true);
}
fn modified_inline_suffix(inline: bool) {
    let (mut data, map, _, _) = fixture();
    let mut words = Vec::new();
    let offset = if inline { 44 } else { 48 };
    if inline {
        method(&mut words, 0, 0xb197);
        pitch_inline_upload(&mut words, 0xc0000 + offset, 4);
        method(&mut words, 0x1b4, 0);
    } else {
        for (m, v) in [
            (0, 0xb197),
            (0x2380, 128),
            (0x2384, 0),
            (0x2388, 0xc0000),
            (0x238c, 48),
            (0x2390, 0),
        ] {
            method(&mut words, m, v);
        }
        assert_eq!(words.len(), 12);
    }
    words.push(0xe0000000); // Invalid until replaced with exact NOP.
    put(&mut data, 0x10000, &words);
    let mut renderer = Renderer::new().unwrap();
    let mut channel = Channel::default();
    let mut changes = TextureChanges::new(1);
    let progress = submit(
        &mut renderer,
        &mut channel,
        &entry(0xb0000, words.len()),
        &map,
        &mut data,
        &mut changes,
    )
    .unwrap();
    assert_eq!(
        (
            progress.words_consumed,
            progress.entries_completed,
            progress.draws_submitted
        ),
        (if inline { 12 } else { 13 }, 1, 0)
    );
    assert_eq!(
        &data[0x10000 + offset as usize..0x10004 + offset as usize],
        &[0; 4]
    );
    assert_eq!(
        renderer.counters.gpfifo_read_calls,
        if inline { 3 } else { 2 }
    );
    assert_eq!(
        if inline {
            renderer.counters.inline_upload_write_bytes
        } else {
            renderer.counters.constant_buffer_write_bytes
        },
        4
    );
    assert_eq!(renderer.counters.readback_operations, 0);
}

#[test]
#[ignore = "requires actual GPU; constant write aliases pending color output"]
fn gpfifo_inline_constant_write_publishes_prior_alias_before_writing() {
    inline_alias_publication(false);
}
#[test]
#[ignore = "requires actual GPU; B197 DMA aliases pending color output"]
fn gpfifo_inline_dma_publishes_prior_alias_before_writing() {
    inline_alias_publication(true);
}
fn inline_alias_publication(inline: bool) {
    let (mut data, map, words, image) = fixture();
    put(&mut data, 0x10000, &words);
    let mut renderer = Renderer::new().unwrap();
    let mut channel = Channel::default();
    let mut changes = TextureChanges::new(1);
    submit(
        &mut renderer,
        &mut channel,
        &entry(0xb0000, words.len()),
        &map,
        &mut data,
        &mut changes,
    )
    .unwrap();
    assert_eq!(renderer.counters.readback_operations, 0);
    let patch = [243, 177, 29, 255];
    let mut upload = Vec::new();
    if inline {
        pitch_inline_upload(&mut upload, 0x90000, 4);
        method(&mut upload, 0x1b4, u32::from_le_bytes(patch));
    } else {
        for (m, v) in [
            (0x2380, 4),
            (0x2384, 0),
            (0x2388, 0x90000),
            (0x238c, 0),
            (0x2390, u32::from_le_bytes(patch)),
        ] {
            method(&mut upload, m, v);
        }
    }
    put(&mut data, 0x12000, &upload);
    submit(
        &mut renderer,
        &mut channel,
        &entry(0xb2000, upload.len()),
        &map,
        &mut data,
        &mut changes,
    )
    .unwrap();
    assert_eq!(renderer.counters.readback_operations, 1);
    assert_eq!(renderer.counters.coherence_barriers, 1);
    assert_eq!(
        if inline {
            renderer.counters.inline_upload_write_bytes
        } else {
            renderer.counters.constant_buffer_write_bytes
        },
        4
    );
    assert_eq!(&data[RT..RT + 4], &patch);
    let mut rest = data[RT..RT + SIZE].to_vec();
    rest[..4].copy_from_slice(&BG);
    check(&rest, &image, None, BG); // All other pixels use the independent triangle oracle.
}

#[test]
#[ignore = "requires actual GPU; failed inline write preserves accepted draw"]
fn failed_inline_write_does_not_mark_journal_or_replay_prior_draw() {
    failed_inline_publication(false);
}
#[test]
#[ignore = "requires actual GPU; B197 DMA atomic write failure preserves earlier draw"]
fn failed_inline_dma_does_not_mark_journal_or_replay_prior_draw() {
    failed_inline_publication(true);
}
fn failed_inline_publication(inline: bool) {
    struct FailsWrite<'a>(&'a [u8]);
    impl GuestMemory for FailsWrite<'_> {
        fn read(&self, address: GuestAddress, length: usize) -> wl_gpu::Result<&[u8]> {
            let start = usize::try_from(address.0).map_err(|_| wl_gpu::Error("address".into()))?;
            self.0
                .get(
                    start
                        ..start
                            .checked_add(length)
                            .ok_or_else(|| wl_gpu::Error("overflow".into()))?,
                )
                .ok_or_else(|| wl_gpu::Error("range".into()))
        }
    }
    impl GuestMemoryWrite for FailsWrite<'_> {
        fn validate_write(&self, address: GuestAddress, length: usize) -> wl_gpu::Result<()> {
            self.read(address, length).map(|_| ())
        }
        fn write(&mut self, _address: GuestAddress, _bytes: &[u8]) -> wl_gpu::Result<()> {
            Err(wl_gpu::Error("injected atomic write failure".into()))
        }
    }
    let (mut data, map, words, image) = fixture();
    put(&mut data, 0x10000, &words);
    let mut renderer = Renderer::new().unwrap();
    let mut channel = Channel::default();
    let mut changes = TextureChanges::new(1);
    submit(
        &mut renderer,
        &mut channel,
        &entry(0xb0000, words.len()),
        &map,
        &mut data,
        &mut changes,
    )
    .unwrap();
    let mut upload = Vec::new();
    if inline {
        pitch_inline_upload(&mut upload, 0x40000, 4);
        method(&mut upload, 0x1b4, 0x12345678);
    } else {
        method(&mut upload, 0x238c, 0);
        method(&mut upload, 0x2390, 0x12345678);
    }
    put(&mut data, 0x12000, &upload);
    let stamp = changes.stamp(64..68);
    let bytes = data[64..68].to_vec();
    let before = renderer.counters;
    let failure = renderer
        .submit_gpfifo_deferred(
            &mut channel,
            &entry(0xb2000, upload.len()),
            &map,
            &mut FailsWrite(&data),
            &mut changes,
        )
        .unwrap_err();
    assert_eq!(
        (
            failure.progress.words_consumed,
            failure.progress.draws_submitted
        ),
        (if inline { 9 } else { 4 }, 0)
    );
    assert!(channel.is_faulted());
    assert_eq!(&data[64..68], bytes);
    assert_eq!(changes.stamp(64..68), stamp);
    assert_eq!(renderer.counters.constant_buffer_write_bytes, 0);
    assert_eq!(renderer.counters.inline_upload_write_bytes, 0);
    assert_eq!(
        renderer.counters.readback_operations,
        before.readback_operations
    );
    check(
        &sync(&mut renderer, &mut data, &mut changes).rgba,
        &image,
        None,
        BG,
    );
    assert_eq!(renderer.counters.draw_commands, before.draw_commands);
    assert!(
        submit(
            &mut renderer,
            &mut channel,
            &entry(0xb2000, upload.len()),
            &map,
            &mut data,
            &mut changes
        )
        .is_err()
    );
    assert_eq!(renderer.counters.draw_commands, before.draw_commands);
}

#[test]
#[ignore = "requires actual GPU; nonindexed restart policy requires global disable"]
fn nonindexed_array_restart_policy_uses_guest_vertices_without_index_fetch() {
    for explicit_disable in [false, true] {
        let (mut data, map, mut words, image) = fixture();
        words.truncate(words.len() - 8); // Remove indexed BEGIN/FIRST/COUNT/END.
        if !explicit_disable {
            let positions: Vec<_> = words
                .iter()
                .enumerate()
                .filter(|(_, w)| **w == 0x20010591)
                .map(|(i, _)| i)
                .collect();
            assert_eq!(positions.len(), 1);
            let i = positions[0];
            assert_eq!(words[i + 1], 0);
            words[i] = 0;
            words[i + 1] = 0; // No global restart state is programmed.
        }
        data[0x6000..0x6006].fill(0xff); // Invalid indices must not be consulted.
        method(&mut words, 0xde8, 1);
        method(&mut words, 0x1214, 0x40030000);
        put(&mut data, 0x10000, &words);
        let mut renderer = Renderer::new().unwrap();
        let mut channel = Channel::default();
        let mut changes = TextureChanges::new(1);
        let result = submit(
            &mut renderer,
            &mut channel,
            &entry(0xb0000, words.len()),
            &map,
            &mut data,
            &mut changes,
        );
        if explicit_disable {
            assert_eq!(result.unwrap().draws_submitted, 1);
            check(
                &sync(&mut renderer, &mut data, &mut changes).rgba,
                &image,
                None,
                BG,
            );
        } else {
            assert!(result.is_err());
            assert!(channel.is_faulted());
            assert_eq!(renderer.counters.draw_commands, 0);
            assert!(data[RT..RT + SIZE].chunks_exact(4).all(|p| p == BG));
        }
        assert_eq!(renderer.counters.index_upload_bytes, 0);
    }
}

#[test]
#[ignore = "requires actual GPU; WFI completion must not publish guest output"]
fn gpfifo_b197_wait_for_idle_completes_work_without_readback_or_publication() {
    let (mut data, map, mut words, image) = fixture();
    // A wait before the first guest draw has no submitted prefix to wait for.
    let draw = words.split_off(words.len() - 8);
    words.extend_from_slice(&[0x20010044, 0]);
    words.extend_from_slice(&draw);
    put(&mut data, 0x10000, &words);
    let mut renderer = Renderer::new().unwrap();
    let mut channel = Channel::default();
    let mut changes = TextureChanges::new(1);
    submit(
        &mut renderer,
        &mut channel,
        &entry(0xb0000, words.len()),
        &map,
        &mut data,
        &mut changes,
    )
    .unwrap();
    assert_eq!(renderer.counters.synchronization_points, 0);
    let before = renderer.counters;
    assert!(data[RT..RT + SIZE].chunks_exact(4).all(|p| p == BG));
    let wait = [0x20010044, 0];
    put(&mut data, 0x10000, &wait);
    let progress = submit(
        &mut renderer,
        &mut channel,
        &entry(0xb0000, wait.len()),
        &map,
        &mut data,
        &mut changes,
    )
    .unwrap();
    assert_eq!(progress.draws_submitted, 0);
    assert_eq!(progress.words_consumed, 2);
    assert_eq!(renderer.counters.submissions, before.submissions);
    assert_eq!(
        renderer.counters.synchronization_points,
        before.synchronization_points + 1
    );
    assert_eq!(
        renderer.counters.readback_operations,
        before.readback_operations
    );
    assert_eq!(
        renderer.counters.guest_framebuffer_write_bytes,
        before.guest_framebuffer_write_bytes
    );
    assert!(data[RT..RT + SIZE].chunks_exact(4).all(|p| p == BG));
    // Invalid WFI data faults the channel without replaying or discarding the
    // earlier draw. Its pending output remains explicitly publishable.
    put(&mut data, 0x10000, &[0x20010044, 1]);
    assert!(
        submit(
            &mut renderer,
            &mut channel,
            &entry(0xb0000, 2),
            &map,
            &mut data,
            &mut changes
        )
        .is_err()
    );
    assert_eq!(renderer.counters.draw_commands, before.draw_commands);
    assert_eq!(renderer.counters.readback_operations, 0);
    check(
        &sync(&mut renderer, &mut data, &mut changes).rgba,
        &image,
        None,
        BG,
    );
    assert_eq!(renderer.counters.draw_commands, before.draw_commands);
}

#[test]
#[ignore = "requires actual GPU; synchronous frame survives a trailing WFI"]
fn direct_synchronous_draw_retains_frame_after_wait_for_idle() {
    let (mut data, map, mut words, image) = fixture();
    words.extend_from_slice(&[0x20010044, 0]);
    let mut renderer = Renderer::new().unwrap();
    let mut channel = Channel::default();
    let mut changes = TextureChanges::new(1);
    let frame = renderer
        .submit_pushbuffer(
            &mut channel,
            &words,
            &map,
            &mut SliceMemoryMut {
                base: GuestAddress(0),
                bytes: &mut data,
            },
            &mut changes,
        )
        .unwrap()
        .unwrap();
    check(&frame.rgba, &image, None, BG);
    assert_eq!(renderer.counters.draw_commands, 1);
    assert_eq!(renderer.counters.readback_operations, 1);
    assert_eq!(renderer.counters.synchronization_points, 2);
}

#[test]
#[ignore = "requires actual GPU; macro fault must preserve completed prior draw"]
fn gpfifo_resumed_macro_fault_preserves_prior_draw_without_replay() {
    let (mut data, map, words, image) = fixture();
    put(&mut data, 0x10000, &words);
    let mut renderer = Renderer::new().unwrap();
    let mut channel = Channel::default();
    let mut changes = TextureChanges::new(1);
    submit(
        &mut renderer,
        &mut channel,
        &entry(0xb0000, words.len()),
        &map,
        &mut data,
        &mut changes,
    )
    .unwrap();
    let before = renderer.counters;
    let mut commands = Vec::new();
    for (m, v) in [(0x11c, 0), (0x120, 0), (0x114, 0)] {
        method(&mut commands, m, v);
    }
    let mut code: Vec<u32> = include_str!("fixtures/mme/streaming-state.words")
        .split_whitespace()
        .map(|w| u32::from_str_radix(w, 16).unwrap())
        .collect();
    code[3] = 0x00118021; // envyas maddr 0x46 -> forbidden running-code upload.
    for word in code {
        method(&mut commands, 0x118, word);
    }
    method(&mut commands, 0x3800, 0x40030000);
    put(&mut data, 0x10000, &commands);
    let error = submit(
        &mut renderer,
        &mut channel,
        &entry(0xb0000, commands.len()),
        &map,
        &mut data,
        &mut changes,
    )
    .unwrap_err();
    assert!(error.cause.to_string().contains("UnsupportedOutput"));
    assert_eq!(renderer.counters.draw_commands, before.draw_commands);
    assert_eq!(
        renderer.counters.synchronization_points,
        before.synchronization_points + 1
    );
    assert_eq!(renderer.counters.readback_operations, 0);
    assert!(data[RT..RT + SIZE].chunks_exact(4).all(|p| p == BG));
    assert!(
        submit(
            &mut renderer,
            &mut channel,
            &entry(0xb0000, commands.len()),
            &map,
            &mut data,
            &mut changes
        )
        .is_err()
    );
    assert_eq!(
        renderer.counters.synchronization_points,
        before.synchronization_points + 1
    );
    check(
        &sync(&mut renderer, &mut data, &mut changes).rgba,
        &image,
        None,
        BG,
    );
    assert_eq!(renderer.counters.draw_commands, before.draw_commands);
}

#[test]
#[ignore = "requires actual GPU; missing/wrong/unmapped macro arguments preserve prior work"]
fn gpfifo_macro_argument_faults_keep_completed_draws_and_do_not_default_to_zero() {
    for failure in 0..4 {
        let (mut data, map, words, image) = fixture();
        put(&mut data, 0x10000, &words);
        let mut renderer = Renderer::new().unwrap();
        let mut channel = Channel::default();
        let mut changes = TextureChanges::new(1);
        submit(
            &mut renderer,
            &mut channel,
            &entry(0xb0000, words.len()),
            &map,
            &mut data,
            &mut changes,
        )
        .unwrap();
        let before = renderer.counters;
        let mut commands = Vec::new();
        for (m, v) in [(0x11c, 0), (0x120, 0), (0x114, 0)] {
            method(&mut commands, m, v);
        }
        for word in include_str!("fixtures/mme/parameter-state.words").split_whitespace() {
            method(&mut commands, 0x118, u32::from_str_radix(word, 16).unwrap());
        }
        method(&mut commands, 0x3800, 0);
        if failure == 3 {
            for value in [0x89abcdef, 0x01234567, 1] {
                method(&mut commands, 0x3804, value);
            }
            // Mode 3 has emitted the mask, but its final argument is absent.
        }
        if failure == 1 {
            method(&mut commands, 0x380c, 1);
        } // Wrong macro DATA port.
        put(&mut data, 0x10000, &commands);
        let mut entries = entry(0xb0000, commands.len()).to_vec();
        if failure == 2 {
            entries.extend_from_slice(&entry(0xe0000, 1));
        }
        let error = submit(
            &mut renderer,
            &mut channel,
            &entries,
            &map,
            &mut data,
            &mut changes,
        )
        .unwrap_err();
        assert_eq!(
            error.phase,
            if failure == 2 {
                FaultPhase::Fetch
            } else {
                FaultPhase::Execute
            }
        );
        assert_eq!(error.progress.draws_submitted, 0);
        if failure == 0 {
            assert!(error.cause.to_string().contains("parameter continuation"));
        }
        assert_eq!(renderer.counters.draw_commands, before.draw_commands);
        assert_eq!(
            renderer.counters.synchronization_points,
            before.synchronization_points + 1
        );
        assert_eq!(renderer.counters.readback_operations, 0);
        assert!(data[RT..RT + SIZE].chunks_exact(4).all(|p| p == BG));
        assert!(
            submit(
                &mut renderer,
                &mut channel,
                &entries,
                &map,
                &mut data,
                &mut changes
            )
            .is_err()
        );
        assert_eq!(
            renderer.counters.synchronization_points,
            before.synchronization_points + 1
        );
        check(
            &sync(&mut renderer, &mut data, &mut changes).rgba,
            &image,
            None,
            BG,
        );
        assert_eq!(renderer.counters.draw_commands, before.draw_commands);
    }
}

#[test]
#[ignore = "requires actual GPU; bounded macro loops preserve pending draws on fault"]
fn gpfifo_macro_branch_faults_preserve_prior_output_without_replay_or_publication() {
    for tail in [&[0x27][..], &[0x8007, 0x27][..]] {
        let (mut data, map, words, image) = fixture();
        put(&mut data, 0x10000, &words);
        let mut renderer = Renderer::new().unwrap();
        let mut channel = Channel::default();
        let mut changes = TextureChanges::new(1);
        submit(
            &mut renderer,
            &mut channel,
            &entry(0xb0000, words.len()),
            &map,
            &mut data,
            &mut changes,
        )
        .unwrap();
        let before = renderer.counters;
        let mut commands = Vec::new();
        for (m, v) in [(0x11c, 0), (0x120, 0), (0x114, 0)] {
            method(&mut commands, m, v);
        }
        // Independently assembled WFI, then either a self-loop or an invalid
        // branch in a branch delay slot. Neither can replay the earlier draw.
        for &word in [0x00110071].iter().chain(tail) {
            method(&mut commands, 0x118, word);
        }
        method(&mut commands, 0x3800, 0);
        put(&mut data, 0x10000, &commands);
        let entries = entry(0xb0000, commands.len());
        let error = submit(
            &mut renderer,
            &mut channel,
            &entries,
            &map,
            &mut data,
            &mut changes,
        )
        .unwrap_err();
        assert_eq!(error.phase, FaultPhase::Execute);
        assert!(error.cause.to_string().contains(if tail.len() == 1 {
            "MissingExit"
        } else {
            "UnsupportedInstruction"
        }));
        assert_eq!(renderer.counters.draw_commands, before.draw_commands);
        assert_eq!(
            renderer.counters.synchronization_points,
            before.synchronization_points + 1
        );
        assert_eq!(renderer.counters.readback_operations, 0);
        assert!(data[RT..RT + SIZE].chunks_exact(4).all(|p| p == BG));
        assert!(
            submit(
                &mut renderer,
                &mut channel,
                &entries,
                &map,
                &mut data,
                &mut changes
            )
            .is_err()
        );
        assert_eq!(
            renderer.counters.synchronization_points,
            before.synchronization_points + 1
        );
        check(
            &sync(&mut renderer, &mut data, &mut changes).rgba,
            &image,
            None,
            BG,
        );
        assert_eq!(renderer.counters.draw_commands, before.draw_commands);
    }
}

#[test]
#[ignore = "requires actual GPU; unknown privileged writes must not acknowledge or replay"]
fn gpfifo_privileged_request_failures_preserve_pending_output_and_fault_the_channel() {
    for failure in 0..4 {
        let (mut data, map, words, image) = fixture();
        put(&mut data, 0x10000, &words);
        let mut renderer = Renderer::new().unwrap();
        let mut channel = Channel::default();
        let mut changes = TextureChanges::new(1);
        submit(
            &mut renderer,
            &mut channel,
            &entry(0xb0000, words.len()),
            &map,
            &mut data,
            &mut changes,
        )
        .unwrap();
        let before = renderer.counters;
        let mut commands = Vec::new();
        for (m, v) in [(0x11c, 0), (0x120, 0), (0x114, 0)] {
            method(&mut commands, m, v);
        }
        let mut code: Vec<u32> = include_str!("fixtures/mme/privileged-state.words")
            .split_whitespace()
            .map(|w| u32::from_str_radix(w, 16).unwrap())
            .collect();
        if failure == 0 {
            code[0] = 0x11;
        } // Omit the required WFI.
        if failure == 3 {
            code[2] = 0x4041;
        } // Request flag must be zero.
        for word in code {
            method(&mut commands, 0x118, word);
        }
        commands.extend_from_slice(&[
            0xa0040e00,
            if failure == 1 { 0x418804 } else { 0x418800 },
            1,
            if failure == 2 { 0 } else { 1 },
            0x1111,
        ]);
        put(&mut data, 0x10000, &commands);
        let entries = entry(0xb0000, commands.len());
        let error = submit(
            &mut renderer,
            &mut channel,
            &entries,
            &map,
            &mut data,
            &mut changes,
        )
        .unwrap_err();
        assert_eq!(error.phase, FaultPhase::Execute);
        assert!(error.cause.to_string().contains(match failure {
            0 => "MissingFirmwareIdle",
            3 => "InvalidFirmwareRequest",
            _ => "UnsupportedPrivilegedWrite",
        }));
        assert_eq!(renderer.counters.submissions, before.submissions);
        assert_eq!(renderer.counters.draw_commands, before.draw_commands);
        assert_eq!(renderer.counters.readback_operations, 0);
        assert_eq!(
            renderer.counters.synchronization_points,
            before.synchronization_points + u64::from(failure != 0)
        );
        assert!(data[RT..RT + SIZE].chunks_exact(4).all(|p| p == BG));
        assert!(
            submit(
                &mut renderer,
                &mut channel,
                &entries,
                &map,
                &mut data,
                &mut changes
            )
            .is_err()
        );
        check(
            &sync(&mut renderer, &mut data, &mut changes).rgba,
            &image,
            None,
            BG,
        );
        assert_eq!(renderer.counters.draw_commands, before.draw_commands);
    }
}

fn macro_code(text: &str) -> Vec<u32> {
    text.split_whitespace()
        .map(|w| u32::from_str_radix(w, 16).unwrap())
        .collect()
}
fn upload_macro(words: &mut Vec<u32>, code: &[u32]) {
    for (m, v) in [(0x11c, 0), (0x120, 0), (0x114, 0)] {
        method(words, m, v);
    }
    for &word in code {
        method(words, 0x118, word);
    }
}

#[test]
#[ignore = "requires actual GPU; the sole indexed draw trigger originates in assembled MME"]
fn gpfifo_mme_indexed_draw_submits_real_geometry_and_resumes_after_the_draw() {
    let (mut data, map, mut words, image) = fixture();
    words.truncate(words.len() - 8); // Remove every caller-side draw method.
    upload_macro(
        &mut words,
        &macro_code(include_str!("fixtures/mme/indexed-draw.words")),
    );
    method(&mut words, 0x3800, 0);
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
    assert_eq!(renderer.counters.draw_commands, 1);
    assert_eq!(renderer.counters.readback_operations, 0);
    assert_eq!(renderer.counters.synchronization_points, 0);
    assert!(data[RT..RT + SIZE].chunks_exact(4).all(|p| p == BG));
    let before = renderer.counters;
    let again = [0x20010e00, 0];
    put(&mut data, 0x10000, &again);
    assert_eq!(
        submit(
            &mut renderer,
            &mut channel,
            &entry(0xb0000, again.len()),
            &map,
            &mut data,
            &mut changes
        )
        .unwrap()
        .draws_submitted,
        1
    );
    assert_eq!(renderer.counters.shader_decodes, before.shader_decodes);
    assert_eq!(
        renderer.counters.pipeline_allocations,
        before.pipeline_allocations
    );
    check(
        &sync(&mut renderer, &mut data, &mut changes).rgba,
        &image,
        None,
        BG,
    );
    assert_eq!(renderer.counters.draw_commands, 2);
    assert_eq!(renderer.counters.readback_operations, 1);
}

#[test]
#[ignore = "requires actual GPU; macro writes mapped constant bytes consumed by genuine LDC"]
fn gpfifo_mme_constant_writes_change_pixels_without_shader_recompilation() {
    mme_memory_writes(false);
}
#[test]
#[ignore = "requires actual GPU; MME inline DMA changes mapped LDC data and real pixels"]
fn gpfifo_mme_inline_dma_changes_pixels_without_shader_recompilation() {
    mme_memory_writes(true);
}
fn mme_memory_writes(inline: bool) {
    let (mut data, map, mut words, image) = fixture();
    let program = include_bytes!("fixtures/maxwell/ldc.bin");
    data[0x4500..0x4500 + program.len()].copy_from_slice(program);
    let draw = words.split_off(words.len() - 8);
    method(&mut words, 0x2144, 0x500); // Guest-selected independently assembled LDC PS.
    upload_macro(
        &mut words,
        &macro_code(if inline {
            include_str!("fixtures/mme/inline-upload.words")
        } else {
            include_str!("fixtures/mme/constant-write.words")
        }),
    );
    let mut renderer = Renderer::new().unwrap();
    let mut channel = Channel::default();
    let mut changes = TextureChanges::new(1);
    for (iteration, color, expected) in [
        (0, [0.25f32, 0.5, 0.75, 1.], [64, 128, 191, 255]),
        (1, [1.0, 0.0, 0.0, 1.], [255, 0, 0, 255]),
    ] {
        let before = renderer.counters;
        let mut commands = if iteration == 0 {
            words.clone()
        } else {
            Vec::new()
        };
        commands.push(if inline { 0xa0050e00 } else { 0xa0040e00 });
        if inline {
            commands.push(0x40010);
        }
        commands.extend(color.map(f32::to_bits));
        commands.extend_from_slice(&draw);
        put(&mut data, 0x10000, &commands);
        assert_eq!(
            submit(
                &mut renderer,
                &mut channel,
                &entry(0xb0000, commands.len()),
                &map,
                &mut data,
                &mut changes
            )
            .unwrap()
            .draws_submitted,
            1
        );
        let written: Vec<u8> = color.into_iter().flat_map(|v| v.to_le_bytes()).collect();
        assert_eq!(&data[80..96], &written);
        assert_eq!(
            if inline {
                renderer.counters.inline_upload_write_bytes
            } else {
                renderer.counters.constant_buffer_write_bytes
            },
            if inline {
                before.inline_upload_write_bytes
            } else {
                before.constant_buffer_write_bytes
            } + 16
        );
        check(
            &sync(&mut renderer, &mut data, &mut changes).rgba,
            &image,
            Some(expected),
            BG,
        );
        if iteration != 0 {
            assert_eq!(renderer.counters.shader_decodes, before.shader_decodes);
            assert_eq!(
                renderer.counters.pipeline_allocations,
                before.pipeline_allocations
            );
        }
    }
}

#[test]
#[ignore = "requires actual GPU; completed memory writes must never grant WFI authorization"]
fn gpfifo_mme_write_completion_cannot_acknowledge_privileged_initialization() {
    let (mut data, map, words, image) = fixture();
    put(&mut data, 0x10000, &words);
    let mut renderer = Renderer::new().unwrap();
    let mut channel = Channel::default();
    let mut changes = TextureChanges::new(1);
    submit(
        &mut renderer,
        &mut channel,
        &entry(0xb0000, words.len()),
        &map,
        &mut data,
        &mut changes,
    )
    .unwrap();
    let before = renderer.counters;
    let mut code = vec![0x00110071]; // Independently assembled actual WFI.
    let writes = macro_code(include_str!("fixtures/mme/constant-write.words"));
    code.extend_from_slice(&writes[..writes.len() - 2]);
    code.extend_from_slice(&[0x02310021, 0x8c0, 0x11]); // Firmware04, EXIT send R1, NOP.
    let mut commands = Vec::new();
    for (m, v) in [(0x3400, 0), (0x3404, 1), (0x3408, 1)] {
        method(&mut commands, m, v);
    }
    upload_macro(&mut commands, &code);
    commands.extend_from_slice(&[0xa0040e00, 0x418800, 1, 2, 3]);
    put(&mut data, 0x10000, &commands);
    let entries = entry(0xb0000, commands.len());
    let error = submit(
        &mut renderer,
        &mut channel,
        &entries,
        &map,
        &mut data,
        &mut changes,
    )
    .unwrap_err();
    assert!(error.cause.to_string().contains("MissingFirmwareIdle"));
    assert_eq!(
        renderer.counters.constant_buffer_write_bytes,
        before.constant_buffer_write_bytes + 16
    );
    assert_eq!(
        renderer.counters.synchronization_points,
        before.synchronization_points + 1
    );
    assert_eq!(renderer.counters.draw_commands, before.draw_commands);
    assert_eq!(renderer.counters.readback_operations, 0);
    assert_eq!(
        u32::from_le_bytes(data[80..84].try_into().unwrap()),
        0x418800
    );
    assert!(
        submit(
            &mut renderer,
            &mut channel,
            &entries,
            &map,
            &mut data,
            &mut changes
        )
        .is_err()
    );
    assert_eq!(
        renderer.counters.constant_buffer_write_bytes,
        before.constant_buffer_write_bytes + 16
    );
    check(
        &sync(&mut renderer, &mut data, &mut changes).rgba,
        &image,
        None,
        BG,
    );
}

#[test]
#[ignore = "requires actual GPU; an MME clear executes through guest color-target state"]
fn gpfifo_mme_clear_preserves_deferred_visibility_without_inventing_a_draw() {
    let (mut data, map, mut words, _) = fixture();
    words.truncate(words.len() - 8);
    for (m, v) in [
        (0x138, 1),
        (0x15d0, 0),
        (0x121c, 1),
        (0x10f8, 0),
        (0xff4, 52 << 16),
        (0xff8, 29 << 16),
        (0x1a00, 0),
        (0x1360, 1),
    ] {
        method(&mut words, m, v);
    }
    for (component, value) in [0.25f32, 0.5, 0.75, 1.].into_iter().enumerate() {
        method(&mut words, 0xd80 + component as u32 * 4, value.to_bits());
    }
    // envyas: maddr 0x674; exit send (add $r1 0); nop;
    upload_macro(&mut words, &[0x019d0021, 0x8c0, 0x11]);
    method(&mut words, 0x3800, 0x3c);
    put(&mut data, 0x10000, &words);
    let mut renderer = Renderer::new().unwrap();
    let mut channel = Channel::default();
    let mut changes = TextureChanges::new(1);
    let report = submit(
        &mut renderer,
        &mut channel,
        &entry(0xb0000, words.len()),
        &map,
        &mut data,
        &mut changes,
    )
    .unwrap();
    assert_eq!(report.draws_submitted, 0);
    assert_eq!(renderer.counters.draw_commands, 0);
    assert_eq!(renderer.counters.readback_operations, 0);
    assert_eq!(renderer.counters.shader_decodes, 0);
    assert!(data[RT..RT + SIZE].chunks_exact(4).all(|p| p == BG));
    let frame = sync(&mut renderer, &mut data, &mut changes);
    assert!(frame.rgba.chunks_exact(4).all(|p| p == [64, 128, 191, 255]));
    assert_eq!(&data[RT..RT + SIZE], &frame.rgba);
    assert_eq!(renderer.counters.readback_operations, 1);
}

#[test]
#[ignore = "requires actual GPU; a finite 517-instruction MME loop initializes the sampled CB"]
fn gpfifo_mme_finite_constant_fill_exceeds_64_steps_and_produces_real_pixels() {
    let (mut data, mut map, mut words, image) = fixture();
    let draw = words.split_off(words.len() - 8);
    map.map(GpuAddress(0xd0000), GuestAddress(0x12000), 0x1000)
        .unwrap();
    data[0x11ff0..0x12210].fill(0xc7);
    data[0x12000..0x12200].fill(0x6d);
    for (m, v) in [(0x2380, 512), (0x2384, 0), (0x2388, 0xd0000), (0x2490, 1)] {
        method(&mut words, m, v);
    }
    upload_macro(
        &mut words,
        &macro_code(include_str!("fixtures/mme/constant-fill.words")),
    );
    method(&mut words, 0x3800, 128);
    words.extend_from_slice(&draw);
    put(&mut data, 0x10000, &words);
    let mut renderer = Renderer::new().unwrap();
    let mut channel = Channel::default();
    let mut changes = TextureChanges::new(1);
    let report = submit(
        &mut renderer,
        &mut channel,
        &entry(0xb0000, words.len()),
        &map,
        &mut data,
        &mut changes,
    )
    .unwrap();
    assert_eq!(report.draws_submitted, 1);
    assert!(data[0x12000..0x12200].iter().all(|&b| b == 0));
    assert!(data[0x11ff0..0x12000].iter().all(|&b| b == 0xc7));
    assert!(data[0x12200..0x12210].iter().all(|&b| b == 0xc7));
    assert_eq!(renderer.counters.constant_buffer_write_bytes, 512);
    assert_eq!(renderer.counters.readback_operations, 0);
    assert_eq!(renderer.counters.synchronization_points, 0);
    check(
        &sync(&mut renderer, &mut data, &mut changes).rgba,
        &image,
        None,
        BG,
    );
}

fn full_mme_work_request(extra_launch: bool) -> (Vec<u32>, usize) {
    use wl_gpu::mme::{MAX_INSTRUCTIONS_PER_INVOCATION, MAX_INSTRUCTIONS_PER_SUBMISSION};
    let mut words = Vec::new();
    for (m, v) in [(0x11c, 0), (0x120, 0), (0x114, 0)] {
        method(&mut words, m, v);
    }
    let mut code = vec![0x11; MAX_INSTRUCTIONS_PER_INVOCATION];
    code[MAX_INSTRUCTIONS_PER_INVOCATION - 2] = 0x91;
    // Literal nonincrementing CODE data packet, so command acquisition stays
    // inside the existing bounded mapping without a permanent payload copy.
    words.push(0x60000046 | ((code.len() as u32) << 16));
    words.extend_from_slice(&code);
    assert_eq!(
        MAX_INSTRUCTIONS_PER_SUBMISSION % MAX_INSTRUCTIONS_PER_INVOCATION,
        0
    );
    for _ in 0..MAX_INSTRUCTIONS_PER_SUBMISSION / MAX_INSTRUCTIONS_PER_INVOCATION {
        method(&mut words, 0x3800, 0);
    }
    let cut = words.len();
    if extra_launch {
        method(&mut words, 0x3800, 0);
    }
    (words, cut)
}

#[test]
#[ignore = "requires actual GPU; work exhaustion across entries retains earlier draw without replay"]
fn gpfifo_mme_submission_work_is_bounded_across_all_invocations_and_entries() {
    let (mut data, map, words, image) = fixture();
    put(&mut data, 0x10000, &words);
    let mut renderer = Renderer::new().unwrap();
    let mut channel = Channel::default();
    let mut changes = TextureChanges::new(1);
    submit(
        &mut renderer,
        &mut channel,
        &entry(0xb0000, words.len()),
        &map,
        &mut data,
        &mut changes,
    )
    .unwrap();
    let before = renderer.counters;
    let (work, cut) = full_mme_work_request(true);
    put(&mut data, 0x10000, &work);
    let entries = [
        entry(0xb0000, cut),
        entry(0xc0000 + cut as u64 * 4, work.len() - cut),
    ]
    .concat();
    let error = submit(
        &mut renderer,
        &mut channel,
        &entries,
        &map,
        &mut data,
        &mut changes,
    )
    .unwrap_err();
    assert_eq!(error.phase, FaultPhase::Execute);
    assert_eq!(error.progress.entries_completed, 1);
    assert!(
        error
            .cause
            .to_string()
            .contains("SubmissionWorkBudgetExceeded")
    );
    assert_eq!(renderer.counters.draw_commands, before.draw_commands);
    assert_eq!(renderer.counters.readback_operations, 0);
    assert_eq!(renderer.counters.synchronization_points, 0);
    assert!(
        submit(
            &mut renderer,
            &mut channel,
            &entries,
            &map,
            &mut data,
            &mut changes
        )
        .is_err()
    );
    check(
        &sync(&mut renderer, &mut data, &mut changes).rgba,
        &image,
        None,
        BG,
    );
    assert_eq!(renderer.counters.draw_commands, before.draw_commands);
}

#[test]
#[ignore = "requires actual GPU; a valid request exhausts its allowance without poisoning the next"]
fn gpfifo_mme_work_budget_resets_at_the_next_valid_submission_only() {
    let (mut data, map, words, image) = fixture();
    put(&mut data, 0x10000, &words);
    let mut renderer = Renderer::new().unwrap();
    let mut channel = Channel::default();
    let mut changes = TextureChanges::new(1);
    submit(
        &mut renderer,
        &mut channel,
        &entry(0xb0000, words.len()),
        &map,
        &mut data,
        &mut changes,
    )
    .unwrap();
    let (work, _) = full_mme_work_request(false);
    put(&mut data, 0x10000, &work);
    assert_eq!(
        submit(
            &mut renderer,
            &mut channel,
            &entry(0xb0000, work.len()),
            &map,
            &mut data,
            &mut changes
        )
        .unwrap()
        .draws_submitted,
        0
    );
    let mut next = Vec::new();
    upload_macro(
        &mut next,
        &macro_code(include_str!("fixtures/mme/indexed-draw.words")),
    );
    method(&mut next, 0x3800, 0);
    put(&mut data, 0x10000, &next);
    assert_eq!(
        submit(
            &mut renderer,
            &mut channel,
            &entry(0xb0000, next.len()),
            &map,
            &mut data,
            &mut changes
        )
        .unwrap()
        .draws_submitted,
        1
    );
    assert_eq!(renderer.counters.readback_operations, 0);
    assert_eq!(renderer.counters.synchronization_points, 0);
    check(
        &sync(&mut renderer, &mut data, &mut changes).rgba,
        &image,
        None,
        BG,
    );
    assert_eq!(renderer.counters.draw_commands, 2);
}

#[test]
#[ignore = "requires GPU renderer; inline DMA bounds preflight and cross-request data continuity"]
fn gpfifo_inline_dma_empty_range_failure_and_ordered_continuation() {
    let (mut data, map, _, _) = fixture();
    let mut renderer = Renderer::new().unwrap();
    let mut changes = TextureChanges::new(1);
    for (address, length, valid) in [(0xa512345678, 0, true), (0x4001c, 8, false)] {
        let mut channel = Channel::default();
        let mut words = vec![0x20010000, 0xb197];
        pitch_inline_upload(&mut words, address, length);
        put(&mut data, 0x12000, &words);
        let before = data.clone();
        let stamp = changes.stamp(0..data.len() as u64);
        let result = submit(
            &mut renderer,
            &mut channel,
            &entry(0xb2000, words.len()),
            &map,
            &mut data,
            &mut changes,
        );
        assert_eq!(result.is_ok(), valid);
        if valid {
            channel.finish().unwrap();
        } else {
            let fault = result.unwrap_err();
            assert_eq!(fault.progress.words_consumed, 9);
            assert_eq!(fault.phase, FaultPhase::Execute);
            assert!(channel.is_faulted());
        }
        assert_eq!(data, before);
        assert_eq!(changes.stamp(0..data.len() as u64), stamp);
    }
    let mut channel = Channel::default();
    let mut first = vec![0x20010000, 0xb197];
    pitch_inline_upload(&mut first, 0x40000, 8);
    // One nonincrementing DATA packet continues into the next API request.
    first.extend_from_slice(&[0x6002006d, 0x04030201]);
    put(&mut data, 0x12000, &first);
    let canary = data[72..76].to_vec();
    submit(
        &mut renderer,
        &mut channel,
        &entry(0xb2000, first.len()),
        &map,
        &mut data,
        &mut changes,
    )
    .unwrap();
    assert_eq!(&data[64..68], &[1, 2, 3, 4]);
    put(&mut data, 0x12000, &[0x08070605]);
    submit(
        &mut renderer,
        &mut channel,
        &entry(0xb2000, 1),
        &map,
        &mut data,
        &mut changes,
    )
    .unwrap();
    channel.finish().unwrap();
    assert_eq!(&data[64..72], &[1, 2, 3, 4, 5, 6, 7, 8]);
    assert_eq!(&data[72..76], &canary);
    assert_eq!(renderer.counters.inline_upload_write_bytes, 8);
    assert_eq!(renderer.counters.readback_operations, 0);
    assert_eq!(renderer.counters.synchronization_points, 0);
    assert_eq!(renderer.counters.submissions, 0);
}

#[test]
#[ignore = "requires actual GPU; direct and MME replay use captured raster bits for real pixels"]
fn gpfifo_shadow_track_passthrough_and_mme_replay_control_pixels() {
    use wl_gpu::maxwell::{ShadowMode, ShadowValue};
    let (mut data, map, mut words, image) = fixture();
    let draw = words.split_off(words.len() - 8);
    for slot in 1..16 {
        // Valid inactive stream fields with deliberately unmapped addresses.
        method(&mut words, 0x1c04 + slot * 16, 0xa5);
        method(&mut words, 0x1c08 + slot * 16, 0xabc000 + slot * 4);
        method(&mut words, 0x1f00 + slot * 8, 0xff);
        method(&mut words, 0x1f04 + slot * 8, u32::MAX);
    }

    // SET_OBJECT then METHOD_TRACK before the independently authored setup.
    words.splice(2..2, [0x20010049, 0]);
    upload_macro(
        &mut words,
        &include_bytes!("fixtures/mme/state-mask.bin")
            .chunks_exact(4)
            .map(|word| u32::from_le_bytes(word.try_into().unwrap()))
            .collect::<Vec<_>>(),
    );
    let mut renderer = Renderer::new().unwrap();
    let mut channel = Channel::default();
    let mut changes = TextureChanges::new(1);
    let mut previous = None;
    for phase in 0..4 {
        let before = renderer.counters;
        if phase != 0 {
            for p in data[RT..RT + SIZE].chunks_exact_mut(4) {
                p.copy_from_slice(&BG);
            }
            changes.mark_write(GuestAddress(RT as u64), SIZE).unwrap();
        }
        let mut commands = if phase == 0 {
            words.clone()
        } else {
            Vec::new()
        };
        match phase {
            0 => method(&mut commands, 0x1a00, 1),
            1 => {
                method(&mut commands, 0x124, 2);
                method(&mut commands, 0x1a00, 0x1111);
            }
            _ => {
                method(&mut commands, 0x124, 3);
                method(&mut commands, 0x3800, if phase == 2 { u32::MAX } else { 0 });
                method(&mut commands, 0x124, 2); // Draw triggers are outside replay subset.
            }
        }
        commands.extend_from_slice(&draw);
        put(&mut data, 0x10000, &commands);
        assert_eq!(
            submit(
                &mut renderer,
                &mut channel,
                &entry(0xb0000, commands.len()),
                &map,
                &mut data,
                &mut changes
            )
            .unwrap()
            .draws_submitted,
            1
        );
        let frame = sync(&mut renderer, &mut data, &mut changes);
        let mut expected = image.clone();
        if phase != 1 {
            for pixel in expected.chunks_exact_mut(4) {
                pixel[1..].copy_from_slice(&BG[1..]);
            }
        }
        check(&frame.rgba, &expected, None, BG);
        assert_eq!(
            channel.engine.vertex_stream_address(0),
            Some(GpuAddress(0x70000))
        );
        for slot in 1..16 {
            assert_eq!(
                channel.engine.vertex_stream_address(slot),
                Some(GpuAddress(0xa500abc000 + slot as u64 * 4))
            );
        }
        assert_eq!(
            channel.engine.shadow_value(0x1a00),
            Some(ShadowValue::Known(1))
        );
        assert_eq!(
            channel.engine.shadow_mode(),
            Some(if phase == 0 {
                ShadowMode::Track
            } else {
                ShadowMode::Passthrough
            })
        );
        if let Some((decodes, modules)) = previous {
            assert_eq!(renderer.counters.shader_decodes, decodes);
            assert_eq!(renderer.counters.shader_module_allocations, modules);
        }
        if phase == 3 {
            assert_eq!(
                renderer.counters.pipeline_allocations,
                before.pipeline_allocations
            );
        }
        previous = Some((
            renderer.counters.shader_decodes,
            renderer.counters.shader_module_allocations,
        ));
    }
}

#[test]
#[ignore = "requires actual GPU; unknown shadow history never corrupts or replays prior draw"]
fn gpfifo_shadow_unknown_replay_preserves_pending_draw_and_latches_fault() {
    for (capture, writes, expected_error) in [
        (
            true,
            vec![(0x124, 1), (0x1a00, 1), (0x124, 3), (0x1a00, 0)],
            "FilteredShadowValue",
        ),
        (false, vec![(0x124, 3), (0x1a00, 0)], "MissingShadowValue"),
        (
            true,
            vec![(0x124, 3), (0x1214, 0x40030000)],
            "UnsupportedShadowReplayMethod",
        ),
    ] {
        let (mut data, map, mut words, image) = fixture();
        if capture {
            words.splice(2..2, [0x20010049, 0]);
        }
        put(&mut data, 0x10000, &words);
        let mut renderer = Renderer::new().unwrap();
        let mut channel = Channel::default();
        let mut changes = TextureChanges::new(1);
        submit(
            &mut renderer,
            &mut channel,
            &entry(0xb0000, words.len()),
            &map,
            &mut data,
            &mut changes,
        )
        .unwrap();
        let before = renderer.counters;
        let mut commands = Vec::new();
        for (m, v) in writes {
            method(&mut commands, m, v);
        }
        put(&mut data, 0x10000, &commands);
        let fault = submit(
            &mut renderer,
            &mut channel,
            &entry(0xb0000, commands.len()),
            &map,
            &mut data,
            &mut changes,
        )
        .unwrap_err();
        assert!(
            fault.cause.to_string().contains(expected_error),
            "{fault:?}"
        );
        assert!(channel.is_faulted());
        assert_eq!(fault.progress.draws_submitted, 0);
        assert_eq!(renderer.counters.draw_commands, before.draw_commands);
        assert_eq!(renderer.counters.submissions, before.submissions);
        assert_eq!(renderer.counters.readback_operations, 0);
        assert!(data[RT..RT + SIZE].chunks_exact(4).all(|p| p == BG));
        check(
            &sync(&mut renderer, &mut data, &mut changes).rgba,
            &image,
            None,
            BG,
        );
        assert!(
            submit(
                &mut renderer,
                &mut channel,
                &entry(0xb0000, commands.len()),
                &map,
                &mut data,
                &mut changes
            )
            .is_err()
        );
        assert_eq!(renderer.counters.draw_commands, before.draw_commands);
    }
}

#[test]
#[ignore = "requires actual GPU; source-derived initialization and cache barriers preserve pixels"]
fn gpfifo_public_initialization_cache_actions_and_privileged_tile_baseline() {
    let (mut data, map, mut commands, image) = fixture();
    let draw = commands.split_off(commands.len() - 8);
    // Public hardware baseline only. No proprietary command bytes are fixtures.
    for (m, v) in [
        (5396, 1),
        (5392, 255),
        (4028, 65535),
        (4032, 65535),
        (4036, 65535),
        (4040, 65535),
        (5408, 1),
        (5636, 8),
        (6416, 1),
        (5400, 1065353216),
        (5016, 255),
        (5020, 255),
        (3928, 255),
        (3932, 255),
        (4424, 0),
        (748, 0),
        (4020, 0),
        (4056, 0),
        (4400, 19),
        (4600, 0),
        (5812, 0),
        (3940, 8388736),
        (3944, 4361),
        (3948, 134742530),
        (4360, 31),
        (3952, 524289),
        (4404, 0),
        (1956, 32767),
        (1960, 32767),
    ] {
        method(&mut commands, m, v);
    }
    // Separate compute subchannel; no compute dispatch or memory accesses.
    commands.extend([0x20012000, 0xb1c0]);
    for (m, v) in [
        (0x1528, 0),
        (0x2608, 2),
        (0x77c, 0x1000000),
        (0x214, 0x3000000),
        (0x1608, 4),
        (0x160c, 0),
        (0x310, 0x503),
    ] {
        commands.extend([0x20012000 | (m / 4), v]);
    }
    commands.extend_from_slice(&draw);
    let mut renderer = Renderer::new().unwrap();
    let mut channel = Channel::default();
    let mut changes = TextureChanges::new(1);
    put(&mut data, 0x10000, &commands);
    submit(
        &mut renderer,
        &mut channel,
        &entry(0xb0000, commands.len()),
        &map,
        &mut data,
        &mut changes,
    )
    .unwrap();
    assert_eq!(renderer.counters.readback_operations, 0);
    assert!(data[RT..RT + SIZE].chunks_exact(4).all(|p| p == BG));
    let before = renderer.counters;
    let mut commands = Vec::new();
    upload_macro(
        &mut commands,
        &macro_code(include_str!("fixtures/mme/privileged-state.words")),
    );
    for (a, v, m) in [
        (0x418e40, 0x7, 0xf),
        (0x418e58, 0x842, 0xffff),
        (0x418e40, 0x70, 0xf0),
        (0x418e58, 0x4f10000, 0xffff0000),
        (0x418e40, 0x700, 0xf00),
        (0x418e5c, 0x53, 0xffff),
        (0x418e40, 0x7000, 0xf000),
        (0x418e5c, 0xe90000, 0xffff0000),
        (0x418e40, 0x70000, 0xf0000),
        (0x418e60, 0xea, 0xffff),
        (0x418e40, 0x700000, 0xf00000),
        (0x418e60, 0xeb0000, 0xffff0000),
        (0x418e40, 0x7000000, 0xf000000),
        (0x418e64, 0x208, 0xffff),
        (0x418e40, 0x70000000, 0xf0000000),
        (0x418e64, 0x2090000, 0xffff0000),
        (0x418e44, 0x7, 0xf),
        (0x418e68, 0x20a, 0xffff),
        (0x418e44, 0x70, 0xf0),
        (0x418e68, 0x20b0000, 0xffff0000),
        (0x418e44, 0x700, 0xf00),
        (0x418e6c, 0x644, 0xffff),
    ] {
        commands.extend([0xa0040e00, a, v, m, 0x1111]);
    }
    method(&mut commands, 0x1288, 0);
    method(&mut commands, 0xda4, 0x1011);
    method(&mut commands, 0x1428, 0);
    method(&mut commands, 0x1424, 0);
    commands.extend_from_slice(&draw);
    put(&mut data, 0x10000, &commands);
    submit(
        &mut renderer,
        &mut channel,
        &entry(0xb0000, commands.len()),
        &map,
        &mut data,
        &mut changes,
    )
    .unwrap();
    assert_eq!(renderer.counters.draw_commands, before.draw_commands + 1);
    assert_eq!(renderer.counters.readback_operations, 0);
    assert_eq!(
        renderer.counters.synchronization_points,
        before.synchronization_points + 22
    );
    assert_eq!(
        renderer.counters.pipeline_allocations,
        before.pipeline_allocations
    );
    assert_eq!(
        renderer.counters.shader_module_allocations,
        before.shader_module_allocations
    );
    assert!(renderer.counters.guest_texture_bytes > before.guest_texture_bytes);
    assert!(renderer.counters.constant_guest_bytes > before.constant_guest_bytes);
    assert!(renderer.counters.shader_binary_bytes > before.shader_binary_bytes);
    // Host operation is independent of engine binding, even an unbound subchannel.
    let commands = [0x2001c00b, 0x80000000, 0x2001c00b, 0x70000000];
    put(&mut data, 0x10000, &commands);
    submit(
        &mut renderer,
        &mut channel,
        &entry(0xb0000, commands.len()),
        &map,
        &mut data,
        &mut changes,
    )
    .unwrap();
    assert_eq!(renderer.counters.readback_operations, 1);
    assert_eq!(renderer.counters.draw_commands, before.draw_commands + 1);
    check(&data[RT..RT + SIZE], &image, None, BG);
}
