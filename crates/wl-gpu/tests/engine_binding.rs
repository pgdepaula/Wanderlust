//! Independent class IDs and NOP offsets from NVIDIA classes at 9fdf5c4062.
use wl_gpu::{fifo::MethodWrite, maxwell::*};

fn write(subchannel: u8, method: u16, value: u32) -> MethodWrite {
    MethodWrite {
        subchannel,
        method,
        value,
    }
}

#[test]
fn engine_bindings_route_without_leaking_auxiliary_writes_into_3d() {
    let mut engine = Maxwell3d::default();
    for (method, value) in [
        (0, 0xb197),
        (0x1608, 0),
        (0x160c, 0x40000),
        (0x2140, 0x51),
        (0x2144, 0),
        (0x214c, 4),
        (0x2150, 0),
    ] {
        engine.apply(write(0, method, value)).unwrap();
    }
    let original = engine.fragment_program().unwrap();
    for (subchannel, class, expected) in [
        (1, 0xb1c0, EngineClass::Compute),
        (2, 0xb0b5, EngineClass::DmaCopy),
        (3, 0x902d, EngineClass::TwoD),
        (4, 0xa140, EngineClass::InlineMemory),
    ] {
        engine.apply(write(subchannel, 0, class)).unwrap();
        assert_eq!(engine.bound_class(subchannel), Some(expected));
        for value in [0, 1, u32::MAX] {
            assert_eq!(
                engine.apply(write(subchannel, 0x100, value)),
                Ok(MethodEffect::State)
            );
        }
        assert_eq!(
            engine.apply(write(subchannel, 0x160c, 0xdead0000)),
            Err(MethodError::UnsupportedEngineMethod {
                class,
                method: 0x160c
            })
        );
        assert_eq!(engine.fragment_program().unwrap(), original);
    }
    assert_eq!(engine.bound_class(0), Some(EngineClass::ThreeD));
}

#[test]
fn class_rebinding_and_invalid_class_are_atomic_and_stage_qualified() {
    let mut engine = Maxwell3d::default();
    engine.apply(write(0, 0, 0xb197)).unwrap();
    for (method, value) in [
        (0x1608, 0),
        (0x160c, 0x40000),
        (0x2140, 0x51),
        (0x2144, 0),
        (0x214c, 4),
        (0x2150, 0),
    ] {
        engine.apply(write(0, method, value)).unwrap();
    }
    let previous_program = engine.fragment_program().unwrap();
    for invalid in [0, 0xffff, 0x1b197, u32::MAX] {
        assert_eq!(
            engine.apply(write(0, 0, invalid)),
            Err(MethodError::UnsupportedClass(invalid))
        );
        assert_eq!(engine.bound_class(0), Some(EngineClass::ThreeD));
    }
    engine.apply(write(0, 0, 0xb1c0)).unwrap();
    assert!(
        engine
            .fragment_program()
            .unwrap_err()
            .0
            .contains("B197 unbound")
    );
    assert!(
        engine
            .vertex_program()
            .unwrap_err()
            .0
            .contains("B197 unbound")
    );
    assert_eq!(
        engine.apply(write(0, 0x2140, 0x51)),
        Err(MethodError::UnsupportedEngineMethod {
            class: 0xb1c0,
            method: 0x2140
        })
    );
    assert_eq!(
        engine.apply(write(8, 0, 0xb197)),
        Err(MethodError::InvalidSubchannel(8))
    );
    assert_eq!(engine.bound_class(8), None);
    assert_eq!(
        engine.apply(write(1, 0x100, 0)),
        Err(MethodError::UnboundSubchannel(1))
    );
    engine.apply(write(0, 0, 0xb197)).unwrap();
    assert_eq!(engine.fragment_program().unwrap(), previous_program);
}

#[test]
fn unsupported_engine_operation_latches_channel_without_replaying() {
    let mut channel = Channel::default();
    for word in [0x20012000, 0xb1c0, 0x20012040, 0] {
        channel.push_word(word).unwrap();
    }
    channel.push_word(0x20012850).unwrap(); // Compute subchannel; B197 PS method.
    let error = channel.push_word(0x51).unwrap_err();
    assert!(error.0.contains("UnsupportedEngineMethod"));
    assert!(channel.is_faulted());
    assert!(channel.push_word(0).is_err());
}
