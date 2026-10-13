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
    pub shader: wgpu::ShaderModule,
    pub layout: wgpu::BindGroupLayout,
    pub program: FragmentProgram,
    pub pipeline: wgpu::RenderPipeline,
    pub binding: wgpu::BindGroup,
    pub ready: bool,
    pub texture_binding: Option<wgpu::BindGroup>,
    texture_id: Option<crate::texture::TextureId>,
    descriptor: Option<(crate::maxwell::DescriptorStamp, crate::texture::TextureDesc)>,
    buffer: wgpu::Buffer,
    data: [u8; 256],
    stamps: [Option<Stamp>; 64],
}
#[derive(Clone, Copy)]
pub(crate) struct SelectedShader {
    guest: GuestAddress,
    size: usize,
    version: (u64, u64),
}
impl Renderer {
    /// Select/load the guest binary using actual B197 pixel-program state. This
    /// synchronizes fragment resources; draw geometry is still the explicit harness.
    pub fn sync_selected_fragment(
        &mut self,
        engine: &Maxwell3d,
        map: &AddressMap,
        memory: &impl GuestMemory,
        changes: &TextureChanges,
    ) -> Result<()> {
        self.validate_guest_resource_access()?;
        if let Some(f) = self.resource_fragment.as_mut() {
            f.ready = false;
        }
        if self.poisoned {
            return Err(invalid("renderer unavailable after backend failure"));
        }
        let source = engine.fragment_program()?;
        if source
            .address
            .0
            .checked_add(80)
            .is_none_or(|end| end > 1 << 40)
        {
            return Err(invalid("shader header exceeds 40-bit address space"));
        }
        let cached = self.selected_shader.filter(|cache| {
            source
                .address
                .0
                .checked_add(cache.size as u64)
                .is_some_and(|end| end <= 1 << 40)
                && map.resolve(source.address, cache.size).ok() == Some(cache.guest)
                && changes.stamp(cache.guest.0..cache.guest.0 + cache.size as u64) == cache.version
        });
        if cached.is_some() {
            self.counters.shader_cache_hits += 1;
        } else {
            self.selected_shader = None;
            let binary = read_program(source.address, map, memory, &mut self.counters, false)?;
            self.counters.shader_decodes += 1;
            let program = FragmentProgram::decode(&binary)?;
            if program.required_registers() > u16::from(source.registers) {
                return Err(invalid("pixel program register count below decoded usage"));
            }
            self.set_resource_fragment(&program)?;
            let guest = map.resolve(source.address, binary.len())?;
            self.selected_shader = Some(SelectedShader {
                guest,
                size: binary.len(),
                version: changes.stamp(guest.0..guest.0 + binary.len() as u64),
            });
        }
        if self
            .resource_fragment
            .as_ref()
            .is_none_or(|f| f.program.required_registers() > u16::from(source.registers))
        {
            return Err(invalid("pixel program register allocation insufficient"));
        }
        self.sync_fragment_resources(engine, source.group, map, memory, changes)
    }
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
                bind_group_layouts: &if program.texture_input().is_some() {
                    vec![&layout, &self.texture_layout]
                } else {
                    vec![&layout]
                },
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
        if let Some(old) = self.resource_fragment.as_ref().and_then(|f| f.texture_id) {
            self.textures.remove(&old);
        }
        self.selected_shader = None;
        self.resource_fragment = Some(ResidentFragment {
            shader,
            layout,
            program: program.clone(),
            pipeline,
            binding,
            ready: false,
            texture_binding: None,
            texture_id: None,
            descriptor: None,
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
        self.validate_guest_resource_access()?;
        if self.poisoned {
            return Err(invalid("renderer unavailable after backend failure"));
        }
        let f = self
            .resource_fragment
            .as_mut()
            .ok_or_else(|| invalid("no translated fragment"))?;
        f.ready = false;
        // Scalar lowering must not accidentally implement a fragmented or
        // effectively unaligned B64 gather. These are supported-subset limits.
        for reference in f.program.constant_pairs() {
            let cb = engine.constant_buffer(group, reference.slot)?;
            if reference
                .offset
                .checked_add(8)
                .is_none_or(|end| end > cb.size)
            {
                return Err(invalid("constant pair exceeds bound size"));
            }
            let address = cb
                .address
                .0
                .checked_add(u64::from(reference.offset))
                .ok_or_else(|| invalid("constant pair address overflow"))?;
            if address % 8 != 0 || address.checked_add(8).is_none_or(|end| end > 1 << 40) {
                return Err(invalid("unsupported constant pair address or alignment"));
            }
            map.resolve(GpuAddress(address), 8)?;
        }
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
        // Resolve the uniform handle through actual descriptor pools before uploads.
        let texture = if let Some(input) = f.program.texture_input() {
            let n = input as usize * 4;
            let handle = u32::from_le_bytes(data[n..n + 4].try_into().unwrap());
            let subchannel = engine.texture_subchannel()?;
            let stamp = engine.descriptor_stamp(subchannel, handle, map, changes)?;
            let desc = if let Some((_, desc)) = f.descriptor.filter(|(old, _)| *old == stamp) {
                desc
            } else {
                self.counters.descriptor_reads += 2;
                engine.resolve_handle(subchannel, handle, map, memory)?
            };
            Some((stamp, desc))
        } else {
            None
        };
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
        if let Some((stamp, desc)) = texture {
            let id = match f.texture_id {
                Some(id) => id,
                None => {
                    let mut candidate = u64::MAX;
                    while self
                        .textures
                        .contains_key(&crate::texture::TextureId(candidate))
                    {
                        candidate = candidate
                            .checked_sub(1)
                            .ok_or_else(|| invalid("texture IDs exhausted"))?;
                    }
                    crate::texture::TextureId(candidate)
                }
            };
            // Release the resident-fragment borrow before using the shared texture cache.
            self.sync_texture(id, desc, map, memory, changes)?;
            let binding = self.textures[&id].bindings[0].clone();
            let f = self.resource_fragment.as_mut().expect("resident fragment");
            f.texture_id = Some(id);
            f.descriptor = Some((stamp, desc));
            f.texture_binding = Some(binding);
            f.ready = true;
        } else {
            f.ready = true;
        }

        self.counters.bind_group_reuses += 1;
        Ok(())
    }
}

pub(crate) fn read_program(
    source: GpuAddress,
    map: &AddressMap,
    memory: &impl GuestMemory,
    counters: &mut crate::Counters,
    vertex: bool,
) -> Result<Vec<u8>> {
    if source.0.checked_add(80).is_none_or(|end| end > 1 << 40) {
        return Err(invalid("shader header exceeds 40-bit address space"));
    }
    let header = map.read(memory, source, 80)?;
    if header.len() != 80 {
        return Err(invalid("short guest shader header"));
    }
    let mut binary = Vec::with_capacity(112);
    binary.extend_from_slice(header);
    if vertex {
        counters.vertex_shader_bytes += 80;
    } else {
        counters.shader_binary_bytes += 80;
    }
    counters.guest_to_host_bytes += 80;
    let mut terminated = false;
    for _ in 0..128 {
        let address = source
            .0
            .checked_add(binary.len() as u64)
            .ok_or_else(|| invalid("shader fetch overflow"))?;
        if address.checked_add(32).is_none_or(|end| end > 1 << 40) {
            return Err(invalid("shader fetch exceeds 40-bit address space"));
        }
        let bundle = map.read(memory, GpuAddress(address), 32)?;
        if bundle.len() != 32 {
            return Err(invalid("short guest shader bundle"));
        }
        binary.extend_from_slice(bundle);
        if vertex {
            counters.vertex_shader_bytes += 32;
        } else {
            counters.shader_binary_bytes += 32;
        }
        counters.guest_to_host_bytes += 32;
        if bundle[8..]
            .chunks_exact(8)
            .any(|word| u64::from_le_bytes(word.try_into().unwrap()) == 0xe30000000007000f)
        {
            terminated = true;
            break;
        }
    }
    if !terminated {
        return Err(invalid("shader has no EXIT within 4 KiB code budget"));
    }
    Ok(binary)
}
