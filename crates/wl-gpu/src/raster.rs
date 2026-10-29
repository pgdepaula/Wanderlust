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
    (0x0d7c, 1), // homogeneous Z interval is zero through positive W
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
    pub format: ColorFormat,
}
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub enum ColorFormat {
    #[default]
    Rgba8,
    Rg11b10Float,
}
impl ColorFormat {
    #[cfg(feature = "wgpu-backend")]
    pub(crate) fn host(self) -> wgpu::TextureFormat {
        match self {
            Self::Rgba8 => wgpu::TextureFormat::Rgba8Unorm,
            Self::Rg11b10Float => wgpu::TextureFormat::Rg11b10Ufloat,
        }
    }
    /// Display-only conversion after exact packed guest publication. HDR is
    /// clamped for the existing RGBA8 diagnostic image, never for guest storage.
    pub fn preview(self, bytes: &mut [u8]) {
        if self == Self::Rgba8 {
            return;
        }
        for pixel in bytes.chunks_exact_mut(4) {
            let packed = u32::from_le_bytes(pixel.try_into().unwrap());
            for (out, (bits, fraction)) in pixel[..3].iter_mut().zip([
                (packed & 2047, 6),
                ((packed >> 11) & 2047, 6),
                (packed >> 22, 5),
            ]) {
                let exponent = bits >> fraction;
                let mantissa = bits & ((1 << fraction) - 1);
                let value = match exponent {
                    0 => mantissa as f32 * f32::from_bits((113 - fraction) << 23),
                    31 if mantissa == 0 => f32::INFINITY,
                    31 => f32::NAN,
                    _ => f32::from_bits(((exponent + 112) << 23) | (mantissa << (23 - fraction))),
                };
                *out = (value.clamp(0., 1.) * 255.).round_ties_even() as u8;
            }
            pixel[3] = 255;
        }
    }
}
impl ColorTarget {
    pub(crate) fn storage(self) -> SurfaceStorage {
        SurfaceStorage {
            address: self.address,
            width: self.width,
            height: self.height,
            layout: self.layout,
            format: SurfaceFormat::Color(self.format),
        }
    }
    pub fn linear_pitch(&self) -> Result<u32> {
        self.storage().linear_pitch()
    }
    pub fn block_linear(&self) -> Result<Option<crate::layout::BlockLinear2d>> {
        self.storage().block_linear()
    }
    pub fn size(&self) -> Result<usize> {
        self.storage().size()
    }
}
/// Storage identity is independent of GPU aliases and host attachment handles.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum SurfaceFormat {
    Color(ColorFormat),
    Depth(DepthFormat),
}
impl SurfaceFormat {
    fn bytes_per_texel(self) -> u32 {
        match self {
            Self::Color(_) => 4,
            Self::Depth(format) => format.bytes_per_sample(),
        }
    }
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct SurfaceStorage {
    pub address: GpuAddress,
    pub width: u32,
    pub height: u32,
    pub layout: ColorLayout,
    pub format: SurfaceFormat,
}
impl SurfaceStorage {
    #[cfg(feature = "wgpu-backend")]
    pub fn depth_format(self) -> Result<DepthFormat> {
        match self.format {
            SurfaceFormat::Depth(format) => Ok(format),
            SurfaceFormat::Color(_) => Err(invalid("color surface used as depth storage")),
        }
    }
    pub fn linear_pitch(self) -> Result<u32> {
        self.width
            .checked_mul(self.format.bytes_per_texel())
            .ok_or_else(|| invalid("surface row overflow"))
    }
    pub fn block_linear(&self) -> Result<Option<crate::layout::BlockLinear2d>> {
        match self.layout {
            ColorLayout::BlockLinear { block_height_log2 } => {
                let layout = crate::layout::BlockLinear2d::new_uncompressed(
                    self.width,
                    self.height,
                    self.format.bytes_per_texel(),
                    block_height_log2,
                )?;
                // Bound the complete publication transaction: padded work image,
                // tiled output, readback/redecoded logical pixels and raw CPU patches.
                if layout.tiled_size > (64 * 1024 * 1024) / 5 {
                    return Err(invalid(
                        "surface conversion transaction exceeds 64 MiB budget",
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
                .map_err(|_| invalid("surface span exceeds host"))?
        };
        if self
            .address
            .0
            .checked_add(size as u64)
            .is_none_or(|end| end > 1 << 40)
        {
            return Err(invalid("surface exceeds 40-bit GPU space"));
        }
        Ok(size)
    }
}
/// Implemented global blending factors, independently of host enum values.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BlendFactor {
    Zero,
    One,
    SourceAlpha,
    OneMinusSourceAlpha,
    SourceColor,
    OneMinusSourceColor,
    DestinationAlpha,
    OneMinusDestinationAlpha,
    DestinationColor,
    OneMinusDestinationColor,
}
fn factor(v: u32) -> Option<BlendFactor> {
    match v {
        0x4000 | 1 => Some(BlendFactor::Zero),
        0x4001 | 2 => Some(BlendFactor::One),
        0x4302 | 5 => Some(BlendFactor::SourceAlpha),
        0x4303 | 6 => Some(BlendFactor::OneMinusSourceAlpha),
        0x4300 | 3 => Some(BlendFactor::SourceColor),
        0x4301 | 4 => Some(BlendFactor::OneMinusSourceColor),
        0x4304 | 7 => Some(BlendFactor::DestinationAlpha),
        0x4305 | 8 => Some(BlendFactor::OneMinusDestinationAlpha),
        0x4306 | 9 => Some(BlendFactor::DestinationColor),
        0x4307 | 10 => Some(BlendFactor::OneMinusDestinationColor),
        _ => None,
    }
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BlendOperation {
    Add,
    Subtract,
    ReverseSubtract,
    Min,
    Max,
}
fn blend_operation(v: u32) -> Option<BlendOperation> {
    use BlendOperation::*;
    Some(match v {
        1 | 0x8006 => Add,
        2 | 0x800a => Subtract,
        3 | 0x800b => ReverseSubtract,
        4 | 0x8007 => Min,
        5 | 0x8008 => Max,
        _ => return None,
    })
}
fn blend_factors(op: BlendOperation, factors: [BlendFactor; 2]) -> [BlendFactor; 2] {
    if matches!(op, BlendOperation::Min | BlendOperation::Max) {
        [BlendFactor::One; 2]
    } else {
        factors
    }
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BlendState {
    pub color: [BlendFactor; 2],
    pub alpha: [BlendFactor; 2],
    pub color_operation: BlendOperation,
    pub alpha_operation: BlendOperation,
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
/// Homogeneous Z clip planes, independent of host API conventions.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub enum DepthClipRange {
    #[default]
    ZeroToOne,
    NegativeOneToOne,
}
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Rasterization {
    pub viewport: [f32; 6],
    pub scissor: [u32; 4],
    pub flip_y: bool,
    pub depth_clip: DepthClipRange,
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
pub enum DepthFormat {
    Float32,
    Unorm16,
}
impl DepthFormat {
    pub fn bytes_per_sample(self) -> u32 {
        match self {
            Self::Float32 => 4,
            Self::Unorm16 => 2,
        }
    }
    fn decode(value: u32) -> Option<Self> {
        match value {
            0x0a => Some(Self::Float32),
            0x13 => Some(Self::Unorm16),
            _ => None,
        }
    }
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DepthState {
    pub format: DepthFormat,
    pub compare: DepthCompare,
    pub write: bool,
}
/// Single-layer block-linear depth, with an explicit guest sample format.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DepthTarget {
    pub format: DepthFormat,
    pub address: GpuAddress,
    pub width: u32,
    pub height: u32,
    pub block_height_log2: u8,
}
impl DepthTarget {
    pub(crate) fn storage(self) -> SurfaceStorage {
        SurfaceStorage {
            address: self.address,
            width: self.width,
            height: self.height,
            layout: ColorLayout::BlockLinear {
                block_height_log2: self.block_height_log2,
            },
            format: SurfaceFormat::Depth(self.format),
        }
    }
    pub fn block_linear(self) -> Result<crate::layout::BlockLinear2d> {
        self.storage()
            .block_linear()?
            .ok_or_else(|| invalid("depth requires block-linear storage"))
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
    inactive_color_formats: [Option<u32>; 7],
    viewports: [[Option<u32>; 6]; 16],
    viewport_swizzles: [Option<u32>; 16],
    viewport_depth: [[Option<u32>; 2]; 16],
    depth_clip: DepthClipRange,
    clip_profile: Option<u32>,
    surface: [Option<u32>; 2],
    clips: [[Option<u32>; 2]; 16],
    scissor: [[Option<u32>; 2]; 16],
    scissor_enable: [Option<u32>; 16],
    fixed: u64,
    blend_enabled: bool,
    blend_per_target: bool,
    target_blend: [[Option<u32>; 7]; 8],
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
    reduction_thresholds: [Option<u32>; 6],
    reduction_disabled: bool,
    alpha_fraction: Option<u8>,
    alpha_coverage_disabled: bool,
    window_clip: [[Option<u32>; 2]; 8],
    window_clip_type: Option<u8>,
    // B197 initialized-context method list seeds 0x194c to FALSE. This is
    // context initialization, not an inferred hardware power-on value.
    window_clip_enabled: bool,
}
impl RasterRegisters {
    pub fn color_target_format(&self, slot: usize) -> Option<u32> {
        if slot == 0 {
            self.color[4]
        } else {
            self.inactive_color_formats.get(slot - 1).copied().flatten()
        }
    }
    fn resolve_blend(&self) -> Result<Option<BlendState>> {
        if !self.blend_enabled {
            return Ok(None);
        }
        let (separate, registers) = if self.blend_per_target {
            let bank = &self.target_blend[0];
            (
                bank[0].ok_or_else(|| invalid("missing per-target alpha blend selection"))? != 0,
                &bank[1..],
            )
        } else {
            (
                self.blend_separate
                    .ok_or_else(|| invalid("missing explicit alpha blend selection"))?,
                self.blend.as_slice(),
            )
        };
        let get = |n: usize| {
            registers[n].ok_or_else(|| {
                invalid(if self.blend_per_target {
                    "incomplete per-target blend state"
                } else {
                    "incomplete global blend state"
                })
            })
        };
        let color_operation = blend_operation(get(0)?).unwrap();
        // Canonicalize ignored MIN/MAX factors only in the resolved pipeline
        // state. Raw registers remain intact for a later ADD/SUBTRACT write.
        let color = blend_factors(
            color_operation,
            [factor(get(1)?).unwrap(), factor(get(2)?).unwrap()],
        );
        let (alpha_operation, alpha) = if separate {
            let op = blend_operation(get(3)?).unwrap();
            (
                op,
                blend_factors(op, [factor(get(4)?).unwrap(), factor(get(5)?).unwrap()]),
            )
        } else {
            (color_operation, color)
        };
        Ok(Some(BlendState {
            color,
            alpha,
            color_operation,
            alpha_operation,
        }))
    }
    pub fn apply(&mut self, method: u16, value: u32) -> std::result::Result<bool, MethodError> {
        if method % 4 != 0 {
            return Err(MethodError::InvalidMethod(method));
        }
        // Explicitly inactive single-sample controls only: color key, AA,
        // alpha-to-coverage/one, and legacy coverage sampling (Nouveau CSAA).
        // The last register is corroborated by Nouveau, not named by clb197.h.
        // Any enabled/reserved behavior requires separate semantics.
        if matches!(method, 0x1354 | 0x1534 | 0x153c | 0x15b4) {
            return if value == 0 {
                if method == 0x153c {
                    self.alpha_coverage_disabled = true;
                }
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
            } else if method == 0x121c {
                // NVB197_SET_CT_SELECT: count bits3:0, TARGET0 bits6:4,
                // TARGET1..7 bits27:7. Inactive target selectors cannot alter
                // a one-output draw; reserved bits and active remaps still fail.
                if value & !0x0fff_ff80 != 1 {
                    return Err(bad());
                }
            } else if method == 0x1918 {
                if value > 1 {
                    return Err(bad());
                }
                self.cull_enabled = value != 0;
            } else if method == 0x0d7c {
                self.depth_clip = match value {
                    0 => DepthClipRange::NegativeOneToOne,
                    1 => DepthClipRange::ZeroToOne,
                    _ => return Err(bad()),
                };
            } else if method == 0x193c {
                // Two source-verified unclamped XYZ-frustum profiles. Do not
                // extrapolate the unnamed guard/control bits to other modes.
                if !matches!(value, 0x1801 | 0x181d) {
                    return Err(bad());
                }
                self.clip_profile = Some(value);
            } else if method == 0x0a18 {
                if value != *expected {
                    return Err(bad());
                }
                self.viewport_swizzles[0] = Some(value);
            } else if matches!(method, 0x0c08 | 0x0c0c) {
                if value != *expected {
                    return Err(bad());
                }
                self.viewport_depth[0][usize::from((method - 0xc08) / 4)] = Some(value);
            } else if method == 0x12e4 {
                if value > 1 {
                    return Err(bad());
                }
                self.blend_per_target = value != 0;
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
            0x1e00..=0x1efc => {
                let bank = usize::from((method - 0x1e00) / 32);
                let field = usize::from((method - 0x1e00) % 32 / 4);
                let valid = match field {
                    0 => value <= 1,
                    1 | 4 => blend_operation(value).is_some(),
                    2 | 3 | 5 | 6 => factor(value).is_some(),
                    _ => false, // Reserved word, not the following bank.
                };
                if !valid {
                    return Err(bad());
                }
                self.target_blend[bank][field] = Some(value);
            }
            0x0d00..=0x0d3c => {
                let index = usize::from((method - 0xd00) / 8);
                let axis = usize::from((method - 0xd00) % 8 / 4);
                self.window_clip[index][axis] = Some(value);
            }
            0x194c => {
                if value > 1 {
                    return Err(bad());
                }
                self.window_clip_enabled = value != 0;
            }
            0x1950 => {
                if value > 2 {
                    return Err(bad());
                }
                self.window_clip_type = Some(value as u8);
            }
            0x074c => {
                if value > 0xff {
                    return Err(bad());
                }
                self.alpha_fraction = Some(value as u8);
            }
            0x0d9c => {
                if value != 0 {
                    return Err(bad());
                }
                self.reduction_disabled = true;
            }
            0x10cc | 0x10e0..=0x10f0 => {
                let slot = if method == 0x10cc {
                    0
                } else {
                    1 + usize::from((method - 0x10e0) / 4)
                };
                let mask = if slot == 3 { 0x003f003f } else { 0x00ff00ff };
                if value & !mask != 0 {
                    return Err(bad());
                }
                self.reduction_thresholds[slot] = Some(value);
            }
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
                // These are configuration bits, not a clear trigger. Preserve
                // defined encodings; resolve_clear still rejects unsupported
                // execution modes before any attachment operation.
                if value & !0x1111 != 0 {
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
                    2 => DepthFormat::decode(value).is_some(),
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
                if (n == 0 || n == 3) && blend_operation(value).is_none()
                    || n != 0 && n != 3 && factor(value).is_none()
                {
                    return Err(bad());
                }
                self.blend[n] = Some(value);
            }
            0x0850..=0x09d0 if (method - 0x0850) % 0x40 == 0 => {
                // Per-target format selectors are configuration, not rendering.
                // Inactive targets never fetch backing or authorize MRT output.
                if !matches!(value, 0 | 0xd5 | 0xe0) {
                    return Err(bad());
                }
                self.inactive_color_formats[usize::from((method - 0x850) / 0x40)] = Some(value);
            }
            0x0800..=0x0820 => {
                let n = ((method - 0x0800) / 4) as usize;
                let valid = match n {
                    0 => value <= 255,
                    1 => value % 256 == 0,
                    2 => value > 0 && value <= 0x0fffffff,
                    3 => value > 0 && value <= 0x1ffff,
                    4 => matches!(value, 0 | 0xd5 | 0xe0),
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
            0x0a00..=0x0bfc => {
                let slot = usize::from((method - 0xa00) / 32);
                let field = usize::from((method - 0xa00) % 32 / 4);
                match field {
                    0..=5 => {
                        if !f32::from_bits(value).is_finite() {
                            return Err(bad());
                        }
                        self.viewports[slot][field] = Some(value);
                    }
                    6 if value == 0x6420 => self.viewport_swizzles[slot] = Some(value),
                    6 => return Err(bad()),
                    _ => return Ok(false),
                }
            }
            0x0ff4 | 0x0ff8 => self.surface[((method - 0xff4) / 4) as usize] = Some(value),
            0x0c00..=0x0cfc => {
                let slot = usize::from((method - 0xc00) / 16);
                let field = usize::from((method - 0xc00) % 16 / 4);
                match field {
                    0 | 1 => self.clips[slot][field] = Some(value),
                    2 if value == 0 => self.viewport_depth[slot][0] = Some(value),
                    3 if value == 0x3f800000 => self.viewport_depth[slot][1] = Some(value),
                    _ => return Err(bad()),
                }
            }
            0x0e00..=0x0ef8 => {
                let slot = usize::from((method - 0xe00) / 16);
                match method % 16 {
                    0 => {
                        // Supported shaders cannot export a viewport index, so
                        // only slot zero is rasterized. Retain other slots
                        // independently rather than aliasing them onto zero.
                        if value > 1 || (slot == 0 && value != 1) {
                            return Err(bad());
                        }
                        self.scissor_enable[slot] = Some(value);
                    }
                    4 | 8 => self.scissor[slot][usize::from((method % 16 - 4) / 4)] = Some(value),
                    _ => return Ok(false), // Reserved word is not another slot.
                }
            }
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
            format: DepthFormat::decode(self.zt[2].ok_or_else(|| invalid("missing depth format"))?)
                .ok_or_else(|| invalid("unsupported depth format"))?,
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
            *out = v.ok_or_else(|| invalid("incomplete depth target"))?;
        }
        let target = DepthTarget {
            format: DepthFormat::decode(z[2]).ok_or_else(|| invalid("unsupported depth format"))?,
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
    fn validate_reduction_state(&self) -> Result<()> {
        if self.reduction_thresholds.iter().any(Option::is_some) && !self.reduction_disabled {
            return Err(invalid(
                "color reduction thresholds require explicit disabled reduction",
            ));
        }
        Ok(())
    }
    fn validate_window_clip_state(&self) -> Result<()> {
        if self.window_clip_enabled {
            // Exclusive half-open rectangles reject only samples inside their
            // union. Eight explicitly empty rectangles therefore cannot clip,
            // regardless of the unobserved enable bit. Do not infer reset
            // values for missing bounds or the inclusive/exclusive mode.
            let empty_exclusive = self.window_clip_type == Some(1)
                && self.window_clip.iter().all(|axes| {
                    axes.iter().all(Option::is_some)
                        && axes.iter().flatten().any(|v| v & 0xffff >= v >> 16)
                });
            if !empty_exclusive {
                return Err(invalid(&format!(
                    "unsupported active or incomplete window clip state: mode={:?}, rectangles={:x?}",
                    self.window_clip_type, self.window_clip
                )));
            }
        }
        Ok(())
    }
    pub fn resolve(&self) -> Result<(ColorTarget, Rasterization)> {
        self.validate_reduction_state()?;
        self.validate_window_clip_state()?;
        if self.alpha_fraction.is_some() && !self.alpha_coverage_disabled {
            return Err(invalid(
                "alpha fraction requires explicit disabled alpha coverage",
            ));
        }
        if self.fixed != (1u64 << FIXED_RASTER_STATE.len()) - 1 || self.scissor_enable[0] != Some(1)
        {
            return Err(invalid("incomplete or unsupported fixed raster state"));
        }
        if !matches!(self.clip_profile, Some(0x1801 | 0x181d)) {
            return Err(invalid("missing supported unclamped clip profile"));
        }
        let blend = self.resolve_blend()?;
        let color = ColorState {
            blend,
            write_mask: self.write_mask,
        };
        let target = self.color_target()?;
        let mut v = [0f32; 6];
        for (out, word) in v.iter_mut().zip(self.viewports[0]) {
            *out = f32::from_bits(word.ok_or_else(|| invalid("incomplete viewport"))?);
        }
        let [sx, sy, sz, tx, ty, tz] = v;
        let canonical_depth = match self.depth_clip {
            DepthClipRange::ZeroToOne => sz == 1. && tz == 0.,
            DepthClipRange::NegativeOneToOne => sz == 0.5 && tz == 0.5,
        };
        let vp = [tx - sx, ty - sy.abs(), 2. * sx, 2. * sy.abs(), 0., 1.];
        if sx <= 0.
            || sy == 0.
            || !canonical_depth
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
                (self.clips[0][axis], true),
                (self.scissor[0][axis], false),
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
                depth_clip: self.depth_clip,
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
        if c[4] == 0 {
            return Err(invalid("selected color target is disabled"));
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
            format: if c[4] == 0xe0 {
                ColorFormat::Rg11b10Float
            } else {
                ColorFormat::Rgba8
            },
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
        self.validate_reduction_state()?;
        self.validate_window_clip_state()?;
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
        let control = self
            .clear_control
            .ok_or_else(|| invalid("incomplete clear controls"))?;
        // Respecting the stencil mask cannot affect a color/depth-only clear;
        // stencil clear is rejected by the command decoder. Scissor zero is
        // admitted below only when it describes the entire attachment.
        if control & !0x101 != 0 || !color && !depth {
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
                if !out.is_finite()
                    || *out < 0.
                    || (target.format == ColorFormat::Rgba8 && *out > 1.)
                {
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
        if control & 0x100 != 0
            && (self.scissor_enable[0] != Some(1)
                || self.scissor[0] != [Some(extent.0 << 16), Some(extent.1 << 16)])
        {
            return Err(invalid(
                "scissored clear requires explicit full-attachment scissor zero",
            ));
        }
        Ok(ClearSurface { color, depth })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn independent_blend_selects_target_zero_without_inheriting_global_alpha() {
        let mut r = RasterRegisters::default();
        for (m, v) in [
            (0x1360, 1),
            (0x133c, 1),
            (0x1340, 1),
            (0x1344, 5),
            (0x1348, 6),
            (0x134c, 1),
            (0x1350, 2),
            (0x1358, 6),
        ] {
            r.apply(m, v).unwrap();
        }
        let global = r.resolve_blend().unwrap();
        r.apply(0x12e4, 1).unwrap();
        assert!(r.resolve_blend().is_err());
        for (m, v) in [
            (0x1e00, 0),
            (0x1e04, 0x8006),
            (0x1e08, 0x4302),
            (0x1e0c, 0x4303),
        ] {
            r.apply(m, v).unwrap();
        }
        let coupled = r.resolve_blend().unwrap().unwrap();
        assert_eq!(coupled.color, coupled.alpha);
        r.apply(0x1e00, 1).unwrap();
        assert!(r.resolve_blend().is_err());
        for (m, v) in [(0x1e10, 0x8006), (0x1e14, 0x4001), (0x1e18, 0x4303)] {
            r.apply(m, v).unwrap();
        }
        assert_eq!(r.resolve_blend().unwrap(), global);
        for bank in 1..8 {
            for (field, value) in [1, 2, 1, 2, 3, 2, 1].into_iter().enumerate() {
                r.apply(0x1e00 + bank * 32 + field as u16 * 4, value)
                    .unwrap();
            }
            assert_eq!(r.resolve_blend().unwrap(), global);
        }
        r.apply(0x1344, 1).unwrap(); // Inactive global bank changes independently.
        assert_eq!(r.resolve_blend().unwrap(), global);
        r.apply(0x12e4, 0).unwrap();
        assert_ne!(r.resolve_blend().unwrap(), global);
        r.apply(0x12e4, 1).unwrap();
        assert_eq!(r.resolve_blend().unwrap(), global);
        assert_eq!(RasterRegisters::default().target_blend, [[None; 7]; 8]);
    }
    #[test]
    fn independent_blend_rejects_holes_modifiers_and_unaligned_methods_atomically() {
        let mut r = RasterRegisters::default();
        r.apply(0x12e4, 1).unwrap();
        for bank in 0..8 {
            let base = 0x1e00 + bank * 32;
            let values = [1, 1, 5, 6, 1, 2, 6];
            for (field, value) in values.into_iter().enumerate() {
                r.apply(base + field as u16 * 4, value).unwrap();
            }
            for (offset, value) in [
                (0, 2),
                (4, 0),
                (8, 14),
                (12, 16),
                (16, 0x8009),
                (20, 0),
                (24, u32::MAX),
                (28, 0),
                (1, 1),
            ] {
                assert!(r.apply(base + offset, value).is_err());
                assert_eq!(r.target_blend[bank as usize], values.map(Some));
            }
        }
        assert!(r.apply(0x12e4, 2).is_err());
        assert!(r.blend_per_target);
        assert!(!r.apply(0x1f00, 0).unwrap());
        assert_eq!(r.blend, [None; 6]);
        assert_eq!(r.blend_separate, None);
    }
    #[test]
    fn unaligned_direct_method_cannot_alias_register() {
        let mut r = RasterRegisters::default();
        for method in [0x1341, 0x0801, 0x0a01, 0x1355] {
            assert_eq!(r.apply(method, 0), Err(MethodError::InvalidMethod(method)));
        }
        assert_eq!(r.blend, [None; 6]);
        assert_eq!(r.color, [None; 9]);
        assert_eq!(r.viewports, [[None; 6]; 16]);
    }
}
