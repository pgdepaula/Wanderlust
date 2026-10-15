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
            format: DepthFormat::Float32,
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
        (0xfe8, 0x12),
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

#[path = "common/gob.rs"]
mod gob;
#[test]
fn z16_native_descriptor_and_exact_two_byte_gob_layout() {
    let mut e = setup();
    set(&mut e, 0xfe8, 0x13).unwrap();
    set(&mut e, 0xff0, 1024).unwrap();
    let d = e.depth_target().unwrap().unwrap();
    assert_eq!(d.format, DepthFormat::Unorm16);
    assert_eq!(d.size().unwrap(), 4096);
    for (width, height) in [
        (1, 1),
        (7, 9),
        (16, 17),
        (33, 13),
        (64, 8),
        (65, 65),
        (127, 31),
    ] {
        for log in 0..=5 {
            let d = DepthTarget {
                width,
                height,
                block_height_log2: log,
                ..d
            };
            let layout = d.block_linear().unwrap();
            let row = width as usize * 2;
            let h = height as usize;
            let expected_size = row.div_ceil(64) * h.div_ceil(8usize << log) * (512usize << log);
            assert_eq!(d.size().unwrap(), expected_size);
            let mut tiled = vec![0xad; expected_size];
            let mut linear = vec![0; row * h];
            let mut visible = vec![false; expected_size];
            for y in 0..h {
                for x in 0..row {
                    let value = ((x * 73 + y * 37 + (x ^ y) * 19) % 256) as u8;
                    let n = gob::address_bytes(x, y, row, log);
                    assert!(!visible[n]);
                    visible[n] = true;
                    tiled[n] = value;
                    linear[y * row + x] = value;
                }
            }
            let mut counters = wl_gpu::Counters::default();
            assert_eq!(layout.to_linear(&tiled, &mut counters).unwrap(), linear);
            let mut padded = layout.padded_linear(&tiled, &mut counters).unwrap();
            for v in &mut linear {
                *v ^= 0xff;
            }
            let encoded = layout
                .from_padded_linear(&mut padded, &linear, &mut counters)
                .unwrap();
            for y in 0..h {
                for x in 0..row {
                    assert_eq!(
                        encoded[gob::address_bytes(x, y, row, log)],
                        linear[y * row + x]
                    );
                }
            }
            for (n, used) in visible.iter().enumerate() {
                if !used {
                    assert_eq!(encoded[n], 0xad, "padding {n}");
                }
            }
            assert_eq!(layout.to_linear(&encoded, &mut counters).unwrap(), linear);
            assert!(
                layout
                    .to_linear(&tiled[..tiled.len() - 1], &mut counters)
                    .is_err()
            );
        }
    }
}
#[test]
fn z16_invalid_geometry_and_format_changes_preserve_state() {
    let mut e = setup();
    set(&mut e, 0xfe8, 0x13).unwrap();
    set(&mut e, 0xff0, 0).unwrap();
    let before = e.depth_target().unwrap().unwrap();
    for value in [0, 0x12, 0x14, 0x113, u32::MAX] {
        assert!(set(&mut e, 0xfe8, value).is_err());
        assert_eq!(e.depth_target().unwrap(), Some(before));
    }
    for d in [
        DepthTarget { width: 0, ..before },
        DepthTarget {
            height: 0,
            ..before
        },
        DepthTarget {
            width: u32::MAX,
            ..before
        },
        DepthTarget {
            block_height_log2: 6,
            ..before
        },
        DepthTarget {
            address: GpuAddress((1u64 << 40) - 256),
            ..before
        },
    ] {
        assert!(d.size().is_err());
    }
    set(&mut e, 0xfe8, 0xa).unwrap();
    assert_eq!(
        e.depth_target().unwrap().unwrap().format,
        DepthFormat::Float32
    );
    assert_eq!(e.depth_target().unwrap().unwrap().size().unwrap(), 8192);
}
