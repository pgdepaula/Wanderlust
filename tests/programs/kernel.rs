// Shared floating-point/SIMD workload, included by guest.rs (AArch64, no_std) and host.rs
// (the oracle). Only correctly rounded IEEE operations are used, so both builds must produce
// bit-identical results. Inputs derive from a runtime seed so nothing is constant-folded.

#[cfg(target_arch = "aarch64")]
mod ops {
    use core::arch::aarch64::*;
    #[inline]
    pub fn sqrt(x: f32) -> f32 {
        unsafe { vgetq_lane_f32::<0>(vsqrtq_f32(vdupq_n_f32(x))) }
    }
    #[inline]
    pub fn fma(a: f32, b: f32, c: f32) -> f32 {
        // a * b + c with one rounding.
        unsafe { vgetq_lane_f32::<0>(vfmaq_f32(vdupq_n_f32(c), vdupq_n_f32(a), vdupq_n_f32(b))) }
    }
    #[inline]
    pub fn round_even(x: f64) -> f64 {
        unsafe { vgetq_lane_f64::<0>(vrndnq_f64(vdupq_n_f64(x))) }
    }
    #[inline]
    pub fn floor(x: f32) -> f32 {
        unsafe { vgetq_lane_f32::<0>(vrndmq_f32(vdupq_n_f32(x))) }
    }
}
#[cfg(not(target_arch = "aarch64"))]
mod ops {
    pub fn sqrt(x: f32) -> f32 {
        x.sqrt()
    }
    pub fn fma(a: f32, b: f32, c: f32) -> f32 {
        a.mul_add(b, c)
    }
    pub fn round_even(x: f64) -> f64 {
        x.round_ties_even()
    }
    pub fn floor(x: f32) -> f32 {
        x.floor()
    }
}

const N: usize = 128;
static WEIGHTS: [f32; 8] = [0.5, -1.25, 3.0, 0.125, -0.75, 2.5, 1.0e-3, 7.0];

fn next(state: &mut u32) -> u32 {
    *state ^= *state << 13;
    *state ^= *state >> 17;
    *state ^= *state << 5;
    *state
}
#[inline(never)]
fn generate(seed: u32, points: &mut [[f32; 4]; N]) {
    let mut s = seed | 1;
    for p in points.iter_mut() {
        for (i, c) in p.iter_mut().enumerate() {
            let r = next(&mut s);
            // Signed integer to float, scaled; includes a few tiny and large values.
            *c = (r as i32 as f32) * 1.0e-6 + WEIGHTS[(r as usize + i) % 8];
        }
    }
}
/// Normalized quaternion -> rotation matrix, then scale/translate.
#[inline(never)]
fn matrix(seed: u32) -> [[f32; 4]; 4] {
    let mut s = seed ^ 0x9e37_79b9;
    let mut q = [0f32; 4];
    for c in q.iter_mut() {
        *c = (next(&mut s) % 2001) as f32 / 1000.0 - 1.0;
    }
    let norm = ops::sqrt(q.iter().map(|c| c * c).sum::<f32>() + 1.0e-6);
    let [w, x, y, z] = q.map(|c| c / norm);
    [
        [1.0 - 2.0 * (y * y + z * z), 2.0 * (x * y - w * z), 2.0 * (x * z + w * y), 0.25],
        [2.0 * (x * y + w * z), 1.0 - 2.0 * (x * x + z * z), 2.0 * (y * z - w * x), -0.5],
        [2.0 * (x * z - w * y), 2.0 * (y * z + w * x), 1.0 - 2.0 * (x * x + y * y), 1.75],
        [0.0, 0.0, 0.0, 1.0],
    ]
}
#[inline(never)]
fn transform(m: &[[f32; 4]; 4], points: &mut [[f32; 4]; N]) {
    for p in points.iter_mut() {
        let v = *p;
        let mut out = [0f32; 4];
        for (r, row) in m.iter().enumerate() {
            out[r] = ops::fma(row[3], v[3], ops::fma(row[2], v[2], ops::fma(row[1], v[1], row[0] * v[0])));
        }
        *p = out;
    }
}
#[inline(never)]
fn reduce(points: &[[f32; 4]; N]) -> (f32, f32, f64, i64) {
    let (mut lo, mut hi) = (f32::INFINITY, f32::NEG_INFINITY);
    let mut energy = 0f64;
    let mut ints = 0i64;
    for p in points {
        for &c in p {
            lo = lo.min(c);
            hi = hi.max(c);
            energy += f64::from(c) * f64::from(c);
            ints = ints.wrapping_add((c * 1024.0) as i32 as i64) ^ (ops::floor(c) as i64);
        }
    }
    (lo, hi, energy, ints)
}
#[inline(never)]
fn histogram(points: &[[f32; 4]; N]) -> [u16; 16] {
    let mut h = [0u16; 16];
    for p in points {
        let len = ops::sqrt(p[0] * p[0] + p[1] * p[1] + p[2] * p[2]);
        let bucket = ((len * 2.0) as u32).min(15);
        h[bucket as usize] = h[bucket as usize].wrapping_add(1);
    }
    h
}
pub fn kernel(seed: u64) -> u64 {
    let mut points = [[0f32; 4]; N];
    generate(seed as u32, &mut points);
    let m = matrix((seed >> 32) as u32);
    transform(&m, &mut points);
    let (lo, hi, energy, ints) = reduce(&points);
    let h = histogram(&points);
    let mut hash = 0xcbf2_9ce4_8422_2325u64;
    let mut mix = |x: u64| hash = (hash ^ x).wrapping_mul(0x0000_0100_0000_01b3);
    for p in &points {
        for c in p {
            mix(u64::from(c.to_bits()));
        }
    }
    mix(u64::from(lo.to_bits()));
    mix(u64::from(hi.to_bits()));
    mix(energy.to_bits());
    mix(ops::round_even(energy * 0.5).to_bits());
    mix(ints as u64);
    for b in h {
        mix(u64::from(b));
    }
    hash
}
