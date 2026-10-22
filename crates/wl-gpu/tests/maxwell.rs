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

#[test]
fn documented_cache_preferences_are_separate_and_do_not_accept_volatile_or_reserved_modes() {
    let mut engine = Maxwell3d::default();
    engine.apply(write(0, CLASS)).unwrap();
    for method in CACHE_POLICY_METHODS {
        assert_eq!(engine.cache_eviction(method), None);
        for (value, policy) in [
            (0, CacheEviction::First),
            (0x10, CacheEviction::Normal),
            (0x20, CacheEviction::Last),
        ] {
            assert_eq!(engine.apply(write(method, value)), Ok(MethodEffect::State));
            assert_eq!(engine.cache_eviction(method), Some(policy));
        }
        for value in [1, 2, 0x30, 0x100, u32::MAX] {
            assert_eq!(
                engine.apply(write(method, value)),
                Err(MethodError::UnsupportedValue { method, value })
            );
            assert_eq!(engine.cache_eviction(method), Some(CacheEviction::Last));
        }
        assert_eq!(
            engine.apply(write(method + 1, 0)),
            Err(MethodError::InvalidMethod(method + 1))
        );
    }
    assert_eq!(engine.cache_eviction(0x1001), None);
    assert_eq!(Maxwell3d::default().cache_eviction(0x1000), None);
}

#[test]
fn shader_format_assertions_and_existing_independent_sampler_mode_are_strict() {
    let mut engine = Maxwell3d::default();
    engine.apply(write(0, CLASS)).unwrap();
    for (method, good) in [
        (0x16a8, 0x00030003),
        (0x1794, 0x00020002),
        (0x1234, 0),
        (0x1528, 0),
    ] {
        assert_eq!(engine.apply(write(method, good)), Ok(MethodEffect::State));
        for value in [good ^ 1, good ^ 0x10000, u32::MAX] {
            assert_eq!(
                engine.apply(write(method, value)),
                Err(MethodError::UnsupportedValue { method, value })
            );
        }
        assert_eq!(engine.apply(write(method, good)), Ok(MethodEffect::State));
    }
    // Checks on an auxiliary subchannel cannot be silently routed into B197.
    engine
        .apply(MethodWrite {
            subchannel: 1,
            method: 0,
            value: 0xb1c0,
        })
        .unwrap();
    assert!(matches!(
        engine.apply(MethodWrite {
            subchannel: 1,
            method: 0x16a8,
            value: 0x00030003
        }),
        Err(MethodError::UnsupportedEngineMethod { .. })
    ));
}

#[test]
fn channel_nop_is_independent_of_engine_binding_and_has_no_register_effects() {
    let mut engine = Maxwell3d::default();
    for subchannel in 0..8 {
        for value in [0, CLASS, u32::MAX] {
            assert_eq!(
                engine.apply(MethodWrite {
                    subchannel,
                    method: 8,
                    value
                }),
                Ok(MethodEffect::State)
            );
            assert_eq!(engine.bound_class(subchannel), None);
        }
    }
    assert_eq!(engine.method_writes(), 24);
    assert_eq!(
        engine.apply(MethodWrite {
            subchannel: 8,
            method: 8,
            value: 0
        }),
        Err(MethodError::InvalidSubchannel(8))
    );
    assert_eq!(
        engine.apply(write(0x100, 0)),
        Err(MethodError::UnboundSubchannel(0))
    );
    engine.apply(write(0, 0xb1c0)).unwrap();
    engine.apply(write(8, u32::MAX)).unwrap();
    assert_eq!(engine.bound_class(0), Some(EngineClass::Compute));
}

#[test]
fn bounded_initialization_metadata_retains_values_without_vertex_defaults_or_shader_calls() {
    let mut engine = Maxwell3d::default();
    engine.apply(write(0, CLASS)).unwrap();
    for value in 0..=8 {
        engine.apply(write(0xd64, value)).unwrap();
        assert_eq!(
            engine.api_call_limit(),
            Some(if value == 0 { 0 } else { 1u16 << (value - 1) })
        );
    }
    for value in [9, 15, 0x100, u32::MAX] {
        assert!(engine.apply(write(0xd64, value)).is_err());
    }
    assert_eq!(engine.api_call_limit(), Some(128));
    for value in [0, 1] {
        engine.apply(write(0x1684, value)).unwrap();
        assert_eq!(engine.provoking_vertex_last(), Some(value != 0));
        engine.apply(write(0x1140, value << 4)).unwrap();
        assert_eq!(engine.blend_other_formats_enabled(), Some(value != 0));
    }
    assert!(engine.apply(write(0x1684, 2)).is_err());
    assert_eq!(engine.provoking_vertex_last(), Some(true));
    assert!(engine.apply(write(0x1140, 1)).is_err());
    assert_eq!(engine.blend_other_formats_enabled(), Some(true));
    engine.apply(write(0x1610, 0x0e)).unwrap();
    for value in (0..64).filter(|v| *v != 0x0e) {
        assert!(engine.apply(write(0x1610, value)).is_err());
        assert_eq!(engine.attribute_defaults(), Some(0x0e));
    }
    assert!(engine.apply(write(0x1610, 64)).is_err());
    assert_eq!(engine.attribute_defaults(), Some(0x0e));
    assert!(engine.vertex_array().is_err()); // No implicit/default guest attributes are manufactured.
    engine.apply(write(0x165c, 0)).unwrap();
    assert!(engine.apply(write(0x165c, 1)).is_err());
}

#[test]
fn inactive_vertex_instance_slots_cannot_alias_stream_zero() {
    let mut engine = Maxwell3d::default();
    engine.apply(write(0, CLASS)).unwrap();
    assert_eq!(engine.vertex_array_instance(0), None);
    for slot in 1..16 {
        for value in [0, 1] {
            engine.apply(write(0x1880 + slot * 4, value)).unwrap();
            assert_eq!(
                engine.vertex_array_instance(usize::from(slot)),
                Some(value != 0)
            );
            assert_eq!(engine.vertex_array_instance(0), None);
        }
        assert!(engine.apply(write(0x1880 + slot * 4, 2)).is_err());
        assert_eq!(engine.vertex_array_instance(usize::from(slot)), Some(true));
    }
    engine.apply(write(0x1880, 0)).unwrap();
    assert_eq!(engine.vertex_array_instance(0), Some(false));
    assert!(engine.apply(write(0x1880, 1)).is_err());
    assert_eq!(engine.vertex_array_instance(0), Some(false));
    assert_eq!(engine.vertex_array_instance(16), None);
}
