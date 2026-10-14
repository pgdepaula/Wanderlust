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
fn ready() -> Maxwell3d {
    ready_channel().engine
}
fn configured() -> Maxwell3d {
    let mut engine = ready();
    for (method, value) in [
        (0x133c, 1),
        (0x1340, 1),
        (0x1344, 5),
        (0x1348, 6),
        (0x134c, 1),
        (0x1350, 1),
        (0x1358, 5),
        (0x1360, 1),
    ] {
        set(&mut engine, method, value).unwrap();
    }
    engine
}
fn blend(engine: &Maxwell3d) -> BlendState {
    engine.rasterization().unwrap().1.color.blend.unwrap()
}
const OPERATIONS: [(u32, u32, BlendOperation); 5] = [
    (1, 0x8006, BlendOperation::Add),
    (2, 0x800a, BlendOperation::Subtract),
    (3, 0x800b, BlendOperation::ReverseSubtract),
    (4, 0x8007, BlendOperation::Min),
    (5, 0x8008, BlendOperation::Max),
];

#[test]
fn literal_compact_and_gl_operations_are_independent_for_rgb_and_alpha() {
    use BlendFactor::*;
    // Expectations are literal B197 alias pairs, not production enum conversion.
    for (rgb_compact, rgb_gl, rgb_op) in OPERATIONS {
        for (alpha_compact, alpha_gl, alpha_op) in OPERATIONS {
            for rgb in [rgb_compact, rgb_gl] {
                for alpha in [alpha_compact, alpha_gl] {
                    let mut engine = configured();
                    set(&mut engine, 0x1340, rgb).unwrap();
                    set(&mut engine, 0x134c, alpha).unwrap();
                    let expected = BlendState {
                        color_operation: rgb_op,
                        alpha_operation: alpha_op,
                        color: if matches!(rgb_op, BlendOperation::Min | BlendOperation::Max) {
                            [One, One]
                        } else {
                            [SourceAlpha, OneMinusSourceAlpha]
                        },
                        alpha: if matches!(alpha_op, BlendOperation::Min | BlendOperation::Max) {
                            [One, One]
                        } else {
                            [Zero, SourceAlpha]
                        },
                    };
                    assert_eq!(blend(&engine), expected, "RGB {rgb:#x}, alpha {alpha:#x}");
                }
            }
        }
    }
}

#[test]
fn shared_alpha_inherits_rgb_equation_and_factors_without_alpha_registers() {
    use BlendFactor::*;
    for (compact, gl, operation) in OPERATIONS {
        for value in [compact, gl] {
            let mut engine = ready();
            for (method, value) in [
                (0x133c, 0),
                (0x1340, value),
                (0x1344, 5),
                (0x1348, 6),
                (0x1360, 1),
            ] {
                set(&mut engine, method, value).unwrap();
            }
            let expected_factors = if matches!(operation, BlendOperation::Min | BlendOperation::Max)
            {
                [One, One]
            } else {
                [SourceAlpha, OneMinusSourceAlpha]
            };
            assert_eq!(
                blend(&engine),
                BlendState {
                    color: expected_factors,
                    alpha: expected_factors,
                    color_operation: operation,
                    alpha_operation: operation,
                }
            );
            // Distinct valid alpha state is ignored while separation is disabled.
            for (method, value) in [(0x134c, 0x8008), (0x1350, 1), (0x1358, 2)] {
                set(&mut engine, method, value).unwrap();
            }
            assert_eq!(blend(&engine).alpha_operation, operation);
            assert_eq!(blend(&engine).alpha, expected_factors);
            set(&mut engine, 0x133c, 1).unwrap();
            assert_eq!(blend(&engine).alpha_operation, BlendOperation::Max);
            assert_eq!(blend(&engine).alpha, [One, One]);
            set(&mut engine, 0x133c, 0).unwrap();
            assert_eq!(blend(&engine).alpha_operation, operation);
            assert_eq!(blend(&engine).alpha, expected_factors);
        }
    }
}

#[test]
fn min_max_canonicalization_does_not_overwrite_later_add_factor_state() {
    use BlendFactor::*;
    let mut engine = configured();
    set(&mut engine, 0x1340, 0x8007).unwrap();
    set(&mut engine, 0x134c, 0x8008).unwrap();
    assert_eq!(blend(&engine).color, [One, One]);
    assert_eq!(blend(&engine).alpha, [One, One]);
    // Writes to ignored factors remain real register state for later equations.
    for (method, value) in [(0x1344, 1), (0x1348, 2), (0x1350, 6), (0x1358, 1)] {
        set(&mut engine, method, value).unwrap();
    }
    assert_eq!(blend(&engine).color, [One, One]);
    assert_eq!(blend(&engine).alpha, [One, One]);
    set(&mut engine, 0x1340, 1).unwrap();
    set(&mut engine, 0x134c, 0x8006).unwrap();
    let expected = BlendState {
        color: [Zero, One],
        alpha: [OneMinusSourceAlpha, Zero],
        color_operation: BlendOperation::Add,
        alpha_operation: BlendOperation::Add,
    };
    assert_eq!(blend(&engine), expected);
    set(&mut engine, 0x1360, 0).unwrap();
    assert!(engine.rasterization().unwrap().1.color.blend.is_none());
    set(&mut engine, 0x1360, 1).unwrap();
    assert_eq!(blend(&engine), expected);
}

#[test]
fn enabled_blending_requires_complete_explicit_selected_state() {
    let mut engine = ready();
    assert!(engine.rasterization().unwrap().1.color.blend.is_none());
    set(&mut engine, 0x1360, 1).unwrap();
    assert!(
        engine
            .rasterization()
            .unwrap_err()
            .to_string()
            .contains("alpha blend selection")
    );
    let fields = [
        (0x1340, 4),
        (0x1344, 5),
        (0x1348, 6),
        (0x134c, 5),
        (0x1350, 1),
        (0x1358, 2),
    ];
    for separate in [false, true] {
        let required = if separate { fields.len() } else { 3 };
        for omitted in 0..required {
            let mut engine = ready();
            set(&mut engine, 0x133c, u32::from(separate)).unwrap();
            set(&mut engine, 0x1360, 1).unwrap();
            for (n, &(method, value)) in fields[..required].iter().enumerate() {
                if n != omitted {
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
fn unsupported_equations_and_unaligned_methods_do_not_mutate_valid_state() {
    let mut engine = configured();
    set(&mut engine, 0x1340, 0x800b).unwrap();
    set(&mut engine, 0x134c, 5).unwrap();
    let before = engine.rasterization().unwrap();
    for method in [0x1340, 0x134c] {
        for value in [0, 6, 0x8009, 0x901c, 0x901d, u32::MAX] {
            assert_eq!(
                set(&mut engine, method, value),
                Err(MethodError::UnsupportedValue { method, value })
            );
            assert_eq!(engine.rasterization().unwrap(), before);
        }
    }
    for method in [0x1341, 0x134d, 0x1351, 0x1355, 0x1359] {
        assert_eq!(
            set(&mut engine, method, 1),
            Err(MethodError::InvalidMethod(method))
        );
        assert_eq!(engine.rasterization().unwrap(), before);
    }
}

fn literal_blend_packet(channel: &mut Channel) {
    // Incrementing byte methods1340..1358. Method1354 is color-key enable,
    // not another blend coefficient. Its explicit inactive value is zero.
    for word in [
        0x200104cf, 1, // separate alpha
        0x200704d0, 0x800a, 5, 6, 0x800b, 1, 0, 2, 0x200104d8, 1, // enable global blending
    ] {
        channel.push_word(word).unwrap();
    }
    channel.finish().unwrap();
}
#[test]
fn literal_fifo_hole_accepts_inactive_state_and_latches_unsupported_enable() {
    use BlendFactor::*;
    let mut channel = ready_channel();
    literal_blend_packet(&mut channel);
    assert_eq!(
        blend(&channel.engine),
        BlendState {
            color: [SourceAlpha, OneMinusSourceAlpha],
            alpha: [Zero, One],
            color_operation: BlendOperation::Subtract,
            alpha_operation: BlendOperation::ReverseSubtract,
        }
    );
    for unsupported in [1, 2, 0x80000000, u32::MAX] {
        let mut channel = ready_channel();
        literal_blend_packet(&mut channel);
        // Successfully consumed fields stay updated; the faulting hole does
        // not consume/reinterpret the final alpha-destination coefficient.
        for word in [0x200704d0, 4, 1, 2, 3, 5] {
            channel.push_word(word).unwrap();
        }
        assert!(channel.push_word(unsupported).is_err());
        assert!(channel.is_faulted());
        let expected = BlendState {
            color: [One, One],
            alpha: [SourceAlpha, One],
            color_operation: BlendOperation::Min,
            alpha_operation: BlendOperation::ReverseSubtract,
        };
        assert_eq!(blend(&channel.engine), expected);
        assert!(channel.push_word(6).is_err());
        assert!(channel.finish().is_err());
        assert_eq!(blend(&channel.engine), expected);
    }
}
