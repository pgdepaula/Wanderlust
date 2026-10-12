use wl_gpu::{fifo::MethodWrite, maxwell::*, memory::GpuAddress, raster::*};
fn set(e: &mut Maxwell3d, m: u16, v: u32) -> Result<MethodEffect, MethodError> {
    e.apply(MethodWrite {
        subchannel: 0,
        method: m,
        value: v,
    })
}
fn setup() -> Maxwell3d {
    let mut e = Maxwell3d::default();
    set(&mut e, 0, 0xb197).unwrap();
    for (m, v) in [
        (0x12cc, 1),
        (0x12e8, 1),
        (0x1538, 1),
        (0x130c, 0x201),
        (0xfe0, 0),
        (0xfe4, 0xc0000),
        (0xfe8, 0xa),
        (0xfec, 0x10),
        (0xff0, 2048),
        (0x1228, 52),
        (0x122c, 29),
        (0x1230, 0x10001),
        (0x179c, 0),
    ] {
        set(&mut e, m, v).unwrap();
    }
    e
}
#[test]
fn zf32_descriptor_size_stage_and_explicit_selection() {
    let mut e = setup();
    let d = e.depth_target().unwrap().unwrap();
    assert_eq!(
        d,
        DepthTarget {
            address: GpuAddress(0xc0000),
            width: 52,
            height: 29,
            block_height_log2: 1
        }
    );
    assert_eq!(d.size().unwrap(), 8192);
    for f in (1..=8).chain(0x200..=0x207) {
        set(&mut e, 0x130c, f).unwrap();
        assert!(e.depth_target().is_ok());
    }
    for m in [0x12cc, 0x1538] {
        let mut e = setup();
        set(&mut e, m, 0).unwrap();
        assert!(e.depth_target().is_err());
    }
    for log in 0..=5 {
        let mut e = setup();
        set(&mut e, 0xfec, log << 4).unwrap();
        set(&mut e, 0xff0, 0).unwrap();
        assert!(e.depth_target().is_ok());
    }
}
#[test]
fn depth_invalid_fields_do_not_replace_valid_descriptor() {
    let mut e = setup();
    let before = e.depth_target().unwrap();
    for (m, v) in [
        (0xfe0, 256),
        (0xfe4, 1),
        (0xfe8, 0x13),
        (0xfec, 0x60),
        (0xfec, 1),
        (0xfec, 0x100),
        (0x1228, 0),
        (0x122c, 0),
        (0x1230, 2),
        (0x179c, 1),
        (0x130c, 0),
        (0x12e8, 2),
        (0x1538, 2),
        (0xfe1, 0),
    ] {
        assert!(set(&mut e, m, v).is_err(), "{m:x}");
        assert_eq!(e.depth_target().unwrap(), before);
    }
    set(&mut e, 0xff0, 1).unwrap();
    assert!(e.depth_target().is_err());
    let mut e = setup();
    set(&mut e, 0xfe0, 255).unwrap();
    set(&mut e, 0xfe4, 0xffffff00).unwrap();
    assert!(e.depth_target().is_err());
    let mut e = Maxwell3d::default();
    set(&mut e, 0, 0xb197).unwrap();
    set(&mut e, 0x12cc, 1).unwrap();
    assert!(e.depth_target().is_err());
}
