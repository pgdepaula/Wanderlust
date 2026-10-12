//! Bounded guest index scanning and persistent host index resources.
use crate::{
    Result, backend::Renderer, invalid, maxwell::Maxwell3d, memory::*, texture::TextureChanges,
};
#[derive(Clone, Copy, PartialEq, Eq)]
struct Stamp {
    guest: GuestAddress,
    size: usize,
    width: u8,
    version: (u64, u64),
}
pub(crate) struct ResidentIndex {
    pub buffer: wgpu::Buffer,
    pub format: wgpu::IndexFormat,
    stamp: Stamp,
    allocated: u64,
    min: u32,
    max: u32,
    pub count: u32,
    pub base: i32,
    pub vertices: u32,
}
impl Renderer {
    pub(crate) fn sync_indices(
        &mut self,
        engine: &Maxwell3d,
        map: &AddressMap,
        memory: &impl GuestMemory,
        changes: &TextureChanges,
        first: u32,
        count: u32,
    ) -> Result<u32> {
        let source = engine.index_buffer(first, count)?;
        let guest = map.resolve(source.address, source.bytes)?;
        let stamp = Stamp {
            guest,
            size: source.bytes,
            width: source.width,
            version: changes.stamp(guest.0..guest.0 + source.bytes as u64),
        };
        let cached = self.indices.as_ref().filter(|r| r.stamp == stamp);
        let bytes = if cached.is_none() {
            let bytes = memory.read(guest, source.bytes)?;
            if bytes.len() != source.bytes {
                return Err(invalid("short guest index read"));
            }
            self.counters.index_guest_bytes += bytes.len() as u64;
            Some(bytes)
        } else {
            None
        };
        let (min, max) = if let Some(r) = cached {
            self.counters.index_cache_hits += 1;
            (r.min, r.max)
        } else {
            let bytes = bytes.unwrap();
            let mut min = u32::MAX;
            let mut max = 0;
            for b in bytes.chunks_exact(source.width as usize) {
                let n = if source.width == 2 {
                    u32::from(u16::from_le_bytes(b.try_into().unwrap()))
                } else {
                    u32::from_le_bytes(b.try_into().unwrap())
                };
                min = min.min(n);
                max = max.max(n);
            }
            (min, max)
        };
        let lower = i64::from(min) + i64::from(source.base);
        let upper = i64::from(max) + i64::from(source.base);
        let vertices =
            u32::try_from(upper + 1).map_err(|_| invalid("indexed vertex range overflow"))?;
        if lower < 0 {
            return Err(invalid("negative indexed vertex reference"));
        }
        // Validate the referenced vertex span before uploading any index data.
        engine.vertex_array()?.span(0, vertices)?;
        if cached.is_none() {
            let bytes = bytes.unwrap();
            let allocated = (source.bytes as u64 + 3) & !3;
            if allocated > self.capabilities.device_limits.max_buffer_size {
                return Err(invalid("index resource exceeds device limit"));
            }
            let buffer = if let Some(r) = self.indices.as_ref().filter(|r| r.allocated == allocated)
            {
                r.buffer.clone()
            } else {
                self.counters.buffer_allocations += 1;
                self.device.create_buffer(&wgpu::BufferDescriptor {
                    label: Some("guest index stream"),
                    size: allocated,
                    usage: wgpu::BufferUsages::INDEX | wgpu::BufferUsages::COPY_DST,
                    mapped_at_creation: false,
                })
            };
            if allocated == bytes.len() as u64 {
                self.queue.write_buffer(&buffer, 0, bytes);
            } else {
                let mut padded = Vec::new();
                padded
                    .try_reserve_exact(allocated as usize)
                    .map_err(|e| crate::Error(e.to_string()))?;
                padded.extend_from_slice(bytes);
                padded.resize(allocated as usize, 0);
                self.queue.write_buffer(&buffer, 0, &padded);
                self.counters.temporary_allocations += 1;
                self.counters.guest_to_host_bytes += bytes.len() as u64;
            }
            self.counters.index_upload_bytes += allocated;
            self.counters.upload_bytes += allocated;
            self.counters.upload_calls += 1;
            self.counters.staging_allocations += 1;
            self.indices = Some(ResidentIndex {
                buffer,
                format: if source.width == 2 {
                    wgpu::IndexFormat::Uint16
                } else {
                    wgpu::IndexFormat::Uint32
                },
                stamp,
                allocated,
                min,
                max,
                count,
                base: source.base,
                vertices,
            });
        }
        let r = self.indices.as_mut().unwrap();
        r.count = count;
        r.base = source.base;
        r.vertices = vertices;
        Ok(vertices)
    }
    pub(crate) fn validate_indices(&self, count: u32, base: i32) -> Result<()> {
        let r = self
            .indices
            .as_ref()
            .ok_or_else(|| invalid("missing guest index stream"))?;
        if count != r.count || base != r.base {
            return Err(invalid("stale guest index selection"));
        }
        self.validate_vertex_span(0, r.vertices)
    }
}
