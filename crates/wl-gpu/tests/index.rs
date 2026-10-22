use wl_gpu::{
    fifo::MethodWrite,
    maxwell::{Maxwell3d, MethodEffect},
    memory::GpuAddress,
};
fn set(e: &mut Maxwell3d, m: u16, v: u32) -> bool {
    e.apply(MethodWrite {
        subchannel: 0,
        method: m,
        value: v,
    })
    .is_ok()
}
fn configured() -> Maxwell3d {
    let mut e = Maxwell3d::default();
    for (m, v) in [
        (0, 0xb197),
        (0x17c8, 0),
        (0x17cc, 0x80000),
        (0x17d0, 0),
        (0x17d4, 0x8000d),
        (0x17d8, 1),
        (0x1434, u32::MAX),
        (0x1438, 0),
        (0x1644, 0),
    ] {
        assert!(set(&mut e, m, v));
    }
    e
}
#[test]
fn literal_index_descriptor_bounds_and_begin_first_count_end() {
    let mut e = configured();
    let b = e.index_buffer(1, 6).unwrap();
    assert_eq!(
        (b.address, b.bytes, b.width, b.base),
        (GpuAddress(0x80002), 12, 2, -1)
    );
    assert!(e.index_buffer(2, 6).is_err());
    assert!(e.index_buffer(u32::MAX, 3).is_err());
    assert!(e.index_buffer(0, 0).is_err());
    assert!(e.index_buffer(0, 2).is_err());
    assert!(!set(&mut e, 0x17e0, 3));
    assert!(set(&mut e, 0x1618, 4));
    assert!(!set(&mut e, 0x1618, 4));
    assert!(!set(&mut e, 0x17e0, 3));
    assert!(set(&mut e, 0x17dc, 1));
    assert_eq!(
        e.apply(MethodWrite {
            subchannel: 0,
            method: 0x17e0,
            value: 6
        })
        .unwrap(),
        MethodEffect::DrawIndices { first: 1, count: 6 }
    );
    assert!(set(&mut e, 0x1614, 0));
    assert!(!set(&mut e, 0x1614, 0));
    assert!(!set(&mut e, 0x17e0, 6));
}
#[test]
fn unsupported_index_modes_alignment_and_40_bit_bounds() {
    let mut e = configured();
    for (m, v) in [
        (0x17d8, 0),
        (0x17d8, 3),
        (0x17c8, 256),
        (0x17d0, 256),
        (0x1438, 1),
        (0x1644, 1),
        (0x1618, 0x4000004),
        (0x17e8, 0x40060001),
    ] {
        assert!(!set(&mut e, m, v), "method{m:x}");
    }
    assert!(set(&mut e, 0x17cc, 0x80001));
    assert!(e.index_buffer(0, 3).is_err());
    let mut e = configured();
    for (m, v) in [
        (0x17c8, 255),
        (0x17cc, 0xfffffffc),
        (0x17d0, 255),
        (0x17d4, u32::MAX),
        (0x17d8, 2),
    ] {
        assert!(set(&mut e, m, v));
    }
    assert!(e.index_buffer(0, 3).is_err());
    let mut e = configured();
    assert!(set(&mut e, 0x17d8, 2));
    assert!(set(&mut e, 0x17d4, 0x8001b));
    assert_eq!(e.index_buffer(1, 6).unwrap().bytes, 24);
}

#[test]
fn array_restart_policy_requires_disabled_global_restart_only_when_it_can_apply() {
    use wl_gpu::maxwell::MethodError;
    let mut engine = Maxwell3d::default();
    assert!(set(&mut engine, 0, 0xb197));
    assert!(set(&mut engine, 0xde8, 1));
    let draw = MethodWrite {
        subchannel: 0,
        method: 0x1214,
        value: 0x40030000,
    };
    assert_eq!(
        engine.apply(draw),
        Err(MethodError::IncompletePrimitiveRestart)
    );
    assert!(!set(&mut engine, 0xde8, 2));
    assert_eq!(
        engine.apply(draw),
        Err(MethodError::IncompletePrimitiveRestart)
    );
    assert!(set(&mut engine, 0x1644, 0));
    assert_eq!(
        engine.apply(draw),
        Ok(MethodEffect::Draw { first: 0, count: 3 })
    );
    assert!(!set(&mut engine, 0x1644, 1));
    assert_eq!(
        engine.apply(draw),
        Ok(MethodEffect::Draw { first: 0, count: 3 })
    );
    let mut independent = Maxwell3d::default();
    assert!(set(&mut independent, 0, 0xb197));
    assert!(set(&mut independent, 0xde8, 0));
    assert_eq!(
        independent.apply(draw),
        Ok(MethodEffect::Draw { first: 0, count: 3 })
    );
}
