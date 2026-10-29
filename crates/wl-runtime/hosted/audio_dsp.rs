//! Checked auxiliary DSP memory exchange. Samples here are normalized floats.
//! Wire semantics: Ryujinx.Audio 4930dced, AuxiliaryBufferCommand,
//! AuxiliaryBufferEffect and AuxiliaryBufferHeader (REV10+ lost-count
//! preservation); independently implemented with checked guest access.
use wl_guest_runtime::{Memory, RuntimeResult};
type GuestWrites = Vec<(u64, Vec<u8>)>;

#[derive(Clone, Debug)]
pub struct Auxiliary {
    /// CPU header address; the DSP-owned header follows at +0x40, samples at +0x80.
    pub send_info: u64,
    pub return_info: u64,
    pub capacity: u32,
    pub enabled: bool,
}
#[derive(Debug)]
pub struct AuxiliaryResult {
    pub quantized_input: Vec<f32>,
    pub output: Vec<f32>,
}
fn address(base: u64, offset: u64) -> RuntimeResult<u64> {
    base.checked_add(offset)
        .ok_or_else(|| "audio auxiliary address overflow".into())
}
fn overlap(a: (u64, u64), b: (u64, u64)) -> bool {
    a.0 < b.1 && b.0 < a.1
}
fn write_plan(memory: &Memory, writes: &[(u64, Vec<u8>)]) -> RuntimeResult<()> {
    for (at, bytes) in writes {
        memory.validate_write(*at, bytes.len())?;
    }
    Ok(())
}
pub(crate) fn commit(memory: &mut Memory, writes: Vec<(u64, Vec<u8>)>) -> RuntimeResult<()> {
    // Exclusive Memory access spans validation and every write; no guest CPU,
    // remapping or asynchronous borrow can intervene after this preflight.
    write_plan(memory, &writes)?;
    for (at, bytes) in writes {
        memory.write_bytes(at, &bytes)?;
    }
    Ok(())
}
impl Auxiliary {
    /// Validate a complete frame before any channel mutates a shared ring.
    pub fn preflight(&self, memory: &Memory, channels: usize, samples: usize) -> RuntimeResult<()> {
        let total = channels
            .checked_mul(samples)
            .ok_or("auxiliary frame overflow")?;
        if channels == 0
            || channels > 6
            || samples == 0
            || samples > 1024
            || total > self.capacity as usize
            || self.capacity == 0
            || self.capacity > 16 * 1024 * 1024
            || self.send_info == 0
            || self.return_info == 0
            || self.send_info % 4 != 0
            || self.return_info % 4 != 0
        {
            return Err("unsupported auxiliary frame geometry".into());
        }
        let bytes = 128usize
            .checked_add(self.capacity as usize * 4)
            .ok_or("auxiliary footprint overflow")?;
        for base in [self.send_info, self.return_info] {
            let end = address(base, bytes as u64)?;
            let mut at = base;
            while at < end {
                let index = memory
                    .region_index(at)
                    .ok_or("unmapped auxiliary backing")?;
                let region = &memory.regions[index];
                if !region.read {
                    return Err("unreadable auxiliary backing".into());
                }
                at = region.end().min(end);
            }
            memory.validate_write(address(base, 64)?, 16)?;
        }
        memory.validate_write(address(self.send_info, 128)?, self.capacity as usize * 4)?;
        for at in [address(self.send_info, 68)?, address(self.return_info, 64)?] {
            if memory.load(at, 4)? > u64::from(self.capacity) {
                return Err("unsupported out-of-range auxiliary cursor".into());
            }
        }
        if overlap(
            (self.send_info, address(self.send_info, bytes as u64)?),
            (self.return_info, address(self.return_info, bytes as u64)?),
        ) {
            return Err("unsupported overlapping auxiliary rings".into());
        }
        Ok(())
    }
    pub fn exchange_channel(
        &self,
        memory: &mut Memory,
        input: &[f32],
        offset: u32,
        update: u32,
    ) -> RuntimeResult<AuxiliaryResult> {
        let (result, writes) = self.plan_channel(memory, input, offset, update)?;
        commit(memory, writes)?;
        Ok(result)
    }
    /// Build a checked transaction without changing the guest ring. Distinct
    /// effect ranges let a graph stage a complete frame before any write.
    pub(crate) fn plan_channel(
        &self,
        memory: &Memory,
        input: &[f32],
        offset: u32,
        update: u32,
    ) -> RuntimeResult<(AuxiliaryResult, GuestWrites)> {
        if input.is_empty()
            || input.len() > 1024
            || self.capacity == 0
            || self.capacity > 16 * 1024 * 1024
            || update > self.capacity
        {
            return Err("unsupported audio auxiliary command geometry".into());
        }
        if self.send_info == 0
            || self.return_info == 0
            || self.send_info % 4 != 0
            || self.return_info % 4 != 0
        {
            return Err("unaligned or null audio auxiliary header".into());
        }
        let footprint = 128u64 + u64::from(self.capacity) * 4;
        let send_end = address(self.send_info, footprint)?;
        let return_end = address(self.return_info, footprint)?;
        if overlap((self.send_info, send_end), (self.return_info, return_end)) {
            return Err("unsupported overlapping audio auxiliary rings".into());
        }
        let send_header = address(self.send_info, 64)?;
        let return_header = address(self.return_info, 64)?;
        let mut writes = Vec::new();
        if !self.enabled {
            for header in [send_header, return_header] {
                for field in [0, 4, 12] {
                    writes.push((address(header, field)?, 0u32.to_le_bytes().to_vec()));
                }
            }
            write_plan(memory, &writes)?;
            return Ok((
                AuxiliaryResult {
                    quantized_input: input.to_vec(),
                    output: input.to_vec(),
                },
                writes,
            ));
        }
        if input.len() > self.capacity as usize {
            return Err("unsupported auxiliary span larger than ring".into());
        }
        let mut packed = Vec::with_capacity(input.len() * 4);
        let mut quantized_input = Vec::with_capacity(input.len());
        for &sample in input {
            if !sample.is_finite() {
                return Err("nonfinite audio auxiliary input".into());
            }
            let value = (f64::from(sample) * 32768.0)
                .trunc()
                .clamp(f64::from(i32::MIN), f64::from(i32::MAX)) as i32;
            packed.extend(value.to_le_bytes());
            quantized_input.push(value as f32 / 32768.0);
        }
        let write_cursor = memory.load(address(send_header, 4)?, 4)? as u32;
        let read_cursor = memory.load(return_header, 4)? as u32;
        let write_start = write_cursor
            .checked_add(offset)
            .ok_or("audio auxiliary write offset overflow")?;
        let read_start = read_cursor
            .checked_add(offset)
            .ok_or("audio auxiliary read offset overflow")?;
        let capacity = self.capacity as usize;
        let count = input.len();
        let mut output = vec![0.0; count];
        if read_start <= self.capacity && count != 0 {
            let first = read_start as usize % capacity;
            let n = count.min(capacity - first);
            let mut bytes = vec![0u8; count * 4];
            memory.read_into(
                address(self.return_info, 128 + first as u64 * 4)?,
                &mut bytes[..n * 4],
            )?;
            if n < count {
                memory.read_into(address(self.return_info, 128)?, &mut bytes[n * 4..])?;
            }
            for (sample, raw) in output.iter_mut().zip(bytes.chunks_exact(4)) {
                *sample = i32::from_le_bytes(raw.try_into().unwrap()) as f32 / 32768.0;
            }
            if update != 0 {
                let next = (u64::from(read_cursor) + u64::from(update)) % u64::from(self.capacity);
                writes.push((return_header, (next as u32).to_le_bytes().to_vec()));
            }
        }
        if write_start <= self.capacity && count != 0 {
            let first = write_start as usize % capacity;
            let n = count.min(capacity - first);
            writes.push((
                address(self.send_info, 128 + first as u64 * 4)?,
                packed[..n * 4].to_vec(),
            ));
            if n < count {
                writes.push((address(self.send_info, 128)?, packed[n * 4..].to_vec()));
            }
            if update != 0 {
                let next = (u64::from(write_cursor) + u64::from(update)) % u64::from(self.capacity);
                writes.push((
                    address(send_header, 4)?,
                    (next as u32).to_le_bytes().to_vec(),
                ));
            }
        }
        write_plan(memory, &writes)?;
        Ok((
            AuxiliaryResult {
                quantized_input,
                output,
            },
            writes,
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn memory() -> Memory {
        let mut m = Memory::default();
        m.map(0x1000, vec![0xa5; 4096], true, true, false).unwrap();
        m.map(0x3000, vec![0x5a; 4096], true, true, false).unwrap();
        m
    }
    fn config() -> Auxiliary {
        Auxiliary {
            send_info: 0x1000,
            return_info: 0x3000,
            capacity: 8,
            enabled: true,
        }
    }
    #[test]
    fn wraps_little_endian_samples_and_only_updates_dsp_owned_cursors() {
        let mut m = memory();
        m.store(0x1044, 4, 6).unwrap();
        m.store(0x3040, 4, 7).unwrap();
        for (i, v) in [16384i32, -16384, 32767, -32768, 0, 1, 2, 3]
            .iter()
            .enumerate()
        {
            m.write_bytes(0x3080 + i as u64 * 4, &v.to_le_bytes())
                .unwrap();
        }
        let cpu = m.regions[0].bytes[..64].to_vec();
        let ret_cpu = m.regions[1].bytes[..64].to_vec();
        let r = config()
            .exchange_channel(&mut m, &[0.5, -0.25, 1.0], 0, 3)
            .unwrap();
        assert_eq!(r.output, vec![3.0 / 32768.0, 0.5, -0.5]);
        assert_eq!(r.quantized_input, vec![0.5, -0.25, 1.0]);
        assert_eq!(m.load(0x1098, 4).unwrap(), 16384);
        assert_eq!(m.load(0x109c, 4).unwrap() as u32, (-8192i32) as u32);
        assert_eq!(m.load(0x1080, 4).unwrap(), 32768);
        assert_eq!(m.load(0x1044, 4).unwrap(), 1);
        assert_eq!(m.load(0x3040, 4).unwrap(), 2);
        assert_eq!(&m.regions[0].bytes[..64], cpu);
        assert_eq!(&m.regions[1].bytes[..64], ret_cpu);
        assert_eq!(m.load(0x1048, 4).unwrap(), 0xa5a5a5a5);
        assert_eq!(m.load(0x3044, 4).unwrap(), 0x5a5a5a5a);
    }
    #[test]
    fn failed_second_destination_preserves_both_rings() {
        let mut m = memory();
        m.store(0x1044, 4, 0).unwrap();
        m.store(0x3040, 4, 0).unwrap();
        m.regions[1].write = false;
        let before: Vec<_> = m.regions.iter().map(|r| r.bytes.clone()).collect();
        assert!(config().exchange_channel(&mut m, &[0.5], 0, 1).is_err());
        for (r, b) in m.regions.iter().zip(before) {
            assert_eq!(r.bytes, b);
        }
    }
    #[test]
    fn disabled_exchange_bypasses_and_resets_only_owned_fields() {
        let mut m = memory();
        let mut c = config();
        c.enabled = false;
        let r = c.exchange_channel(&mut m, &[0.25, -0.5], 0, 2).unwrap();
        assert_eq!(r.output, vec![0.25, -0.5]);
        for header in [0x1040, 0x3040] {
            for field in [0, 4, 12] {
                assert_eq!(m.load(header + field, 4).unwrap(), 0);
            }
        }
        assert_eq!(m.load(0x1048, 4).unwrap(), 0xa5a5a5a5);
        assert_eq!(m.load(0x1080, 4).unwrap(), 0xa5a5a5a5);
    }
    #[test]
    fn malformed_geometry_and_nonfinite_input_have_no_writes() {
        let mut m = memory();
        let before = m.regions[0].bytes.clone();
        for c in [
            Auxiliary {
                capacity: 0,
                ..config()
            },
            Auxiliary {
                return_info: 0x1080,
                ..config()
            },
            Auxiliary {
                send_info: u64::MAX - 3,
                ..config()
            },
        ] {
            assert!(c.exchange_channel(&mut m, &[0.0], 0, 1).is_err());
        }
        assert!(
            config()
                .exchange_channel(&mut m, &[f32::NAN], 0, 1)
                .is_err()
        );
        assert!(config().exchange_channel(&mut m, &[], 0, 1).is_err());
        assert!(
            config()
                .exchange_channel(&mut m, &[0.0], 0, u32::MAX)
                .is_err()
        );
        assert_eq!(m.regions[0].bytes, before);
    }
}

/// Stateful bounded reverb3d processing on normalized PCM.
pub mod reverb3d {
    /*
    MIT License

    Copyright (c) Ryujinx Team and Contributors
    Copyright (c) 2026 MacNativeGames contributors

    Permission is hereby granted, free of charge, to any person obtaining a copy of this software and associated documentation files (the "Software"), to deal in the Software without restriction, including without limitation the rights to use, copy, modify, merge, publish, distribute, sublicense, and/or sell copies of the Software, and to permit persons to whom the Software is furnished to do so, subject to the following conditions:

    The above copyright notice and this permission notice shall be included in all copies or substantial portions of the Software.

    THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF ANY KIND, EXPRESS OR IMPLIED, INCLUDING BUT NOT LIMITED TO THE WARRANTIES OF MERCHANTABILITY, FITNESS FOR A PARTICULAR PURPOSE AND NONINFRINGEMENT. IN NO EVENT SHALL THE AUTHORS OR COPYRIGHT HOLDERS BE LIABLE FOR ANY CLAIM, DAMAGES OR OTHER LIABILITY, WHETHER IN AN ACTION OF CONTRACT, TORT OR OTHERWISE, ARISING FROM, OUT OF OR IN CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER DEALINGS IN THE SOFTWARE.

    */
    // Rust adaptation: MacNativeGames 66b89f3 audio_reverb.c and Ryujinx.Audio
    // 4930dced Reverb3dState/Command/DelayLine3d. Mono, stereo and quad initially.
    use wl_guest_runtime::RuntimeResult;
    #[derive(Clone, Copy, Debug, PartialEq)]
    pub struct ReverbParams {
        pub channels: usize,
        pub rate: u32,
        pub room_hf: f32,
        pub hf_reference: f32,
        pub decay: f32,
        pub hf_decay: f32,
        pub room_gain: f32,
        pub reflections_gain: f32,
        pub reverb_gain: f32,
        pub diffusion: f32,
        pub reflection_delay: f32,
        pub reverb_delay: f32,
        pub density: f32,
        pub dry_gain: f32,
    }
    impl ReverbParams {
        pub fn validate(self) -> RuntimeResult<()> {
            let floats = [
                self.room_hf,
                self.hf_reference,
                self.decay,
                self.hf_decay,
                self.room_gain,
                self.reflections_gain,
                self.reverb_gain,
                self.diffusion,
                self.reflection_delay,
                self.reverb_delay,
                self.density,
                self.dry_gain,
            ];
            if ![1, 2, 4].contains(&self.channels)
                || ![32000, 48000].contains(&self.rate)
                || floats.iter().any(|v| !v.is_finite())
                || self.decay <= 0.0
                || self.hf_decay <= 0.0
                || !(1.0..self.rate as f32 / 2.0).contains(&self.hf_reference)
                || !(0.0..=100.0).contains(&self.density)
                || !(0.0..=100.0).contains(&self.diffusion)
                || self.reflection_delay < 0.0
                || self.reverb_delay < 0.0
                || self.reflection_delay + self.reverb_delay > 0.4
                || self.dry_gain.abs() > 64.0
            {
                return Err("unsupported reverb3d parameters".into());
            }
            Ok(())
        }
    }
    const FMIN: [f32; 4] = [5., 6., 13., 14.];
    const FMAX: [f32; 4] = [45.704, 82.782, 149.94, 271.58];
    const AMAX: [f32; 4] = [17., 13., 9., 7.];
    const BMAX: [f32; 4] = [19., 11., 10., 6.];
    const ETIME: [f32; 20] = [
        0.017136, 0.059154, 0.16173, 0.39019, 0.42526, 0.45541, 0.68974, 0.74591, 0.83384, 0.8595,
        0., 0.075024, 0.16879, 0.2999, 0.33744, 0.3719, 0.59901, 0.71674, 0.81786, 0.85166,
    ];
    const EGAIN: [f32; 20] = [
        0.67096, 0.61027, 1., 0.35680, 0.68361, 0.65978, 0.51939, 0.24712, 0.45945, 0.45021,
        0.64196, 0.54879, 0.92925, 0.38270, 0.72867, 0.69794, 0.5464, 0.24563, 0.45214, 0.44042,
    ];
    fn samples(rate: u32, ms: f32) -> usize {
        ((rate / 1000) as f32 * ms).round_ties_even() as usize
    }
    fn gain(x: f32) -> f32 {
        if x >= 0. {
            1.
        } else if x <= -5.3 {
            0.
        } else {
            10f32.powf(x)
        }
    }
    fn radians(v: f32) -> f32 {
        v * std::f32::consts::PI / 180.
    }
    #[derive(Clone, Debug)]
    struct Delay {
        data: Vec<f32>,
        cap: usize,
        delay: usize,
        read: usize,
        write: usize,
    }
    impl Delay {
        fn new(rate: u32, ms: f32) -> RuntimeResult<Self> {
            let cap = samples(rate, ms);
            let mut data = Vec::new();
            data.try_reserve_exact(cap + 1)
                .map_err(|_| "reverb delay allocation")?;
            data.resize(cap + 1, 0.);
            Ok(Self {
                data,
                cap,
                delay: cap,
                read: 0,
                write: cap,
            })
        }
        fn set(&mut self, count: usize) {
            self.delay = count;
            self.write = (self.read + count) % (self.cap + 1);
        }
        fn update(&mut self, value: f32) -> f32 {
            self.data[self.write] = value;
            let out = self.data[self.read];
            self.write += 1;
            self.read += 1;
            // The documented reference uses cap (not allocation length) for update wrap.
            if self.write >= self.cap {
                self.write = 0
            }
            if self.read >= self.cap {
                self.read = 0
            }
            out
        }
        fn tap(&self, n: usize) -> f32 {
            self.data[(self.write + self.cap - n) % (self.cap + 1)]
        }
        fn allpass(&mut self, x: f32, g: f32) -> f32 {
            let value = x - g * self.data[self.read];
            self.update(value) + g * value
        }
    }
    #[derive(Clone, Debug)]
    pub struct Reverb3d {
        params: ReverbParams,
        fdn: Vec<Delay>,
        a: Vec<Delay>,
        b: Vec<Delay>,
        pre: Delay,
        early: [usize; 20],
        reflection: usize,
        pre_previous: f32,
        pre_gain: f32,
        pre_target: f32,
        early_gain: f32,
        late_gain: f32,
        diffusion: f32,
        direct: [f32; 4],
        current: [f32; 4],
        feedback: [f32; 4],
        history: [f32; 4],
    }
    impl Reverb3d {
        pub fn new(params: ReverbParams) -> RuntimeResult<Self> {
            params.validate()?;
            let mut result = Self {
                params,
                fdn: FMAX
                    .iter()
                    .map(|v| Delay::new(params.rate, *v))
                    .collect::<RuntimeResult<_>>()?,
                a: AMAX
                    .iter()
                    .map(|v| Delay::new(params.rate, *v))
                    .collect::<RuntimeResult<_>>()?,
                b: BMAX
                    .iter()
                    .map(|v| Delay::new(params.rate, *v))
                    .collect::<RuntimeResult<_>>()?,
                pre: Delay::new(params.rate, 400.)?,
                early: [0; 20],
                reflection: 0,
                pre_previous: 0.,
                pre_gain: 0.,
                pre_target: 1.,
                early_gain: 0.,
                late_gain: 0.,
                diffusion: 0.,
                direct: [0.; 4],
                current: [0.; 4],
                feedback: [0.; 4],
                history: [0.; 4],
            };
            result.set_params(params)?;
            Ok(result)
        }
        pub fn set_params(&mut self, p: ReverbParams) -> RuntimeResult<()> {
            p.validate()?;
            if p.rate != self.params.rate || p.channels != self.params.channels {
                return Err("reverb geometry change requires new state".into());
            }
            let hf = gain(p.room_hf / 2000.);
            let pre_gain = if hf < 1. {
                let a = 1. - hf;
                let b = 2. - 2. * hf * radians(256. * p.hf_reference / p.rate as f32).cos();
                (b - (b * b - 4. * a * a).sqrt()) / (2. * a)
            } else {
                0.
            };
            let mut delays = [0; 4];
            let mut direct = [0.; 4];
            let mut current = [0.; 4];
            let mut feedback = [0.; 4];
            for i in 0..4 {
                delays[i] = samples(p.rate, FMIN[i] + p.density / 100. * (FMAX[i] - FMIN[i]));
                if delays[i] > self.fdn[i].cap {
                    return Err("reverb FDN delay exceeds allocation".into());
                }
                let n = (delays[i] + self.a[i].delay + self.b[i].delay) as f32;
                let a = -60. * n / (p.decay * p.rate as f32);
                let b = a / p.hf_decay;
                let angle = radians(128. * 0.5 * p.hf_reference / p.rate as f32);
                let c = angle.cos() / angle.sin();
                let d = gain((b - a) / 40.);
                let e = gain((b + a) / 40.) * (7071.0 / 10000.0);
                direct[i] = e * (d * c + 1.) / (c + d);
                current[i] = e * (1. - d * c) / (c + d);
                feedback[i] = (c - d) / (c + d);
            }
            if !pre_gain.is_finite()
                || direct
                    .iter()
                    .chain(&current)
                    .chain(&feedback)
                    .any(|v| !v.is_finite() || v.abs() > 2.)
            {
                return Err("unsupported nonfinite reverb coefficients".into());
            }
            let early = std::array::from_fn(|i| {
                samples(
                    p.rate,
                    p.reflection_delay * 1000.
                        + ETIME[i] * 1000. * (p.reverb_delay * 0.9998 + 0.02),
                )
                .min(self.pre.cap)
            });
            let reflection = samples(p.rate, 1000. * (p.reflection_delay + p.reverb_delay));
            if reflection > self.pre.cap {
                return Err("reverb reflection exceeds allocation".into());
            }
            self.params = p;
            self.direct = direct;
            self.current = current;
            self.feedback = feedback;
            self.history = [0.; 4];
            self.pre_previous = 0.;
            self.pre_gain = pre_gain;
            self.pre_target = 1. - pre_gain;
            self.early_gain = gain((p.room_gain + p.reflections_gain).min(5000.) / 2000.);
            self.late_gain = gain((p.room_gain + p.reverb_gain).min(5000.) / 2000.);
            self.diffusion = 0.6 * p.diffusion * 0.01;
            self.early = early;
            self.reflection = reflection;
            for (delay, count) in self.fdn.iter_mut().zip(delays) {
                delay.set(count)
            }
            Ok(())
        }
        pub fn process(&mut self, input: &[Vec<f32>]) -> RuntimeResult<Vec<Vec<f32>>> {
            let channels = self.params.channels;
            let frames = input.first().map_or(0, Vec::len);
            if input.len() != channels
                || frames > 1024
                || input.iter().any(|c| {
                    c.len() != frames || c.iter().any(|v| !v.is_finite() || v.abs() > 65536.)
                })
            {
                return Err("unsupported reverb input geometry/range".into());
            }
            let mut out = vec![vec![0.; frames]; channels];
            const STEREO: [usize; 20] =
                [0, 0, 0, 1, 1, 1, 1, 0, 0, 0, 1, 1, 1, 0, 0, 0, 0, 1, 1, 1];
            const QUAD: [usize; 20] = [0, 0, 0, 1, 1, 1, 1, 2, 2, 2, 1, 1, 1, 0, 0, 0, 0, 3, 3, 3];
            for k in 0..frames {
                let mut early = [0.; 4];
                let tap = self.pre.tap(self.reflection);
                for i in 0..20 {
                    let channel = if channels == 1 {
                        0
                    } else if channels == 2 {
                        STEREO[i]
                    } else {
                        QUAD[i]
                    };
                    early[channel] += self.pre.tap(self.early[i]) * EGAIN[i];
                }
                let sum = (0..channels).map(|c| input[c][k]).sum::<f32>();
                self.pre_previous = sum * self.pre_target + self.pre_previous * self.pre_gain;
                self.pre.update(self.pre_previous);
                let mut fb = [0.; 4];
                for (i, value) in fb.iter_mut().enumerate() {
                    let x = self.fdn[i].data[self.fdn[i].read];
                    *value = x * self.direct[i] + self.history[i];
                    self.history[i] = x * self.current[i] + *value * self.feedback[i];
                }
                let feedback = [fb[2] + fb[1], -fb[0] - fb[3], fb[0] - fb[3], fb[1] - fb[2]];
                let mut v = [0.; 4];
                for i in 0..4 {
                    let first =
                        self.a[i].allpass(tap * self.late_gain + feedback[i], self.diffusion);
                    v[i] = self.b[i].allpass(first, -0.9 * self.diffusion);
                    self.fdn[i].update(v[i]);
                }
                for c in 0..channels {
                    out[c][k] =
                        early[c] * self.early_gain + v[c] + input[c][k] * self.params.dry_gain;
                }
                if channels == 1 {
                    out[0][k] += v[1];
                }
            }
            Ok(out)
        }
    }
    #[cfg(test)]
    mod tests {
        use super::*;
        fn params(channels: usize) -> ReverbParams {
            ReverbParams {
                channels,
                rate: 48000,
                room_hf: -1200.,
                hf_reference: 4000.,
                decay: 1.3,
                hf_decay: 0.6,
                room_gain: -1000.,
                reflections_gain: -100.,
                reverb_gain: 50.,
                diffusion: 65.,
                reflection_delay: 0.005,
                reverb_delay: 0.012,
                density: 55.,
                dry_gain: 0.25,
            }
        }
        #[test]
        fn silence_is_a_fixed_point_for_each_supported_channel_count() {
            for channels in [1, 2, 4] {
                let mut r = Reverb3d::new(params(channels)).unwrap();
                for _ in 0..32 {
                    let out = r.process(&vec![vec![0.; 240]; channels]).unwrap();
                    assert!(out.iter().flatten().all(|v| *v == 0.));
                }
            }
        }
        #[test]
        fn dry_impulse_precedes_reflections_and_later_response_is_nonzero() {
            let mut r = Reverb3d::new(params(2)).unwrap();
            let mut input = vec![vec![0.; 1024]; 2];
            input[0][0] = 1.;
            let out = r.process(&input).unwrap();
            assert_eq!(out[0][0], 0.25);
            assert_eq!(out[1][0], 0.);
            assert!(out.iter().all(|c| c[1..128].iter().all(|v| *v == 0.)));
            assert!(out.iter().any(|c| c[240..].iter().any(|v| *v != 0.)));
            assert!(out.iter().flatten().all(|v| v.is_finite()));
        }
        #[test]
        fn chunking_does_not_reset_delay_state() {
            let mut whole = Reverb3d::new(params(4)).unwrap();
            let mut chunks = whole.clone();
            let input: Vec<Vec<f32>> = (0..4)
                .map(|c| {
                    (0..1024)
                        .map(|i| (((i + c * 3) % 31) - 15) as f32 * 0.0625)
                        .collect()
                })
                .collect();
            let expected = whole.process(&input).unwrap();
            let mut observed = vec![Vec::new(); 4];
            for i in 0..16 {
                let part: Vec<Vec<f32>> = input
                    .iter()
                    .map(|c| c[i * 64..(i + 1) * 64].to_vec())
                    .collect();
                for (dst, src) in observed.iter_mut().zip(chunks.process(&part).unwrap()) {
                    dst.extend(src);
                }
            }
            assert_eq!(expected, observed);
        }
        #[test]
        fn rejected_parameters_preserve_existing_filter_state() {
            let mut r = Reverb3d::new(params(2)).unwrap();
            let mut reference = r.clone();
            for change in 0..4 {
                let mut p = params(2);
                match change {
                    0 => p.density = 200.,
                    1 => p.hf_reference = 0.,
                    2 => p.room_gain = f32::NAN,
                    _ => p.rate = 32000,
                };
                assert!(r.set_params(p).is_err());
            }
            let input = vec![vec![0.5; 240], vec![-0.25; 240]];
            assert_eq!(
                r.process(&input).unwrap(),
                reference.process(&input).unwrap()
            );
        }
    }
}

/// Stateful bounded limiter processing on normalized PCM.
pub mod limiter {
    // Semantics independently implemented from Ryujinx.Audio 4930dced,
    // LimiterCommandVersion2, LimiterState and LimiterStatistics (MIT).
    use wl_guest_runtime::RuntimeResult;
    #[derive(Clone, Copy, Debug, PartialEq)]
    pub struct LimiterParams {
        pub channels: usize,
        pub delay_min: usize,
        pub delay_max: usize,
        pub attack: f32,
        pub release: f32,
        pub threshold: f32,
        pub input_gain: f32,
        pub output_gain: f32,
        pub statistics_enabled: bool,
    }
    impl LimiterParams {
        pub fn validate(self) -> RuntimeResult<()> {
            if ![1, 2, 4, 6].contains(&self.channels)
                || self.delay_min == 0
                || self.delay_min > self.delay_max
                || self.delay_max > 192_000
                || [
                    self.attack,
                    self.release,
                    self.threshold,
                    self.input_gain,
                    self.output_gain,
                ]
                .iter()
                .any(|v| !v.is_finite())
                || !(0.0..=1.0).contains(&self.attack)
                || !(0.0..=1.0).contains(&self.release)
                || !(0.0..=64.0).contains(&self.threshold)
                || !(0.0..=64.0).contains(&self.input_gain)
                || !(0.0..=64.0).contains(&self.output_gain)
            {
                return Err("unsupported limiter geometry or coefficient".into());
            }
            Ok(())
        }
    }
    #[derive(Clone, Debug, PartialEq)]
    pub struct LimiterStatistics {
        pub peak: [f32; 6],
        pub minimum_gain: [f32; 6],
    }
    impl Default for LimiterStatistics {
        fn default() -> Self {
            Self {
                peak: [0.; 6],
                minimum_gain: [1.; 6],
            }
        }
    }
    #[derive(Clone, Debug)]
    pub struct Limiter {
        params: LimiterParams,
        delay: Vec<f32>,
        position: [usize; 6],
        detector: [f32; 6],
        compression: [f32; 6],
        pub statistics: LimiterStatistics,
        reset_statistics: bool,
    }
    impl Limiter {
        pub fn new(params: LimiterParams) -> RuntimeResult<Self> {
            params.validate()?;
            Ok(Self {
                params,
                delay: vec![0.; params.channels * params.delay_max],
                position: [0; 6],
                detector: [0.; 6],
                compression: [1.; 6],
                statistics: LimiterStatistics::default(),
                reset_statistics: false,
            })
        }
        /// Preserve delay/filter history on coefficient updates. Geometry changes
        /// require a new effect; failure leaves the old state usable.
        pub fn set_params(
            &mut self,
            params: LimiterParams,
            reset_statistics: bool,
        ) -> RuntimeResult<()> {
            params.validate()?;
            if params.channels != self.params.channels || params.delay_max != self.params.delay_max
            {
                return Err("limiter geometry change requires new state".into());
            }
            self.params = params;
            // A reset belongs to a command generation, not to every future frame.
            self.reset_statistics |= reset_statistics;
            Ok(())
        }
        pub fn process(
            &mut self,
            input: &[Vec<f32>],
            enabled: bool,
        ) -> RuntimeResult<Vec<Vec<f32>>> {
            let p = self.params;
            let samples = input.first().map_or(0, Vec::len);
            if input.len() != p.channels
                || samples == 0
                || samples > 1024
                || input.iter().any(|c| {
                    c.len() != samples || c.iter().any(|v| !v.is_finite() || v.abs() > 65536.)
                })
            {
                return Err("unsupported limiter input".into());
            }
            // Disabled commands bypass and do not consume the delay or reset flag.
            if !enabled {
                return Ok(input.to_vec());
            }
            if self.reset_statistics {
                self.statistics = LimiterStatistics::default();
                self.reset_statistics = false;
            }
            let mut output = vec![vec![0.; samples]; p.channels];
            for c in 0..p.channels {
                for k in 0..samples {
                    let x = (input[c][k] * (32768. / 32767.)) * p.input_gain;
                    let level = x.abs();
                    let coefficient = if level > self.detector[c] {
                        p.attack
                    } else {
                        p.release
                    };
                    self.detector[c] += coefficient * (level - self.detector[c]);
                    let attenuation = if self.detector[c] > p.threshold {
                        p.threshold / self.detector[c]
                    } else {
                        1.
                    };
                    let coefficient = if self.compression[c] > attenuation {
                        p.attack
                    } else {
                        p.release
                    };
                    self.compression[c] += coefficient * (attenuation - self.compression[c]);
                    let slot = c * p.delay_max + self.position[c];
                    output[c][k] =
                        self.delay[slot] * self.compression[c] * p.output_gain * (32767. / 32768.);
                    self.delay[slot] = x;
                    self.position[c] += 1;
                    // New delay_min may be smaller than the previous cursor.
                    self.position[c] %= p.delay_min;
                    if p.statistics_enabled {
                        self.statistics.peak[c] = self.statistics.peak[c].max(level);
                        self.statistics.minimum_gain[c] =
                            self.statistics.minimum_gain[c].min(self.compression[c]);
                    }
                }
            }
            Ok(output)
        }
    }
    #[cfg(test)]
    mod tests {
        use super::*;
        fn params() -> LimiterParams {
            LimiterParams {
                channels: 1,
                delay_min: 2,
                delay_max: 9,
                attack: 1.,
                release: 1.,
                threshold: 0.5,
                input_gain: 1.,
                output_gain: 1.,
                statistics_enabled: true,
            }
        }
        #[test]
        fn lookahead_and_threshold_have_independent_expected_values() {
            let mut r = Limiter::new(params()).unwrap();
            let y = r.process(&[vec![1.; 6]], true).unwrap();
            assert_eq!(&y[0][..2], &[0., 0.]);
            let expected = 0.5 * (32767. / 32768.);
            for v in &y[0][2..] {
                assert!((v - expected).abs() < 1e-6);
            }
            assert_eq!(r.statistics.peak[1..], [0.; 5]);
            assert_eq!(r.statistics.minimum_gain[1..], [1.; 5]);
            assert!((r.statistics.minimum_gain[0] - expected).abs() < 1e-6);
        }
        #[test]
        fn disabled_bypass_keeps_pending_delay_and_reset_is_once() {
            let mut r = Limiter::new(params()).unwrap();
            r.process(&[vec![1.; 4]], true).unwrap();
            r.set_params(params(), true).unwrap();
            assert_eq!(r.process(&[vec![0.25; 3]], false).unwrap()[0], [0.25; 3]);
            let y = r.process(&[vec![0.; 2]], true).unwrap();
            assert!(y[0].iter().all(|v| *v > 0.99));
            assert_eq!(r.statistics.peak, [0.; 6]);
            r.process(&[vec![0.25]], true).unwrap();
            let peak = r.statistics.peak;
            r.process(&[vec![0.]], true).unwrap();
            assert_eq!(r.statistics.peak, peak);
        }
        #[test]
        fn chunking_and_invalid_update_preserve_state() {
            let p = LimiterParams {
                channels: 6,
                attack: 0.2,
                release: 0.03,
                ..params()
            };
            let mut a = Limiter::new(p).unwrap();
            let mut b = Limiter::new(p).unwrap();
            let input: Vec<Vec<f32>> = (0..6)
                .map(|c| (0..240).map(|k| ((k + c) % 13) as f32 / 16.).collect())
                .collect();
            let all = a.process(&input, true).unwrap();
            let left: Vec<_> = input.iter().map(|c| c[..73].to_vec()).collect();
            let right: Vec<_> = input.iter().map(|c| c[73..].to_vec()).collect();
            let l = b.process(&left, true).unwrap();
            assert!(
                b.set_params(
                    LimiterParams {
                        attack: f32::NAN,
                        ..p
                    },
                    true
                )
                .is_err()
            );
            assert!(b.process(&[vec![f32::NAN]], true).is_err());
            let r = b.process(&right, true).unwrap();
            for c in 0..6 {
                assert_eq!(all[c], [l[c].as_slice(), r[c].as_slice()].concat());
            }
            assert_eq!(a.statistics, b.statistics);
        }
    }
}

pub mod voice {
    //! Checked source primitives for the next queued-voice integration.
    //! Semantics: Ryujinx.Audio 4930dced AdpcmHelper, ResamplerHelper and
    //! BiquadFilterHelper; no borrowed guest pointers or commercial samples.
    use wl_guest_runtime::RuntimeResult;
    #[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
    pub struct AdpcmContext {
        pub predictor_scale: u16,
        pub history: [i16; 2],
    }
    impl AdpcmContext {
        pub fn from_le_bytes(bytes: &[u8]) -> RuntimeResult<Self> {
            if bytes.len() != 6 {
                return Err("ADPCM context must be six bytes".into());
            }
            Ok(Self {
                predictor_scale: u16::from_le_bytes(bytes[..2].try_into().unwrap()),
                history: [
                    i16::from_le_bytes(bytes[2..4].try_into().unwrap()),
                    i16::from_le_bytes(bytes[4..6].try_into().unwrap()),
                ],
            })
        }
    }
    /// Start denotes a sample offset in the supplied encoded stream. For a partial
    /// frame, context must contain the predictor and histories immediately before
    /// that sample (e.g. the guest's explicit loop context); no prefix is invented.
    pub fn decode_adpcm(
        encoded: &[u8],
        start: usize,
        count: usize,
        coefficients: &[i16; 16],
        context: &mut AdpcmContext,
    ) -> RuntimeResult<Vec<i16>> {
        if count > 16384 {
            return Err("ADPCM command sample budget".into());
        }
        if count == 0 {
            return Ok(Vec::new());
        }
        let end = start
            .checked_add(count)
            .ok_or("ADPCM sample range overflow")?;
        let last = end - 1;
        let bytes = (last / 14)
            .checked_mul(8)
            .and_then(|v| v.checked_add(2 + (last % 14) / 2))
            .ok_or("ADPCM byte range overflow")?;
        if bytes > encoded.len() {
            return Err("truncated ADPCM data".into());
        }
        let mut next = *context;
        let mut output = Vec::with_capacity(count);
        for i in start..end {
            let frame = i / 14 * 8;
            let nibble = i % 14;
            if nibble == 0 {
                next.predictor_scale = u16::from(encoded[frame]);
            }
            let predictor = (next.predictor_scale >> 4) as usize;
            if next.predictor_scale > 255 || predictor >= 8 {
                return Err("unsupported ADPCM predictor".into());
            }
            let scale = 1i64 << (next.predictor_scale & 15);
            let raw = (encoded[frame + 1 + nibble / 2] >> if nibble % 2 == 0 { 4 } else { 0 }) & 15;
            let signed = if raw >= 8 {
                i64::from(raw) - 16
            } else {
                i64::from(raw)
            };
            let accum = signed * scale * 2048
                + i64::from(coefficients[predictor * 2]) * i64::from(next.history[0])
                + i64::from(coefficients[predictor * 2 + 1]) * i64::from(next.history[1])
                + 1024;
            // The pinned DSP implementation uses a 32-bit accumulator. Reject the
            // unverified overflow form rather than making host overflow a behavior.
            if i32::try_from(accum).is_err() {
                return Err("unsupported ADPCM accumulator overflow".into());
            }
            let sample = (accum >> 11).clamp(i64::from(i16::MIN), i64::from(i16::MAX)) as i16;
            next.history = [sample, next.history[0]];
            output.push(sample);
        }
        *context = next;
        Ok(output)
    }
    /// Default-quality SRC, exactly unit ratio and integral phase. The four-sample
    /// history and pinned Q15 phase-zero filter are required even at 48 -> 48 kHz;
    /// this is not a linear interpolator or a zero-latency passthrough.
    #[derive(Clone, Debug, Default)]
    pub struct UnitResampler {
        history: [i16; 4],
    }
    impl UnitResampler {
        pub fn process(&mut self, samples: &[i16]) -> RuntimeResult<Vec<f32>> {
            if samples.len() > 1024 {
                return Err("unit SRC frame budget".into());
            }
            let mut source = self.history.to_vec();
            source.extend_from_slice(samples);
            let mut out = Vec::with_capacity(samples.len());
            for x in source.windows(4).take(samples.len()) {
                let a = x[0] as f32 * (-68. / 32768.);
                let b = x[1] as f32 * (32639. / 32768.);
                let c = x[2] as f32 * (69. / 32768.);
                let d = x[3] as f32 * (-5. / 32768.);
                out.push(((a + b) + (c + d)).round_ties_even() / 32768.);
            }
            self.history.copy_from_slice(&source[source.len() - 4..]);
            Ok(out)
        }
    }
    #[derive(Clone, Copy, Debug, Default)]
    pub struct Biquad {
        history: [f32; 2],
    }
    impl Biquad {
        pub fn process(
            &mut self,
            samples: &[f32],
            numerator: [i16; 3],
            denominator: [i16; 2],
        ) -> RuntimeResult<Vec<f32>> {
            if samples.len() > 1024 || samples.iter().any(|v| !v.is_finite()) {
                return Err("unsupported biquad input".into());
            }
            let a = numerator.map(|v| v as f32 / 16384.);
            let b = denominator.map(|v| v as f32 / 16384.);
            let mut history = self.history;
            let mut output = Vec::with_capacity(samples.len());
            for &input in samples {
                let value = input * a[0] + history[0];
                history = [
                    input * a[1] + value * b[0] + history[1],
                    input * a[2] + value * b[1],
                ];
                if !value.is_finite() || history.iter().any(|v| !v.is_finite()) {
                    return Err("nonfinite biquad result".into());
                }
                output.push(value);
            }
            self.history = history;
            Ok(output)
        }
    }
    #[cfg(test)]
    mod tests {
        use super::*;
        #[test]
        fn adpcm_nibble_order_sign_scale_rounding_and_saturation() {
            let encoded = [0x02, 0x17, 0x8f, 0x24, 0x60, 0xad, 0x35, 0xcb];
            let mut c = AdpcmContext::default();
            assert_eq!(
                decode_adpcm(&encoded, 0, 14, &[0; 16], &mut c).unwrap(),
                vec![4, 28, -32, -4, 8, 16, 24, 0, -24, -12, 12, 20, -16, -20]
            );
            let encoded = [0x0f, 0x78, 0x78, 0x78, 0x78, 0x78, 0x78, 0x78];
            assert_eq!(
                decode_adpcm(&encoded, 0, 14, &[0; 16], &mut c).unwrap(),
                vec![
                    32767, -32768, 32767, -32768, 32767, -32768, 32767, -32768, 32767, -32768,
                    32767, -32768, 32767, -32768
                ]
            );
            let mut coef = [0; 16];
            coef[0] = 1024;
            let mut c = AdpcmContext {
                predictor_scale: 0,
                history: [1, 0],
            };
            assert_eq!(decode_adpcm(&[0; 8], 0, 1, &coef, &mut c).unwrap(), [1]); // +0.5 -> +1
            let mut c = AdpcmContext {
                predictor_scale: 0,
                history: [-1, 0],
            };
            assert_eq!(decode_adpcm(&[0; 8], 0, 1, &coef, &mut c).unwrap(), [0]); // -0.5 -> 0
        }
        #[test]
        fn every_split_matches_the_same_stateful_encoded_stream() {
            let data: Vec<u8> = (0..32)
                .flat_map(|i| [0x12, (i * 7) as u8, 0x17, 0x8f, 0x24, 0x60, 0xad, 0x35])
                .collect();
            let mut coefficients = [0; 16];
            coefficients[2] = 2048;
            coefficients[3] = -512;
            let mut state = AdpcmContext::default();
            let expected = decode_adpcm(&data, 0, 448, &coefficients, &mut state).unwrap();
            for split in 0..=448 {
                let mut c = AdpcmContext::default();
                let mut output = decode_adpcm(&data, 0, split, &coefficients, &mut c).unwrap();
                output.extend(
                    decode_adpcm(&data, split, 448 - split, &coefficients, &mut c).unwrap(),
                );
                assert_eq!(output, expected);
                assert_eq!(c, state);
            }
        }
        #[test]
        fn malformed_decode_leaves_context_unmodified() {
            let initial = AdpcmContext {
                predictor_scale: 0,
                history: [100, 200],
            };
            for (data, start, count) in [
                (vec![0x80; 8], 0, 14),
                (vec![0; 7], 0, 14),
                (vec![0; 8], usize::MAX, 1),
                (vec![0; 8], 0, 16385),
            ] {
                let mut c = initial;
                assert!(decode_adpcm(&data, start, count, &[0; 16], &mut c).is_err());
                assert_eq!(c, initial);
            }
            let mut c = AdpcmContext {
                predictor_scale: 0xff00,
                history: [100, 200],
            };
            let before = c;
            assert!(decode_adpcm(&[0; 8], 1, 1, &[0; 16], &mut c).is_err());
            assert_eq!(c, before);
            assert_eq!(
                AdpcmContext::from_le_bytes(&[0x34, 0, 0xff, 0xff, 0, 0x80]).unwrap(),
                AdpcmContext {
                    predictor_scale: 0x34,
                    history: [-1, -32768]
                }
            );
            assert!(AdpcmContext::from_le_bytes(&[0; 5]).is_err());
        }
        #[test]
        fn default_src_has_known_phase_zero_impulse_and_chunk_continuity() {
            let samples = [32767, 0, 0, 0, 0, 0, 0, 0];
            let mut r = UnitResampler::default();
            let y = r.process(&samples).unwrap();
            assert_eq!(
                y.iter().map(|v| (v * 32768.) as i32).collect::<Vec<_>>(),
                [0, -5, 69, 32638, -68, 0, 0, 0]
            );
            let mut r = UnitResampler::default();
            let a = r.process(&samples[..3]).unwrap();
            let b = r.process(&samples[3..]).unwrap();
            assert_eq!([a, b].concat(), y);
        }
        #[test]
        fn q14_filter_has_analytic_response_and_failed_input_preserves_delay() {
            let mut b = Biquad::default();
            let a = [16384, 8192, 0];
            let d = [8192, 0];
            let y = b.process(&[1., 0., 0., 0.], a, d).unwrap();
            assert_eq!(y, [1., 1., 0.5, 0.25]);
            assert!(b.process(&[f32::NAN], a, d).is_err());
            assert_eq!(b.process(&[0.], a, d).unwrap(), [0.125]);
        }
    }

    #[derive(Clone, Copy, Debug)]
    pub struct Wave {
        pub address: u64,
        pub bytes: u64,
        pub start: u32,
        pub end: u32,
        pub context: Option<u64>,
    }
    #[derive(Clone, Debug, Default)]
    pub struct AdpcmQueue {
        buffers: [Option<Wave>; 4],
        index: usize,
        offset: u32,
        entered: bool,
        context: AdpcmContext,
        src: UnitResampler,
        pub played: u64,
        pub consumed: u32,
    }
    impl AdpcmQueue {
        /// This bounded first subset handles nonlooping/non-EOS mono buffers. The
        /// wire parser must reject other forms before calling this operation.
        pub fn push(&mut self, slot: usize, wave: Wave) -> RuntimeResult<()> {
            let last = wave
                .end
                .checked_sub(1)
                .filter(|_| wave.start < wave.end)
                .ok_or("empty/reversed ADPCM wave")?;
            let required = (u64::from(last) / 14) * 8 + 2 + u64::from(last % 14) / 2;
            if slot >= 4
                || self.buffers[slot].is_some()
                || wave.address == 0
                || required > wave.bytes
                || wave.address.checked_add(wave.bytes).is_none()
                || wave.start % 14 != 0 && wave.context.is_none()
            {
                return Err("unsupported ADPCM wave descriptor/slot".into());
            }
            self.buffers[slot] = Some(wave);
            Ok(())
        }
        pub fn render(
            &mut self,
            memory: &wl_guest_runtime::Memory,
            coefficients_address: u64,
            samples: usize,
        ) -> RuntimeResult<Vec<f32>> {
            if samples == 0 || samples > 1024 {
                return Err("ADPCM queue frame budget".into());
            }
            let mut coefficient_bytes = [0; 32];
            if self.buffers.iter().any(Option::is_some) {
                memory.read_into(coefficients_address, &mut coefficient_bytes)?;
            }
            let coefficients = std::array::from_fn(|i| {
                i16::from_le_bytes(coefficient_bytes[i * 2..i * 2 + 2].try_into().unwrap())
            });
            let mut next = self.clone();
            let mut decoded = Vec::with_capacity(samples);
            while decoded.len() < samples {
                let Some(wave) = next.buffers[next.index] else {
                    break;
                };
                if !next.entered {
                    if let Some(at) = wave.context {
                        let mut raw = [0; 6];
                        memory.read_into(at, &mut raw)?;
                        next.context = AdpcmContext::from_le_bytes(&raw)?;
                    }
                    next.entered = true;
                }
                let start = wave
                    .start
                    .checked_add(next.offset)
                    .ok_or("ADPCM queue cursor overflow")?;
                let count = (wave.end - start).min((samples - decoded.len()) as u32);
                let first_frame = u64::from(start) / 14;
                let last_frame = u64::from(start + count - 1) / 14;
                let offset = first_frame * 8;
                let byte_count = (last_frame - first_frame + 1) * 8;
                // Permit the exact partial final encoded frame; never demand bytes
                // outside the wire-provided span just for an unused low nibble.
                let byte_count = byte_count.min(
                    wave.bytes
                        .checked_sub(offset)
                        .ok_or("ADPCM queue backing range")?,
                );
                let mut bytes = vec![0; byte_count as usize];
                memory.read_into(
                    wave.address
                        .checked_add(offset)
                        .ok_or("ADPCM queue address overflow")?,
                    &mut bytes,
                )?;
                decoded.extend(decode_adpcm(
                    &bytes,
                    (start % 14) as usize,
                    count as usize,
                    &coefficients,
                    &mut next.context,
                )?);
                next.offset += count;
                next.played = next
                    .played
                    .checked_add(u64::from(count))
                    .ok_or("ADPCM played count overflow")?;
                if next.offset == wave.end - wave.start {
                    next.buffers[next.index] = None;
                    next.index = (next.index + 1) % 4;
                    next.offset = 0;
                    next.entered = false;
                    next.consumed = next
                        .consumed
                        .checked_add(1)
                        .ok_or("ADPCM consumed count overflow")?;
                }
            }
            decoded.resize(samples, 0);
            let output = next.src.process(&decoded)?;
            *self = next;
            Ok(output)
        }
    }
    #[cfg(test)]
    mod queue_tests {
        use super::*;
        use wl_guest_runtime::Memory;
        fn memory() -> Memory {
            let mut m = Memory::default();
            let mut bytes = vec![0; 4096];
            for i in 0..4 {
                bytes[i * 8 + 1..i * 8 + 8].fill(0x11);
            }
            m.map(0x1000, bytes, true, true, false).unwrap();
            m
        }
        fn wave(start: u32, end: u32) -> Wave {
            Wave {
                address: 0x1000,
                bytes: 32,
                start,
                end,
                context: None,
            }
        }
        #[test]
        fn four_buffer_order_starvation_and_requeue_preserve_actual_counts() {
            let m = memory();
            let mut q = AdpcmQueue::default();
            for i in 0..4 {
                q.push(i, wave(0, 14)).unwrap();
            }
            let output = q.render(&m, 0x1100, 64).unwrap();
            assert_eq!(q.played, 56);
            assert_eq!(q.consumed, 4);
            assert!(output[4..56].iter().all(|s| *s > 0.));
            assert!(output[60..].iter().all(|s| *s == 0.));
            q.push(0, wave(0, 14)).unwrap();
            q.render(&m, 0x1100, 64).unwrap();
            assert_eq!(q.played, 70);
            assert_eq!(q.consumed, 5);
        }
        #[test]
        fn second_buffer_fetch_failure_does_not_consume_first() {
            let mut m = memory();
            let mut q = AdpcmQueue::default();
            q.push(0, wave(0, 14)).unwrap();
            q.push(
                1,
                Wave {
                    address: 0x3000,
                    ..wave(0, 14)
                },
            )
            .unwrap();
            assert!(q.render(&m, 0x1100, 64).is_err());
            assert_eq!((q.played, q.consumed), (0, 0));
            let mut bytes = vec![0x11; 4096];
            bytes[0] = 0;
            m.map(0x3000, bytes, true, true, false).unwrap();
            q.render(&m, 0x1100, 64).unwrap();
            assert_eq!((q.played, q.consumed), (28, 2));
        }
        #[test]
        fn partial_start_uses_supplied_loop_context_and_checked_little_endian_coefficients() {
            let mut m = memory();
            let mut q = AdpcmQueue::default();
            assert!(q.push(0, wave(5, 14)).is_err());
            m.write_bytes(0x1120, &[0, 0, 1, 0, 0, 0]).unwrap();
            m.write_bytes(0x1100, &2048i16.to_le_bytes()).unwrap();
            q.push(
                0,
                Wave {
                    context: Some(0x1120),
                    ..wave(5, 14)
                },
            )
            .unwrap();
            let out = q.render(&m, 0x1100, 16).unwrap();
            assert_eq!((q.played, q.consumed), (9, 1));
            assert!(out.iter().any(|v| *v > 1. / 32768.));
        }
    }
    #[cfg(test)]
    mod stress_tests {
        use super::*;
        use wl_guest_runtime::Memory;
        #[test]
        fn retail_sized_synthetic_wave_geometry_survives_thousands_of_ticks_without_recounting() {
            let mut memory = Memory::default();
            let mut bytes = vec![0x11; 131072];
            for frame in bytes.chunks_exact_mut(8) {
                frame[0] = 0;
            }
            memory.map(0x1000, bytes, true, true, false).unwrap();
            memory
                .map(0x300000, vec![0; 4096], true, true, false)
                .unwrap();
            let mut q = AdpcmQueue::default();
            for (slot, start, end) in [
                (0, 0, 229376),
                (1, 0, 59060),
                (2, 21556, 229376),
                (3, 0, 59060),
            ] {
                q.push(
                    slot,
                    Wave {
                        address: 0x1000,
                        bytes: 131072,
                        start,
                        end,
                        context: if start == 0 { None } else { Some(0x300100) },
                    },
                )
                .unwrap();
            }
            let expected = 229376 + 59060 + (229376 - 21556) + 59060;
            for _ in 0..2400 {
                let output = q.render(&memory, 0x300000, 240).unwrap();
                assert_eq!(output.len(), 240);
                assert!(output.iter().all(|s| s.is_finite()));
            }
            assert_eq!(q.played, expected);
            assert_eq!(q.consumed, 4);
            let before = (q.played, q.consumed);
            q.render(&memory, 0x300000, 240).unwrap();
            assert_eq!((q.played, q.consumed), before);
        }
        #[test]
        fn overflowed_accumulator_is_explicitly_rejected_without_changing_context() {
            let mut c = AdpcmContext {
                predictor_scale: 0x0f,
                history: [32767, 32767],
            };
            let before = c;
            assert!(decode_adpcm(&[0x0f, 0x77], 1, 1, &[32767; 16], &mut c).is_err());
            assert_eq!(c, before);
        }
    }

    #[derive(Clone, Copy, Debug)]
    pub struct VoiceFilter {
        pub enabled: bool,
        pub numerator: [i16; 3],
        pub denominator: [i16; 2],
    }
    #[derive(Clone, Debug)]
    pub struct MonoVoice {
        pub id: u32,
        pub priority: u32,
        pub order: u32,
        pub splitter: u32,
        pub playing: bool,
        pub gain: f32,
        previous_gain: f32,
        coefficients: u64,
        filters: [VoiceFilter; 2],
        filter_state: [Biquad; 2],
        pub queue: AdpcmQueue,
    }
    impl MonoVoice {
        pub fn prepare(old: Option<&Self>, bytes: &[u8], rate: u32) -> RuntimeResult<Self> {
            if bytes.len() != 0x170 {
                return Err("mono voice wire revision/length".into());
            }
            let word = |i: usize| u32::from_le_bytes(bytes[i..i + 4].try_into().unwrap());
            let wide = |i: usize| u64::from_le_bytes(bytes[i..i + 8].try_into().unwrap());
            let short = |i: usize| i16::from_le_bytes(bytes[i..i + 2].try_into().unwrap());
            let new = bytes[8];
            let used = bytes[9];
            let state = bytes[10];
            let gain = f32::from_bits(word(32));
            if new > 1
                || used != 1
                || ![0, 2].contains(&state)
                || bytes[11] != 6
                || word(12) != rate
                || word(24) != 1
                || f32::from_bits(word(28)) != 1.
                || !gain.is_finite()
                || gain.abs() > 64.
                || word(60) > 4
                || word(64) >= 4
                || wide(72) == 0
                || wide(80) != 32
                || word(88) != 0x7fffffff
                || word(92) == 0x7fffffff
                || bytes[344..351].iter().any(|v| *v != 0)
            {
                return Err("unsupported queued mono ADPCM voice mode".into());
            }
            let filters = std::array::from_fn(|i| {
                let at = 36 + i * 12;
                VoiceFilter {
                    enabled: bytes[at] != 0,
                    numerator: std::array::from_fn(|n| short(at + 2 + n * 2)),
                    denominator: std::array::from_fn(|n| short(at + 8 + n * 2)),
                }
            });
            if bytes[36] > 1 || bytes[48] > 1 {
                return Err("invalid voice biquad enable".into());
            }
            let mut voice = if new == 0 { old.cloned() } else { None }.unwrap_or(Self {
                id: word(0),
                priority: 0,
                order: 0,
                splitter: 0,
                playing: false,
                gain,
                previous_gain: gain,
                coefficients: wide(72),
                filters,
                filter_state: [Biquad::default(); 2],
                queue: AdpcmQueue::default(),
            });
            if voice.id != word(0) {
                return Err("voice id changed without new state".into());
            }
            // Nonzero initial queue index needs the server's matching persistent
            // channel state, which this first subset has not established.
            if (new != 0 || old.is_none()) && word(64) != 0 {
                return Err("unsupported nonzero initial voice queue index".into());
            }
            for i in 0..4 {
                let at = 96 + i * 56;
                let sent = bytes[at + 26];
                if bytes[at + 24] > 1 || bytes[at + 25] > 1 || sent > 1 {
                    return Err("invalid voice wave flags".into());
                }
                if sent != 0 {
                    continue;
                }
                if wide(at) == 0 && wide(at + 8) == 0 {
                    continue;
                }
                if bytes[at + 24] != 0 || bytes[at + 25] != 0 {
                    return Err("loop/EOS voice queue unsupported".into());
                }
                let context = match (wide(at + 32), wide(at + 40)) {
                    (0, 0) => None,
                    (at, 6) if at != 0 => Some(at),
                    _ => return Err("invalid ADPCM context range".into()),
                };
                voice.queue.push(
                    i,
                    Wave {
                        address: wide(at),
                        bytes: wide(at + 8),
                        start: word(at + 16),
                        end: word(at + 20),
                        context,
                    },
                )?;
            }
            voice.priority = word(16);
            voice.order = word(20);
            voice.splitter = word(92);
            voice.playing = state == 0;
            voice.gain = gain;
            voice.coefficients = wide(72);
            voice.filters = filters;
            for (i, filter) in filters.iter().enumerate() {
                if !filter.enabled {
                    voice.filter_state[i] = Biquad::default();
                }
            }
            Ok(voice)
        }
        pub fn render(
            &mut self,
            memory: &wl_guest_runtime::Memory,
            samples: usize,
        ) -> RuntimeResult<Vec<f32>> {
            if samples == 0 || samples > 1024 {
                return Err("mono voice frame budget".into());
            }
            if !self.playing {
                return Ok(vec![0.; samples]);
            }
            let mut next = self.clone();
            let mut data = next.queue.render(memory, next.coefficients, samples)?;
            for (i, filter) in next.filters.iter().enumerate() {
                if filter.enabled {
                    data = next.filter_state[i].process(
                        &data,
                        filter.numerator,
                        filter.denominator,
                    )?;
                }
            }
            let mut gain = next.previous_gain;
            let increment = (next.gain - gain) / samples as f32;
            for v in &mut data {
                *v = (*v * 32768. * gain).round_ties_even() / 32768.;
                gain += increment;
                if !v.is_finite() {
                    return Err("nonfinite voice gain output".into());
                }
            }
            next.previous_gain = next.gain;
            *self = next;
            Ok(data)
        }
    }
    #[cfg(test)]
    mod wire_tests {
        use super::*;
        use wl_guest_runtime::Memory;
        fn input() -> Vec<u8> {
            let mut b = vec![0; 368];
            b[8] = 1;
            b[9] = 1;
            b[11] = 6;
            for (at, value) in [
                (12, 48000u32),
                (24, 1),
                (28, 1f32.to_bits()),
                (32, 1f32.to_bits()),
                (60, 1),
                (88, 0x7fffffff),
                (92, 2),
            ] {
                b[at..at + 4].copy_from_slice(&value.to_le_bytes());
            }
            b[72..80].copy_from_slice(&0x2000u64.to_le_bytes());
            b[80..88].copy_from_slice(&32u64.to_le_bytes());
            for i in 0..4 {
                b[96 + i * 56 + 26] = 1;
            }
            b[96..104].copy_from_slice(&0x1000u64.to_le_bytes());
            b[104..112].copy_from_slice(&8u64.to_le_bytes());
            b[116..120].copy_from_slice(&14u32.to_le_bytes());
            b[122] = 0;
            b
        }
        #[test]
        fn wire_preparation_and_real_sample_path_keep_counts_when_paused() {
            let input = input();
            let mut memory = Memory::default();
            memory
                .map(0x1000, vec![0x11; 4096], true, true, false)
                .unwrap();
            memory.write_bytes(0x1000, &[0]).unwrap();
            memory
                .map(0x2000, vec![0; 4096], true, true, false)
                .unwrap();
            let mut v = MonoVoice::prepare(None, &input, 48000).unwrap();
            let pcm = v.render(&memory, 8).unwrap();
            assert!(pcm.iter().any(|s| *s > 0.));
            assert_eq!(v.queue.played, 8);
            let mut next = input.clone();
            next[8] = 0;
            next[10] = 2;
            next[122] = 1;
            let mut paused = MonoVoice::prepare(Some(&v), &next, 48000).unwrap();
            assert_eq!(paused.render(&memory, 8).unwrap(), vec![0.; 8]);
            assert_eq!(paused.queue.played, 8);
            next[10] = 0;
            let mut resumed = MonoVoice::prepare(Some(&paused), &next, 48000).unwrap();
            resumed.render(&memory, 8).unwrap();
            assert_eq!((resumed.queue.played, resumed.queue.consumed), (14, 1));
        }
        #[test]
        fn unsupported_mode_or_failed_destination_does_not_modify_old_queue() {
            let v = MonoVoice::prepare(None, &input(), 48000).unwrap();
            for (at, value) in [(11, 2u8), (350, 1), (120, 1)] {
                let mut b = input();
                b[at] = value;
                assert!(MonoVoice::prepare(Some(&v), &b, 48000).is_err());
            }
            assert_eq!(v.queue.played, 0);
            let mut v = v;
            assert!(v.render(&Memory::default(), 240).is_err());
            assert_eq!(v.queue.played, 0);
        }
    }

    impl MonoVoice {
        pub fn backing_ranges(&self) -> Vec<(u64, u64)> {
            let mut result = Vec::new();
            for wave in self.queue.buffers.iter().flatten() {
                result.push((wave.address, wave.bytes));
                if let Some(at) = wave.context {
                    result.push((at, 6));
                }
            }
            if !result.is_empty() {
                result.push((self.coefficients, 32));
            }
            result
        }
        pub fn has_queued_buffers(&self) -> bool {
            self.queue.buffers.iter().any(Option::is_some)
        }
    }
}

pub mod routing {
    //! Persistent bounded REV11 unfiltered splitter updates. Client fields follow
    //! libnx feebd026 and Ryujinx.Audio 4930dced SplitterState/SplitterDestinationData.
    use wl_guest_runtime::RuntimeResult;
    fn word(b: &[u8], i: usize) -> u32 {
        u32::from_le_bytes(b[i..i + 4].try_into().unwrap())
    }
    fn take<'a>(b: &'a [u8], at: &mut usize, n: usize) -> RuntimeResult<&'a [u8]> {
        let end = at
            .checked_add(n)
            .filter(|e| *e <= b.len())
            .ok_or("splitter truncated record")?;
        let r = &b[*at..end];
        *at = end;
        Ok(r)
    }
    #[derive(Clone, Debug, PartialEq)]
    pub struct Splitter {
        pub rate: u32,
        pub destinations: Vec<usize>,
    }
    #[derive(Clone, Debug, PartialEq)]
    pub struct Destination {
        pub active: bool,
        pub mix: usize,
        pub volume: [f32; 24],
        previous_volume: [f32; 24],
        used_this_frame: bool,
    }
    #[derive(Clone, Debug, PartialEq)]
    pub struct Routing {
        pub splitters: Vec<Option<Splitter>>,
        pub destinations: Vec<Option<Destination>>,
    }
    impl Routing {
        pub fn new(splitters: usize, destinations: usize) -> RuntimeResult<Self> {
            if splitters > 1024 || destinations > 8192 {
                return Err("splitter state budget".into());
            }
            Ok(Self {
                splitters: vec![None; splitters],
                destinations: vec![None; destinations],
            })
        }
        pub fn update(&self, bytes: &[u8], mix_count: usize) -> RuntimeResult<Self> {
            if bytes.len() < 32 || bytes.len() % 16 != 0 || word(bytes, 0) != 0x48444e53 {
                return Err("splitter header/alignment".into());
            }
            let infos = word(bytes, 4) as usize;
            let dests = word(bytes, 8) as usize;
            if infos > self.splitters.len() || dests > self.destinations.len() {
                return Err("splitter count budget".into());
            }
            let mut result = self.clone();
            let mut at = 32;
            let mut seen = vec![false; self.splitters.len()];
            for _ in 0..infos {
                let info = take(bytes, &mut at, 16)?;
                let id = word(info, 4) as usize;
                let rate = word(info, 8);
                let count = word(info, 12) as usize;
                if word(info, 0) != 0x49444e53
                    || id >= seen.len()
                    || seen[id]
                    || count > self.destinations.len()
                    || ![32000, 48000].contains(&rate)
                {
                    return Err("splitter info id/count/rate".into());
                }
                seen[id] = true;
                let mut destinations = Vec::with_capacity(count);
                for raw in take(bytes, &mut at, count * 4)?.chunks_exact(4) {
                    let id = word(raw, 0) as usize;
                    if id >= self.destinations.len() || destinations.contains(&id) {
                        return Err("splitter cyclic/out-of-range destinations".into());
                    }
                    destinations.push(id);
                }
                take(bytes, &mut at, 12)?;
                result.splitters[id] = Some(Splitter { rate, destinations });
            }
            let mut seen = vec![false; self.destinations.len()];
            for _ in 0..dests {
                let d = take(bytes, &mut at, 112)?;
                let id = word(d, 4) as usize;
                let active = d[108];
                if word(d, 0) != 0x44444e53 || id >= seen.len() || seen[id] || active > 1 {
                    return Err("splitter destination id/use".into());
                }
                seen[id] = true;
                let mix = word(d, 104) as usize;
                let volume = std::array::from_fn(|i| f32::from_bits(word(d, 8 + i * 4)));
                if active != 0
                    && (mix >= mix_count || volume.iter().any(|v| !v.is_finite() || v.abs() > 64.))
                {
                    return Err("splitter destination mix/gain".into());
                }
                let previous_volume =
                    if active != 0 && self.destinations[id].as_ref().is_none_or(|d| !d.active) {
                        volume
                    } else {
                        self.destinations[id]
                            .as_ref()
                            .map_or([0.; 24], |d| d.previous_volume)
                    };
                result.destinations[id] = Some(Destination {
                    active: active != 0,
                    mix,
                    volume,
                    previous_volume,
                    used_this_frame: false,
                });
            }
            if at.div_ceil(16) * 16 != bytes.len() {
                return Err("splitter trailing records".into());
            }
            Ok(result)
        }
        /// Mono fanout to every active entry in the selected splitter. Validate the
        /// complete set before mutating any destination; no fallback mix is injected.
        pub fn route_mono(
            &mut self,
            id: usize,
            rate: u32,
            source: &[f32],
            mixes: &mut [Vec<Vec<f32>>],
        ) -> RuntimeResult<()> {
            let splitter = self
                .splitters
                .get(id)
                .and_then(Option::as_ref)
                .ok_or("unconfigured voice splitter")?;
            if splitter.rate != rate || source.len() > 1024 || source.iter().any(|v| !v.is_finite())
            {
                return Err("splitter mono source geometry".into());
            }
            let mut updates = Vec::new();
            let mut touched = Vec::new();
            for &id in &splitter.destinations {
                let destination = self.destinations[id]
                    .as_ref()
                    .ok_or("missing splitter destination")?;
                if !destination.active {
                    continue;
                }
                let mix = mixes
                    .get(destination.mix)
                    .filter(|m| !m.is_empty() && m.len() <= 24)
                    .ok_or("inactive splitter target")?;
                if mix.iter().any(|c| c.len() != source.len()) {
                    return Err("splitter target frame geometry".into());
                }
                touched.push(id);
                for (c, channel) in mix.iter().enumerate() {
                    let mut gain = destination.previous_volume[c];
                    let increment = (destination.volume[c] - gain) / source.len().max(1) as f32;
                    let values: Vec<_> = channel
                        .iter()
                        .zip(source)
                        .map(|(to, from)| {
                            let mixed = *to + (from * 32768. * gain).round_ties_even() / 32768.;
                            gain += increment;
                            mixed
                        })
                        .collect();
                    if values.iter().any(|v| !v.is_finite()) {
                        return Err("splitter nonfinite mixed output".into());
                    }
                    updates.push((destination.mix, c, values));
                }
            }
            // Reject a repeated destination mix in this initial subset instead of staging
            // two writes from the same old buffer and accidentally dropping one sum.
            for (i, (mix, channel, _)) in updates.iter().enumerate() {
                if updates[..i]
                    .iter()
                    .any(|(m, c, _)| m == mix && c == channel)
                {
                    return Err("splitter repeated mix accumulation subset".into());
                }
            }
            for (mix, channel, values) in updates {
                mixes[mix][channel] = values;
            }
            for id in touched {
                self.destinations[id].as_mut().unwrap().used_this_frame = true;
            }
            Ok(())
        }
        pub fn finish_frame(&mut self) {
            for dest in self.destinations.iter_mut().flatten() {
                if dest.active && dest.used_this_frame {
                    dest.previous_volume = dest.volume;
                }
                dest.used_this_frame = false;
            }
        }
    }
    #[cfg(test)]
    mod tests {
        use super::*;
        fn header(i: u32, d: u32) -> Vec<u8> {
            let mut b = vec![0xa5; 32];
            b[..4].copy_from_slice(b"SNDH");
            b[4..8].copy_from_slice(&i.to_le_bytes());
            b[8..12].copy_from_slice(&d.to_le_bytes());
            b
        }
        fn fixture() -> Vec<u8> {
            let mut b = header(1, 2);
            b.extend(b"SNDI");
            b.extend(0u32.to_le_bytes());
            b.extend(48000u32.to_le_bytes());
            b.extend(2u32.to_le_bytes());
            b.extend(0u32.to_le_bytes());
            b.extend(1u32.to_le_bytes());
            b.extend([0x77; 12]);
            for id in 0..2u32 {
                let mut d = [0; 112];
                d[..4].copy_from_slice(b"SNDD");
                d[4..8].copy_from_slice(&id.to_le_bytes());
                d[8..12].copy_from_slice(&(0.25 + id as f32).to_le_bytes());
                d[104..108].copy_from_slice(&id.to_le_bytes());
                d[108] = 1;
                b.extend(d);
            }
            b.resize(b.len().div_ceil(16) * 16, 0x55);
            b
        }
        #[test]
        fn dirty_empty_update_retains_splitters_and_nonzero_fanout() {
            let mut r = Routing::new(2, 3).unwrap().update(&fixture(), 2).unwrap();
            assert_eq!(r.update(&header(0, 0), 2).unwrap(), r);
            let mut mixes = vec![vec![vec![0.; 2]], vec![vec![1.; 2]]];
            r.route_mono(0, 48000, &[1., -1.], &mut mixes).unwrap();
            assert_eq!(mixes[0][0], [0.25, -0.25]);
            assert_eq!(mixes[1][0], [2.25, -0.25]);
        }
        #[test]
        fn missing_late_destination_does_not_change_first_mix() {
            let mut r = Routing::new(2, 3).unwrap().update(&fixture(), 2).unwrap();
            r.destinations[1] = None;
            let mut mixes = vec![vec![vec![0.; 2]], vec![vec![1.; 2]]];
            let old = mixes.clone();
            assert!(r.route_mono(0, 48000, &[1., 1.], &mut mixes).is_err());
            assert_eq!(mixes, old);
        }
        #[test]
        fn truncated_and_duplicate_ids_preserve_prior_state() {
            let r = Routing::new(2, 3).unwrap().update(&fixture(), 2).unwrap();
            let original = r.clone();
            let mut bad = fixture();
            bad[52..56].copy_from_slice(&0u32.to_le_bytes());
            assert!(r.update(&bad, 2).is_err());
            assert!(r.update(&fixture()[..64], 2).is_err());
            assert_eq!(r, original);
        }
    }
}
