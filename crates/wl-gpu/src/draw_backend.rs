//! Synchronous channel validation entry: B197 state selects every draw resource.
use crate::{
    Result,
    backend::{Frame, Renderer},
    command::Command,
    invalid,
    maxwell::{Channel, Maxwell3d, MethodEffect},
    memory::*,
    texture::TextureChanges,
};
#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) struct GuestTargetStamp {
    guest: GuestAddress,
    size: usize,
    width: u32,
    height: u32,
    version: (u64, u64),
}
impl Renderer {
    /// Consume a channel chunk in order. A complete compact B197 draw executes
    /// synchronously and publishes its framebuffer after submission completion.
    /// The last completed frame is returned. Keep the decoder across split chunks;
    /// call Channel::finish at the actual end of the stream. Any fault latches the
    /// channel until it is replaced; earlier completed draws remain visible.
    pub fn submit_pushbuffer(
        &mut self,
        channel: &mut Channel,
        words: &[u32],
        map: &AddressMap,
        memory: &mut impl GuestMemoryWrite,
        changes: &mut TextureChanges,
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
                        last =
                            Some(self.guest_draw(&channel.engine, effect, map, memory, changes)?);
                    }
                }
            }
            Ok(last)
        })();
        if result.is_err() {
            channel.faulted = true;
            self.vertex_ready = false;
            self.guest_target = None;
        }
        result
    }
    fn guest_draw(
        &mut self,
        engine: &Maxwell3d,
        draw: MethodEffect,
        map: &AddressMap,
        memory: &mut impl GuestMemoryWrite,
        changes: &mut TextureChanges,
    ) -> Result<Frame> {
        self.vertex_ready = false;
        self.hardware_raster = None;
        let (target, raster) = engine.rasterization()?;
        let size = target.size()?;
        let guest = map.resolve(target.address, size)?;
        memory.validate_write(guest, size)?;
        // All source borrows end at queue writes. The synchronous adapter remains
        // exclusively held across submission and the one atomic framebuffer write.
        self.sync_selected_fragment(engine, map, memory, changes)?;
        self.hardware_raster = Some(raster);
        let result = (|| {
            let command = match draw {
                MethodEffect::Draw { first, count } => {
                    self.sync_selected_vertices(engine, map, memory, changes, first, count)?;
                    Command::DrawVertices { first, count }
                }
                MethodEffect::DrawIndices { first, count } => {
                    let vertices = self.sync_indices(engine, map, memory, changes, first, count)?;
                    self.sync_selected_vertices(engine, map, memory, changes, 0, vertices)?;
                    Command::DrawGuestIndices {
                        count,
                        base: engine.index_buffer(first, count)?.base,
                    }
                }
                _ => return Err(invalid("not a draw effect")),
            };
            let stamp = GuestTargetStamp {
                guest,
                size,
                width: target.width,
                height: target.height,
                version: changes.stamp(guest.0..guest.0 + size as u64),
            };
            self.device.push_error_scope(wgpu::ErrorFilter::OutOfMemory);
            self.device.push_error_scope(wgpu::ErrorFilter::Validation);
            let prepared: Result<()> = (|| {
                let old_allocations = self.counters.texture_allocations;
                self.ensure_target(target.width, target.height)?;
                let created = self.counters.texture_allocations != old_allocations;
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
            let frame = self.execute(target.width, target.height, &[command])?;
            // Conservative whole-target generation publication. Marking before
            // the atomic write is safe on failure (extra invalidation, never stale).
            changes.mark_write(guest, size)?;
            memory.write(guest, &frame.rgba)?;
            self.counters.guest_framebuffer_write_bytes += size as u64;
            self.guest_target = Some(GuestTargetStamp {
                version: changes.stamp(guest.0..guest.0 + size as u64),
                ..stamp
            });
            Ok(frame)
        })();
        self.hardware_raster = None;
        if result.is_err() {
            self.vertex_ready = false;
            self.guest_target = None;
        }
        result
    }
}
