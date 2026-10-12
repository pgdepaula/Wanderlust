use crate::{Error, Result, backend::Renderer, format::Format, invalid, memory::*, texture::*};
use std::{
    borrow::Cow,
    ops::Range,
    time::{Duration, Instant},
};

pub(crate) struct ResidentTexture {
    pub desc: TextureDesc,
    pub host_format: wgpu::TextureFormat,
    pub texture: wgpu::Texture,
    pub bindings: Vec<wgpu::BindGroup>,
    levels: Vec<MipLayout>,
    stamps: Vec<(Range<u64>, (u64, u64))>,
}
#[derive(Debug, Default, Clone, Copy)]
pub struct TextureTimings {
    pub source: Duration,
    pub deswizzle: Duration,
    pub upload: Duration,
    pub decompress: Duration,
}
pub(crate) fn pipeline(device: &wgpu::Device) -> (wgpu::BindGroupLayout, wgpu::RenderPipeline) {
    let bindings = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
        label: Some("sampled guest texture"),
        entries: &[
            wgpu::BindGroupLayoutEntry {
                binding: 0,
                visibility: wgpu::ShaderStages::FRAGMENT,
                ty: wgpu::BindingType::Texture {
                    sample_type: wgpu::TextureSampleType::Float { filterable: true },
                    view_dimension: wgpu::TextureViewDimension::D2,
                    multisampled: false,
                },
                count: None,
            },
            wgpu::BindGroupLayoutEntry {
                binding: 1,
                visibility: wgpu::ShaderStages::FRAGMENT,
                ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                count: None,
            },
        ],
    });
    let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
        label: Some("texture sampling"),
        source: wgpu::ShaderSource::Wgsl(include_str!("texture.wgsl").into()),
    });
    let layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
        label: Some("texture pipeline layout"),
        bind_group_layouts: &[&bindings],
        push_constant_ranges: &[],
    });
    let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
        label: Some("guest texture sample"),
        layout: Some(&layout),
        vertex: wgpu::VertexState {
            module: &shader,
            entry_point: Some("vs_main"),
            compilation_options: Default::default(),
            buffers: &[],
        },
        fragment: Some(wgpu::FragmentState {
            module: &shader,
            entry_point: Some("fs_main"),
            compilation_options: Default::default(),
            targets: &[Some(wgpu::ColorTargetState {
                format: wgpu::TextureFormat::Rgba8Unorm,
                blend: None,
                write_mask: wgpu::ColorWrites::ALL,
            })],
        }),
        primitive: Default::default(),
        depth_stencil: None,
        multisample: Default::default(),
        multiview: None,
        cache: None,
    });
    (bindings, pipeline)
}

/// Auto uses native BC1 for block-aligned base extents on a capable adapter.
/// ASTC currently always uses the validated LDR software subset.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TextureUploadPolicy {
    Auto,
    Software,
}

impl Renderer {
    pub fn texture_host_format(&self, id: TextureId) -> Result<wgpu::TextureFormat> {
        self.textures
            .get(&id)
            .map(|t| t.host_format)
            .ok_or_else(|| invalid("unknown texture"))
    }

    /// Synchronize changed whole mips. Guest writes are canonical addresses, so
    /// aliases invalidate together. No wait is inserted: queue writes precede draws.
    pub fn sync_texture(
        &mut self,
        id: TextureId,
        desc: TextureDesc,
        map: &AddressMap,
        memory: &impl GuestMemory,
        changes: &TextureChanges,
    ) -> Result<()> {
        self.sync_texture_with_policy(id, desc, map, memory, changes, TextureUploadPolicy::Auto)
    }

    pub fn sync_texture_with_policy(
        &mut self,
        id: TextureId,
        desc: TextureDesc,
        map: &AddressMap,
        memory: &impl GuestMemory,
        changes: &TextureChanges,
        policy: TextureUploadPolicy,
    ) -> Result<()> {
        self.validate_guest_resource_access()?;
        if self.poisoned {
            return Err(invalid("renderer unavailable after backend failure"));
        }
        let native_bc = policy == TextureUploadPolicy::Auto
            && self
                .capabilities
                .enabled
                .contains(wgpu::Features::TEXTURE_COMPRESSION_BC)
            && desc.width % 4 == 0
            && desc.height % 4 == 0;
        let host_format = match desc.format {
            Format::Rgba8Unorm => wgpu::TextureFormat::Rgba8Unorm,
            Format::Rgba8Srgb => wgpu::TextureFormat::Rgba8UnormSrgb,
            Format::Bc1Rgba if native_bc => wgpu::TextureFormat::Bc1RgbaUnorm,
            Format::Bc1RgbaSrgb if native_bc => wgpu::TextureFormat::Bc1RgbaUnormSrgb,
            Format::Bc1Rgba | Format::Astc4x4 => wgpu::TextureFormat::Rgba8Unorm,
            Format::Bc1RgbaSrgb | Format::Astc4x4Srgb => wgpu::TextureFormat::Rgba8UnormSrgb,
            _ => return Err(invalid("compressed format sampling not implemented")),
        };
        let fallback = desc.format.block().0 > 1 && !host_format.is_compressed();
        if desc.width > self.capabilities.device_limits.max_texture_dimension_2d
            || desc.height > self.capabilities.device_limits.max_texture_dimension_2d
        {
            return Err(invalid("texture exceeds device limits"));
        }
        let existing = self
            .textures
            .get(&id)
            .filter(|t| t.desc == desc && t.host_format == host_format);
        let levels = match existing {
            Some(t) => Cow::Borrowed(t.levels.as_slice()),
            None => Cow::Owned(desc.levels()?),
        };
        if fallback {
            let mut total = 0u64;
            for m in levels.iter() {
                total = total
                    .checked_add(Format::Rgba8Unorm.layout(m.width, m.height, 1)?.size)
                    .ok_or_else(|| invalid("decoded surface size overflow"))?;
            }
            if total > 64 * 1024 * 1024 {
                return Err(invalid("decoded surface exceeds 64 MiB budget"));
            }
        }
        let fingerprint = |m: &MipLayout| -> Result<(Range<u64>, (u64, u64))> {
            let gpu = GpuAddress(
                desc.address
                    .0
                    .checked_add(m.offset as u64)
                    .ok_or_else(|| invalid("mip address overflow"))?,
            );
            let guest = map.resolve(gpu, m.source_size)?;
            let end = guest
                .0
                .checked_add(m.source_size as u64)
                .ok_or_else(|| invalid("mip backing overflow"))?;
            let range = guest.0..end;
            Ok((range.clone(), changes.stamp(range)))
        };
        let mut changed = false;
        for (n, m) in levels.iter().enumerate() {
            let stamp = fingerprint(m)?;
            changed |= existing.is_none_or(|t| t.stamps[n] != stamp);
        }
        if !changed {
            self.counters.texture_cache_hits += 1;
            return Ok(());
        }

        // Complete source validation/conversion before allocating or mutating GPU data.
        let mut uploads = Vec::new();
        let mut stamps = Vec::with_capacity(levels.len());
        for (n, m) in levels.iter().enumerate() {
            let stamp = fingerprint(m)?;
            if existing.is_none_or(|t| t.stamps[n] != stamp) {
                let start = Instant::now();
                let raw = memory.read(GuestAddress(stamp.0.start), m.source_size)?;
                if raw.len() != m.source_size {
                    return Err(invalid("guest adapter returned a short texture read"));
                }
                self.texture_timings.source += start.elapsed();
                self.counters.guest_texture_bytes += raw.len() as u64;
                let data = if let Some(tiled) = m.tiled {
                    let start = Instant::now();
                    let data = tiled.to_linear(raw, &mut self.counters)?;
                    self.texture_timings.deswizzle += start.elapsed();
                    self.counters.deswizzle_bytes += data.len() as u64;
                    self.counters.temporary_allocations += 1;
                    Cow::Owned(data)
                } else {
                    Cow::Borrowed(raw)
                };
                let data = if fallback {
                    let start = Instant::now();
                    let rgba =
                        crate::compression::decode_rgba8(desc.format, m.width, m.height, &data)?;
                    self.texture_timings.decompress += start.elapsed();
                    self.counters.decompress_bytes += rgba.len() as u64;
                    self.counters.temporary_allocations += 1;
                    Cow::Owned(rgba)
                } else {
                    data
                };
                uploads.push((n as u32, data));
            }
            stamps.push(stamp);
        }
        let new_levels = levels.into_owned();
        let replace = existing.is_none();
        self.device.push_error_scope(wgpu::ErrorFilter::OutOfMemory);
        self.device.push_error_scope(wgpu::ErrorFilter::Validation);
        let start = Instant::now();
        if replace {
            let texture = self.device.create_texture(&wgpu::TextureDescriptor {
                label: Some("guest 2D texture"),
                size: wgpu::Extent3d {
                    width: desc.width,
                    height: desc.height,
                    depth_or_array_layers: 1,
                },
                mip_level_count: desc.mip_count,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format: host_format,
                usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
                view_formats: &[],
            });
            let filter = match desc.filter {
                Filter::Nearest => wgpu::FilterMode::Nearest,
                Filter::Linear => wgpu::FilterMode::Linear,
            };
            let sampler = self.device.create_sampler(&wgpu::SamplerDescriptor {
                label: Some("guest clamp sampler"),
                mag_filter: filter,
                min_filter: filter,
                address_mode_u: wgpu::AddressMode::ClampToEdge,
                address_mode_v: wgpu::AddressMode::ClampToEdge,
                ..Default::default()
            });
            let bindings = (0..desc.mip_count)
                .map(|mip| {
                    let view = texture.create_view(&wgpu::TextureViewDescriptor {
                        base_mip_level: mip,
                        mip_level_count: Some(1),
                        ..Default::default()
                    });
                    self.device.create_bind_group(&wgpu::BindGroupDescriptor {
                        label: Some("guest mip binding"),
                        layout: &self.texture_layout,
                        entries: &[
                            wgpu::BindGroupEntry {
                                binding: 0,
                                resource: wgpu::BindingResource::TextureView(&view),
                            },
                            wgpu::BindGroupEntry {
                                binding: 1,
                                resource: wgpu::BindingResource::Sampler(&sampler),
                            },
                        ],
                    })
                })
                .collect();
            self.textures.insert(
                id,
                ResidentTexture {
                    desc,
                    host_format,
                    texture,
                    bindings,
                    levels: new_levels,
                    stamps: Vec::new(),
                },
            );
            self.counters.texture_allocations += 1;
            self.counters.bind_group_allocations += u64::from(desc.mip_count);
            self.counters.texture_cache_misses += 1;
        }
        let resident = self.textures.get_mut(&id).expect("texture allocated");
        for (mip, data) in uploads {
            let level = resident.levels[mip as usize];
            let layout = if fallback {
                Format::Rgba8Unorm
            } else {
                desc.format
            }
            .layout(level.width, level.height, 1)?;
            self.queue.write_texture(
                wgpu::TexelCopyTextureInfo {
                    texture: &resident.texture,
                    mip_level: mip,
                    origin: wgpu::Origin3d::ZERO,
                    aspect: wgpu::TextureAspect::All,
                },
                &data,
                wgpu::TexelCopyBufferLayout {
                    offset: 0,
                    bytes_per_row: Some(layout.row_bytes),
                    rows_per_image: Some(layout.rows),
                },
                wgpu::Extent3d {
                    width: level.width,
                    height: level.height,
                    depth_or_array_layers: 1,
                }
                .physical_size(host_format),
            );
            self.counters.upload_bytes += data.len() as u64;
            self.counters.texture_uploads += 1;
            self.counters.staging_allocations += 1;
            self.counters.upload_calls += 1;
            self.counters.dirty_mip_uploads += u64::from(!replace);
        }
        self.texture_timings.upload += start.elapsed();
        let validation = pollster::block_on(self.device.pop_error_scope());
        let allocation = pollster::block_on(self.device.pop_error_scope());
        if let Some(error) = validation.or(allocation) {
            self.poisoned = true;
            return Err(Error(format!("texture backend failure: {error}")));
        }
        // Acknowledgment means owned queue staging accepted, not GPU completion.
        resident.stamps = stamps;
        Ok(())
    }

    /// Drops CPU handles only; encoded/submitted references remain owned by wgpu.
    pub fn remove_texture(&mut self, id: TextureId) -> Result<()> {
        self.textures
            .remove(&id)
            .ok_or_else(|| invalid("unknown texture"))?;
        Ok(())
    }
}
