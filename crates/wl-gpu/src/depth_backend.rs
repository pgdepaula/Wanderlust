//! Portable ZF32/Z16 transfers: GLES does not guarantee depth buffer copies.
//! Integer color textures carry ZF32 bits or Z16 codes around initialization and
//! export passes. Testing and writes occur in the native depth attachment.
use crate::{
    Result,
    backend::Renderer,
    invalid,
    raster::{DepthCompare, DepthFormat, DepthState},
};
use std::{sync::mpsc, time::Duration};
fn host_format(format: DepthFormat) -> wgpu::TextureFormat {
    match format {
        DepthFormat::Float32 => wgpu::TextureFormat::Depth32Float,
        DepthFormat::Unorm16 => wgpu::TextureFormat::Depth16Unorm,
    }
}
pub(crate) fn host_depth(s: DepthState) -> wgpu::DepthStencilState {
    use DepthCompare::*;
    wgpu::DepthStencilState {
        format: host_format(s.format),
        depth_write_enabled: s.write,
        depth_compare: match s.compare {
            Never => wgpu::CompareFunction::Never,
            Less => wgpu::CompareFunction::Less,
            Equal => wgpu::CompareFunction::Equal,
            LessEqual => wgpu::CompareFunction::LessEqual,
            Greater => wgpu::CompareFunction::Greater,
            NotEqual => wgpu::CompareFunction::NotEqual,
            GreaterEqual => wgpu::CompareFunction::GreaterEqual,
            Always => wgpu::CompareFunction::Always,
        },
        stencil: Default::default(),
        bias: Default::default(),
    }
}
pub(crate) struct ResidentDepth {
    pub format: DepthFormat,
    pub extent: (u32, u32),
    pub view: wgpu::TextureView,
    upload: wgpu::Texture,
    export: wgpu::Texture,
    export_view: wgpu::TextureView,
    readback: wgpu::Buffer,
    pitch: u32,
    init: wgpu::RenderPipeline,
    extract: wgpu::RenderPipeline,
    init_binding: wgpu::BindGroup,
    extract_binding: wgpu::BindGroup,
}
const VERTEX: &str = "@vertex fn vs(@builtin(vertex_index) n:u32)->@builtin(position) vec4<f32> {\nlet p=array<vec2<f32>,3>(vec2(-1.,-1.),vec2(3.,-1.),vec2(-1.,3.)); return vec4(p[n],0.,1.); }";
impl Renderer {
    pub(crate) fn prepare_depth(
        &mut self,
        format: DepthFormat,
        width: u32,
        height: u32,
        bytes: Option<&[u8]>,
    ) -> Result<()> {
        if width == 0
            || height == 0
            || width > self.capabilities.device_limits.max_texture_dimension_2d
            || height > self.capabilities.device_limits.max_texture_dimension_2d
        {
            return Err(invalid("depth extent exceeds device"));
        }
        let pixels = (width as usize)
            .checked_mul(height as usize)
            .ok_or_else(|| invalid("depth sample count overflow"))?;
        let row_bytes = width
            .checked_mul(4)
            .ok_or_else(|| invalid("depth transfer row overflow"))?;
        let transfer_size = pixels
            .checked_mul(4)
            .ok_or_else(|| invalid("depth transfer size overflow"))?;
        let guest_size = pixels
            .checked_mul(format.bytes_per_sample() as usize)
            .ok_or_else(|| invalid("depth guest size overflow"))?;
        // The portable transfer image uses four bytes per sample even for Z16.
        // Expand only when guest data needs uploading; the warm path allocates
        // no conversion buffer here.
        let mut expanded = Vec::new();
        let upload_bytes = if let Some(bytes) = bytes {
            if bytes.len() != guest_size {
                return Err(invalid("incorrect depth upload length"));
            }
            match format {
                DepthFormat::Float32 => {
                    // frag_depth initialization has a normalized-depth contract.
                    for b in bytes.chunks_exact(4) {
                        let z = f32::from_le_bytes(b.try_into().unwrap());
                        if !z.is_finite()
                            || z.is_subnormal()
                            || z.to_bits() == 0x80000000
                            || !(0.0..=1.0).contains(&z)
                        {
                            return Err(invalid("unsupported initial ZF32 value"));
                        }
                    }
                    Some(bytes)
                }
                DepthFormat::Unorm16 => {
                    expanded
                        .try_reserve_exact(transfer_size)
                        .map_err(|e| crate::Error(e.to_string()))?;
                    for b in bytes.chunks_exact(2) {
                        let code = u16::from_le_bytes(b.try_into().unwrap());
                        expanded.extend_from_slice(&u32::from(code).to_le_bytes());
                    }
                    self.counters.temporary_allocations += 1;
                    Some(expanded.as_slice())
                }
            }
        } else {
            None
        };
        if self
            .depth
            .as_ref()
            .is_none_or(|d| d.extent != (width, height) || d.format != format)
        {
            let texture = |label, format, usage| {
                self.device.create_texture(&wgpu::TextureDescriptor {
                    label: Some(label),
                    size: wgpu::Extent3d {
                        width,
                        height,
                        depth_or_array_layers: 1,
                    },
                    mip_level_count: 1,
                    sample_count: 1,
                    dimension: wgpu::TextureDimension::D2,
                    format,
                    usage,
                    view_formats: &[],
                })
            };
            let depth = texture(
                "guest depth",
                host_format(format),
                wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::TEXTURE_BINDING,
            );
            let view = depth.create_view(&Default::default());
            let upload = texture(
                "depth input bits",
                wgpu::TextureFormat::R32Uint,
                wgpu::TextureUsages::COPY_DST | wgpu::TextureUsages::TEXTURE_BINDING,
            );
            let export = texture(
                "depth output bits",
                wgpu::TextureFormat::R32Uint,
                wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
            );
            let export_view = export.create_view(&Default::default());
            let make = |input, source: &str, out: bool| {
                let layout =
                    self.device
                        .create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
                            label: Some("depth transfer"),
                            entries: &[wgpu::BindGroupLayoutEntry {
                                binding: 0,
                                visibility: wgpu::ShaderStages::FRAGMENT,
                                ty: wgpu::BindingType::Texture {
                                    sample_type: input,
                                    view_dimension: wgpu::TextureViewDimension::D2,
                                    multisampled: false,
                                },
                                count: None,
                            }],
                        });
                let shader = self
                    .device
                    .create_shader_module(wgpu::ShaderModuleDescriptor {
                        label: Some("depth transfer"),
                        source: wgpu::ShaderSource::Wgsl(format!("{VERTEX}\n{source}").into()),
                    });
                let pl = self
                    .device
                    .create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
                        label: Some("depth transfer"),
                        bind_group_layouts: &[&layout],
                        push_constant_ranges: &[],
                    });
                let targets = [Some(wgpu::ColorTargetState {
                    format: wgpu::TextureFormat::R32Uint,
                    blend: None,
                    write_mask: wgpu::ColorWrites::ALL,
                })];
                let pipeline =
                    self.device
                        .create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                            label: Some("depth transfer"),
                            layout: Some(&pl),
                            vertex: wgpu::VertexState {
                                module: &shader,
                                entry_point: Some("vs"),
                                compilation_options: Default::default(),
                                buffers: &[],
                            },
                            fragment: Some(wgpu::FragmentState {
                                module: &shader,
                                entry_point: Some("fs"),
                                compilation_options: Default::default(),
                                targets: if out { &targets } else { &[] },
                            }),
                            primitive: Default::default(),
                            depth_stencil: (!out).then(|| {
                                host_depth(DepthState {
                                    format,
                                    compare: DepthCompare::Always,
                                    write: true,
                                })
                            }),
                            multisample: Default::default(),
                            multiview: None,
                            cache: None,
                        });
                (layout, pipeline)
            };
            let (initialize, export_code) = match format {
                DepthFormat::Float32 => (
                    "bitcast<f32>(textureLoad(t,vec2<i32>(p.xy),0).x)",
                    "bitcast<u32>(textureLoad(t,vec2<i32>(p.xy),0).x)",
                ),
                DepthFormat::Unorm16 => (
                    "f32(textureLoad(t,vec2<i32>(p.xy),0).x) / 65535.0",
                    "u32(floor(clamp(textureLoad(t,vec2<i32>(p.xy),0).x,0.0,1.0) * 65535.0 + 0.5))",
                ),
            };
            let (il, init) = make(
                wgpu::TextureSampleType::Uint,
                &format!(
                    "@group(0) @binding(0) var t:texture_2d<u32>; @fragment fn fs(@builtin(position) p:vec4<f32>)->@builtin(frag_depth) f32 {{ return {initialize}; }}"
                ),
                false,
            );
            let (el, extract) = make(
                wgpu::TextureSampleType::Float { filterable: false },
                &format!(
                    "@group(0) @binding(0) var t:texture_2d<f32>; @fragment fn fs(@builtin(position) p:vec4<f32>)->@location(0) u32 {{ return {export_code}; }}"
                ),
                true,
            );
            let binding = |layout, view| {
                self.device.create_bind_group(&wgpu::BindGroupDescriptor {
                    label: Some("depth transfer"),
                    layout,
                    entries: &[wgpu::BindGroupEntry {
                        binding: 0,
                        resource: wgpu::BindingResource::TextureView(view),
                    }],
                })
            };
            let upload_view = upload.create_view(&Default::default());
            let init_binding = binding(&il, &upload_view);
            let extract_binding = binding(&el, &view);
            let pitch = row_bytes
                .checked_add(255)
                .map(|n| n / 256 * 256)
                .ok_or_else(|| invalid("depth readback pitch overflow"))?;
            let readback = self.device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("depth readback"),
                size: u64::from(pitch) * u64::from(height),
                usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
                mapped_at_creation: false,
            });
            self.depth = Some(ResidentDepth {
                format,
                extent: (width, height),
                view,
                upload,
                export,
                export_view,
                readback,
                pitch,
                init,
                extract,
                init_binding,
                extract_binding,
            });
            self.counters.depth_allocations += 1;
            self.counters.texture_allocations += 3;
            self.counters.pipeline_allocations += 2;
            self.counters.shader_module_allocations += 2;
            self.counters.bind_group_allocations += 2;
            self.counters.buffer_allocations += 1;
        }
        if let Some(bytes) = upload_bytes {
            let d = self.depth.as_ref().unwrap();
            self.queue.write_texture(
                wgpu::TexelCopyTextureInfo {
                    texture: &d.upload,
                    mip_level: 0,
                    origin: wgpu::Origin3d::ZERO,
                    aspect: wgpu::TextureAspect::All,
                },
                bytes,
                wgpu::TexelCopyBufferLayout {
                    offset: 0,
                    bytes_per_row: Some(row_bytes),
                    rows_per_image: Some(height),
                },
                wgpu::Extent3d {
                    width,
                    height,
                    depth_or_array_layers: 1,
                },
            );
            let mut encoder = self
                .device
                .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                    label: Some("initialize guest depth"),
                });
            {
                let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                    label: Some("initialize guest depth"),
                    color_attachments: &[],
                    depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                        view: &d.view,
                        depth_ops: Some(wgpu::Operations {
                            load: wgpu::LoadOp::Clear(1.),
                            store: wgpu::StoreOp::Store,
                        }),
                        stencil_ops: None,
                    }),
                    timestamp_writes: None,
                    occlusion_query_set: None,
                });
                pass.set_pipeline(&d.init);
                pass.set_bind_group(0, &d.init_binding, &[]);
                pass.draw(0..3, 0..1);
            }
            self.last_submission = Some(self.queue.submit([encoder.finish()]));
            self.counters.submissions += 1;
            self.counters.upload_calls += 1;
            self.counters.staging_allocations += 1;
            self.counters.upload_bytes += bytes.len() as u64;
            self.counters.depth_upload_bytes += bytes.len() as u64;
        }
        Ok(())
    }
    pub(crate) fn read_depth(&mut self) -> Result<Vec<u8>> {
        let d = self
            .depth
            .as_ref()
            .ok_or_else(|| invalid("missing resident depth"))?;
        let (width, height) = d.extent;
        let row_bytes = (width as usize)
            .checked_mul(4)
            .ok_or_else(|| invalid("depth readback row overflow"))?;
        let output_size = (width as usize)
            .checked_mul(height as usize)
            .and_then(|n| n.checked_mul(d.format.bytes_per_sample() as usize))
            .ok_or_else(|| invalid("depth readback output overflow"))?;
        // Allocation must succeed before mapping. Once mapped, every packing
        // path below drops the borrowed view and unmaps before returning.
        let mut out = Vec::new();
        out.try_reserve_exact(output_size)
            .map_err(|e| crate::Error(e.to_string()))?;
        let mut e = self
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("publish guest depth"),
            });
        {
            let mut p = e.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("export depth bits"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &d.export_view,
                    depth_slice: None,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(wgpu::Color::TRANSPARENT),
                        store: wgpu::StoreOp::Store,
                    },
                })],
                depth_stencil_attachment: None,
                timestamp_writes: None,
                occlusion_query_set: None,
            });
            p.set_pipeline(&d.extract);
            p.set_bind_group(0, &d.extract_binding, &[]);
            p.draw(0..3, 0..1);
        }
        e.copy_texture_to_buffer(
            wgpu::TexelCopyTextureInfo {
                texture: &d.export,
                mip_level: 0,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            wgpu::TexelCopyBufferInfo {
                buffer: &d.readback,
                layout: wgpu::TexelCopyBufferLayout {
                    offset: 0,
                    bytes_per_row: Some(d.pitch),
                    rows_per_image: Some(height),
                },
            },
            wgpu::Extent3d {
                width,
                height,
                depth_or_array_layers: 1,
            },
        );
        let submission = self.queue.submit([e.finish()]);
        self.last_submission = Some(submission.clone());
        self.counters.submissions += 1;
        let (tx, rx) = mpsc::sync_channel(1);
        d.readback
            .slice(..)
            .map_async(wgpu::MapMode::Read, move |r| {
                let _ = tx.send(r);
            });
        if let Err(error) = self.device.poll(wgpu::PollType::wait_for(submission)) {
            d.readback.unmap();
            return Err(crate::Error(error.to_string()));
        }
        self.counters.synchronization_points += 1;
        let mapping = rx
            .recv_timeout(Duration::from_secs(30))
            .map_err(|e| crate::Error(e.to_string()))
            .and_then(|result| result.map_err(|e| crate::Error(e.to_string())));
        if let Err(error) = mapping {
            d.readback.unmap();
            return Err(error);
        }
        let mapped = d.readback.slice(..).get_mapped_range();
        let packed = (|| -> Result<()> {
            for row in mapped.chunks_exact(d.pitch as usize) {
                let pixels = &row[..row_bytes];
                match d.format {
                    DepthFormat::Float32 => out.extend_from_slice(pixels),
                    DepthFormat::Unorm16 => {
                        for b in pixels.chunks_exact(4) {
                            let code = u32::from_le_bytes(b.try_into().unwrap());
                            let code = u16::try_from(code)
                                .map_err(|_| invalid("depth export exceeds Z16 code range"))?;
                            out.extend_from_slice(&code.to_le_bytes());
                        }
                    }
                }
            }
            Ok(())
        })();
        drop(mapped);
        d.readback.unmap();
        packed?;
        self.counters.readback_operations += 1;
        self.counters.readback_bytes += u64::from(d.pitch) * u64::from(height);
        self.counters.readback_host_copy_bytes += out.len() as u64;
        self.counters.owned_output_allocations += 1;
        Ok(out)
    }
}
