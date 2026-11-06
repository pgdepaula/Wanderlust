#![cfg(feature = "wgpu-backend")]
mod common;
use common::maxwell::*;
use wl_gpu::{
    backend::Renderer, command::Command, fifo::MethodWrite, memory::*,
    shader_program::FragmentProgram, texture::*,
};

#[test]
#[ignore = "requires actual GPU; explicitly run --ignored --nocapture"]
fn guest_tic_r8_pitch_swizzle_padding_mutation_and_warm_reuse_produce_exact_rgba() {
    let mut renderer = Renderer::new().expect("actual GPU requested");
    println!("adapter={:?}", renderer.capabilities.adapter);
    let mut engine = configured();
    for (method, value) in [(0x2380, 32), (0x2384, 0), (0x2388, 0x40000), (0x2490, 1)] {
        engine
            .apply(MethodWrite {
                subchannel: 0,
                method,
                value,
            })
            .unwrap();
    }
    let tic = [
        0x2ffa491d, 0x10000, 0x00400000, 1, 0xe3800006, 0x80000004, 0x03000000, 0,
    ];
    let tsc = [
        0x26080, 0x391, 0xfff00000, 0x0ffff000, 0x3f800000, 0x3f800000, 0x3f800000, 0x3f800000,
    ];
    let mut data = vec![0u8; 4096];
    data[..64].copy_from_slice(&descriptor_bytes(tic, tsc));
    data[256..416].fill(0xa5);
    for y in 0..5 {
        for x in 0..7 {
            data[256 + y * 32 + x] = (11 + x * 27 + y * 5) as u8;
        }
    }
    let padding = data[256..416].to_vec();
    let mut map = AddressMap::default();
    for (gpu, guest, size) in [
        (0x30000, 0, 32),
        (0x31000, 32, 32),
        (0x40000, 64, 32),
        (0x10000, 256, 160),
    ] {
        map.map(GpuAddress(gpu), GuestAddress(guest), size).unwrap();
    }
    let program = FragmentProgram::decode(include_bytes!("fixtures/maxwell/tex.bin")).unwrap();
    renderer.set_resource_fragment(&program).unwrap();
    let mut changes = TextureChanges::new(1);
    for cycle in 0..2 {
        renderer
            .sync_fragment_resources(
                &engine,
                4,
                &map,
                &SliceMemory {
                    base: GuestAddress(0),
                    bytes: &data,
                },
                &changes,
            )
            .unwrap();
        let frame = renderer.execute(7, 5, &[Command::DrawFragment]).unwrap();
        for y in 0..5 {
            for x in 0..7 {
                let expected = [
                    255,
                    255,
                    255,
                    if cycle == 1 && x == 3 && y == 2 {
                        229
                    } else {
                        (11 + x * 27 + y * 5) as u8
                    },
                ];
                assert_eq!(
                    frame.rgba[(y * 7 + x) * 4..(y * 7 + x + 1) * 4],
                    expected,
                    "{x},{y} cycle{cycle}"
                );
            }
        }
        let before = renderer.counters;
        for _ in 0..32 {
            renderer
                .sync_fragment_resources(
                    &engine,
                    4,
                    &map,
                    &SliceMemory {
                        base: GuestAddress(0),
                        bytes: &data,
                    },
                    &changes,
                )
                .unwrap();
        }
        assert_eq!(
            renderer.counters.guest_texture_bytes,
            before.guest_texture_bytes
        );
        assert_eq!(renderer.counters.upload_bytes, before.upload_bytes);
        assert_eq!(
            renderer.counters.texture_format_conversion_bytes,
            before.texture_format_conversion_bytes
        );
        if cycle == 0 {
            data[256 + 2 * 32 + 3] = 229;
            changes
                .mark_write(GuestAddress(256 + 2 * 32 + 3), 1)
                .unwrap();
        }
    }
    assert_eq!(renderer.counters.guest_texture_bytes, 320);
    assert_eq!(renderer.counters.texture_format_conversion_bytes, 280);
    for y in 0..5 {
        assert_eq!(
            &data[256 + y * 32 + 7..256 + (y + 1) * 32],
            &padding[y * 32 + 7..(y + 1) * 32]
        );
    }
    println!(
        "source={} converted={} uploads={} modules={} pipelines={}",
        renderer.counters.guest_texture_bytes,
        renderer.counters.texture_format_conversion_bytes,
        renderer.counters.texture_uploads,
        renderer.counters.shader_module_allocations,
        renderer.counters.pipeline_allocations
    );
}
