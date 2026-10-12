//! Strict single-target raster state decoded from NVIDIA MAXWELL_B byte methods.
use crate::{Result, invalid, maxwell::MethodError, memory::GpuAddress};
/// Explicit state accepted by the initial single-sample, no-depth/no-blend slice.
pub const FIXED_RASTER_STATE: &[(u16, u32)] = &[
    (0x0138, 1),      // unconditional rendering
    (0x037c, 1),      // rasterization enabled
    (0x0a18, 0x6420), // identity viewport coordinate swizzle XYZW
    (0x0df8, 0),
    (0x0dfc, 0), // window offsets
    (0x12ec, 0), // alpha test disabled
    (0x0fa4, 0), // sample masking disabled
    (0x121c, 1),
    (0x12cc, 0),
    (0x12e8, 0),
    (0x1380, 0),
    (0x1360, 0),
    (0x135c, 0),
    (0x12e4, 0),
    (0x1918, 0),
    (0x13ac, 0),
    (0x1924, 0),
    (0x192c, 1),
    (0x193c, 0x1801),
    (0x15d0, 0),
    (0x1538, 0),
    (0x1a00, 0x1111),
    (0x0dac, 0x1b02),
    (0x0db0, 0x1b02),
    (0x19c4, 0),
    (0x19bc, 0),
    (0x0db4, 0),
    (0x0dc8, 0),
    (0x168c, 0),
    (0x0c08, 0),
    (0x0c0c, 0x3f800000),
];
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ColorTarget {
    pub address: GpuAddress,
    pub pitch: u32,
    pub width: u32,
    pub height: u32,
}
impl ColorTarget {
    pub fn size(&self) -> Result<usize> {
        usize::try_from(u64::from(self.pitch) * u64::from(self.height))
            .map_err(|_| invalid("color span exceeds host"))
    }
}
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Rasterization {
    pub viewport: [f32; 6],
    pub scissor: [u32; 4],
    pub flip_y: bool,
}
#[derive(Debug, Default)]
pub(crate) struct RasterRegisters {
    color: [Option<u32>; 9],
    viewport: [Option<u32>; 6],
    surface: [Option<u32>; 2],
    clip: [Option<u32>; 2],
    scissor: [Option<u32>; 2],
    scissor_enable: Option<u32>,
    fixed: u32,
}
impl RasterRegisters {
    pub fn apply(&mut self, method: u16, value: u32) -> std::result::Result<bool, MethodError> {
        let bad = || MethodError::UnsupportedValue { method, value };
        if let Some((n, (_, expected))) = FIXED_RASTER_STATE
            .iter()
            .enumerate()
            .find(|(_, v)| v.0 == method)
        {
            if value != *expected {
                return Err(bad());
            }
            self.fixed |= 1 << n;
            return Ok(true);
        }
        match method {
            0x0800..=0x0820 => {
                let n = ((method - 0x0800) / 4) as usize;
                let valid = match n {
                    0 => value <= 255,
                    1 => value % 256 == 0,
                    2 => value > 0 && value <= 0x0fffffff && value % 4 == 0,
                    3 => value > 0 && value <= 0x1ffff,
                    4 => value == 0xd5,
                    5 => value == 0x1000,
                    6 => value == 1,
                    7 | 8 => value == 0,
                    _ => false,
                };
                if !valid {
                    return Err(bad());
                }
                self.color[n] = Some(value);
            }
            0x0a00..=0x0a14 => {
                if !f32::from_bits(value).is_finite() {
                    return Err(bad());
                }
                self.viewport[((method - 0xa00) / 4) as usize] = Some(value);
            }
            0x0ff4 | 0x0ff8 => self.surface[((method - 0xff4) / 4) as usize] = Some(value),
            0x0c00 | 0x0c04 => self.clip[((method - 0xc00) / 4) as usize] = Some(value),
            0x0e00 => {
                if value != 1 {
                    return Err(bad());
                }
                self.scissor_enable = Some(value);
            }
            0x0e04 | 0x0e08 => self.scissor[((method - 0xe04) / 4) as usize] = Some(value),
            _ => return Ok(false),
        }
        Ok(true)
    }
    pub fn resolve(&self) -> Result<(ColorTarget, Rasterization)> {
        if self.fixed != (1u32 << FIXED_RASTER_STATE.len()) - 1 || self.scissor_enable != Some(1) {
            return Err(invalid("incomplete or unsupported fixed raster state"));
        }
        let mut c = [0u32; 9];
        for (out, v) in c.iter_mut().zip(self.color) {
            *out = v.ok_or_else(|| invalid("incomplete color target"))?;
        }
        let address = (u64::from(c[0]) << 32) | u64::from(c[1]);
        let size = u64::from(c[2]) * u64::from(c[3]);
        if address.checked_add(size).is_none_or(|v| v > 1 << 40) {
            return Err(invalid("color target exceeds 40-bit GPU space"));
        }
        // For pitch layout WIDTH is byte pitch, not a pixel dimension (Mesa FB setup).
        let target = ColorTarget {
            address: GpuAddress(address),
            pitch: c[2],
            width: c[2] / 4,
            height: c[3],
        };
        let mut v = [0f32; 6];
        for (out, word) in v.iter_mut().zip(self.viewport) {
            *out = f32::from_bits(word.ok_or_else(|| invalid("incomplete viewport"))?);
        }
        let [sx, sy, sz, tx, ty, tz] = v;
        let vp = [tx - sx, ty - sy.abs(), 2. * sx, 2. * sy.abs(), 0., 1.];
        if sx <= 0.
            || sy == 0.
            || sz != 1.
            || tz != 0.
            || vp.iter().any(|v| !v.is_finite())
            || vp[..4].iter().any(|v| v.fract() != 0.)
            || vp[0] < 0.
            || vp[1] < 0.
            || vp[0] + vp[2] > target.width as f32
            || vp[1] + vp[3] > target.height as f32
        {
            return Err(invalid("unsupported/out-of-bounds viewport transform"));
        }
        let mut lo = [0u32; 2];
        let mut hi = [target.width, target.height];
        for axis in 0..2 {
            for (word, extent) in [
                (self.surface[axis], true),
                (self.clip[axis], true),
                (self.scissor[axis], false),
            ] {
                let word =
                    word.ok_or_else(|| invalid("incomplete surface/viewport/scissor clipping"))?;
                let a = word & 65535;
                let b = if extent { a + (word >> 16) } else { word >> 16 };
                if b <= a {
                    return Err(invalid("empty/reversed clip rectangle unsupported"));
                }
                lo[axis] = lo[axis].max(a);
                hi[axis] = hi[axis].min(b);
            }
        }
        if lo[0] >= hi[0] || lo[1] >= hi[1] {
            return Err(invalid("empty clipped draw unsupported"));
        }
        Ok((
            target,
            Rasterization {
                viewport: vp,
                scissor: [lo[0], lo[1], hi[0] - lo[0], hi[1] - lo[1]],
                flip_y: sy > 0.,
            },
        ))
    }
}
