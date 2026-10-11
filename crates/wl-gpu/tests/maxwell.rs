mod common;
use common::maxwell::*;
use wl_gpu::{fifo::*, format::Format, maxwell::*, memory::*, texture::*};
fn write(method: u16, value: u32) -> MethodWrite {
    MethodWrite {
        subchannel: 0,
        method,
        value,
    }
}
fn pools() -> AddressMap {
    let mut map = AddressMap::default();
    map.map(GpuAddress(0x30000), GuestAddress(0x80000), 32)
        .unwrap();
    map.map(GpuAddress(0x31000), GuestAddress(0x80020), 32)
        .unwrap();
    map
}
#[test]
fn literal_modern_tic_tsc_fields_and_rejections() {
    let image = TextureImage::decode(TIC_RGBA).unwrap();
    assert_eq!(
        (
            image.address,
            image.width,
            image.height,
            image.mip_count,
            image.format,
            image.layout
        ),
        (
            GpuAddress(0x10000),
            37,
            19,
            1,
            Format::Rgba8Unorm,
            TextureLayout::BlockLinear {
                block_height_log2: 2
            }
        )
    );
    assert_eq!(
        TextureImage::decode(TIC_BC1).unwrap().format,
        Format::Bc1Rgba
    );
    assert_eq!(
        TextureImage::decode(TIC_ASTC).unwrap().format,
        Format::Astc4x4
    );
    let mut high = TIC_RGBA;
    high[2] |= 0xabcd;
    assert_eq!(
        TextureImage::decode(high).unwrap().address,
        GpuAddress(0xabcd00010000)
    );
    let mut mips = TIC_RGBA;
    mips[3] |= 2 << 28;
    mips[7] = 0x20;
    assert_eq!(TextureImage::decode(mips).unwrap().mip_count, 3);
    let mut srgb = TIC_RGBA;
    srgb[4] |= 1 << 22;
    assert_eq!(
        TextureImage::decode(srgb).unwrap().format,
        Format::Rgba8Srgb
    );
    for (word, bit) in [
        (0, 1 << 19),
        (1, 1),
        (1, 1 << 5),
        (2, 1 << 16),
        (3, 1),
        (3, 1 << 6),
        (4, 1 << 27),
        (5, 1 << 16),
        (6, 1),
        (7, 1 << 8),
    ] {
        let mut words = TIC_RGBA;
        words[word] ^= bit;
        assert!(
            TextureImage::decode(words).is_err(),
            "unsupported word{word} bit{bit:x}"
        );
    }
    for (word, value) in [
        (0, 0x58d2497f),
        (2, 0x00400000),
        (3, 0x38),
        (4, 0x01000024),
        (5, 0x12),
        (7, 0x10),
        (3, 15 << 28),
    ] {
        let mut words = TIC_RGBA;
        words[word] = value;
        assert!(TextureImage::decode(words).is_err());
    }
    assert_eq!(
        TextureSampler::decode(TSC_NEAREST).unwrap().filter,
        Filter::Nearest
    );
    let mut linear = TSC_NEAREST;
    linear[1] = 0x62;
    assert_eq!(
        TextureSampler::decode(linear).unwrap().filter,
        Filter::Linear
    );
    for (word, value) in [
        (0, 0),
        (0, 0x292),
        (1, 0x50),
        (1, 0x61),
        (1, 0x151),
        (2, 0x1000),
        (3, 1),
        (4, 1),
    ] {
        let mut words = TSC_NEAREST;
        words[word] = value;
        assert!(TextureSampler::decode(words).is_err());
    }
}
#[test]
fn fifo_pool_state_chunks_shared_subchannels_and_bounds() {
    for chunk in 1..=POOL_PACKETS.len() {
        let mut fifo = Decoder::default();
        let mut engine = Maxwell3d::default();
        for words in POOL_PACKETS.chunks(chunk) {
            for &word in words {
                if let Some(method) = fifo.push_word(word).unwrap() {
                    engine.apply(method).unwrap();
                }
            }
        }
        fifo.finish().unwrap();
        let bytes = descriptor_bytes(TIC_RGBA, TSC_NEAREST);
        let memory = SliceMemory {
            base: GuestAddress(0x80000),
            bytes: &bytes,
        };
        let d = engine.resolve_texture(0, 0, 0, &pools(), &memory).unwrap();
        assert_eq!(d.width, 37);
        assert!(engine.resolve_texture(0, 1, 0, &pools(), &memory).is_err());
        assert!(engine.resolve_texture(0, 0, 1, &pools(), &memory).is_err());
        assert!(engine.resolve_texture(1, 0, 0, &pools(), &memory).is_err());
        engine
            .apply(MethodWrite {
                subchannel: 1,
                method: 0,
                value: CLASS,
            })
            .unwrap();
        assert_eq!(
            engine.resolve_texture(1, 0, 0, &pools(), &memory).unwrap(),
            d
        );
        // A rejected high field does not truncate the current pool base.
        assert!(engine.apply(write(0x1574, 0x100)).is_err());
        assert_eq!(
            engine.resolve_texture(0, 0, 0, &pools(), &memory).unwrap(),
            d
        );
        engine.apply(write(0x1578, 0x30001)).unwrap();
        assert!(engine.resolve_texture(0, 0, 0, &pools(), &memory).is_err());
    }
    let mut engine = Maxwell3d::default();
    assert_eq!(
        engine.apply(write(0x1574, 0)),
        Err(MethodError::UnboundSubchannel(0))
    );
    assert!(engine.apply(write(0, 0xb097)).is_err());
    assert!(
        engine
            .apply(MethodWrite {
                subchannel: 8,
                method: 0,
                value: CLASS
            })
            .is_err()
    );
    engine.apply(write(0, CLASS)).unwrap();
    assert!(engine.apply(write(3, 0)).is_err());
    assert!(engine.apply(write(0x4000, 0)).is_err());
    assert!(engine.apply(write(0xf10, 0)).is_err());
    assert!(engine.apply(write(0x157c, 0x400000)).is_err());
    assert!(engine.apply(write(0x1564, 0x100000)).is_err());
    assert!(engine.apply(write(0x2608, 0)).is_err()); // cbuf slot is NOT a texture bind
    assert!(engine.apply(write(0x1334, 0)).is_err()); // WFI effects are not implemented
    assert!(
        engine
            .resolve_texture(
                0,
                0,
                0,
                &pools(),
                &SliceMemory {
                    base: GuestAddress(0x80000),
                    bytes: &[0; 64]
                }
            )
            .is_err()
    );
}
#[test]
fn descriptor_read_truncation_mapping_and_invalidation() {
    let mut engine = configured();
    let bytes = descriptor_bytes(TIC_RGBA, TSC_NEAREST);
    let memory = SliceMemory {
        base: GuestAddress(0x80000),
        bytes: &bytes,
    };
    assert!(
        engine
            .resolve_texture(0, 0, 0, &AddressMap::default(), &memory)
            .is_err()
    );
    assert!(
        engine
            .resolve_texture(
                0,
                0,
                0,
                &pools(),
                &SliceMemory {
                    base: GuestAddress(0x80000),
                    bytes: &bytes[..63]
                }
            )
            .is_err()
    );
    assert_eq!(
        engine.apply(write(0x1428, 0)),
        Ok(MethodEffect::TextureHeadersInvalidated(None))
    );
    assert_eq!(
        engine.apply(write(0x1424, 0x31)),
        Ok(MethodEffect::SamplersInvalidated(Some(3)))
    );
    assert_eq!((engine.texture_header_epoch, engine.sampler_epoch), (1, 1));
    assert!(engine.apply(write(0x1428, 2)).is_err());
    // No cached descriptor can hide a backing update, including a format change.
    let new_bytes = descriptor_bytes(TIC_ASTC, TSC_NEAREST);
    assert_eq!(
        engine
            .resolve_texture(
                0,
                0,
                0,
                &pools(),
                &SliceMemory {
                    base: memory.base,
                    bytes: &new_bytes
                }
            )
            .unwrap()
            .format,
        Format::Astc4x4
    );
    // Pool addresses are 40-bit; descriptor image addresses above retain 48 bits.
    engine.apply(write(0x1574, 255)).unwrap();
    engine.apply(write(0x1578, 0xffffffe0)).unwrap();
    engine.apply(write(0x157c, 1)).unwrap();
    assert!(engine.resolve_texture(0, 1, 0, &pools(), &memory).is_err());
}
