#![cfg(feature = "wgpu-backend")]
mod common;
use common::*;
use wl_gpu::{backend::Renderer, command::Command, format::Format, memory::*, texture::*};

#[test]
#[ignore = "requires actual headless adapter; run explicitly with --ignored --nocapture"]
fn guest_tegra_mips_alias_updates_and_sampler_execute() {
    let mut r = Renderer::new().expect("GPU execution requested");
    println!("adapter={:?}", r.capabilities);
    let d = desc(37, 35, 6);
    let (mut source, expected) = rgba_surface(d);
    let mut map = AddressMap::default();
    map.map(d.address, GuestAddress(0x80000), source.len() as u64)
        .unwrap();
    map.map(
        GpuAddress(0x20000),
        GuestAddress(0x80000),
        source.len() as u64,
    )
    .unwrap();
    let mut changes = TextureChanges::new(1);
    let id = TextureId(1);
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
    for mip in 0..6 {
        let frame = r
            .execute(
                (37 >> mip).max(1),
                (35 >> mip).max(1),
                &[Command::DrawTexture { id, mip }],
            )
            .unwrap();
        assert_pixels(&frame.rgba, &expected[mip as usize], 0);
    }
    let uploads = r.counters.texture_uploads;
    let allocations = r.counters.texture_allocations;
    for _ in 0..4 {
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
    assert_eq!(r.counters.texture_uploads, uploads);
    assert_eq!(r.counters.texture_allocations, allocations);
    // Canonical bytes spanning GOB columns and a mip boundary conservatively refresh
    // exactly affected mips; the same guest changes invalidate aliased texture IDs.
    let alias = TextureDesc {
        address: GpuAddress(0x20000),
        ..d
    };
    r.sync_texture(
        TextureId(2),
        alias,
        &map,
        &SliceMemory {
            base: GuestAddress(0x80000),
            bytes: &source,
        },
        &changes,
    )
    .unwrap();
    let index = gob_address(64, 9, 37 * 4, 2);
    source[index..index + 4].copy_from_slice(&[9, 87, 231, 113]);
    changes
        .mark_write(GuestAddress(0x80000 + index as u64), 4)
        .unwrap();
    changes
        .mark_write(GuestAddress(0x80000 + index as u64 + 1), 2)
        .unwrap();
    let before = r.counters;
    for (id, desc) in [(id, d), (TextureId(2), alias)] {
        r.sync_texture(
            id,
            desc,
            &map,
            &SliceMemory {
                base: GuestAddress(0x80000),
                bytes: &source,
            },
            &changes,
        )
        .unwrap();
        let frame = r
            .execute(37, 35, &[Command::DrawTexture { id, mip: 0 }])
            .unwrap();
        let mut pixels = expected[0].clone();
        let linear = (9 * 37 + 16) * 4;
        pixels[linear..linear + 4].copy_from_slice(&[9, 87, 231, 113]);
        assert_pixels(&frame.rgba, &pixels, 0);
    }
    assert_eq!(r.counters.texture_uploads - before.texture_uploads, 2);
    changes
        .mark_write(GuestAddress(0x80000 + 9216 - 2), 4)
        .unwrap();
    let before = r.counters.texture_uploads;
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
    assert_eq!(r.counters.texture_uploads - before, 2);
    // Truncated final mip must fail before allocating/replacing the resident resource.
    let before = r.counters.texture_allocations;
    assert!(
        r.sync_texture(
            TextureId(3),
            d,
            &map,
            &SliceMemory {
                base: GuestAddress(0x80000),
                bytes: &source[..source.len() - 1]
            },
            &changes
        )
        .is_err()
    );
    assert_eq!(r.counters.texture_allocations, before);
    assert!(
        r.execute(1, 1, &[Command::DrawTexture { id, mip: 6 }])
            .is_err()
    );
    r.remove_texture(id).unwrap();
    assert!(
        r.execute(1, 1, &[Command::DrawTexture { id, mip: 0 }])
            .is_err()
    );
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
    // Replacing a mapping changes canonical backing without changing resource ID/VA.
    let (original, _) = rgba_surface(d);
    let mut replacement = AddressMap::default();
    replacement
        .map(d.address, GuestAddress(0xa0000), original.len() as u64)
        .unwrap();
    let before = r.counters.texture_allocations;
    r.sync_texture(
        id,
        d,
        &replacement,
        &SliceMemory {
            base: GuestAddress(0xa0000),
            bytes: &original,
        },
        &changes,
    )
    .unwrap();
    assert_eq!(r.counters.texture_allocations, before);
    assert_pixels(
        &r.execute(37, 35, &[Command::DrawTexture { id, mip: 0 }])
            .unwrap()
            .rgba,
        &expected[0],
        0,
    );
    // Bilinear filtering: four independently specified corner colors average at center.
    let linear = [
        255, 0, 0, 255, 0, 255, 0, 255, 0, 0, 255, 255, 255, 255, 255, 255,
    ];
    let bilinear = TextureDesc {
        address: GpuAddress(0x30000),
        width: 2,
        height: 2,
        mip_count: 1,
        format: Format::Rgba8Unorm,
        layout: TextureLayout::Linear,
        filter: Filter::Linear,
    };
    map.map(bilinear.address, GuestAddress(0x90000), 16)
        .unwrap();
    r.sync_texture(
        TextureId(4),
        bilinear,
        &map,
        &SliceMemory {
            base: GuestAddress(0x90000),
            bytes: &linear,
        },
        &changes,
    )
    .unwrap();
    let frame = r
        .execute(
            1,
            1,
            &[Command::DrawTexture {
                id: TextureId(4),
                mip: 0,
            }],
        )
        .unwrap();
    assert_pixels(&frame.rgba, &[128, 128, 128, 255], 1);
    // sRGB affects RGB sampling but preserves alpha. Independent transfer equation.
    let srgb = TextureDesc {
        filter: Filter::Nearest,
        format: Format::Rgba8Srgb,
        ..bilinear
    };
    let values = [
        128, 64, 200, 73, 128, 64, 200, 73, 128, 64, 200, 73, 128, 64, 200, 73,
    ];
    r.sync_texture(
        TextureId(5),
        srgb,
        &map,
        &SliceMemory {
            base: GuestAddress(0x90000),
            bytes: &values,
        },
        &TextureChanges::new(2),
    )
    .unwrap();
    let frame = r
        .execute(
            2,
            2,
            &[Command::DrawTexture {
                id: TextureId(5),
                mip: 0,
            }],
        )
        .unwrap();
    let rgb = [128_u8, 64, 200].map(|v| {
        let s = v as f64 / 255.0;
        (if s <= 0.04045 {
            s / 12.92
        } else {
            ((s + 0.055) / 1.055).powf(2.4)
        } * 255.0)
            .round() as u8
    });
    for pixel in frame.rgba.chunks_exact(4) {
        assert_pixels(pixel, &[rgb[0], rgb[1], rgb[2], 73], 1);
    }
    println!(
        "texture counters={:?}; stage timings={:?}",
        r.counters, r.texture_timings
    );
}
