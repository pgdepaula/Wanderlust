//! Explicit block geometry and transfer pitch; no implicit format reinterpretation.
use crate::{Result, invalid};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Format {
    Rgba8Unorm,
    Rgba8Srgb,
    Bc1Rgba,
    Bc1RgbaSrgb,
    Bc3Rgba,
    Astc4x4,
    Astc4x4Srgb,
}
impl Format {
    pub fn block(self) -> (u32, u32, u32) {
        match self {
            Self::Rgba8Unorm | Self::Rgba8Srgb => (1, 1, 4),
            Self::Bc1Rgba | Self::Bc1RgbaSrgb => (4, 4, 8),
            Self::Bc3Rgba | Self::Astc4x4 | Self::Astc4x4Srgb => (4, 4, 16),
        }
    }
    pub fn layout(self, width: u32, height: u32, alignment: u32) -> Result<LinearLayout> {
        if width == 0 || height == 0 || !alignment.is_power_of_two() {
            return Err(invalid("invalid extent or row alignment"));
        }
        let (bw, bh, bytes) = self.block();
        let row_bytes = width
            .div_ceil(bw)
            .checked_mul(bytes)
            .ok_or_else(|| invalid("row overflow"))?;
        let row_pitch = row_bytes
            .checked_add(alignment - 1)
            .ok_or_else(|| invalid("pitch overflow"))?
            & !(alignment - 1);
        let rows = height.div_ceil(bh);
        let size = u64::from(row_pitch)
            .checked_mul(u64::from(rows))
            .ok_or_else(|| invalid("texture overflow"))?;
        Ok(LinearLayout {
            row_bytes,
            row_pitch,
            rows,
            size,
        })
    }
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LinearLayout {
    pub row_bytes: u32,
    pub row_pitch: u32,
    pub rows: u32,
    pub size: u64,
}
