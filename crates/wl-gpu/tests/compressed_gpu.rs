#![cfg(feature = "wgpu-backend")]
mod common;
use common::{compressed::*, *};
use wl_gpu::{
    TextureUploadPolicy, backend::Renderer, command::Command, format::Format, memory::*, texture::*,
};

#[test]
#[ignore = "requires actual GPU; explicitly run --ignored --nocapture"]
fn compressed_guest_textures_sample_native_and_fallback() {
    let mut r = Renderer::new().expect("GPU execution requested");
    println!("adapter={:?}", r.capabilities);
    for (number, format) in [
        Format::Astc4x4,
        Format::Bc1Rgba,
        Format::Astc4x4Srgb,
        Format::Bc1RgbaSrgb,
    ]
    .into_iter()
    .enumerate()
    {
        // Aligned base allows native BC1. Edge blocks in narrow mips need padded copies.
        let d = TextureDesc {
            format,
            ..desc(36, 36, 6)
        };
        let (mut source, expected) = surface(d);
        let mut map = AddressMap::default();
        map.map(d.address, GuestAddress(0x80000), source.len() as u64)
            .unwrap();
        let mut changes = TextureChanges::new(1);
        let id = TextureId(number as u64 + 1);
        for policy in [TextureUploadPolicy::Auto, TextureUploadPolicy::Software] {
            r.sync_texture_with_policy(
                id,
                d,
                &map,
                &SliceMemory {
                    base: GuestAddress(0x80000),
                    bytes: &source,
                },
                &changes,
                policy,
            )
            .unwrap();
            let native = r.texture_host_format(id).unwrap().is_compressed();
            if policy == TextureUploadPolicy::Software {
                assert!(!native);
            }
            if format == Format::Bc1Rgba && policy == TextureUploadPolicy::Auto {
                assert_eq!(
                    native,
                    r.capabilities
                        .enabled
                        .contains(wgpu::Features::TEXTURE_COMPRESSION_BC)
                );
            }
            for (mip, pixels) in expected.iter().enumerate() {
                let frame = r
                    .execute(
                        (d.width >> mip).max(1),
                        (d.height >> mip).max(1),
                        &[Command::DrawTexture {
                            id,
                            mip: mip as u32,
                        }],
                    )
                    .unwrap();
                let mut expected = pixels.clone();
                if matches!(format, Format::Astc4x4Srgb | Format::Bc1RgbaSrgb) {
                    for p in expected.chunks_exact_mut(4) {
                        for c in &mut p[..3] {
                            let s = f64::from(*c) / 255.;
                            *c = (if s <= 0.04045 {
                                s / 12.92
                            } else {
                                ((s + 0.055) / 1.055).powf(2.4)
                            } * 255.)
                                .round() as u8;
                        }
                    }
                }
                if native && format == Format::Bc1RgbaSrgb {
                    // BC interpolation may differ by one encoded UNORM step.
                    // Propagate that interval through sRGB, rather than allowing
                    // a broad tolerance on all channels or ignoring alpha.
                    let linear = |v: u8| {
                        let s = f64::from(v) / 255.;
                        (if s <= 0.04045 {
                            s / 12.92
                        } else {
                            ((s + 0.055) / 1.055).powf(2.4)
                        } * 255.)
                            .round() as u8
                    };
                    for (n, (&actual, &encoded)) in frame.rgba.iter().zip(pixels).enumerate() {
                        if n % 4 == 3 {
                            assert_eq!(actual, encoded);
                        } else {
                            let low = linear(encoded.saturating_sub(1)).saturating_sub(1);
                            let high = linear(encoded.saturating_add(1)).saturating_add(1);
                            assert!(
                                (low..=high).contains(&actual),
                                "byte {n}: {actual} outside {low}..={high}"
                            );
                        }
                    }
                } else {
                    assert_pixels(&frame.rgba, &expected, 1);
                }
                for (actual, expected) in frame.rgba.chunks_exact(4).zip(expected.chunks_exact(4)) {
                    assert_eq!(actual[3], expected[3]);
                }
            }
            println!(
                "format={format:?} policy={policy:?} host={:?}",
                r.texture_host_format(id).unwrap()
            );
        }
        let before = r.counters;
        for _ in 0..32 {
            r.sync_texture_with_policy(
                id,
                d,
                &map,
                &SliceMemory {
                    base: GuestAddress(0x80000),
                    bytes: &source,
                },
                &changes,
                TextureUploadPolicy::Software,
            )
            .unwrap();
        }
        assert_eq!(r.counters.upload_bytes, before.upload_bytes);
        assert_eq!(r.counters.decompress_bytes, before.decompress_bytes);
        assert_eq!(
            r.counters.texture_cache_hits - before.texture_cache_hits,
            32
        );
        // Change a complete block; conservative refresh affects exactly one mip.
        let level = d.levels().unwrap()[1];
        let block = if matches!(format, Format::Astc4x4 | Format::Astc4x4Srgb) {
            astc_constant([255, 0, 0, 255]).to_vec()
        } else {
            BC1.to_vec()
        };
        source[level.offset..level.offset + block.len()].copy_from_slice(&block);
        changes
            .mark_write(GuestAddress(0x80000 + level.offset as u64), block.len())
            .unwrap();
        changes
            .mark_write(GuestAddress(0x80000 + level.offset as u64 + 1), 2)
            .unwrap();
        let before = r.counters.texture_uploads;
        r.sync_texture_with_policy(
            id,
            d,
            &map,
            &SliceMemory {
                base: GuestAddress(0x80000),
                bytes: &source,
            },
            &changes,
            TextureUploadPolicy::Software,
        )
        .unwrap();
        assert_eq!(r.counters.texture_uploads - before, 1);
        let frame = r
            .execute(18, 18, &[Command::DrawTexture { id, mip: 1 }])
            .unwrap();
        if matches!(format, Format::Astc4x4 | Format::Astc4x4Srgb) {
            assert_eq!(&frame.rgba[..4], &[255, 0, 0, 255]);
        }
        if format == Format::Astc4x4 {
            let saved = source[..16].to_vec();
            source[..16].fill(0);
            changes.mark_write(GuestAddress(0x80000), 16).unwrap();
            let before = r.counters.texture_uploads;
            assert!(
                r.sync_texture(
                    id,
                    d,
                    &map,
                    &SliceMemory {
                        base: GuestAddress(0x80000),
                        bytes: &source
                    },
                    &changes
                )
                .is_err()
            );
            assert_eq!(r.counters.texture_uploads, before);
            // Failed conversion preserves the previously sampled resident image.
            let frame = r
                .execute(36, 36, &[Command::DrawTexture { id, mip: 0 }])
                .unwrap();
            assert_pixels(&frame.rgba, &expected[0], 1);
            source[..16].copy_from_slice(&saved);
            changes.mark_write(GuestAddress(0x80000), 16).unwrap();
            r.sync_texture(
                id,
                d,
                &map,
                &SliceMemory {
                    base: GuestAddress(0x80000),
                    bytes: &source,
                },
                &changes,
            )
            .unwrap();
        }
        r.remove_texture(id).unwrap();
        r.sync_texture_with_policy(
            id,
            d,
            &map,
            &SliceMemory {
                base: GuestAddress(0x80000),
                bytes: &source,
            },
            &changes,
            TextureUploadPolicy::Software,
        )
        .unwrap();
        let recreated = r
            .execute(18, 18, &[Command::DrawTexture { id, mip: 1 }])
            .unwrap();
        assert_eq!(&recreated.rgba[..4], &[255, 0, 0, 255]);
        r.remove_texture(id).unwrap();
    }
    // Non-block-aligned source edges must use RGBA8 fallback, not a resized UV domain.
    for format in [Format::Astc4x4, Format::Bc1Rgba] {
        let d = TextureDesc {
            format,
            ..desc(37, 35, 1)
        };
        let (source, pixels) = surface(d);
        let mut map = AddressMap::default();
        map.map(d.address, GuestAddress(0x90000), source.len() as u64)
            .unwrap();
        r.sync_texture(
            TextureId(99),
            d,
            &map,
            &SliceMemory {
                base: GuestAddress(0x90000),
                bytes: &source,
            },
            &TextureChanges::new(3),
        )
        .unwrap();
        assert!(
            !r.texture_host_format(TextureId(99))
                .unwrap()
                .is_compressed()
        );
        let frame = r
            .execute(
                37,
                35,
                &[Command::DrawTexture {
                    id: TextureId(99),
                    mip: 0,
                }],
            )
            .unwrap();
        assert_pixels(&frame.rgba, &pixels[0], 1);
    }
    println!("counters={:?}; timings={:?}", r.counters, r.texture_timings);
}
