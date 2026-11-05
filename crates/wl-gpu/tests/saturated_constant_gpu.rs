#![cfg(feature = "wgpu-backend")]
use wl_gpu::{
    backend::Renderer, command::Command, fifo::Decoder, maxwell::*, memory::*,
    shader_program::FragmentProgram, texture::TextureChanges,
};

#[test]
#[ignore = "requires actual GPU; explicitly run --ignored --nocapture"]
fn saturated_guest_constants_flush_nan_and_bound_colors() {
    let mut renderer = Renderer::new().expect("actual GPU requested");
    println!("adapter={:?}", renderer.capabilities);
    let program =
        FragmentProgram::decode(include_bytes!("fixtures/maxwell/saturated-constant.bin")).unwrap();
    let mut engine = Maxwell3d::default();
    let mut fifo = Decoder::default();
    for word in [
        0x20010000, CLASS, 0x200308e0, 32, 0, 0x10000, 0x20010924, 0x31,
    ] {
        if let Some(write) = fifo.push_word(word).unwrap() {
            engine.apply(write).unwrap();
        }
    }
    fifo.finish().unwrap();
    let mut map = AddressMap::default();
    map.map(GpuAddress(0x10000), GuestAddress(0), 32).unwrap();
    let mut bytes = [0u8; 32];
    let mut changes = TextureChanges::new(1);
    renderer.set_resource_fragment(&program).unwrap();
    let modules = renderer.counters.shader_module_allocations;
    let pipelines = renderer.counters.pipeline_allocations;
    // Literal expected UNORM pixels, independent of the production FTZ/SAT code.
    for (bits, expected) in [
        (
            [0x3e800000u32, 0x3f000000, 0x3f400000, 0x3f800000],
            [64, 128, 191, 255],
        ),
        (
            [0xbf800000, 0x40000000, 0x7f800000, 0xff800000],
            [0, 255, 255, 0],
        ),
        (
            [0x7fc00001, 0x7f800001, 0xffc00001, 0x3f800000],
            [0, 0, 0, 255],
        ),
        ([1, 0x80000001, 0x80000000, 0], [0, 0, 0, 0]),
    ] {
        for (n, value) in bits.iter().enumerate() {
            bytes[16 + n * 4..20 + n * 4].copy_from_slice(&value.to_le_bytes());
        }
        changes.mark_write(GuestAddress(16), 16).unwrap();
        renderer
            .sync_fragment_resources(
                &engine,
                4,
                &map,
                &SliceMemory {
                    base: GuestAddress(0),
                    bytes: &bytes,
                },
                &changes,
            )
            .unwrap();
        let frame = renderer.execute(37, 19, &[Command::DrawFragment]).unwrap();
        for pixel in frame.rgba.chunks_exact(4) {
            assert_eq!(pixel, expected);
        }
        assert_eq!(renderer.counters.shader_module_allocations, modules);
        assert_eq!(renderer.counters.pipeline_allocations, pipelines);
    }
}
