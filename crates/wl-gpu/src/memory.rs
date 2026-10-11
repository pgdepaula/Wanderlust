//! GPU addresses resolve to guest addresses, never host pointers.
use crate::{Result, invalid};
use std::ops::Range;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct GuestAddress(pub u64);
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct GpuAddress(pub u64);

/// Read permission and contiguous-borrow policy belong to the runtime adapter.
/// A returned borrow cannot outlive the backing or cross asynchronous submission.
pub trait GuestMemory {
    fn read(&self, address: GuestAddress, length: usize) -> Result<&[u8]>;
}

/// Independent test adapter, not a replacement for the process memory manager.
pub struct SliceMemory<'a> {
    pub base: GuestAddress,
    pub bytes: &'a [u8],
}
impl GuestMemory for SliceMemory<'_> {
    fn read(&self, address: GuestAddress, length: usize) -> Result<&[u8]> {
        address
            .0
            .checked_add(length as u64)
            .ok_or_else(|| invalid("guest address overflow"))?;
        let offset = address
            .0
            .checked_sub(self.base.0)
            .ok_or_else(|| invalid("unmapped guest address"))?;
        let start = usize::try_from(offset).map_err(|_| invalid("host offset overflow"))?;
        let end = start
            .checked_add(length)
            .ok_or_else(|| invalid("guest length overflow"))?;
        self.bytes
            .get(start..end)
            .ok_or_else(|| invalid("unmapped guest range"))
    }
}

#[derive(Debug, Clone)]
struct Mapping {
    gpu: Range<u64>,
    guest: GuestAddress,
}
#[derive(Debug, Default)]
pub struct AddressMap {
    mappings: Vec<Mapping>,
}
impl AddressMap {
    /// GPU overlaps are rejected; distinct GPU ranges may alias the same guest backing.
    pub fn map(&mut self, gpu: GpuAddress, guest: GuestAddress, size: u64) -> Result<()> {
        let end = gpu
            .0
            .checked_add(size)
            .ok_or_else(|| invalid("GPU address overflow"))?;
        guest
            .0
            .checked_add(size)
            .ok_or_else(|| invalid("guest address overflow"))?;
        if size == 0
            || self
                .mappings
                .iter()
                .any(|m| gpu.0 < m.gpu.end && m.gpu.start < end)
        {
            return Err(invalid("empty or overlapping GPU mapping"));
        }
        self.mappings.push(Mapping {
            gpu: gpu.0..end,
            guest,
        });
        Ok(())
    }
    /// Cross-mapping reads require a runtime-owned gather operation, not an unsafe borrow.
    pub fn resolve(&self, gpu: GpuAddress, size: usize) -> Result<GuestAddress> {
        let end = gpu
            .0
            .checked_add(size as u64)
            .ok_or_else(|| invalid("GPU range overflow"))?;
        let m = self
            .mappings
            .iter()
            .find(|m| m.gpu.start <= gpu.0 && gpu.0 < m.gpu.end && end <= m.gpu.end)
            .ok_or_else(|| invalid("unmapped or noncontiguous GPU range"))?;
        Ok(GuestAddress(m.guest.0 + (gpu.0 - m.gpu.start)))
    }
    pub fn read<'a>(
        &self,
        memory: &'a impl GuestMemory,
        gpu: GpuAddress,
        size: usize,
    ) -> Result<&'a [u8]> {
        memory.read(self.resolve(gpu, size)?, size)
    }
}

/// Sorted, coalesced intervals. Caller owns dirty acknowledgment after successful upload.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct DirtyRanges {
    ranges: Vec<Range<u64>>,
}
impl DirtyRanges {
    pub fn insert(&mut self, range: Range<u64>) -> Result<()> {
        if range.start >= range.end {
            return Err(invalid("empty or reversed dirty range"));
        }
        self.ranges.push(range);
        self.ranges.sort_unstable_by_key(|r| r.start);
        let mut count = 0;
        for i in 0..self.ranges.len() {
            if count > 0 && self.ranges[i].start <= self.ranges[count - 1].end {
                self.ranges[count - 1].end = self.ranges[count - 1].end.max(self.ranges[i].end);
            } else {
                self.ranges[count] = self.ranges[i].clone();
                count += 1;
            }
        }
        self.ranges.truncate(count);
        Ok(())
    }
    pub fn ranges(&self) -> &[Range<u64>] {
        &self.ranges
    }
    pub fn covers(&self, range: Range<u64>) -> bool {
        range.start == range.end
            || self
                .ranges
                .iter()
                .any(|r| r.start <= range.start && range.end <= r.end)
    }
    pub fn clear(&mut self) {
        self.ranges.clear();
    }
}
