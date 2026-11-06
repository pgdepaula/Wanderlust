mod common;
use common::maxwell::*;
use wl_gpu::{format::Format, memory::*, texture::*};

const TIC: [u32; 8] = [
    0x2ffa491d, 0x10000, 0x00400000, 1, 0xe3800006, 0x80000004, 0x03000000, 0,
];
const TSC: [u32; 8] = [
    0x26080, 0x391, 0xfff00000, 0x0ffff000, 0x3f800000, 0x3f800000, 0x3f800000, 0x3f800000,
];
fn resolve(tic: [u32; 8], tsc: [u32; 8]) -> wl_gpu::Result<TextureDesc> {
    let engine = configured();
    let mut map = AddressMap::default();
    map.map(GpuAddress(0x30000), GuestAddress(0), 32)?;
    map.map(GpuAddress(0x31000), GuestAddress(32), 32)?;
    let bytes = descriptor_bytes(tic, tsc);
    engine.resolve_handle(
        0,
        0,
        &map,
        &SliceMemory {
            base: GuestAddress(0),
            bytes: &bytes,
        },
    )
}
#[test]
fn literal_pitched_r8_white_rgb_red_alpha_and_repeat_resolve() {
    let desc = resolve(TIC, TSC).unwrap();
    assert_eq!((desc.width, desc.height, desc.mip_count), (7, 5, 1));
    assert_eq!(desc.format, Format::R8Unorm);
    assert_eq!(desc.channels, TextureChannels::RedAsAlpha);
    assert_eq!(desc.wrap, [Wrap::Repeat; 2]);
    assert_eq!(desc.layout, TextureLayout::Pitch { bytes_per_row: 32 });
    assert_eq!(desc.source_size().unwrap(), 160);
    let mut rgba = TIC;
    rgba[0] = 0x58d24908;
    assert_eq!(resolve(rgba, TSC).unwrap().format, Format::Rgba8Unorm);
    let mut too_wide = rgba;
    too_wide[4] = (too_wide[4] & !65535) | 8;
    assert!(resolve(too_wide, TSC).is_err()); // Nine RGBA texels need 36 bytes.
}
#[test]
fn pitch_bad_swizzle_dimensions_layout_and_sampler_do_not_masquerade_as_supported() {
    for (index, value) in [
        (3, 0),
        (0, 0x58d2491d),
        (3, 0x10000001),
        (4, 0xe2800006),
        (5, 0x80010004),
        (2, 0x00200000),
    ] {
        let mut tic = TIC;
        tic[index] = value;
        assert!(resolve(tic, TSC).is_err(), "word{index}={value:x}");
    }
    for (index, value) in [(0, 0x26083), (0, 0x26280), (1, 0x02000391), (2, 1), (3, 1)] {
        let mut tsc = TSC;
        tsc[index] = value;
        assert!(resolve(TIC, tsc).is_err(), "sampler{index}={value:x}");
    }
    let mut desc = resolve(TIC, TSC).unwrap();
    desc.mip_count = 2;
    assert!(desc.source_size().is_err());
    desc.mip_count = 1;
    desc.width = u32::MAX;
    assert!(desc.source_size().is_err());
}
