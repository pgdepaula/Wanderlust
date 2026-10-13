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
pub struct ClearSurface {
    pub color: Option<(ColorTarget, [f32; 4])>,
    pub depth: Option<(DepthTarget, f32)>,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FrontFace {
    Clockwise,
    CounterClockwise,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CullFace {
    Front,
    Back,
}
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub enum Culling {
    #[default]
    Disabled,
    Enabled {
        face: CullFace,
        front: FrontFace,
    },
}
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Rasterization {
    pub viewport: [f32; 6],
    pub scissor: [u32; 4],
    pub flip_y: bool,
    pub color: ColorState,
    pub depth: Option<DepthState>,
    pub cull: Culling,
}
/// B197 comparison identity, independent of host API enum values.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DepthCompare {
    Never,
    Less,
    Equal,
    LessEqual,
    Greater,
    NotEqual,
    GreaterEqual,
    Always,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DepthState {
    pub compare: DepthCompare,
    pub write: bool,
}
/// Single-layer ZF32, block-linear, four little-endian bytes per depth sample.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DepthTarget {
    pub address: GpuAddress,
    pub width: u32,
    pub height: u32,
    pub block_height_log2: u8,
}
impl DepthTarget {
    // ColorTarget is reused ONLY as a checked four-byte storage geometry, not a
    // color format interpretation. Depth transfers remain f32 throughout.
    pub(crate) fn storage(self) -> ColorTarget {
        ColorTarget {
            address: self.address,
            width: self.width,
            height: self.height,
            layout: ColorLayout::BlockLinear {
                block_height_log2: self.block_height_log2,
            },
        }
    }
    pub fn size(self) -> Result<usize> {
        self.storage().size()
    }
}
fn depth_compare(value: u32) -> Option<DepthCompare> {
    use DepthCompare::*;
    Some(match value {
        1 | 0x200 => Never,
        2 | 0x201 => Less,
        3 | 0x202 => Equal,
        4 | 0x203 => LessEqual,
        5 | 0x204 => Greater,
        6 | 0x205 => NotEqual,
        7 | 0x206 => GreaterEqual,
        8 | 0x207 => Always,
        _ => return None,
    })
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
    depth_test: bool,
    depth_write: bool,
    depth_select: bool,
    depth_func: Option<DepthCompare>,
    zt: [Option<u32>; 9],
    clear_control: Option<u32>,
    clear_color: [Option<u32>; 4],
    clear_depth: Option<u32>,
    cull_enabled: bool,
    front_face: Option<FrontFace>,
    cull_face: Option<CullFace>,
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
            if matches!(method, 0x12cc | 0x12e8 | 0x1538) {
                if value > 1 {
                    return Err(bad());
                }
                match method {
                    0x12cc => self.depth_test = value != 0,
                    0x12e8 => self.depth_write = value != 0,
                    _ => self.depth_select = value != 0,
                }
            } else if method == 0x1918 {
                if value > 1 {
                    return Err(bad());
                }
                self.cull_enabled = value != 0;
            } else if method == 0x1360 {
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
            0x191c => {
                self.front_face = Some(match value {
                    0x900 => FrontFace::Clockwise,
                    0x901 => FrontFace::CounterClockwise,
                    _ => return Err(bad()),
                })
            }
            0x1920 => {
                self.cull_face = Some(match value {
                    0x404 => CullFace::Front,
                    0x405 => CullFace::Back,
                    _ => return Err(bad()),
                })
            }
            0x0d80..=0x0d8c => self.clear_color[((method - 0xd80) / 4) as usize] = Some(value),
            0x0d90 => self.clear_depth = Some(value),
            0x10f8 => {
                if value != 0 {
                    return Err(bad());
                }
                self.clear_control = Some(value);
            }
            0x130c => self.depth_func = Some(depth_compare(value).ok_or_else(bad)?),
            0x0fe0..=0x0ff0 | 0x1228..=0x1230 | 0x179c => {
                let n = match method {
                    0xfe0..=0xff0 => ((method - 0xfe0) / 4) as usize,
                    0x1228..=0x1230 => 5 + ((method - 0x1228) / 4) as usize,
                    _ => 8,
                };
                let valid = match n {
                    0 => value <= 255,
                    1 => value % 256 == 0,
                    2 => value == 0xa,
                    3 => value & !0x70 == 0 && value >> 4 <= 5,
                    4 => true,
                    5 => value > 0 && value <= 0xfffffff,
                    6 => value > 0 && value <= 0x1ffff,
                    7 => matches!(value, 1 | 0x10001),
                    8 => value == 0,
                    _ => false,
                };
                if !valid {
                    return Err(bad());
                }
                self.zt[n] = Some(value);
            }
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
    fn depth_state(&self) -> Result<Option<DepthState>> {
        if !self.depth_test && !self.depth_write && !self.depth_select {
            return Ok(None);
        }
        if !self.depth_test || !self.depth_select {
            return Err(invalid("unsupported depth test/target selection"));
        }
        Ok(Some(DepthState {
            compare: self
                .depth_func
                .ok_or_else(|| invalid("missing depth comparison"))?,
            write: self.depth_write,
        }))
    }
    pub fn depth_target(&self) -> Result<Option<DepthTarget>> {
        if self.depth_state()?.is_none() {
            return Ok(None);
        }
        self.raw_depth_target().map(Some)
    }
    fn raw_depth_target(&self) -> Result<DepthTarget> {
        let mut z = [0; 9];
        for (out, v) in z.iter_mut().zip(self.zt) {
            *out = v.ok_or_else(|| invalid("incomplete ZF32 target"))?;
        }
        let target = DepthTarget {
            address: GpuAddress((u64::from(z[0]) << 32) | u64::from(z[1])),
            width: z[5],
            height: z[6],
            block_height_log2: (z[3] >> 4) as u8,
        };
        let size = target.size()?;
        if z[4] != 0 && u64::from(z[4]) * 4 != size as u64 {
            return Err(invalid("unsupported depth array pitch"));
        }
        Ok(target)
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
        let target = self.color_target()?;
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
                depth: self.depth_state()?,
                cull: if self.cull_enabled {
                    Culling::Enabled {
                        face: self
                            .cull_face
                            .ok_or_else(|| invalid("incomplete cull face"))?,
                        front: self
                            .front_face
                            .ok_or_else(|| invalid("incomplete front face"))?,
                    }
                } else {
                    Culling::Disabled
                },
            },
        ))
    }
    fn color_target(&self) -> Result<ColorTarget> {
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
        Ok(target)
    }
    pub fn resolve_clear(&self, color: bool, depth: bool) -> Result<ClearSurface> {
        for method in [0x0138, 0x15d0] {
            let n = FIXED_RASTER_STATE
                .iter()
                .position(|v| v.0 == method)
                .unwrap();
            if self.fixed & (1 << n) == 0 {
                return Err(invalid(
                    "clear requires explicit unconditional single-sample state",
                ));
            }
        }
        if self.clear_control != Some(0) || !color && !depth {
            return Err(invalid("unsupported or incomplete clear controls"));
        }
        let color = if color {
            let n = FIXED_RASTER_STATE
                .iter()
                .position(|v| v.0 == 0x121c)
                .unwrap();
            if self.fixed & (1 << n) == 0 {
                return Err(invalid("clear requires explicit target-zero selection"));
            }
            let target = self.color_target()?;
            let mut values = [0.; 4];
            for (out, word) in values.iter_mut().zip(self.clear_color) {
                *out =
                    f32::from_bits(word.ok_or_else(|| invalid("incomplete color clear values"))?);
                if !out.is_finite() || !(0.0..=1.0).contains(out) {
                    return Err(invalid("unsupported UNORM clear value"));
                }
            }
            Some((target, values))
        } else {
            None
        };
        let depth = if depth {
            let n = FIXED_RASTER_STATE
                .iter()
                .position(|v| v.0 == 0x1538)
                .unwrap();
            if self.fixed & (1 << n) == 0 || !self.depth_select {
                return Err(invalid("clear requires selected depth target"));
            }
            let target = self.raw_depth_target()?;
            let bits = self
                .clear_depth
                .ok_or_else(|| invalid("incomplete depth clear value"))?;
            let value = f32::from_bits(bits);
            if !value.is_finite()
                || !(0.0..=1.0).contains(&value)
                || value.is_subnormal()
                || bits == 0x80000000
            {
                return Err(invalid("unsupported ZF32 clear value"));
            }
            Some((target, value))
        } else {
            None
        };
        let extent = color
            .map(|v| (v.0.width, v.0.height))
            .or_else(|| depth.map(|v| (v.0.width, v.0.height)))
            .unwrap();
        if depth.is_some_and(|v| (v.0.width, v.0.height) != extent) {
            return Err(invalid("clear attachment dimensions differ"));
        }
        if extent.0 > 65535
            || extent.1 > 65535
            || self.surface != [Some(extent.0 << 16), Some(extent.1 << 16)]
        {
            return Err(invalid("only full surface-clip clear is implemented"));
        }
        Ok(ClearSurface { color, depth })
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
