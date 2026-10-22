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
fn b197_rgba8_zero_weights_suppress_nonfinite_inputs_without_shader_recompilation() {
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

    method(&mut words, 0x0fdc, 1);
    method(&mut words, 0x19c0, 1);
    for (m, v) in [
        (0x1360, 1),
        (0x133c, 1),
        (0x1340, 0x8006),
        (0x1344, 0x4302),
        (0x1348, 0x4001),
        (0x134c, 0x8006),
        (0x1350, 0x4000),
        (0x1358, 0x4001),
    ] {
        method(&mut words, m, v);
    }
    let mut r = Renderer::new().unwrap();
    let mut c = Channel::default();
    let mut changes = TextureChanges::new(1);
    let shader_before = r.counters.shader_module_allocations;
    for source_factor in [0x4000, 0x4302] {
        method(&mut words, 0x1344, source_factor);
        for bits in [
            0x3f000000u32,
            0x7fc00001,
            0xffc00001,
            0x7f800000,
            0xff800000,
        ] {
            for k in 0..4 {
                data[80 + k * 4..84 + k * 4].copy_from_slice(&bits.to_le_bytes());
            }
            data[92..96].copy_from_slice(&0u32.to_le_bytes());
            changes.mark_write(GuestAddress(80), 16).unwrap();
            let mut command = words.clone();
            method(&mut command, 0x1214, 0x40030000);
            draw(&mut r, &mut c, &command, &map, &mut data, &mut changes);
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
            for (i, p) in frame.rgba.chunks_exact(4).enumerate() {
                assert_eq!(p, BG, "bits={bits:x} pixel={i}");
            }
        }
    }
    // All zero-weight cases share exactly one translated vertex/fragment pair.
    assert_eq!(r.counters.shader_module_allocations, shader_before + 2);
    let before_positive = r.counters.shader_module_allocations;
    for k in 0..4 {
        data[80 + k * 4..84 + k * 4].copy_from_slice(&0.5f32.to_le_bytes());
    }
    changes.mark_write(GuestAddress(80), 16).unwrap();
    let mut command = Vec::new();
    for (m, v) in [
        (0x1344, 0x4001),
        (0x1348, 0x4000),
        (0x1350, 0x4001),
        (0x1358, 0x4000),
        (0x1214, 0x40030000),
    ] {
        method(&mut command, m, v);
    }
    draw(&mut r, &mut c, &command, &map, &mut data, &mut changes);
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
    assert!(
        frame
            .rgba
            .chunks_exact(4)
            .filter(|p| p.iter().all(|v| v.abs_diff(128) <= 1))
            .count()
            > 20
    );
    assert!(frame.rgba.chunks_exact(4).filter(|p| *p == BG).count() > 20);
    assert_eq!(r.counters.shader_module_allocations, before_positive);
    println!(
        "finite/NaN/infinity constant/runtime zero-factor cases and positive coverage control passed {:?} counters={:?}",
        r.capabilities.adapter, r.counters
    );
}
