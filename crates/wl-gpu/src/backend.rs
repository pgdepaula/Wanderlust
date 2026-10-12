//! Synchronous headless wgpu executor. Borrowed uploads end before submission;
//! mapped readback is accessed only after the submitted work completes.
use crate::{Counters, Error, Result, command::*, format::Format, invalid};
use std::{
    collections::HashMap,
    sync::mpsc,
    time::{Duration, Instant},
};

pub const DRAW_SHADER: &str = include_str!("draw.wgsl");

#[derive(Debug)]
pub struct Capabilities {
    pub adapter: wgpu::AdapterInfo,
    pub supported: wgpu::Features,
    pub enabled: wgpu::Features,
    pub adapter_limits: wgpu::Limits,
    pub device_limits: wgpu::Limits,
}

pub struct Frame {
    pub width: u32,
    pub height: u32,
    pub rgba: Vec<u8>,
    pub encode_time: Duration,
    pub total_time: Duration,
    pub gpu_nanoseconds: Option<f64>,
}
impl Frame {
    /// Validate every pixel, not just a sample; requires an even width.
    pub fn validate_test_pattern(&self) -> Result<()> {
        if self.width == 0
            || self.height == 0
            || self.width % 2 != 0
            || self.rgba.len() != self.width as usize * self.height as usize * 4
        {
            return Err(invalid("invalid pattern extent"));
        }
        for (n, p) in self.rgba.chunks_exact(4).enumerate() {
            let expected = if n % (self.width as usize) < self.width as usize / 2 {
                [255, 0, 0, 255]
            } else {
                [0, 255, 0, 255]
            };
            if p != expected {
                return Err(Error(format!("pixel {n}: {p:?}, expected {expected:?}")));
            }
        }
        Ok(())
    }
}

pub(crate) struct Target {
    extent: (u32, u32),
    pub(crate) texture: wgpu::Texture,
    view: wgpu::TextureView,
    readback: wgpu::Buffer,
    pitch: u32,
    image_size: u64,
}

struct Buffer {
    handle: wgpu::Buffer,
    uploaded: bool,
}

pub struct Renderer {
    pub capabilities: Capabilities,
    pub counters: Counters,
    pub(crate) device: wgpu::Device,
    pub(crate) queue: wgpu::Queue,
    pipeline: wgpu::RenderPipeline,
    default_pipeline: wgpu::RenderPipeline,
    indexed_shader: wgpu::ShaderModule,
    indexed_layout: wgpu::PipelineLayout,
    pub(crate) selected_shader: Option<crate::shader_backend::SelectedShader>,
    pub(crate) resource_fragment: Option<crate::shader_backend::ResidentFragment>,
    pub(crate) vertex: Option<crate::vertex_backend::ResidentVertex>,
    pub(crate) selected_vertex: Option<crate::vertex_backend::SelectedVertex>,
    pub(crate) vertex_ready: bool,
    constant_pipeline: Option<(crate::shader::ConstantFragment, wgpu::RenderPipeline)>,
    buffers: HashMap<BufferId, Buffer>,
    state: ResourceState,
    pub(crate) target: Option<Target>,
    pub(crate) hardware_raster: Option<crate::raster::Rasterization>,
    pub(crate) guest_target: Option<crate::draw_backend::GuestTargetStamp>,
    timestamps: Option<(wgpu::QuerySet, wgpu::Buffer)>,
    pub(crate) poisoned: bool,
    pub(crate) textures:
        HashMap<crate::texture::TextureId, crate::texture_backend::ResidentTexture>,
    pub(crate) texture_pipeline: wgpu::RenderPipeline,
    pub(crate) texture_layout: wgpu::BindGroupLayout,
    pub texture_timings: crate::texture_backend::TextureTimings,
}

impl Renderer {
    /// Enumerates actual native adapters; WGPU_BACKEND may select vulkan, dx12, gl.
    /// No uncommon feature is required. Timestamp queries are enabled if supported.
    pub fn new() -> Result<Self> {
        let backends = match std::env::var("WGPU_BACKEND").as_deref() {
            Ok("vulkan") => wgpu::Backends::VULKAN,
            Ok("dx12") => wgpu::Backends::DX12,
            Ok("gl") => wgpu::Backends::GL,
            Ok(other) => return Err(Error(format!("unsupported WGPU_BACKEND: {other}"))),
            Err(_) => wgpu::Backends::VULKAN | wgpu::Backends::DX12 | wgpu::Backends::GL,
        };
        let instance = wgpu::Instance::new(&wgpu::InstanceDescriptor {
            backends,
            ..Default::default()
        });
        let adapter = pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions {
            power_preference: wgpu::PowerPreference::HighPerformance,
            force_fallback_adapter: false,
            compatible_surface: None,
        }))
        .map_err(|e| Error(format!("no headless adapter: {e}")))?;
        let supported = adapter.features();
        let enabled =
            supported & (wgpu::Features::TIMESTAMP_QUERY | wgpu::Features::TEXTURE_COMPRESSION_BC);
        let adapter_limits = adapter.limits();
        let required_limits =
            wgpu::Limits::downlevel_defaults().using_resolution(adapter_limits.clone());
        let (device, queue) = pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor {
            label: Some("Wanderlust headless GPU"),
            required_features: enabled,
            required_limits,
            memory_hints: wgpu::MemoryHints::MemoryUsage,
            trace: wgpu::Trace::Off,
        }))
        .map_err(|e| Error(format!("device request: {e}")))?;
        let capabilities = Capabilities {
            adapter: adapter.get_info(),
            supported,
            enabled,
            adapter_limits,
            device_limits: device.limits(),
        };
        device.push_error_scope(wgpu::ErrorFilter::Validation);
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("controlled test shader"),
            source: wgpu::ShaderSource::Wgsl(DRAW_SHADER.into()),
        });
        let layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("draw layout"),
            bind_group_layouts: &[],
            push_constant_ranges: &[],
        });
        let pipeline = indexed_pipeline(&device, &layout, &shader, &shader);
        let (texture_layout, texture_pipeline) = crate::texture_backend::pipeline(&device);
        if let Some(e) = pollster::block_on(device.pop_error_scope()) {
            return Err(Error(e.to_string()));
        }
        let timestamps = enabled.contains(wgpu::Features::TIMESTAMP_QUERY).then(|| {
            (
                device.create_query_set(&wgpu::QuerySetDescriptor {
                    label: Some("render timestamps"),
                    ty: wgpu::QueryType::Timestamp,
                    count: 2,
                }),
                device.create_buffer(&wgpu::BufferDescriptor {
                    label: Some("timestamp resolve"),
                    size: 16,
                    usage: wgpu::BufferUsages::QUERY_RESOLVE | wgpu::BufferUsages::COPY_SRC,
                    mapped_at_creation: false,
                }),
            )
        });
        let counters = Counters {
            pipeline_allocations: 2,
            shader_module_allocations: 2,
            buffer_allocations: u64::from(timestamps.is_some()),
            ..Default::default()
        };
        Ok(Self {
            capabilities,
            counters,
            device,
            queue,
            default_pipeline: pipeline.clone(),
            pipeline,
            indexed_shader: shader,
            indexed_layout: layout,
            constant_pipeline: None,
            resource_fragment: None,
            vertex: None,
            selected_vertex: None,
            vertex_ready: false,
            selected_shader: None,
            buffers: HashMap::new(),
            state: ResourceState::new(),
            target: None,
            hardware_raster: None,
            guest_target: None,
            timestamps,
            poisoned: false,
            textures: HashMap::new(),
            texture_pipeline,
            texture_layout,
            texture_timings: Default::default(),
        })
    }

    pub(crate) fn ensure_target(&mut self, width: u32, height: u32) -> Result<()> {
        let limit = self.capabilities.device_limits.max_texture_dimension_2d;
        if width == 0 || height == 0 || width > limit || height > limit {
            return Err(invalid("invalid render extent"));
        }
        if self
            .target
            .as_ref()
            .is_some_and(|t| t.extent == (width, height))
        {
            self.counters.cache_hits += 1;
            return Ok(());
        }
        let layout =
            Format::Rgba8Unorm.layout(width, height, wgpu::COPY_BYTES_PER_ROW_ALIGNMENT)?;
        let readback_size = layout.size + if self.timestamps.is_some() { 16 } else { 0 };
        if readback_size > self.capabilities.device_limits.max_buffer_size {
            return Err(invalid("readback exceeds device buffer limit"));
        }
        let texture = self.device.create_texture(&wgpu::TextureDescriptor {
            label: Some("offscreen rgba8"),
            size: wgpu::Extent3d {
                width,
                height,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Rgba8Unorm,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT
                | wgpu::TextureUsages::COPY_SRC
                | wgpu::TextureUsages::COPY_DST,
            view_formats: &[],
        });
        let view = texture.create_view(&wgpu::TextureViewDescriptor::default());
        let readback = self.device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("aligned offscreen readback"),
            size: readback_size,
            usage: wgpu::BufferUsages::MAP_READ | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        self.target = Some(Target {
            extent: (width, height),
            texture,
            view,
            readback,
            pitch: layout.row_pitch,
            image_size: layout.size,
        });
        self.counters.texture_allocations += 1;
        self.counters.buffer_allocations += 1;
        self.counters.cache_misses += 1;
        Ok(())
    }

    /// Executes a whole validated stream and waits for this submission's readback.
    /// Buffer drop releases the CPU handle; wgpu retains submitted resource references.
    /// No explicit Buffer::destroy is used while encoding/submitted work can refer to it.
    pub fn execute(&mut self, width: u32, height: u32, commands: &[Command<'_>]) -> Result<Frame> {
        if self.hardware_raster.is_none() {
            self.guest_target = None;
        }
        if self.poisoned {
            return Err(invalid("renderer unavailable after backend failure"));
        }
        for command in commands {
            if let Command::DrawVertices { first, count } = command {
                self.validate_vertices(*first, *count)?;
            }
            if matches!(
                command,
                Command::DrawFragment | Command::DrawVertices { .. }
            ) && self.resource_fragment.as_ref().is_none_or(|f| !f.ready)
            {
                return Err(invalid("fragment resources not synchronized"));
            }
            if let Command::DrawTexture { id, mip } = command {
                if self
                    .textures
                    .get(id)
                    .is_none_or(|t| *mip >= t.desc.mip_count)
                {
                    return Err(invalid("unknown texture or invalid sampled mip"));
                }
            }
        }
        let next = validate(
            commands,
            &self.state,
            self.capabilities.device_limits.max_buffer_size,
        )?;
        let layout =
            Format::Rgba8Unorm.layout(width, height, wgpu::COPY_BYTES_PER_ROW_ALIGNMENT)?;
        if width > self.capabilities.device_limits.max_texture_dimension_2d
            || height > self.capabilities.device_limits.max_texture_dimension_2d
            || layout.size + 16 > self.capabilities.device_limits.max_buffer_size
        {
            return Err(invalid("render extent exceeds device limits"));
        }
        self.device.push_error_scope(wgpu::ErrorFilter::OutOfMemory);
        self.device.push_error_scope(wgpu::ErrorFilter::Validation);
        let result = self.execute_validated(width, height, commands, next);
        let validation = pollster::block_on(self.device.pop_error_scope());
        let allocation = pollster::block_on(self.device.pop_error_scope());
        if let Some(error) = validation.or(allocation) {
            self.poisoned = true;
            return Err(Error(format!("GPU backend failure: {error}")));
        }
        if result.is_err() {
            self.poisoned = true;
        }
        result
    }

    fn execute_validated(
        &mut self,
        width: u32,
        height: u32,
        commands: &[Command<'_>],
        next: ResourceState,
    ) -> Result<Frame> {
        let start = Instant::now();
        self.ensure_target(width, height)?;
        let target = self.target.as_ref().expect("validated target");
        let mut encoder = self
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("graphics stream"),
            });
        // One pass per draw supports ordered clears and lifetime-safe drops between passes.
        let mut first = true;
        let mut draws_remaining = commands
            .iter()
            .filter(|c| {
                matches!(
                    c,
                    Command::DrawIndexed { .. }
                        | Command::DrawTexture { .. }
                        | Command::DrawFragment
                        | Command::DrawVertices { .. }
                )
            })
            .count();
        for cmd in commands {
            match cmd {
                Command::CreateBuffer { id, size, kind } => {
                    let usage = wgpu::BufferUsages::COPY_DST
                        | match kind {
                            BufferKind::Vertex => wgpu::BufferUsages::VERTEX,
                            BufferKind::Index => wgpu::BufferUsages::INDEX,
                        };
                    self.buffers.insert(
                        *id,
                        Buffer {
                            handle: self.device.create_buffer(&wgpu::BufferDescriptor {
                                label: Some("translated graphics buffer"),
                                size: *size,
                                usage,
                                mapped_at_creation: false,
                            }),
                            uploaded: false,
                        },
                    );
                    self.counters.buffer_allocations += 1;
                    self.counters.cache_misses += 1;
                }
                Command::Upload { id, offset, data } => {
                    let buffer = self.buffers.get_mut(id).expect("validated upload resource");
                    self.counters.buffer_reuploads += u64::from(buffer.uploaded);
                    self.queue.write_buffer(&buffer.handle, *offset, data);
                    buffer.uploaded = true;
                    self.counters.upload_bytes += data.len() as u64;
                    self.counters.upload_calls += 1;
                    // wgpu 26 documents a new native staging allocation per write_buffer.
                    self.counters.staging_allocations += 1;
                }
                Command::DrawIndexed {
                    vertex,
                    index,
                    index_count,
                } => {
                    let timestamp_writes =
                        self.timestamps
                            .as_ref()
                            .map(|(q, _)| wgpu::RenderPassTimestampWrites {
                                query_set: q,
                                beginning_of_pass_write_index: first.then_some(0),
                                end_of_pass_write_index: (draws_remaining == 1).then_some(1),
                            });
                    let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                        label: Some("indexed draw"),
                        color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                            view: &target.view,
                            resolve_target: None,
                            ops: wgpu::Operations {
                                load: if first {
                                    wgpu::LoadOp::Clear(wgpu::Color::BLACK)
                                } else {
                                    wgpu::LoadOp::Load
                                },
                                store: wgpu::StoreOp::Store,
                            },
                            depth_slice: None,
                        })],
                        depth_stencil_attachment: None,
                        timestamp_writes,
                        occlusion_query_set: None,
                    });
                    pass.set_pipeline(&self.pipeline);
                    pass.set_vertex_buffer(0, self.buffers[vertex].handle.slice(..));
                    pass.set_index_buffer(
                        self.buffers[index].handle.slice(..),
                        wgpu::IndexFormat::Uint16,
                    );
                    pass.draw_indexed(0..*index_count, 0, 0..1);
                    draws_remaining -= 1;
                    first = false;
                    self.counters.cache_hits += 2;
                }
                Command::DrawFragment | Command::DrawVertices { .. } => {
                    let fragment = self.resource_fragment.as_ref().expect("validated fragment");
                    let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                        label: Some("translated Maxwell resource fragment"),
                        color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                            view: &target.view,
                            resolve_target: None,
                            depth_slice: None,
                            ops: wgpu::Operations {
                                load: if first && self.hardware_raster.is_none() {
                                    wgpu::LoadOp::Clear(wgpu::Color::BLACK)
                                } else {
                                    wgpu::LoadOp::Load
                                },
                                store: wgpu::StoreOp::Store,
                            },
                        })],
                        depth_stencil_attachment: None,
                        timestamp_writes: self.timestamps.as_ref().map(|(q, _)| {
                            wgpu::RenderPassTimestampWrites {
                                query_set: q,
                                beginning_of_pass_write_index: first.then_some(0),
                                end_of_pass_write_index: (draws_remaining == 1).then_some(1),
                            }
                        }),
                        occlusion_query_set: None,
                    });
                    if let Command::DrawVertices { first, count } = cmd {
                        let vertex = self.vertex.as_ref().expect("validated vertex stage");
                        pass.set_pipeline(&vertex.pipeline);
                        if let Some(raster) = self.hardware_raster {
                            let v = raster.viewport;
                            pass.set_viewport(v[0], v[1], v[2], v[3], v[4], v[5]);
                            let s = raster.scissor;
                            pass.set_scissor_rect(s[0], s[1], s[2], s[3]);
                        }
                        pass.set_vertex_buffer(
                            0,
                            vertex.buffer.as_ref().expect("uploaded vertices").slice(..),
                        );
                        pass.set_bind_group(0, &fragment.binding, &[]);
                        if let Some(binding) = &fragment.texture_binding {
                            pass.set_bind_group(1, binding, &[]);
                        }
                        pass.draw(*first..*first + *count, 0..1);
                    } else {
                        pass.set_pipeline(&fragment.pipeline);
                        pass.set_bind_group(0, &fragment.binding, &[]);
                        if let Some(binding) = &fragment.texture_binding {
                            pass.set_bind_group(1, binding, &[]);
                        }
                        pass.draw(0..3, 0..1);
                    }
                    draws_remaining -= 1;
                    first = false;
                    self.counters.draw_commands += 1;
                }
                Command::DrawTexture { id, mip } => {
                    let timestamps =
                        self.timestamps
                            .as_ref()
                            .map(|(q, _)| wgpu::RenderPassTimestampWrites {
                                query_set: q,
                                beginning_of_pass_write_index: first.then_some(0),
                                end_of_pass_write_index: (draws_remaining == 1).then_some(1),
                            });
                    let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                        label: Some("sample guest texture"),
                        color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                            view: &target.view,
                            resolve_target: None,
                            depth_slice: None,
                            ops: wgpu::Operations {
                                load: if first {
                                    wgpu::LoadOp::Clear(wgpu::Color::BLACK)
                                } else {
                                    wgpu::LoadOp::Load
                                },
                                store: wgpu::StoreOp::Store,
                            },
                        })],
                        depth_stencil_attachment: None,
                        timestamp_writes: timestamps,
                        occlusion_query_set: None,
                    });
                    pass.set_pipeline(&self.texture_pipeline);
                    pass.set_bind_group(0, &self.textures[id].bindings[*mip as usize], &[]);
                    pass.draw(0..3, 0..1);
                    draws_remaining -= 1;
                    first = false;
                    self.counters.cache_hits += 1;
                }
                Command::DestroyBuffer { id } => {
                    self.buffers.remove(id);
                }
            }
        }
        if first {
            // An empty stream has a deterministic black result too.
            let _pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("empty stream clear"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &target.view,
                    resolve_target: None,
                    depth_slice: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(wgpu::Color::BLACK),
                        store: wgpu::StoreOp::Store,
                    },
                })],
                depth_stencil_attachment: None,
                timestamp_writes: None,
                occlusion_query_set: None,
            });
        }
        encoder.copy_texture_to_buffer(
            wgpu::TexelCopyTextureInfo {
                texture: &target.texture,
                mip_level: 0,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            wgpu::TexelCopyBufferInfo {
                buffer: &target.readback,
                layout: wgpu::TexelCopyBufferLayout {
                    offset: 0,
                    bytes_per_row: Some(target.pitch),
                    rows_per_image: Some(height),
                },
            },
            wgpu::Extent3d {
                width,
                height,
                depth_or_array_layers: 1,
            },
        );
        if !first {
            if let Some((query, resolve)) = &self.timestamps {
                encoder.resolve_query_set(query, 0..2, resolve, 0);
                encoder.copy_buffer_to_buffer(resolve, 0, &target.readback, target.image_size, 16);
            }
        }
        let submission = self.queue.submit([encoder.finish()]);
        self.counters.submissions += 1;
        let encode_time = start.elapsed();
        let (tx, rx) = mpsc::sync_channel(1);
        target
            .readback
            .slice(..)
            .map_async(wgpu::MapMode::Read, move |r| {
                let _ = tx.send(r);
            });
        self.device
            .poll(wgpu::PollType::wait_for(submission))
            .map_err(|e| Error(e.to_string()))?;
        self.counters.synchronization_points += 1;
        rx.recv_timeout(Duration::from_secs(30))
            .map_err(|e| Error(e.to_string()))?
            .map_err(|e| Error(e.to_string()))?;
        let mapped = target.readback.slice(..).get_mapped_range();
        let mut rgba = Vec::new();
        rgba.try_reserve_exact(width as usize * height as usize * 4)
            .map_err(|e| Error(e.to_string()))?;
        for row in mapped[..target.image_size as usize].chunks_exact(target.pitch as usize) {
            rgba.extend_from_slice(&row[..width as usize * 4]);
        }
        let gpu_nanoseconds = if !first && self.timestamps.is_some() {
            let offset = target.image_size as usize;
            let begin = u64::from_le_bytes(mapped[offset..offset + 8].try_into().unwrap());
            let end = u64::from_le_bytes(mapped[offset + 8..offset + 16].try_into().unwrap());
            Some(end.wrapping_sub(begin) as f64 * f64::from(self.queue.get_timestamp_period()))
        } else {
            None
        };
        self.counters.readback_host_copy_bytes += rgba.len() as u64;
        self.counters.owned_output_allocations += 1; // owned packed pixel output only
        self.counters.readback_bytes +=
            target.image_size + if gpu_nanoseconds.is_some() { 16 } else { 0 };
        drop(mapped);
        target.readback.unmap();
        self.state = next;
        Ok(Frame {
            width,
            height,
            rgba,
            encode_time,
            total_time: start.elapsed(),
            gpu_nanoseconds,
        })
    }
}

fn indexed_pipeline(
    device: &wgpu::Device,
    layout: &wgpu::PipelineLayout,
    vertex_shader: &wgpu::ShaderModule,
    fragment_shader: &wgpu::ShaderModule,
) -> wgpu::RenderPipeline {
    let attributes = wgpu::vertex_attr_array![0 => Float32x2, 1 => Float32x4];
    device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
        label: Some("indexed color pipeline"),
        layout: Some(layout),
        vertex: wgpu::VertexState {
            module: vertex_shader,
            entry_point: Some("vs_main"),
            compilation_options: Default::default(),
            buffers: &[wgpu::VertexBufferLayout {
                array_stride: 24,
                step_mode: wgpu::VertexStepMode::Vertex,
                attributes: &attributes,
            }],
        },
        fragment: Some(wgpu::FragmentState {
            module: fragment_shader,
            entry_point: Some("fs_main"),
            compilation_options: Default::default(),
            targets: &[Some(wgpu::ColorTargetState {
                format: wgpu::TextureFormat::Rgba8Unorm,
                blend: None,
                write_mask: wgpu::ColorWrites::ALL,
            })],
        }),
        primitive: wgpu::PrimitiveState::default(),
        depth_stencil: None,
        multisample: wgpu::MultisampleState::default(),
        multiview: None,
        cache: None,
    })
}

impl Renderer {
    /// Install only a successfully decoded constant Maxwell fragment. This does not
    /// execute guest shader-program registers or alter the independent texture path.
    pub fn set_constant_fragment(
        &mut self,
        fragment: crate::shader::ConstantFragment,
    ) -> Result<()> {
        if self.poisoned {
            return Err(invalid("renderer unavailable after backend failure"));
        }
        if let Some((cached, pipeline)) = &self.constant_pipeline {
            if *cached == fragment {
                self.pipeline = pipeline.clone();
                self.counters.shader_cache_hits += 1;
                return Ok(());
            }
        }
        self.device.push_error_scope(wgpu::ErrorFilter::OutOfMemory);
        self.device.push_error_scope(wgpu::ErrorFilter::Validation);
        let shader = self
            .device
            .create_shader_module(wgpu::ShaderModuleDescriptor {
                label: Some("translated constant Maxwell fragment"),
                source: wgpu::ShaderSource::Wgsl(fragment.wgsl().into()),
            });
        let pipeline = indexed_pipeline(
            &self.device,
            &self.indexed_layout,
            &self.indexed_shader,
            &shader,
        );
        let validation = pollster::block_on(self.device.pop_error_scope());
        let allocation = pollster::block_on(self.device.pop_error_scope());
        if let Some(error) = validation.or(allocation) {
            self.poisoned = true;
            return Err(Error(format!("translated shader backend failure: {error}")));
        }
        self.counters.shader_module_allocations += 1;
        self.counters.pipeline_allocations += 1;
        self.pipeline = pipeline.clone();
        self.constant_pipeline = Some((fragment, pipeline));
        Ok(())
    }
    /// Restore the original interpolated-color pipeline without compiling a shader.
    pub fn restore_default_fragment(&mut self) -> Result<()> {
        if self.poisoned {
            return Err(invalid("renderer unavailable after backend failure"));
        }
        self.pipeline = self.default_pipeline.clone();
        Ok(())
    }
}
