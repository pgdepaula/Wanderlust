#![cfg(feature = "wgpu-backend")]
use wl_gpu::{
    backend::Renderer, command::Command, maxwell::Maxwell3d, memory::*,
    shader_program::FragmentProgram, texture::TextureChanges,
};

#[test]
#[ignore = "requires actual GPU; explicitly run --ignored --nocapture"]
fn independently_assembled_kil_preserves_discarded_pixels_on_each_half() {
    let mut renderer = Renderer::new().expect("actual GPU requested");
    println!("adapter={:?}", renderer.capabilities);
    for (condition, killed_left) in [(0u64, true), (8, false)] {
        let mut binary = include_bytes!("fixtures/maxwell/kill.bin").to_vec();
        binary[120..128].copy_from_slice(&(0xe33000000000000fu64 | condition << 16).to_le_bytes());
        let program = FragmentProgram::decode(&binary).unwrap();
        renderer.set_resource_fragment(&program).unwrap();
        renderer
            .sync_fragment_resources(
                &Maxwell3d::default(),
                4,
                &AddressMap::default(),
                &SliceMemory {
                    base: GuestAddress(0),
                    bytes: &[],
                },
                &TextureChanges::new(1),
            )
            .unwrap();
        let frame = renderer.execute(64, 19, &[Command::DrawFragment]).unwrap();
        for y in 0..19 {
            for x in 0..64 {
                let expected = if (x < 32) == killed_left {
                    [0, 0, 0, 255]
                } else {
                    [255, 0, 0, 255]
                };
                assert_eq!(
                    frame.rgba[(y * 64 + x) * 4..(y * 64 + x + 1) * 4],
                    expected,
                    "pixel {x},{y}, condition={condition}"
                );
            }
        }
    }
}
