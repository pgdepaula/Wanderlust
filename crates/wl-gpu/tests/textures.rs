mod common;
use common::*;
use wl_gpu::{format::Format, memory::*, texture::*};

#[test]
fn mip_packing_matches_independent_geometry_and_surface_library() {
    let d = desc(37, 35, 6);
    let levels = d.levels().unwrap();
    assert_eq!(
        levels.iter().map(|m| m.offset).collect::<Vec<_>>(),
        [0, 9216, 13312, 13824, 14336, 14848]
    );
    assert_eq!(d.source_size().unwrap(), 15360);
    let (source, mips) = rgba_surface(d);
    let packed: Vec<_> = mips.into_iter().flatten().collect();
    let library = tegra_swizzle::surface::swizzle_surface(
        37,
        35,
        1,
        &packed,
        tegra_swizzle::surface::BlockDim::uncompressed(),
        Some(tegra_swizzle::BlockHeight::Two),
        4,
        6,
        1,
    )
    .unwrap();
    // Padding bytes are unspecified: verify only independently addressed texel bytes.
    for (n, m) in levels.iter().enumerate() {
        let mut counts = Default::default();
        let got = m
            .tiled
            .unwrap()
            .to_linear(&source[m.offset..m.offset + m.source_size], &mut counts)
            .unwrap();
        let reference = m
            .tiled
            .unwrap()
            .to_linear(&library[m.offset..m.offset + m.source_size], &mut counts)
            .unwrap();
        assert_eq!(got, rgba_mip(m.width, m.height, n as u32));
        assert_eq!(got, reference);
    }
}
#[test]
fn texture_bounds_and_canonical_dirty_versions() {
    let d = desc(37, 35, 6);
    for bad in [
        TextureDesc { width: 0, ..d },
        TextureDesc { mip_count: 0, ..d },
        TextureDesc { mip_count: 7, ..d },
        TextureDesc {
            address: GpuAddress(u64::MAX - 10),
            ..d
        },
        TextureDesc {
            width: u32::MAX,
            height: u32::MAX,
            ..d
        },
        TextureDesc {
            layout: TextureLayout::BlockLinear {
                block_height_log2: 6,
            },
            ..d
        },
    ] {
        assert!(bad.levels().is_err());
    }
    let mut versions = TextureChanges::new(7);
    assert_eq!(versions.stamp(100..200), (7, 0));
    versions.mark_write(GuestAddress(120), 4).unwrap();
    versions.mark_write(GuestAddress(122), 4).unwrap();
    assert_eq!(versions.stamp(100..200), (7, 2));
    assert_eq!(versions.stamp(200..220), (7, 0));
    for _ in 0..70 {
        versions.mark_write(GuestAddress(1000), 4).unwrap();
    }
    assert!(versions.stamp(200..220).1 > 0); // bounded journal evicts conservatively
    assert!(versions.mark_write(GuestAddress(u64::MAX), 2).is_err());
    assert!(versions.mark_write(GuestAddress(0), 0).is_err());
    assert_eq!(Format::Rgba8Srgb.block(), (1, 1, 4));
}
#[test]
fn texture_shader_validates_without_gpu() {
    let module = naga::front::wgsl::parse_str(include_str!("../src/texture.wgsl")).unwrap();
    naga::valid::Validator::new(
        naga::valid::ValidationFlags::all(),
        naga::valid::Capabilities::empty(),
    )
    .validate(&module)
    .unwrap();
}
