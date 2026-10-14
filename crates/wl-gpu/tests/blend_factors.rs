mod common;
use wl_gpu::{fifo::MethodWrite, maxwell::*, raster::*};

fn set(engine: &mut Maxwell3d, method: u16, value: u32) -> Result<MethodEffect, MethodError> {
    engine.apply(MethodWrite {
        subchannel: 0,
        method,
        value,
    })
}
fn ready_channel() -> Channel {
    let mut channel = Channel::default();
    for word in common::hardware::setup() {
        channel.push_word(word).unwrap();
    }
    channel.finish().unwrap();
    channel
}
fn configured() -> Maxwell3d {
    let mut engine = ready_channel().engine;
    for (method, value) in [
        (0x133c, 1),
        (0x1340, 1),
        (0x1344, 5),
        (0x1348, 6),
        (0x134c, 1),
        (0x1350, 2),
        (0x1358, 1),
        (0x1360, 1),
    ] {
        set(&mut engine, method, value).unwrap();
    }
    engine
}
fn blend(engine: &Maxwell3d) -> BlendState {
    engine.rasterization().unwrap().1.color.blend.unwrap()
}

// Literal aliases from NVB197 SET_BLEND_{COLOR,ALPHA}_{SOURCE,DEST}_COEFF.
// These expectations do not call the production coefficient decoder.
const FACTORS: [(u32, u32, BlendFactor); 10] = [
    (1, 0x4000, BlendFactor::Zero),
    (2, 0x4001, BlendFactor::One),
    (3, 0x4300, BlendFactor::SourceColor),
    (4, 0x4301, BlendFactor::OneMinusSourceColor),
    (5, 0x4302, BlendFactor::SourceAlpha),
    (6, 0x4303, BlendFactor::OneMinusSourceAlpha),
    (7, 0x4304, BlendFactor::DestinationAlpha),
    (8, 0x4305, BlendFactor::OneMinusDestinationAlpha),
    (9, 0x4306, BlendFactor::DestinationColor),
    (10, 0x4307, BlendFactor::OneMinusDestinationColor),
];
const METHODS: [u16; 4] = [0x1344, 0x1348, 0x1350, 0x1358];

fn replace_factor(state: &mut BlendState, index: usize, factor: BlendFactor) {
    match index {
        0 => state.color[0] = factor,
        1 => state.color[1] = factor,
        2 => state.alpha[0] = factor,
        3 => state.alpha[1] = factor,
        _ => unreachable!(),
    }
}

#[test]
fn every_compact_and_gl_alias_changes_only_its_selected_coefficient() {
    let baseline = BlendState {
        color: [BlendFactor::SourceAlpha, BlendFactor::OneMinusSourceAlpha],
        alpha: [BlendFactor::One, BlendFactor::Zero],
        color_operation: BlendOperation::Add,
        alpha_operation: BlendOperation::Add,
    };
    assert_eq!(blend(&configured()), baseline);
    for (index, method) in METHODS.into_iter().enumerate() {
        for (compact, gl, factor) in FACTORS {
            let mut engine = configured();
            let mut expected = baseline;
            replace_factor(&mut expected, index, factor);
            set(&mut engine, method, compact).unwrap();
            assert_eq!(blend(&engine), expected, "method {method:#x}, {compact:#x}");
            let compact_state = engine.rasterization().unwrap();
            set(&mut engine, method, gl).unwrap();
            assert_eq!(blend(&engine), expected, "method {method:#x}, {gl:#x}");
            // Equivalent wire aliases must resolve to the same pipeline state.
            assert_eq!(engine.rasterization().unwrap(), compact_state);
        }
    }
}

#[test]
fn independent_alpha_and_shared_inheritance_retain_distinct_raw_factors() {
    for (src, _, source) in FACTORS {
        for (_, dst, destination) in FACTORS {
            let mut engine = ready_channel().engine;
            for (method, value) in [
                (0x133c, 0),
                (0x1340, 1),
                (0x1344, src),
                (0x1348, dst),
                (0x1360, 1),
            ] {
                set(&mut engine, method, value).unwrap();
            }
            let inherited = BlendState {
                color: [source, destination],
                alpha: [source, destination],
                color_operation: BlendOperation::Add,
                alpha_operation: BlendOperation::Add,
            };
            assert_eq!(blend(&engine), inherited);
            // No alpha registers were required by shared-alpha mode. Switching
            // modes must expose their absence rather than inventing defaults.
            set(&mut engine, 0x133c, 1).unwrap();
            assert!(engine.rasterization().is_err());
            for (method, value) in [(0x134c, 3), (0x1350, 0x4307), (0x1358, 0x4304)] {
                set(&mut engine, method, value).unwrap();
            }
            let separate = BlendState {
                color: [source, destination],
                alpha: [
                    BlendFactor::OneMinusDestinationColor,
                    BlendFactor::DestinationAlpha,
                ],
                color_operation: BlendOperation::Add,
                alpha_operation: BlendOperation::ReverseSubtract,
            };
            assert_eq!(blend(&engine), separate);
            set(&mut engine, 0x133c, 0).unwrap();
            assert_eq!(blend(&engine), inherited);
            set(&mut engine, 0x133c, 1).unwrap();
            assert_eq!(blend(&engine), separate);
        }
    }
}

#[test]
fn min_max_ignore_factors_without_erasing_any_raw_coefficient() {
    for (index, method) in METHODS.into_iter().enumerate() {
        for (compact, gl, factor) in FACTORS {
            let mut engine = configured();
            set(&mut engine, 0x1340, 4).unwrap();
            set(&mut engine, 0x134c, 5).unwrap();
            let canonical = BlendState {
                color: [BlendFactor::One; 2],
                alpha: [BlendFactor::One; 2],
                color_operation: BlendOperation::Min,
                alpha_operation: BlendOperation::Max,
            };
            for value in [compact, gl] {
                set(&mut engine, method, value).unwrap();
                assert_eq!(blend(&engine), canonical);
            }
            let mut retained = BlendState {
                color: [BlendFactor::SourceAlpha, BlendFactor::OneMinusSourceAlpha],
                alpha: [BlendFactor::One, BlendFactor::Zero],
                color_operation: BlendOperation::Add,
                alpha_operation: BlendOperation::Add,
            };
            replace_factor(&mut retained, index, factor);
            set(&mut engine, 0x1340, 1).unwrap();
            assert_eq!(blend(&engine).color, retained.color);
            assert_eq!(blend(&engine).alpha, [BlendFactor::One; 2]);
            set(&mut engine, 0x134c, 1).unwrap();
            assert_eq!(blend(&engine), retained);
            set(&mut engine, 0x1360, 0).unwrap();
            assert!(engine.rasterization().unwrap().1.color.blend.is_none());
            set(&mut engine, 0x1360, 1).unwrap();
            assert_eq!(blend(&engine), retained);
        }
    }
}

#[test]
fn even_min_max_require_explicit_selected_factor_registers() {
    let fields = [(0x1344, 3), (0x1348, 8), (0x1350, 9), (0x1358, 4)];
    for separate in [false, true] {
        let required = if separate { 4 } else { 2 };
        for omitted in 0..required {
            let mut engine = ready_channel().engine;
            for (method, value) in [
                (0x133c, u32::from(separate)),
                (0x1340, 4),
                (0x134c, 5),
                (0x1360, 1),
            ] {
                set(&mut engine, method, value).unwrap();
            }
            for (index, &(method, value)) in fields[..required].iter().enumerate() {
                if index != omitted {
                    set(&mut engine, method, value).unwrap();
                }
            }
            let error = engine.rasterization().unwrap_err().to_string();
            assert!(error.contains("incomplete global blend"), "{error}");
            set(&mut engine, fields[omitted].0, fields[omitted].1).unwrap();
            assert!(engine.rasterization().is_ok());
        }
    }
}

#[test]
fn unsupported_factors_and_unaligned_methods_preserve_state_even_when_ignored() {
    // Saturation, constant factors, and dual-source factors have real hardware
    // encodings; this bounded implementation must explicitly reject them.
    let unsupported = [
        0,
        11,
        12,
        13,
        14,
        15,
        16,
        17,
        18,
        19,
        0x4308,
        0xc001,
        0xc002,
        0xc003,
        0xc004,
        0xc900,
        0xc901,
        0xc902,
        0xc903,
        0x4002,
        0x14300,
        u32::MAX,
    ];
    for operation in [1, 4, 5] {
        let mut engine = configured();
        for (method, value) in [(0x1344, 3), (0x1348, 8), (0x1350, 9), (0x1358, 4)] {
            set(&mut engine, method, value).unwrap();
        }
        set(&mut engine, 0x1340, operation).unwrap();
        set(&mut engine, 0x134c, operation).unwrap();
        let before = engine.rasterization().unwrap();
        for method in METHODS {
            for value in unsupported {
                assert_eq!(
                    set(&mut engine, method, value),
                    Err(MethodError::UnsupportedValue { method, value })
                );
                assert_eq!(engine.rasterization().unwrap(), before);
            }
            for offset in 1..=3 {
                let unaligned = method + offset;
                assert_eq!(
                    set(&mut engine, unaligned, 3),
                    Err(MethodError::InvalidMethod(unaligned))
                );
                assert_eq!(engine.rasterization().unwrap(), before);
            }
        }
        // Re-expose all factors after MIN/MAX so mutation of ignored raw state
        // cannot hide behind canonical ONE/ONE during the preceding assertions.
        set(&mut engine, 0x1340, 1).unwrap();
        set(&mut engine, 0x134c, 1).unwrap();
        assert_eq!(
            blend(&engine).color,
            [
                BlendFactor::SourceColor,
                BlendFactor::OneMinusDestinationAlpha
            ]
        );
        assert_eq!(
            blend(&engine).alpha,
            [
                BlendFactor::DestinationColor,
                BlendFactor::OneMinusSourceColor
            ]
        );
    }
}

#[test]
fn literal_incrementing_packet_keeps_hole_and_coefficient_fault_prefixes() {
    let mut channel = ready_channel();
    for word in [
        0x200104cf, 1,
        // Seven incrementing values cover 1340 through 1358, including 1354.
        0x200704d0, 1, 0x4300, 8, 3, 9, 0, 0x4301, 0x200104d8, 1,
    ] {
        channel.push_word(word).unwrap();
    }
    channel.finish().unwrap();
    let initial = BlendState {
        color: [
            BlendFactor::SourceColor,
            BlendFactor::OneMinusDestinationAlpha,
        ],
        alpha: [
            BlendFactor::DestinationColor,
            BlendFactor::OneMinusSourceColor,
        ],
        color_operation: BlendOperation::Add,
        alpha_operation: BlendOperation::ReverseSubtract,
    };
    assert_eq!(blend(&channel.engine), initial);

    // First a hole fault: earlier coefficient writes survive, while the alpha
    // destination suffix must neither execute nor be mistaken for another field.
    for word in [0x200704d0, 2, 10, 7, 1, 3] {
        channel.push_word(word).unwrap();
    }
    assert!(channel.push_word(1).is_err());
    assert!(channel.is_faulted());
    let prefix = BlendState {
        color: [
            BlendFactor::OneMinusDestinationColor,
            BlendFactor::DestinationAlpha,
        ],
        alpha: [BlendFactor::SourceColor, BlendFactor::OneMinusSourceColor],
        color_operation: BlendOperation::Subtract,
        alpha_operation: BlendOperation::Add,
    };
    assert_eq!(blend(&channel.engine), prefix);
    for word in [0x4305, 0x20010485, 0x40030000] {
        assert!(channel.push_word(word).is_err());
    }
    assert!(channel.finish().is_err());
    assert_eq!(blend(&channel.engine), prefix);

    // Then a coefficient fault before the hole, using a fresh channel. The
    // faulting coefficient itself retains its old value, as do all later fields.
    let mut channel = ready_channel();
    channel.engine = configured();
    for word in [0x200704d0, 3, 7] {
        channel.push_word(word).unwrap();
    }
    assert!(channel.push_word(0xc001).is_err());
    let prefix = BlendState {
        color: [
            BlendFactor::DestinationAlpha,
            BlendFactor::OneMinusSourceAlpha,
        ],
        alpha: [BlendFactor::One, BlendFactor::Zero],
        color_operation: BlendOperation::ReverseSubtract,
        alpha_operation: BlendOperation::Add,
    };
    assert_eq!(blend(&channel.engine), prefix);
    for word in [1, 9, 0, 10] {
        assert!(channel.push_word(word).is_err());
    }
    assert!(channel.finish().is_err());
    assert_eq!(blend(&channel.engine), prefix);
}
