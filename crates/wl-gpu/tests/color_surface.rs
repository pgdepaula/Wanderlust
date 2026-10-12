mod common;
#[path = "common/gob.rs"]
mod gob;
use wl_gpu::{
    Counters, format::Format, layout::BlockLinear2d, maxwell::Channel, memory::GpuAddress,
    raster::*,
};
#[test]
fn independent_byte_oracle_bidirectional_padding_and_exact_roundtrips() {
    for (w, h) in [
        (1, 1),
        (7, 9),
        (16, 17),
        (33, 13),
        (64, 8),
        (65, 65),
        (127, 31),
    ] {
        for log2 in 0..=5 {
            let size = gob::size(w, h, log2);
            let original: Vec<u8> = (0..size).map(|n| (n * 37 + n / 17) as u8).collect();
            let pixels = common::rgba_mip(w as u32, h as u32, 3);
            let expected = gob::encode(&pixels, w, h, log2, &original);
            let layout = BlockLinear2d::new(w as u32, h as u32, Format::Rgba8Unorm, log2).unwrap();
            assert_eq!(layout.tiled_size, size);
            let mut c = Counters::default();
            assert_eq!(
                layout
                    .to_tiled_preserving(&pixels, &original, &mut c)
                    .unwrap(),
                expected
            );
            assert_eq!(layout.to_linear(&expected, &mut c).unwrap(), pixels);
            let source = layout.to_linear(&original, &mut c).unwrap();
            assert_eq!(
                layout
                    .to_tiled_preserving(&source, &original, &mut c)
                    .unwrap(),
                original
            );
            let mut used = vec![false; size];
            for y in 0..h {
                for x in 0..w * 4 {
                    let a = gob::address(x, y, w, log2);
                    assert!(!used[a]);
                    used[a] = true;
                }
            }
            for (n, used) in used.into_iter().enumerate() {
                if !used {
                    assert_eq!(expected[n], original[n]);
                }
            }
            assert!(gob::address(w * 4 - 1, h - 1, w, log2) < size);
            assert!(
                layout
                    .to_tiled_preserving(&pixels[..pixels.len() - 1], &original, &mut c)
                    .is_err()
            );
            assert!(
                layout
                    .to_tiled_preserving(&pixels, &original[..size - 1], &mut c)
                    .is_err()
            );
        }
    }
    // Literal within-GOB landmarks, independent of a converter roundtrip.
    for (x, y, offset) in [
        (0, 0, 0),
        (16, 0, 32),
        (32, 0, 256),
        (0, 1, 16),
        (0, 2, 64),
        (64, 0, 1024),
        (0, 8, 512),
    ] {
        assert_eq!(gob::address(x, y, 33, 1), offset);
    }
}
fn state(width: u32, height: u32, memory: u32) -> Channel {
    let mut c = Channel::default();
    for w in common::hardware::setup() {
        c.push_word(w).unwrap();
    }
    for (m, v) in [
        (0x808, width),
        (0x80c, height),
        (0x814, memory),
        (0xa00, 0.5f32.to_bits()),
        (0xa04, (-0.5f32).to_bits()),
        (0xa0c, 0.5f32.to_bits()),
        (0xa10, 0.5f32.to_bits()),
    ] {
        c.push_word(0x20010000 | (m / 4)).unwrap();
        c.push_word(v).unwrap();
    }
    c
}
#[test]
fn b197_layout_fields_width_units_and_rejected_variants() {
    for log2 in 0..=5 {
        let c = state(33, 13, log2 << 4);
        let (t, _) = c.engine.rasterization().unwrap();
        assert_eq!(t.width, 33);
        assert_eq!(t.height, 13);
        assert_eq!(
            t.layout,
            ColorLayout::BlockLinear {
                block_height_log2: log2 as u8
            }
        );
        assert_eq!(t.size().unwrap(), gob::size(33, 13, log2 as u8));
        let mut c = c;
        for value in [0, (t.size().unwrap() / 4) as u32] {
            c.push_word(0x20010207).unwrap();
            c.push_word(value).unwrap();
            assert!(c.engine.rasterization().is_ok());
        }
        c.push_word(0x20010207).unwrap();
        c.push_word(1).unwrap();
        assert!(c.engine.rasterization().is_err());
    }
    let c = state(132, 13, 0x1000);
    let (t, _) = c.engine.rasterization().unwrap();
    assert_eq!(t.width, 33);
    assert_eq!(t.size().unwrap(), 132 * 13);
    assert_eq!(t.layout, ColorLayout::Pitch { bytes_per_row: 132 });
    assert!(state(133, 13, 0x1000).engine.rasterization().is_err());
    for memory in [1, 0x60, 0x100, 0x10000, 0x1010, 0x80000000] {
        let mut c = Channel::default();
        for w in [0x20010000, 0xb197, 0x20010205] {
            c.push_word(w).unwrap();
        }
        assert!(c.push_word(memory).is_err());
    }
}
#[test]
fn surface_limits_and_full_footprint_address_range() {
    for (w, h, log) in [(0, 1, 0), (1, 0, 0), (1, 1, 6), (u32::MAX, u32::MAX, 5)] {
        assert!(BlockLinear2d::new(w, h, Format::Rgba8Unorm, log).is_err());
    }
    let mut t = ColorTarget {
        address: GpuAddress((1 << 40) - 256),
        width: 1,
        height: 1,
        layout: ColorLayout::BlockLinear {
            block_height_log2: 0,
        },
    };
    assert!(t.size().is_err());
    t.address = GpuAddress((1 << 40) - 512);
    assert_eq!(t.size().unwrap(), 512);
    t.width = 4096;
    t.height = 4096;
    assert!(t.size().is_err());
    t.width = u32::MAX;
    assert!(t.linear_pitch().is_err());
}
