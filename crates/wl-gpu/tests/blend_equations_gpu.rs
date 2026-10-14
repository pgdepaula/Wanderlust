#![cfg(feature = "wgpu-backend")]
mod common;
#[path = "common/gob.rs"]
mod gob;
#[global_allocator]
static ALLOCATOR: &stats_alloc::StatsAlloc<std::alloc::System> = &stats_alloc::INSTRUMENTED_SYSTEM;
use wl_gpu::{driver::*, driver_backend::*, memory::*};
const W: usize = 52;
const H: usize = 29;
const RT: usize = 0x8000;
const SOURCE: [f32; 4] = [0.75, 0.25, 0.625, 0.375];
const BG: [u8; 4] = [51, 153, 204, 230];
const POS: [[[f32; 2]; 3]; 2] = [
    [[-1.2, -0.8], [0.6, -0.6], [-0.5, 0.85]],
    [[-0.5, -0.7], [0.9, 0.1], [-0.1, 0.8]],
];
fn method(w: &mut Vec<u32>, m: u32, v: u32) {
    w.extend([0x20010000 | (m / 4), v]);
}
fn draw(w: &mut Vec<u32>, triangle: u32) {
    w.extend([
        0x20010586,
        4,
        0x200105f7,
        triangle * 3,
        0x200105f8,
        3,
        0x20010585,
        0,
    ]);
}
fn mem(data: &mut [u8]) -> SliceMemoryMut<'_> {
    SliceMemoryMut {
        base: GuestAddress(0),
        bytes: data,
    }
}
struct Fixture {
    d: GpuDriver,
    c: ChannelId,
    b: BackingId,
    data: Vec<u8>,
    setup: Vec<u32>,
    tiled: bool,
}
impl Fixture {
    fn new(tiled: bool) -> Self {
        let mut data = vec![0xabu8; 0x20000];
        for (off, bin) in [
            (
                0x4200,
                include_bytes!("fixtures/maxwell/vertex.bin").as_slice(),
            ),
            (
                0x4500,
                include_bytes!("fixtures/maxwell/ldc.bin").as_slice(),
            ),
        ] {
            data[off..off + bin.len()].copy_from_slice(bin);
        }
        for (n, p) in POS.iter().flatten().enumerate() {
            for (k, v) in [p[0], p[1], 0., 1., 0., 0.].into_iter().enumerate() {
                data[0x3000 + n * 24 + k * 4..0x3004 + n * 24 + k * 4]
                    .copy_from_slice(&v.to_le_bytes());
            }
        }
        for (n, i) in (0u16..6).enumerate() {
            data[0x6000 + n * 2..0x6002 + n * 2].copy_from_slice(&i.to_le_bytes());
        }
        for (n, v) in SOURCE.into_iter().enumerate() {
            data[0x50 + n * 4..0x54 + n * 4].copy_from_slice(&v.to_le_bytes());
        }
        let mut setup = common::hardware::setup();
        for word in &mut setup {
            if *word == 0x40000 {
                *word = 0x40040;
            }
        }
        for (m, v) in [
            (0x2144, 0x500),
            (0x1f04, 0x7008f),
            (0x17c8, 0),
            (0x17cc, 0x80000),
            (0x17d0, 0),
            (0x17d4, 0x8000b),
            (0x17d8, 1),
            (0x1434, 0),
            (0x1438, 0),
            (0x1644, 0),
            (0x1360, 1),
        ] {
            method(&mut setup, m, v);
        }
        if tiled {
            method(&mut setup, 0x808, W as u32);
            method(&mut setup, 0x814, 0x10);
        }
        let mut d = GpuDriver::new().unwrap();
        eprintln!("adapter {:?}", d.capabilities().adapter);
        let b = d
            .register_backing(GuestAddress(0), data.len() as u64)
            .unwrap();
        let s = d.create_space().unwrap();
        let c = d.create_channel(s).unwrap();
        for (gpu, offset, size) in [
            (0x40000, 0, 0x1000),
            (0x50000, 0x4000, 0x1000),
            (0x70000, 0x3000, 0x1000),
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
            b,
            data,
            setup,
            tiled,
        };
        f.reset();
        f
    }
    fn reset(&mut self) {
        let linear: Vec<u8> = (0..W * H).flat_map(|_| BG).collect();
        let bytes = if self.tiled {
            gob::encode(&linear, W, H, 1, &vec![0xab; gob::size(W, H, 1)])
        } else {
            linear
        };
        self.d
            .write_backing(self.b, RT as u64, &bytes, &mut mem(&mut self.data))
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
        let mut request = [0; 24];
        request[8..12].copy_from_slice(&1u32.to_le_bytes());
        request[12..16].copy_from_slice(&4u32.to_le_bytes());
        self.d
            .submit(
                self.c,
                KICKOFF_PB,
                &request,
                Some(&entry),
                &mut mem(&mut self.data),
            )
            .unwrap()
    }
    fn publish(&mut self) -> Vec<u8> {
        self.d
            .publish(self.c, &mut mem(&mut self.data))
            .unwrap()
            .unwrap();
        if self.tiled {
            gob::decode(&self.data[RT..RT + gob::size(W, H, 1)], W, H, 1)
        } else {
            self.data[RT..RT + W * H * 4].to_vec()
        }
    }
}
// Independent mathematical equation labels; no production BlendOperation/factor
// decoder participates in this oracle. Source is exact binary fractions.
fn equation(op: usize, s: f64, d: f64, sf: f64, df: f64) -> f64 {
    match op {
        0 => s * sf + d * df,
        1 => s * sf - d * df,
        2 => d * df - s * sf,
        3 => s.min(d),
        4 => s.max(d),
        _ => unreachable!(),
    }
}
fn expected(destination: [u8; 4], ops: [usize; 2], factors: [[f64; 2]; 2], mask: u8) -> [u8; 4] {
    let mut out = destination;
    for k in 0..4 {
        if mask & (1 << k) != 0 {
            let n = usize::from(k == 3);
            out[k] = (equation(
                ops[n],
                f64::from(SOURCE[k]),
                f64::from(destination[k]) / 255.,
                factors[n][0],
                factors[n][1],
            )
            .clamp(0., 1.)
                * 255.)
                .round() as u8;
        }
    }
    out
}
fn coverage(x: usize, y: usize, triangle: usize) -> Option<bool> {
    let p = [
        (x as f64 + 0.5) * 2. / 47. - 1.,
        1. - (y as f64 + 0.5) * 2. / 29.,
    ];
    let v = POS[triangle].map(|p| p.map(f64::from));
    let edge = |a: [f64; 2], b: [f64; 2], p: [f64; 2]| {
        (b[0] - a[0]) * (p[1] - a[1]) - (b[1] - a[1]) * (p[0] - a[0])
    };
    let area = edge(v[0], v[1], v[2]);
    let b = [
        edge(v[1], v[2], p) / area,
        edge(v[2], v[0], p) / area,
        edge(v[0], v[1], p) / area,
    ];
    if b.iter().any(|v| v.abs() < 0.005) {
        None
    } else {
        Some(x < 47 && b.iter().all(|v| *v > 0.))
    }
}
type OracleLayer = ([usize; 2], [[f64; 2]; 2], u8, usize);
fn check(pixels: &[u8], layers: &[OracleLayer]) {
    let mut inside = 0;
    let mut outside = 0;
    for y in 0..H {
        for x in 0..W {
            let mut color = BG;
            let mut hit = false;
            let mut tie = false;
            for &(ops, factors, mask, triangle) in layers {
                match coverage(x, y, triangle) {
                    Some(true) => {
                        color = expected(color, ops, factors, mask);
                        hit = true;
                    }
                    Some(false) => (),
                    None => {
                        tie = true;
                        break;
                    }
                }
            }
            if tie {
                continue;
            }
            if hit {
                inside += 1;
            } else {
                outside += 1;
            }
            let actual = &pixels[(y * W + x) * 4..(y * W + x + 1) * 4];
            for k in 0..4 {
                assert!(
                    actual[k].abs_diff(color[k]) <= 1,
                    "{x},{y}: {actual:?} expected {color:?}"
                );
            }
        }
    }
    assert!(inside > 100 && outside > 300);
}
fn blend(w: &mut Vec<u32>, ops: [u32; 2], factors: [u32; 4], separate: bool) {
    method(w, 0x133c, u32::from(separate));
    // Literal incrementing B06F packet crosses inactive color-key register1354.
    w.extend([
        0x200704d0, ops[0], factors[0], factors[1], ops[1], factors[2], 0, factors[3],
    ]);
}
#[test]
#[ignore = "requires actual GPU; all independent RGB/alpha equations and compact/GL aliases"]
fn driver_blend_equation_matrix_aliases_factors_and_pipeline_identity() {
    let mut f = Fixture::new(false);
    let aliases = [0x8006, 0x800a, 0x800b, 0x8007, 0x8008];
    let mut baseline = None;
    for rgb in 0..5 {
        for alpha in 0..5 {
            for gl in [false, true] {
                f.reset();
                let before = f.d.graphics_counters();
                let mut w = if baseline.is_none() {
                    f.setup.clone()
                } else {
                    Vec::new()
                };
                let ops = if gl {
                    [aliases[rgb], aliases[alpha]]
                } else {
                    [rgb as u32 + 1, alpha as u32 + 1]
                };
                // Deliberately contradictory ZERO factors for MIN/MAX.
                let rf = if rgb >= 3 { 1 } else { 2 };
                let af = if alpha >= 3 { 1 } else { 2 };
                blend(&mut w, ops, [rf, rf, af, af], true);
                draw(&mut w, 0);
                let r = f.submit(&w);
                assert!(r.fault.is_none(), "{:?}", r.fault);
                let after = f.d.graphics_counters();
                if gl {
                    assert_eq!(
                        after.pipeline_allocations, before.pipeline_allocations,
                        "alias rebuilds pipeline"
                    );
                }
                if let Some((modules, decodes, vertex_decodes)) = baseline {
                    assert_eq!(
                        (
                            after.shader_module_allocations,
                            after.shader_decodes,
                            after.vertex_shader_decodes
                        ),
                        (modules, decodes, vertex_decodes)
                    );
                }
                baseline = Some((
                    after.shader_module_allocations,
                    after.shader_decodes,
                    after.vertex_shader_decodes,
                ));
                let p = f.publish();
                check(&p, &[([rgb, alpha], [[1.; 2]; 2], 15, 0)]);
                f.d.retire(r.token).unwrap();
            }
        }
    }
    // Source-alpha factors must still affect SUBTRACT/REVERSE_SUBTRACT.
    for op in [1, 2] {
        f.reset();
        let mut w = Vec::new();
        blend(&mut w, [op as u32 + 1, 1], [5, 6, 5, 1], true);
        draw(&mut w, 0);
        let r = f.submit(&w);
        assert!(r.fault.is_none());
        let p = f.publish();
        check(&p, &[([op, 0], [[0.375, 0.625], [0.375, 0.]], 15, 0)]);
        f.d.retire(r.token).unwrap();
    }
    eprintln!("blend matrix counters {:?}", f.d.graphics_counters());
    f.d.close(f.c, &mut mem(&mut f.data)).unwrap();
}
#[test]
#[ignore = "requires actual GPU; ordered equations, tiled padding, raw factors and partial fault"]
fn driver_deferred_tiled_blends_masks_factor_restore_and_fault_publication() {
    let mut f = Fixture::new(true);
    let original = f.data[RT..RT + gob::size(W, H, 1)].to_vec();
    let mut w = f.setup.clone();
    blend(&mut w, [4, 5], [5, 6, 1, 1], true); // MINrgb/MAXalpha ignores factors.
    draw(&mut w, 0);
    let a = f.submit(&w);
    assert!(a.fault.is_none());
    let mut w = Vec::new();
    method(&mut w, 0x1340, 3); // Reverse subtraction restores raw SrcAlpha/inverse.
    method(&mut w, 0x133c, 0); // Alpha inherits both RGB operation and factors.
    method(&mut w, 0x1a00, 0x101); // Write only R/B.
    draw(&mut w, 1);
    let b = f.submit(&w);
    assert!(b.fault.is_none());
    assert_eq!(&f.data[RT..RT + original.len()], original);
    assert_eq!(f.d.graphics_counters().readback_operations, 0);
    let before = f.d.graphics_counters();
    let mut w = Vec::new();
    method(&mut w, 0x134c, 0x901c); // Weighted MIN is unsupported, even while separate=false.
    draw(&mut w, 0);
    let fault = f.submit(&w);
    assert!(
        fault.fault.as_ref().is_some_and(|e| e
            .to_string()
            .contains("UnsupportedValue { method: 4940, value: 36892 }")),
        "{:?}",
        fault.fault
    );
    assert_eq!(f.d.graphics_counters().draw_commands, before.draw_commands);
    assert_eq!(&f.data[RT..RT + original.len()], original);
    let p = f.publish();
    check(
        &p,
        &[
            ([3, 4], [[1.; 2]; 2], 15, 0),
            ([2, 2], [[0.375, 0.625]; 2], 5, 1),
        ],
    );
    // Oracle-addressed padding must stay byte-identical, including final GOBs.
    let mut valid = vec![false; original.len()];
    for y in 0..H {
        for x in 0..W * 4 {
            valid[gob::address(x, y, W, 1)] = true;
        }
    }
    for (i, used) in valid.into_iter().enumerate() {
        if !used {
            assert_eq!(f.data[RT + i], original[i], "padding {i}");
        }
    }
    assert_eq!(f.d.graphics_counters().readback_operations, 1);
    assert_eq!(f.d.graphics_counters().draw_commands, 2);
    assert_eq!(
        f.d.graphics_counters().shader_module_allocations,
        before.shader_module_allocations
    );
    f.d.close(f.c, &mut mem(&mut f.data)).unwrap();
}
#[test]
#[ignore = "requires actual GPU; MIN factor normalization preserves warm deferred residency"]
fn driver_min_ignored_factor_changes_and_warm_deferred_draws() {
    let mut f = Fixture::new(true);
    let mut w = f.setup.clone();
    blend(&mut w, [4, 4], [1; 4], false);
    draw(&mut w, 0);
    assert!(f.submit(&w).fault.is_none());
    let before = f.d.graphics_counters();
    let mut w = Vec::new();
    blend(&mut w, [0x8007, 0x8008], [5, 6, 2, 2], false);
    draw(&mut w, 0);
    assert!(f.submit(&w).fault.is_none());
    assert_eq!(
        f.d.graphics_counters().pipeline_allocations,
        before.pipeline_allocations
    );
    let before = f.d.graphics_counters();
    let region = stats_alloc::Region::new(ALLOCATOR);
    let start = std::time::Instant::now();
    for _ in 0..32 {
        assert!(f.submit_existing(w.len()).fault.is_none());
    }
    let elapsed = start.elapsed();
    let alloc = region.change();
    let after = f.d.graphics_counters();
    assert_eq!(after.draw_commands - before.draw_commands, 32);
    assert_eq!(after.submissions - before.submissions, 32);
    assert_eq!(after.upload_bytes, before.upload_bytes);
    assert_eq!(after.texture_conversions, before.texture_conversions);
    assert_eq!(after.target_deswizzle_bytes, before.target_deswizzle_bytes);
    assert_eq!(after.target_swizzle_bytes, before.target_swizzle_bytes);
    assert_eq!(
        after.render_target_allocations,
        before.render_target_allocations
    );
    assert_eq!(after.shader_binary_bytes, before.shader_binary_bytes);
    assert_eq!(after.vertex_shader_bytes, before.vertex_shader_bytes);
    assert_eq!(after.shader_decodes, before.shader_decodes);
    assert_eq!(after.vertex_shader_decodes, before.vertex_shader_decodes);
    assert_eq!(
        after.shader_module_allocations,
        before.shader_module_allocations
    );
    assert_eq!(after.pipeline_allocations, before.pipeline_allocations);
    assert_eq!(after.readback_operations, before.readback_operations);
    assert_eq!(after.synchronization_points, before.synchronization_points);
    let p = f.publish();
    check(&p, &[([3, 3], [[1.; 2]; 2], 15, 0)]);
    eprintln!(
        "blend warm32 {elapsed:?} allocations={} reallocations={} before={before:?} after={after:?}",
        alloc.allocations, alloc.reallocations
    );
    f.d.close(f.c, &mut mem(&mut f.data)).unwrap();
}

// Independent coefficient mathematics. Labels below are test-local, not the
// production enum or decoder: zero, one, source color, its complement,
// destination alpha, its complement, destination color, its complement.
fn component_blend(
    destination: [u8; 4],
    coefficients: [u8; 4],
    separate: bool,
    mask: u8,
) -> [u8; 4] {
    let s = SOURCE.map(f64::from);
    let d = destination.map(|v| f64::from(v) / 255.);
    let factor = |n, k| match n {
        0 => 0.,
        1 => 1.,
        2 => s[k],
        3 => 1. - s[k],
        4 => d[3],
        5 => 1. - d[3],
        6 => d[k],
        7 => 1. - d[k],
        _ => unreachable!(),
    };
    let mut result = destination;
    for k in 0..4 {
        if mask & (1 << k) != 0 {
            let n = if k == 3 && separate { 2 } else { 0 };
            result[k] = ((s[k] * factor(coefficients[n], k)
                + d[k] * factor(coefficients[n + 1], k))
            .clamp(0., 1.)
                * 255.)
                .round() as u8;
        }
    }
    result
}
type ComponentLayer = ([u8; 4], bool, u8, usize);
fn check_components(pixels: &[u8], layers: &[ComponentLayer]) {
    let mut checked = [0usize; 4];
    for y in 0..H {
        for x in 0..W {
            let mut expected = BG;
            let mut hits = 0;
            let mut tie = false;
            for &(coefficients, separate, mask, triangle) in layers {
                match coverage(x, y, triangle) {
                    Some(true) => {
                        expected = component_blend(expected, coefficients, separate, mask);
                        hits |= 1 << triangle;
                    }
                    Some(false) => (),
                    None => {
                        tie = true;
                        break;
                    }
                }
            }
            if tie {
                continue;
            }
            for k in 0..4 {
                let actual = pixels[(y * W + x) * 4 + k];
                assert!(
                    actual.abs_diff(expected[k]) <= 1,
                    "component blend {x},{y} channel{k}: {actual} != {}",
                    expected[k]
                );
            }
            checked[hits] += 1;
        }
    }
    assert!(
        checked[0] > 300 && checked.iter().skip(1).sum::<usize>() > 100,
        "{checked:?}"
    );
    if layers.len() == 2 {
        assert!(checked.iter().all(|n| *n > 20), "{checked:?}");
    }
}
#[test]
#[ignore = "requires actual GPU; component and destination-alpha coefficients in all four B197 slots"]
fn driver_component_blend_factor_matrix_and_alias_reuse() {
    let mut f = Fixture::new(false);
    let encodings = [
        (3, 0x4300),
        (4, 0x4301),
        (7, 0x4304),
        (8, 0x4305),
        (9, 0x4306),
        (10, 0x4307),
    ];
    let mut baseline = None;
    for slot in 0..4 {
        for (kind, &(compact, gl_alias)) in encodings.iter().enumerate() {
            for gl in [false, true] {
                f.reset();
                let before = f.d.graphics_counters();
                let mut w = if baseline.is_none() {
                    f.setup.clone()
                } else {
                    Vec::new()
                };
                let mut raw = [2, 1, 2, 1];
                raw[slot] = if gl { gl_alias } else { compact };
                let mut oracle = [1, 0, 1, 0];
                oracle[slot] = kind as u8 + 2;
                blend(&mut w, [1, 1], raw, true);
                draw(&mut w, 0);
                let report = f.submit(&w);
                assert!(report.fault.is_none(), "{:?}", report.fault);
                let after = f.d.graphics_counters();
                if gl {
                    assert_eq!(after.pipeline_allocations, before.pipeline_allocations);
                }
                let identity = (
                    after.shader_module_allocations,
                    after.shader_decodes,
                    after.vertex_shader_decodes,
                );
                if let Some(old) = baseline {
                    assert_eq!(identity, old);
                }
                baseline = Some(identity);
                check_components(&f.publish(), &[(oracle, true, 15, 0)]);
                f.d.retire(report.token).unwrap();
            }
        }
    }
    // Overwrite with S*S on every draw: repeated real blending has a stable
    // independent result while exercising new factors without CPU visibility.
    f.reset();
    let mut w = Vec::new();
    blend(&mut w, [1, 1], [3, 1, 3, 1], true);
    draw(&mut w, 0);
    let first = f.submit(&w);
    assert!(first.fault.is_none());
    let before = f.d.graphics_counters();
    let mut w = Vec::new();
    draw(&mut w, 0);
    let region = stats_alloc::Region::new(&stats_alloc::INSTRUMENTED_SYSTEM);
    let start = std::time::Instant::now();
    let mut tokens = [None; 32];
    for token in &mut tokens {
        let r = f.submit(&w);
        assert!(r.fault.is_none());
        *token = Some(r.token);
    }
    let elapsed = start.elapsed();
    let allocation = region.change();
    let after = f.d.graphics_counters();
    assert_eq!(after.draw_commands - before.draw_commands, 32);
    assert_eq!(after.submissions - before.submissions, 32);
    assert_eq!(
        (
            after.upload_bytes,
            after.readback_operations,
            after.synchronization_points,
            after.pipeline_allocations,
            after.shader_module_allocations,
            after.shader_binary_bytes,
            after.vertex_shader_bytes,
            after.vertex_guest_bytes,
            after.index_guest_bytes,
            after.descriptor_reads,
            after.render_target_guest_bytes
        ),
        (
            before.upload_bytes,
            before.readback_operations,
            before.synchronization_points,
            before.pipeline_allocations,
            before.shader_module_allocations,
            before.shader_binary_bytes,
            before.vertex_shader_bytes,
            before.vertex_guest_bytes,
            before.index_guest_bytes,
            before.descriptor_reads,
            before.render_target_guest_bytes
        )
    );
    check_components(&f.publish(), &[([2, 0, 2, 0], true, 15, 0)]);
    f.d.retire(first.token).unwrap();
    for token in tokens {
        f.d.retire(token.unwrap()).unwrap();
    }
    assert_eq!(
        f.d.graphics_counters().readback_operations - after.readback_operations,
        1
    );
    assert_eq!(
        f.d.graphics_counters().synchronization_points - after.synchronization_points,
        1
    );
    eprintln!(
        "component coefficients warm32 {elapsed:?} allocations={} reallocations={}",
        allocation.allocations, allocation.reallocations
    );
    eprintln!("component coefficient matrix {:?}", f.d.graphics_counters());
    f.d.close(f.c, &mut mem(&mut f.data)).unwrap();
}
#[test]
#[ignore = "requires actual GPU; destination reads, inherited alpha, masks and tiled overlapping draws"]
fn driver_component_blend_order_masks_and_pending_fault() {
    let mut f = Fixture::new(true);
    let mut w = f.setup.clone();
    // S*S + D*(1-S), including alpha inherited from RGB despite unrelated
    // explicitly programmed alpha coefficients.
    blend(&mut w, [1, 1], [3, 4, 1, 1], false);
    draw(&mut w, 0);
    let first = f.submit(&w);
    assert!(first.fault.is_none());
    assert_eq!(
        &f.data[RT..RT + gob::size(W, H, 1)],
        gob::encode(&BG.repeat(W * H), W, H, 1, &vec![0xab; gob::size(W, H, 1)])
    );
    // Read the first draw's updated destination alpha; mask alpha off, but
    // destination alpha must still affect the second draw's RGB equation.
    let mut w = Vec::new();
    blend(&mut w, [1, 1], [7, 9, 9, 10], true);
    method(&mut w, 0x1a00, 0x101);
    draw(&mut w, 1);
    let second = f.submit(&w);
    assert!(second.fault.is_none());
    let before = f.d.graphics_counters();
    let unchanged = gob::encode(&BG.repeat(W * H), W, H, 1, &vec![0xab; gob::size(W, H, 1)]);
    assert_eq!(&f.data[RT..RT + unchanged.len()], unchanged);
    assert_eq!(before.readback_operations, 0);
    assert_eq!(before.synchronization_points, 0);
    // Independent rational arithmetic: overlap [230,131,234,180]; reversing
    // draw order gives [189,131,195,180], well beyond conversion tolerance.
    assert_eq!(
        component_blend(
            component_blend(BG, [2, 3, 0, 0], false, 15),
            [4, 6, 6, 7],
            true,
            5
        ),
        [230, 131, 234, 180]
    );
    // Constant blending remains unsupported. Earlier draws remain publishable.
    let fault = f.submit(&[0x200104d1, 0xc001]);
    assert!(
        fault.fault.as_ref().is_some_and(|e| e
            .to_string()
            .contains("UnsupportedValue { method: 4932, value: 49153 }")),
        "{:?}",
        fault.fault
    );
    assert_eq!(&f.data[RT..RT + unchanged.len()], unchanged);
    assert_eq!(f.d.graphics_counters().readback_operations, 0);
    assert_eq!(f.d.graphics_counters().synchronization_points, 0);
    assert_eq!(f.d.graphics_counters().draw_commands, before.draw_commands);
    assert_eq!(f.d.graphics_counters().submissions, before.submissions);
    check_components(
        &f.publish(),
        &[([2, 3, 0, 0], false, 15, 0), ([4, 6, 6, 7], true, 5, 1)],
    );
    assert_eq!(f.d.graphics_counters().readback_operations, 1);
    assert_eq!(f.d.graphics_counters().synchronization_points, 1);
    let mut used = vec![false; gob::size(W, H, 1)];
    for y in 0..H {
        for x in 0..W * 4 {
            used[gob::address(x, y, W, 1)] = true;
        }
    }
    for (n, used) in used.into_iter().enumerate() {
        if !used {
            assert_eq!(f.data[RT + n], 0xab);
        }
    }
    assert_eq!(f.d.graphics_counters().draw_commands, before.draw_commands);
    f.d.close(f.c, &mut mem(&mut f.data)).unwrap();
}
