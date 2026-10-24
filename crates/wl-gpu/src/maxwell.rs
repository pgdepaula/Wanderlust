//! MAXWELL_B (B197) texture pool state and modern block-linear TIC/TSC subset.
//! Register offsets are bytes; FIFO packet addresses are separately word-based.
use crate::{Error, Result, fifo::MethodWrite, format::Format, invalid, memory::*, texture::*};

pub const CLASS: u32 = 0xb197;
/// Eviction preference only, not a flush, memory barrier or visibility event.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CacheEviction {
    First,
    Normal,
    Last,
}
pub const CACHE_POLICY_METHODS: [u16; 6] = [0x0218, 0x1000, 0x10fc, 0x1290, 0x12d8, 0x12dc];

/// Retained configuration only. No 2D launch or memory transfer is implemented.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct TwoDConfiguration {
    pub source_copy: bool,
    pub clipping_disabled: bool,
    pub corral_size: Option<u16>,
}

/// Configuration whose observations are restricted by the admitted shader and
/// topology subset. This does not provide statistics reports or line rendering.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct AuxiliaryRasterControls {
    pub zcull_statistics: Option<bool>,
    /// Hierarchical-test permissions (depth, stencil), not fragment-test enables.
    pub zcull_tests: Option<(bool, bool)>,
    /// Hierarchical bounds hints (minimum unbounded, maximum unbounded).
    pub zcull_unbounded: Option<(bool, bool)>,
    pub zcull_region: Option<u8>,
    /// Packed SFUNC/SREF/SMASK; force-match/no-invalidate remain unsupported.
    pub zcull_stencil_criterion: Option<u32>,
    pub aliased_line_width: Option<bool>,
    pub conditional_constant_load: Option<bool>,
    pub pixel_sample_mask_usage: Option<u8>,
    pub allow_float_pixel_kills: Option<bool>,
    pub zero_times_anything_is_zero: Option<bool>,
    pub draw_generated_polygon_edges: Option<bool>,
    pub two_sided_stencil: Option<bool>,
    pub spa_version: Option<u16>,
    pub local_memory_window: Option<u32>,
    pub target_independent_raster_control: Option<u8>,
    pub balanced_primitive_workload: Option<u8>,
    pub geometry_selects_target_layer: Option<bool>,
    pub compression_sample_threshold: Option<u16>,
    pub pixel_warp_watermarks: Option<(u16, u16)>,
}

/// Classes whose SET_OBJECT and documented NOP can be routed safely. Binding
/// a class does not imply implementation of its execution methods.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u32)]
pub enum EngineClass {
    ThreeD = CLASS,
    Compute = 0xb1c0,
    DmaCopy = 0xb0b5,
    InlineMemory = 0xa140,
    TwoD = 0x902d,
}
impl EngineClass {
    fn decode(value: u32) -> Option<Self> {
        match value {
            CLASS => Some(Self::ThreeD),
            0xb1c0 => Some(Self::Compute),
            0xb0b5 => Some(Self::DmaCopy),
            0xa140 => Some(Self::InlineMemory),
            0x902d => Some(Self::TwoD),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MethodError {
    InvalidSubchannel(u8),
    InvalidMethod(u16),
    UnboundSubchannel(u8),
    UnsupportedClass(u32),
    UnsupportedEngineMethod { class: u32, method: u16 },
    Macro(crate::mme::MacroError),
    UnsupportedMethod(u16),
    UnsupportedValue { method: u16, value: u32 },
    EpochOverflow,
    IncompleteConstantBuffer,
    IncompletePrimitiveRestart,
}
impl std::fmt::Display for MethodError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "Maxwell method fault: {self:?}")
    }
}
impl std::error::Error for MethodError {}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MethodEffect {
    State,
    TextureHeadersInvalidated(Option<u32>),
    SamplersInvalidated(Option<u32>),
    Draw { first: u32, count: u32 },
    DrawIndices { first: u32, count: u32 },
    Clear { color: bool, depth: bool },
    ConstantWrite { address: GpuAddress, value: u32 },
    WaitIdle,
}
#[derive(Debug, Clone, Copy, Default)]
struct Pool {
    upper: Option<u32>,
    lower: Option<u32>,
    maximum: Option<u32>,
}
impl Pool {
    fn descriptor(&self, index: u32) -> Result<GpuAddress> {
        let (Some(high), Some(low), Some(maximum)) = (self.upper, self.lower, self.maximum) else {
            return Err(invalid("incomplete Maxwell descriptor pool"));
        };
        if index > maximum {
            return Err(invalid("descriptor index exceeds inclusive pool limit"));
        }
        let base = (u64::from(high) << 32) | u64::from(low);
        // Accepted implementation subset, not an inferred hardware fault rule.
        if base % 32 != 0 {
            return Err(invalid("descriptor pool base must be 32-byte aligned"));
        }
        let address = base
            .checked_add(u64::from(index) * 32)
            .ok_or_else(|| invalid("descriptor address overflow"))?;
        if address.checked_add(32).is_none_or(|end| end > 1 << 40) {
            return Err(invalid("descriptor exceeds 40-bit pool address space"));
        }
        Ok(GpuAddress(address))
    }
}
/// A stage/slot binding snapshots the selector at BIND_GROUP_CONSTANT_BUFFER.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ConstantBuffer {
    pub address: GpuAddress,
    pub size: u32,
}
impl ConstantBuffer {
    pub fn read_word(
        &self,
        offset: u32,
        map: &AddressMap,
        memory: &impl GuestMemory,
    ) -> Result<u32> {
        if offset % 4 != 0 || offset.checked_add(4).is_none_or(|end| end > self.size) {
            return Err(invalid("unaligned/out-of-bounds constant buffer word"));
        }
        let address = self
            .address
            .0
            .checked_add(u64::from(offset))
            .ok_or_else(|| invalid("constant address overflow"))?;
        let bytes = map.read(memory, GpuAddress(address), 4)?;
        if bytes.len() != 4 {
            return Err(invalid("short constant buffer read"));
        }
        Ok(u32::from_le_bytes(
            bytes.try_into().expect("checked constant word"),
        ))
    }
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct DescriptorStamp {
    image: GuestAddress,
    sampler: GuestAddress,
    image_version: (u64, u64),
    sampler_version: (u64, u64),
    image_epoch: u64,
    sampler_epoch: u64,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ProgramSource {
    pub address: GpuAddress,
    pub registers: u8,
    pub group: u8,
}
/// Backward-compatible name for the pixel-stage source descriptor.
pub type FragmentSource = ProgramSource;
/// Supported attribute-zero float component widths. Missing FLOAT3 W is filled
/// with one by native vertex fetch; guest storage is never expanded on the CPU.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PositionFormat {
    Float32x3,
    Float32x4,
}
impl PositionFormat {
    pub fn byte_size(self) -> u16 {
        match self {
            Self::Float32x3 => 12,
            Self::Float32x4 => 16,
        }
    }
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct VertexArray {
    pub address: GpuAddress,
    pub limit: GpuAddress,
    pub stride: u16,
    pub offsets: [u16; 2],
    pub position_format: PositionFormat,
}
impl VertexArray {
    pub fn size(&self, first: u32, count: u32) -> Result<usize> {
        if count % 3 != 0 {
            return Err(invalid("invalid triangle vertex count"));
        }
        self.span(first, count)
    }
    pub fn span(&self, first: u32, count: u32) -> Result<usize> {
        let size = first
            .checked_add(count)
            .and_then(|n| u64::from(n).checked_mul(u64::from(self.stride)))
            .ok_or_else(|| invalid("vertex range overflow"))?;
        if count == 0
            || size == 0
            || self
                .address
                .0
                .checked_add(size - 1)
                .is_none_or(|end| end > self.limit.0)
        {
            return Err(invalid("vertex fetch exceeds inclusive limit"));
        }
        usize::try_from(size).map_err(|_| invalid("vertex span exceeds host"))
    }
}
/// One channel's single MAXWELL_B instance. Subchannels route to the same state;
/// rebinding the same class does not manufacture an independent register bank.
#[derive(Debug, Default)]
pub struct Maxwell3d {
    method_writes: u64,
    raster: crate::raster::RasterRegisters,
    indices: crate::index::IndexRegisters,
    bound: [Option<EngineClass>; 8],
    macros: crate::mme::MacroEngine,
    macro_work: crate::mme::WorkBudget,
    active_macro: Option<ActiveMacro>,
    cache_policies: [Option<CacheEviction>; 6],
    two_d: TwoDConfiguration,
    auxiliary_raster: AuxiliaryRasterControls,
    instrumentation_header: Option<u32>,
    instrumentation_data: Option<u32>,
    bindless_texture_constant_slot: Option<u8>,
    vertex_id_uses_array_start: Option<bool>,
    provoking_last: Option<bool>,
    api_call_limit: Option<u16>,
    attribute_defaults: Option<u8>,
    blend_other_formats: Option<bool>,
    modern_headers: bool,
    images: Pool,
    program_upper: Option<u32>,
    program_lower: Option<u32>,
    vertex_enabled: bool,
    vertex_offset: Option<u32>,
    vertex_registers: Option<u8>,
    vertex_group: Option<u8>,
    vertex_format: Option<u32>,
    vertex_upper: Option<u32>,
    vertex_lower: Option<u32>,
    vertex_limit_upper: Option<u32>,
    vertex_limit_lower: Option<u32>,
    vertex_substitute_upper: Option<u8>,
    vertex_substitute_lower: Option<u32>,
    vertex_frequency: Option<u32>,
    vertex_instance: [Option<u32>; 16],
    vertex_attributes: [Option<u32>; 2],
    fragment_enabled: bool,
    fragment_offset: Option<u32>,
    fragment_registers: Option<u8>,
    fragment_group: Option<u8>,
    cb_size: Option<u32>,
    cb_upper: Option<u32>,
    cb_lower: Option<u32>,
    cb_offset: Option<u32>,
    constants: [[Option<ConstantBuffer>; 32]; 5],
    samplers: Pool,
    pub texture_header_epoch: u64,
    pub sampler_epoch: u64,
}
#[derive(Debug)]
struct ActiveMacro {
    subchannel: u8,
    parameter_method: u16,
    invocation: crate::mme::Invocation,
    pending_backend: Option<MethodEffect>,
    idle_completed: bool,
}
impl Maxwell3d {
    /// Configured runout address. Supported fetches still require in-bounds
    /// geometry; no substitute data or host default is silently read.
    pub fn vertex_stream_substitute_address(&self) -> Option<GpuAddress> {
        Some(GpuAddress(
            (u64::from(self.vertex_substitute_upper?) << 32)
                | u64::from(self.vertex_substitute_lower?),
        ))
    }
    pub(crate) fn begin_submission(&mut self) -> std::result::Result<(), MethodError> {
        if self.active_macro.is_some() {
            return Err(MethodError::Macro(crate::mme::MacroError::ActiveInvocation));
        }
        self.macro_work.reset();
        Ok(())
    }
    /// Retained source-verified initialization metadata, not profiling results.
    pub fn instrumentation_initialization(&self) -> (Option<u32>, Option<u32>) {
        (self.instrumentation_header, self.instrumentation_data)
    }
    /// Resume only after the previously returned backend effect was executed.
    pub(crate) fn resume_macro(
        &mut self,
    ) -> std::result::Result<Option<MethodEffect>, MethodError> {
        let Some(mut active) = self.active_macro.take() else {
            return Ok(None);
        };
        // Only a completed WFI grants privileged initialization permission.
        // Draws, writes and invalidations must never masquerade as idle waits.
        if let Some(effect) = active.pending_backend.take() {
            active.idle_completed = effect == MethodEffect::WaitIdle;
        }
        loop {
            let Some((method, value)) = self
                .macros
                .next_output_budgeted(&mut active.invocation, &mut self.macro_work)
                .map_err(MethodError::Macro)?
            else {
                if active.invocation.waiting_for_parameter() {
                    self.active_macro = Some(active);
                }
                return Ok(None);
            };
            if method == 0x2310 {
                self.method_writes = self.method_writes.saturating_add(1);
                self.macros
                    .initialize_privileged_register(value, active.idle_completed)
                    .map_err(MethodError::Macro)?;
                active.idle_completed = false;
                continue;
            }
            // Reuse the strict B197 dispatcher. Channel/engine rebinding,
            // mutation of the running program and recursive launches are not
            // permitted through this bounded MME execution interface.
            if method < 0x100 || (0x114..=0x120).contains(&method) || method >= 0x3800 {
                return Err(MethodError::Macro(
                    crate::mme::MacroError::UnsupportedOutput(method),
                ));
            }
            let effect = self.apply(MethodWrite {
                subchannel: active.subchannel,
                method,
                value,
            })?;
            if !matches!(
                effect,
                MethodEffect::State
                    | MethodEffect::TextureHeadersInvalidated(_)
                    | MethodEffect::SamplersInvalidated(_)
            ) {
                active.pending_backend = Some(effect);
                self.active_macro = Some(active);
                return Ok(Some(effect));
            }
        }
    }
    pub(crate) fn macro_waiting_for_parameter(&self) -> bool {
        self.active_macro.as_ref().is_some_and(|active| {
            active.pending_backend.is_none() && active.invocation.waiting_for_parameter()
        })
    }
    pub fn bindless_texture_constant_slot(&self) -> Option<u8> {
        self.bindless_texture_constant_slot
    }
    pub fn auxiliary_raster_controls(&self) -> AuxiliaryRasterControls {
        self.auxiliary_raster
    }
    pub fn two_d_configuration(&self) -> TwoDConfiguration {
        self.two_d
    }
    pub fn vertex_id_uses_array_start(&self) -> Option<bool> {
        self.vertex_id_uses_array_start
    }
    pub fn provoking_vertex_last(&self) -> Option<bool> {
        self.provoking_last
    }
    pub fn api_call_limit(&self) -> Option<u16> {
        self.api_call_limit
    }
    pub fn attribute_defaults(&self) -> Option<u8> {
        self.attribute_defaults
    }
    pub fn blend_other_formats_enabled(&self) -> Option<bool> {
        self.blend_other_formats
    }
    pub fn vertex_array_instance(&self, slot: usize) -> Option<bool> {
        self.vertex_instance
            .get(slot)
            .copied()
            .flatten()
            .map(|v| v != 0)
    }
    pub fn cache_eviction(&self, method: u16) -> Option<CacheEviction> {
        CACHE_POLICY_METHODS
            .iter()
            .position(|m| *m == method)
            .and_then(|index| self.cache_policies[index])
    }
    pub fn bound_class(&self, subchannel: u8) -> Option<EngineClass> {
        self.bound.get(subchannel as usize).copied().flatten()
    }
    /// Ordinary register writes allocate nothing. First MME code upload owns a
    /// bounded RAM allocation; subsequent updates reuse it. Stop on a fault.
    pub fn apply(&mut self, write: MethodWrite) -> std::result::Result<MethodEffect, MethodError> {
        self.method_writes = self.method_writes.saturating_add(1);
        let MethodWrite {
            subchannel,
            method,
            value,
        } = write;
        if subchannel >= 8 {
            return Err(MethodError::InvalidSubchannel(subchannel));
        }
        if method % 4 != 0 || method > 0x3ffc {
            return Err(MethodError::InvalidMethod(method));
        }
        if let Some(active) = self.active_macro.as_mut() {
            if active.pending_backend.is_none()
                && subchannel == active.subchannel
                && method == active.parameter_method
            {
                active
                    .invocation
                    .provide_parameter(value)
                    .map_err(MethodError::Macro)?;
                return Ok(self.resume_macro()?.unwrap_or(MethodEffect::State));
            }
            return Err(MethodError::Macro(crate::mme::MacroError::ActiveInvocation));
        }
        let bad = || MethodError::UnsupportedValue { method, value };
        // NVB06F_NOP is a channel method, independent of subchannel binding.
        // It is distinct from engine NO_OPERATION at byte offset 0x100.
        if method == 8 {
            return Ok(MethodEffect::State);
        }
        if method == 0 {
            // Engine instance zero only; SET_OBJECT carries a class, not texture ID.
            let class = EngineClass::decode(value).ok_or(MethodError::UnsupportedClass(value))?;
            self.bound[subchannel as usize] = Some(class);
            return Ok(MethodEffect::State);
        }
        let class = self
            .bound_class(subchannel)
            .ok_or(MethodError::UnboundSubchannel(subchannel))?;
        if class != EngineClass::ThreeD {
            if class == EngineClass::TwoD {
                match method {
                    0x02ac => {
                        if value != 3 {
                            return Err(bad());
                        }
                        self.two_d.source_copy = true;
                        return Ok(MethodEffect::State);
                    }
                    0x0290 => {
                        if value != 0 {
                            return Err(bad());
                        }
                        self.two_d.clipping_disabled = true;
                        return Ok(MethodEffect::State);
                    }
                    0x0884 => {
                        if value > 0x3ff {
                            return Err(bad());
                        }
                        self.two_d.corral_size = Some(value as u16);
                        return Ok(MethodEffect::State);
                    }
                    _ => {}
                }
            }
            // All five pinned classes define their NOP at this offset. No
            // compute, DMA, inline upload or 2D action is routed into B197.
            return if method == 0x100 {
                Ok(MethodEffect::State)
            } else {
                Err(MethodError::UnsupportedEngineMethod {
                    class: class as u32,
                    method,
                })
            };
        }
        if self
            .macros
            .upload(method, value)
            .map_err(MethodError::Macro)?
        {
            return Ok(MethodEffect::State);
        }
        if (0x3800..0x3c00).contains(&method) && method % 8 == 0 {
            let invocation = self
                .macros
                .begin((method - 0x3800) / 8, value)
                .map_err(MethodError::Macro)?;
            self.active_macro = Some(ActiveMacro {
                subchannel,
                parameter_method: method + 4,
                invocation,
                pending_backend: None,
                idle_completed: false,
            });
            return Ok(self.resume_macro()?.unwrap_or(MethodEffect::State));
        }
        if let Some(index) = CACHE_POLICY_METHODS.iter().position(|m| *m == method) {
            // POLICY bits 5:4 are preferences. Keep VAF system-memory volatility
            // FALSE; TRUE needs a separately verified memory-access contract.
            let policy = match value {
                0 => CacheEviction::First,
                0x10 => CacheEviction::Normal,
                0x20 => CacheEviction::Last,
                _ => return Err(bad()),
            };
            self.cache_policies[index] = Some(policy);
            return Ok(MethodEffect::State);
        }
        match method {
            0x0f84 => {
                if value > 0xff {
                    return Err(bad());
                }
                self.vertex_substitute_upper = Some(value as u8);
            }
            0x0f88 => self.vertex_substitute_lower = Some(value),
            0x0150 => {
                // Only the deko3d baseline initialization pair is admitted.
                // Unknown instrumentation protocols remain explicit errors.
                if value != 0x49000000 {
                    return Err(bad());
                }
                self.instrumentation_header = Some(value);
                self.instrumentation_data = None;
            }
            0x0154 => {
                if self.instrumentation_header != Some(0x49000000)
                    || self.instrumentation_data.is_some()
                    || value != 0x49000001
                {
                    return Err(bad());
                }
                self.instrumentation_data = Some(value);
            }
            0x0dd8 => {
                if value & 0x0000_ff00 != 0 || value & 0xff > 7 {
                    return Err(bad());
                }
                self.auxiliary_raster.zcull_stencil_criterion = Some(value);
            }
            0x1590 => {
                if value > 0x3f {
                    return Err(bad());
                }
                self.auxiliary_raster.zcull_region = Some(value as u8);
            }
            0x1968 | 0x196c => {
                if value & !0x11 != 0 {
                    return Err(bad());
                }
                // Keep the optimization controls while performing complete
                // per-fragment host tests. No ZCULL metadata or reports are
                // synthesized, and these hints cannot enable stencil/depth.
                let bits = Some((value & 1 != 0, value & 0x10 != 0));
                if method == 0x1968 {
                    self.auxiliary_raster.zcull_tests = bits;
                } else {
                    self.auxiliary_raster.zcull_unbounded = bits;
                }
            }
            0x0110 => {
                if value != 0 {
                    return Err(bad());
                }
                return Ok(MethodEffect::WaitIdle);
            }
            0x2608 => {
                if value > 31 {
                    return Err(bad());
                }
                // TEX.B obtains its handle from a GPR and explicit LDC dataflow,
                // not this selector. Automatic CB handle forms remain rejected.
                self.bindless_texture_constant_slot = Some(value as u8);
            }
            0x1220 => {
                if value > 12 {
                    return Err(bad());
                }
                // Retain the documented sample-count enum. This does not add
                // compression tags, compressed attachments or a physical codec.
                self.auxiliary_raster.compression_sample_threshold =
                    Some(if value == 0 { 0 } else { 1u16 << (value - 1) });
            }
            0x1450 => {
                let low = value as u16;
                let high = (value >> 16) as u16;
                if low > high {
                    return Err(bad());
                }
                // Ordered watermarks only. Host warp scheduling remains wgpu's
                // responsibility, not an emulated timing/completion primitive.
                self.auxiliary_raster.pixel_warp_watermarks = Some((low, high));
            }
            0x0f8c | 0x1594 => {
                if value > 1 {
                    return Err(bad());
                }
                // Polygon LINE mode and stencil enable remain rejected by
                // raster validation, independently of these retained controls.
                if method == 0x0f8c {
                    self.auxiliary_raster.draw_generated_polygon_edges = Some(value == 0);
                } else {
                    self.auxiliary_raster.two_sided_stencil = Some(value != 0);
                }
            }
            0x0310 => {
                if value != 0x0503 {
                    return Err(bad());
                }
                self.auxiliary_raster.spa_version = Some(value as u16);
            }
            0x077c => {
                // A shader-local window base is not a mapped GPU allocation.
                // Neither SPH decoder admits local-memory storage or accesses.
                self.auxiliary_raster.local_memory_window = Some(value);
            }
            0x1130 | 0x0374 => {
                let mask = if method == 0x1130 { 0x13 } else { 0x11 };
                if value & !mask != 0 {
                    return Err(bad());
                }
                if method == 0x1130 {
                    // Only one raster/color sample, no alpha-to-coverage, and
                    // no ZPASS query are admitted by the execution subset.
                    self.auxiliary_raster.target_independent_raster_control = Some(value as u8);
                } else {
                    self.auxiliary_raster.balanced_primitive_workload = Some(value as u8);
                }
            }
            0x15cc => {
                if !matches!(value, 0 | 0x10000) {
                    return Err(bad());
                }
                // Layer zero only; no geometry shader/layer export is admitted.
                self.auxiliary_raster.geometry_selects_target_layer = Some(value != 0);
            }
            0x0fdc => {
                if value > 1 {
                    return Err(bad());
                }
                // Optimization permission, not a request to kill a fragment.
                // Float color attachments remain outside the target subset.
                self.auxiliary_raster.allow_float_pixel_kills = Some(value != 0);
            }
            0x19c0 => {
                if value != 1 {
                    return Err(bad());
                }
                // Admitted only with the existing RGBA8_UNORM color-target
                // subset. IEEE propagation for float targets is not implied.
                self.auxiliary_raster.zero_times_anything_is_zero = Some(true);
            }
            0x151c | 0x020c => {
                if value > 1 {
                    return Err(bad());
                }
                if method == 0x151c {
                    self.auxiliary_raster.zcull_statistics = Some(value != 0);
                } else {
                    self.auxiliary_raster.aliased_line_width = Some(value != 0);
                }
            }
            0x030c => {
                if value != 0 {
                    return Err(bad());
                }
                self.auxiliary_raster.conditional_constant_load = Some(false);
            }
            0x0300 => {
                if value & !3 != 0 {
                    return Err(bad());
                }
                // Both strict SPH decoders reject sample-mask exports. Enabling
                // use of a nonexistent output cannot create host sample masks.
                self.auxiliary_raster.pixel_sample_mask_usage = Some(value as u8);
            }
            0x164c => {
                if value & !0x1000 != 0 {
                    return Err(bad());
                }
                // Guest VertexID loads are not admitted by the shader decoder.
                // This does not change position/UV vertex-array fetching.
                self.vertex_id_uses_array_start = Some(value != 0);
            }
            0x0d64 => {
                if value > 8 {
                    return Err(bad());
                }
                self.api_call_limit = Some(if value == 0 { 0 } else { 1u16 << (value - 1) });
            }
            0x1140 => {
                if value & !0x10 != 0 {
                    return Err(bad());
                }
                // This gate names SNORM8/UNORM16/SNORM16, all unsupported color
                // target formats. RGBA8 blending keeps its existing state.
                self.blend_other_formats = Some(value != 0);
            }
            0x1610 => {
                // Match the independently pinned canonical 0001 expansion.
                // Other defaults need separately verified Float3/component
                // expansion and interface handling, rather than host defaults.
                if value != 0x0e {
                    return Err(bad());
                }
                // Missing/default vertex attributes still fail interface checks.
                self.attribute_defaults = Some(value as u8);
            }
            0x165c => {
                if value != 0 {
                    return Err(bad());
                } // OGL point center; no points are rendered.
            }
            0x1684 => {
                if value > 1 {
                    return Err(bad());
                }
                // Guest Flat IPA remains rejected; all admitted varyings are
                // smooth/screen-linear and cannot observe the provoking vertex.
                self.provoking_last = Some(value != 0);
            }
            0x16a8 | 0x1794 => {
                // Only the SPH3/AAM2 format pairs used by this translator's
                // pinned Maxwell layout are admitted, not a firmware emulator.
                let version = if method == 0x16a8 { 3 } else { 2 };
                if value != (version | (version << 16)) {
                    return Err(bad());
                }
            }
            0x1234 | 0x1528 => {
                // Existing path: independent TIC/TSC indices, no shader traps.
                // Header-bound samplers and enabled shader exceptions need new
                // resource/exception semantics and remain explicit failures.
                if value != 0 {
                    return Err(bad());
                }
            }
            0x1608 => {
                if value > 0xff {
                    return Err(bad());
                }
                self.program_upper = Some(value);
            }
            0x160c => self.program_lower = Some(value),
            0x2040 => {
                if !matches!(value, 0x10 | 0x11) {
                    return Err(bad());
                }
                self.vertex_enabled = value & 1 != 0;
            }
            0x2044 => self.vertex_offset = Some(value),
            0x204c => {
                if value == 0 || value > 255 {
                    return Err(bad());
                }
                self.vertex_registers = Some(value as u8);
            }
            0x2050 => {
                if value > 4 {
                    return Err(bad());
                }
                self.vertex_group = Some(value as u8);
            }
            0x1c00 => {
                if value & !0x1fff != 0 {
                    return Err(bad());
                }
                self.vertex_format = Some(value);
            }
            0x1c04 | 0x1f00 => {
                if value > 255 {
                    return Err(bad());
                }
                if method == 0x1c04 {
                    self.vertex_upper = Some(value);
                } else {
                    self.vertex_limit_upper = Some(value);
                }
            }
            0x1c08 => self.vertex_lower = Some(value),
            0x1f04 => self.vertex_limit_lower = Some(value),
            0x1c0c => {
                if value != 1 {
                    return Err(bad());
                }
                self.vertex_frequency = Some(value);
            }
            0x1880..=0x18bc => {
                let slot = usize::from((method - 0x1880) / 4);
                if value > 1 || (slot == 0 && value != 0) {
                    return Err(bad());
                }
                self.vertex_instance[slot] = Some(value);
            }
            0x1160 | 0x1164 => {
                let fields = value & !(0x3fffu32 << 7);
                let valid = if method == 0x1160 {
                    matches!(fields, 0x38200000 | 0x38400000)
                } else {
                    fields == 0x38800000
                };
                if !valid {
                    return Err(bad());
                }
                self.vertex_attributes[((method - 0x1160) / 4) as usize] = Some(value);
            }
            0x2140 => {
                if !matches!(value, 0x50 | 0x51) {
                    return Err(bad());
                }
                self.fragment_enabled = value & 1 != 0;
            }
            0x2144 => self.fragment_offset = Some(value),
            0x214c => {
                if value == 0 || value > 255 {
                    return Err(bad());
                }
                self.fragment_registers = Some(value as u8);
            }
            0x2150 => {
                if value > 4 {
                    return Err(bad());
                }
                self.fragment_group = Some(value as u8);
            }
            0x2380 => {
                if value == 0 || value > 65536 || value % 4 != 0 {
                    return Err(bad());
                }
                self.cb_size = Some(value);
            }
            0x2384 => {
                if value > 0xff {
                    return Err(bad());
                }
                self.cb_upper = Some(value);
            }
            0x2388 => {
                if value % 4 != 0 {
                    return Err(bad());
                }
                self.cb_lower = Some(value);
            }
            0x238c => {
                if value > 0xffff || value % 4 != 0 {
                    return Err(bad());
                }
                self.cb_offset = Some(value);
            }
            0x2390..=0x23cc => {
                let (Some(size), Some(hi), Some(lo), Some(offset)) =
                    (self.cb_size, self.cb_upper, self.cb_lower, self.cb_offset)
                else {
                    return Err(MethodError::IncompleteConstantBuffer);
                };
                let end = offset.checked_add(4).ok_or_else(bad)?;
                let base = (u64::from(hi) << 32) | u64::from(lo);
                if end > size || base + u64::from(size) > 1 << 40 {
                    return Err(bad());
                }
                self.cb_offset = Some(end);
                return Ok(MethodEffect::ConstantWrite {
                    address: GpuAddress(base + u64::from(offset)),
                    value,
                });
            }
            m if (0x2410..=0x2490).contains(&m) && (m - 0x2410) % 32 == 0 => {
                if value & !0x1f1 != 0 {
                    return Err(bad());
                }
                let group = ((m - 0x2410) / 32) as usize;
                let slot = ((value >> 4) & 31) as usize;
                let binding = if value & 1 == 0 {
                    None
                } else {
                    let (Some(size), Some(hi), Some(lo)) =
                        (self.cb_size, self.cb_upper, self.cb_lower)
                    else {
                        return Err(MethodError::IncompleteConstantBuffer);
                    };
                    let address = (u64::from(hi) << 32) | u64::from(lo);
                    if address
                        .checked_add(u64::from(size))
                        .is_none_or(|end| end > 1 << 40)
                    {
                        return Err(bad());
                    }
                    Some(ConstantBuffer {
                        address: GpuAddress(address),
                        size,
                    })
                };
                self.constants[group][slot] = binding;
            }
            0x0100 => {} // documented NO_OPERATION
            0x0f10 => {
                if value != 1 {
                    return Err(bad());
                }
                self.modern_headers = true;
            }
            0x155c | 0x1574 => {
                if value & !0xff != 0 {
                    return Err(bad());
                }
                if method == 0x155c {
                    self.samplers.upper = Some(value);
                } else {
                    self.images.upper = Some(value);
                }
            }
            0x1560 => self.samplers.lower = Some(value),
            0x1578 => self.images.lower = Some(value),
            0x1564 => {
                if value & !0xfffff != 0 {
                    return Err(bad());
                }
                self.samplers.maximum = Some(value);
            }
            0x157c => {
                if value & !0x3fffff != 0 {
                    return Err(bad());
                }
                self.images.maximum = Some(value);
            }
            0x1424 | 0x1428 => {
                if value & !0x03fffff1 != 0 {
                    return Err(bad());
                }
                let index = (value & 1 != 0).then_some((value >> 4) & 0x3fffff);
                let epoch = if method == 0x1424 {
                    &mut self.sampler_epoch
                } else {
                    &mut self.texture_header_epoch
                };
                *epoch = epoch.checked_add(1).ok_or(MethodError::EpochOverflow)?;
                return Ok(if method == 0x1424 {
                    MethodEffect::SamplersInvalidated(index)
                } else {
                    MethodEffect::TextureHeadersInvalidated(index)
                });
            }
            0x1214 => {
                let first = value & 65535;
                let count = (value >> 16) & 4095;
                if value >> 28 != 4 || count == 0 || count % 3 != 0 {
                    return Err(bad());
                }
                if !self.indices.nonindexed_restart_ready() {
                    return Err(MethodError::IncompletePrimitiveRestart);
                }
                return Ok(MethodEffect::Draw { first, count });
            }
            0x19d0 => {
                if !matches!(value, 0x01 | 0x3c | 0x3d) {
                    return Err(bad());
                }
                return Ok(MethodEffect::Clear {
                    color: value & 0x3c != 0,
                    depth: value & 1 != 0,
                });
            }
            _ => {
                if let Some(effect) = self.indices.apply(method, value)? {
                    return Ok(effect);
                }
                if !self.raster.apply(method, value)? {
                    return Err(MethodError::UnsupportedMethod(method));
                }
            }
        }
        Ok(MethodEffect::State)
    }
    /// Counts attempted method writes, including rejected writes; saturates at u64::MAX.
    pub fn method_writes(&self) -> u64 {
        self.method_writes
    }
    pub fn clear_surface(&self, color: bool, depth: bool) -> Result<crate::raster::ClearSurface> {
        self.raster.resolve_clear(color, depth)
    }
    /// Pixel pipeline index five explicitly selects its constant-binding group.
    pub fn fragment_program(&self) -> Result<FragmentSource> {
        if !self.bound.contains(&Some(EngineClass::ThreeD)) || !self.fragment_enabled {
            return Err(invalid("pixel program disabled or B197 unbound"));
        }
        let (Some(hi), Some(lo), Some(offset), Some(registers), Some(group)) = (
            self.program_upper,
            self.program_lower,
            self.fragment_offset,
            self.fragment_registers,
            self.fragment_group,
        ) else {
            return Err(invalid("incomplete pixel program state"));
        };
        let address = ((u64::from(hi) << 32) | u64::from(lo))
            .checked_add(u64::from(offset))
            .ok_or_else(|| invalid("shader program address overflow"))?;
        if address >= 1 << 40 {
            return Err(invalid("shader program exceeds 40-bit address space"));
        }
        Ok(FragmentSource {
            address: GpuAddress(address),
            registers,
            group,
        })
    }
    pub fn index_buffer(&self, first: u32, count: u32) -> Result<crate::index::IndexBuffer> {
        self.indices.resolve(first, count)
    }
    pub fn rasterization(
        &self,
    ) -> Result<(crate::raster::ColorTarget, crate::raster::Rasterization)> {
        self.raster.resolve()
    }
    pub fn depth_target(&self) -> Result<Option<crate::raster::DepthTarget>> {
        self.raster.depth_target()
    }
    pub fn vertex_program(&self) -> Result<ProgramSource> {
        if !self.bound.contains(&Some(EngineClass::ThreeD)) || !self.vertex_enabled {
            return Err(invalid("vertex program disabled or B197 unbound"));
        }
        let (Some(hi), Some(lo), Some(offset), Some(registers), Some(group)) = (
            self.program_upper,
            self.program_lower,
            self.vertex_offset,
            self.vertex_registers,
            self.vertex_group,
        ) else {
            return Err(invalid("incomplete vertex program state"));
        };
        let address = ((u64::from(hi) << 32) | u64::from(lo))
            .checked_add(u64::from(offset))
            .filter(|a| *a < 1 << 40)
            .ok_or_else(|| invalid("vertex program address overflow"))?;
        Ok(ProgramSource {
            address: GpuAddress(address),
            registers,
            group,
        })
    }
    pub fn vertex_array(&self) -> Result<VertexArray> {
        let (
            Some(format),
            Some(hi),
            Some(lo),
            Some(lhi),
            Some(llo),
            Some(1),
            Some(0),
            [Some(a0), Some(a1)],
        ) = (
            self.vertex_format,
            self.vertex_upper,
            self.vertex_lower,
            self.vertex_limit_upper,
            self.vertex_limit_lower,
            self.vertex_frequency,
            self.vertex_instance[0],
            self.vertex_attributes,
        )
        else {
            return Err(invalid("incomplete vertex stream/attributes"));
        };
        let stride = (format & 4095) as u16;
        let offsets = [((a0 >> 7) & 0x3fff) as u16, ((a1 >> 7) & 0x3fff) as u16];
        let position_format = if (a0 >> 21) & 0x3f == 2 {
            PositionFormat::Float32x3
        } else {
            PositionFormat::Float32x4
        };
        let address = (u64::from(hi) << 32) | u64::from(lo);
        let limit = (u64::from(lhi) << 32) | u64::from(llo);
        if format & 0x1000 == 0
            || stride == 0
            || stride % 4 != 0
            || offsets.iter().any(|v| v % 4 != 0)
            || u32::from(offsets[0]) + u32::from(position_format.byte_size()) > u32::from(stride)
            || u32::from(offsets[1]) + 8 > u32::from(stride)
            || address % 4 != 0
            || limit < address
        {
            return Err(invalid("invalid vertex stream bounds/layout"));
        }
        Ok(VertexArray {
            address: GpuAddress(address),
            limit: GpuAddress(limit),
            stride,
            offsets,
            position_format,
        })
    }
    pub fn constant_buffer(&self, group: u8, slot: u8) -> Result<ConstantBuffer> {
        self.constants
            .get(group as usize)
            .and_then(|g| g.get(slot as usize))
            .copied()
            .flatten()
            .ok_or_else(|| invalid("unbound constant buffer group/slot"))
    }
    /// Maxwell's 32-bit combined handle: low 20 bits TIC, high 12 bits TSC.
    /// TEX.B supplies this handle from a register; SET_BINDLESS_TEXTURE instead
    /// selects a CB slot for the distinct bound-TEX instruction form.
    pub fn resolve_handle(
        &self,
        subchannel: u8,
        handle: u32,
        map: &AddressMap,
        memory: &impl GuestMemory,
    ) -> Result<TextureDesc> {
        self.resolve_texture(subchannel, handle & 0xfffff, handle >> 20, map, memory)
    }
    pub(crate) fn texture_subchannel(&self) -> Result<u8> {
        self.bound
            .iter()
            .position(|b| *b == Some(EngineClass::ThreeD))
            .map(|n| n as u8)
            .ok_or_else(|| invalid("no bound B197 subchannel"))
    }
    pub(crate) fn descriptor_stamp(
        &self,
        subchannel: u8,
        handle: u32,
        map: &AddressMap,
        changes: &TextureChanges,
    ) -> Result<DescriptorStamp> {
        if self.bound_class(subchannel) != Some(EngineClass::ThreeD) || !self.modern_headers {
            return Err(invalid("texture engine not configured"));
        }
        let image = map.resolve(self.images.descriptor(handle & 0xfffff)?, 32)?;
        let sampler = map.resolve(self.samplers.descriptor(handle >> 20)?, 32)?;
        Ok(DescriptorStamp {
            image,
            sampler,
            image_version: changes.stamp(image.0..image.0 + 32),
            sampler_version: changes.stamp(sampler.0..sampler.0 + 32),
            image_epoch: self.texture_header_epoch,
            sampler_epoch: self.sampler_epoch,
        })
    }
    /// Descriptor indices are supplied by a caller, not a fabricated bind method.
    /// Original shader handle/constant-buffer decoding is not implemented.
    pub fn resolve_texture(
        &self,
        subchannel: u8,
        image_index: u32,
        sampler_index: u32,
        map: &AddressMap,
        memory: &impl GuestMemory,
    ) -> Result<TextureDesc> {
        if self.bound_class(subchannel) != Some(EngineClass::ThreeD) || !self.modern_headers {
            return Err(invalid(
                "unbound engine or modern texture headers not selected",
            ));
        }
        let image = TextureImage::decode(read_descriptor(
            self.images.descriptor(image_index)?,
            map,
            memory,
        )?)?;
        let sampler = TextureSampler::decode(read_descriptor(
            self.samplers.descriptor(sampler_index)?,
            map,
            memory,
        )?)?;
        let desc = image.with_sampler(sampler);
        Ok(desc)
    }
}
fn read_descriptor(
    address: GpuAddress,
    map: &AddressMap,
    memory: &impl GuestMemory,
) -> Result<[u32; 8]> {
    let guest = map.resolve(address, 32)?;
    let bytes = memory.read(guest, 32)?;
    if bytes.len() != 32 {
        return Err(invalid("short descriptor read"));
    }
    Ok(std::array::from_fn(|n| {
        u32::from_le_bytes(
            bytes[n * 4..n * 4 + 4]
                .try_into()
                .expect("checked descriptor"),
        )
    }))
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TextureImage {
    pub address: GpuAddress,
    pub width: u32,
    pub height: u32,
    pub mip_count: u32,
    pub format: Format,
    pub layout: TextureLayout,
}
impl TextureImage {
    pub fn decode(words: [u32; 8]) -> Result<Self> {
        // These masks select implemented fields, not merely reserved bits. Fields
        // such as depth, sparse modes and multisampling are rejected explicitly.
        let masks = [
            u32::MAX,
            0xfffffe00,
            0x00e0ffff,
            0xf0000038,
            0x07c0ffff,
            0x8000ffff,
            0,
            0xff,
        ];
        for (n, (&word, &mask)) in words.iter().zip(&masks).enumerate() {
            if word & !mask != 0 {
                return Err(Error(format!(
                    "unsupported TIC word {n} bits {:#x}",
                    word & !mask
                )));
            }
        }
        if words[0] & !0x7f != 0x58d24900 {
            return Err(invalid("unsupported TIC channel types or swizzle"));
        }
        if words[2] >> 21 & 7 != 3 || words[4] >> 23 & 15 != 1 || words[5] >> 31 != 1 {
            return Err(invalid("requires normalized block-linear 2D TIC"));
        }
        let srgb = words[4] & (1 << 22) != 0;
        let format = match (words[0] & 0x7f, srgb) {
            (0x08, false) => Format::Rgba8Unorm,
            (0x08, true) => Format::Rgba8Srgb,
            (0x24, false) => Format::Bc1Rgba,
            (0x24, true) => Format::Bc1RgbaSrgb,
            (0x40, false) => Format::Astc4x4,
            (0x40, true) => Format::Astc4x4Srgb,
            _ => return Err(invalid("unsupported TIC format")),
        };
        let max_mip = words[3] >> 28;
        if words[7] & 15 != 0 || words[7] >> 4 & 15 != max_mip {
            return Err(invalid("TIC mip subviews unsupported"));
        }
        let image = Self {
            address: GpuAddress(u64::from(words[1]) | (u64::from(words[2] & 0xffff) << 32)),
            width: (words[4] & 0xffff) + 1,
            height: (words[5] & 0xffff) + 1,
            mip_count: max_mip + 1,
            format,
            layout: TextureLayout::BlockLinear {
                block_height_log2: ((words[3] >> 3) & 7) as u8,
            },
        };
        image
            .with_sampler(TextureSampler {
                filter: Filter::Nearest,
            })
            .validate()?;
        Ok(image)
    }
    pub fn with_sampler(self, sampler: TextureSampler) -> TextureDesc {
        TextureDesc {
            address: self.address,
            width: self.width,
            height: self.height,
            mip_count: self.mip_count,
            format: self.format,
            layout: self.layout,
            filter: sampler.filter,
        }
    }
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TextureSampler {
    pub filter: Filter,
}
impl TextureSampler {
    pub fn decode(words: [u32; 8]) -> Result<Self> {
        if words[0] != 0x92 || words[1] & !0xf7 != 0 || words[2..].iter().any(|&v| v != 0) {
            return Err(invalid(
                "unsupported TSC wrap, compare, LOD, bias, anisotropy or border fields",
            ));
        }
        let mag = words[1] & 7;
        let min = (words[1] >> 4) & 3;
        let mip = (words[1] >> 6) & 3;
        if mag != min || !matches!(mip, 1 | 2) {
            return Err(invalid("unsupported TSC filter combination"));
        }
        let filter = match mag {
            1 => Filter::Nearest,
            2 => Filter::Linear,
            _ => return Err(invalid("unsupported TSC filter")),
        };
        Ok(Self { filter })
    }
}

/// State owned by one ordered GPU channel. FIFO packet continuation and engine
/// registers survive chunks; faults require explicit channel replacement.
#[derive(Debug, Default)]
pub struct Channel {
    pub fifo: crate::fifo::Decoder,
    pub engine: Maxwell3d,
    pub(crate) faulted: bool,
}
impl Channel {
    pub(crate) fn resume_macro(&mut self) -> Result<Option<MethodEffect>> {
        if self.faulted {
            return Err(invalid("GPU channel fault is latched"));
        }
        let result = self
            .engine
            .resume_macro()
            .map_err(|e| crate::Error(e.to_string()));
        if result.is_err() {
            self.faulted = true;
        }
        result
    }
    /// Shared FIFO/engine boundary; consumes one LE-decoded word and latches errors.
    /// A returned draw effect still requires backend execution by the caller.
    pub fn push_word(&mut self, word: u32) -> Result<MethodEffect> {
        if self.faulted {
            return Err(invalid("GPU channel fault is latched"));
        }
        let result = (|| match self
            .fifo
            .push_word(word)
            .map_err(|e| crate::Error(e.to_string()))?
        {
            Some(method) => self
                .engine
                .apply(method)
                .map_err(|e| crate::Error(e.to_string())),
            None => Ok(MethodEffect::State),
        })();
        if result.is_err() {
            self.faulted = true;
        }
        result
    }
    /// Finalize the actual stream, latching a truncated packet as a channel fault.
    pub fn finish(&mut self) -> Result<()> {
        if self.faulted {
            return Err(invalid("GPU channel fault is latched"));
        }
        self.fifo.finish().map_err(|e| {
            self.faulted = true;
            crate::Error(e.to_string())
        })
    }
    pub fn is_faulted(&self) -> bool {
        self.faulted
    }
}
