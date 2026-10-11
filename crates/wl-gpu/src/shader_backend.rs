//! Runtime binding for the narrow translated fragment harness, not a hardware draw.
use crate::{
    Error, Result, backend::Renderer, invalid, maxwell::Maxwell3d, memory::*,
    shader_program::FragmentProgram, texture::TextureChanges,
};

#[derive(Clone, Copy, PartialEq, Eq)]
struct Stamp {
    address: GuestAddress,
    version: (u64, u64),
}
pub(crate) struct ResidentFragment {
    pub program: FragmentProgram,
    pub pipeline: wgpu::RenderPipeline,
    pub binding: wgpu::BindGroup,
    pub ready: bool,
    buffer: wgpu::Buffer,
    data: [u8; 256],
    stamps: [Option<Stamp>; 64],
}
impl Renderer {
    /// Code only. Mutable constants are synchronized separately, never embedded in WGSL.
    pub fn set_resource_fragment(&mut self, program: &FragmentProgram) -> Result<()> {
        if self.poisoned {
            return Err(invalid("renderer unavailable after backend failure"));
        }
        if self
            .resource_fragment
            .as_ref()
            .is_some_and(|f| f.program == *program)
        {
            self.counters.shader_cache_hits += 1;
            return Ok(());
        }
        self.device.push_error_scope(wgpu::ErrorFilter::OutOfMemory);
        self.device.push_error_scope(wgpu::ErrorFilter::Validation);
        let layout = self
            .device
            .create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
                label: Some("translated constant reads"),
                entries: &[wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Uniform,
                        has_dynamic_offset: false,
                        min_binding_size: wgpu::BufferSize::new(256),
                    },
                    count: None,
                }],
            });
        let buffer = self.device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("packed Maxwell constant words"),
            size: 256,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let binding = self.device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("Maxwell constants"),
            layout: &layout,
            entries: &[wgpu::BindGroupEntry {
                binding: 0,
                resource: buffer.as_entire_binding(),
            }],
        });
        let pipeline_layout = self
            .device
            .create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
                label: Some("translated resource layout"),
                bind_group_layouts: &[&layout],
                push_constant_ranges: &[],
            });
        let shader = self
            .device
            .create_shader_module(wgpu::ShaderModuleDescriptor {
                label: Some("Maxwell resource fragment"),
                source: wgpu::ShaderSource::Wgsl(program.wgsl().into()),
            });
        let pipeline = self
            .device
            .create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                label: Some("Maxwell fragment harness"),
                layout: Some(&pipeline_layout),
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
        let validation = pollster::block_on(self.device.pop_error_scope());
        let allocation = pollster::block_on(self.device.pop_error_scope());
        if let Some(e) = validation.or(allocation) {
            self.poisoned = true;
            return Err(Error(e.to_string()));
        }
        self.resource_fragment = Some(ResidentFragment {
            program: program.clone(),
            pipeline,
            binding,
            ready: false,
            buffer,
            data: [0; 256],
            stamps: [None; 64],
        });
        self.counters.pipeline_allocations += 1;
        self.counters.shader_module_allocations += 1;
        self.counters.buffer_allocations += 1;
        self.counters.bind_group_allocations += 1;
        Ok(())
    }
    /// Caller supplies the stage's B197 binding group, not constant data. Guest write
    /// journal epochs must include remaps/backing replacement; borrows end before return.
    /// A failed preflight makes the harness unready; it cannot draw the last good data.
    pub fn sync_fragment_resources(
        &mut self,
        engine: &Maxwell3d,
        group: u8,
        map: &AddressMap,
        memory: &impl GuestMemory,
        changes: &TextureChanges,
    ) -> Result<()> {
        if self.poisoned {
            return Err(invalid("renderer unavailable after backend failure"));
        }
        let f = self
            .resource_fragment
            .as_mut()
            .ok_or_else(|| invalid("no translated fragment"))?;
        f.ready = false;
        let mut data = f.data;
        let mut stamps = f.stamps;
        for (n, reference) in f.program.constants().iter().enumerate() {
            let cb = engine.constant_buffer(group, reference.slot)?;
            if reference
                .offset
                .checked_add(4)
                .is_none_or(|end| end > cb.size)
            {
                return Err(invalid("constant read exceeds bound size"));
            }
            let gpu = cb
                .address
                .0
                .checked_add(u64::from(reference.offset))
                .ok_or_else(|| invalid("constant address overflow"))?;
            let guest = map.resolve(GpuAddress(gpu), 4)?;
            let stamp = Stamp {
                address: guest,
                version: changes.stamp(guest.0..guest.0 + 4),
            };
            if stamps[n] != Some(stamp) {
                let word = cb.read_word(reference.offset, map, memory)?;
                data[n * 4..n * 4 + 4].copy_from_slice(&word.to_le_bytes());
                stamps[n] = Some(stamp);
                self.counters.constant_guest_bytes += 4;
                self.counters.guest_to_host_bytes += 4;
            }
        }
        // Small fixed staging image; upload only the enclosing changed word interval.
        let first = data
            .chunks_exact(4)
            .zip(f.data.chunks_exact(4))
            .position(|(a, b)| a != b);
        if let Some(first) = first {
            let last = data
                .chunks_exact(4)
                .zip(f.data.chunks_exact(4))
                .rposition(|(a, b)| a != b)
                .unwrap();
            let bytes = &data[first * 4..(last + 1) * 4];
            self.queue
                .write_buffer(&f.buffer, (first * 4) as u64, bytes);
            self.counters.constant_upload_bytes += bytes.len() as u64;
            self.counters.upload_bytes += bytes.len() as u64;
            self.counters.upload_calls += 1;
            self.counters.staging_allocations += 1;
        }
        f.data = data;
        f.stamps = stamps;
        f.ready = true;
        self.counters.bind_group_reuses += 1;
        Ok(())
    }
}
