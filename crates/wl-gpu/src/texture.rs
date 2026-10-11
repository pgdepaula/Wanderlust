//! Bounded 2D guest surfaces and canonical guest-byte coherency.
use crate::{Result, format::Format, invalid, layout::BlockLinear2d, memory::*};
use std::{collections::VecDeque, ops::Range};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct TextureId(pub u64);
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TextureLayout {
    /// Tightly packed rows and successive mips, without a pitch field.
    Linear,
    BlockLinear {
        block_height_log2: u8,
    },
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Filter {
    Nearest,
    Linear,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TextureDesc {
    pub address: GpuAddress,
    pub width: u32,
    pub height: u32,
    pub mip_count: u32,
    pub format: Format,
    pub layout: TextureLayout,
    /// Clamp-to-edge in both axes; mip selection is explicit in the draw command.
    pub filter: Filter,
}
#[derive(Debug, Clone, Copy)]
pub struct MipLayout {
    pub width: u32,
    pub height: u32,
    pub offset: usize,
    pub source_size: usize,
    pub tiled: Option<BlockLinear2d>,
}
impl TextureDesc {
    /// One layer, depth one. Matches tegra_swizzle's sequential mip packing.
    /// Layer alignment/3D and pitch-linear hardware descriptors are not represented.
    pub fn levels(&self) -> Result<Vec<MipLayout>> {
        let mut levels = Vec::with_capacity(self.mip_count.min(32) as usize);
        self.visit_levels(|level| levels.push(level))?;
        Ok(levels)
    }
    pub(crate) fn validate(&self) -> Result<()> {
        self.visit_levels(|_| {})
    }
    fn visit_levels(&self, mut visit: impl FnMut(MipLayout)) -> Result<()> {
        if self.width == 0
            || self.height == 0
            || self.mip_count == 0
            || self.mip_count > 32 - self.width.max(self.height).leading_zeros()
        {
            return Err(invalid("invalid 2D texture dimensions or mip count"));
        }
        let mut offset = 0_usize;
        for mip in 0..self.mip_count {
            let width = (self.width >> mip).max(1);
            let height = (self.height >> mip).max(1);
            let tiled = match self.layout {
                TextureLayout::Linear => None,
                TextureLayout::BlockLinear { block_height_log2 } => {
                    if block_height_log2 > 5 {
                        return Err(invalid("invalid block height"));
                    }
                    let base = tegra_swizzle::BlockHeight::new(1 << block_height_log2)
                        .ok_or_else(|| invalid("invalid block height"))?;
                    let rows = height.div_ceil(self.format.block().1);
                    let log2 = (tegra_swizzle::mip_block_height(rows, base) as u32).ilog2() as u8;
                    Some(BlockLinear2d::new(width, height, self.format, log2)?)
                }
            };
            let source_size = if let Some(tiled) = tiled {
                tiled.tiled_size
            } else {
                usize::try_from(self.format.layout(width, height, 1)?.size)
                    .map_err(|_| invalid("texture host size overflow"))?
            };
            visit(MipLayout {
                width,
                height,
                offset,
                source_size,
                tiled,
            });
            offset = offset
                .checked_add(source_size)
                .ok_or_else(|| invalid("mip offset overflow"))?;
            if offset > 64 * 1024 * 1024 {
                return Err(invalid("surface exceeds 64 MiB budget"));
            }
        }
        self.address
            .0
            .checked_add(offset as u64)
            .ok_or_else(|| invalid("surface address overflow"))?;
        Ok(())
    }
    pub fn source_size(&self) -> Result<usize> {
        let levels = self.levels()?;
        let last = levels.last().expect("validated mips");
        Ok(last.offset + last.source_size)
    }
}

/// Bounded write journal in canonical guest addresses, shared by all aliased views.
/// The runtime adapter must report every CPU/GPU write and use a fresh epoch when
/// backing/process identity changes. This is testable bookkeeping, not runtime wiring.
pub struct TextureChanges {
    epoch: u64,
    revision: u64,
    floor: u64,
    writes: VecDeque<(u64, Range<u64>)>,
}
impl TextureChanges {
    pub fn new(epoch: u64) -> Self {
        Self {
            epoch,
            revision: 0,
            floor: 0,
            writes: VecDeque::with_capacity(64),
        }
    }
    pub fn mark_write(&mut self, address: GuestAddress, size: usize) -> Result<()> {
        let end = address
            .0
            .checked_add(size as u64)
            .ok_or_else(|| invalid("dirty address overflow"))?;
        if size == 0 {
            return Err(invalid("empty texture write"));
        }
        self.revision = self
            .revision
            .checked_add(1)
            .ok_or_else(|| invalid("dirty revision overflow"))?;
        if self.writes.len() == 64 {
            self.floor = self.writes.pop_front().expect("full journal").0;
        }
        self.writes.push_back((self.revision, address.0..end));
        Ok(())
    }
    /// A floor advance conservatively invalidates older resources after journal eviction.
    pub fn stamp(&self, range: Range<u64>) -> (u64, u64) {
        let revision = self
            .writes
            .iter()
            .filter(|(_, r)| r.start < range.end && range.start < r.end)
            .map(|(v, _)| *v)
            .max()
            .unwrap_or(0)
            .max(self.floor);
        (self.epoch, revision)
    }
}
