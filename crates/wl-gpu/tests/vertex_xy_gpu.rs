#![cfg(feature = "wgpu-backend")]
mod common;
use wl_gpu::{driver::*, driver_backend::*, memory::*};
const POS: [[f32; 2]; 3] = [[-0.8, -0.6], [0.6, -0.4], [-0.2, 0.7]];
fn method(w: &mut Vec<u32>, m: u32, v: u32) {
    w.extend([0x20010000 | (m / 4), v]);
}
fn memory(data: &mut [u8]) -> SliceMemoryMut<'_> {
    SliceMemoryMut {
        base: GuestAddress(0),
        bytes: data,
    }
}
#[test]
#[ignore = "requires actual GPU; explicitly run --ignored --nocapture"]
fn channel_xy_vertex_constants_transform_guest_geometry() {
    let mut d = GpuDriver::new().unwrap();
    println!("adapter={:?}", d.capabilities().adapter);
    let mut data = vec![0u8; 0x20000];
    for (off, b) in [
        (
            0x4000,
            include_bytes!("fixtures/maxwell/red.bin").as_slice(),
        ),
        (
            0x4200,
            include_bytes!("fixtures/maxwell/vertex-xy.bin").as_slice(),
        ),
    ] {
        data[off..off + b.len()].copy_from_slice(b);
    }
    for (n, p) in POS.iter().enumerate() {
        for (c, v) in [p[0], p[1], f32::NAN, f32::NAN, 0., 0.]
            .into_iter()
            .enumerate()
        {
            data[0x3000 + n * 24 + c * 4..0x3004 + n * 24 + c * 4]
                .copy_from_slice(&v.to_le_bytes());
        }
    }
    let backing = d
        .register_backing(GuestAddress(0), data.len() as u64)
        .unwrap();
    let space = d.create_space().unwrap();
    let channel = d.create_channel(space).unwrap();
    for (gpu, off, size) in [
        (0x30000, 0, 0x1000),
        (0x31000, 0, 0x1000),
        (0x40000, 0, 0x1000),
        (0x50000, 0x4000, 0x1000),
        (0x70000, 0x3000, 0x1000),
        (0x90000, 0x8000, 0x4000),
        (0xb0000, 0x10000, 0x2000),
    ] {
        let reservation = d.reserve_fixed(space, GpuAddress(gpu), size).unwrap();
        d.map(space, reservation, backing, off).unwrap();
    }
    let mut setup = common::hardware::setup();
    for (m, v) in [
        (0x2380, 16),
        (0x2384, 0),
        (0x2388, 0x40000),
        (0x2410, 0x31),
        (0x1160, 0x38800000),
    ] {
        method(&mut setup, m, v);
    }
    let mut header = [0u8; 24];
    header[8..12].copy_from_slice(&1u32.to_le_bytes());
    header[12..16].copy_from_slice(&4u32.to_le_bytes());
    let mut cached = None;
    for translate in [0.125f32, -0.25] {
        let cb: Vec<u8> = [0.75f32, 0.5, translate, -0.0625]
            .into_iter()
            .flat_map(f32::to_le_bytes)
            .collect();
        d.write_backing(backing, 0, &cb, &mut memory(&mut data))
            .unwrap();
        let background: Vec<u8> = (0..52 * 29).flat_map(|_| [11, 17, 23, 255]).collect();
        d.write_backing(backing, 0x8000, &background, &mut memory(&mut data))
            .unwrap();
        let mut words = std::mem::take(&mut setup);
        words.extend([0x20010485, 0x40030000]);
        let bytes: Vec<u8> = words.iter().flat_map(|v| v.to_le_bytes()).collect();
        d.write_backing(backing, 0x10000, &bytes, &mut memory(&mut data))
            .unwrap();
        let entry = (0xb0000u64 | ((words.len() as u64) << 42)).to_le_bytes();
        d.submit(
            channel,
            KICKOFF_PB,
            &header,
            Some(&entry),
            &mut memory(&mut data),
        )
        .unwrap();
        d.publish(channel, &mut memory(&mut data)).unwrap().unwrap();
        let transformed = POS.map(|p| {
            [
                f64::from(p[0]) * 0.75 + f64::from(translate),
                -f64::from(p[1]) * 0.5 - 0.0625,
            ]
        });
        let edge = |a: [f64; 2], b: [f64; 2], p: [f64; 2]| {
            (b[0] - a[0]) * (p[1] - a[1]) - (b[1] - a[1]) * (p[0] - a[0])
        };
        let area = edge(transformed[0], transformed[1], transformed[2]);
        let mut inside = 0;
        let mut outside = 0;
        for y in 0..29 {
            for x in 0..47 {
                let p = [
                    (x as f64 + 0.5) * 2. / 47. - 1.,
                    1. - (y as f64 + 0.5) * 2. / 29.,
                ];
                let b = [
                    edge(transformed[1], transformed[2], p) / area,
                    edge(transformed[2], transformed[0], p) / area,
                    edge(transformed[0], transformed[1], p) / area,
                ];
                if b.iter().any(|e| e.abs() < 0.01) {
                    continue;
                }
                let expected = if b.iter().all(|e| *e > 0.) {
                    inside += 1;
                    [255, 0, 0, 255]
                } else {
                    outside += 1;
                    [11, 17, 23, 255]
                };
                let n = 0x8000 + (y * 52 + x) * 4;
                assert_eq!(&data[n..n + 4], expected, "{x},{y}");
            }
        }
        assert!(inside > 50 && outside > 500);
        let c = d.graphics_counters();
        let identity = (c.pipeline_allocations, c.shader_module_allocations);
        if let Some(previous) = cached {
            assert_eq!(identity, previous);
        }
        cached = Some(identity);
    }
}

#[test]
#[ignore = "requires actual GPU; explicitly run --ignored --nocapture"]
fn missing_vertex_varyings_are_rejected_before_pipeline_creation() {
    use wl_gpu::{
        backend::Renderer,
        shader_program::{FragmentProgram, VertexProgram},
    };
    let mut renderer = Renderer::new().unwrap();
    renderer
        .set_resource_fragment(
            &FragmentProgram::decode(include_bytes!("fixtures/maxwell/tex.bin")).unwrap(),
        )
        .unwrap();
    let original = include_bytes!("fixtures/maxwell/vertex-xy.bin");
    let mut binary = original[..80].to_vec();
    let code = [
        (96usize, 0x711u64),
        (136, 0xfe6),
        (152, 0x7e6),
        (160, 0x7e6),
        (168, 0x7e6),
    ];
    for group in code.chunks(3) {
        let control = (0..3).fold(0u64, |v, n| {
            v | (group.get(n).map_or(0x7e6, |x| x.1) << (21 * n))
        });
        binary.extend(control.to_le_bytes());
        for n in 0..3 {
            if let Some((off, _)) = group.get(n) {
                binary.extend_from_slice(&original[*off..*off + 8]);
            } else {
                binary.extend(0x50b0000000070f00u64.to_le_bytes());
            }
        }
    }
    let program = VertexProgram::decode(&binary).unwrap();
    let before = renderer.counters;
    let error = renderer
        .set_vertex_program(&program, 24, [0, 16])
        .unwrap_err();
    assert!(error.to_string().contains("UV absent from vertex OMAP"));
    assert_eq!(
        renderer.counters.pipeline_allocations,
        before.pipeline_allocations
    );
    assert_eq!(
        renderer.counters.shader_module_allocations,
        before.shader_module_allocations
    );
}
