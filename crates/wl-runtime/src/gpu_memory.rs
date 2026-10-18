//! Bounded process-owned registration of canonical GPU backing ranges.
//!
//! These are runtime lifetime guards, not nvmap handles or NVIDIA page tables.
//! The initial adapter supports one plain, contiguous NORMAL RW region per pin.
use crate::{Memory, RuntimeResult, horizon::state};
use std::sync::atomic::{AtomicU64, Ordering};

static NEXT_PIN: AtomicU64 = AtomicU64::new(1);
pub const MAX_GPU_PINS: usize = 64;
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
    /// aliases, locked or reprotected memory, executable pages and gathers are
    /// deliberately unsupported. Registration does not copy the payload.
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
        let region = &self.regions[index];
        if end > region.end()
            || region.state != state::NORMAL
            || region.attribute != 0
            || !region.read
            || !region.write
            || region.execute
        {
            return Err("GPU backing requires one plain NORMAL RW region".into());
        }
        // Globally distinct tokens prevent cross-process confusion, including
        // two Memory values with identical numeric guest addresses.
        let id = NEXT_PIN
            .fetch_update(Ordering::Relaxed, Ordering::Relaxed, |v| v.checked_add(1))
            .map_err(|_| "GPU backing token exhaustion")?;
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
