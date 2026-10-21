//! Bounded process-owned registration of canonical GPU backing ranges.
//!
//! These are runtime lifetime guards, not nvmap handles or NVIDIA page tables.
//! Adjacent compatible NORMAL RW regions can be consolidated at registration;
//! subsequent reads borrow one owned region, with no escaped guest borrow.
use crate::{
    Memory, Region, RuntimeResult,
    horizon::{attribute, state},
};
use std::sync::atomic::{AtomicU64, Ordering};

static NEXT_PIN: AtomicU64 = AtomicU64::new(1);
pub const MAX_GPU_PINS: usize = 64;
/// Upper bound for an exceptional consolidation copy, not single-region pins.
pub const MAX_GPU_JOIN_BYTES: u64 = 256 * 1024 * 1024;
/// Opaque process-owned registration; a foreign or released token is invalid.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct GpuPin(u64);
#[derive(Debug)]
pub(crate) struct PinRange {
    token: GpuPin,
    start: u64,
    end: u64,
}
impl Memory {
    /// Rejects any overlap with registered GPU backing, before structural changes.
    /// CPU byte writes remain permitted; their GPU visibility requires the host's
    /// write journal or conservative synchronous acquisition policy.
    pub fn ensure_gpu_unpinned(&self, address: u64, size: u64) -> RuntimeResult<()> {
        let end = address
            .checked_add(size)
            .ok_or("GPU backing range overflow")?;
        if size != 0
            && self
                .gpu_pins
                .iter()
                .any(|p| address < p.end && p.start < end)
        {
            return Err("guest mapping is registered as GPU backing".into());
        }
        Ok(())
    }
    pub(crate) fn gpu_split_blocked(&self, address: u64) -> bool {
        self.gpu_pins
            .iter()
            .any(|p| p.start < address && address < p.end)
    }
    /// Registers a complete, page-aligned canonical span. Copied stack/shared
    /// aliases, locked or reprotected memory and executable pages are unsupported.
    /// A span across compatible adjacent runtime regions is consolidated once;
    /// ordinary single-region registration does not copy the payload.
    ///
    /// Trusted hosts must not bypass this contract by mutating public `regions`
    /// or Region structural fields. No guest borrow may survive an operation.
    pub fn pin_gpu_range(&mut self, address: u64, size: u64) -> RuntimeResult<GpuPin> {
        if size == 0 || address % 4096 != 0 || size % 4096 != 0 {
            return Err("GPU backing must be a nonempty page-aligned range".into());
        }
        if self.gpu_pins.len() >= MAX_GPU_PINS {
            return Err("GPU backing registration capacity".into());
        }
        self.ensure_gpu_unpinned(address, size)?;
        let end = address
            .checked_add(size)
            .ok_or("GPU backing range overflow")?;
        let index = self.region_index(address).ok_or("unmapped GPU backing")?;
        let attributes = self.regions[index].attribute;
        let mut cursor = address;
        for region in &self.regions[index..] {
            if region.base > cursor
                || region.end() <= cursor
                || region.state != state::NORMAL
                || !matches!(region.attribute, 0 | attribute::UNCACHED)
                || region.attribute != attributes
                || !region.read
                || !region.write
                || region.execute
            {
                return Err("GPU backing requires contiguous compatible NORMAL RW storage".into());
            }
            cursor = region.end().min(end);
            if cursor == end {
                break;
            }
        }
        if cursor != end {
            return Err("unmapped GPU backing suffix".into());
        }
        let packed = if end > self.regions[index].end() {
            if size > MAX_GPU_JOIN_BYTES {
                return Err("GPU backing consolidation budget".into());
            }
            let mut bytes = Vec::new();
            bytes
                .try_reserve_exact(size as usize)
                .map_err(|_| "GPU backing consolidation allocation")?;
            for region in &self.regions[index..] {
                let low = address.max(region.base);
                let high = end.min(region.end());
                if low >= high {
                    break;
                }
                bytes.extend_from_slice(
                    &region.bytes[(low - region.base) as usize..(high - region.base) as usize],
                );
                if high == end {
                    break;
                }
            }
            self.regions
                .try_reserve(2)
                .map_err(|_| "GPU backing region metadata allocation")?;
            Some(bytes)
        } else {
            None
        };
        self.gpu_pins
            .try_reserve(1)
            .map_err(|_| "GPU backing pin metadata allocation")?;
        // Globally distinct tokens prevent cross-process confusion, including
        // two Memory values with identical numeric guest addresses.
        let mut current = NEXT_PIN.load(Ordering::Relaxed);
        let id = loop {
            let next = current
                .checked_add(1)
                .ok_or("GPU backing token exhaustion")?;
            match NEXT_PIN.compare_exchange_weak(
                current,
                next,
                Ordering::Relaxed,
                Ordering::Relaxed,
            ) {
                Ok(id) => break id,
                Err(observed) => current = observed,
            }
        };
        if let Some(bytes) = packed {
            // Both boundaries are outside every existing pin by preflight.
            // Keep surrounding regions and consolidate only this exact span.
            self.split_at(address);
            self.split_at(end);
            let first = self.region_index(address).ok_or("lost GPU backing start")?;
            let last = self.regions.partition_point(|r| r.base < end);
            self.regions.splice(
                first..last,
                [Region {
                    base: address,
                    bytes,
                    read: true,
                    write: true,
                    execute: false,
                    state: state::NORMAL,
                    attribute: attributes,
                }],
            );
        }
        let token = GpuPin(id);
        self.gpu_pins.push(PinRange {
            token,
            start: address,
            end,
        });
        Ok(token)
    }
    /// The host must first drain GPU output and remove every dependent GPU
    /// mapping/resource. This call releases only the runtime lifetime guard.
    pub fn release_gpu_pin(&mut self, token: GpuPin) -> RuntimeResult<()> {
        let index = self
            .gpu_pins
            .iter()
            .position(|p| p.token == token)
            .ok_or("unknown GPU backing registration")?;
        self.gpu_pins.remove(index);
        Ok(())
    }
}
