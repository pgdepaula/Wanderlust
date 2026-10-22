use wl_gpu::{fifo::MethodWrite, maxwell::*, raster::*};
fn set(e: &mut Maxwell3d, m: u16, v: u32) -> Result<MethodEffect, MethodError> {
    e.apply(MethodWrite {
        subchannel: 0,
        method: m,
        value: v,
    })
}
fn minimal() -> Maxwell3d {
    let mut e = Maxwell3d::default();
    for (m, v) in [
        (0, 0xb197),
        (0x138, 1),
        (0x15d0, 0),
        (0x121c, 1),
        (0x10f8, 0),
        (0xff4, 7 << 16),
        (0xff8, 9 << 16),
    ] {
        set(&mut e, m, v).unwrap();
    }
    e
}
fn color(e: &mut Maxwell3d) {
    for (n, v) in [0, 0x90000, 28, 9, 0xd5, 0x1000, 1, 0, 0]
        .into_iter()
        .enumerate()
    {
        set(e, 0x800 + n as u16 * 4, v).unwrap();
    }
    for (n, v) in [0.25f32, 0.5, 0.75, 1.].into_iter().enumerate() {
        set(e, 0xd80 + n as u16 * 4, v.to_bits()).unwrap();
    }
}
fn depth(e: &mut Maxwell3d) {
    for (m, v) in [
        (0xfe0, 0),
        (0xfe4, 0xd0000),
        (0xfe8, 0xa),
        (0xfec, 0x10),
        (0xff0, 0),
        (0x1228, 7),
        (0x122c, 9),
        (0x1230, 1),
        (0x179c, 0),
        (0x1538, 1),
        (0x12cc, 0),
        (0x12e8, 0),
        (0xd90, 0x3e800000),
    ] {
        set(e, m, v).unwrap();
    }
}
#[test]
fn clear_descriptor_ignores_missing_draw_state_and_draw_masks() {
    let mut e = minimal();
    color(&mut e);
    set(&mut e, 0x1a00, 0).unwrap();
    set(&mut e, 0x1360, 1).unwrap();
    assert!(e.rasterization().is_err());
    let clear = e.clear_surface(true, false).unwrap();
    let (t, v) = clear.color.unwrap();
    assert_eq!(
        (t.width, t.height, t.layout),
        (7, 9, ColorLayout::Pitch { bytes_per_row: 28 })
    );
    assert_eq!(v, [0.25, 0.5, 0.75, 1.]);
    assert!(clear.depth.is_none());
}
#[test]
fn depth_only_clear_does_not_require_color_or_depth_test_write() {
    let mut e = minimal();
    depth(&mut e);
    assert!(e.depth_target().is_err());
    let (t, v) = e.clear_surface(false, true).unwrap().depth.unwrap();
    assert_eq!((t.width, t.height, t.block_height_log2, v), (7, 9, 1, 0.25));
    color(&mut e);
    assert!(e.clear_surface(true, true).is_ok());
    set(&mut e, 0x1228, 8).unwrap();
    assert!(e.clear_surface(true, true).is_err());
}
#[test]
fn clear_flags_values_and_partial_surface_fail_explicitly() {
    let mut e = minimal();
    color(&mut e);
    for v in [0, 2, 4, 0x7c, 0x400, 0x80000000] {
        assert!(set(&mut e, 0x19d0, v).is_err());
    }
    for v in [16, 256, 4096] {
        set(&mut e, 0x10f8, v).unwrap();
        assert!(e.clear_surface(true, false).is_err());
    }
    set(&mut e, 0x10f8, 1).unwrap();
    assert!(e.clear_surface(true, false).is_ok()); // No stencil clear is requested.
    set(&mut e, 0x10f8, 0).unwrap();
    for v in [2, 4, 0x200, 0x10000, u32::MAX] {
        assert!(set(&mut e, 0x10f8, v).is_err());
    }
    assert!(e.clear_surface(true, false).is_ok());
    set(&mut e, 0xff4, 6 << 16).unwrap();
    assert!(e.clear_surface(true, false).is_err());
    set(&mut e, 0xff4, 7 << 16).unwrap();
    for v in [0x7fc00000, 0x7f800000, 0xbf800000, 0x40000000] {
        set(&mut e, 0xd80, v).unwrap();
        assert!(e.clear_surface(true, false).is_err());
    }
    depth(&mut e);
    assert!(e.clear_surface(false, true).is_ok());
    for v in [1, 0x80000000, 0x7fc00000, 0x40000000] {
        set(&mut e, 0xd90, v).unwrap();
        assert!(e.clear_surface(false, true).is_err());
    }
}
#[test]
fn literal_incrementing_clear_values_continue_across_packet_chunks() {
    let mut c = Channel::default();
    for word in [0x20010000, 0xb197, 0x20050360, 0x3e800000, 0x3f000000] {
        c.push_word(word).unwrap();
    }
    for word in [
        0x3f400000, 0x3f800000, 0x3e800000, 0x2001043e, 0, 0x20010674,
    ] {
        c.push_word(word).unwrap();
    }
    assert_eq!(
        c.push_word(0x3d).unwrap(),
        MethodEffect::Clear {
            color: true,
            depth: true
        }
    );
    c.finish().unwrap();
    assert!(c.engine.clear_surface(true, true).is_err());
}

#[test]
fn full_scissor_clear_controls_preserve_full_target_and_reject_partial_regions() {
    let mut e = minimal();
    color(&mut e);
    let expected = e.clear_surface(true, false).unwrap();
    set(&mut e, 0x10f8, 0x101).unwrap();
    assert!(e.clear_surface(true, false).is_err());
    for (m, v) in [(0xe00, 1), (0xe04, 7 << 16), (0xe08, 9 << 16)] {
        set(&mut e, m, v).unwrap();
    }
    assert_eq!(e.clear_surface(true, false).unwrap(), expected);
    for value in [0x100, 0x101, 0, 1] {
        set(&mut e, 0x10f8, value).unwrap();
        assert_eq!(e.clear_surface(true, false).unwrap(), expected);
    }
    set(&mut e, 0x10f8, 0x101).unwrap();
    for value in [(6 << 16), (7 << 16) | 1, (8 << 16)] {
        set(&mut e, 0xe04, value).unwrap();
        assert!(e.clear_surface(true, false).is_err());
    }
    set(&mut e, 0xe04, 7 << 16).unwrap();
    assert_eq!(e.clear_surface(true, false).unwrap(), expected);
    set(&mut e, 0x10f8, 0x1111).unwrap();
    assert!(e.clear_surface(true, false).is_err());
}
