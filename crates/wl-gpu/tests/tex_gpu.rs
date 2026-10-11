#![cfg(feature = "wgpu-backend")]
mod common;
use common::{maxwell::*, *};
use stats_alloc::{INSTRUMENTED_SYSTEM, Region, StatsAlloc};
use std::{alloc::System, time::Instant};
#[global_allocator]
static ALLOCATOR: &StatsAlloc<System> = &INSTRUMENTED_SYSTEM;
use wl_gpu::{
    backend::Renderer, command::Command, fifo::MethodWrite, maxwell::*, memory::*,
    shader_program::FragmentProgram, texture::*,
};
fn write(engine: &mut Maxwell3d, method: u16, value: u32) {
    engine
        .apply(MethodWrite {
            subchannel: 0,
            method,
            value,
        })
        .unwrap();
}
fn words(backing: &mut [u8], at: usize, words: &[u32]) {
    for (n, w) in words.iter().enumerate() {
        backing[at + n * 4..at + n * 4 + 4].copy_from_slice(&w.to_le_bytes());
    }
}
fn bilinear(source: &[u8], w: usize, h: usize, ow: usize, oh: usize, srgb: bool) -> Vec<u8> {
    let mut out = vec![];
    for y in 0..oh {
        for x in 0..ow {
            let sx = (x as f64 + 0.5) * w as f64 / ow as f64 - 0.5;
            let sy = (y as f64 + 0.5) * h as f64 / oh as f64 - 0.5;
            let ix = sx.floor() as isize;
            let iy = sy.floor() as isize;
            let fx = sx - ix as f64;
            let fy = sy - iy as f64;
            for c in 0..4 {
                let mut v = 0.;
                for (dy, wy) in [(0, 1. - fy), (1, fy)] {
                    for (dx, wx) in [(0, 1. - fx), (1, fx)] {
                        let tx = (ix + dx).clamp(0, w as isize - 1) as usize;
                        let ty = (iy + dy).clamp(0, h as isize - 1) as usize;
                        let encoded = source[(ty * w + tx) * 4 + c] as f64 / 255.;
                        let value = if srgb && c < 3 {
                            if encoded <= 0.04045 {
                                encoded / 12.92
                            } else {
                                ((encoded + 0.055) / 1.055).powf(2.4)
                            }
                        } else {
                            encoded
                        };
                        v += value * 255. * wx * wy;
                    }
                }
                out.push(v.round() as u8);
            }
        }
    }
    out
}
#[test]
#[ignore = "requires actual GPU; explicitly run --ignored --nocapture"]
fn binary_tex_uses_ldc_handle_and_literal_pools() {
    let mut r = Renderer::new().expect("actual GPU requested");
    println!("adapter={:?}", r.capabilities);
    let program = FragmentProgram::decode(include_bytes!("fixtures/maxwell/tex.bin")).unwrap();
    r.set_resource_fragment(&program).unwrap();
    let modules = r.counters.shader_module_allocations;
    let pipelines = r.counters.pipeline_allocations;
    let mut srgb_tic = TIC_RGBA;
    srgb_tic[4] |= 1 << 22;
    for tic in [TIC_RGBA, TIC_ASTC, TIC_BC1, srgb_tic] {
        let mut engine = configured();
        write(&mut engine, 0x157c, 1);
        write(&mut engine, 0x1564, 1);
        for (m, v) in [(0x2380, 32), (0x2384, 0), (0x2388, 0x40000), (0x2490, 1)] {
            write(&mut engine, m, v);
        }
        let desc = TextureImage::decode(tic)
            .unwrap()
            .with_sampler(TextureSampler::decode(TSC_NEAREST).unwrap());
        let (surface, expected) = if matches!(
            desc.format,
            wl_gpu::format::Format::Rgba8Unorm | wl_gpu::format::Format::Rgba8Srgb
        ) {
            rgba_surface(desc)
        } else {
            common::compressed::surface(desc)
        };
        let srgb = desc.format == wl_gpu::format::Format::Rgba8Srgb;
        let direct = bilinear(&expected[0], 37, 19, 37, 19, srgb);
        let mut data = vec![0; 0x1000 + surface.len()];
        data[0x1000..].copy_from_slice(&surface);
        words(&mut data, 32, &tic);
        words(&mut data, 64, &TSC_NEAREST);
        words(&mut data, 96, &[0x92, 0x62, 0, 0, 0, 0, 0, 0]);
        words(&mut data, 128, &[1]); // TIC1, TSC0: deliberately different indices
        let mut map = AddressMap::default();
        for (gpu, guest, size) in [
            (0x10000, 0x1000, surface.len() as u64),
            (0x20000, 0x1000, surface.len() as u64),
            (0x30000, 0, 64),
            (0x31000, 64, 64),
            (0x40000, 128, 32),
        ] {
            map.map(GpuAddress(gpu), GuestAddress(guest), size).unwrap();
        }
        let mut changes = TextureChanges::new(17 + tic[0] as u64);
        r.sync_fragment_resources(
            &engine,
            4,
            &map,
            &SliceMemory {
                base: GuestAddress(0),
                bytes: &data,
            },
            &changes,
        )
        .unwrap();
        let frame = r.execute(37, 19, &[Command::DrawFragment]).unwrap();
        assert_pixels(&frame.rgba, &direct, 1);
        let before = r.counters;
        let region = Region::new(ALLOCATOR);
        let start = Instant::now();
        for _ in 0..32 {
            r.sync_fragment_resources(
                &engine,
                4,
                &map,
                &SliceMemory {
                    base: GuestAddress(0),
                    bytes: &data,
                },
                &changes,
            )
            .unwrap();
        }
        let elapsed = start.elapsed();
        let allocations = region.change();
        assert_eq!(allocations.allocations, 0);
        assert_eq!(allocations.reallocations, 0);
        println!("32 unchanged TEX syncs {elapsed:?}, allocations={allocations:?}");
        assert_eq!(before.descriptor_reads, r.counters.descriptor_reads);
        assert_eq!(before.constant_guest_bytes, r.counters.constant_guest_bytes);
        assert_eq!(before.upload_bytes, r.counters.upload_bytes);
        assert_eq!(before.texture_allocations, r.counters.texture_allocations);
        // Same TIC, different TSC: sampler selection comes from handle's high twelve bits.
        words(&mut data, 128, &[0x100001]);
        changes.mark_write(GuestAddress(128), 4).unwrap();
        r.sync_fragment_resources(
            &engine,
            4,
            &map,
            &SliceMemory {
                base: GuestAddress(0),
                bytes: &data,
            },
            &changes,
        )
        .unwrap();
        let frame = r.execute(74, 38, &[Command::DrawFragment]).unwrap();
        assert_pixels(
            &frame.rgba,
            &bilinear(&expected[0], 37, 19, 74, 38, srgb),
            1,
        );
        // Descriptor points through a GPU alias; invalidation is in canonical guest space.
        let mut alias = tic;
        alias[1] = 0x20000;
        words(&mut data, 32, &alias);
        changes.mark_write(GuestAddress(32), 32).unwrap();
        write(&mut engine, 0x1428, 0);
        r.sync_fragment_resources(
            &engine,
            4,
            &map,
            &SliceMemory {
                base: GuestAddress(0),
                bytes: &data,
            },
            &changes,
        )
        .unwrap();
        assert_pixels(
            &r.execute(37, 19, &[Command::DrawFragment]).unwrap().rgba,
            &direct,
            1,
        );
        if desc.format == wl_gpu::format::Format::Rgba8Unorm {
            data[0x1000..0x1004].copy_from_slice(&[255, 31, 63, 255]);
            changes.mark_write(GuestAddress(0x1000), 4).unwrap();
            words(&mut data, 128, &[1]);
            changes.mark_write(GuestAddress(128), 4).unwrap();
            r.sync_fragment_resources(
                &engine,
                4,
                &map,
                &SliceMemory {
                    base: GuestAddress(0),
                    bytes: &data,
                },
                &changes,
            )
            .unwrap();
            let frame = r.execute(37, 19, &[Command::DrawFragment]).unwrap();
            assert_eq!(&frame.rgba[..4], &[255, 31, 63, 255]);
            assert_pixels(&frame.rgba[4..], &expected[0][4..], 1);
        }
        for handle in [2, 0x200001, 0] {
            words(&mut data, 128, &[handle]);
            changes.mark_write(GuestAddress(128), 4).unwrap();
            assert!(
                r.sync_fragment_resources(
                    &engine,
                    4,
                    &map,
                    &SliceMemory {
                        base: GuestAddress(0),
                        bytes: &data
                    },
                    &changes
                )
                .is_err()
            );
            assert!(r.execute(37, 19, &[Command::DrawFragment]).is_err());
        }
        assert_eq!(modules, r.counters.shader_module_allocations);
        assert_eq!(pipelines, r.counters.pipeline_allocations);
    }
    println!("counters={:?}", r.counters);
}
