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

#[cfg(any(feature = "wgpu-backend", test))]
#[derive(Clone, Copy, Debug)]
pub(crate) enum PendingAttachment {
    Color,
    Depth,
}
#[cfg(any(feature = "wgpu-backend", test))]
impl PendingAttachment {
    fn index(self) -> usize {
        match self {
            Self::Color => 0,
            Self::Depth => 1,
        }
    }
}
#[cfg(any(feature = "wgpu-backend", test))]
#[derive(Clone, Copy)]
struct PendingWrite {
    start: u64,
    end: u64,
}
#[cfg(any(feature = "wgpu-backend", test))]
struct PendingHistory {
    range: Range<u64>,
    baseline: (u64, u64),
    latest: u64,
    writes: [PendingWrite; 32],
    count: usize,
}

/// Bounded canonical write journal shared by aliases. Pending color/depth output
/// retains exact overlapping RAM writes independently of global cache eviction.
/// All writes must be reported; backing/process replacement requires a fresh
/// epoch after pending output is published. This is not runtime memory wiring.
pub struct TextureChanges {
    epoch: u64,
    revision: u64,
    floor: u64,
    writes: VecDeque<(u64, Range<u64>)>,
    #[cfg(any(feature = "wgpu-backend", test))]
    pending: [Option<PendingHistory>; 2],
}

impl TextureChanges {
    pub fn new(epoch: u64) -> Self {
        Self {
            epoch,
            revision: 0,
            floor: 0,
            writes: VecDeque::with_capacity(64),
            #[cfg(any(feature = "wgpu-backend", test))]
            pending: [None, None],
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
    fn checked_range(address: GuestAddress, size: usize) -> Result<Range<u64>> {
        let end = address
            .0
            .checked_add(size as u64)
            .ok_or_else(|| invalid("dirty address overflow"))?;
        if size == 0 {
            return Err(invalid("empty texture write"));
        }
        Ok(address.0..end)
    }
    /// Preflight journal capacity before an atomic CPU memory write. The caller
    /// must retain exclusive memory/journal ownership through mark_write; this
    /// is not an asynchronous reservation or a guest-memory transaction itself.
    pub fn can_mark_write_at(&self, address: GuestAddress, size: usize) -> Result<()> {
        let range = Self::checked_range(address, size)?;
        self.can_mark_writes(1)?;
        #[cfg(any(feature = "wgpu-backend", test))]
        for history in self.pending.iter().flatten() {
            if history.baseline.0 != self.epoch {
                return Err(invalid("pending attachment epoch changed"));
            }
            if range.start < history.range.end
                && history.range.start < range.end
                && history.count == 32
            {
                return Err(invalid(
                    "publish before pending attachment patch budget exhausted",
                ));
            }
        }
        #[cfg(not(any(feature = "wgpu-backend", test)))]
        let _ = range;
        Ok(())
    }
    fn record_global(&mut self, range: Range<u64>) {
        self.revision = self
            .revision
            .checked_add(1)
            .expect("preflighted dirty revision");
        if self.writes.len() == 64 {
            self.floor = self.writes.pop_front().expect("full journal").0;
        }
        self.writes.push_back((self.revision, range));
    }
    pub fn mark_write(&mut self, address: GuestAddress, size: usize) -> Result<()> {
        self.can_mark_write_at(address, size)?;
        let range = Self::checked_range(address, size)?;
        self.record_global(range.clone());
        #[cfg(any(feature = "wgpu-backend", test))]
        for history in self.pending.iter_mut().flatten() {
            if range.start < history.range.end && history.range.start < range.end {
                history.writes[history.count] = PendingWrite {
                    start: range.start.max(history.range.start),
                    end: range.end.min(history.range.end),
                };
                history.count += 1;
                history.latest = self.revision;
            }
        }
        Ok(())
    }
    #[cfg(any(feature = "wgpu-backend", test))]
    pub(crate) fn preflight_gpu_write(
        &self,
        slot: PendingAttachment,
        address: GuestAddress,
        size: usize,
    ) -> Result<()> {
        let range = Self::checked_range(address, size)?;
        self.can_mark_writes(1)?;
        for (n, history) in self.pending.iter().enumerate() {
            if let Some(h) = history {
                if h.baseline.0 != self.epoch {
                    return Err(invalid("pending attachment epoch changed"));
                }
                if n == slot.index() {
                    if h.range != range {
                        return Err(invalid("publish before replacing protected attachment"));
                    }
                } else if range.start < h.range.end && h.range.start < range.end {
                    return Err(invalid("pending attachment storage overlaps"));
                }
            }
        }
        Ok(())
    }
    /// A first GPU write or full clear starts/rebases one attachment. Repeated
    /// ordinary draws must not call this: they retain its CPU-after-GPU patches.
    #[cfg(any(feature = "wgpu-backend", test))]
    pub(crate) fn commit_gpu_write(
        &mut self,
        slot: PendingAttachment,
        address: GuestAddress,
        size: usize,
    ) -> Result<()> {
        self.preflight_gpu_write(slot, address, size)?;
        let range = Self::checked_range(address, size)?;
        self.record_global(range.clone());
        self.pending[slot.index()] = Some(PendingHistory {
            range,
            baseline: (self.epoch, self.revision),
            latest: self.revision,
            writes: [PendingWrite { start: 0, end: 0 }; 32],
            count: 0,
        });
        Ok(())
    }
    #[cfg(any(feature = "wgpu-backend", test))]
    pub(crate) fn preflight_publication(
        &self,
        slot: PendingAttachment,
        address: GuestAddress,
        size: usize,
        baseline: (u64, u64),
    ) -> Result<()> {
        let range = Self::checked_range(address, size)?;
        self.can_mark_writes(1)?;
        let h = self.pending[slot.index()]
            .as_ref()
            .ok_or_else(|| invalid("missing protected attachment history"))?;
        if h.range != range || h.baseline != baseline || baseline.0 != self.epoch {
            return Err(invalid("stale protected attachment baseline"));
        }
        Ok(())
    }
    /// Called only after successful atomic RAM publication. Bypasses this slot's
    /// CPU patch budget and cannot consume another attachment's patch records.
    #[cfg(any(feature = "wgpu-backend", test))]
    pub(crate) fn commit_publication(
        &mut self,
        slot: PendingAttachment,
        address: GuestAddress,
        size: usize,
        baseline: (u64, u64),
    ) -> Result<()> {
        self.preflight_publication(slot, address, size, baseline)?;
        let range = Self::checked_range(address, size)?;
        self.pending[slot.index()] = None;
        self.record_global(range);
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
        if self.epoch != version.0 {
            return Err(invalid("pending target backing epoch changed"));
        }
        if let Some(h) = self.pending.iter().flatten().find(|h| h.range == range) {
            if h.baseline != version {
                return Err(invalid("stale protected attachment baseline"));
            }
            let mut dirty = crate::memory::DirtyRanges::default();
            for r in &h.writes[..h.count] {
                dirty.insert(r.start..r.end)?;
            }
            return Ok(dirty.ranges().to_vec());
        }
        if self.floor > version.1 {
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
        #[cfg(any(feature = "wgpu-backend", test))]
        if let Some(h) = self
            .pending
            .iter()
            .flatten()
            .find(|h| h.range == range && h.baseline.0 == self.epoch)
        {
            return (self.epoch, h.latest);
        }
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

#[cfg(test)]
mod pending_history_tests {
    use super::*;
    use PendingAttachment::{Color, Depth};
    fn begin(c: &mut TextureChanges, slot: PendingAttachment, start: u64) -> (u64, u64) {
        c.commit_gpu_write(slot, GuestAddress(start), 100).unwrap();
        c.stamp(start..start + 100)
    }
    #[test]
    fn exact_patches_and_content_identity_survive_unrelated_eviction() {
        let mut c = TextureChanges::new(7);
        let baseline = begin(&mut c, Color, 100);
        let unprotected = c.stamp(500..600);
        c.mark_write(GuestAddress(90), 20).unwrap();
        c.mark_write(GuestAddress(195), 20).unwrap();
        let latest = c.stamp(100..200);
        for n in 0..4096 {
            c.mark_write(GuestAddress(1000 + n * 4), 4).unwrap();
        }
        assert_eq!(c.stamp(100..200), latest);
        assert_eq!(
            c.writes_since(100..200, baseline).unwrap(),
            vec![100..110, 195..200]
        );
        assert!(c.writes_since(500..600, unprotected).is_err());
        assert!(!c.unchanged_since(500..600, unprotected));
        assert!(c.writes_since(100..200, latest).is_err()); // not the protected baseline
    }
    #[test]
    fn overlap_limit_preflights_before_ram_or_journal_mutation() {
        let mut c = TextureChanges::new(7);
        let baseline = begin(&mut c, Color, 100);
        let mut ram = [0u8; 100];
        for n in 0..32 {
            c.can_mark_write_at(GuestAddress(100 + n), 1).unwrap();
            ram[n as usize] = n as u8;
            c.mark_write(GuestAddress(100 + n), 1).unwrap();
        }
        let before = c.observation();
        let original = ram;
        assert!(c.can_mark_write_at(GuestAddress(150), 1).is_err());
        assert!(c.mark_write(GuestAddress(150), 1).is_err());
        assert_eq!(c.observation(), before);
        assert_eq!(ram, original);
        for _ in 0..4096 {
            c.mark_write(GuestAddress(1000), 4).unwrap();
        }
        // Publication bypasses a full CPU patch budget, then retires protection.
        c.preflight_publication(Color, GuestAddress(100), 100, baseline)
            .unwrap();
        c.commit_publication(Color, GuestAddress(100), 100, baseline)
            .unwrap();
        assert!(c.pending[0].is_none());
        assert_eq!(c.observation().1, before.1 + 4097);
        c.mark_write(GuestAddress(150), 1).unwrap();
        assert!(
            c.preflight_publication(Color, GuestAddress(100), 100, baseline)
                .is_err()
        );
    }
    #[test]
    fn clear_rebases_only_its_attachment_and_rejects_stale_baselines() {
        let mut c = TextureChanges::new(7);
        let old = begin(&mut c, Color, 100);
        let depth = begin(&mut c, Depth, 300);
        c.mark_write(GuestAddress(105), 4).unwrap();
        c.mark_write(GuestAddress(305), 4).unwrap();
        let new = begin(&mut c, Color, 100);
        assert_ne!(new, old);
        assert!(c.writes_since(100..200, old).is_err());
        assert!(
            c.preflight_publication(Color, GuestAddress(100), 100, old)
                .is_err()
        );
        assert!(c.writes_since(100..200, new).unwrap().is_empty());
        assert_eq!(c.writes_since(300..400, depth).unwrap(), vec![305..309]);
        c.mark_write(GuestAddress(110), 4).unwrap();
        assert_eq!(c.writes_since(100..200, new).unwrap(), vec![110..114]);
        let before = c.observation();
        assert!(
            c.preflight_gpu_write(Color, GuestAddress(500), 100)
                .is_err()
        );
        assert!(
            c.preflight_gpu_write(Depth, GuestAddress(150), 100)
                .is_err()
        );
        assert_eq!(c.observation(), before);
    }
    #[test]
    fn partial_publication_keeps_other_history_and_can_retry() {
        let mut c = TextureChanges::new(7);
        let color = begin(&mut c, Color, 100);
        let depth = begin(&mut c, Depth, 300);
        c.mark_write(GuestAddress(110), 4).unwrap();
        c.mark_write(GuestAddress(310), 4).unwrap();
        c.commit_publication(Depth, GuestAddress(300), 100, depth)
            .unwrap();
        assert!(c.pending[1].is_none());
        // A failed atomic color write must not call commit_publication.
        c.preflight_publication(Color, GuestAddress(100), 100, color)
            .unwrap();
        for _ in 0..4096 {
            c.mark_write(GuestAddress(1000), 4).unwrap();
        }
        c.mark_write(GuestAddress(120), 4).unwrap();
        assert_eq!(
            c.writes_since(100..200, color).unwrap(),
            vec![110..114, 120..124]
        );
        assert!(
            c.commit_publication(Color, GuestAddress(100), 100, depth)
                .is_err()
        );
        assert!(c.pending[0].is_some());
        c.commit_publication(Color, GuestAddress(100), 100, color)
            .unwrap();
        assert!(c.pending.iter().all(Option::is_none));
    }
    #[test]
    fn invalid_ranges_epochs_and_revision_overflow_cannot_change_history() {
        let mut c = TextureChanges::new(7);
        let baseline = begin(&mut c, Color, 100);
        let before = c.observation();
        for (address, size) in [(GuestAddress(100), 0), (GuestAddress(u64::MAX), 2)] {
            assert!(c.can_mark_write_at(address, size).is_err());
            assert!(c.preflight_gpu_write(Depth, address, size).is_err());
            assert!(
                c.preflight_publication(Color, address, size, baseline)
                    .is_err()
            );
        }
        assert_eq!(c.observation(), before);
        assert!(c.writes_since(100..200, (7, baseline.1 + 1)).is_err());
        c.epoch = 8;
        assert!(c.mark_write(GuestAddress(1000), 4).is_err());
        assert!(c.writes_since(100..200, baseline).is_err());
        assert!(
            c.preflight_publication(Color, GuestAddress(100), 100, baseline)
                .is_err()
        );
        c.epoch = 7;
        c.revision = u64::MAX;
        assert!(c.mark_write(GuestAddress(1000), 4).is_err());
        assert!(c.commit_gpu_write(Depth, GuestAddress(300), 100).is_err());
        assert!(
            c.commit_publication(Color, GuestAddress(100), 100, baseline)
                .is_err()
        );
        assert_eq!(c.pending[0].as_ref().unwrap().baseline, baseline);
        assert!(c.pending[1].is_none());
    }
}
