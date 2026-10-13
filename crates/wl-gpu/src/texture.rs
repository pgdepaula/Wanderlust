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
    #[cfg(feature = "wgpu-backend")]
    pub(crate) fn can_mark_write(&self) -> Result<()> {
        self.can_mark_writes(1)
    }
    pub(crate) fn can_mark_writes(&self, count: u64) -> Result<()> {
        self.revision
            .checked_add(count)
            .ok_or_else(|| invalid("dirty revision overflow"))?;
        Ok(())
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
    /// Exact subsequent writes for preserving CPU changes over pending GPU output.
    /// Evicted history or a new backing epoch cannot be merged safely.
    #[cfg(any(feature = "wgpu-backend", test))]
    pub(crate) fn writes_since(
        &self,
        range: Range<u64>,
        version: (u64, u64),
    ) -> Result<Vec<Range<u64>>> {
        if self.epoch != version.0 || self.floor > version.1 {
            return Err(invalid(
                "pending target dirty history unavailable; synchronize before epoch change or journal eviction",
            ));
        }
        let mut dirty = crate::memory::DirtyRanges::default();
        for (v, r) in &self.writes {
            if *v > version.1 && r.start < range.end && range.start < r.end {
                dirty.insert(r.start.max(range.start)..r.end.min(range.end))?;
            }
        }
        Ok(dirty.ranges().to_vec())
    }
    /// Global observation cursor for caches checked frequently enough to retain history.
    /// This is not a content identity and must not replace pending-output patch stamps.
    #[cfg(any(feature = "wgpu-backend", test))]
    pub(crate) fn observation(&self) -> (u64, u64) {
        (self.epoch, self.revision)
    }
    /// Prove that a source has not changed since a prior observation without allocating.
    /// Missing history, a new epoch, and a future cursor all require a fresh source read.
    #[cfg(any(feature = "wgpu-backend", test))]
    pub(crate) fn unchanged_since(&self, range: Range<u64>, observed: (u64, u64)) -> bool {
        observed.0 == self.epoch
            && self.floor <= observed.1
            && observed.1 <= self.revision
            && !self
                .writes
                .iter()
                .any(|(v, r)| *v > observed.1 && r.start < range.end && range.start < r.end)
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

#[cfg(test)]
mod journal_tests {
    use super::*;
    #[test]
    fn frequently_observed_sources_survive_unrelated_history_eviction() {
        let mut c = TextureChanges::new(4);
        let mut observed = c.observation();
        for n in 0..1024 {
            c.mark_write(GuestAddress(1000 + n), 1).unwrap();
            assert!(c.unchanged_since(100..200, observed));
            observed = c.observation();
        }
        c.mark_write(GuestAddress(200), 1).unwrap();
        c.mark_write(GuestAddress(99), 1).unwrap();
        assert!(c.unchanged_since(100..200, observed));
        c.mark_write(GuestAddress(199), 2).unwrap();
        assert!(!c.unchanged_since(100..200, observed));
        // An overlapping write remains a change even if later writes are unrelated.
        c.mark_write(GuestAddress(1000), 1).unwrap();
        assert!(!c.unchanged_since(100..200, observed));
        let current = c.observation();
        assert!(c.unchanged_since(100..200, current));
        assert!(!c.unchanged_since(100..200, (5, current.1)));
        assert!(!c.unchanged_since(100..200, (4, current.1 + 1)));
        for _ in 0..65 {
            c.mark_write(GuestAddress(1000), 1).unwrap();
        }
        assert!(!c.unchanged_since(100..200, current));
        // Exact pending-output patch retrieval still rejects evicted history.
        assert!(c.writes_since(100..200, current).is_err());
    }
    #[test]
    fn exact_patch_union_and_history_bounds() {
        let mut c = TextureChanges::new(4);
        let version = c.stamp(100..200);
        c.mark_write(GuestAddress(90), 20).unwrap();
        c.mark_write(GuestAddress(105), 25).unwrap();
        c.mark_write(GuestAddress(195), 20).unwrap();
        assert_eq!(
            c.writes_since(100..200, version).unwrap(),
            vec![100..130, 195..200]
        );
        assert!(c.writes_since(100..200, (5, 0)).is_err());
        let now = c.stamp(100..200);
        assert!(c.writes_since(100..200, now).unwrap().is_empty());
        for _ in 0..65 {
            c.mark_write(GuestAddress(1000), 4).unwrap();
        }
        assert!(c.writes_since(100..200, version).is_err());
    }
}
