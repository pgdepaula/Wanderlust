//! Ordered channel draws with explicit guest visibility and bounded target residency.
use crate::{
    Result,
    backend::{Frame, Renderer},
    command::Command,
    invalid,
    maxwell::{Channel, Maxwell3d, MethodEffect},
    memory::*,
    texture::TextureChanges,
};
use std::cell::Cell;
#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) struct GuestTargetStamp {
    guest: GuestAddress,
    size: usize,
    width: u32,
    height: u32,
    version: (u64, u64),
}
impl GuestTargetStamp {
    fn same_backing(self, other: Self) -> bool {
        self.guest == other.guest
            && self.size == other.size
            && self.width == other.width
            && self.height == other.height
    }
}
// A pending GPU target is newer than RAM. Cache generations force overlapping
// sources through this guard; an overlap requires a coherence barrier and retry.
struct SourceMemory<'a, M> {
    memory: &'a M,
    pending: Option<GuestTargetStamp>,
    hazard: Cell<bool>,
}
impl<M: GuestMemory> GuestMemory for SourceMemory<'_, M> {
    fn read(&self, address: GuestAddress, size: usize) -> Result<&[u8]> {
        let end = address
            .0
            .checked_add(size as u64)
            .ok_or_else(|| invalid("source read overflow"))?;
        if self
            .pending
            .is_some_and(|p| address.0 < p.guest.0 + p.size as u64 && p.guest.0 < end)
        {
            self.hazard.set(true);
            return Err(invalid("source depends on pending GPU output"));
        }
        self.memory.read(address, size)
    }
}
impl Renderer {
    /// Compatibility entry: every completed draw is read back and atomically
    /// published before returning; earlier draws survive a subsequent channel fault.
    pub fn submit_pushbuffer(
        &mut self,
        channel: &mut Channel,
        words: &[u32],
        map: &AddressMap,
        memory: &mut impl GuestMemoryWrite,
        changes: &mut TextureChanges,
    ) -> Result<Option<Frame>> {
        self.consume_pushbuffer(channel, words, map, memory, changes, true)
    }
    /// Submit hardware draws in queue order without waiting for CPU visibility.
    /// Returns the number of draws accepted in this chunk. Completion/readback is
    /// explicit via synchronize_target. Rebinding, CPU writes and source feedback
    /// may require a conditional coherence barrier, never an unconditional per-draw
    /// readback. This is not a Horizon fence or nvhost syncpoint implementation.
    pub fn submit_pushbuffer_deferred(
        &mut self,
        channel: &mut Channel,
        words: &[u32],
        map: &AddressMap,
        memory: &mut impl GuestMemoryWrite,
        changes: &mut TextureChanges,
    ) -> Result<u64> {
        let before = self.counters.draw_commands;
        self.consume_pushbuffer(channel, words, map, memory, changes, false)?;
        Ok(self.counters.draw_commands - before)
    }
    fn consume_pushbuffer(
        &mut self,
        channel: &mut Channel,
        words: &[u32],
        map: &AddressMap,
        memory: &mut impl GuestMemoryWrite,
        changes: &mut TextureChanges,
        synchronous: bool,
    ) -> Result<Option<Frame>> {
        self.vertex_ready = false;
        if channel.faulted {
            return Err(invalid("GPU channel fault is latched"));
        }
        let result = (|| {
            let mut last = None;
            for word in words {
                if let Some(method) = channel
                    .fifo
                    .push_word(*word)
                    .map_err(|e| crate::Error(e.to_string()))?
                {
                    let effect = channel
                        .engine
                        .apply(method)
                        .map_err(|e| crate::Error(e.to_string()))?;
                    if matches!(
                        effect,
                        MethodEffect::Draw { .. } | MethodEffect::DrawIndices { .. }
                    ) {
                        last = self.guest_draw(
                            &channel.engine,
                            effect,
                            map,
                            memory,
                            changes,
                            synchronous,
                        )?;
                    }
                }
            }
            Ok(last)
        })();
        if result.is_err() {
            channel.faulted = true;
            self.vertex_ready = false;
            if !self.target_pending {
                self.guest_target = None;
            }
        }
        result
    }
    /// Complete pending draws and atomically expose the latest framebuffer. CPU
    /// writes declared since the last draw are merged by exact byte range. A lost
    /// journal or changed epoch is rejected before writeback, never guessed.
    /// Works after a channel fault so earlier submitted draws remain observable.
    pub fn synchronize_target(
        &mut self,
        memory: &mut impl GuestMemoryWrite,
        changes: &mut TextureChanges,
    ) -> Result<Option<Frame>> {
        if !self.target_pending {
            return Ok(None);
        }
        let stamp = self
            .guest_target
            .ok_or_else(|| invalid("missing pending target backing"))?;
        memory.validate_write(stamp.guest, stamp.size)?;
        let dirty = changes.writes_since(
            stamp.guest.0..stamp.guest.0 + stamp.size as u64,
            stamp.version,
        )?;
        let patched = !dirty.is_empty();
        let mut patches = Vec::new();
        for range in dirty {
            let size = (range.end - range.start) as usize;
            let bytes = memory.read(GuestAddress(range.start), size)?;
            if bytes.len() != size {
                return Err(invalid("short CPU target patch"));
            }
            let mut patch = Vec::new();
            patch
                .try_reserve_exact(size)
                .map_err(|e| crate::Error(e.to_string()))?;
            patch.extend_from_slice(bytes);
            self.counters.temporary_allocations += 1;
            self.counters.guest_to_host_bytes += size as u64;
            patches.push(((range.start - stamp.guest.0) as usize, patch));
        }
        changes.can_mark_write()?;
        let mut frame = self.read_resident_target()?;
        for (offset, patch) in patches {
            frame.rgba[offset..offset + patch.len()].copy_from_slice(&patch);
        }
        // An unsuccessful atomic write must not become a false CPU dirty patch.
        memory.write(stamp.guest, &frame.rgba)?;
        changes.mark_write(stamp.guest, stamp.size)?;
        self.counters.guest_framebuffer_write_bytes += stamp.size as u64;
        // If patches changed the GPU readback, RAM is now newer than the resident
        // image: clear the stamp to force initialization on the next draw.
        self.guest_target = if patched {
            None
        } else {
            Some(GuestTargetStamp {
                version: changes.stamp(stamp.guest.0..stamp.guest.0 + stamp.size as u64),
                ..stamp
            })
        };
        self.target_pending = false;
        Ok(Some(frame))
    }
    fn guest_draw(
        &mut self,
        engine: &Maxwell3d,
        draw: MethodEffect,
        map: &AddressMap,
        memory: &mut impl GuestMemoryWrite,
        changes: &mut TextureChanges,
        synchronous: bool,
    ) -> Result<Option<Frame>> {
        self.vertex_ready = false;
        self.hardware_raster = None;
        let (target, raster) = engine.rasterization()?;
        let size = target.size()?;
        let guest = map.resolve(target.address, size)?;
        memory.validate_write(guest, size)?;
        let mut stamp = GuestTargetStamp {
            guest,
            size,
            width: target.width,
            height: target.height,
            version: changes.stamp(guest.0..guest.0 + size as u64),
        };
        if self.target_pending
            && self
                .guest_target
                .is_none_or(|p| !p.same_backing(stamp) || p.version != stamp.version)
        {
            self.counters.coherence_barriers += 1;
            self.synchronize_target(memory, changes)?;
            stamp.version = changes.stamp(guest.0..guest.0 + size as u64);
        }
        self.hardware_raster = Some(raster);
        let result = (|| {
            let mut command = None;
            for attempt in 0..2 {
                let source = SourceMemory {
                    memory: &*memory,
                    pending: self.guest_target.filter(|_| self.target_pending),
                    hazard: Cell::new(false),
                };
                let resolved = (|| {
                    self.sync_selected_fragment(engine, map, &source, changes)?;
                    match draw {
                        MethodEffect::Draw { first, count } => {
                            self.sync_selected_vertices(
                                engine, map, &source, changes, first, count,
                            )?;
                            Ok(Command::DrawVertices { first, count })
                        }
                        MethodEffect::DrawIndices { first, count } => {
                            let vertices =
                                self.sync_indices(engine, map, &source, changes, first, count)?;
                            self.sync_selected_vertices(
                                engine, map, &source, changes, 0, vertices,
                            )?;
                            Ok(Command::DrawGuestIndices {
                                count,
                                base: engine.index_buffer(first, count)?.base,
                            })
                        }
                        _ => Err(invalid("not a draw effect")),
                    }
                })();
                let hazard = source.hazard.get();
                match resolved {
                    Ok(c) => {
                        command = Some(c);
                        break;
                    }
                    Err(_) if hazard && attempt == 0 => {
                        self.counters.coherence_barriers += 1;
                        self.synchronize_target(memory, changes)?;
                        stamp.version = changes.stamp(guest.0..guest.0 + size as u64);
                    }
                    Err(e) => return Err(e),
                }
            }
            let command = command.ok_or_else(|| invalid("unresolved draw sources"))?;
            self.device.push_error_scope(wgpu::ErrorFilter::OutOfMemory);
            self.device.push_error_scope(wgpu::ErrorFilter::Validation);
            let prepared: Result<()> = (|| {
                let old = self.counters.texture_allocations;
                self.ensure_target(target.width, target.height)?;
                let created = self.counters.texture_allocations != old;
                if created {
                    self.counters.render_target_allocations += 1;
                } else {
                    self.counters.render_target_reuses += 1;
                }
                if created || self.guest_target != Some(stamp) {
                    let bytes = memory.read(guest, size)?;
                    if bytes.len() != size {
                        return Err(invalid("short guest target upload"));
                    }
                    self.queue.write_texture(
                        wgpu::TexelCopyTextureInfo {
                            texture: &self.target.as_ref().unwrap().texture,
                            mip_level: 0,
                            origin: wgpu::Origin3d::ZERO,
                            aspect: wgpu::TextureAspect::All,
                        },
                        bytes,
                        wgpu::TexelCopyBufferLayout {
                            offset: 0,
                            bytes_per_row: Some(target.pitch),
                            rows_per_image: Some(target.height),
                        },
                        wgpu::Extent3d {
                            width: target.width,
                            height: target.height,
                            depth_or_array_layers: 1,
                        },
                    );
                    self.counters.render_target_guest_bytes += size as u64;
                    self.counters.render_target_upload_bytes += size as u64;
                    self.counters.upload_bytes += size as u64;
                    self.counters.upload_calls += 1;
                    self.counters.staging_allocations += 1;
                }
                Ok(())
            })();
            let validation = pollster::block_on(self.device.pop_error_scope());
            let allocation = pollster::block_on(self.device.pop_error_scope());
            if let Some(error) = validation.or(allocation) {
                self.poisoned = true;
                return Err(crate::Error(error.to_string()));
            }
            prepared?;
            // Each draw is submitted before the next queue write. Deferring all
            // submits would make earlier draws see later buffer/constant updates.
            changes.can_mark_write()?;
            self.execute_mode(target.width, target.height, &[command], false)?;
            // Invalidate overlapping source caches once per pending interval.
            // Further GPU writes are ordered and still unreadable through the
            // guard; do not consume the bounded CPU journal on every draw.
            if !self.target_pending {
                changes.mark_write(guest, size)?;
            }
            self.counters.gpu_target_writes += 1;
            stamp.version = changes.stamp(guest.0..guest.0 + size as u64);
            self.guest_target = Some(stamp);
            self.target_pending = true;
            if synchronous {
                self.synchronize_target(memory, changes)
            } else {
                Ok(None)
            }
        })();
        self.hardware_raster = None;
        if result.is_err() {
            self.vertex_ready = false;
            if !self.target_pending {
                self.guest_target = None;
            }
        }
        result
    }
}
