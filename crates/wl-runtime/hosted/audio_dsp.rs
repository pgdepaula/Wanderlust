//! Checked auxiliary DSP memory exchange. Samples here are normalized floats.
//! Wire semantics: Ryujinx.Audio 4930dced, AuxiliaryBufferCommand,
//! AuxiliaryBufferEffect and AuxiliaryBufferHeader (REV10+ lost-count
//! preservation); independently implemented with checked guest access.
use wl_guest_runtime::{Memory, RuntimeResult};

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
fn commit(memory: &mut Memory, writes: Vec<(u64, Vec<u8>)>) -> RuntimeResult<()> {
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
            commit(memory, writes)?;
            return Ok(AuxiliaryResult {
                quantized_input: input.to_vec(),
                output: input.to_vec(),
            });
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
        commit(memory, writes)?;
        Ok(AuxiliaryResult {
            quantized_input,
            output,
        })
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
