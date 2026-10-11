//! Bounded adapter around the MIT-licensed Tegra X1 block-linear implementation.
//! One 2D mip only. Compressed formats use block coordinates, not pixel coordinates.
use crate::{Counters, Error, Result, format::Format, invalid};
use tegra_swizzle::{BlockHeight, swizzle::deswizzle_block_linear};

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
        if width == 0 || height == 0 || block_height_log2 > 5 {
            return Err(invalid("invalid Tegra 2D geometry"));
        }
        let (bw, bh, bytes_per_block) = format.block();
        let width_blocks = width.div_ceil(bw);
        let height_blocks = height.div_ceil(bh);
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
}
