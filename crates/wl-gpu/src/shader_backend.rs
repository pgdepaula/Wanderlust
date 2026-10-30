//! Runtime binding for the narrow translated fragment harness, not a hardware draw.
use crate::{
    Error, Result, backend::Renderer, invalid, maxwell::Maxwell3d, memory::*,
    shader_program::FragmentProgram, texture::TextureChanges,
};

#[derive(Clone, Copy, PartialEq, Eq)]
struct Stamp {
    address: GuestAddress,
    observed: (u64, u64),
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
    pub(crate) buffer: wgpu::Buffer,
    data: [u8; 256],
    stamps: [Option<Stamp>; 64],
}
#[derive(Clone, Copy)]
pub(crate) struct SelectedShader {
    guest: GuestAddress,
    size: usize,
    observed: (u64, u64),
}
impl Renderer {
    pub(crate) fn invalidate_shader_caches(&mut self, flags: u32) {
        if flags & 1 != 0 {
            self.selected_shader = None;
            self.selected_vertex = None;
            self.vertex_ready = false;
        }
        if flags & 0x1000 != 0 {
            if let Some(fragment) = self.resource_fragment.as_mut() {
                fragment.stamps.fill(None);
            }
        }
        // No admitted shader instruction performs cached global-memory loads.
        // The GLOBAL_DATA bit therefore has no resident LDG data to discard.
    }

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
                && changes.unchanged_since(
                    cache.guest.0..cache.guest.0 + cache.size as u64,
                    cache.observed,
                )
        });
        if cached.is_some() {
            self.selected_shader
                .as_mut()
                .expect("cached shader")
                .observed = changes.observation();
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
                observed: changes.observation(),
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
                    visibility: wgpu::ShaderStages::VERTEX_FRAGMENT,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Uniform,
                        has_dynamic_offset: false,
                        min_binding_size: wgpu::BufferSize::new(512),
                    },
                    count: None,
                }],
            });
        let buffer = self.device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("packed Maxwell constant words"),
            size: 512,
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
                bind_group_layouts: &if program.uses_texture() {
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
        let bound_reference = f
            .program
            .bound_texture_offset()
            .map(|offset| {
                Ok(crate::shader_program::ConstantRef {
                    slot: engine
                        .bindless_texture_constant_slot()
                        .ok_or_else(|| invalid("missing bound-texture CB selector"))?,
                    offset,
                })
            })
            .transpose()?;
        if f.program.constants().len() + usize::from(bound_reference.is_some()) > stamps.len() {
            return Err(invalid("too many fragment resource words"));
        }
        for (n, reference) in f
            .program
            .constants()
            .iter()
            .copied()
            .chain(bound_reference)
            .enumerate()
        {
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
                observed: changes.observation(),
            };
            if !stamps[n].is_some_and(|old| {
                old.address == guest && changes.unchanged_since(guest.0..guest.0 + 4, old.observed)
            }) {
                let word = cb.read_word(reference.offset, map, memory)?;
                data[n * 4..n * 4 + 4].copy_from_slice(&word.to_le_bytes());
                self.counters.constant_guest_bytes += 4;
                self.counters.guest_to_host_bytes += 4;
            }
            // Advance only after a proved hit or a successful acquisition.
            // Missing journal history still forces a conservative reread.
            stamps[n] = Some(stamp);
        }
        // Resolve the uniform handle through actual descriptor pools before uploads.
        let texture = if let Some(input) = f
            .program
            .texture_input()
            .map(usize::from)
            .or_else(|| bound_reference.map(|_| f.program.constants().len()))
        {
            let n = input * 4;
            let handle = u32::from_le_bytes(data[n..n + 4].try_into().unwrap());
            let subchannel = engine.texture_subchannel()?;
            let stamp = engine.descriptor_stamp(subchannel, handle, map, changes)?;
            let desc = if let Some((_, desc)) = f.descriptor.filter(|(old, _)| *old == stamp) {
                desc
            } else {
                self.counters.descriptor_reads += 2;
                engine.resolve_handle(subchannel, handle, map, memory)?
            };
            if f.program.bound_texture_offset().is_some() && desc.mip_count != 1 {
                return Err(invalid(
                    "implicit-LOD TEXS requires one mip in the supported texture-view subset",
                ));
            }
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

/// Opt-in first-draw acquisition of both selected stages, before resource
/// validation can stop normal execution. No decode, cache or GPU state changes.
/// Caller must use a separate protected source whose hazard flag is discarded.
pub(crate) fn capture_selected_program_diagnostics(
    engine: &Maxwell3d,
    map: &AddressMap,
    memory: &impl GuestMemory,
    counters: &mut crate::Counters,
) {
    use std::sync::{
        OnceLock,
        atomic::{AtomicBool, Ordering},
    };
    static ENABLED: OnceLock<bool> = OnceLock::new();
    static CLAIMED: AtomicBool = AtomicBool::new(false);
    if !*ENABLED.get_or_init(|| std::env::var_os("WL_GPU_SHADER_DIAGNOSTICS_DIR").is_some())
        || CLAIMED.swap(true, Ordering::Relaxed)
    {
        return;
    }
    for (source, vertex) in [
        (engine.vertex_program_address(), true),
        (engine.fragment_program().map(|s| s.address), false),
    ] {
        if let Ok(address) = source {
            // Acquisition contributes to existing guest/shader-byte counters;
            // file failures and unsupported headers never alter draw validity.
            let _ = read_program(address, map, memory, counters, vertex);
        }
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
    capture_shader_diagnostic(source, &binary, vertex, terminated);
    if !terminated {
        return Err(invalid("shader has no EXIT within 4 KiB code budget"));
    }
    Ok(binary)
}

/// Private, opt-in acquisition evidence, never executable shader cache state.
/// At most 64 already-bounded programs/prefixes per process. Failures cannot
/// affect guest execution; no guest borrows or host GPU resources are retained.
fn capture_shader_diagnostic(source: GpuAddress, binary: &[u8], vertex: bool, terminated: bool) {
    use std::{
        fs,
        io::Write,
        sync::atomic::{AtomicUsize, Ordering},
    };
    static ATTEMPTS: AtomicUsize = AtomicUsize::new(0);
    let Some(directory) = std::env::var_os("WL_GPU_SHADER_DIAGNOSTICS_DIR") else {
        return;
    };
    let Ok(index) = ATTEMPTS.fetch_update(Ordering::Relaxed, Ordering::Relaxed, |n| {
        (n < 64).then_some(n + 1)
    }) else {
        return;
    };
    let directory = std::path::PathBuf::from(directory);
    if fs::create_dir_all(&directory).is_err() {
        return;
    }
    let name = format!(
        "{}-{index:02}-{}-{:010x}-{}.gm107",
        std::process::id(),
        if vertex { "vertex" } else { "pixel" },
        source.0,
        if terminated { "exit" } else { "prefix" }
    );
    let final_path = directory.join(name);
    let temporary = final_path.with_extension("part");
    let _ = (|| -> std::io::Result<()> {
        let mut options = fs::OpenOptions::new();
        options.write(true).create_new(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        let mut file = options.open(&temporary)?;
        file.write_all(binary)?;
        file.flush()?;
        drop(file);
        fs::hard_link(&temporary, &final_path)
    })();
    let _ = fs::remove_file(temporary);
}
