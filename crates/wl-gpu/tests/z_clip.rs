mod common;
use wl_gpu::{
    fifo::{Decoder, MethodWrite},
    maxwell::{Channel, Maxwell3d, MethodEffect, MethodError},
    raster::{ColorLayout, DepthClipRange, FIXED_RASTER_STATE},
};

fn set(engine: &mut Maxwell3d, method: u16, value: u32) -> Result<MethodEffect, MethodError> {
    engine.apply(MethodWrite {
        subchannel: 0,
        method,
        value,
    })
}
fn without(methods: &[u16]) -> Maxwell3d {
    // Decode the independently authored literal hardware fixture, withholding
    // only the requested writes. Do not synthesize defaults from the fixed mask.
    let mut engine = Maxwell3d::default();
    let mut fifo = Decoder::default();
    for word in common::hardware::setup() {
        if let Some(write) = fifo.push_word(word).unwrap() {
            if !methods.contains(&write.method) {
                engine.apply(write).unwrap();
            }
        }
    }
    fifo.finish().unwrap();
    engine
}

#[test]
fn homogeneous_z_interval_requires_its_own_explicit_selector() {
    let mut engine = without(&[0x0d7c]);
    // Geometry clip flags and viewport depth mapping do not select this interval.
    set(&mut engine, 0x193c, 0x1801).unwrap();
    let error = engine.rasterization().unwrap_err().to_string();
    assert!(
        error.contains("incomplete or unsupported fixed raster state"),
        "{error}"
    );
    assert_eq!(
        set(&mut engine, 0x1214, 0x40030000).unwrap(),
        MethodEffect::Draw { first: 0, count: 3 }
    );
    // A parsed draw effect does not authorize backend rasterization or fill a
    // missing selector. Actual host execution is covered by separate GPU tests.
    assert!(engine.rasterization().is_err());
    set(&mut engine, 0x0d7c, 0).unwrap();
    assert!(engine.rasterization().is_err()); // Must also program the matching Z scale/bias.
    set(&mut engine, 0xa08, 0.5f32.to_bits()).unwrap();
    set(&mut engine, 0xa14, 0.5f32.to_bits()).unwrap();
    assert_eq!(
        engine.rasterization().unwrap().1.depth_clip,
        DepthClipRange::NegativeOneToOne
    );
    set(&mut engine, 0xa08, 1f32.to_bits()).unwrap();
    set(&mut engine, 0xa14, 0).unwrap();
    set(&mut engine, 0x0d7c, 1).unwrap();
    assert_eq!(
        engine.rasterization().unwrap(),
        without(&[]).rasterization().unwrap()
    );
}

#[test]
fn complete_thirty_two_state_mask_requires_both_high_and_low_bits() {
    assert_eq!(FIXED_RASTER_STATE.len(), 32);
    assert_eq!(FIXED_RASTER_STATE.last(), Some(&(0x0d7c, 1)));
    assert!(without(&[]).rasterization().is_ok());
    // Bit 31 is newly occupied. Neither shifting by 32 nor dropping the highest
    // required bit may panic or accidentally accept an incomplete mask.
    for (method, value) in [(0x0138, 1), (0x0c0c, 0x3f800000), (0x0d7c, 1)] {
        let mut engine = without(&[method]);
        assert!(engine.rasterization().is_err(), "missing {method:#x}");
        set(&mut engine, method, value).unwrap();
        assert!(engine.rasterization().is_ok());
    }
}

#[test]
fn reserved_modes_and_unaligned_offsets_cannot_mutate_valid_state() {
    let mut engine = without(&[]);
    let before = engine.rasterization().unwrap();
    for value in [2, 3, 0x100, 0x80000000, u32::MAX] {
        assert_eq!(
            set(&mut engine, 0x0d7c, value),
            Err(MethodError::UnsupportedValue {
                method: 0x0d7c,
                value
            })
        );
        assert_eq!(engine.rasterization().unwrap(), before);
    }
    for method in [0x0d7d, 0x0d7e, 0x0d7f, 0x193d] {
        assert_eq!(
            set(&mut engine, method, 1),
            Err(MethodError::InvalidMethod(method))
        );
        assert_eq!(engine.rasterization().unwrap(), before);
    }
}

#[test]
fn literal_b06f_packets_program_z_interval_and_clip_control_independently() {
    let mut channel = Channel::default();
    channel.engine = without(&[0x0d7c, 0x193c]);
    assert!(channel.engine.rasterization().is_err());
    // 0xd7c / 4 = 0x35f, whereas 0x193c / 4 = 0x64f. These are distinct methods.
    for word in [0x2001035f, 1] {
        channel.push_word(word).unwrap();
    }
    channel.finish().unwrap();
    assert!(channel.engine.rasterization().is_err());
    for word in [0x2001064f, 0x1801] {
        channel.push_word(word).unwrap();
    }
    channel.finish().unwrap();
    assert!(channel.engine.rasterization().is_ok());
    let mut engine = without(&[0x0d7c, 0x193c]);
    set(&mut engine, 0x193c, 0x1801).unwrap();
    assert!(engine.rasterization().is_err());
    set(&mut engine, 0x0d7c, 1).unwrap();
    assert_eq!(
        engine.rasterization().unwrap(),
        channel.engine.rasterization().unwrap()
    );
}

#[test]
fn genuine_clear_descriptor_does_not_require_draw_z_clip_selection() {
    let mut engine = Maxwell3d::default();
    for (method, value) in [
        (0, 0xb197),
        (0x0138, 1),
        (0x15d0, 0),
        (0x121c, 1),
        (0x10f8, 0),
        (0x0ff4, 7 << 16),
        (0x0ff8, 9 << 16),
        (0x0fe0, 0),
        (0x0fe4, 0xd0000),
        (0x0fe8, 0xa),
        (0x0fec, 0x10),
        (0x0ff0, 0),
        (0x1228, 7),
        (0x122c, 9),
        (0x1230, 1),
        (0x179c, 0),
        (0x1538, 1),
        (0x0d90, 0x3e800000),
    ] {
        set(&mut engine, method, value).unwrap();
    }
    for (n, value) in [0, 0x90000, 28, 9, 0xd5, 0x1000, 1, 0, 0]
        .into_iter()
        .enumerate()
    {
        set(&mut engine, 0x0800 + n as u16 * 4, value).unwrap();
    }
    for (n, value) in [0.25f32, 0.5, 0.75, 1.].into_iter().enumerate() {
        set(&mut engine, 0x0d80 + n as u16 * 4, value.to_bits()).unwrap();
    }
    assert!(engine.rasterization().is_err());
    let mut channel = Channel::default();
    channel.engine = engine;
    channel.push_word(0x20010674).unwrap();
    assert_eq!(
        channel.push_word(0x3d).unwrap(),
        MethodEffect::Clear {
            color: true,
            depth: true
        }
    );
    channel.finish().unwrap();
    let clear = channel.engine.clear_surface(true, true).unwrap();
    let (color, values) = clear.color.unwrap();
    assert_eq!((color.width, color.height), (7, 9));
    assert_eq!(color.layout, ColorLayout::Pitch { bytes_per_row: 28 });
    assert_eq!(values, [0.25, 0.5, 0.75, 1.]);
    let (depth, value) = clear.depth.unwrap();
    assert_eq!(
        (depth.width, depth.height, depth.block_height_log2, value),
        (7, 9, 1, 0.25)
    );
    // Neither 0xd7c nor 0x193c was programmed. Resolving a clear must not infer
    // their draw state; subsequent rasterization still remains incomplete.
    assert!(channel.engine.rasterization().is_err());
}

#[test]
fn unsupported_channel_clip_mode_keeps_prefix_and_never_consumes_suffix_draw() {
    let mut channel = Channel::default();
    channel.engine = without(&[]);
    // A valid earlier scissor change remains observable after the later fault.
    for word in [0x20010381, (43 << 16) | 5, 0x2001035f] {
        channel.push_word(word).unwrap();
    }
    assert!(channel.push_word(2).is_err());
    assert!(channel.is_faulted());
    let before = channel.engine.rasterization().unwrap();
    assert_eq!(before.1.scissor, [5, 0, 38, 29]);
    // A suffix that would reset scissor and emit a draw cannot be retried into
    // the faulted channel. This is packet/effect validation, not GPU execution.
    for word in [0x20010381, 47 << 16, 0x20010485, 0x40030000] {
        assert!(channel.push_word(word).is_err());
    }
    assert!(channel.finish().is_err());
    assert_eq!(channel.engine.rasterization().unwrap(), before);
}

#[test]
fn negative_depth_profile_requires_an_explicit_interval_and_matching_viewport() {
    let mut e = without(&[0xd7c]);
    set(&mut e, 0x193c, 0x181d).unwrap();
    set(&mut e, 0xa08, 0.5f32.to_bits()).unwrap();
    set(&mut e, 0xa14, 0.5f32.to_bits()).unwrap();
    assert!(e.rasterization().is_err());
    set(&mut e, 0xd7c, 0).unwrap();
    let before = e.rasterization().unwrap();
    assert_eq!(before.1.depth_clip, DepthClipRange::NegativeOneToOne);
    for value in [0, 0x101a, 0x181c, 0x181f, u32::MAX] {
        assert!(set(&mut e, 0x193c, value).is_err());
        assert_eq!(e.rasterization().unwrap(), before);
    }
    set(&mut e, 0x193c, 0x1801).unwrap();
    assert_eq!(e.rasterization().unwrap(), before);
}

#[test]
fn inactive_viewport_banks_retain_independent_scale_clip_and_depth_state() {
    let mut e = without(&[]);
    let before = e.rasterization().unwrap();
    for slot in 1..16 {
        for (field, value) in [2f32, -3., 0.5, 4., 5., 0.5].into_iter().enumerate() {
            set(
                &mut e,
                0xa00 + slot * 32 + field as u16 * 4,
                value.to_bits(),
            )
            .unwrap();
        }
        set(&mut e, 0xa18 + slot * 32, 0x6420).unwrap();
        for (field, value) in [(0, 17 << 16), (4, 19 << 16), (8, 0), (12, 1f32.to_bits())] {
            set(&mut e, 0xc00 + slot * 16 + field, value).unwrap();
        }
        assert_eq!(e.rasterization().unwrap(), before);
        for (method, value) in [
            (0xa1c + slot * 32, 0),
            (0xa00 + slot * 32, f32::NAN.to_bits()),
            (0xa18 + slot * 32, 0),
            (0xc08 + slot * 16, 1f32.to_bits()),
        ] {
            assert!(set(&mut e, method, value).is_err());
            assert_eq!(e.rasterization().unwrap(), before);
        }
    }
}
