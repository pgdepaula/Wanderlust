#![cfg(feature = "wgpu-backend")]
mod common;
use common::maxwell::{TIC_RGBA, TSC_NEAREST, descriptor_bytes};
use wl_gpu::{driver::*, driver_backend::*, memory::*};
#[global_allocator]
static ALLOCATOR: &stats_alloc::StatsAlloc<std::alloc::System> = &stats_alloc::INSTRUMENTED_SYSTEM;
const RT: usize = 0x8000;
const W: usize = 52;
const H: usize = 29;
const BG: [u8; 4] = [11, 17, 23, 255];
const POS: [[f32; 2]; 3] = [[-0.8, -0.6], [0.6, -0.4], [-0.2, 0.7]];
const V: [f32; 3] = [0.1, 0.2, 0.85];
fn method(w: &mut Vec<u32>, m: u32, v: u32) {
    w.extend([0x20010000 | (m / 4), v]);
}
fn draw(w: &mut Vec<u32>) {
    w.extend([0x20010586, 4, 0x200105f7, 0, 0x200105f8, 3, 0x20010585, 0]);
}
fn mem(data: &mut [u8]) -> SliceMemoryMut<'_> {
    SliceMemoryMut {
        base: GuestAddress(0),
        bytes: data,
    }
}
// Input texture specified mathematically, independently of production decoding,
// swizzling or sampling. The scalar test helper packs this specified image.
fn texel(x: usize, y: usize) -> [u8; 4] {
    [
        (x * 17 + y * 3) as u8,
        (y * 29 + 7) as u8,
        (x * 5 + y * 11 + 41) as u8,
        (64 + (x * 3 + y * 7) % 192) as u8,
    ]
}
struct Fixture {
    d: GpuDriver,
    c: ChannelId,
    s: AddressSpaceId,
    b: BackingId,
    vertex_reservation: ReservationId,
    vertex_mapping: MappingId,
    data: Vec<u8>,
    setup: Vec<u32>,
}
impl Fixture {
    fn new() -> Self {
        let mut data = vec![0u8; 0x20000];
        data[..64].copy_from_slice(&descriptor_bytes(TIC_RGBA, TSC_NEAREST));
        let linear: Vec<u8> = (0..19)
            .flat_map(|y| (0..37).flat_map(move |x| texel(x, y)))
            .collect();
        // Literal TIC word3=0x10 encodes block-height log2=2 at bits5:3.
        let tiled = common::tile_mip(&linear, 37, 19, wl_gpu::format::Format::Rgba8Unorm, 2);
        data[0x1000..0x1000 + tiled.len()].copy_from_slice(&tiled);
        for (off, bin) in [
            (
                0x4000,
                include_bytes!("fixtures/maxwell/tex.bin").as_slice(),
            ),
            (
                0x4300,
                include_bytes!("fixtures/maxwell/vertex-float3-w.bin").as_slice(),
            ),
        ] {
            data[off..off + bin.len()].copy_from_slice(bin);
        }
        for (n, i) in [0u16, 1, 2].into_iter().enumerate() {
            data[0x6000 + n * 2..0x6002 + n * 2].copy_from_slice(&i.to_le_bytes());
        }
        let mut setup = common::hardware::setup();
        for word in &mut setup {
            if *word == 0x31000 {
                *word = 0x31020;
            } else if *word == 0x40000 {
                *word = 0x40040;
            }
        }
        for (m, v) in [
            (0x2044, 0x300),
            (0x17c8, 0),
            (0x17cc, 0x80000),
            (0x17d0, 0),
            (0x17d4, 0x80005),
            (0x17d8, 1),
            (0x1434, 0),
            (0x1438, 0),
            (0x1644, 0),
        ] {
            method(&mut setup, m, v);
        }
        let mut d = GpuDriver::new().unwrap();
        eprintln!("adapter {:?}", d.capabilities().adapter);
        let b = d
            .register_backing(GuestAddress(0), data.len() as u64)
            .unwrap();
        let s = d.create_space().unwrap();
        let c = d.create_channel(s).unwrap();
        let vertex_reservation = d.reserve_fixed(s, GpuAddress(0x70000), 0x1000).unwrap();
        let vertex_mapping = d.map(s, vertex_reservation, b, 0x3000).unwrap();
        for (gpu, offset, size) in [
            (0x10000, 0x1000, 0x2000),
            (0x30000, 0, 0x1000),
            (0x31000, 0, 0x1000),
            (0x40000, 0, 0x1000),
            (0x50000, 0x4000, 0x1000),
            (0x71000, 0x3000, 0x1000),
            (0x80000, 0x6000, 0x1000),
            (0x90000, 0x8000, 0x4000),
            (0xb0000, 0x10000, 0x2000),
        ] {
            let r = d.reserve_fixed(s, GpuAddress(gpu), size).unwrap();
            d.map(s, r, b, offset).unwrap();
        }
        let mut f = Self {
            d,
            c,
            s,
            b,
            vertex_reservation,
            vertex_mapping,
            data,
            setup,
        };
        f.reset();
        f
    }
    fn reset(&mut self) {
        let bytes: Vec<u8> = (0..W * H).flat_map(|_| BG).collect();
        self.d
            .write_backing(self.b, RT as u64, &bytes, &mut mem(&mut self.data))
            .unwrap();
    }
    fn vertices(
        &mut self,
        backing_offset: u64,
        stride: usize,
        offsets: [usize; 2],
        pos: [[f32; 2]; 3],
    ) {
        let mut bytes = vec![0x99; stride * 3];
        for n in 0..3 {
            for (k, v) in [pos[n][0], pos[n][1], 0., 0.5].into_iter().enumerate() {
                // The fourth word is padding for FLOAT3 and explicit W for
                // FLOAT4. In tight20 layout UV overwrites it with0.25 below.
                let a = n * stride + offsets[0] + k * 4;
                bytes[a..a + 4].copy_from_slice(&v.to_le_bytes());
            }
            for (k, v) in [0.25f32, V[n]].into_iter().enumerate() {
                let a = n * stride + offsets[1] + k * 4;
                bytes[a..a + 4].copy_from_slice(&v.to_le_bytes());
            }
        }
        self.d
            .write_backing(self.b, backing_offset, &bytes, &mut mem(&mut self.data))
            .unwrap();
    }
    fn submit(&mut self, words: &[u32]) -> SubmissionReport {
        let bytes: Vec<u8> = words.iter().flat_map(|w| w.to_le_bytes()).collect();
        self.d
            .write_backing(self.b, 0x10000, &bytes, &mut mem(&mut self.data))
            .unwrap();
        self.submit_existing(words.len())
    }
    fn submit_existing(&mut self, length: usize) -> SubmissionReport {
        let entry = (0xb0000u64 | ((length as u64) << 42)).to_le_bytes();
        let mut h = [0; 24];
        h[8..12].copy_from_slice(&1u32.to_le_bytes());
        h[12..16].copy_from_slice(&4u32.to_le_bytes());
        self.d
            .submit(
                self.c,
                KICKOFF_PB,
                &h,
                Some(&entry),
                &mut mem(&mut self.data),
            )
            .unwrap()
    }
    fn publish(&mut self) {
        self.d
            .publish(self.c, &mut mem(&mut self.data))
            .unwrap()
            .unwrap();
    }
}
fn layout(w: &mut Vec<u32>, stride: u32, offsets: [u32; 2], float4: bool, address: u32) {
    for (m, v) in [
        (0x1c00, 0x1000 | stride),
        (0x1c08, address),
        (0x1f04, address + stride * 3 - 1),
        (
            0x1160,
            if float4 { 0x38200000 } else { 0x38400000 } | (offsets[0] << 7),
        ),
        (0x1164, 0x38800000 | (offsets[1] << 7)),
    ] {
        method(w, m, v);
    }
}
fn check(data: &[u8], pos: [[f32; 2]; 3], clip_w: f64) {
    let v = pos.map(|p| p.map(|x| f64::from(x) / clip_w));
    let edge = |a: [f64; 2], b: [f64; 2], p: [f64; 2]| {
        (b[0] - a[0]) * (p[1] - a[1]) - (b[1] - a[1]) * (p[0] - a[0])
    };
    let area = edge(v[0], v[1], v[2]);
    let mut inside = 0;
    let mut outside = 0;
    for y in 0..H {
        for x in 0..W {
            let p = [
                (x as f64 + 0.5) * 2. / 47. - 1.,
                1. - (y as f64 + 0.5) * 2. / 29.,
            ];
            let b = [
                edge(v[1], v[2], p) / area,
                edge(v[2], v[0], p) / area,
                edge(v[0], v[1], p) / area,
            ];
            if b.iter().any(|v| v.abs() < 0.005) {
                continue;
            }
            let actual = &data[RT + (y * W + x) * 4..RT + (y * W + x + 1) * 4];
            if x >= 47 || b.iter().any(|v| *v < 0.) {
                assert_eq!(actual, BG, "outside {x},{y}");
                outside += 1;
                continue;
            }
            // The assembled VS exports fetched W as UV.x. W is uniform for all
            // vertices here; screen-linear interpolation of varying V is analytic.
            let fy = (0..3).map(|n| b[n] * f64::from(V[n])).sum::<f64>() * 19.;
            if (fy - fy.round()).abs() < 0.02 {
                continue;
            }
            let tx = (clip_w * 37.).floor().clamp(0., 36.) as usize;
            let ty = fy.floor().clamp(0., 18.) as usize;
            assert_eq!(actual, texel(tx, ty), "{x},{y}: W={clip_w} texel{tx},{ty}");
            inside += 1;
        }
    }
    assert!(inside > 150 && outside > 200, "{inside}/{outside}");
}
#[test]
#[ignore = "requires actual GPU; native FLOAT3 default W feeds assembled ALD/MOV/AST and TEX"]
fn driver_float3_tight_reordered_inputs_and_synthesized_w() {
    let mut f = Fixture::new();
    let mut baseline = None;
    for (stride, offsets) in [(20, [0, 12]), (32, [8, 0]), (24, [0, 16])] {
        f.reset();
        f.vertices(0x3000, stride, offsets, POS);
        let mut w = if baseline.is_none() {
            f.setup.clone()
        } else {
            Vec::new()
        };
        layout(
            &mut w,
            stride as u32,
            offsets.map(|x| x as u32),
            false,
            0x70000,
        );
        draw(&mut w);
        let before = f.d.graphics_counters();
        let r = f.submit(&w);
        assert!(r.fault.is_none(), "{:?}", r.fault);
        f.publish();
        check(&f.data, POS, 1.);
        f.d.retire(r.token).unwrap();
        let c = f.d.graphics_counters();
        assert_eq!(
            c.vertex_upload_bytes,
            before.vertex_upload_bytes + (stride * 3) as u64
        );
        assert_eq!(
            c.vertex_guest_bytes,
            before.vertex_guest_bytes + (stride * 3) as u64
        );
        let shader = (
            c.shader_module_allocations,
            c.shader_decodes,
            c.vertex_shader_decodes,
        );
        if let Some(old) = baseline {
            assert_eq!(shader, old, "layout change recompiles shader");
        }
        baseline = Some(shader);
    }
    let mut w = Vec::new();
    draw(&mut w);
    assert!(f.submit(&w).fault.is_none());
    let before = f.d.graphics_counters();
    let region = stats_alloc::Region::new(ALLOCATOR);
    let start = std::time::Instant::now();
    for _ in 0..32 {
        assert!(f.submit_existing(w.len()).fault.is_none());
    }
    let elapsed = start.elapsed();
    let alloc = region.change();
    let after = f.d.graphics_counters();
    assert_eq!(after.submissions - before.submissions, 32);
    assert_eq!(after.vertex_upload_bytes, before.vertex_upload_bytes);
    assert_eq!(after.vertex_guest_bytes, before.vertex_guest_bytes);
    assert_eq!(after.index_guest_bytes, before.index_guest_bytes);
    assert_eq!(after.guest_texture_bytes, before.guest_texture_bytes);
    assert_eq!(after.constant_guest_bytes, before.constant_guest_bytes);
    assert_eq!(after.descriptor_reads, before.descriptor_reads);
    assert_eq!(after.upload_bytes, before.upload_bytes);
    assert_eq!(after.shader_binary_bytes, before.shader_binary_bytes);
    assert_eq!(after.vertex_shader_bytes, before.vertex_shader_bytes);
    assert_eq!(after.shader_decodes, before.shader_decodes);
    assert_eq!(after.vertex_shader_decodes, before.vertex_shader_decodes);
    assert_eq!(after.pipeline_allocations, before.pipeline_allocations);
    assert_eq!(
        after.shader_module_allocations,
        before.shader_module_allocations
    );
    assert_eq!(after.readback_operations, before.readback_operations);
    assert_eq!(after.synchronization_points, before.synchronization_points);
    f.publish();
    assert_eq!(
        f.d.graphics_counters().readback_operations,
        after.readback_operations + 1
    );
    assert_eq!(
        f.d.graphics_counters().synchronization_points,
        after.synchronization_points + 1
    );
    check(&f.data, POS, 1.);
    eprintln!(
        "FLOAT3 warm32 {elapsed:?} allocations={} reallocations={} counters={:?}",
        alloc.allocations,
        alloc.reallocations,
        f.d.graphics_counters()
    );
    f.d.close(f.c, &mut mem(&mut f.data)).unwrap();
}
#[test]
#[ignore = "requires actual GPU; format-only pipeline identity, alias/remap and fault-safe publication"]
fn driver_float3_float4_switch_reuses_data_and_shaders_but_not_pipeline() {
    let mut f = Fixture::new();
    f.vertices(0x3000, 24, [0, 16], POS);
    let mut w = f.setup.clone();
    layout(&mut w, 24, [0, 16], false, 0x70000);
    draw(&mut w);
    let r = f.submit(&w);
    assert!(r.fault.is_none());
    f.publish();
    check(&f.data, POS, 1.);
    f.d.retire(r.token).unwrap();
    for (four, address) in [(true, 0x70000), (false, 0x70000), (false, 0x71000)] {
        f.reset();
        let before = f.d.graphics_counters();
        let mut w = Vec::new();
        layout(&mut w, 24, [0, 16], four, address);
        draw(&mut w);
        let r = f.submit(&w);
        assert!(r.fault.is_none(), "{:?}", r.fault);
        let after = f.d.graphics_counters();
        assert_eq!(
            after.pipeline_allocations,
            before.pipeline_allocations + u64::from(address == 0x70000)
        );
        assert_eq!(
            after.shader_module_allocations,
            before.shader_module_allocations
        );
        assert_eq!(after.vertex_shader_decodes, before.vertex_shader_decodes);
        assert_eq!(after.shader_decodes, before.shader_decodes);
        assert_eq!(after.vertex_upload_bytes, before.vertex_upload_bytes);
        f.publish();
        check(&f.data, POS, if four { 0.5 } else { 1. });
        f.d.retire(r.token).unwrap();
    }
    let moved = POS.map(|p| [p[0] + 0.2, p[1] - 0.1]);
    f.reset();
    f.vertices(0x3000, 24, [0, 16], moved);
    let before = f.d.graphics_counters();
    let mut w = Vec::new();
    draw(&mut w);
    let r = f.submit(&w);
    assert!(r.fault.is_none());
    f.publish();
    check(&f.data, moved, 1.);
    f.d.retire(r.token).unwrap();
    let after = f.d.graphics_counters();
    assert_eq!(after.vertex_upload_bytes, before.vertex_upload_bytes + 72);
    assert_eq!(
        after.shader_module_allocations,
        before.shader_module_allocations
    );
    assert_eq!(after.pipeline_allocations, before.pipeline_allocations);
    // Replace numerical GPU VA70000 with a distinct canonical backing subrange.
    let remapped = POS.map(|p| [p[0] - 0.15, p[1] + 0.1]);
    f.reset();
    f.vertices(0x12000, 24, [0, 16], remapped);
    f.d.unmap(f.s, f.vertex_mapping).unwrap();
    f.vertex_mapping = f.d.map(f.s, f.vertex_reservation, f.b, 0x12000).unwrap();
    let mut w = Vec::new();
    layout(&mut w, 24, [0, 16], false, 0x70000);
    draw(&mut w);
    let r = f.submit(&w);
    assert!(r.fault.is_none(), "{:?}", r.fault);
    assert!(f.data[RT..RT + W * H * 4].chunks_exact(4).all(|p| p == BG));
    let before = f.d.graphics_counters();
    let mut bad = Vec::new();
    // Legal FLOAT4 at offset12 cannot fit the existing24-byte stride. Resolution
    // must fault before reusing the previous resident FLOAT3 pipeline.
    method(&mut bad, 0x1160, 0x38200600);
    draw(&mut bad);
    let r = f.submit(&bad);
    assert!(
        r.fault.as_ref().is_some_and(|e| e
            .to_string()
            .contains("invalid vertex stream bounds/layout")),
        "{:?}",
        r.fault
    );
    assert_eq!(f.d.graphics_counters().draw_commands, before.draw_commands);
    assert_eq!(f.d.graphics_counters().submissions, before.submissions);
    assert_eq!(
        f.d.graphics_counters().readback_operations,
        before.readback_operations
    );
    assert!(f.data[RT..RT + W * H * 4].chunks_exact(4).all(|p| p == BG));
    f.publish();
    check(&f.data, remapped, 1.);
    assert_eq!(f.d.graphics_counters().draw_commands, before.draw_commands);
    eprintln!("FLOAT3 transition counters {:?}", f.d.graphics_counters());
    f.d.close(f.c, &mut mem(&mut f.data)).unwrap();
}
