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
