#![cfg(feature = "wgpu-backend")]
mod common;
use wl_gpu::{backend::Renderer, maxwell::Channel, memory::*, texture::TextureChanges};
const SIZE: usize = 208 * 29;
const BG: [u8; 4] = [11, 17, 23, 255];
fn method(words: &mut Vec<u32>, m: u32, v: u32) {
    words.extend([0x20010000 | (m / 4), v]);
}
fn draw(words: &mut Vec<u32>, first: u32, count: u32) {
    method(words, 0x1618, 4);
    method(words, 0x17dc, first);
    method(words, 0x17e0, count);
    method(words, 0x1614, 0);
}
fn check(pixels: &[u8], half: bool) {
    // Independent axis-aligned quad: clip [-0.75,0.5] x [-0.5,0.75].
    // The shared diagonal is y=x+0.25; no edge-tie pixels are asserted.
    let mut inside = 0;
    let mut outside = 0;
    for y in 0..29 {
        for x in 0..52 {
            let px = (x as f64 + 0.5) * 2. / 47. - 1.;
            let py = 1. - (y as f64 + 0.5) * 2. / 29.;
            if [px + 0.75, px - 0.5, py + 0.5, py - 0.75, py - px - 0.25]
                .iter()
                .any(|n| n.abs() < 0.01)
            {
                continue;
            }
            let covered = x < 47
                && px > -0.75
                && px < 0.5
                && py > -0.5
                && py < 0.75
                && (!half || py - px - 0.25 < 0.);
            let expected = if covered { [64, 128, 191, 255] } else { BG };
            let p = &pixels[(y * 52 + x) * 4..(y * 52 + x + 1) * 4];
            assert_eq!(p, expected, "{x},{y}, clip{px},{py}");
            if covered {
                inside += 1;
            } else {
                outside += 1;
            }
        }
    }
    assert!(inside > 100 && outside > 700);
}
#[test]
#[ignore = "requires actual GPU; run explicitly"]
fn b197_indexed_quad_offsets_base_mutation_and_remap() {
    let mut data = vec![0u8; 0xe000];
    for (off, bin) in [
        (
            0x4100,
            include_bytes!("fixtures/maxwell/quarters.bin").as_slice(),
        ),
        (
            0x4200,
            include_bytes!("fixtures/maxwell/vertex.bin").as_slice(),
        ),
    ] {
        data[off..off + bin.len()].copy_from_slice(bin);
    }
    // Dummy vertex zero must never be fetched; base +1 selects four shared vertices.
    for (n, pos) in [
        [8., 8.],
        [-0.75, -0.5],
        [0.5, -0.5],
        [0.5, 0.75],
        [-0.75, 0.75],
    ]
    .iter()
    .enumerate()
    {
        for (c, v) in [pos[0], pos[1], 0f32, 1., 0.25, 0.75].iter().enumerate() {
            data[0x3000 + n * 24 + c * 4..0x3000 + n * 24 + c * 4 + 4]
                .copy_from_slice(&v.to_le_bytes());
        }
    }
    for (n, i) in [65535u16, 0, 1, 2, 0, 2, 3].iter().enumerate() {
        data[0x6000 + n * 2..0x6002 + n * 2].copy_from_slice(&i.to_le_bytes());
    }
    for p in data[0x8000..0x8000 + SIZE].chunks_exact_mut(4) {
        p.copy_from_slice(&BG);
    }
    let mut map = AddressMap::default();
    for (g, a, n) in [
        (0x50000, 0x4000, 0x1000),
        (0x70000, 0x3000, 120),
        (0x80000, 0x6000, 14),
        (0x90000, 0x8000, SIZE as u64),
    ] {
        map.map(GpuAddress(g), GuestAddress(a), n).unwrap();
    }
    let mut setup = common::hardware::setup();
    method(&mut setup, 0x2144, 0x100); // real constant binary fragment, no texture needed
    method(&mut setup, 0x1f04, 0x70077);
    for (m, v) in [
        (0x17c8, 0),
        (0x17cc, 0x80000),
        (0x17d0, 0),
        (0x17d4, 0x8000d),
        (0x17d8, 1),
        (0x1434, 1),
        (0x1438, 0),
        (0x1644, 0),
    ] {
        method(&mut setup, m, v);
    }
    draw(&mut setup, 1, 6);
    let mut r = Renderer::new().unwrap();
    let mut channel = Channel::default();
    let mut changes = TextureChanges::new(1);
    let mut run =
        |r: &mut Renderer, words: &[u32], data: &mut [u8], changes: &mut TextureChanges| {
            r.submit_pushbuffer(
                &mut channel,
                words,
                &map,
                &mut SliceMemoryMut {
                    base: GuestAddress(0),
                    bytes: data,
                },
                changes,
            )
            .unwrap()
            .unwrap()
        };
    let frame = run(&mut r, &setup, &mut data, &mut changes);
    check(&frame.rgba, false);
    let before = r.counters;
    let mut again = Vec::new();
    draw(&mut again, 1, 6);
    let frame = run(&mut r, &again, &mut data, &mut changes);
    check(&frame.rgba, false);
    assert_eq!(r.counters.index_upload_bytes, before.index_upload_bytes);
    assert_eq!(r.counters.pipeline_allocations, before.pipeline_allocations);
    // Remove the second triangle through guest indices; reset destination explicitly.
    for (n, i) in [0u16, 1, 2].iter().enumerate() {
        data[0x6008 + n * 2..0x600a + n * 2].copy_from_slice(&i.to_le_bytes());
    }
    changes.mark_write(GuestAddress(0x6008), 6).unwrap();
    for p in data[0x8000..0x8000 + SIZE].chunks_exact_mut(4) {
        p.copy_from_slice(&BG);
    }
    changes.mark_write(GuestAddress(0x8000), SIZE).unwrap();
    let frame = run(&mut r, &again, &mut data, &mut changes);
    check(&frame.rgba, true);
    assert_eq!(r.counters.pipeline_allocations, before.pipeline_allocations);
    // Same GPU VA, new backing and 32-bit element format; negative base is valid.
    for (n, i) in [u32::MAX, 2, 3, 4, 2, 4, 5].iter().enumerate() {
        data[0x6100 + n * 4..0x6104 + n * 4].copy_from_slice(&i.to_le_bytes());
    }
    let mut map = AddressMap::default();
    for (g, a, n) in [
        (0x50000, 0x4000, 0x1000),
        (0x70000, 0x3000, 120),
        (0x90000, 0x8000, SIZE as u64),
    ] {
        map.map(GpuAddress(g), GuestAddress(a), n).unwrap();
    }
    map.map(GpuAddress(0x80000), GuestAddress(0x6100), 28)
        .unwrap();
    let mut words = Vec::new();
    method(&mut words, 0x17d4, 0x8001b);
    method(&mut words, 0x17d8, 2);
    method(&mut words, 0x1434, u32::MAX);
    draw(&mut words, 1, 6);
    let frame = r
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
    check(&frame.rgba, false);
    // A second GPU VA aliases the same index backing: no reread or upload.
    map.map(GpuAddress(0xe0000), GuestAddress(0x6100), 28)
        .unwrap();
    let before = r.counters;
    let mut alias = Vec::new();
    method(&mut alias, 0x17cc, 0xe0000);
    method(&mut alias, 0x17d4, 0xe001b);
    draw(&mut alias, 1, 6);
    let frame = r
        .submit_pushbuffer(
            &mut channel,
            &alias,
            &map,
            &mut SliceMemoryMut {
                base: GuestAddress(0),
                bytes: &mut data,
            },
            &mut changes,
        )
        .unwrap()
        .unwrap();
    check(&frame.rgba, false);
    assert_eq!(r.counters.index_guest_bytes, before.index_guest_bytes);
    assert_eq!(r.counters.index_upload_bytes, before.index_upload_bytes);
    // An odd U16 count has six source bytes and an eight-byte host upload.
    for (n, i) in [0u16, 1, 2].iter().enumerate() {
        data[0x6200 + n * 2..0x6202 + n * 2].copy_from_slice(&i.to_le_bytes());
    }
    map.map(GpuAddress(0xf0000), GuestAddress(0x6200), 6)
        .unwrap();
    for p in data[0x8000..0x8000 + SIZE].chunks_exact_mut(4) {
        p.copy_from_slice(&BG);
    }
    changes.mark_write(GuestAddress(0x8000), SIZE).unwrap();
    let mut words = Vec::new();
    for (m, v) in [
        (0x17cc, 0xf0000),
        (0x17d4, 0xf0005),
        (0x17d8, 1),
        (0x1434, 1),
    ] {
        method(&mut words, m, v);
    }
    draw(&mut words, 0, 3);
    let before = r.counters;
    let frame = r
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
    check(&frame.rgba, true);
    assert_eq!(r.counters.index_guest_bytes - before.index_guest_bytes, 6);
    assert_eq!(r.counters.index_upload_bytes - before.index_upload_bytes, 8);
    assert_eq!(r.counters.pipeline_allocations, before.pipeline_allocations);
    // Invalid resulting vertex index faults before submission/writeback.
    let before = r.counters;
    let framebuffer = data[0x8000..0x8000 + SIZE].to_vec();
    data[0x6200..0x6202].copy_from_slice(&999u16.to_le_bytes());
    changes.mark_write(GuestAddress(0x6200), 2).unwrap();
    assert!(
        r.submit_pushbuffer(
            &mut channel,
            &words,
            &map,
            &mut SliceMemoryMut {
                base: GuestAddress(0),
                bytes: &mut data
            },
            &mut changes
        )
        .is_err()
    );
    assert_eq!(r.counters.submissions, before.submissions);
    assert_eq!(&data[0x8000..0x8000 + SIZE], framebuffer);
    assert!(
        r.submit_pushbuffer(
            &mut channel,
            &[],
            &map,
            &mut SliceMemoryMut {
                base: GuestAddress(0),
                bytes: &mut data
            },
            &mut changes
        )
        .is_err()
    );
    println!("adapter={:?} counters={:?}", r.capabilities, r.counters);
}
