#![cfg(feature = "wgpu-backend")]
mod common;
use stats_alloc::{INSTRUMENTED_SYSTEM, Region, StatsAlloc};
use std::alloc::System;
#[global_allocator]
static ALLOCATOR: &StatsAlloc<System> = &INSTRUMENTED_SYSTEM;
use common::{maxwell::*, *};
use wl_gpu::{
    backend::{Frame, Renderer},
    maxwell::{Channel, TextureImage, TextureSampler},
    memory::*,
    texture::*,
};
const POS: [[f32; 2]; 3] = [[-0.8, -0.6], [0.6, -0.4], [-0.2, 0.7]];
const UV: [[f32; 2]; 3] = [[0.1, 0.2], [0.9, 0.1], [0.25, 0.85]];
const BG: [u8; 4] = [11, 17, 23, 255];
const DRAW: [u32; 2] = [0x20010485, 0x40030000];
const RT: usize = 0x8000;
const SIZE: usize = 208 * 29;
fn vertices(data: &mut [u8], pos: [[f32; 2]; 3], uv: [[f32; 2]; 3]) {
    for n in 0..3 {
        for (c, v) in [pos[n][0], pos[n][1], 0., 1., uv[n][0], uv[n][1]]
            .into_iter()
            .enumerate()
        {
            data[0x3000 + n * 24 + c * 4..0x3000 + n * 24 + c * 4 + 4]
                .copy_from_slice(&v.to_le_bytes());
        }
    }
}
fn background(data: &mut [u8], offset: usize, bg: [u8; 4]) {
    for p in data[offset..offset + SIZE].chunks_exact_mut(4) {
        p.copy_from_slice(&bg);
    }
}
fn edge(a: [f64; 2], b: [f64; 2], p: [f64; 2]) -> f64 {
    (b[0] - a[0]) * (p[1] - a[1]) - (b[1] - a[1]) * (p[0] - a[0])
}
fn check(
    pixels: &[u8],
    pos: [[f32; 2]; 3],
    uv: [[f32; 2]; 3],
    image: &[u8],
    solid: Option<[u8; 4]>,
    scissor: [usize; 4],
) {
    let v = pos.map(|p| p.map(f64::from));
    let area = edge(v[0], v[1], v[2]);
    let mut covered = 0;
    let mut outside = 0;
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
            let actual = &pixels[(y * 52 + x) * 4..(y * 52 + x + 1) * 4];
            let clipped =
                x >= 47 || x < scissor[0] || y < scissor[1] || x >= scissor[2] || y >= scissor[3];
            if !clipped && b.iter().any(|v| v.abs() < 0.005) {
                continue;
            }
            if clipped || b.iter().any(|v| *v < 0.) {
                assert_eq!(actual, BG, "outside {x},{y}, pos={pos:?}, bary={b:?}");
                outside += 1;
                continue;
            }
            let expected = if let Some(color) = solid {
                color
            } else {
                let u = (0..3).map(|n| b[n] * f64::from(uv[n][0])).sum::<f64>() * 37.;
                let v = (0..3).map(|n| b[n] * f64::from(uv[n][1])).sum::<f64>() * 19.;
                if (u - u.round()).abs() < 0.02 || (v - v.round()).abs() < 0.02 {
                    continue;
                }
                let n = (v.floor().clamp(0., 18.) as usize * 37
                    + u.floor().clamp(0., 36.) as usize)
                    * 4;
                image[n..n + 4].try_into().unwrap()
            };
            for c in 0..4 {
                assert!(
                    actual[c].abs_diff(expected[c]) <= 1,
                    "pixel{x},{y}: {actual:?} != {expected:?}"
                );
            }
            covered += 1;
        }
    }
    assert!(covered > 100);
    assert!(outside > 600);
}
fn run(
    r: &mut Renderer,
    c: &mut Channel,
    words: &[u32],
    m: &AddressMap,
    data: &mut [u8],
    changes: &mut TextureChanges,
) -> Option<Frame> {
    r.submit_pushbuffer(
        c,
        words,
        m,
        &mut SliceMemoryMut {
            base: GuestAddress(0),
            bytes: data,
        },
        changes,
    )
    .unwrap()
}
#[test]
#[ignore = "requires actual GPU; run explicitly with --ignored --nocapture"]
fn b197_guest_geometry_shaders_raster_target_and_draw() {
    let desc = TextureImage::decode(TIC_RGBA)
        .unwrap()
        .with_sampler(TextureSampler::decode(TSC_NEAREST).unwrap());
    let (surface, image) = rgba_surface(desc);
    let mut data = vec![0u8; 0xe000];
    data[..64].copy_from_slice(&descriptor_bytes(TIC_RGBA, TSC_NEAREST));
    data[0x1000..0x1000 + surface.len()].copy_from_slice(&surface);
    for (off, b) in [
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
            0x4300,
            include_bytes!("fixtures/maxwell/vertex-swap.bin").as_slice(),
        ),
    ] {
        data[off..off + b.len()].copy_from_slice(b);
    }
    vertices(&mut data, POS, UV);
    background(&mut data, RT, BG);
    background(&mut data, 0xb000, BG);
    let mut m = AddressMap::default();
    for (g, a, n) in [
        (0x10000, 0x1000, surface.len() as u64),
        (0x30000, 0, 32),
        (0x31000, 32, 32),
        (0x40000, 64, 32),
        (0x50000, 0x4000, 0x1000),
        (0x70000, 0x3000, 72),
        (0x90000, RT as u64, SIZE as u64),
        (0xa0000, 0xb000, SIZE as u64),
    ] {
        m.map(GpuAddress(g), GuestAddress(a), n).unwrap();
    }
    let mut words = POOL_PACKETS.to_vec();
    // Literal B197 program/vertex state, then RT WIDTH=208-byte pitch (52 pixels).
    words.extend_from_slice(&[
        0x20020582,
        0,
        0x50000,
        0x20020850,
        0x51,
        0,
        0x20020853,
        8,
        4,
        0x200308e0,
        32,
        0,
        0x40000,
        0x20010924,
        1,
        0x20020810,
        0x11,
        0x200,
        0x20020813,
        8,
        0,
        0x20040700,
        0x1018,
        0,
        0x70000,
        1,
        0x200207c0,
        0,
        0x70047,
        0x20010620,
        0,
        0x20020458,
        0x38200000,
        0x38800800,
        0x20090200,
        0,
        0x90000,
        208,
        29,
        0xd5,
        0x1000,
        1,
        0,
        0,
        0x20060280,
        0x41bc0000,
        0xc1680000,
        0x3f800000,
        0x41bc0000,
        0x41680000,
        0,
        0x200203fd,
        47 << 16,
        29 << 16,
        0x20020300,
        47 << 16,
        29 << 16,
        0x20030380,
        1,
        47 << 16,
        29 << 16,
        0x20010487,
        1,
        0x200104b3,
        0,
        0x200104ba,
        0,
        0x200104e0,
        0,
        0x200104d8,
        0,
        0x200104d7,
        0,
        0x200104b9,
        0,
        0x20010646,
        0,
        0x200104eb,
        0,
        0x20010649,
        0,
        0x2001064b,
        1,
        0x2001064f,
        0x1801,
        0x20010574,
        0,
        0x2001054e,
        0,
        0x20010680,
        0x1111,
        0x2001036b,
        0x1b02,
        0x2001036c,
        0x1b02,
        0x20010671,
        0,
        0x2001066f,
        0,
        0x2001036d,
        0,
        0x20010372,
        0,
        0x200105a3,
        0,
        0x20020302,
        0,
        0x3f800000,
    ]);
    words.extend_from_slice(&[
        0x2001004e, 1, 0x200100df, 1, 0x20010286, 0x6420, 0x2002037e, 0, 0, 0x200104bb, 0,
        0x200103e9, 0,
    ]);
    words.extend_from_slice(&DRAW);
    let mut r = Renderer::new().unwrap();
    let mut c = Channel::default();
    let mut changes = TextureChanges::new(80);
    println!("adapter={:?}", r.capabilities.adapter);
    // Split a packet mid-data: FIFO continuation survives the chunk boundary.
    assert!(run(&mut r, &mut c, &words[..3], &m, &mut data, &mut changes).is_none());
    let frame = run(&mut r, &mut c, &words[3..], &m, &mut data, &mut changes).unwrap();
    c.finish().unwrap();
    assert_eq!((frame.width, frame.height), (52, 29));
    assert_eq!(frame.rgba, data[RT..RT + SIZE]);
    check(&frame.rgba, POS, UV, &image[0], None, [0, 0, 47, 29]);
    let cold = r.counters;
    let start = std::time::Instant::now();
    let region = Region::new(ALLOCATOR);
    let again = run(&mut r, &mut c, &DRAW, &m, &mut data, &mut changes).unwrap();
    let warm_allocations = region.change();
    assert_eq!(frame.rgba, again.rgba);
    assert_eq!(cold.upload_bytes, r.counters.upload_bytes);
    assert_eq!(cold.pipeline_allocations, r.counters.pipeline_allocations);
    assert_eq!(
        cold.render_target_guest_bytes,
        r.counters.render_target_guest_bytes
    );
    println!(
        "repeat elapsed={:?}, process-wide Rust allocations={warm_allocations:?}, cold={cold:?}, repeated={:?}",
        start.elapsed(),
        r.counters
    );
    let moved = POS.map(|p| [p[0] + 0.2, p[1] - 0.1]);
    vertices(&mut data, moved, UV);
    changes.mark_write(GuestAddress(0x3000), 72).unwrap();
    background(&mut data, RT, BG);
    changes.mark_write(GuestAddress(RT as u64), SIZE).unwrap();
    let f = run(&mut r, &mut c, &DRAW, &m, &mut data, &mut changes).unwrap();
    check(&f.rgba, moved, UV, &image[0], None, [0, 0, 47, 29]);
    assert_eq!(cold.pipeline_allocations, r.counters.pipeline_allocations);
    // Guest texture-only writes change sampled colors, never shader code identity.
    for p in data[0x1000..0x1000 + surface.len()].chunks_exact_mut(4) {
        p.copy_from_slice(&[9, 200, 41, 255]);
    }
    changes
        .mark_write(GuestAddress(0x1000), surface.len())
        .unwrap();
    background(&mut data, RT, BG);
    changes.mark_write(GuestAddress(RT as u64), SIZE).unwrap();
    let f = run(&mut r, &mut c, &DRAW, &m, &mut data, &mut changes).unwrap();
    check(
        &f.rgba,
        moved,
        UV,
        &image[0],
        Some([9, 200, 41, 255]),
        [0, 0, 47, 29],
    );
    assert_eq!(cold.pipeline_allocations, r.counters.pipeline_allocations);
    // Select another guest pixel binary, retaining geometry and destination.
    background(&mut data, RT, BG);
    changes.mark_write(GuestAddress(RT as u64), SIZE).unwrap();
    let f = run(
        &mut r,
        &mut c,
        &[0x20010851, 0x100, DRAW[0], DRAW[1]],
        &m,
        &mut data,
        &mut changes,
    )
    .unwrap();
    check(
        &f.rgba,
        moved,
        UV,
        &image[0],
        Some([64, 128, 191, 255]),
        [0, 0, 47, 29],
    );
    let old = data[RT..RT + SIZE].to_vec();
    let f = run(
        &mut r,
        &mut c,
        &[0x20010201, 0xa0000, DRAW[0], DRAW[1]],
        &m,
        &mut data,
        &mut changes,
    )
    .unwrap();
    assert_eq!(old, data[RT..RT + SIZE]);
    assert_eq!(f.rgba, data[0xb000..0xb000 + SIZE]);
    check(
        &f.rgba,
        moved,
        UV,
        &image[0],
        Some([64, 128, 191, 255]),
        [0, 0, 47, 29],
    );
    // Positive Y scale must invert WGSL clip Y to preserve Maxwell's affine transform.
    background(&mut data, 0xb000, BG);
    changes.mark_write(GuestAddress(0xb000), SIZE).unwrap();
    let f = run(
        &mut r,
        &mut c,
        &[0x20010281, 14.5f32.to_bits(), DRAW[0], DRAW[1]],
        &m,
        &mut data,
        &mut changes,
    )
    .unwrap();
    check(
        &f.rgba,
        moved.map(|p| [p[0], -p[1]]),
        UV,
        &image[0],
        Some([64, 128, 191, 255]),
        [0, 0, 47, 29],
    );
    // A real scissor narrows coverage, with the guest background preserved.
    background(&mut data, 0xb000, BG);
    changes.mark_write(GuestAddress(0xb000), SIZE).unwrap();
    let f = run(
        &mut r,
        &mut c,
        &[0x20010381, (43 << 16) | 5, DRAW[0], DRAW[1]],
        &m,
        &mut data,
        &mut changes,
    )
    .unwrap();
    check(
        &f.rgba,
        moved.map(|p| [p[0], -p[1]]),
        UV,
        &image[0],
        Some([64, 128, 191, 255]),
        [5, 0, 43, 29],
    );
    // A guest shader offset changes geometry; no host program or vertices argument.
    background(&mut data, 0xb000, BG);
    changes.mark_write(GuestAddress(0xb000), SIZE).unwrap();
    let f = run(
        &mut r,
        &mut c,
        &[0x20010811, 0x300, DRAW[0], DRAW[1]],
        &m,
        &mut data,
        &mut changes,
    )
    .unwrap();
    check(
        &f.rgba,
        moved.map(|p| [p[1], -p[0]]),
        UV,
        &image[0],
        Some([64, 128, 191, 255]),
        [5, 0, 43, 29],
    );
    // In-place guest code mutation invalidates the selected VS, even at the same VA.
    let binary = include_bytes!("fixtures/maxwell/vertex.bin");
    data[0x4300..0x4300 + binary.len()].copy_from_slice(binary);
    changes
        .mark_write(GuestAddress(0x4300), binary.len())
        .unwrap();
    background(&mut data, 0xb000, BG);
    changes.mark_write(GuestAddress(0xb000), SIZE).unwrap();
    let decoded = r.counters.vertex_shader_decodes;
    let f = run(&mut r, &mut c, &DRAW, &m, &mut data, &mut changes).unwrap();
    assert_eq!(r.counters.vertex_shader_decodes, decoded + 1);
    check(
        &f.rgba,
        moved.map(|p| [p[0], -p[1]]),
        UV,
        &image[0],
        Some([64, 128, 191, 255]),
        [5, 0, 43, 29],
    );
    // Relocate the destination's GPU mapping to different guest backing at the same VA.
    background(&mut data, RT, BG);
    changes.mark_write(GuestAddress(RT as u64), SIZE).unwrap();
    let mut remap = AddressMap::default();
    for (g, a, n) in [
        (0x50000, 0x4000, 0x1000),
        (0x70000, 0x3000, 72),
        (0xa0000, RT as u64, SIZE as u64),
    ] {
        remap.map(GpuAddress(g), GuestAddress(a), n).unwrap();
    }
    let old = data[0xb000..0xb000 + SIZE].to_vec();
    let f = run(&mut r, &mut c, &DRAW, &remap, &mut data, &mut changes).unwrap();
    assert_eq!(f.rgba, data[RT..RT + SIZE]);
    assert_eq!(old, data[0xb000..0xb000 + SIZE]);
    check(
        &f.rgba,
        moved.map(|p| [p[0], -p[1]]),
        UV,
        &image[0],
        Some([64, 128, 191, 255]),
        [5, 0, 43, 29],
    );
    // Aliased target VA reuses the same canonical backing and host target contents.
    remap
        .map(GpuAddress(0xa2000), GuestAddress(RT as u64), SIZE as u64)
        .unwrap();
    let uploaded = r.counters.upload_bytes;
    run(
        &mut r,
        &mut c,
        &[0x20010201, 0xa2000, DRAW[0], DRAW[1]],
        &remap,
        &mut data,
        &mut changes,
    )
    .unwrap();
    assert_eq!(uploaded, r.counters.upload_bytes);
    // Narrow viewport comes from scale/translate registers, not host arguments.
    background(&mut data, RT, BG);
    changes.mark_write(GuestAddress(RT as u64), SIZE).unwrap();
    let f = run(
        &mut r,
        &mut c,
        &[
            0x20010280,
            12f32.to_bits(),
            0x20010283,
            12f32.to_bits(),
            DRAW[0],
            DRAW[1],
        ],
        &remap,
        &mut data,
        &mut changes,
    )
    .unwrap();
    check(
        &f.rgba,
        moved.map(|p| [(p[0] + 1.) * 24. / 47. - 1., -p[1]]),
        UV,
        &image[0],
        Some([64, 128, 191, 255]),
        [5, 0, 43, 29],
    );
    println!(
        "methods={}, final={:?}",
        c.engine.method_writes(),
        r.counters
    );
    // Unsupported RT format faults before any draw and cannot reuse stale state.
    let before = r.counters.submissions;
    assert!(
        r.submit_pushbuffer(
            &mut c,
            &[0x20010204, 0xd6, DRAW[0], DRAW[1]],
            &m,
            &mut SliceMemoryMut {
                base: GuestAddress(0),
                bytes: &mut data
            },
            &mut changes
        )
        .is_err()
    );
    assert!(c.is_faulted());
    assert!(
        r.submit_pushbuffer(
            &mut c,
            &DRAW,
            &m,
            &mut SliceMemoryMut {
                base: GuestAddress(0),
                bytes: &mut data
            },
            &mut changes
        )
        .is_err()
    );
    assert_eq!(before, r.counters.submissions);
    for bad in 0..3 {
        let mut channel = Channel::default();
        run(
            &mut r,
            &mut channel,
            &words[..words.len() - 2],
            &m,
            &mut data,
            &mut changes,
        );
        let mut command = Vec::new();
        if bad == 0 {
            command.extend_from_slice(&[0x20010201, 0xdead0000]);
        }
        if bad == 1 {
            command.extend_from_slice(&[0x200107c1, 0x70040]);
        }
        command.extend_from_slice(&DRAW);
        let size = if bad == 2 { RT + SIZE - 1 } else { data.len() };
        assert!(
            r.submit_pushbuffer(
                &mut channel,
                &command,
                &m,
                &mut SliceMemoryMut {
                    base: GuestAddress(0),
                    bytes: &mut data[..size]
                },
                &mut changes
            )
            .is_err()
        );
        assert!(channel.is_faulted());
        assert_eq!(before, r.counters.submissions);
    }
}
