//! Strict single-target raster state decoded from NVIDIA MAXWELL_B byte methods.
use crate::{Result, invalid, maxwell::MethodError, memory::GpuAddress};
/// Explicit baseline state for the single-sample slice. Blend enable and channel
/// masks additionally accept the bounded dynamic values validated below.
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
pub enum ColorLayout {
    Pitch { bytes_per_row: u32 },
    BlockLinear { block_height_log2: u8 },
}
/// RGBA8_UNORM, single layer, depth one. Logical host pixels and guest storage
/// are distinct. Pitch WIDTH retains the existing full-pitch pixel interpretation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ColorTarget {
    pub address: GpuAddress,
    pub width: u32,
    pub height: u32,
    pub layout: ColorLayout,
}
impl ColorTarget {
    pub fn linear_pitch(&self) -> Result<u32> {
        self.width
            .checked_mul(4)
            .ok_or_else(|| invalid("color row overflow"))
    }
    pub fn block_linear(&self) -> Result<Option<crate::layout::BlockLinear2d>> {
        match self.layout {
            ColorLayout::BlockLinear { block_height_log2 } => {
                let layout = crate::layout::BlockLinear2d::new(
                    self.width,
                    self.height,
                    crate::format::Format::Rgba8Unorm,
                    block_height_log2,
                )?;
                // Bound the complete publication transaction: padded work image,
                // tiled output, readback/redecoded logical pixels and raw CPU patches.
                if layout.tiled_size > (64 * 1024 * 1024) / 5 {
                    return Err(invalid(
                        "color conversion transaction exceeds 64 MiB budget",
                    ));
                }
                Ok(Some(layout))
            }
            ColorLayout::Pitch { bytes_per_row } => {
                if self.width == 0 || self.height == 0 || bytes_per_row != self.linear_pitch()? {
                    return Err(invalid("invalid pitch color geometry"));
                }
                Ok(None)
            }
        }
    }
    pub fn size(&self) -> Result<usize> {
        let size = if let Some(layout) = self.block_linear()? {
            layout.tiled_size
        } else {
            usize::try_from(u64::from(self.linear_pitch()?) * u64::from(self.height))
                .map_err(|_| invalid("color span exceeds host"))?
        };
        if self
            .address
            .0
            .checked_add(size as u64)
            .is_none_or(|end| end > 1 << 40)
        {
            return Err(invalid("color target exceeds 40-bit GPU space"));
        }
        Ok(size)
    }
}
/// Implemented global additive blending factors, independently of host enum values.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BlendFactor {
    Zero,
    One,
    SourceAlpha,
    OneMinusSourceAlpha,
}
fn factor(v: u32) -> Option<BlendFactor> {
    match v {
        0x4000 | 1 => Some(BlendFactor::Zero),
        0x4001 | 2 => Some(BlendFactor::One),
        0x4302 | 5 => Some(BlendFactor::SourceAlpha),
        0x4303 | 6 => Some(BlendFactor::OneMinusSourceAlpha),
        _ => None,
    }
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BlendState {
    pub color: [BlendFactor; 2],
    pub alpha: [BlendFactor; 2],
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ColorState {
    pub blend: Option<BlendState>,
    pub write_mask: u8,
}
impl Default for ColorState {
    fn default() -> Self {
        Self {
            blend: None,
            write_mask: 15,
        }
    }
}
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Rasterization {
    pub viewport: [f32; 6],
    pub scissor: [u32; 4],
    pub flip_y: bool,
    pub color: ColorState,
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
    blend_enabled: bool,
    blend_separate: Option<bool>,
    blend: [Option<u32>; 6],
    write_mask: u8,
}
impl RasterRegisters {
    pub fn apply(&mut self, method: u16, value: u32) -> std::result::Result<bool, MethodError> {
        if method % 4 != 0 {
            return Err(MethodError::InvalidMethod(method));
        }
        // NVB197_SET_GLOBAL_COLOR_KEY_ENABLE is bit 0. Only FALSE is
        // implemented; enabling color-key comparison requires separate semantics.
        if method == 0x1354 {
            return if value == 0 {
                Ok(true)
            } else {
                Err(MethodError::UnsupportedValue { method, value })
            };
        }
        let bad = || MethodError::UnsupportedValue { method, value };
        if let Some((n, (_, expected))) = FIXED_RASTER_STATE
            .iter()
            .enumerate()
            .find(|(_, v)| v.0 == method)
        {
            if method == 0x1360 {
                if value > 1 {
                    return Err(bad());
                }
                self.blend_enabled = value != 0;
            } else if method == 0x1a00 {
                if value & !0x1111 != 0 {
                    return Err(bad());
                }
                self.write_mask = (0..4).fold(0, |m, n| m | (((value >> (n * 4)) & 1) as u8) << n);
            } else if value != *expected {
                return Err(bad());
            }
            self.fixed |= 1 << n;
            return Ok(true);
        }
        match method {
            0x133c => {
                if value > 1 {
                    return Err(bad());
                }
                self.blend_separate = Some(value != 0);
            }
            0x1340..=0x1350 | 0x1358 => {
                let n = if method == 0x1358 {
                    5
                } else {
                    ((method - 0x1340) / 4) as usize
                };
                if (n == 0 || n == 3) && !matches!(value, 1 | 0x8006)
                    || n != 0 && n != 3 && factor(value).is_none()
                {
                    return Err(bad());
                }
                self.blend[n] = Some(value);
            }
            0x0800..=0x0820 => {
                let n = ((method - 0x0800) / 4) as usize;
                let valid = match n {
                    0 => value <= 255,
                    1 => value % 256 == 0,
                    2 => value > 0 && value <= 0x0fffffff,
                    3 => value > 0 && value <= 0x1ffff,
                    4 => value == 0xd5,
                    5 => value == 0x1000 || (value & !0x70 == 0 && (value >> 4) <= 5),
                    6 => value == 1,
                    7 => true, // DWORD array pitch is checked after complete geometry is known.
                    8 => value == 0,
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
        let blend = if self.blend_enabled {
            let separate = self
                .blend_separate
                .ok_or_else(|| invalid("missing explicit alpha blend selection"))?;
            let get =
                |n: usize| self.blend[n].ok_or_else(|| invalid("incomplete global blend state"));
            get(0)?;
            let color = [factor(get(1)?).unwrap(), factor(get(2)?).unwrap()];
            let alpha = if separate {
                get(3)?;
                [factor(get(4)?).unwrap(), factor(get(5)?).unwrap()]
            } else {
                color
            };
            Some(BlendState { color, alpha })
        } else {
            None
        };
        let color = ColorState {
            blend,
            write_mask: self.write_mask,
        };
        let mut c = [0u32; 9];
        for (out, v) in c.iter_mut().zip(self.color) {
            *out = v.ok_or_else(|| invalid("incomplete color target"))?;
        }
        let address = (u64::from(c[0]) << 32) | u64::from(c[1]);
        // WIDTH is byte pitch for PITCH, pixels for BLOCKLINEAR (Mesa FB setup).
        let (width, layout) = if c[5] == 0x1000 {
            if c[2] % 4 != 0 {
                return Err(invalid("RGBA8 pitch is not pixel aligned"));
            }
            (
                c[2] / 4,
                ColorLayout::Pitch {
                    bytes_per_row: c[2],
                },
            )
        } else {
            (
                c[2],
                ColorLayout::BlockLinear {
                    block_height_log2: (c[5] >> 4) as u8,
                },
            )
        };
        let target = ColorTarget {
            address: GpuAddress(address),
            width,
            height: c[3],
            layout,
        };
        let size = target.size()?;
        if c[7] != 0
            && (matches!(layout, ColorLayout::Pitch { .. }) || u64::from(c[7]) * 4 != size as u64)
        {
            return Err(invalid("unsupported single-layer array pitch"));
        }
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
                color,
            },
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn unaligned_direct_method_cannot_alias_register() {
        let mut r = RasterRegisters::default();
        for method in [0x1341, 0x0801, 0x0a01, 0x1355] {
            assert_eq!(r.apply(method, 0), Err(MethodError::InvalidMethod(method)));
        }
        assert_eq!(r.blend, [None; 6]);
        assert_eq!(r.color, [None; 9]);
        assert_eq!(r.viewport, [None; 6]);
    }
}
