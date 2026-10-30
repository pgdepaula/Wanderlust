//! Paired translated stages; resource ownership stays within the existing renderer.
use crate::{
    Result,
    backend::Renderer,
    invalid,
    maxwell::{Maxwell3d, PositionFormat},
    memory::*,
    shader_program::{FragmentProgram, VertexProgram},
    texture::TextureChanges,
};

#[derive(Clone, Copy, PartialEq, Eq)]
struct BackingStamp {
    guest: GuestAddress,
    size: usize,
    version: (u64, u64),
}
impl BackingStamp {
    fn matches(&mut self, address: GpuAddress, map: &AddressMap, changes: &TextureChanges) -> bool {
        if address
            .0
            .checked_add(self.size as u64)
            .is_some_and(|e| e <= 1 << 40)
            && map.resolve(address, self.size).ok() == Some(self.guest)
            && changes.unchanged_since(self.guest.0..self.guest.0 + self.size as u64, self.version)
        {
            self.version = changes.observation();
            true
        } else {
            false
        }
    }
    fn new(
        address: GpuAddress,
        size: usize,
        map: &AddressMap,
        changes: &TextureChanges,
    ) -> Result<Self> {
        let guest = map.resolve(address, size)?;
        Ok(Self {
            guest,
            size,
            version: changes.observation(),
        })
    }
}
pub(crate) struct SelectedVertex {
    stamp: BackingStamp,
    program: VertexProgram,
    constant_data: [u8; 256],
    constant_stamps: [Option<BackingStamp>; 64],
    // Exact typed buffer identity. A changed program or replaced fragment
    // uniform allocation must upload even when new constant values are zero.
    constant_buffer: Option<wgpu::Buffer>,
}

pub(crate) struct ResidentVertex {
    program: VertexProgram,
    fragment: FragmentProgram,
    pub pipeline: wgpu::RenderPipeline,
    pub buffer: Option<wgpu::Buffer>,
    size: u64,
    stride: u64,
    offsets: [u64; 2],
    position_format: PositionFormat,
    target_format: crate::raster::ColorFormat,
    stamp: Option<BackingStamp>,
    flip_y: bool,
    depth_clip: crate::raster::DepthClipRange,
    color: crate::raster::ColorState,
    depth: Option<crate::raster::DepthState>,
    cull: crate::raster::Culling,
    shader: wgpu::ShaderModule,
}
impl Renderer {
    /// Intermediate stage validation API. Guest method resolution supplies these
    /// values in the hardware path; this function itself is not a hardware draw.
    pub fn set_vertex_program(
        &mut self,
        program: &VertexProgram,
        stride: u64,
        offsets: [u64; 2],
    ) -> Result<()> {
        self.vertex_ready = false;
        if !program.constants().is_empty() {
            self.vertex = None;
            return Err(invalid(
                "vertex constants require guest stage binding and synchronization",
            ));
        }
        self.set_vertex_program_raster(
            program,
            stride,
            offsets,
            PositionFormat::Float32x4,
            None,
            crate::raster::ColorFormat::Rgba8,
        )
    }
    fn set_vertex_program_raster(
        &mut self,
        program: &VertexProgram,
        stride: u64,
        offsets: [u64; 2],
        position_format: PositionFormat,
        raster: Option<crate::raster::Rasterization>,
        target_format: crate::raster::ColorFormat,
    ) -> Result<()> {
        let flip_y = raster.is_some_and(|r| r.flip_y);
        let depth_clip = raster.map_or_else(Default::default, |r| r.depth_clip);
        let color = raster.map_or_else(Default::default, |r| r.color);
        let depth = raster.and_then(|r| r.depth);
        let cull = raster.map_or_else(Default::default, |r| r.cull);
        self.vertex_ready = false;
        if self.poisoned {
            self.vertex = None;
            return Err(invalid("renderer unavailable after backend failure"));
        }
        let fragment = self
            .resource_fragment
            .as_ref()
            .ok_or_else(|| invalid("no fragment for vertex linkage"))?;
        if stride == 0
            || stride % 4 != 0
            || stride
                > u64::from(
                    self.capabilities
                        .device_limits
                        .max_vertex_buffer_array_stride,
                )
            || offsets.iter().any(|v| v % 4 != 0)
            || offsets[0]
                .checked_add(u64::from(position_format.byte_size()))
                .is_none_or(|n| n > stride)
            || program.needs_uv_input() && offsets[1].checked_add(8).is_none_or(|n| n > stride)
        {
            self.vertex = None;
            return Err(invalid("unsupported vertex layout"));
        }
        if self.vertex.as_ref().is_some_and(|v| {
            v.program == *program
                && v.fragment == fragment.program
                && v.stride == stride
                && v.offsets == offsets
                && v.position_format == position_format
                && v.target_format == target_format
                && v.flip_y == flip_y
                && v.depth_clip == depth_clip
                && v.color == color
                && v.depth == depth
                && v.cull == cull
        }) {
            self.counters.shader_cache_hits += 1;
            return Ok(());
        }
        let previous = self.vertex.take();
        self.device.push_error_scope(wgpu::ErrorFilter::Validation);
        self.device.push_error_scope(wgpu::ErrorFilter::OutOfMemory);
        let cached_shader = previous
            .as_ref()
            .filter(|v| {
                v.program == *program
                    && v.flip_y == flip_y
                    && v.depth_clip == depth_clip
                    && v.fragment.perspective() == fragment.program.perspective()
            })
            .map(|v| v.shader.clone());
        let new_shader = cached_shader.is_none();
        let shader = cached_shader.unwrap_or_else(|| {
            self.device
                .create_shader_module(wgpu::ShaderModuleDescriptor {
                    label: Some("translated Maxwell vertex"),
                    source: wgpu::ShaderSource::Wgsl(
                        program
                            .wgsl_for_raster(flip_y, fragment.program.perspective(), depth_clip)
                            .into(),
                    ),
                })
        });
        let mut groups = vec![&fragment.layout];
        if fragment.program.uses_texture() {
            groups.push(&self.texture_layout);
        }
        let layout = self
            .device
            .create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
                label: Some("linked Maxwell stages"),
                bind_group_layouts: &groups,
                push_constant_ranges: &[],
            });
        let attributes = [
            wgpu::VertexAttribute {
                format: match position_format {
                    PositionFormat::Float16x4 => wgpu::VertexFormat::Float16x4,
                    PositionFormat::Float32x3 => wgpu::VertexFormat::Float32x3,
                    PositionFormat::Float32x4 => wgpu::VertexFormat::Float32x4,
                },
                offset: offsets[0],
                shader_location: 0,
            },
            wgpu::VertexAttribute {
                format: wgpu::VertexFormat::Float32x2,
                offset: offsets[1],
                shader_location: 1,
            },
        ];
        let pipeline = self
            .device
            .create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                label: Some("translated vertex/pixel pipeline"),
                layout: Some(&layout),
                vertex: wgpu::VertexState {
                    module: &shader,
                    entry_point: Some("vs_main"),
                    compilation_options: Default::default(),
                    buffers: &[wgpu::VertexBufferLayout {
                        array_stride: stride,
                        step_mode: wgpu::VertexStepMode::Vertex,
                        attributes: &attributes[..if program.needs_uv_input() { 2 } else { 1 }],
                    }],
                },
                fragment: Some(wgpu::FragmentState {
                    module: &fragment.shader,
                    entry_point: Some("fs_main"),
                    compilation_options: Default::default(),
                    targets: &[Some(wgpu::ColorTargetState {
                        format: target_format.host(),
                        blend: color.blend.map(host_blend),
                        write_mask: wgpu::ColorWrites::from_bits_truncate(u32::from(
                            color.write_mask,
                        )),
                    })],
                }),
                primitive: host_primitive(cull),
                depth_stencil: depth.map(crate::depth_backend::host_depth),
                multisample: Default::default(),
                multiview: None,
                cache: None,
            });
        let allocation = pollster::block_on(self.device.pop_error_scope());
        let validation = pollster::block_on(self.device.pop_error_scope());
        if let Some(e) = allocation.or(validation) {
            return Err(crate::Error(e.to_string()));
        }
        self.vertex = Some(ResidentVertex {
            program: program.clone(),
            fragment: fragment.program.clone(),
            pipeline,
            buffer: previous.as_ref().and_then(|v| v.buffer.clone()),
            size: previous.as_ref().map_or(0, |v| v.size),
            stamp: previous.as_ref().and_then(|v| v.stamp),
            stride,
            offsets,
            position_format,
            target_format,
            flip_y,
            depth_clip,
            color,
            depth,
            cull,
            shader,
        });
        if new_shader {
            self.counters.shader_module_allocations += 1;
        } else {
            self.counters.shader_cache_hits += 1;
        }
        self.counters.pipeline_allocations += 1;
        Ok(())
    }
    pub fn upload_vertices(&mut self, bytes: &[u8]) -> Result<()> {
        self.vertex_ready = false;
        let v = self
            .vertex
            .as_mut()
            .ok_or_else(|| invalid("vertex stage not selected"))?;
        if bytes.is_empty()
            || bytes.len() % 4 != 0
            || bytes.len() as u64 > self.capabilities.device_limits.max_buffer_size
        {
            v.buffer = None;
            v.size = 0;
            return Err(invalid("invalid vertex upload"));
        }
        if v.size != bytes.len() as u64 {
            v.buffer = Some(self.device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("guest vertices"),
                size: bytes.len() as u64,
                usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            }));
            v.size = bytes.len() as u64;
            self.counters.buffer_allocations += 1;
        }
        self.queue
            .write_buffer(v.buffer.as_ref().unwrap(), 0, bytes);
        self.counters.vertex_upload_bytes += bytes.len() as u64;
        self.counters.upload_bytes += bytes.len() as u64;
        self.counters.upload_calls += 1;
        self.counters.staging_allocations += 1;
        v.stamp = None;
        self.vertex_ready = true;
        Ok(())
    }
    pub(crate) fn validate_vertices(&self, first: u32, count: u32) -> Result<()> {
        if count % 3 != 0 {
            return Err(invalid("invalid triangle vertex count"));
        }
        self.validate_vertex_span(first, count)
    }
    pub(crate) fn validate_vertex_span(&self, first: u32, count: u32) -> Result<()> {
        let v = self
            .vertex
            .as_ref()
            .ok_or_else(|| invalid("missing translated vertex stage"))?;
        let f = self
            .resource_fragment
            .as_ref()
            .ok_or_else(|| invalid("missing linked fragment stage"))?;
        if self.poisoned
            || !self.vertex_ready
            || v.fragment != f.program
            || v.buffer.is_none()
            || count == 0
            || first
                .checked_add(count)
                .and_then(|n| u64::from(n).checked_mul(v.stride))
                .is_none_or(|n| n > v.size)
        {
            return Err(invalid("invalid vertex draw range or stale shader pair"));
        }
        Ok(())
    }
    fn sync_vertex_constants(
        &mut self,
        engine: &Maxwell3d,
        group: u8,
        map: &AddressMap,
        memory: &impl GuestMemory,
        changes: &TextureChanges,
    ) -> Result<()> {
        let selected = self
            .selected_vertex
            .as_mut()
            .ok_or_else(|| invalid("no selected vertex constants"))?;
        if selected.program.constants().is_empty() {
            return Ok(());
        }
        let fragment = self
            .resource_fragment
            .as_ref()
            .ok_or_else(|| invalid("missing shared constant buffer"))?;
        let references = selected.program.constants();
        if references.len() > 64 {
            return Err(invalid("too many vertex constant words"));
        }
        let mut data = selected.constant_data;
        let mut stamps = selected.constant_stamps;
        for (n, reference) in references.iter().enumerate() {
            let cb = engine.constant_buffer(group, reference.slot)?;
            if reference
                .offset
                .checked_add(4)
                .is_none_or(|end| end > cb.size)
            {
                return Err(invalid("vertex constant exceeds bound size"));
            }
            let address = cb
                .address
                .0
                .checked_add(u64::from(reference.offset))
                .map(GpuAddress)
                .ok_or_else(|| invalid("vertex constant address overflow"))?;
            let cached = stamps[n]
                .as_mut()
                .is_some_and(|stamp| stamp.matches(address, map, changes));
            if !cached {
                let word = cb.read_word(reference.offset, map, memory)?;
                data[n * 4..n * 4 + 4].copy_from_slice(&word.to_le_bytes());
                stamps[n] = Some(BackingStamp::new(address, 4, map, changes)?);
                self.counters.constant_guest_bytes += 4;
                self.counters.guest_to_host_bytes += 4;
            }
        }
        let replaced = selected.constant_buffer.as_ref() != Some(&fragment.buffer);
        let changed = |n: usize| data[n * 4..n * 4 + 4] != selected.constant_data[n * 4..n * 4 + 4];
        let first = if replaced {
            Some(0)
        } else {
            (0..references.len()).find(|n| changed(*n))
        };
        if let Some(first) = first {
            let last = if replaced {
                references.len() - 1
            } else {
                (first..references.len()).rfind(|n| changed(*n)).unwrap()
            };
            let bytes = &data[first * 4..(last + 1) * 4];
            self.queue
                .write_buffer(&fragment.buffer, 256 + (first * 4) as u64, bytes);
            self.counters.constant_upload_bytes += bytes.len() as u64;
            self.counters.upload_bytes += bytes.len() as u64;
            self.counters.upload_calls += 1;
            self.counters.staging_allocations += 1;
        }
        if replaced {
            selected.constant_buffer = Some(fragment.buffer.clone());
        }
        selected.constant_data = data;
        selected.constant_stamps = stamps;
        Ok(())
    }
    /// Resolve B197 vertex stage/stream through checked guest mappings. No host
    /// geometry or stage binary argument is accepted. Draw/target remain separate.
    pub fn sync_selected_vertices(
        &mut self,
        engine: &Maxwell3d,
        map: &AddressMap,
        memory: &impl GuestMemory,
        changes: &TextureChanges,
        first: u32,
        count: u32,
    ) -> Result<()> {
        self.validate_guest_resource_access()?;
        self.vertex_ready = false;
        let source = engine.vertex_program()?;
        let array = engine.vertex_array()?;
        let size = array.span(first, count)?;
        if self
            .selected_vertex
            .as_mut()
            .is_none_or(|s| !s.stamp.matches(source.address, map, changes))
        {
            self.selected_vertex = None;
            let binary = crate::shader_backend::read_program(
                source.address,
                map,
                memory,
                &mut self.counters,
                true,
            )?;
            self.counters.vertex_shader_decodes += 1;
            let program = VertexProgram::decode(&binary)?;
            let stamp = BackingStamp::new(source.address, binary.len(), map, changes)?;
            self.selected_vertex = Some(SelectedVertex {
                stamp,
                program,
                constant_data: [0; 256],
                constant_stamps: [None; 64],
                constant_buffer: None,
            });
        } else {
            self.counters.shader_cache_hits += 1;
        }
        let selected = self.selected_vertex.as_ref().unwrap();
        if selected.program.needs_uv_input() && !array.uv_active {
            return Err(invalid(
                "shader reads inactive UV; constant vertex attributes unsupported",
            ));
        }
        if selected.program.required_registers() > u16::from(source.registers) {
            return Err(invalid("vertex register allocation insufficient"));
        }
        self.sync_vertex_constants(engine, source.group, map, memory, changes)?;
        let selected = self.selected_vertex.as_ref().unwrap();
        let fragment = self
            .resource_fragment
            .as_ref()
            .ok_or_else(|| invalid("missing fragment for vertex pairing"))?;
        let stride = u64::from(array.stride);
        let target_format = if self.hardware_raster.is_some() {
            engine.rasterization()?.0.format
        } else {
            crate::raster::ColorFormat::Rgba8
        };
        let offsets = array.offsets.map(u64::from);
        let flip_y = self.hardware_raster.is_some_and(|r| r.flip_y);
        let depth_clip = self
            .hardware_raster
            .map_or_else(Default::default, |r| r.depth_clip);
        let color = self
            .hardware_raster
            .map_or_else(Default::default, |r| r.color);
        let depth = self.hardware_raster.and_then(|r| r.depth);
        let cull = self
            .hardware_raster
            .map_or_else(Default::default, |r| r.cull);
        if self.vertex.as_ref().is_none_or(|v| {
            v.depth != depth
                || v.cull != cull
                || v.color != color
                || v.flip_y != flip_y
                || v.depth_clip != depth_clip
                || v.program != selected.program
                || v.fragment != fragment.program
                || v.stride != stride
                || v.offsets != offsets
                || v.position_format != array.position_format
                || v.target_format != target_format
        }) {
            let program = selected.program.clone();
            self.set_vertex_program_raster(
                &program,
                stride,
                offsets,
                array.position_format,
                self.hardware_raster,
                target_format,
            )?;
        }
        let cached = self
            .vertex
            .as_mut()
            .unwrap()
            .stamp
            .as_mut()
            .is_some_and(|s| s.size == size && s.matches(array.address, map, changes));
        if !cached {
            let stamp = BackingStamp::new(array.address, size, map, changes)?;
            let bytes = map.read(memory, array.address, size)?;
            if bytes.len() != size {
                return Err(invalid("short guest vertex read"));
            }
            self.counters.vertex_guest_bytes += size as u64;
            self.upload_vertices(bytes)?;
            self.vertex.as_mut().unwrap().stamp = Some(stamp);
        } else {
            self.counters.cache_hits += 1;
        }
        self.vertex_ready = true;
        self.validate_vertex_span(first, count)
    }
}

fn host_blend(state: crate::raster::BlendState) -> wgpu::BlendState {
    let factor = |f| match f {
        crate::raster::BlendFactor::Zero => wgpu::BlendFactor::Zero,
        crate::raster::BlendFactor::One => wgpu::BlendFactor::One,
        crate::raster::BlendFactor::SourceAlpha => wgpu::BlendFactor::SrcAlpha,
        crate::raster::BlendFactor::OneMinusSourceAlpha => wgpu::BlendFactor::OneMinusSrcAlpha,
        crate::raster::BlendFactor::SourceColor => wgpu::BlendFactor::Src,
        crate::raster::BlendFactor::OneMinusSourceColor => wgpu::BlendFactor::OneMinusSrc,
        crate::raster::BlendFactor::DestinationAlpha => wgpu::BlendFactor::DstAlpha,
        crate::raster::BlendFactor::OneMinusDestinationAlpha => wgpu::BlendFactor::OneMinusDstAlpha,
        crate::raster::BlendFactor::DestinationColor => wgpu::BlendFactor::Dst,
        crate::raster::BlendFactor::OneMinusDestinationColor => wgpu::BlendFactor::OneMinusDst,
    };
    let component = |f: [crate::raster::BlendFactor; 2], op| wgpu::BlendComponent {
        src_factor: factor(f[0]),
        dst_factor: factor(f[1]),
        operation: match op {
            crate::raster::BlendOperation::Add => wgpu::BlendOperation::Add,
            crate::raster::BlendOperation::Subtract => wgpu::BlendOperation::Subtract,
            crate::raster::BlendOperation::ReverseSubtract => wgpu::BlendOperation::ReverseSubtract,
            crate::raster::BlendOperation::Min => wgpu::BlendOperation::Min,
            crate::raster::BlendOperation::Max => wgpu::BlendOperation::Max,
        },
    };
    wgpu::BlendState {
        color: component(state.color, state.color_operation),
        alpha: component(state.alpha, state.alpha_operation),
    }
}

fn host_primitive(cull: crate::raster::Culling) -> wgpu::PrimitiveState {
    use crate::raster::{CullFace, Culling, FrontFace};
    let (cull_mode, front_face) = match cull {
        Culling::Disabled => (None, wgpu::FrontFace::Ccw),
        Culling::Enabled { face, front } => (
            Some(match face {
                CullFace::Front => wgpu::Face::Front,
                CullFace::Back => wgpu::Face::Back,
            }),
            match front {
                FrontFace::Clockwise => wgpu::FrontFace::Cw,
                FrontFace::CounterClockwise => wgpu::FrontFace::Ccw,
            },
        ),
    };
    // Supported B197 WINDOW_ORIGIN=UPPER_LEFT. The existing vertex Y adjustment
    // already realizes guest viewport orientation; do not invert front_face again.
    wgpu::PrimitiveState {
        cull_mode,
        front_face,
        ..Default::default()
    }
}
