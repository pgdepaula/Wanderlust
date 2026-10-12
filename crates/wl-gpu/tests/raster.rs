use wl_gpu::{fifo::MethodWrite, maxwell::*, memory::*, raster::*};
fn set(e: &mut Maxwell3d, m: u16, v: u32) -> Result<MethodEffect, MethodError> {
    e.apply(MethodWrite {
        subchannel: 0,
        method: m,
        value: v,
    })
}
fn ready() -> Maxwell3d {
    let mut e = Maxwell3d::default();
    set(&mut e, 0, 0xb197).unwrap();
    for &(m, v) in FIXED_RASTER_STATE {
        set(&mut e, m, v).unwrap();
    }
    for (n, v) in [0, 0x90000, 208, 29, 0xd5, 0x1000, 1, 0, 0]
        .into_iter()
        .enumerate()
    {
        set(&mut e, 0x800 + n as u16 * 4, v).unwrap();
    }
    for (n, v) in [23.5f32, -14.5, 1., 23.5, 14.5, 0.].into_iter().enumerate() {
        set(&mut e, 0xa00 + n as u16 * 4, v.to_bits()).unwrap();
    }
    for (m, v) in [
        (0xff4, 47 << 16),
        (0xff8, 29 << 16),
        (0xc00, 47 << 16),
        (0xc04, 29 << 16),
        (0xe00, 1),
        (0xe04, 47 << 16),
        (0xe08, 29 << 16),
    ] {
        set(&mut e, m, v).unwrap();
    }
    e
}
#[test]
fn documented_pitch_raster_and_compact_draw_fields() {
    let mut e = ready();
    let (t, r) = e.rasterization().unwrap();
    assert_eq!(
        (t.width, t.height, t.pitch, t.size().unwrap()),
        (52, 29, 208, 6032)
    );
    assert_eq!(t.address, GpuAddress(0x90000));
    assert_eq!(r.viewport, [0., 0., 47., 29., 0., 1.]);
    assert_eq!(r.scissor, [0, 0, 47, 29]);
    assert!(!r.flip_y);
    assert_eq!(
        set(&mut e, 0x1214, 0x40030002).unwrap(),
        MethodEffect::Draw { first: 2, count: 3 }
    );
    for v in [0, 0x40020000, 0x50030000] {
        assert!(set(&mut e, 0x1214, v).is_err());
    }
    set(&mut e, 0xe04, (43 << 16) | 5).unwrap();
    assert_eq!(e.rasterization().unwrap().1.scissor, [5, 0, 38, 29]);
    set(&mut e, 0xa04, 14.5f32.to_bits()).unwrap();
    assert!(e.rasterization().unwrap().1.flip_y);
}
#[test]
fn raster_invalid_incomplete_bounds_and_unsupported_formats() {
    let mut e = Maxwell3d::default();
    set(&mut e, 0, 0xb197).unwrap();
    assert!(e.rasterization().is_err());
    for (m, v) in [
        (0x800, 256),
        (0x804, 1),
        (0x808, 0),
        (0x808, 207),
        (0x80c, 0),
        (0x810, 0xd6),
        (0x814, 0),
        (0x818, 2),
        (0x81c, 1),
        (0x820, 1),
        (0xa00, f32::NAN.to_bits()),
        (0x12cc, 1),
        (0xe00, 0),
    ] {
        assert!(set(&mut ready(), m, v).is_err(), "{m:x}");
    }
    for (m, v) in [
        (0xa00, 0),
        (0xa04, 0),
        (0xa00, 11.75f32.to_bits()),
        (0xa08, 2f32.to_bits()),
        (0xa0c, 100f32.to_bits()),
        (0xe04, 0),
        (0xc00, (2 << 16) | 49),
        (0x800, 255),
        (0xff8, 0),
    ] {
        let mut e = ready();
        set(&mut e, m, v).unwrap();
        if m == 0x800 {
            set(&mut e, 0x804, 0xffffff00).unwrap();
        }
        assert!(e.rasterization().is_err(), "{m:x}");
    }
}
#[test]
fn guest_write_is_checked_and_atomic() {
    let mut bytes = [5u8; 8];
    let mut m = SliceMemoryMut {
        base: GuestAddress(100),
        bytes: &mut bytes,
    };
    m.write(GuestAddress(102), &[1, 2, 3, 4]).unwrap();
    let before = m.bytes.to_vec();
    for (a, n) in [(99, 1), (107, 2), (u64::MAX, 2)] {
        assert!(m.validate_write(GuestAddress(a), n).is_err());
        assert!(m.write(GuestAddress(a), &vec![0; n]).is_err());
        assert_eq!(m.bytes, before);
    }
    assert_eq!(m.read(GuestAddress(102), 4).unwrap(), [1, 2, 3, 4]);
}

#[test]
fn channel_truncation_fault_is_latched() {
    let mut c = Channel::default();
    assert!(c.fifo.push_word(0x20010000).unwrap().is_none());
    assert!(c.finish().is_err());
    assert!(c.is_faulted());
    assert!(c.finish().is_err());
}
