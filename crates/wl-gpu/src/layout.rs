//! Bounded adapter around the MIT-licensed Tegra X1 block-linear implementation.
//! One 2D mip only. Compressed formats use block coordinates, not pixel coordinates.
use crate::{Counters, Error, Result, format::Format, invalid};
use tegra_swizzle::{
    BlockHeight,
    swizzle::{deswizzle_block_linear, swizzle_block_linear},
};

const MAX_CONVERSION_BYTES: u64 = 64 * 1024 * 1024;
#[derive(Debug, Clone, Copy)]
pub struct BlockLinear2d {
    width_blocks: u32,
    height_blocks: u32,
    bytes_per_block: u32,
    block_height: BlockHeight,
    pub linear_size: usize,
    pub tiled_size: usize,
}
impl BlockLinear2d {
    pub fn new(width: u32, height: u32, format: Format, block_height_log2: u8) -> Result<Self> {
        let (bw, bh, bytes_per_block) = format.block();
        let width_blocks = width.div_ceil(bw);
        let height_blocks = height.div_ceil(bh);
        Self::from_blocks(
            width_blocks,
            height_blocks,
            bytes_per_block,
            block_height_log2,
        )
    }
    pub(crate) fn new_uncompressed(
        width: u32,
        height: u32,
        bytes_per_texel: u32,
        block_height_log2: u8,
    ) -> Result<Self> {
        if !matches!(bytes_per_texel, 2 | 4) {
            return Err(invalid("unsupported uncompressed surface texel width"));
        }
        Self::from_blocks(width, height, bytes_per_texel, block_height_log2)
    }
    fn from_blocks(
        width_blocks: u32,
        height_blocks: u32,
        bytes_per_block: u32,
        block_height_log2: u8,
    ) -> Result<Self> {
        if width_blocks == 0 || height_blocks == 0 || block_height_log2 > 5 {
            return Err(invalid("invalid Tegra 2D geometry"));
        }
        let rows_per_block = 8_u64 << block_height_log2;
        let width_bytes = u64::from(width_blocks) * u64::from(bytes_per_block);
        let linear_size = width_bytes
            .checked_mul(u64::from(height_blocks))
            .ok_or_else(|| invalid("linear size overflow"))?;
        let tiled_size = width_bytes
            .div_ceil(64)
            .checked_mul(u64::from(height_blocks).div_ceil(rows_per_block))
            .and_then(|n| n.checked_mul(rows_per_block * 64))
            .ok_or_else(|| invalid("tiled size overflow"))?;
        if tiled_size > MAX_CONVERSION_BYTES || linear_size > MAX_CONVERSION_BYTES {
            return Err(invalid("conversion exceeds 64 MiB budget"));
        }
        let block_height = match block_height_log2 {
            0 => BlockHeight::One,
            1 => BlockHeight::Two,
            2 => BlockHeight::Four,
            3 => BlockHeight::Eight,
            4 => BlockHeight::Sixteen,
            _ => BlockHeight::ThirtyTwo,
        };
        Ok(Self {
            width_blocks,
            height_blocks,
            bytes_per_block,
            block_height,
            linear_size: linear_size as usize,
            tiled_size: tiled_size as usize,
        })
    }
    pub fn to_linear(&self, tiled: &[u8], counters: &mut Counters) -> Result<Vec<u8>> {
        if tiled.len() < self.tiled_size {
            return Err(invalid("truncated block-linear mip"));
        }
        let result = deswizzle_block_linear(
            self.width_blocks,
            self.height_blocks,
            1,
            tiled,
            self.block_height,
            self.bytes_per_block,
        )
        .map_err(|e| Error(format!("Tegra layout conversion: {e:?}")))?;
        counters.owned_output_allocations += 1;
        counters.guest_to_host_bytes += self.linear_size as u64;
        counters.texture_conversions += 1;
        Ok(result)
    }
    /// Decode all padded byte coordinates, including incomplete edge GOBs. This
    /// creates a bijective linear working image of the entire storage footprint.
    pub fn padded_linear(&self, tiled: &[u8], counters: &mut Counters) -> Result<Vec<u8>> {
        if tiled.len() != self.tiled_size {
            return Err(invalid("padded surface storage size mismatch"));
        }
        let (width, height) = self.padded_dimensions();
        let result = deswizzle_block_linear(
            width,
            height,
            1,
            tiled,
            self.block_height,
            self.bytes_per_block,
        )
        .map_err(|e| Error(format!("Tegra padded decode: {e:?}")))?;
        counters.temporary_allocations += 1;
        counters.target_deswizzle_bytes += self.tiled_size as u64;
        Ok(result)
    }
    fn padded_dimensions(&self) -> (u32, u32) {
        let row = self.width_blocks * self.bytes_per_block;
        let rows = 8 * self.block_height as u32;
        (
            row.div_ceil(64) * 64 / self.bytes_per_block,
            self.height_blocks.div_ceil(rows) * rows,
        )
    }
    /// Replace only logical rows in a previously decoded padded storage image,
    /// then use the dependency to encode every byte, preserving edge/padding data.
    pub fn from_padded_linear(
        &self,
        padded: &mut [u8],
        linear: &[u8],
        counters: &mut Counters,
    ) -> Result<Vec<u8>> {
        if padded.len() != self.tiled_size || linear.len() != self.linear_size {
            return Err(invalid("linear/padded surface size mismatch"));
        }
        let (width, height) = self.padded_dimensions();
        let row = self.width_blocks as usize * self.bytes_per_block as usize;
        let pitch = width as usize * self.bytes_per_block as usize;
        for (dst, src) in padded.chunks_exact_mut(pitch).zip(linear.chunks_exact(row)) {
            dst[..row].copy_from_slice(src);
        }
        let result = swizzle_block_linear(
            width,
            height,
            1,
            padded,
            self.block_height,
            self.bytes_per_block,
        )
        .map_err(|e| Error(format!("Tegra padded encode: {e:?}")))?;
        counters.temporary_allocations += 1;
        counters.target_swizzle_bytes += self.tiled_size as u64;
        counters.target_pack_copy_bytes += self.linear_size as u64;
        Ok(result)
    }
    pub fn to_tiled_preserving(
        &self,
        linear: &[u8],
        tiled: &[u8],
        counters: &mut Counters,
    ) -> Result<Vec<u8>> {
        let mut padded = self.padded_linear(tiled, counters)?;
        self.from_padded_linear(&mut padded, linear, counters)
    }
}
