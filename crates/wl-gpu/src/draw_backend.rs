//! Ordered channel draws with explicit guest visibility and bounded target residency.
use crate::{
    Result,
    backend::{Frame, Renderer},
    command::Command,
    invalid,
    maxwell::{Channel, Maxwell3d, MethodEffect},
    memory::*,
    texture::{PendingAttachment, TextureChanges},
};
use std::{
    cell::Cell,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
};
#[derive(Clone, Copy)]
pub(crate) struct GuestTargetStamp {
    guest: GuestAddress,
    size: usize,
    surface: crate::raster::SurfaceStorage,
    version: (u64, u64),
}
impl GuestTargetStamp {
    fn same_backing(self, other: Self) -> bool {
        self.guest == other.guest
            && self.size == other.size
            && self.surface.width == other.surface.width
            && self.surface.height == other.surface.height
            && self.surface.layout == other.surface.layout
            && self.surface.format == other.surface.format
    }
}
impl PartialEq for GuestTargetStamp {
    fn eq(&self, other: &Self) -> bool {
        self.same_backing(*other) && self.version == other.version
    }
}
impl Eq for GuestTargetStamp {}
// A pending GPU target is newer than RAM. Cache generations force overlapping
// sources through this guard; an overlap requires a coherence barrier and retry.
struct SourceMemory<'a, M> {
    memory: &'a M,
    pending: [Option<GuestTargetStamp>; 2],
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
            .iter()
            .flatten()
            .any(|p| address.0 < p.guest.0 + p.size as u64 && p.guest.0 < end)
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
            if words.len() as u64 > crate::gpfifo::MAX_WORDS {
                return Err(invalid(
                    "pushbuffer submission exceeds supported word budget",
                ));
            }
            channel
                .engine
                .begin_submission()
                .map_err(|e| crate::Error(e.to_string()))?;
            let mut last = None;
            for word in words {
                {
                    let effect = channel.push_word(*word)?;
                    if matches!(
                        effect,
                        MethodEffect::Draw { .. }
                            | MethodEffect::DrawIndices { .. }
                            | MethodEffect::Clear { .. }
                            | MethodEffect::ConstantWrite { .. }
                            | MethodEffect::InlineUploadStart { .. }
                            | MethodEffect::InlineWrite { .. }
                            | MethodEffect::SemaphoreAcquire { .. }
                            | MethodEffect::SemaphoreRelease { .. }
                            | MethodEffect::WaitIdle
                            | MethodEffect::HostCacheMaintenance
                            | MethodEffect::ShaderCachesInvalidated(_)
                            | MethodEffect::TextureDataInvalidated
                    ) {
                        if let Some(frame) =
                            self.channel_effect(channel, effect, map, memory, changes, synchronous)?
                        {
                            last = Some(frame);
                        }
                    }
                }
            }
            if channel.engine.macro_waiting_for_parameter() {
                return Err(invalid(
                    "unsupported MME parameter continuation across submissions",
                ));
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
        // Publication is atomic per attachment, not across disjoint allocations.
        // Publish depth first; if color fails, depth is already retired and is not
        // copied again on retry. Commands are never replayed.
        self.synchronize_depth(memory, changes)?;
        self.synchronize_color(memory, changes)
    }
    fn synchronize_color(
        &mut self,
        memory: &mut impl GuestMemoryWrite,
        changes: &mut TextureChanges,
    ) -> Result<Option<Frame>> {
        if !self.color_pending {
            return Ok(None);
        }
        let stamp = self
            .guest_target
            .ok_or_else(|| invalid("missing pending target backing"))?;
        memory.validate_write(stamp.guest, stamp.size)?;
        changes.preflight_publication(
            PendingAttachment::Color,
            stamp.guest,
            stamp.size,
            stamp.version,
        )?;
        let dirty = changes.writes_since(
            stamp.guest.0..stamp.guest.0 + stamp.size as u64,
            stamp.version,
        )?;
        let patched = !dirty.is_empty();
        let tiled_layout = stamp.surface.block_linear()?;
        let mut padded = if let Some(layout) = &tiled_layout {
            let bytes = memory.read(stamp.guest, stamp.size)?;
            if bytes.len() != stamp.size {
                return Err(invalid("short padded target read"));
            }
            self.counters.render_target_guest_bytes += stamp.size as u64;
            Some(layout.padded_linear(bytes, &mut self.counters)?)
        } else {
            None
        };
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
            self.counters.target_patch_bytes += size as u64;
            patches.push(((range.start - stamp.guest.0) as usize, patch));
        }
        changes.can_mark_write()?;
        let mut frame = self.read_resident_target()?;
        // CPU writes are guest byte intervals. Overlay them AFTER conversion;
        // a tiled offset is never interpreted as a host row-major pixel offset.
        if let Some(layout) = tiled_layout {
            let mut tiled = layout.from_padded_linear(
                padded.as_mut().unwrap(),
                &frame.rgba,
                &mut self.counters,
            )?;
            for (offset, patch) in patches {
                tiled[offset..offset + patch.len()].copy_from_slice(&patch);
            }
            // Keep the public Frame in logical RGBA coordinates, including CPU patches.
            if patched {
                frame.rgba = layout.to_linear(&tiled, &mut self.counters)?;
                self.counters.target_deswizzle_bytes += frame.rgba.len() as u64;
            }
            memory.write(stamp.guest, &tiled)?;
        } else {
            for (offset, patch) in patches {
                frame.rgba[offset..offset + patch.len()].copy_from_slice(&patch);
            }
            memory.write(stamp.guest, &frame.rgba)?;
        }
        // Failed atomic writes retain the original pending GPU image and journal.
        changes.commit_publication(
            PendingAttachment::Color,
            stamp.guest,
            stamp.size,
            stamp.version,
        )?;
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
        self.color_pending = false;
        self.target_pending = self.depth_pending;
        Ok(Some(frame))
    }
    fn synchronize_depth(
        &mut self,
        memory: &mut impl GuestMemoryWrite,
        changes: &mut TextureChanges,
    ) -> Result<()> {
        if !self.depth_pending {
            return Ok(());
        }
        let stamp = self
            .guest_depth
            .ok_or_else(|| invalid("missing pending depth backing"))?;
        memory.validate_write(stamp.guest, stamp.size)?;
        changes.preflight_publication(
            PendingAttachment::Depth,
            stamp.guest,
            stamp.size,
            stamp.version,
        )?;
        let dirty = changes.writes_since(
            stamp.guest.0..stamp.guest.0 + stamp.size as u64,
            stamp.version,
        )?;
        let patched = !dirty.is_empty();
        let source = memory.read(stamp.guest, stamp.size)?;
        if source.len() != stamp.size {
            return Err(invalid("short depth storage read"));
        }
        // Snapshot padding and exact raw CPU patches before GPU readback.
        let mut original = Vec::new();
        original
            .try_reserve_exact(stamp.size)
            .map_err(|e| crate::Error(e.to_string()))?;
        original.extend_from_slice(source);
        self.counters.temporary_allocations += 1;
        self.counters.depth_guest_bytes += stamp.size as u64;
        changes.can_mark_write()?;
        let linear = self.read_depth()?;
        let layout = stamp
            .surface
            .block_linear()?
            .ok_or_else(|| invalid("depth must be block-linear"))?;
        let mut tiled = layout.to_tiled_preserving(&linear, &original, &mut self.counters)?;
        for range in dirty {
            let start = (range.start - stamp.guest.0) as usize;
            let end = (range.end - stamp.guest.0) as usize;
            tiled[start..end].copy_from_slice(&original[start..end]);
            self.counters.target_patch_bytes += (end - start) as u64;
        }
        memory.write(stamp.guest, &tiled)?;
        changes.commit_publication(
            PendingAttachment::Depth,
            stamp.guest,
            stamp.size,
            stamp.version,
        )?;
        self.counters.depth_write_bytes += stamp.size as u64;
        self.guest_depth = if patched {
            None
        } else {
            Some(GuestTargetStamp {
                version: changes.stamp(stamp.guest.0..stamp.guest.0 + stamp.size as u64),
                ..stamp
            })
        };
        self.depth_pending = false;
        self.target_pending = self.color_pending;
        Ok(())
    }
    pub(crate) fn synchronize_command_source(
        &mut self,
        address: GuestAddress,
        size: usize,
        memory: &mut impl GuestMemoryWrite,
        changes: &mut TextureChanges,
    ) -> Result<()> {
        let end = address
            .0
            .checked_add(size as u64)
            .ok_or_else(|| invalid("command source overflow"))?;
        if self.target_pending
            && [
                self.guest_target.filter(|_| self.color_pending),
                self.guest_depth.filter(|_| self.depth_pending),
            ]
            .iter()
            .flatten()
            .any(|p| address.0 < p.guest.0 + p.size as u64 && p.guest.0 < end)
        {
            self.counters.coherence_barriers += 1;
            self.synchronize_target(memory, changes)?;
        }
        Ok(())
    }
    pub(crate) fn channel_effect(
        &mut self,
        channel: &mut Channel,
        effect: MethodEffect,
        map: &AddressMap,
        memory: &mut impl GuestMemoryWrite,
        changes: &mut TextureChanges,
        synchronous: bool,
    ) -> Result<Option<Frame>> {
        let mut current = Some(effect);
        let mut last = None;
        while let Some(effect) = current {
            if let Some(frame) =
                self.guest_effect(&channel.engine, effect, map, memory, changes, synchronous)?
            {
                last = Some(frame);
            }
            current = channel.resume_macro()?;
        }
        Ok(last)
    }
    pub(crate) fn guest_effect(
        &mut self,
        engine: &Maxwell3d,
        effect: MethodEffect,
        map: &AddressMap,
        memory: &mut impl GuestMemoryWrite,
        changes: &mut TextureChanges,
        synchronous: bool,
    ) -> Result<Option<Frame>> {
        match effect {
            MethodEffect::TextureDataInvalidated => {
                self.invalidate_texture_data();
                Ok(None)
            }
            MethodEffect::HostCacheMaintenance => {
                // Stronger than hardware cache timing: make pending attachment
                // output canonical before later memory reads. No fabricated fence.
                if self.poisoned {
                    return Err(invalid("renderer unavailable after backend failure"));
                }
                if self.target_pending {
                    self.counters.coherence_barriers += 1;
                }
                self.synchronize_target(memory, changes)
            }
            MethodEffect::ShaderCachesInvalidated(flags) => {
                self.invalidate_shader_caches(flags);
                Ok(None)
            }
            MethodEffect::WaitIdle => {
                if self.poisoned {
                    return Err(invalid("renderer unavailable after backend failure"));
                }
                if let Some(index) = self.last_submission.clone() {
                    let done = Arc::new(AtomicBool::new(false));
                    let signal = done.clone();
                    self.queue.on_submitted_work_done(move || {
                        signal.store(true, Ordering::Release);
                    });
                    self.counters.synchronization_points += 1;
                    self.completion
                        .wait(&self.device, index, &done, &mut self.counters)?;
                }
                // Completion is not guest RAM publication. Keep resident color
                // and depth output pending; no readback or draw replay occurs.
                Ok(None)
            }
            MethodEffect::SemaphoreAcquire { address, payload } => {
                if self.poisoned {
                    return Err(invalid("renderer unavailable after backend failure"));
                }
                let source = map.resolve(address, 4)?;
                self.synchronize_command_source(source, 4, memory, changes)?;
                let bytes = memory.read(source, 4)?;
                if bytes.len() != 4 {
                    return Err(invalid("short B06F semaphore read"));
                }
                let actual = u32::from_le_bytes(bytes.try_into().expect("checked semaphore word"));
                if actual != payload {
                    return Err(invalid(
                        "unsatisfied B06F equality acquire requires channel suspension",
                    ));
                }
                Ok(None)
            }
            MethodEffect::SemaphoreRelease { address, payload } => {
                if self.poisoned {
                    return Err(invalid("renderer unavailable after backend failure"));
                }
                let destination = map.resolve(address, 4)?;
                memory.validate_write(destination, 4)?;
                changes.can_mark_write_at(destination, 4)?;
                // WFI enabled means prior actual host GPU work must finish.
                // Publish prior output before making this CPU-visible payload
                // observable, including targets that alias the semaphore.
                self.guest_effect(engine, MethodEffect::WaitIdle, map, memory, changes, false)?;
                if let Some(frame) = self.synchronize_target(memory, changes)? {
                    self.semaphore_frame = Some(frame);
                }
                // Publication can advance the journal; recheck its capacity
                // before the semaphore's own guest mutation.
                changes.can_mark_write_at(destination, 4)?;
                memory.write(destination, &payload.to_le_bytes())?;
                changes.mark_write(destination, 4)?;
                Ok(None)
            }
            MethodEffect::InlineUploadStart { address, length } => {
                if self.poisoned {
                    return Err(invalid("renderer unavailable after backend failure"));
                }
                if length == 0 {
                    return Ok(None); // Empty transfer performs no guest access or flush work.
                }
                let length = length as usize;
                let destination = map.resolve(address, length)?;
                memory.validate_write(destination, length)?;
                changes.can_mark_write_at(destination, length)?;
                self.synchronize_command_source(destination, length, memory, changes)?;
                Ok(None)
            }
            MethodEffect::ConstantWrite { address, value }
            | MethodEffect::InlineWrite { address, value } => {
                if self.poisoned {
                    return Err(invalid("renderer unavailable after backend failure"));
                }
                let destination = map.resolve(address, 4)?;
                memory.validate_write(destination, 4)?;
                self.synchronize_command_source(destination, 4, memory, changes)?;
                changes.can_mark_write_at(destination, 4)?;
                memory.write(destination, &value.to_le_bytes())?;
                changes.mark_write(destination, 4)?;
                if matches!(effect, MethodEffect::InlineWrite { .. }) {
                    self.counters.inline_upload_write_bytes += 4;
                } else {
                    self.counters.constant_buffer_write_bytes += 4;
                }
                Ok(None)
            }
            MethodEffect::Clear { color, depth } => self.guest_clear(
                engine.clear_surface(color, depth)?,
                map,
                memory,
                changes,
                synchronous,
            ),
            _ => self.guest_draw(engine, effect, map, memory, changes, synchronous),
        }
    }
    fn guest_clear(
        &mut self,
        clear: crate::raster::ClearSurface,
        map: &AddressMap,
        memory: &mut impl GuestMemoryWrite,
        changes: &mut TextureChanges,
        synchronous: bool,
    ) -> Result<Option<Frame>> {
        self.vertex_ready = false;
        self.hardware_raster = None;
        if self.poisoned {
            return Err(invalid("renderer unavailable after backend failure"));
        }
        let resolve = |surface: crate::raster::SurfaceStorage| -> Result<GuestTargetStamp> {
            let size = surface.size()?;
            let guest = map.resolve(surface.address, size)?;
            memory.validate_write(guest, size)?;
            Ok(GuestTargetStamp {
                guest,
                size,
                surface,
                version: changes.stamp(guest.0..guest.0 + size as u64),
            })
        };
        let mut color_stamp = clear.color.map(|v| resolve(v.0.storage())).transpose()?;
        let mut depth_stamp = clear.depth.map(|v| resolve(v.0.storage())).transpose()?;
        let overlap = |a: GuestTargetStamp, b: GuestTargetStamp| {
            a.guest.0 < b.guest.0 + b.size as u64 && b.guest.0 < a.guest.0 + a.size as u64
        };
        let effective_color = color_stamp.or(self.guest_target.filter(|_| self.color_pending));
        let effective_depth = depth_stamp.or(self.guest_depth.filter(|_| self.depth_pending));
        if effective_color
            .zip(effective_depth)
            .is_some_and(|(a, b)| overlap(a, b))
        {
            return Err(invalid("overlapping clear color/depth backing unsupported"));
        }
        // A full clear supersedes earlier logical pixels, not a process/mapping epoch.
        for old in [
            self.guest_target.filter(|_| self.color_pending),
            self.guest_depth.filter(|_| self.depth_pending),
        ]
        .into_iter()
        .flatten()
        {
            if old.version.0 != changes.stamp(old.guest.0..old.guest.0 + old.size as u64).0 {
                return Err(invalid("pending clear backing epoch changed"));
            }
        }
        let replaced = color_stamp.is_some_and(|new| {
            self.color_pending && self.guest_target.is_none_or(|old| !old.same_backing(new))
        }) || depth_stamp.is_some_and(|new| {
            self.depth_pending && self.guest_depth.is_none_or(|old| !old.same_backing(new))
        });
        if replaced {
            self.counters.coherence_barriers += 1;
            self.synchronize_target(memory, changes)?;
            for stamp in [&mut color_stamp, &mut depth_stamp].into_iter().flatten() {
                stamp.version = changes.stamp(stamp.guest.0..stamp.guest.0 + stamp.size as u64);
            }
        }
        // Coalesce unchanged pending clears, but invalidate caches again after CPU writes.
        let mark_color =
            color_stamp.is_some_and(|s| !self.color_pending || self.guest_target != Some(s));
        let mark_depth =
            depth_stamp.is_some_and(|s| !self.depth_pending || self.guest_depth != Some(s));
        changes.can_mark_writes(u64::from(mark_color) + u64::from(mark_depth))?;
        if let Some(s) = color_stamp.filter(|_| mark_color) {
            changes.preflight_gpu_write(PendingAttachment::Color, s.guest, s.size)?;
        }
        if let Some(s) = depth_stamp.filter(|_| mark_depth) {
            changes.preflight_gpu_write(PendingAttachment::Depth, s.guest, s.size)?;
        }
        self.device.push_error_scope(wgpu::ErrorFilter::OutOfMemory);
        self.device.push_error_scope(wgpu::ErrorFilter::Validation);
        let prepared: Result<wgpu::CommandBuffer> = (|| {
            if let Some((target, _)) = clear.color {
                let before = self.counters.texture_allocations;
                self.ensure_target(target.width, target.height)?;
                if before != self.counters.texture_allocations {
                    self.counters.render_target_allocations += 1;
                } else {
                    self.counters.render_target_reuses += 1;
                }
            }
            if let Some((target, _)) = clear.depth {
                self.prepare_depth(target.format, target.width, target.height, None)?;
            }
            let mut encoder = self
                .device
                .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                    label: Some("B197 full attachment clear"),
                });
            let attachments = [clear.color.map(|(_, v)| wgpu::RenderPassColorAttachment {
                view: &self.target.as_ref().unwrap().view,
                resolve_target: None,
                depth_slice: None,
                ops: wgpu::Operations {
                    load: wgpu::LoadOp::Clear(wgpu::Color {
                        r: f64::from(v[0]),
                        g: f64::from(v[1]),
                        b: f64::from(v[2]),
                        a: f64::from(v[3]),
                    }),
                    store: wgpu::StoreOp::Store,
                },
            })];
            {
                let _pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                    label: Some("guest CLEAR_SURFACE"),
                    color_attachments: if clear.color.is_some() {
                        &attachments
                    } else {
                        &[]
                    },
                    depth_stencil_attachment: clear.depth.map(|(_, v)| {
                        wgpu::RenderPassDepthStencilAttachment {
                            view: &self.depth.as_ref().unwrap().view,
                            depth_ops: Some(wgpu::Operations {
                                load: wgpu::LoadOp::Clear(v),
                                store: wgpu::StoreOp::Store,
                            }),
                            stencil_ops: None,
                        }
                    }),
                    timestamp_writes: None,
                    occlusion_query_set: None,
                });
            }
            Ok(encoder.finish())
        })();
        let validation = pollster::block_on(self.device.pop_error_scope());
        let allocation = pollster::block_on(self.device.pop_error_scope());
        if let Some(error) = validation.or(allocation) {
            self.poisoned = true;
            return Err(crate::Error(error.to_string()));
        }
        self.last_submission = Some(self.queue.submit([prepared?]));
        self.completion.track(&self.queue);
        self.counters.submissions += 1;
        self.counters.clear_commands += 1;
        if let Some(mut stamp) = color_stamp {
            if mark_color {
                changes.commit_gpu_write(PendingAttachment::Color, stamp.guest, stamp.size)?;
            }
            stamp.version = changes.stamp(stamp.guest.0..stamp.guest.0 + stamp.size as u64);
            self.guest_target = Some(stamp);
            self.color_pending = true;
        }
        if let Some(mut stamp) = depth_stamp {
            if mark_depth {
                changes.commit_gpu_write(PendingAttachment::Depth, stamp.guest, stamp.size)?;
            }
            stamp.version = changes.stamp(stamp.guest.0..stamp.guest.0 + stamp.size as u64);
            self.guest_depth = Some(stamp);
            self.depth_pending = true;
        }
        self.target_pending = self.color_pending || self.depth_pending;
        if synchronous {
            self.synchronize_target(memory, changes)
        } else {
            Ok(None)
        }
    }
    pub(crate) fn guest_draw(
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
            surface: target.storage(),
            version: changes.stamp(guest.0..guest.0 + size as u64),
        };
        let depth_target = engine.depth_target()?;
        let mut depth_stamp = if let Some(d) = depth_target {
            if (d.width, d.height) != (target.width, target.height) {
                return Err(invalid("depth/color dimensions differ"));
            }
            let size = d.size()?;
            let guest = map.resolve(d.address, size)?;
            memory.validate_write(guest, size)?;
            if guest.0 < stamp.guest.0 + stamp.size as u64 && stamp.guest.0 < guest.0 + size as u64
            {
                return Err(invalid("overlapping color/depth attachments unsupported"));
            }
            Some(GuestTargetStamp {
                guest,
                size,
                surface: d.storage(),
                version: changes.stamp(guest.0..guest.0 + size as u64),
            })
        } else {
            None
        };
        let changed_depth = self.depth_pending
            && match (self.guest_depth, depth_stamp) {
                (Some(old), Some(new)) => old != new,
                _ => true,
            };
        if self.target_pending
            && (changed_depth
                || (self.color_pending
                    && self
                        .guest_target
                        .is_none_or(|p| !p.same_backing(stamp) || p.version != stamp.version)))
        {
            self.counters.coherence_barriers += 1;
            self.synchronize_target(memory, changes)?;
            stamp.version = changes.stamp(guest.0..guest.0 + size as u64);
            if let Some(d) = &mut depth_stamp {
                d.version = changes.stamp(d.guest.0..d.guest.0 + d.size as u64);
            }
        }
        self.hardware_raster = Some(raster);
        let result = (|| {
            let mut command = None;
            for attempt in 0..2 {
                let source = SourceMemory {
                    memory: &*memory,
                    pending: [
                        self.guest_target.filter(|_| self.color_pending),
                        self.guest_depth.filter(|_| self.depth_pending),
                    ],
                    hazard: Cell::new(false),
                };
                self.resolving_channel_sources = true;
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
                self.resolving_channel_sources = false;
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
                        if let Some(d) = &mut depth_stamp {
                            d.version = changes.stamp(d.guest.0..d.guest.0 + d.size as u64);
                        }
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
                    let linear;
                    let upload = if let Some(layout) = target.block_linear()? {
                        linear = layout.to_linear(bytes, &mut self.counters)?;
                        self.counters.target_deswizzle_bytes += linear.len() as u64;
                        linear.as_slice()
                    } else {
                        bytes
                    };
                    self.queue.write_texture(
                        wgpu::TexelCopyTextureInfo {
                            texture: &self.target.as_ref().unwrap().texture,
                            mip_level: 0,
                            origin: wgpu::Origin3d::ZERO,
                            aspect: wgpu::TextureAspect::All,
                        },
                        upload,
                        wgpu::TexelCopyBufferLayout {
                            offset: 0,
                            bytes_per_row: Some(target.linear_pitch()?),
                            rows_per_image: Some(target.height),
                        },
                        wgpu::Extent3d {
                            width: target.width,
                            height: target.height,
                            depth_or_array_layers: 1,
                        },
                    );
                    self.counters.render_target_guest_bytes += size as u64;
                    self.counters.render_target_upload_bytes += upload.len() as u64;
                    self.counters.upload_bytes += upload.len() as u64;
                    self.counters.upload_calls += 1;
                    self.counters.staging_allocations += 1;
                }
                if let Some(d) = depth_stamp {
                    let bytes;
                    let upload = if self.guest_depth != Some(d)
                        || self.depth.as_ref().is_none_or(|r| {
                            r.extent != (d.surface.width, d.surface.height)
                                || Some(r.format) != d.surface.depth_format().ok()
                        }) {
                        let tiled = memory.read(d.guest, d.size)?;
                        bytes = d
                            .surface
                            .block_linear()?
                            .unwrap()
                            .to_linear(tiled, &mut self.counters)?;
                        self.counters.depth_guest_bytes += d.size as u64;
                        Some(bytes.as_slice())
                    } else {
                        None
                    };
                    self.prepare_depth(
                        d.surface.depth_format()?,
                        d.surface.width,
                        d.surface.height,
                        upload,
                    )?;
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
            changes.can_mark_writes(
                u64::from(!self.color_pending)
                    + u64::from(
                        depth_stamp.is_some()
                            && raster.depth.is_some_and(|s| s.write)
                            && !self.depth_pending,
                    ),
            )?;
            if !self.color_pending {
                changes.preflight_gpu_write(PendingAttachment::Color, guest, size)?;
            }
            if let Some(d) =
                depth_stamp.filter(|_| raster.depth.is_some_and(|s| s.write) && !self.depth_pending)
            {
                changes.preflight_gpu_write(PendingAttachment::Depth, d.guest, d.size)?;
            }
            self.execute_mode(target.width, target.height, &[command], false)?;
            // Invalidate overlapping source caches once per pending interval.
            // Further GPU writes are ordered and still unreadable through the
            // guard; do not consume the bounded CPU journal on every draw.
            if !self.color_pending {
                changes.commit_gpu_write(PendingAttachment::Color, guest, size)?;
            }
            if let Some(mut d) = depth_stamp {
                if raster.depth.is_some_and(|s| s.write) {
                    if !self.depth_pending {
                        changes.commit_gpu_write(PendingAttachment::Depth, d.guest, d.size)?;
                    }
                    self.depth_pending = true;
                }
                d.version = changes.stamp(d.guest.0..d.guest.0 + d.size as u64);
                self.guest_depth = Some(d);
            } else {
                self.guest_depth = None;
            }
            self.counters.gpu_target_writes += 1;
            stamp.version = changes.stamp(guest.0..guest.0 + size as u64);
            self.guest_target = Some(stamp);
            self.color_pending = true;
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
