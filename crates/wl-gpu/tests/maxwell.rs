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
