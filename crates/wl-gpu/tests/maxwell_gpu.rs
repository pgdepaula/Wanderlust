#![cfg(feature = "wgpu-backend")]
mod common;
use common::{maxwell::*, *};
use wl_gpu::{
    backend::Renderer, command::Command, fifo::Decoder, format::Format, maxwell::*, memory::*,
    texture::*,
};

#[test]
#[ignore = "requires actual GPU; explicitly run --ignored --nocapture"]
fn literal_maxwell_pool_methods_resolve_and_sample_guest_textures() {
    let mut renderer = Renderer::new().expect("GPU execution requested");
    println!("adapter={:?}", renderer.capabilities);
    for words in [TIC_RGBA, TIC_BC1, TIC_ASTC] {
        let mut fifo = Decoder::default();
        let mut engine = Maxwell3d::default();
        // One-word chunks deliberately retain the pending packet across every boundary.
        for word in POOL_PACKETS {
            if let Some(write) = fifo.push_word(word).unwrap() {
                engine.apply(write).unwrap();
            }
        }
        fifo.finish().unwrap();
        let image = TextureImage::decode(words).unwrap();
        let d = image.with_sampler(TextureSampler::decode(TSC_NEAREST).unwrap());
        assert_eq!((d.width, d.height, d.mip_count), (37, 19, 1));
        let (surface, expected) = if d.format == Format::Rgba8Unorm {
            rgba_surface(d)
        } else {
            compressed::surface(d)
        };
        let mut backing = vec![0; 0x1000 + surface.len()];
        backing[..64].copy_from_slice(&descriptor_bytes(words, TSC_NEAREST));
        backing[0x1000..].copy_from_slice(&surface);
        let mut map = AddressMap::default();
        map.map(GpuAddress(0x30000), GuestAddress(0x80000), 32)
            .unwrap();
        map.map(GpuAddress(0x31000), GuestAddress(0x80020), 32)
            .unwrap();
        map.map(d.address, GuestAddress(0x81000), surface.len() as u64)
            .unwrap();
        let memory = SliceMemory {
            base: GuestAddress(0x80000),
            bytes: &backing,
        };
        // Explicit descriptor indices stand in for shader handles, not for hardware
        // methods. Only the pool/descriptor path is guest-command-driven here.
        let resolved = engine.resolve_texture(0, 0, 0, &map, &memory).unwrap();
        assert_eq!(resolved, d);
        let changes = TextureChanges::new(1);
        let id = TextureId(55);
        renderer
            .sync_texture(id, resolved, &map, &memory, &changes)
            .unwrap();
        let frame = renderer
            .execute(37, 19, &[Command::DrawTexture { id, mip: 0 }])
            .unwrap();
        assert_pixels(
            &frame.rgba,
            &expected[0],
            if d.format == Format::Rgba8Unorm { 0 } else { 1 },
        );
        let uploads = renderer.counters.texture_uploads;
        renderer
            .sync_texture(
                id,
                engine.resolve_texture(0, 0, 0, &map, &memory).unwrap(),
                &map,
                &memory,
                &changes,
            )
            .unwrap();
        assert_eq!(renderer.counters.texture_uploads, uploads);
        // Real NO_WFI header invalidation has no invented completion or texture-data
        // write effects. Repoint the pool to an independently located descriptor.
        let before = renderer.counters.synchronization_points;
        for word in [0x2001050a, 0] {
            if let Some(write) = fifo.push_word(word).unwrap() {
                assert_eq!(
                    engine.apply(write).unwrap(),
                    MethodEffect::TextureHeadersInvalidated(None)
                );
            }
        }
        assert_eq!(renderer.counters.synchronization_points, before);
        assert_eq!(engine.texture_header_epoch, 1);
        let mut replacement = words;
        replacement[1] = 0x20000;
        backing[0x100..0x120].copy_from_slice(&descriptor_bytes(replacement, TSC_NEAREST)[..32]);
        map.map(GpuAddress(0x30100), GuestAddress(0x80100), 32)
            .unwrap();
        map.map(
            GpuAddress(0x20000),
            GuestAddress(0x81000),
            surface.len() as u64,
        )
        .unwrap();
        for word in [0x2001055e, 0x30100] {
            if let Some(write) = fifo.push_word(word).unwrap() {
                engine.apply(write).unwrap();
            }
        }
        fifo.finish().unwrap();
        let memory = SliceMemory {
            base: GuestAddress(0x80000),
            bytes: &backing,
        };
        let resolved = engine.resolve_texture(0, 0, 0, &map, &memory).unwrap();
        assert_eq!(resolved.address, GpuAddress(0x20000));
        renderer
            .sync_texture(id, resolved, &map, &memory, &changes)
            .unwrap();
        let frame = renderer
            .execute(37, 19, &[Command::DrawTexture { id, mip: 0 }])
            .unwrap();
        assert_pixels(
            &frame.rgba,
            &expected[0],
            if d.format == Format::Rgba8Unorm { 0 } else { 1 },
        );
        println!(
            "resolved format={:?} image address={:?}",
            resolved.format, resolved.address
        );
    }
    println!("counters={:?}", renderer.counters);
}
