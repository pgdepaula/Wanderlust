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
    // The now-supported selector is still NOT a texture/constant binding.
    let epochs = (engine.texture_header_epoch, engine.sampler_epoch);
    assert_eq!(engine.apply(write(0x2608, 0)), Ok(MethodEffect::State));
    assert_eq!(engine.bindless_texture_constant_slot(), Some(0));
    assert_eq!((engine.texture_header_epoch, engine.sampler_epoch), epochs);
    assert!(engine.constant_buffer(4, 0).is_err());
    assert!(engine.apply(write(0x1334, 0)).is_err()); // unsupported header-cache invalidation method
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

#[test]
fn two_d_configuration_is_separate_and_does_not_claim_transfer_execution() {
    let mut engine = Maxwell3d::default();
    engine.apply(write(0, CLASS)).unwrap();
    let auxiliary = |method, value| MethodWrite {
        subchannel: 3,
        method,
        value,
    };
    engine.apply(auxiliary(0, 0x902d)).unwrap();
    assert_eq!(engine.two_d_configuration(), TwoDConfiguration::default());
    for (method, value) in [(0x2ac, 3), (0x290, 0), (0x884, 0x3f)] {
        assert_eq!(
            engine.apply(auxiliary(method, value)),
            Ok(MethodEffect::State)
        );
    }
    let snapshot = engine.two_d_configuration();
    assert_eq!(
        snapshot,
        TwoDConfiguration {
            source_copy: true,
            clipping_disabled: true,
            corral_size: Some(63)
        }
    );
    for (method, value) in [
        (0x2ac, 0),
        (0x2ac, 7),
        (0x2ac, 0x10003),
        (0x290, 1),
        (0x290, 2),
        (0x884, 1024),
        (0x884, u32::MAX),
    ] {
        assert!(engine.apply(auxiliary(method, value)).is_err());
        assert_eq!(engine.two_d_configuration(), snapshot);
    }
    for method in [0x2ac, 0x290, 0x884] {
        assert!(engine.apply(write(method, 0)).is_err());
    }
    // PIXELS_FROM_MEMORY_SRC_Y0_INT remains an unsupported launch.
    assert!(matches!(
        engine.apply(auxiliary(0x8dc, 0)),
        Err(MethodError::UnsupportedEngineMethod {
            class: 0x902d,
            method: 0x8dc
        })
    ));
    assert_eq!(engine.two_d_configuration(), snapshot);
    assert_eq!(
        Maxwell3d::default().two_d_configuration(),
        TwoDConfiguration::default()
    );
    for value in [0, 1023] {
        engine.apply(auxiliary(0x884, value)).unwrap();
        assert_eq!(engine.two_d_configuration().corral_size, Some(value as u16));
    }
    // Alignment failure must not alias CORRAL_SIZE.
    assert!(engine.apply(auxiliary(0x885, 7)).is_err());
    assert_eq!(engine.two_d_configuration().corral_size, Some(1023));
}

#[test]
fn vertex_id_array_start_control_validates_its_single_documented_bit() {
    let mut engine = Maxwell3d::default();
    engine.apply(write(0, CLASS)).unwrap();
    assert_eq!(engine.vertex_id_uses_array_start(), None);
    for (value, expected) in [(0x1000, true), (0, false)] {
        engine.apply(write(0x164c, value)).unwrap();
        assert_eq!(engine.vertex_id_uses_array_start(), Some(expected));
        for malformed in [1, 0x1001, u32::MAX] {
            assert!(engine.apply(write(0x164c, malformed)).is_err());
            assert_eq!(engine.vertex_id_uses_array_start(), Some(expected));
        }
    }
}

#[test]
fn auxiliary_raster_controls_retain_state_without_fabricating_queries_or_outputs() {
    let mut e = Maxwell3d::default();
    e.apply(write(0, CLASS)).unwrap();
    assert_eq!(
        e.auxiliary_raster_controls(),
        AuxiliaryRasterControls::default()
    );
    for (method, value) in [(0x151c, 1), (0x20c, 1), (0x30c, 0), (0x300, 3)] {
        e.apply(write(method, value)).unwrap();
    }
    let expected = AuxiliaryRasterControls {
        zcull_statistics: Some(true),
        aliased_line_width: Some(true),
        conditional_constant_load: Some(false),
        pixel_sample_mask_usage: Some(3),
        ..AuxiliaryRasterControls::default()
    };
    assert_eq!(e.auxiliary_raster_controls(), expected);
    for (method, value) in [
        (0x151c, 2),
        (0x20c, 2),
        (0x30c, 1),
        (0x300, 4),
        (0x300, u32::MAX),
        (0x1b0c, 0),
    ] {
        assert!(e.apply(write(method, value)).is_err());
        assert_eq!(e.auxiliary_raster_controls(), expected);
    }
    assert_eq!(
        Maxwell3d::default().auxiliary_raster_controls(),
        AuxiliaryRasterControls::default()
    );
    for value in 0..4 {
        e.apply(write(0x300, value)).unwrap();
        assert_eq!(
            e.auxiliary_raster_controls().pixel_sample_mask_usage,
            Some(value as u8)
        );
    }
    for method in [0x151c, 0x20c] {
        e.apply(write(method, 0)).unwrap();
    }
    assert_eq!(e.auxiliary_raster_controls().zcull_statistics, Some(false));
    assert_eq!(
        e.auxiliary_raster_controls().aliased_line_width,
        Some(false)
    );
    // No admitted fragment interface may export a sample mask. This invariant
    // must be revisited if the strict shader subset is expanded in the future.
    let bytes = include_bytes!("fixtures/maxwell/red.bin");
    assert!(wl_gpu::shader_program::FragmentProgram::decode(bytes).is_ok());
    assert!(wl_gpu::shader::ConstantFragment::decode(bytes).is_ok());
    let mut mask = bytes.to_vec();
    mask[76] |= 1;
    assert!(wl_gpu::shader_program::FragmentProgram::decode(&mask).is_err());
    assert!(wl_gpu::shader::ConstantFragment::decode(&mask).is_err());
}

#[test]
fn normalized_blend_controls_reject_ieee_mode_and_float_targets() {
    let mut e = Maxwell3d::default();
    e.apply(write(0, CLASS)).unwrap();
    for value in [0, 1] {
        e.apply(write(0xfdc, value)).unwrap();
        assert_eq!(
            e.auxiliary_raster_controls().allow_float_pixel_kills,
            Some(value != 0)
        );
    }
    e.apply(write(0x19c0, 1)).unwrap();
    let snapshot = e.auxiliary_raster_controls();
    for (method, value) in [
        (0xfdc, 2),
        (0xfdc, u32::MAX),
        (0x19c0, 0),
        (0x19c0, 2),
        (0x19c0, u32::MAX),
    ] {
        assert!(e.apply(write(method, value)).is_err());
        assert_eq!(e.auxiliary_raster_controls(), snapshot);
    }
    // NVIDIA RF32_GF32_BF32_AF32 and RF16_GF16_BF16_AF16 cannot
    // inherit normalized-target behavior merely because the control was set.
    for format in [0xc0, 0xca] {
        assert!(e.apply(write(0x810, format)).is_err());
    }
    assert_eq!(e.auxiliary_raster_controls(), snapshot);
}

#[test]
fn bounded_initialization_controls_preserve_shader_and_raster_restrictions() {
    let mut e = Maxwell3d::default();
    e.apply(write(0, CLASS)).unwrap();
    for (method, value) in [
        (0xf8c, 0),
        (0x1594, 1),
        (0x310, 0x503),
        (0x77c, 0x01000000),
        (0x1130, 0x13),
        (0x374, 0),
        (0x15cc, 0x10000),
    ] {
        e.apply(write(method, value)).unwrap();
    }
    let c = e.auxiliary_raster_controls();
    assert_eq!(c.draw_generated_polygon_edges, Some(true));
    assert_eq!(c.two_sided_stencil, Some(true));
    assert_eq!(c.spa_version, Some(0x503));
    assert_eq!(c.local_memory_window, Some(0x01000000));
    assert_eq!(c.target_independent_raster_control, Some(0x13));
    assert_eq!(c.balanced_primitive_workload, Some(0));
    assert_eq!(c.geometry_selects_target_layer, Some(true));
    for (method, value) in [
        (0xf8c, 2),
        (0x1594, 2),
        (0x310, 0x500),
        (0x310, 0x10503),
        (0x1130, 4),
        (0x1130, 0x20),
        (0x374, 2),
        (0x15cc, 1),
        (0x15cc, 0x10001),
        (0x15cc, 0x20000),
    ] {
        assert!(e.apply(write(method, value)).is_err());
        assert_eq!(e.auxiliary_raster_controls(), c);
    }
    // The controls must never smuggle stencil, line polygons or geometry into
    // the existing triangle/layer-zero pipeline.
    for (method, value) in [
        (0x1380, 1),
        (0xdac, 0x1b01),
        (0xdb0, 0x1b01),
        (0x2100, 0x41),
    ] {
        assert!(e.apply(write(method, value)).is_err());
    }
    for base in [0, u32::MAX, 0xff000000] {
        e.apply(write(0x77c, base)).unwrap();
        assert_eq!(
            e.auxiliary_raster_controls().local_memory_window,
            Some(base)
        );
    }
    let bytes = include_bytes!("fixtures/maxwell/red.bin");
    for offset in [4, 8, 12] {
        let mut local = bytes.to_vec();
        local[offset] = 1;
        assert!(wl_gpu::shader_program::FragmentProgram::decode(&local).is_err());
        assert!(wl_gpu::shader::ConstantFragment::decode(&local).is_err());
    }
}

#[test]
fn bindless_constant_selector_has_five_bits_and_does_not_bind_resources() {
    let mut e = Maxwell3d::default();
    e.apply(write(0, CLASS)).unwrap();
    assert_eq!(e.bindless_texture_constant_slot(), None);
    for slot in 0..32 {
        e.apply(write(0x2608, slot)).unwrap();
        assert_eq!(e.bindless_texture_constant_slot(), Some(slot as u8));
    }
    for invalid in [32, 0x10002, u32::MAX] {
        assert!(e.apply(write(0x2608, invalid)).is_err());
        assert_eq!(e.bindless_texture_constant_slot(), Some(31));
    }
    assert_eq!(Maxwell3d::default().bindless_texture_constant_slot(), None);
    assert!(e.vertex_array().is_err());
    // Configuration does not manufacture a constant-buffer binding.
    assert!(e.constant_buffer(4, 31).is_err());
}

#[test]
fn compression_threshold_and_warp_watermarks_retain_exact_bounded_fields() {
    let mut e = Maxwell3d::default();
    e.apply(write(0, CLASS)).unwrap();
    for (encoding, samples) in [0, 1, 2, 4, 8, 16, 32, 64, 128, 256, 512, 1024, 2048]
        .into_iter()
        .enumerate()
    {
        e.apply(write(0x1220, encoding as u32)).unwrap();
        assert_eq!(
            e.auxiliary_raster_controls().compression_sample_threshold,
            Some(samples)
        );
    }
    for invalid in [13, 15, 16, u32::MAX] {
        assert!(e.apply(write(0x1220, invalid)).is_err());
        assert_eq!(
            e.auxiliary_raster_controls().compression_sample_threshold,
            Some(2048)
        );
    }
    for (value, expected) in [
        (0, (0, 0)),
        (0x00800008, (8, 128)),
        (u32::MAX, (65535, 65535)),
    ] {
        e.apply(write(0x1450, value)).unwrap();
        assert_eq!(
            e.auxiliary_raster_controls().pixel_warp_watermarks,
            Some(expected)
        );
    }
    assert!(e.apply(write(0x1450, 0x00010002)).is_err());
    assert_eq!(
        e.auxiliary_raster_controls().pixel_warp_watermarks,
        Some((65535, 65535))
    );
}

#[test]
fn b197_wait_for_idle_is_an_execution_effect_not_an_engine_reset() {
    let mut e = Maxwell3d::default();
    assert!(e.apply(write(0x110, 0)).is_err());
    e.apply(write(0, CLASS)).unwrap();
    e.apply(write(0x164c, 0x1000)).unwrap();
    assert_eq!(e.apply(write(0x110, 0)), Ok(MethodEffect::WaitIdle));
    assert_eq!(e.vertex_id_uses_array_start(), Some(true));
    assert!(e.apply(write(0x110, 1)).is_err());
    assert_eq!(e.vertex_id_uses_array_start(), Some(true));
    assert!(e.apply(write(0x111, 0)).is_err());
    assert_eq!(e.bound_class(0), Some(EngineClass::ThreeD));
    e.apply(MethodWrite {
        subchannel: 3,
        method: 0,
        value: 0x902d,
    })
    .unwrap();
    assert!(
        e.apply(MethodWrite {
            subchannel: 3,
            method: 0x110,
            value: 0
        })
        .is_err()
    );
}

#[test]
fn zcull_hints_preserve_fragment_tests_and_validate_literal_packet_fields() {
    let mut channel = Channel::default();
    // Independent literal FIFO packet: bind MAXWELL_B, then consecutive
    // byte offsets 1968 and 196c (word index65a), exactly two arguments.
    for word in [0x20010000, 0xb197, 0x2002065a, 0x11, 0] {
        assert_eq!(channel.push_word(word).unwrap(), MethodEffect::State);
    }
    channel.finish().unwrap();
    let controls = channel.engine.auxiliary_raster_controls();
    assert_eq!(controls.zcull_tests, Some((true, true)));
    assert_eq!(controls.zcull_unbounded, Some((false, false)));
    assert_eq!(controls.zcull_statistics, None);
    for value in [0, 1, 0x10, 0x11] {
        for method in [0x1968, 0x196c] {
            channel.engine.apply(write(method, value)).unwrap();
        }
        let controls = channel.engine.auxiliary_raster_controls();
        let expected = Some((value & 1 != 0, value & 0x10 != 0));
        assert_eq!(controls.zcull_tests, expected);
        assert_eq!(controls.zcull_unbounded, expected);
    }
    let before = channel.engine.auxiliary_raster_controls();
    for method in [0x1968, 0x196c] {
        for value in [2, 4, 0x20, 0x100, u32::MAX] {
            assert!(channel.engine.apply(write(method, value)).is_err());
            assert_eq!(channel.engine.auxiliary_raster_controls(), before);
        }
    }
    for (method, value) in [
        (0x1969, 0),
        (0x196d, 0),
        (0x1978, 0),
        (0x1b0c, 0),
        (0x1380, 1),
    ] {
        assert!(channel.engine.apply(write(method, value)).is_err());
        assert_eq!(channel.engine.auxiliary_raster_controls(), before);
    }
    assert_eq!(
        Maxwell3d::default().auxiliary_raster_controls().zcull_tests,
        None
    );
    for region in [0, 1, 7, 0x3f] {
        channel.engine.apply(write(0x1590, region)).unwrap();
        assert_eq!(
            channel.engine.auxiliary_raster_controls().zcull_region,
            Some(region as u8)
        );
    }
    for region in [0x40, 0x100, u32::MAX] {
        assert!(channel.engine.apply(write(0x1590, region)).is_err());
        assert_eq!(
            channel.engine.auxiliary_raster_controls().zcull_region,
            Some(0x3f)
        );
    }
    for comparison in 0..8 {
        let value = 0xa7850000 | comparison;
        channel.engine.apply(write(0xdd8, value)).unwrap();
        assert_eq!(
            channel
                .engine
                .auxiliary_raster_controls()
                .zcull_stencil_criterion,
            Some(value)
        );
    }
    for value in [8, 0x80, 0x100, 0x200, 0x400, u32::MAX] {
        assert!(channel.engine.apply(write(0xdd8, value)).is_err());
        assert_eq!(
            channel
                .engine
                .auxiliary_raster_controls()
                .zcull_stencil_criterion,
            Some(0xa7850007)
        );
    }
}

#[test]
fn instrumentation_initialization_is_ordered_bounded_and_channel_local() {
    let mut engine = Maxwell3d::default();
    engine.apply(write(0, CLASS)).unwrap();
    assert_eq!(engine.instrumentation_initialization(), (None, None));
    assert!(engine.apply(write(0x154, 0x49000001)).is_err());
    assert_eq!(engine.instrumentation_initialization(), (None, None));
    // Literal FIFO sequence checks byte-vs-word method addressing.
    let mut channel = Channel::default();
    for word in [0x20010000, CLASS, 0x20020054, 0x49000000, 0x49000001] {
        assert_eq!(channel.push_word(word).unwrap(), MethodEffect::State);
    }
    channel.finish().unwrap();
    let expected = (Some(0x49000000), Some(0x49000001));
    assert_eq!(channel.engine.instrumentation_initialization(), expected);
    for (method, value) in [
        (0x150, 0),
        (0x150, 0x49000001),
        (0x154, 0),
        (0x154, 0x49000001),
        (0x151, 0x49000000),
    ] {
        assert!(channel.engine.apply(write(method, value)).is_err());
        assert_eq!(channel.engine.instrumentation_initialization(), expected);
    }
    channel.engine.apply(write(0x150, 0x49000000)).unwrap();
    assert_eq!(
        channel.engine.instrumentation_initialization(),
        (Some(0x49000000), None)
    );
    channel.engine.apply(write(0x154, 0x49000001)).unwrap();
    assert_eq!(
        Maxwell3d::default().instrumentation_initialization(),
        (None, None)
    );
}

#[test]
fn vertex_substitute_address_is_40_bit_partial_persistent_and_channel_local() {
    let mut channel = Channel::default();
    for word in [0x20010000, CLASS, 0x200203e1, 0xff, u32::MAX] {
        assert_eq!(channel.push_word(word).unwrap(), MethodEffect::State);
    }
    channel.finish().unwrap();
    let maximum = Some(GpuAddress(0xffffffffff));
    assert_eq!(channel.engine.vertex_stream_substitute_address(), maximum);
    for (method, value) in [(0xf84, 0x100), (0xf84, u32::MAX), (0xf85, 0), (0xf89, 0)] {
        assert!(channel.engine.apply(write(method, value)).is_err());
        assert_eq!(channel.engine.vertex_stream_substitute_address(), maximum);
    }
    channel.engine.apply(write(0xf88, 0x12345679)).unwrap();
    assert_eq!(
        channel.engine.vertex_stream_substitute_address(),
        Some(GpuAddress(0xff12345679))
    );
    channel.engine.apply(write(0xf84, 0)).unwrap();
    assert_eq!(
        channel.engine.vertex_stream_substitute_address(),
        Some(GpuAddress(0x12345679))
    );
    let mut independent = Maxwell3d::default();
    independent.apply(write(0, CLASS)).unwrap();
    assert_eq!(independent.vertex_stream_substitute_address(), None);
    independent.apply(write(0xf88, 0)).unwrap();
    assert_eq!(independent.vertex_stream_substitute_address(), None);
    independent.apply(write(0xf84, 0)).unwrap();
    assert_eq!(
        independent.vertex_stream_substitute_address(),
        Some(GpuAddress(0))
    );
    let bounded = VertexArray {
        address: GpuAddress(0x1000),
        limit: GpuAddress(0x1047),
        stride: 24,
        offsets: [0, 16],
        position_format: PositionFormat::Float32x4,
    };
    assert_eq!(bounded.span(0, 3).unwrap(), 72);
    assert!(bounded.span(1, 3).is_err());
    assert!(bounded.span(u32::MAX, 3).is_err());
    // A configured pointer does not manufacture a missing vertex array.
    assert!(independent.vertex_array().is_err());
    assert_eq!(
        channel.engine.vertex_stream_substitute_address(),
        Some(GpuAddress(0x12345679))
    );
}

fn inline_engine(length: u32, address: u64) -> Maxwell3d {
    let mut engine = Maxwell3d::default();
    for (m, v) in [
        (0, CLASS),
        (0x180, length),
        (0x184, 1),
        (0x188, (address >> 32) as u32),
        (0x18c, address as u32),
    ] {
        engine.apply(write(m, v)).unwrap();
    }
    engine
}

#[test]
fn inline_upload_validates_descriptor_flags_and_exact_word_extent() {
    let mut engine = inline_engine(8, 0x1234567800);
    for (m, v) in [
        (0x180, 3),
        (0x180, 65540),
        (0x184, 2),
        (0x188, 256),
        (0x181, 8),
    ] {
        assert!(engine.apply(write(m, v)).is_err());
    }
    for bit in 0..32 {
        assert!(engine.apply(write(0x1b0, 0x1011 ^ (1 << bit))).is_err());
    }
    assert!(matches!(
        engine.apply(write(0x1b4, 1)),
        Err(MethodError::IncompleteInlineUpload)
    ));
    assert_eq!(
        engine.apply(write(0x1b0, 0x1011)).unwrap(),
        MethodEffect::InlineUploadStart {
            address: GpuAddress(0x1234567800),
            length: 8
        }
    );
    for m in [0x180, 0x184, 0x188, 0x18c, 0x1b0] {
        assert_eq!(
            engine.apply(write(m, 0)),
            Err(MethodError::InlineUploadActive)
        );
    }
    for (address, value) in [(0x1234567800, 0x12345678), (0x1234567804, 0xabcdef01)] {
        assert_eq!(
            engine.apply(write(0x1b4, value)).unwrap(),
            MethodEffect::InlineWrite {
                address: GpuAddress(address),
                value
            }
        );
    }
    assert_eq!(
        engine.apply(write(0x1b4, 0)),
        Err(MethodError::IncompleteInlineUpload)
    );
    // New launch snapshots the persistent descriptor; it does not inherit cursor.
    assert_eq!(
        engine.apply(write(0x1b0, 0x1011)).unwrap(),
        MethodEffect::InlineUploadStart {
            address: GpuAddress(0x1234567800),
            length: 8
        }
    );
    assert!(
        inline_engine(4, 0xfffffffffc)
            .apply(write(0x1b0, 0x1011))
            .is_ok()
    );
    assert!(
        inline_engine(8, 0xfffffffffc)
            .apply(write(0x1b0, 0x1011))
            .is_err()
    );
    assert!(inline_engine(4, 3).apply(write(0x1b0, 0x1011)).is_err());
    let mut empty = inline_engine(0, 0xa512345678);
    assert_eq!(
        empty.apply(write(0x1b0, 0x1011)).unwrap(),
        MethodEffect::InlineUploadStart {
            address: GpuAddress(0xa512345678),
            length: 0
        }
    );
    assert_eq!(
        empty.apply(write(0x1b4, 0)),
        Err(MethodError::IncompleteInlineUpload)
    );
    empty.apply(write(0x180, 4)).unwrap();
    assert!(
        inline_engine(65536, 0x1000)
            .apply(write(0x1b0, 0x1011))
            .is_ok()
    );
}

#[test]
fn inline_upload_packet_continuation_is_channel_local_and_finish_detects_truncation() {
    let mut channel = Channel::default();
    for word in [
        0x20010000, CLASS, 0x20040060, 8, 1, 0, 0x123400, 0x2001006c, 0x1011, 0x6002006d,
    ] {
        channel.push_word(word).unwrap();
    }
    assert_eq!(
        channel.push_word(0x04030201).unwrap(),
        MethodEffect::InlineWrite {
            address: GpuAddress(0x123400),
            value: 0x04030201
        }
    );
    assert_eq!(
        channel.push_word(0x08070605).unwrap(),
        MethodEffect::InlineWrite {
            address: GpuAddress(0x123404),
            value: 0x08070605
        }
    );
    channel.finish().unwrap();
    let mut other = Maxwell3d::default();
    other.apply(write(0, CLASS)).unwrap();
    assert_eq!(
        other.apply(write(0x1b0, 0x1011)),
        Err(MethodError::IncompleteInlineUpload)
    );
    assert_eq!(
        other.apply(write(0x1b4, 0)),
        Err(MethodError::IncompleteInlineUpload)
    );
    let mut unfinished = Channel::default();
    for (m, v) in [
        (0, CLASS),
        (0x180, 8),
        (0x184, 1),
        (0x188, 0),
        (0x18c, 0x1000),
    ] {
        unfinished.engine.apply(write(m, v)).unwrap();
    }
    unfinished.engine.apply(write(0x1b0, 0x1011)).unwrap();
    assert!(unfinished.finish().is_err());
    assert!(unfinished.is_faulted());
}

#[test]
fn edge_flag_retains_only_one_bit_and_does_not_enable_wireframe() {
    let mut channel = Channel::default();
    for word in [0x20010000, CLASS, 0x20010579, 1] {
        assert_eq!(channel.push_word(word).unwrap(), MethodEffect::State);
    }
    channel.finish().unwrap();
    assert_eq!(
        channel.engine.auxiliary_raster_controls().edge_flag,
        Some(true)
    );
    for (method, value) in [
        (0x15e4, 2),
        (0x15e4, u32::MAX),
        (0x15e5, 0),
        (0xdac, 0x1b01),
        (0xdb0, 0x1b00),
    ] {
        assert!(channel.engine.apply(write(method, value)).is_err());
        assert_eq!(
            channel.engine.auxiliary_raster_controls().edge_flag,
            Some(true)
        );
    }
    channel.engine.apply(write(0x15e4, 0)).unwrap();
    assert_eq!(
        channel.engine.auxiliary_raster_controls().edge_flag,
        Some(false)
    );
    assert_eq!(
        Maxwell3d::default().auxiliary_raster_controls().edge_flag,
        None
    );
}

#[test]
fn shadow_track_passthrough_replay_and_filtered_history_are_distinct() {
    let mut c = Channel::default();
    for word in common::hardware::setup() {
        c.push_word(word).unwrap();
    }
    assert_eq!(c.engine.shadow_mode(), None);
    assert_eq!(c.engine.shadow_value(0x1a00), Some(ShadowValue::Unwritten));
    for (m, v) in [(0x124, 0), (0x1a00, 1), (0x124, 2), (0x1a00, 0x1111)] {
        c.engine.apply(write(m, v)).unwrap();
    }
    assert_eq!(c.engine.shadow_value(0x1a00), Some(ShadowValue::Known(1)));
    c.engine.apply(write(0x124, 3)).unwrap();
    c.engine.apply(write(0x1a00, u32::MAX)).unwrap(); // Incoming value is not replayed.
    assert_eq!(c.engine.rasterization().unwrap().1.color.write_mask, 1);
    assert_eq!(c.engine.shadow_value(0x1a00), Some(ShadowValue::Known(1)));
    for v in [4, 0x100, u32::MAX] {
        assert!(c.engine.apply(write(0x124, v)).is_err());
        assert_eq!(c.engine.shadow_mode(), Some(ShadowMode::Replay));
    }
    c.engine.apply(write(0x124, 1)).unwrap();
    assert!(c.engine.apply(write(0x1a00, u32::MAX)).is_err());
    assert_eq!(c.engine.shadow_value(0x1a00), Some(ShadowValue::Known(1)));
    c.engine.apply(write(0x1a00, 0x1111)).unwrap();
    assert_eq!(c.engine.rasterization().unwrap().1.color.write_mask, 15);
    assert_eq!(
        c.engine.shadow_value(0x1a00),
        Some(ShadowValue::FilteredUnverified)
    );
    c.engine.apply(write(0x124, 3)).unwrap();
    assert_eq!(
        c.engine.apply(write(0x1a00, 0)),
        Err(MethodError::FilteredShadowValue(0x1a00))
    );
    assert_eq!(c.engine.rasterization().unwrap().1.color.write_mask, 15);
    for (m, v) in [
        (0x124, 0),
        (0x1a00, 0x101),
        (0x124, 2),
        (0x1a00, 0x1111),
        (0x124, 3),
        (0x1a00, 0),
    ] {
        c.engine.apply(write(m, v)).unwrap();
    }
    assert_eq!(c.engine.rasterization().unwrap().1.color.write_mask, 5);
    assert_eq!(
        c.engine.shadow_value(0x1a00),
        Some(ShadowValue::Known(0x101))
    );
    assert_eq!(
        c.engine.apply(write(0x1214, 0x40030000)),
        Err(MethodError::UnsupportedShadowReplayMethod(0x1214))
    );
    assert_eq!(c.engine.shadow_value(0x1a01), None);
    assert_eq!(c.engine.shadow_value(0x800), None); // Attachment addresses cannot be replayed.
}

#[test]
fn shadow_missing_capture_channel_isolation_and_literal_fifo_fault_prefix() {
    let mut c = Channel::default();
    for word in [
        0x20010000,
        CLASS,
        0x20010049,
        0,
        0x20010680,
        1,
        0x20010049,
        2,
        0x20010680,
        0x1111,
        0x20010049,
        3,
        0x20010680,
        u32::MAX,
    ] {
        c.push_word(word).unwrap();
    }
    c.finish().unwrap();
    assert_eq!(c.engine.shadow_value(0x1a00), Some(ShadowValue::Known(1)));
    let independent = Maxwell3d::default();
    assert_eq!(independent.shadow_mode(), None);
    assert_eq!(
        independent.shadow_value(0x1a00),
        Some(ShadowValue::Unwritten)
    );
    let old = c.engine.shadow_value(0x1a00);
    assert!(c.engine.apply(write(0x125, 0)).is_err());
    assert_eq!(c.engine.shadow_value(0x1a00), old);
    // Replay from an uninitialized bank fails, rather than inventing zeros.
    for word in [0x20010049, 3, 0x200104d8] {
        c.push_word(word).unwrap();
    }
    assert!(c.push_word(0).is_err()); // BLEND_ENABLE was never captured here.
    assert!(c.is_faulted());
    assert!(c.push_word(0x20010049).is_err());
    assert_eq!(c.engine.shadow_value(0x1a00), old);
}

#[test]
fn inactive_vertex_a_addresses_are_40_bit_independent_banks_without_fetch_defaults() {
    let mut e = Maxwell3d::default();
    e.apply(write(0, CLASS)).unwrap();
    e.apply(write(0x1c04, 0x12)).unwrap();
    e.apply(write(0x1c08, 0x34567890)).unwrap();
    e.apply(write(0x1f00, 0x12)).unwrap();
    e.apply(write(0x1f04, 0x3456ffff)).unwrap();
    let limit_zero = Some(GpuAddress(0x123456ffff));
    let zero = Some(GpuAddress(0x1234567890));
    for slot in 1..16 {
        let base = 0x1c00 + slot as u16 * 16;
        assert_eq!(e.vertex_stream_address(slot), None);
        e.apply(write(base + 4, 0xff)).unwrap();
        assert_eq!(e.vertex_stream_address(slot), None);
        e.apply(write(base + 8, u32::MAX - slot as u32)).unwrap();
        let expected = Some(GpuAddress(0xff00000000 | u64::from(u32::MAX - slot as u32)));
        assert_eq!(e.vertex_stream_address(slot), expected);
        assert_eq!(e.vertex_stream_address(0), zero);
        let limit_base = 0x1f00 + slot as u16 * 8;
        e.apply(write(limit_base, 0xff)).unwrap();
        assert_eq!(e.vertex_stream_limit(slot), None);
        e.apply(write(limit_base + 4, u32::MAX)).unwrap();
        assert_eq!(e.vertex_stream_limit(slot), Some(GpuAddress(0xffffffffff)));
        assert!(e.apply(write(limit_base, 256)).is_err());
        assert!(e.apply(write(limit_base + 1, 0)).is_err());
        assert_eq!(e.vertex_stream_limit(slot), Some(GpuAddress(0xffffffffff)));
        assert_eq!(e.vertex_stream_limit(0), limit_zero);

        for (m, v) in [(base + 4, 0x100), (base + 5, 0)] {
            assert!(e.apply(write(m, v)).is_err());
            assert_eq!(e.vertex_stream_address(slot), expected);
        }
    }
    assert_eq!(e.vertex_stream_address(16), None);
    assert_eq!(e.vertex_stream_limit(16), None);
    assert_eq!(Maxwell3d::default().vertex_stream_address(1), None);
    assert!(e.vertex_array().is_err()); // Addresses do not supply a missing layout.
}

#[test]
fn compute_initialization_is_isolated_and_does_not_admit_dispatch() {
    let mut e = Maxwell3d::default();
    e.apply(write(0, CLASS)).unwrap();
    e.apply(write(0x310, 0x503)).unwrap();
    e.apply(write(0, EngineClass::Compute as u32)).unwrap();
    for (m, v) in [
        (0x1528, 0),
        (0x2608, 7),
        (0x77c, 0x1000000),
        (0x214, 0x3000000),
        (0x1608, 255),
        (0x160c, u32::MAX),
        (0x310, 0x503),
    ] {
        assert_eq!(e.apply(write(m, v)), Ok(MethodEffect::State));
    }
    let old = e.compute_configuration();
    assert_eq!(old.program_upper, Some(255));
    assert_eq!(old.bindless_constant_slot, Some(7));
    for (m, v) in [
        (0x1528, 2),
        (0x2608, 8),
        (0x1608, 256),
        (0x310, 0x504),
        (0x2b4, 1),
    ] {
        assert!(e.apply(write(m, v)).is_err());
        assert_eq!(e.compute_configuration(), old);
    }
    assert_eq!(e.auxiliary_raster_controls().spa_version, Some(0x503));
    assert!(e.vertex_program().is_err());
}
#[test]
fn cache_methods_are_actions_with_strict_fields_and_no_engine_binding_requirement_for_host() {
    let mut e = Maxwell3d::default();
    for v in [0x70000000, 0x80000000] {
        assert_eq!(
            e.apply(write(0x2c, v)),
            Ok(MethodEffect::HostCacheMaintenance)
        );
    }
    for v in [0, 0x80000001, 0x48000000] {
        assert!(e.apply(write(0x2c, v)).is_err());
    }
    e.apply(write(0, CLASS)).unwrap();
    for v in [0, 1, 0x10, 0x1000, 0x1011] {
        assert_eq!(
            e.apply(write(0xda4, v)),
            Ok(MethodEffect::ShaderCachesInvalidated(v))
        );
    }
    assert!(e.apply(write(0xda4, 2)).is_err());
    assert_eq!(
        e.apply(write(0x1288, 0)),
        Ok(MethodEffect::TextureDataInvalidated)
    );
    assert!(e.apply(write(0x1288, 1)).is_err());
    for m in [0x7a4, 0x7a8] {
        e.apply(write(m, 0x7fff)).unwrap();
        assert!(e.apply(write(m, 0)).is_err());
    }
    assert_eq!(
        e.auxiliary_raster_controls()
            .zero_bandwidth_clear_disable_masks,
        [Some(0x7fff); 2]
    );
}
#[test]
fn inactive_triangle_initialization_fields_validate_before_mutating() {
    let mut e = Maxwell3d::default();
    e.apply(write(0, CLASS)).unwrap();
    for (m, v, bad) in [
        (0x1514, 1, 2),
        (0x1510, 255, 256),
        (0xfbc, 65535, 65536),
        (0x1520, 1, 2),
        (0x1604, 8, 3),
        (0x1910, 1, 2),
        (0x1518, 0x3f800000, 0x7fc00000),
        (0x1398, 255, 256),
        (0x1148, 0, 1),
        (0x2ec, 0, 1),
        (0xfb4, 0, 1),
        (0xfd8, 0, 1),
        (0x1130, 0x13, 4),
        (0x11f8, 0, 1),
        (0x16b4, 3, 4),
        (0xf60, 0, 1),
        (0xf64, 0x800080, 0),
        (0xf68, 0x1109, 0),
        (0xf6c, 0x08080202, 0),
        (0x1108, 0x1f, 0),
        (0xf70, 0x80001, 0),
        (0x1134, 0, 1),
    ] {
        e.apply(write(m, v)).unwrap();
        let old = e.auxiliary_raster_controls();
        assert!(e.apply(write(m, bad)).is_err(), "{m:x}");
        assert_eq!(e.auxiliary_raster_controls(), old, "{m:x}");
    }
    assert!(e.apply(write(0x1380, 1)).is_err()); // No stencil enable.
    assert!(e.apply(write(0x15dc, 0)).is_err()); // No statistics report.
}
#[test]
fn b06f_semaphore_selectors_are_channel_wide_checked_and_snapshot_actions() {
    let mut state = Maxwell3d::default();
    assert!(state.apply(write(0x1c, 0x01000002)).is_err());
    for (m, v) in [(0x10, 0x12), (0x14, 0x12345678), (0x18, 0x89abcdef)] {
        assert_eq!(
            state
                .apply(MethodWrite {
                    subchannel: 7,
                    method: m,
                    value: v
                })
                .unwrap(),
            MethodEffect::State
        );
    }
    assert!(state.apply(write(0x10, 256)).is_err());
    assert!(state.apply(write(0x14, 3)).is_err());
    let expected = GpuAddress(0x1212345678);
    assert_eq!(
        state.apply(write(0x1c, 0x01000002)).unwrap(),
        MethodEffect::SemaphoreRelease {
            address: expected,
            payload: 0x89abcdef,
            long: false,
            wait: true,
        }
    );
    assert_eq!(
        state.apply(write(0x1c, 1)).unwrap(),
        MethodEffect::SemaphoreAcquire {
            address: expected,
            payload: 0x89abcdef
        }
    );
    for value in [4, 8, 16, 0x1001, 0x81000002] {
        assert_eq!(
            state.apply(write(0x1c, value)),
            Err(MethodError::UnsupportedValue {
                method: 0x1c,
                value
            })
        );
    }
    assert!(Maxwell3d::default().apply(write(0x1c, 1)).is_err());
}

#[test]
fn b06f_reports_encode_all_release_sizes_and_ordering_without_fake_ticks() {
    let mut state = Maxwell3d::default();
    for (m, v) in [(0x10, 0), (0x14, 0x1000), (0x18, 77)] {
        state.apply(write(m, v)).unwrap();
    }
    for (control, long, wait) in [
        (2, true, true),
        (0x00100002, true, false),
        (0x01000002, false, true),
        (0x01100002, false, false),
    ] {
        assert_eq!(
            state.apply(write(0x1c, control)).unwrap(),
            MethodEffect::SemaphoreRelease {
                address: GpuAddress(0x1000),
                payload: 77,
                long,
                wait
            }
        );
    }
    for (ns, expected) in [
        (0, 0),
        (625, 384),
        (1250, 768),
        (1_000_000_000, 614_400_000),
        (
            u64::MAX,
            (u64::MAX / 625) * 384 + (u64::MAX % 625) * 384 / 625,
        ),
    ] {
        assert_eq!(timestamp_ticks(ns), expected);
    }
}

#[test]
fn tile_cache_barrier_and_flush_are_ordered_effects_not_noops() {
    let mut engine = Maxwell3d::default();
    for method in [0x0f7c, 0x0f80] {
        assert!(engine.apply(write(method, 0)).is_err());
    }
    engine.apply(write(0, 0xb197)).unwrap();
    for method in [0x0f7c, 0x0f80] {
        assert_eq!(
            engine.apply(write(method, 0)),
            Ok(MethodEffect::HostCacheMaintenance)
        );
        for value in [1, 2, 0x80000000, u32::MAX] {
            assert_eq!(
                engine.apply(write(method, value)),
                Err(MethodError::UnsupportedValue { method, value })
            );
        }
        assert_eq!(
            engine.apply(write(method + 1, 0)),
            Err(MethodError::InvalidMethod(method + 1))
        );
    }
    // Real literal B06F methods, including the exact rejected operand in diagnostics.
    let mut channel = Channel::default();
    channel.push_word(0x20010000).unwrap();
    channel.push_word(0xb197).unwrap();
    channel.push_word(0x200103e0).unwrap();
    assert_eq!(channel.push_word(0), Ok(MethodEffect::HostCacheMaintenance));
    channel.push_word(0x200103e0).unwrap();
    let error = channel.push_word(1).unwrap_err().to_string();
    assert!(error.contains("method=0x0f80 value=0x00000001"));
    assert!(channel.is_faulted());
}

#[test]
fn compute_pool_and_local_memory_fields_are_bounded_partial_and_engine_local() {
    let mut e = Maxwell3d::default();
    e.apply(write(0, EngineClass::Compute as u32)).unwrap();
    // Literal NVB1C0 field maxima, not B197's sampler/header widths inferred
    // from a shared descriptor consumer. No address is dereferenced here.
    for (method, maximum) in [
        (0x155c, 255),
        (0x1564, 0xfffff),
        (0x1574, 255),
        (0x157c, 0x3fffff),
        (0x790, 255),
        (0x2e4, 255),
        (0x2ec, 511),
        (0x2f0, 255),
        (0x2f8, 511),
    ] {
        e.apply(write(method, maximum)).unwrap();
        let before = e.compute_configuration();
        assert_eq!(
            e.apply(write(method, maximum + 1)),
            Err(MethodError::UnsupportedValue {
                method,
                value: maximum + 1
            })
        );
        assert_eq!(e.compute_configuration(), before);
        assert!(e.apply(write(method + 1, 0)).is_err());
        assert_eq!(e.compute_configuration(), before);
    }
    let partial = e.compute_configuration();
    assert_eq!(partial.sampler_pool.lower, None);
    assert_eq!(partial.header_pool.lower, None);
    for method in [0x1560, 0x1578, 0x794, 0x2e8, 0x2f4] {
        e.apply(write(method, u32::MAX)).unwrap();
    }
    let filled = e.compute_configuration();
    assert_eq!(filled.sampler_pool.maximum_index, Some(0xfffff));
    assert_eq!(filled.header_pool.maximum_index, Some(0x3fffff));
    assert_eq!(filled.local_memory_lower, Some(u32::MAX));
    assert_eq!(filled.non_throttled.lower, Some(u32::MAX));
    assert_eq!(filled.throttled.max_sm_count, Some(511));
    // Binding graphics and writing identical pool ports cannot mutate compute.
    e.apply(write(0, CLASS)).unwrap();
    for method in [0x155c, 0x1560, 0x1564, 0x1574, 0x1578, 0x157c] {
        e.apply(write(method, 0)).unwrap();
    }
    assert_eq!(e.compute_configuration(), filled);
    e.apply(write(0, EngineClass::Compute as u32)).unwrap();
    assert_eq!(e.compute_configuration(), filled);
    // QMD launch/PCAS is still unsupported: retaining selectors is not compute.
    assert!(e.apply(write(0x2b4, 1)).is_err());
    assert_eq!(
        Maxwell3d::default().compute_configuration(),
        ComputeConfiguration::default()
    );
}

#[test]
fn graphics_scratch_selectors_use_40_bit_address_and_38_bit_size_fields() {
    let mut e = Maxwell3d::default();
    e.apply(write(0, CLASS)).unwrap();
    for (method, maximum) in [(0x790, 255), (0x798, 63), (0x7a0, 0x03ffffff)] {
        e.apply(write(method, maximum)).unwrap();
        let old = e.auxiliary_raster_controls();
        assert_eq!(
            e.apply(write(method, maximum + 1)),
            Err(MethodError::UnsupportedValue {
                method,
                value: maximum + 1
            })
        );
        assert!(e.apply(write(method + 1, 0)).is_err());
        assert_eq!(e.auxiliary_raster_controls(), old);
    }
    assert_eq!(
        e.auxiliary_raster_controls().local_memory.address_lower,
        None
    );
    assert_eq!(e.auxiliary_raster_controls().local_memory.size_lower, None);
    e.apply(write(0x794, u32::MAX)).unwrap();
    e.apply(write(0x79c, u32::MAX)).unwrap();
    let before = e.auxiliary_raster_controls().local_memory;
    assert_eq!(before.address_upper, Some(255));
    assert_eq!(before.address_lower, Some(u32::MAX));
    assert_eq!(before.size_upper, Some(63));
    assert_eq!(before.size_lower, Some(u32::MAX));
    assert_eq!(before.default_size_per_warp, Some(0x03ffffff));
    e.apply(write(0, EngineClass::Compute as u32)).unwrap();
    e.apply(write(0x790, 0)).unwrap();
    e.apply(write(0x794, 0)).unwrap();
    assert_eq!(e.auxiliary_raster_controls().local_memory, before);
    e.apply(write(0, CLASS)).unwrap();
    assert_eq!(e.auxiliary_raster_controls().local_memory, before);
    assert_eq!(
        Maxwell3d::default()
            .auxiliary_raster_controls()
            .local_memory,
        GraphicsLocalMemory::default()
    );
}

#[test]
fn engine_reports_require_own_selectors_and_reject_unimplemented_counters() {
    let mut e = Maxwell3d::default();
    e.apply(write(0, CLASS)).unwrap();
    assert_eq!(
        e.apply(write(0x1b0c, 0x1000f010)),
        Err(MethodError::IncompleteReport)
    );
    for (m, v) in [(0x1b00, 255), (0x1b04, 0xfffffffc), (0x1b08, 77)] {
        e.apply(write(m, v)).unwrap();
    }
    assert!(e.apply(write(0x1b00, 256)).is_err());
    for control in [
        0x10000000, 0x10000010, 0x1000f000, 0x1000f010, 2, 0xf002, 0xf012,
    ] {
        let effect = e.apply(write(0x1b0c, control)).unwrap();
        assert_eq!(
            effect,
            MethodEffect::SemaphoreRelease {
                address: GpuAddress(0xfffffffffc),
                payload: 77,
                long: control & 3 == 2,
                wait: true,
            }
        );
    }
    for control in [
        0,
        1,
        3,
        0x10000002,
        0x10000004,
        0x10000008,
        0x10010000,
        0x10100000,
        0x00800002,
        0x10002000,
        0x20000000,
        u32::MAX,
    ] {
        assert_eq!(
            e.apply(write(0x1b0c, control)),
            Err(MethodError::UnsupportedValue {
                method: 0x1b0c,
                value: control
            })
        );
    }
    e.apply(write(0x1b04, 3)).unwrap();
    assert!(e.apply(write(0x1b0c, 2)).is_err());
    // Engine report selectors are separate from B06F and the other engine.
    e.apply(write(0, EngineClass::Compute as u32)).unwrap();
    assert_eq!(
        e.apply(write(0x1b0c, 0x10000000)),
        Err(MethodError::IncompleteReport)
    );
    for (m, v) in [(0x1b00, 0), (0x1b04, 0x1234), (0x1b08, 99)] {
        e.apply(write(m, v)).unwrap();
    }
    assert_eq!(
        e.apply(write(0x1b0c, 0x10000000)),
        Ok(MethodEffect::SemaphoreRelease {
            address: GpuAddress(0x1234),
            payload: 99,
            long: false,
            wait: true,
        })
    );
    assert!(e.apply(write(0x1b0c, 2)).is_err());
    e.apply(write(0, CLASS)).unwrap();
    e.apply(write(0x1b04, 0xfffffffc)).unwrap();
    assert_eq!(
        e.apply(write(0x1b0c, 2)),
        Ok(MethodEffect::SemaphoreRelease {
            address: GpuAddress(0xfffffffffc),
            payload: 77,
            long: true,
            wait: true,
        })
    );
}
