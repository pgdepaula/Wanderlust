#![cfg(feature = "wgpu-backend")]
mod common;
use wl_gpu::{backend::Renderer, maxwell::Channel, memory::*, texture::TextureChanges};
const SIZE: usize = 208 * 29;
const RT: usize = 0x8000;
const BG: [u8; 4] = [11, 17, 23, 128];
const POS: [[[f32; 2]; 3]; 2] = [
    [[-0.9, -0.8], [0.6, -0.6], [-0.5, 0.85]],
    [[-0.5, -0.7], [0.9, 0.1], [-0.1, 0.8]],
];
const COLOR: [[f32; 4]; 2] = [[0.25, 1., 0.5, 0.5], [0.75, 0., 0.25, 0.25]];
fn method(w: &mut Vec<u32>, m: u32, v: u32) {
    w.extend([0x20010000 | (m / 4), v]);
}
fn edge(a: [f64; 2], b: [f64; 2], p: [f64; 2]) -> f64 {
    (b[0] - a[0]) * (p[1] - a[1]) - (b[1] - a[1]) * (p[0] - a[0])
}
fn reference(pixels: &[u8], masked: bool) {
    let mut counts = [0; 4];
    for y in 0..29 {
        for x in 0..52 {
            let p = [
                (x as f64 + 0.5) * 2. / 47. - 1.,
                1. - (y as f64 + 0.5) * 2. / 29.,
            ];
            let mut result = BG;
            let mut coverage = 0;
            let mut tie = false;
            for (n, triangle) in POS.iter().enumerate() {
                let v = triangle.map(|p| p.map(f64::from));
                let area = edge(v[0], v[1], v[2]);
                let b = [
                    edge(v[1], v[2], p) / area,
                    edge(v[2], v[0], p) / area,
                    edge(v[0], v[1], p) / area,
                ];
                if b.iter().any(|v| v.abs() < 0.005) {
                    tie = true;
                }
                if x >= 47 || b.iter().any(|v| *v < 0.) {
                    continue;
                }
                coverage |= 1 << n;
                let c = COLOR[n].map(f64::from);
                for k in 0..4 {
                    let source = if k == 3 {
                        c[k] * 255.
                    } else {
                        c[k] * 255. * c[3]
                    };
                    result[k] = (source + f64::from(result[k]) * (1. - c[3])).round() as u8;
                }
            }
            if tie {
                continue;
            }
            if masked && coverage & 2 != 0 {
                result[0] = (COLOR[1][0] * 255.).round() as u8;
                result[2] = (COLOR[1][2] * 255.).round() as u8;
            }
            let actual = &pixels[(y * 52 + x) * 4..(y * 52 + x + 1) * 4];
            for k in 0..4 {
                assert!(
                    actual[k].abs_diff(result[k]) <= 1,
                    "{x},{y}: {actual:?} != {result:?}"
                );
            }
            counts[coverage] += 1;
        }
    }
    assert!(counts.iter().all(|n| *n > 20), "{counts:?}");
}
fn constants(data: &mut [u8], n: usize) {
    for (k, v) in COLOR[n].iter().enumerate() {
        data[80 + k * 4..84 + k * 4].copy_from_slice(&v.to_le_bytes());
    }
}
fn draw(
    r: &mut Renderer,
    c: &mut Channel,
    w: &[u32],
    map: &AddressMap,
    data: &mut [u8],
    changes: &mut TextureChanges,
) {
    r.submit_pushbuffer_deferred(
        c,
        w,
        map,
        &mut SliceMemoryMut {
            base: GuestAddress(0),
            bytes: data,
        },
        changes,
    )
    .unwrap();
}
#[test]
#[ignore = "requires actual GPU; run explicitly"]
fn b197_alpha_blend_mutable_constants_masks_and_pipeline_identity() {
    blend_scene(false, false);
}
#[test]
#[ignore = "requires actual GPU; target-zero independent blend bank controls real pixels"]
fn b197_per_target_blend_uses_selected_bank_and_preserves_masks() {
    blend_scene(true, false);
}
#[test]
#[ignore = "requires actual GPU; equivalent global/per-target state shares pipeline"]
fn b197_global_to_per_target_blend_selection_reuses_effective_pipeline() {
    blend_scene(false, true);
}
fn blend_method(words: &mut Vec<u32>, m: u32, value: u32, per_target: bool) {
    let selected = if per_target {
        match m {
            0x133c => 0x1e00,
            0x1340 => 0x1e04,
            0x1344 => 0x1e08,
            0x1348 => 0x1e0c,
            0x134c => 0x1e10,
            0x1350 => 0x1e14,
            0x1358 => 0x1e18,
            _ => m,
        }
    } else {
        m
    };
    method(words, selected, value);
}
fn blend_scene(per_target: bool, switch: bool) {
    let mut data = vec![0u8; 0xe000];
    for (off, bin) in [
        (
            0x4200,
            include_bytes!("fixtures/maxwell/vertex.bin").as_slice(),
        ),
        (
            0x4500,
            include_bytes!("fixtures/maxwell/ldc.bin").as_slice(),
        ),
    ] {
        data[off..off + bin.len()].copy_from_slice(bin);
    }
    for (n, p) in POS.iter().flatten().enumerate() {
        for (k, v) in [p[0], p[1], 0., 1., 0., 0.].iter().enumerate() {
            data[0x3000 + n * 24 + k * 4..0x3004 + n * 24 + k * 4]
                .copy_from_slice(&v.to_le_bytes());
        }
    }
    for p in data[RT..RT + SIZE].chunks_exact_mut(4) {
        p.copy_from_slice(&BG);
    }
    constants(&mut data, 0);
    let mut map = AddressMap::default();
    for (g, a, n) in [
        (0x40000, 64, 32),
        (0x50000, 0x4000, 0x1000),
        (0x70000, 0x3000, 144),
        (0x90000, RT as u64, SIZE as u64),
    ] {
        map.map(GpuAddress(g), GuestAddress(a), n).unwrap();
    }
    let mut words = common::hardware::setup();
    method(&mut words, 0x2144, 0x500);
    method(&mut words, 0x1f04, 0x7008f);
    method(&mut words, 0x15e4, 1);
    if per_target {
        method(&mut words, 0x12e4, 1);
    }
    // Literal bank-one packet deliberately differs from active target zero.
    words.extend_from_slice(&[0x20070788, 1, 1, 2, 1, 1, 2, 1]);
    for (m, v) in [
        (0x1360, 1),
        (0x133c, 1),
        (0x1340, 0x8006),
        (0x1344, 0x4302),
        (0x1348, 0x4303),
        (0x134c, 0x8006),
        (0x1350, 0x4001),
        (0x1358, 0x4303),
        (0x1214, 0x40030000),
    ] {
        blend_method(&mut words, m, v, per_target);
    }
    let mut r = Renderer::new().unwrap();
    let mut c = Channel::default();
    let mut changes = TextureChanges::new(1);
    draw(&mut r, &mut c, &words, &map, &mut data, &mut changes);
    let before = r.counters;
    constants(&mut data, 1);
    changes.mark_write(GuestAddress(80), 16).unwrap();
    // Numerically equivalent D3D encodings must reuse the existing pipeline.
    let mut words = Vec::new();
    method(&mut words, 0x15e4, 0); // Marking must not erase filled triangle edges.
    if switch {
        method(&mut words, 0x12e4, 1);
    }
    if per_target || switch {
        method(&mut words, 0x1e00, 1);
    }
    for (m, v) in [
        (0x1340, 1),
        (0x1344, 5),
        (0x1348, 6),
        (0x134c, 1),
        (0x1350, 2),
        (0x1358, 6),
        (0x1214, 0x40030003),
    ] {
        blend_method(&mut words, m, v, per_target || switch);
    }
    draw(&mut r, &mut c, &words, &map, &mut data, &mut changes);
    assert_eq!(r.counters.pipeline_allocations, before.pipeline_allocations);
    assert_eq!(
        r.counters.shader_module_allocations,
        before.shader_module_allocations
    );
    assert_eq!(r.counters.readback_operations, 0);
    let frame = r
        .synchronize_target(
            &mut SliceMemoryMut {
                base: GuestAddress(0),
                bytes: &mut data,
            },
            &mut changes,
        )
        .unwrap()
        .unwrap();
    reference(&frame.rgba, false);
    // Disable blending and write red/blue only. Green and alpha retain old output.
    let before = r.counters;
    let mut words = Vec::new();
    method(&mut words, 0x1360, 0);
    method(&mut words, 0x1a00, 0x101);
    method(&mut words, 0x1214, 0x40030003);
    draw(&mut r, &mut c, &words, &map, &mut data, &mut changes);
    assert_eq!(
        r.counters.pipeline_allocations,
        before.pipeline_allocations + 1
    );
    assert_eq!(
        r.counters.shader_module_allocations,
        before.shader_module_allocations
    );
    let frame = r
        .synchronize_target(
            &mut SliceMemoryMut {
                base: GuestAddress(0),
                bytes: &mut data,
            },
            &mut changes,
        )
        .unwrap()
        .unwrap();
    reference(&frame.rgba, true);
    if switch {
        let before = r.counters;
        let mut incomplete = common::hardware::setup();
        for (m, v) in [
            (0x2144, 0x500),
            (0x1f04, 0x7008f),
            (0x1360, 1),
            (0x12e4, 1),
            (0x1214, 0x40030000),
        ] {
            method(&mut incomplete, m, v);
        }
        let mut invalid = Channel::default();
        let error = r
            .submit_pushbuffer_deferred(
                &mut invalid,
                &incomplete,
                &map,
                &mut SliceMemoryMut {
                    base: GuestAddress(0),
                    bytes: &mut data,
                },
                &mut changes,
            )
            .unwrap_err();
        assert!(
            error
                .to_string()
                .contains("missing per-target alpha blend selection"),
            "{error}"
        );
        assert!(invalid.is_faulted());
        assert_eq!(r.counters.draw_commands, before.draw_commands);
        assert_eq!(
            r.counters.shader_module_allocations,
            before.shader_module_allocations
        );
        assert_eq!(r.counters.readback_operations, before.readback_operations);
        reference(&data[RT..RT + SIZE], true);
    }
    println!(
        "adapter={:?} counters={:?}",
        r.capabilities.adapter, r.counters
    );
}
