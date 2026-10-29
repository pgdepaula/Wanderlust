//! Adversarial Maxwell B197 indexed draw conformance suite.
//!
//! Independently verifies hardware method semantics, descriptors, alignment,
//! 40-bit boundaries, signed base vertices, and primitive state transitions
//! against authoritative reference implementations:
//! - NVIDIA open-gpu-doc `clb197.h` (pinned rev `9fdf5c4062007929d9f4e6cbad9c9771fe61b880`):
//!   <https://github.com/NVIDIA/open-gpu-doc/blob/9fdf5c4062007929d9f4e6cbad9c9771fe61b880/classes/3d/clb197.h>
//! - Mesa Gallium3D Nouveau `nvc0_vbo.c` (pinned rev `e7959c6009d85d278585b2896495b2b3046d461b`):
//!   <https://gitlab.freedesktop.org/mesa/mesa/-/blob/e7959c6009d85d278585b2896495b2b3046d461b/src/gallium/drivers/nouveau/nvc0/nvc0_vbo.c#L724-738>
//!   <https://gitlab.freedesktop.org/mesa/mesa/-/blob/e7959c6009d85d278585b2896495b2b3046d461b/src/gallium/drivers/nouveau/nvc0/nvc0_vbo.c#L1136-1141>
//! - Nouveau / Envytools rnndb register mappings for Maxwell B197 3D classes.

use wl_gpu::{
    fifo::{Decoder, MethodWrite},
    maxwell::{Maxwell3d, MethodEffect, MethodError},
    memory::GpuAddress,
};

fn send(engine: &mut Maxwell3d, method: u16, value: u32) -> Result<MethodEffect, MethodError> {
    engine.apply(MethodWrite {
        subchannel: 0,
        method,
        value,
    })
}

fn try_send(engine: &mut Maxwell3d, method: u16, value: u32) -> bool {
    send(engine, method, value).is_ok()
}

/// Baseline Maxwell 3D engine configured with full valid B197 indexed state.
///
/// Methods independently verified against Mesa nvc0_vbo.c and rnndb definitions:
/// - 0x0000: B197 subchannel class bind
/// - 0x17c8..0x17cc: INDEX_ARRAY_START_HIGH / LOW (40-bit GPU virtual address)
/// - 0x17d0..0x17d4: INDEX_ARRAY_LIMIT_HIGH / LOW (40-bit inclusive upper bound: buf->address + buf->base.width0 - 1)
/// - 0x17d8: INDEX_FORMAT (1 = I16, 2 = I32; info->index_size >> 1)
/// - 0x17dc: INDEX_BATCH_FIRST (element index)
/// - 0x1434: VB_ELEMENT_BASE (signed 32-bit vertex bias)
/// - 0x1438: VB_INSTANCE_BASE (must be 0)
/// - 0x1644: PRIM_RESTART_ENABLE (must be 0)
fn setup_u16_engine() -> Maxwell3d {
    let mut engine = Maxwell3d::default();
    assert!(try_send(&mut engine, 0x0000, 0xb197));
    assert!(try_send(&mut engine, 0x17c8, 0x00));
    assert!(try_send(&mut engine, 0x17cc, 0x10000));
    assert!(try_send(&mut engine, 0x17d0, 0x00));
    assert!(try_send(&mut engine, 0x17d4, 0x1001f)); // 32 bytes (16 u16 elements)
    assert!(try_send(&mut engine, 0x17d8, 1)); // u16
    assert!(try_send(&mut engine, 0x17dc, 0)); // first = 0
    assert!(try_send(&mut engine, 0x1434, 0)); // base = 0
    assert!(try_send(&mut engine, 0x1438, 0)); // instance = 0
    assert!(try_send(&mut engine, 0x1644, 0)); // restart = 0
    engine
}

fn setup_u32_engine() -> Maxwell3d {
    let mut engine = Maxwell3d::default();
    assert!(try_send(&mut engine, 0x0000, 0xb197));
    assert!(try_send(&mut engine, 0x17c8, 0x00));
    assert!(try_send(&mut engine, 0x17cc, 0x20000));
    assert!(try_send(&mut engine, 0x17d0, 0x00));
    assert!(try_send(&mut engine, 0x17d4, 0x2003f)); // 64 bytes (16 u32 elements)
    assert!(try_send(&mut engine, 0x17d8, 2)); // u32
    assert!(try_send(&mut engine, 0x17dc, 0));
    assert!(try_send(&mut engine, 0x1434, 0));
    assert!(try_send(&mut engine, 0x1438, 0));
    assert!(try_send(&mut engine, 0x1644, 0));
    engine
}

#[test]
fn u16_descriptor_resolution_and_exact_inclusive_limits() {
    let e = setup_u16_engine();
    // 6 elements (12 bytes) starting at element 0 -> byte range 0x10000..=0x1000b
    let b = e.index_buffer(0, 6).unwrap();
    assert_eq!(
        (b.address, b.bytes, b.width, b.base),
        (GpuAddress(0x10000), 12, 2, 0)
    );

    // Exact limit: 16 elements (32 bytes) at element 0 -> 0x10000..=0x1001f
    // 15 elements (divisible by 3) = 30 bytes -> fits with 2 bytes to spare
    assert!(e.index_buffer(0, 15).is_ok());

    // Nonzero first index: first = 2 (4 bytes offset), count = 12 elements (24 bytes)
    // address = 0x10004, bytes = 24 -> last byte = 0x10004 + 24 - 1 = 0x1001b <= 0x1001f
    let b_offset = e.index_buffer(2, 12).unwrap();
    assert_eq!(
        (
            b_offset.address,
            b_offset.bytes,
            b_offset.width,
            b_offset.base
        ),
        (GpuAddress(0x10004), 24, 2, 0)
    );

    // Exact boundary fit: first = 10 (20 bytes), count = 6 (12 bytes)
    // 20 + 12 = 32 bytes -> last byte = 0x10000 + 32 - 1 = 0x1001f == limit
    assert!(e.index_buffer(10, 6).is_ok());

    // Over-by-one: first = 11 (22 bytes), count = 6 (12 bytes)
    // 22 + 12 = 34 bytes -> last byte = 0x10021 > 0x1001f (limit) -> MUST reject
    assert!(e.index_buffer(11, 6).is_err());
}

#[test]
fn u32_descriptor_resolution_and_alignment() {
    let mut e = setup_u32_engine();
    let b = e.index_buffer(1, 6).unwrap();
    // first = 1 * 4 bytes = 4; address = 0x20004, bytes = 24
    assert_eq!(
        (b.address, b.bytes, b.width, b.base),
        (GpuAddress(0x20004), 24, 4, 0)
    );

    // Alignment constraint: start must be divisible by width (4)
    assert!(try_send(&mut e, 0x17cc, 0x20002));
    assert!(
        e.index_buffer(0, 3).is_err(),
        "unaligned u32 base must fail"
    );

    assert!(try_send(&mut e, 0x17cc, 0x20001));
    assert!(e.index_buffer(0, 3).is_err(), "odd u32 base must fail");

    assert!(try_send(&mut e, 0x17cc, 0x20003));
    assert!(
        e.index_buffer(0, 3).is_err(),
        "misaligned u32 base must fail"
    );

    // Restore 4-byte aligned start
    assert!(try_send(&mut e, 0x17cc, 0x20000));
    assert!(e.index_buffer(0, 3).is_ok());
}

#[test]
fn signed_base_vertex_preservation() {
    let mut e = setup_u16_engine();

    // Positive base vertex
    assert!(try_send(&mut e, 0x1434, 42));
    assert_eq!(e.index_buffer(0, 3).unwrap().base, 42);

    // Negative base vertex: -1 (0xffffffff)
    assert!(try_send(&mut e, 0x1434, u32::MAX));
    assert_eq!(e.index_buffer(0, 3).unwrap().base, -1);

    // Large negative base vertex: -32768 (0xffff8000)
    assert!(try_send(&mut e, 0x1434, 0xffff_8000));
    assert_eq!(e.index_buffer(0, 3).unwrap().base, -32768);

    // i32::MIN (0x80000000)
    assert!(try_send(&mut e, 0x1434, 0x8000_0000));
    assert_eq!(e.index_buffer(0, 3).unwrap().base, i32::MIN);

    // i32::MAX (0x7fffffff)
    assert!(try_send(&mut e, 0x1434, 0x7fff_ffff));
    assert_eq!(e.index_buffer(0, 3).unwrap().base, i32::MAX);
}

#[test]
fn missing_descriptor_fields_rejected() {
    // Missing START_HIGH (0x17c8)
    let mut e = Maxwell3d::default();
    assert!(try_send(&mut e, 0x0000, 0xb197));
    assert!(try_send(&mut e, 0x17cc, 0x10000));
    assert!(try_send(&mut e, 0x17d0, 0));
    assert!(try_send(&mut e, 0x17d4, 0x1001f));
    assert!(try_send(&mut e, 0x17d8, 1));
    assert!(try_send(&mut e, 0x1434, 0));
    assert!(try_send(&mut e, 0x1438, 0));
    assert!(try_send(&mut e, 0x1644, 0));
    assert!(
        e.index_buffer(0, 3).is_err(),
        "missing START_HIGH must fail"
    );

    // Missing FORMAT (0x17d8)
    let mut e = Maxwell3d::default();
    assert!(try_send(&mut e, 0x0000, 0xb197));
    assert!(try_send(&mut e, 0x17c8, 0));
    assert!(try_send(&mut e, 0x17cc, 0x10000));
    assert!(try_send(&mut e, 0x17d0, 0));
    assert!(try_send(&mut e, 0x17d4, 0x1001f));
    assert!(try_send(&mut e, 0x1434, 0));
    assert!(try_send(&mut e, 0x1438, 0));
    assert!(try_send(&mut e, 0x1644, 0));
    assert!(e.index_buffer(0, 3).is_err(), "missing FORMAT must fail");

    // Missing BASE_VERTEX (0x1434)
    let mut e = Maxwell3d::default();
    assert!(try_send(&mut e, 0x0000, 0xb197));
    assert!(try_send(&mut e, 0x17c8, 0));
    assert!(try_send(&mut e, 0x17cc, 0x10000));
    assert!(try_send(&mut e, 0x17d0, 0));
    assert!(try_send(&mut e, 0x17d4, 0x1001f));
    assert!(try_send(&mut e, 0x17d8, 1));
    assert!(try_send(&mut e, 0x1438, 0));
    assert!(try_send(&mut e, 0x1644, 0));
    assert!(
        e.index_buffer(0, 3).is_err(),
        "missing BASE_VERTEX must fail"
    );

    // Missing INSTANCE_BASE (0x1438)
    let mut e = Maxwell3d::default();
    assert!(try_send(&mut e, 0x0000, 0xb197));
    assert!(try_send(&mut e, 0x17c8, 0));
    assert!(try_send(&mut e, 0x17cc, 0x10000));
    assert!(try_send(&mut e, 0x17d0, 0));
    assert!(try_send(&mut e, 0x17d4, 0x1001f));
    assert!(try_send(&mut e, 0x17d8, 1));
    assert!(try_send(&mut e, 0x1434, 0));
    assert!(try_send(&mut e, 0x1644, 0));
    assert!(
        e.index_buffer(0, 3).is_err(),
        "missing INSTANCE_BASE must fail"
    );

    // Missing PRIM_RESTART (0x1644)
    let mut e = Maxwell3d::default();
    assert!(try_send(&mut e, 0x0000, 0xb197));
    assert!(try_send(&mut e, 0x17c8, 0));
    assert!(try_send(&mut e, 0x17cc, 0x10000));
    assert!(try_send(&mut e, 0x17d0, 0));
    assert!(try_send(&mut e, 0x17d4, 0x1001f));
    assert!(try_send(&mut e, 0x17d8, 1));
    assert!(try_send(&mut e, 0x1434, 0));
    assert!(try_send(&mut e, 0x1438, 0));
    assert!(
        e.index_buffer(0, 3).is_err(),
        "missing PRIM_RESTART must fail"
    );
}

#[test]
fn unsupported_formats_and_non_zero_restrictions() {
    let mut e = setup_u16_engine();

    // Format zero is the documented one-byte input, widened only by the host backend.
    assert!(try_send(&mut e, 0x17d8, 0));
    assert_eq!(e.index_buffer(0, 3).unwrap().width, 1);

    // Format 3 or higher is undefined in Maxwell
    assert!(!try_send(&mut e, 0x17d8, 3));
    assert!(!try_send(&mut e, 0x17d8, 4));
    assert!(!try_send(&mut e, 0x17d8, 0xffff));

    // High 8-bit address validation: > 255 rejected
    assert!(!try_send(&mut e, 0x17c8, 256));
    assert!(!try_send(&mut e, 0x17d0, 256));

    // Exactly 255 (top of 40-bit address space) accepted
    assert!(try_send(&mut e, 0x17c8, 255));
    assert!(try_send(&mut e, 0x17d0, 255));

    // Non-zero instance base rejected
    assert!(!try_send(&mut e, 0x1438, 1));
    assert!(!try_send(&mut e, 0x1438, u32::MAX));

    // Non-zero primitive restart rejected
    assert!(!try_send(&mut e, 0x1644, 1));
    assert!(!try_send(&mut e, 0x1644, 0xffff));
}

#[test]
fn count_and_first_boundary_adversarial() {
    let mut e = setup_u16_engine();

    // Zero count rejected
    assert!(e.index_buffer(0, 0).is_err());

    // Non-multiple of 3 rejected
    assert!(e.index_buffer(0, 1).is_err());
    assert!(e.index_buffer(0, 2).is_err());
    assert!(e.index_buffer(0, 4).is_err());
    assert!(e.index_buffer(0, 5).is_err());

    // Extreme first index causes overflow or limit overrun
    assert!(e.index_buffer(u32::MAX, 3).is_err());
    assert!(e.index_buffer(u32::MAX / 2, 3).is_err());

    // Bounded scan budget isolation:
    // Expand inclusive limit to 4 MiB (0x10000..=0x40ffff) to ensure limits do not trigger
    assert!(try_send(&mut e, 0x17d4, 0x40ffff));

    // Maximum supported divisible element count: 1_048_575 elements (<= 1<<20 elements).
    // In U16, 1_048_575 elements = 2_097_150 bytes; last byte is 0x10000 + 2_097_150 - 1 = 0x20fffd <= 0x40ffff.
    let max_ok = e.index_buffer(0, 1_048_575);
    assert!(max_ok.is_ok(), "max divisible count 1,048,575 must resolve");
    assert_eq!(max_ok.unwrap().bytes, 2_097_150);

    // First divisible element count exceeding the 1M-element budget (1,048,578 elements):
    // Must be rejected strictly for exceeding scan budget, not for limit overrun.
    let budget_err = e.index_buffer(0, 1_048_578).unwrap_err();
    assert!(
        budget_err
            .to_string()
            .contains("unsupported index scan budget exceeds one million elements"),
        "error must be scan budget exceeded, got: {budget_err}"
    );
}

#[test]
fn limit_less_than_start_and_40_bit_boundary() {
    let mut e = setup_u16_engine();

    // limit < start
    assert!(try_send(&mut e, 0x17cc, 0x50000));
    assert!(try_send(&mut e, 0x17d4, 0x4ffff));
    assert!(
        e.index_buffer(0, 3).is_err(),
        "limit < start must be rejected"
    );

    // limit == start (only 1 byte available, but count=3 needs at least 6 bytes)
    assert!(try_send(&mut e, 0x17d4, 0x50000));
    assert!(
        e.index_buffer(0, 3).is_err(),
        "limit == start cannot fit 6 bytes"
    );

    // 40-bit boundary wrap / overflow: start near (1 << 40)
    let mut e40 = setup_u16_engine();
    assert!(try_send(&mut e40, 0x17c8, 255));
    assert!(try_send(&mut e40, 0x17cc, 0xffff_fff0));
    assert!(try_send(&mut e40, 0x17d0, 255));
    assert!(try_send(&mut e40, 0x17d4, 0xffff_ffff));
    // 0xffff_fff0 to 0xffff_ffff is 16 bytes. Count=6 is 12 bytes.
    assert!(e40.index_buffer(0, 6).is_ok());

    // Count=9 is 18 bytes, which exceeds 16 bytes
    assert!(e40.index_buffer(0, 9).is_err());

    // Address + bytes exceeding 1 << 40
    assert!(try_send(&mut e40, 0x17cc, 0xffff_fffe));
    assert!(e40.index_buffer(0, 3).is_err());
}

#[test]
fn vertex_begin_end_and_draw_state_machine() {
    let mut e = setup_u16_engine();

    // Cannot draw before BEGIN
    assert!(!try_send(&mut e, 0x17e0, 6));

    // Non-triangle primitive modes in VERTEX_BEGIN_GL (0x1618) rejected:
    // 0 = POINTS, 1 = LINES, 2 = LINE_LOOP, 3 = LINE_STRIP, 5 = TRIANGLE_STRIP, etc.
    assert!(!try_send(&mut e, 0x1618, 0));
    assert!(!try_send(&mut e, 0x1618, 1));
    assert!(!try_send(&mut e, 0x1618, 2));
    assert!(!try_send(&mut e, 0x1618, 3));
    assert!(!try_send(&mut e, 0x1618, 5));
    assert!(!try_send(&mut e, 0x1618, 7));

    // Valid BEGIN (4 = TRIANGLES)
    assert!(try_send(&mut e, 0x1618, 4));

    // Cannot call BEGIN again while active
    assert!(!try_send(&mut e, 0x1618, 4));

    // Draw without setting first (0x17dc) would fail, but setup already sets first=0
    // Draw with count not divisible by 3 rejected
    assert!(!try_send(&mut e, 0x17e0, 4));
    assert!(!try_send(&mut e, 0x17e0, 0));

    // Valid Draw
    let effect = send(&mut e, 0x17e0, 6).unwrap();
    assert_eq!(effect, MethodEffect::DrawIndices { first: 0, count: 6 });

    // Mutate first index during active begin: change to first=3
    assert!(try_send(&mut e, 0x17dc, 3));
    let effect2 = send(&mut e, 0x17e0, 6).unwrap();
    assert_eq!(effect2, MethodEffect::DrawIndices { first: 3, count: 6 });

    // Valid END (0x1614, value must be 0)
    assert!(!try_send(&mut e, 0x1614, 1)); // nonzero rejected
    assert!(try_send(&mut e, 0x1614, 0));

    // Cannot call END again
    assert!(!try_send(&mut e, 0x1614, 0));

    // Cannot draw after END
    assert!(!try_send(&mut e, 0x17e0, 6));
}

#[test]
fn decoder_pushbuffer_chunk_split_preserves_draw_indices() {
    let mut decoder = Decoder::default();
    let mut engine = Maxwell3d::default();

    // Command stream setting up indexed draw across split packets
    let words = [
        0x20010000, 0xb197,     // set subchannel 0 class 0xb197
        0x200505f2, // 5 methods starting at 0x17c8 (0x5f2 * 4)
        0, 0x10000, // start high, low
        0, 0x1002f, // limit high, low (48 bytes)
        1,       // format 1 (u16)
        0x2001050d, 0, // base vertex 0
        0x2001050e, 0, // base instance 0
        0x20010591, 0, // primitive restart 0
        0x20010586, 4, // VERTEX_BEGIN_GL = TRIANGLES
        0x200105f7, 2, // INDEX_BATCH_FIRST = 2
        0x200105f8, 9, // INDEX_BATCH_COUNT = 9 -> DrawIndices
        0x20010585, 0, // VERTEX_END_GL = 0
    ];

    let mut last_draw = None;
    // Feed word by word to verify packet state across individual word chunks
    for &w in &words {
        if let Some(method) = decoder.push_word(w).unwrap() {
            let effect = engine.apply(method).unwrap();
            if matches!(effect, MethodEffect::DrawIndices { .. }) {
                last_draw = Some(effect);
            }
        }
    }

    // Verify decoder finishes cleanly with no remaining truncated packets
    assert!(decoder.finish().is_ok());

    assert_eq!(
        last_draw,
        Some(MethodEffect::DrawIndices { first: 2, count: 9 })
    );

    // Verify the resolved index buffer matches
    let ib = engine.index_buffer(2, 9).unwrap();
    assert_eq!(ib.address, GpuAddress(0x10004));
    assert_eq!(ib.bytes, 18);
    assert_eq!(ib.width, 2);
    assert_eq!(ib.base, 0);
}
