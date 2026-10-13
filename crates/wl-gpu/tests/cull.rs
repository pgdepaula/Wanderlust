mod common;
use wl_gpu::{fifo::MethodWrite, maxwell::*, raster::*};
fn set(e: &mut Maxwell3d, m: u16, v: u32) -> Result<MethodEffect, MethodError> {
    e.apply(MethodWrite {
        subchannel: 0,
        method: m,
        value: v,
    })
}
fn ready() -> Maxwell3d {
    let mut c = Channel::default();
    for word in common::hardware::setup() {
        c.push_word(word).unwrap();
    }
    c.engine
}
#[test]
fn cull_state_requires_explicit_enabled_interface_and_normalizes_disabled_state() {
    let mut e = ready();
    assert_eq!(e.rasterization().unwrap().1.cull, Culling::Disabled);
    set(&mut e, 0x1918, 1).unwrap();
    assert!(e.rasterization().is_err());
    set(&mut e, 0x191c, 0x901).unwrap();
    assert!(e.rasterization().is_err());
    set(&mut e, 0x1920, 0x405).unwrap();
    assert_eq!(
        e.rasterization().unwrap().1.cull,
        Culling::Enabled {
            face: CullFace::Back,
            front: FrontFace::CounterClockwise
        }
    );
    set(&mut e, 0x191c, 0x900).unwrap();
    set(&mut e, 0x1920, 0x404).unwrap();
    assert_eq!(
        e.rasterization().unwrap().1.cull,
        Culling::Enabled {
            face: CullFace::Front,
            front: FrontFace::Clockwise
        }
    );
    set(&mut e, 0x1918, 0).unwrap();
    assert_eq!(e.rasterization().unwrap().1.cull, Culling::Disabled);
}
#[test]
fn cull_invalid_direct_values_do_not_change_valid_state() {
    let mut e = ready();
    for (m, v) in [(0x1918, 1), (0x191c, 0x901), (0x1920, 0x405)] {
        set(&mut e, m, v).unwrap();
    }
    let state = e.rasterization().unwrap().1;
    for (m, v) in [
        (0x1918, 2),
        (0x191c, 0),
        (0x191c, 0x902),
        (0x1920, 0x408),
        (0x1920, 0x406),
        (0x1308, 1),
        (0x1919, 0),
    ] {
        assert!(set(&mut e, m, v).is_err());
        assert_eq!(e.rasterization().unwrap().1, state);
    }
}
#[test]
fn viewport_y_sign_does_not_reinterpret_guest_front_face() {
    let mut e = ready();
    for (m, v) in [(0x1918, 1), (0x191c, 0x901), (0x1920, 0x405)] {
        set(&mut e, m, v).unwrap();
    }
    let negative = e.rasterization().unwrap().1;
    set(&mut e, 0xa04, 14.5f32.to_bits()).unwrap();
    let positive = e.rasterization().unwrap().1;
    assert!(!negative.flip_y && positive.flip_y);
    assert_eq!(negative.cull, positive.cull);
}
