//! Serialized GPU-owned channel controller. No Horizon fd/event or syncpoint ABI.
use crate::{
    Error, Result,
    backend::{Capabilities, Frame, Renderer},
    driver::*,
    gpfifo::{Progress, SubmissionFault},
    invalid,
    maxwell::Channel,
    memory::*,
    texture::TextureChanges,
};
use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SubmissionToken {
    channel: ChannelId,
    sequence: u64,
}
#[derive(Debug)]
pub struct SubmissionReport {
    pub token: SubmissionToken,
    pub progress: Progress,
    /// An accepted request can fault after a successfully executed prefix.
    pub fault: Option<SubmissionFault>,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CompletionStatus {
    pub progress: Progress,
    pub faulted: bool,
    pub host_completed: bool,
    /// Conservative: explicit publication covers all earlier channel work.
    pub guest_visible: bool,
}
#[derive(Debug, Default, Clone, Copy)]
pub struct DriverCounters {
    pub metadata_borrowed_bytes: u64,
    pub accepted: u64,
    pub rejected: u64,
    pub completion_queries: u64,
    pub completion_waits: u64,
    pub visibility_barriers: u64,
}
#[derive(Clone)]
struct HostFence {
    index: wgpu::SubmissionIndex,
    done: Arc<AtomicBool>,
}
struct Record {
    token: SubmissionToken,
    status: CompletionStatus,
    host: Option<HostFence>,
}
struct Context {
    id: ChannelId,
    space: AddressSpaceId,
    engine: Channel,
    sequence: u64,
}
pub const MAX_COMPLETIONS: usize = 64;
/// Owns the renderer, mapping registry and canonical dirty journal. The runtime
/// must not mutate/recycle registered backing except through the checked write
/// boundary (or with equivalent exclusive notification). No guest borrows escape.
/// One channel may own pending output. Mapping mutation and channel switching are
/// rejected until publication; these restrictions are stricter than real nvhost.
pub struct GpuDriver {
    renderer: Renderer,
    memory: MemoryManager,
    changes: TextureChanges,
    channels: Vec<Context>,
    records: Vec<Record>,
    active: Option<ChannelId>,
    last_host: Option<HostFence>,
    pending_cpu_writes: u32,
    pub counters: DriverCounters,
}
impl GpuDriver {
    /// Starts with fresh renderer/journal ownership. Importing an already-warmed
    /// renderer could collide with caller-chosen journal epochs, so is not exposed.
    pub fn new() -> Result<Self> {
        let renderer = Renderer::new()?;
        Ok(Self {
            renderer,
            memory: MemoryManager::default(),
            changes: TextureChanges::new(identity()?),
            channels: Vec::with_capacity(16),
            records: Vec::with_capacity(MAX_COMPLETIONS),
            active: None,
            last_host: None,
            pending_cpu_writes: 0,
            counters: DriverCounters::default(),
        })
    }
    pub fn graphics_counters(&self) -> crate::Counters {
        self.renderer.counters
    }
    pub fn capabilities(&self) -> &Capabilities {
        &self.renderer.capabilities
    }
    pub fn has_pending_output(&self) -> bool {
        self.renderer.target_pending
    }
    fn idle_mappings(&self) -> Result<()> {
        if self.renderer.target_pending {
            return Err(invalid(
                "publish pending output before mapping/backing mutation",
            ));
        }
        Ok(())
    }
    pub fn register_backing(&mut self, base: GuestAddress, size: u64) -> Result<BackingId> {
        self.idle_mappings()?;
        let n = usize::try_from(size).map_err(|_| invalid("backing exceeds host"))?;
        self.changes.can_mark_write()?;
        let id = self.memory.register_backing(base, size)?;
        // Reuse of an old canonical address cannot inherit cached contents.
        self.changes.mark_write(base, n)?;
        Ok(id)
    }
    pub fn release_backing(&mut self, backing: BackingId) -> Result<()> {
        self.idle_mappings()?;
        self.memory.release_backing(backing)
    }
    pub fn create_space(&mut self) -> Result<AddressSpaceId> {
        self.memory.create_space()
    }
    pub fn reserve_fixed(
        &mut self,
        space: AddressSpaceId,
        address: GpuAddress,
        size: u64,
    ) -> Result<ReservationId> {
        self.idle_mappings()?;
        self.memory.reserve_fixed(space, address, size)
    }
    pub fn map(
        &mut self,
        space: AddressSpaceId,
        reservation: ReservationId,
        backing: BackingId,
        offset: u64,
    ) -> Result<MappingId> {
        self.idle_mappings()?;
        self.memory.map(space, reservation, backing, offset)
    }
    pub fn unmap(&mut self, space: AddressSpaceId, mapping: MappingId) -> Result<()> {
        self.idle_mappings()?;
        self.memory.unmap(space, mapping)
    }
    pub fn free_reservation(
        &mut self,
        space: AddressSpaceId,
        reservation: ReservationId,
    ) -> Result<()> {
        self.idle_mappings()?;
        self.memory.free_reservation(space, reservation)
    }
    pub fn destroy_space(&mut self, space: AddressSpaceId) -> Result<()> {
        self.idle_mappings()?;
        if self.channels.iter().any(|c| c.space == space) {
            return Err(invalid("address space still bound to channel"));
        }
        self.memory.destroy_space(space)
    }
    pub fn mapping_epoch(&self, space: AddressSpaceId) -> Result<u64> {
        self.memory.mapping_epoch(space)
    }
    pub fn create_channel(&mut self, space: AddressSpaceId) -> Result<ChannelId> {
        self.memory.address_map(space)?;
        if self.channels.len() >= 16 {
            return Err(invalid("channel capacity"));
        }
        let id = ChannelId(identity()?);
        self.channels.push(Context {
            id,
            space,
            engine: Channel::default(),
            sequence: 0,
        });
        Ok(id)
    }
    fn context(&self, channel: ChannelId) -> Result<usize> {
        self.channels
            .iter()
            .position(|c| c.id == channel)
            .ok_or_else(|| invalid("unknown or closed channel"))
    }
    /// Runtime-checked buffers are borrowed only for this synchronous command
    /// acquisition call. Pushbuffers remain borrowed/reacquired by the existing
    /// executor. No metadata or guest pointer is retained for later execution.
    pub fn submit(
        &mut self,
        channel: ChannelId,
        code: u32,
        payload: &[u8],
        extra: Option<&[u8]>,
        memory: &mut impl GuestMemoryWrite,
    ) -> Result<SubmissionReport> {
        let result = self.submit_checked(channel, code, payload, extra, memory);
        if result.is_err() {
            self.counters.rejected += 1;
        }
        result
    }
    fn submit_checked(
        &mut self,
        channel: ChannelId,
        code: u32,
        payload: &[u8],
        extra: Option<&[u8]>,
        memory: &mut impl GuestMemoryWrite,
    ) -> Result<SubmissionReport> {
        let n = self.context(channel)?;
        if self.channels[n].engine.is_faulted() {
            return Err(invalid("channel fault is latched"));
        }
        if self.renderer.target_pending && self.active != Some(channel) {
            return Err(invalid("another channel owns pending output"));
        }
        if self.records.len() >= MAX_COMPLETIONS {
            return Err(invalid(
                "completion capacity; retire visible completed tokens",
            ));
        }
        let request = SubmitRequest::decode(code, payload, extra)?;
        let sequence = self.channels[n]
            .sequence
            .checked_add(1)
            .ok_or_else(|| invalid("channel sequence exhausted"))?;
        // All metadata/identity errors above are preflight, do not consume state.
        self.active = Some(channel);
        self.channels[n].sequence = sequence;
        let token = SubmissionToken { channel, sequence };
        let before = self.renderer.counters.submissions;
        let space = self.channels[n].space;
        let result = self.renderer.submit_gpfifo_deferred(
            &mut self.channels[n].engine,
            request.entries(),
            self.memory.address_map(space)?,
            memory,
            &mut self.changes,
        );
        if self.renderer.counters.submissions != before {
            let index = self
                .renderer
                .last_submission
                .clone()
                .expect("tracked queue submit");
            let done = Arc::new(AtomicBool::new(false));
            let signal = done.clone();
            self.renderer
                .queue
                .on_submitted_work_done(move || signal.store(true, Ordering::Release));
            self.last_host = Some(HostFence { index, done });
        }
        let (progress, fault) = match result {
            Ok(p) => (p, None),
            Err(e) => (e.progress, Some(e)),
        };
        let visible = !self.renderer.target_pending;
        if visible {
            self.pending_cpu_writes = 0;
        }
        self.records.push(Record {
            token,
            status: CompletionStatus {
                progress,
                faulted: fault.is_some(),
                host_completed: self.last_host.is_none(),
                guest_visible: visible,
            },
            host: self.last_host.clone(),
        });
        self.counters.metadata_borrowed_bytes += 24 + request.entries().len() as u64;
        self.counters.accepted += 1;
        Ok(SubmissionReport {
            token,
            progress,
            fault,
        })
    }
    fn record(&self, token: SubmissionToken) -> Result<usize> {
        self.context(token.channel)?;
        self.records
            .iter()
            .position(|r| r.token == token)
            .ok_or_else(|| invalid("unknown or retired completion token"))
    }
    /// Nonblocking host progress. A fast GPU may already be complete on first
    /// query; acceptance never fabricates an initially incomplete GPU status.
    pub fn query(&mut self, token: SubmissionToken) -> Result<CompletionStatus> {
        let n = self.record(token)?;
        self.counters.completion_queries += 1;
        self.renderer
            .device
            .poll(wgpu::PollType::Poll)
            .map_err(|e| Error(e.to_string()))?;
        let r = &self.records[n];
        Ok(CompletionStatus {
            host_completed: r
                .host
                .as_ref()
                .is_none_or(|h| h.done.load(Ordering::Acquire)),
            ..r.status
        })
    }
    /// Wait for this token's queue prefix, never publishes framebuffer bytes.
    pub fn wait(&mut self, token: SubmissionToken) -> Result<CompletionStatus> {
        let n = self.record(token)?;
        if let Some(host) = &self.records[n].host {
            if !host.done.load(Ordering::Acquire) {
                self.counters.completion_waits += 1;
                self.renderer
                    .device
                    .poll(wgpu::PollType::wait_for(host.index.clone()))
                    .map_err(|e| Error(e.to_string()))?;
            }
        }
        self.query(token)
    }
    /// Atomic checked publication is independently retryable, including after a
    /// channel fault. Failure never replays commands or retires the pending image.
    pub fn publish(
        &mut self,
        channel: ChannelId,
        memory: &mut impl GuestMemoryWrite,
    ) -> Result<Option<Frame>> {
        self.context(channel)?;
        if self.renderer.target_pending && self.active != Some(channel) {
            return Err(invalid("publication channel does not own pending output"));
        }
        let frame = if self.active == Some(channel) {
            self.renderer
                .synchronize_target(memory, &mut self.changes)?
        } else {
            None
        };
        for r in &mut self.records {
            if r.token.channel == channel {
                r.status.guest_visible = true;
            }
        }
        self.pending_cpu_writes = 0;
        self.counters.visibility_barriers += u64::from(frame.is_some());
        Ok(frame)
    }
    pub fn retire(&mut self, token: SubmissionToken) -> Result<()> {
        let status = self.query(token)?;
        if !status.host_completed || !status.guest_visible {
            return Err(invalid("completion not yet visible and complete"));
        }
        let n = self.record(token)?;
        self.records.remove(n);
        Ok(())
    }
    /// Performs and records an atomic CPU write, preserving CPU-after-GPU patches
    /// through the existing journal. Bounded pending history must be published
    /// before excessive writes; dropping dirty history is never silently allowed.
    pub fn write_backing(
        &mut self,
        backing: BackingId,
        offset: u64,
        bytes: &[u8],
        memory: &mut impl GuestMemoryWrite,
    ) -> Result<()> {
        let address = self.memory.backing_range(backing, offset, bytes.len())?;
        if self.renderer.target_pending && self.pending_cpu_writes >= 32 {
            return Err(invalid("publish before pending CPU write budget exhausted"));
        }
        self.changes.can_mark_write()?;
        memory.validate_write(address, bytes.len())?;
        memory.write(address, bytes)?;
        self.changes.mark_write(address, bytes.len())?;
        if self.renderer.target_pending {
            self.pending_cpu_writes += 1;
        }
        Ok(())
    }
    /// Close is drain + publish, not cancellation. Failure keeps the channel and
    /// resources available for publication retry. Partial CPU packet state is
    /// discarded only on successful close; submitted work cannot be cancelled.
    pub fn close(&mut self, channel: ChannelId, memory: &mut impl GuestMemoryWrite) -> Result<()> {
        self.context(channel)?;
        if self.renderer.target_pending && self.active != Some(channel) {
            return Err(invalid("close blocked by another channel's pending output"));
        }
        if let Some(token) = self
            .records
            .iter()
            .rev()
            .find(|r| r.token.channel == channel)
            .map(|r| r.token)
        {
            self.wait(token)?;
        }
        self.publish(channel, memory)?;
        let n = self.context(channel)?;
        self.channels.remove(n);
        self.records.retain(|r| r.token.channel != channel);
        if self.active == Some(channel) {
            self.active = None;
        }
        Ok(())
    }
}
