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

/// Configuration only. Compute dispatch and shader execution remain unsupported.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct ComputeDescriptorPool {
    pub upper: Option<u8>,
    pub lower: Option<u32>,
    pub maximum_index: Option<u32>,
}

/// Retained B1C0 local-memory size and SM-count fields, not allocated host memory.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct ComputeLocalMemorySize {
    pub upper: Option<u8>,
    pub lower: Option<u32>,
    pub max_sm_count: Option<u16>,
}

/// B197 scratch selectors only. Supported shader headers/instructions do not
/// admit local-memory accesses, so these fields never authorize a guest fetch.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct GraphicsLocalMemory {
    pub address_upper: Option<u8>,
    pub address_lower: Option<u32>,
    pub size_upper: Option<u8>,
    pub size_lower: Option<u32>,
    pub default_size_per_warp: Option<u32>,
}

/// Configuration only. Compute dispatch and shader execution remain unsupported.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct ComputeConfiguration {
    pub shader_exceptions: Option<bool>,
    pub bindless_constant_slot: Option<u8>,
    pub local_memory_window: Option<u32>,
    pub shared_memory_window: Option<u32>,
    pub program_upper: Option<u8>,
    pub program_lower: Option<u32>,
    pub spa_version: Option<u16>,
    pub sampler_pool: ComputeDescriptorPool,
    pub header_pool: ComputeDescriptorPool,
    pub local_memory_upper: Option<u8>,
    pub local_memory_lower: Option<u32>,
    pub non_throttled: ComputeLocalMemorySize,
    pub throttled: ComputeLocalMemorySize,
}

/// Configuration whose observations are restricted by the admitted shader and
/// topology subset. This does not provide statistics reports or line rendering.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct AuxiliaryRasterControls {
    pub zcull_statistics: Option<bool>,
    /// Enable state only; ZPASS report/reset methods remain unsupported.
    pub zpass_pixel_count: Option<bool>,
    /// Admitted VTG headers/AST outputs export no user clip distances.
    pub user_clip_enable: Option<u8>,
    /// Inert while the required raster sample-mask controls remain disabled.
    pub sample_masks: [Option<u16>; 4],
    /// Point controls are retained; supported primitive assembly is triangles.
    pub point_sprite: Option<bool>,
    pub point_sprite_select: Option<u16>,
    pub point_size_attribute: Option<u16>,
    pub point_size_bits: Option<u32>,
    /// Front ref/read/write mask, then back ref/write/read mask; stencil stays disabled.
    pub stencil_refs_masks: [Option<u8>; 6],
    pub conservative_raster: Option<bool>,
    pub raster_bounding_box: Option<u8>,
    pub tir_mode: Option<u8>,
    pub tir_modulation: Option<u8>,
    pub coverage_to_color: Option<bool>,
    /// Iterated blending (color, alpha); only disabled controls execute.
    pub iterated_blend: Option<(bool, bool)>,
    /// Eight-bit pass count, inert while iterated blending is disabled.
    pub iterated_blend_pass: Option<u8>,
    /// Raw RGBA fixed-blend constants. Constant blend factors remain unsupported.
    pub blend_constant_bits: [Option<u32>; 4],
    /// Subtile resource allocation hints, not a host scheduling implementation.
    pub subtile_performance: [Option<u32>; 2],
    /// Supported disabled pixel controls; active variants require new semantics.
    pub api_mandated_early_z: Option<bool>,
    pub post_z_pixel_sample_mask: Option<bool>,
    pub constant_color_rendering: Option<bool>,
    pub hybrid_anti_alias_control: Option<u8>,
    pub pixel_shader_interlock: Option<u8>,
    pub offset_render_target_index: Option<u32>,
    /// Disabled vertex-cull, tessellation-init, tessellation and geometry TYPE.
    pub disabled_pipeline_types: [Option<u8>; 4],
    pub alpha_coverage_qualifier: Option<u8>,
    /// Disabled tile cache and exact public-driver baseline tuning words only.
    pub tile_cache_profile: [Option<u32>; 7],
    pub zero_bandwidth_clear_disable_masks: [Option<u16>; 2],
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
    /// Vertex edge marking, inert under the strictly supported polygon FILL mode.
    pub edge_flag: Option<bool>,
    pub two_sided_stencil: Option<bool>,
    pub spa_version: Option<u16>,
    pub local_memory_window: Option<u32>,
    pub local_memory: GraphicsLocalMemory,
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

/// Explicit B197 shadow mode; no unspecified reset mode is assumed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ShadowMode {
    Track,
    TrackWithFilter,
    Passthrough,
    Replay,
}
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub enum ShadowValue {
    #[default]
    Unwritten,
    Known(u32),
    /// Retained diagnostic classification for unverified filtered extensions.
    /// The supported pure-state capture subset records validated known words.
    FilteredUnverified,
}
#[derive(Debug, Default)]
struct ShadowRam {
    mode: Option<ShadowMode>,
    words: Vec<ShadowValue>,
}
impl ShadowRam {
    // Bounded, side-effect-free raster register subset. Attachment addresses,
    // resource selectors, actions, scratch and unknown methods are not replayed.
    fn eligible(method: u16) -> bool {
        method >= 0x200
            && method % 4 == 0
            && (crate::raster::FIXED_RASTER_STATE
                .iter()
                .any(|v| v.0 == method)
                || match method {
                    0xa00..=0xbfc => (method - 0xa00) % 32 < 28,
                    0xc00..=0xcfc => true,
                    0xe00..=0xef8 => (method - 0xe00) % 16 < 12,
                    0x1e00..=0x1efc => (method - 0x1e00) % 32 < 28,
                    0x1364..=0x137c | 0x1a04..=0x1a1c => true,
                    0x131c..=0x1328 => true,
                    0x156c | 0x15bc | 0x187c | 0x0360 | 0x0364 => true,
                    0x0210 | 0x0f1c | 0x0f40 | 0x1224 | 0x11f0 | 0x0754 => true,
                    _ => matches!(
                        method,
                        0x02ec
                            | 0x0fb4
                            | 0x0fd8
                            | 0x0dd0
                            | 0x0dd4
                            | 0x1148
                            | 0x11f8
                            | 0xd7c
                            | 0x12cc
                            | 0x12e4
                            | 0x12e8
                            | 0x130c
                            | 0x133c
                            | 0x1340
                            | 0x1344
                            | 0x1348
                            | 0x134c
                            | 0x1350
                            | 0x1358
                            | 0x1360
                            | 0x193c
                            | 0x1a00
                    ),
                })
    }
    fn value(&self, method: u16) -> ShadowValue {
        self.words
            .get(usize::from(method / 4))
            .copied()
            .unwrap_or_default()
    }
    fn prepare(&self, write: &mut MethodWrite) -> std::result::Result<(), MethodError> {
        if self.mode != Some(ShadowMode::Replay) || write.method < 0x200 || write.method >= 0x3800 {
            return Ok(());
        }
        if !Self::eligible(write.method) {
            return Err(MethodError::UnsupportedShadowReplayMethod(write.method));
        }
        write.value = match self.value(write.method) {
            ShadowValue::Known(value) => value,
            ShadowValue::Unwritten => return Err(MethodError::MissingShadowValue(write.method)),
            ShadowValue::FilteredUnverified => {
                return Err(MethodError::FilteredShadowValue(write.method));
            }
        };
        Ok(())
    }
    fn capture(&mut self, write: MethodWrite) {
        if !Self::eligible(write.method) {
            return;
        }
        let value = match self.mode {
            // Source-pinned DeviceStateWithShadow captures ordinary validated
            // state in both track modes. Limit this behavior to eligible pure
            // state above; actions/resources and unspecified initial contents
            // do not become replayable by selecting the filtered mode.
            Some(ShadowMode::Track | ShadowMode::TrackWithFilter) => {
                ShadowValue::Known(write.value)
            }
            _ => return,
        };
        if self.words.is_empty() {
            // One bounded allocation on first successful capture, not per draw.
            self.words.resize(0x800, ShadowValue::Unwritten);
        }
        self.words[usize::from(write.method / 4)] = value;
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
    IncompleteInlineUpload,
    IncompleteReport,
    InlineUploadActive,
    UnsupportedShadowReplayMethod(u16),
    MissingShadowValue(u16),
    FilteredShadowValue(u16),
    IncompletePrimitiveRestart,
}
impl std::fmt::Display for MethodError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // Preserve the established diagnostic prefix for existing consumers,
        // adding byte-address context without changing the typed failure.
        write!(f, "Maxwell method fault: {self:?}")?;
        match self {
            Self::UnsupportedMethod(method) => {
                write!(f, " [B197 method {method:#06x}]")
            }
            Self::UnsupportedValue { method, value } => {
                write!(f, " [B197 method {method:#06x}, value {value:#010x}]")
            }
            _ => Ok(()),
        }
    }
}
impl std::error::Error for MethodError {}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MethodEffect {
    State,
    TextureHeadersInvalidated(Option<u32>),
    SamplersInvalidated(Option<u32>),
    Draw {
        first: u32,
        count: u32,
    },
    DrawIndices {
        first: u32,
        count: u32,
    },
    Clear {
        color: bool,
        depth: bool,
    },
    ConstantWrite {
        address: GpuAddress,
        value: u32,
    },
    InlineUploadStart {
        address: GpuAddress,
        length: u32,
    },
    InlineWrite {
        address: GpuAddress,
        value: u32,
    },
    WaitIdle,
    /// B06F release report. WFI-enabled reports observe prior GPU completion;
    /// WFI-disabled reports do not promise it. Not a Host1x syncpoint.
    SemaphoreRelease {
        address: GpuAddress,
        payload: u32,
        long: bool,
        wait: bool,
    },
    /// Only an already-satisfied equality acquire is currently executable.
    SemaphoreAcquire {
        address: GpuAddress,
        payload: u32,
    },
    /// Conservative guest visibility barrier, not a Switch fence.
    HostCacheMaintenance,
    ShaderCachesInvalidated(u32),
    TextureDataInvalidated,
    /// Whole texture-data cache invalidation after real host queue completion.
    TextureDataInvalidatedWaitIdle,
}
/// Bounded pitch-linear, one-row B197 inline upload. Configuration remains
/// separate from the active transfer snapshot; no guest memory is retained.
#[derive(Debug, Default)]
struct InlineUpload {
    length: Option<u32>,
    lines: Option<u32>,
    upper: Option<u8>,
    lower: Option<u32>,
    active: Option<(GpuAddress, u32)>,
}
#[derive(Debug, Default)]
struct ReportSelector {
    upper: Option<u8>,
    lower: Option<u32>,
    payload: Option<u32>,
}
impl ReportSelector {
    fn apply(
        &mut self,
        method: u16,
        value: u32,
        graphics: bool,
    ) -> std::result::Result<MethodEffect, MethodError> {
        let bad = || MethodError::UnsupportedValue { method, value };
        match method {
            0x1b00 => {
                if value > 255 {
                    return Err(bad());
                }
                self.upper = Some(value as u8);
                Ok(MethodEffect::State)
            }
            0x1b04 => {
                self.lower = Some(value);
                Ok(MethodEffect::State)
            }
            0x1b08 => {
                self.payload = Some(value);
                Ok(MethodEffect::State)
            }
            0x1b0c => {
                // Plain payload release (one word), or B197 payload/timestamp
                // REPORT_ONLY (four words). No counters, traps, reductions,
                // awaken events, acquire, or per-stage pipeline observations.
                let mask = if graphics { 0x1000f010 } else { 0x10000000 };
                let long = graphics && value & 3 == 2;
                let allowed = if long { mask | 2 } else { mask };
                if value & !allowed != 0
                    || (graphics && !matches!((value >> 12) & 15, 0 | 15))
                    || (!long && value & 0x10000000 == 0)
                    || (long && value & 0x10000000 != 0)
                {
                    return Err(bad());
                }
                let (Some(upper), Some(lower), Some(payload)) =
                    (self.upper, self.lower, self.payload)
                else {
                    return Err(MethodError::IncompleteReport);
                };
                if lower % 4 != 0 {
                    return Err(bad());
                }
                Ok(MethodEffect::SemaphoreRelease {
                    address: GpuAddress((u64::from(upper) << 32) | u64::from(lower)),
                    payload,
                    long,
                    wait: true,
                })
            }
            _ => unreachable!("report method range is checked by the engine dispatcher"),
        }
    }
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
    Float16x4,
}
impl PositionFormat {
    pub fn byte_size(self) -> u16 {
        match self {
            Self::Float16x4 => 8,
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
    /// Inactive UV inputs may be omitted only if the shader never reads them.
    pub uv_active: bool,
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
    semaphore_upper: Option<u8>,
    semaphore_lower: Option<u32>,
    semaphore_payload: Option<u32>,
    graphics_report: ReportSelector,
    compute_report: ReportSelector,
    raster: crate::raster::RasterRegisters,
    indices: crate::index::IndexRegisters,
    bound: [Option<EngineClass>; 8],
    macros: crate::mme::MacroEngine,
    shadow: ShadowRam,
    macro_work: crate::mme::WorkBudget,
    active_macro: Option<ActiveMacro>,
    cache_policies: [Option<CacheEviction>; 6],
    two_d: TwoDConfiguration,
    compute: ComputeConfiguration,
    auxiliary_raster: AuxiliaryRasterControls,
    inline_upload: InlineUpload,
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
    inactive_vertex_addresses: [[Option<u32>; 2]; 15],
    inactive_vertex_limits: [[Option<u32>; 2]; 15],
    vertex_substitute_upper: Option<u8>,
    vertex_substitute_lower: Option<u32>,
    vertex_frequency: Option<u32>,
    vertex_instance: [Option<u32>; 16],
    vertex_attributes: [Option<u32>; 2],
    inactive_vertex_attributes: [Option<u32>; 30],
    inactive_vertex_formats: [Option<u16>; 15],
    inactive_pipeline_programs: [Option<u32>; 4],
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
    /// Deliberate initialization by the supported software driver, not a claim
    /// about unobserved hardware reset values. Raw channels remain strict.
    #[cfg(feature = "wgpu-backend")]
    pub(crate) fn initialize_driver_context(&mut self) -> Result<()> {
        // The driver starts without a global predicate. Base layer zero is
        // independently listed by the pinned B197 context initialization table.
        for (method, value) in [
            (0x0138, 1),
            (0x0820, 0),
            // Source-verified B197 context initialization, not wishful draw defaults.
            (0x037c, 1),
            (0x0a18, 0x6420),
            (0x0df8, 0),
            (0x0dfc, 0),
            (0x0fa4, 1),
            (0x12e8, 0),
            (0x1360, 0),
            (0x135c, 0),
            (0x1924, 0),
            (0x1a00, 0x1111),
            (0x19c4, 0),
            (0x19bc, 0),
            (0x0db4, 0),
            (0x168c, 0),
        ] {
            self.raster
                .apply(method, value)
                .map_err(|error| invalid(&format!("driver context initialization: {error:?}")))?;
            self.macros.observe_color_target(method, value);
        }
        // The pinned B197 context method table initializes these to 0 and 1.
        self.auxiliary_raster.iterated_blend = Some((false, false));
        self.auxiliary_raster.iterated_blend_pass = Some(1);
        self.auxiliary_raster.blend_constant_bits = [Some(0); 4];
        self.auxiliary_raster.sample_masks = [Some(0xffff); 4];
        // Same pinned context table: vertex-B/pixel constant groups are 0/4;
        // index base, base instance and restart are zero. Program addresses,
        // register allocations and buffer bindings still come from guest methods.
        self.vertex_group = Some(0);
        self.fragment_group = Some(4);
        self.vertex_frequency = Some(0);
        self.vertex_instance[0] = Some(0);
        for method in [0x1434, 0x1438, 0x1644] {
            self.indices
                .apply(method, 0)
                .map_err(|e| invalid(&format!("driver index initialization: {e}")))?;
        }
        Ok(())
    }
    /// Configured A-stream address. Only stream zero can supply vertex fetches
    /// in the supported attribute subset; inactive addresses are not read.
    pub fn vertex_stream_format(&self, slot: usize) -> Option<u16> {
        if slot == 0 {
            self.vertex_format.map(|v| v as u16)
        } else {
            self.inactive_vertex_formats
                .get(slot - 1)
                .copied()
                .flatten()
        }
    }
    pub fn vertex_attribute(&self, slot: usize) -> Option<u32> {
        if slot < 2 {
            self.vertex_attributes[slot]
        } else {
            self.inactive_vertex_attributes
                .get(slot - 2)
                .copied()
                .flatten()
        }
    }
    pub fn vertex_stream_address(&self, slot: usize) -> Option<GpuAddress> {
        let (hi, lo) = if slot == 0 {
            (self.vertex_upper?, self.vertex_lower?)
        } else {
            let address = self.inactive_vertex_addresses.get(slot.checked_sub(1)?)?;
            (address[0]?, address[1]?)
        };
        Some(GpuAddress((u64::from(hi) << 32) | u64::from(lo)))
    }
    /// Inclusive bound; inactive stream bounds do not authorize extra fetches.
    pub fn vertex_stream_limit(&self, slot: usize) -> Option<GpuAddress> {
        let (hi, lo) = if slot == 0 {
            (self.vertex_limit_upper?, self.vertex_limit_lower?)
        } else {
            let limit = self.inactive_vertex_limits.get(slot.checked_sub(1)?)?;
            (limit[0]?, limit[1]?)
        };
        Some(GpuAddress((u64::from(hi) << 32) | u64::from(lo)))
    }
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
    pub fn compute_configuration(&self) -> ComputeConfiguration {
        self.compute
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
    pub fn shadow_mode(&self) -> Option<ShadowMode> {
        self.shadow.mode
    }
    /// Known or explicitly unverified capture for the bounded raster subset.
    /// This is not a guest-memory address or a general MME register READ API.
    pub fn shadow_value(&self, method: u16) -> Option<ShadowValue> {
        ShadowRam::eligible(method).then(|| self.shadow.value(method))
    }
    pub fn apply(
        &mut self,
        mut write: MethodWrite,
    ) -> std::result::Result<MethodEffect, MethodError> {
        let routed = self.active_macro.is_none()
            && write.method % 4 == 0
            && self.bound_class(write.subchannel) == Some(EngineClass::ThreeD);
        if routed {
            self.shadow.prepare(&mut write)?;
        }
        let effect = self.apply_inner(write)?;
        if effect == MethodEffect::State
            && self.bound_class(write.subchannel) == Some(EngineClass::ThreeD)
        {
            self.macros.observe_color_target(write.method, write.value);
        }
        if routed && effect == MethodEffect::State {
            self.shadow.capture(write);
        }
        Ok(effect)
    }
    fn apply_inner(
        &mut self,
        write: MethodWrite,
    ) -> std::result::Result<MethodEffect, MethodError> {
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
        // NVIDIA clb06f.h (9fdf5c4): these are channel methods, regardless
        // of the selected engine/subchannel. Selectors have no memory effect.
        match method {
            0x10 => {
                if value > 255 {
                    return Err(bad());
                }
                self.semaphore_upper = Some(value as u8);
                return Ok(MethodEffect::State);
            }
            0x14 => {
                if value & 3 != 0 {
                    return Err(bad());
                }
                self.semaphore_lower = Some(value);
                return Ok(MethodEffect::State);
            }
            0x18 => {
                self.semaphore_payload = Some(value);
                return Ok(MethodEffect::State);
            }
            0x1c => {
                // Exact EQ acquire or RELEASE, with independently decoded
                // report size and WFI ordering. Reductions/acquire-switch and
                // other acquire conditions remain explicit unsupported forms.
                if !matches!(value, 1 | 2 | 0x0010_0002 | 0x0100_0002 | 0x0110_0002) {
                    return Err(bad());
                }
                let (Some(hi), Some(lo), Some(payload)) = (
                    self.semaphore_upper,
                    self.semaphore_lower,
                    self.semaphore_payload,
                ) else {
                    return Err(bad());
                };
                let address = GpuAddress((u64::from(hi) << 32) | u64::from(lo));
                return Ok(if value == 1 {
                    MethodEffect::SemaphoreAcquire { address, payload }
                } else {
                    MethodEffect::SemaphoreRelease {
                        address,
                        payload,
                        long: value & 0x0100_0000 == 0,
                        wait: value & 0x0010_0000 == 0,
                    }
                });
            }
            _ => {}
        }
        // Switch Maxwell host MEM_OP_B (word 0x0b); only operand-free
        // L2 dirty flush/system invalidation. Other operations remain unsupported.
        if method == 0x2c {
            if !matches!(value, 0x8000_0000 | 0x7000_0000) {
                return Err(bad());
            }
            return Ok(MethodEffect::HostCacheMaintenance);
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
            if class == EngineClass::Compute {
                if (0x1b00..=0x1b0c).contains(&method) {
                    return self.compute_report.apply(method, value, false);
                }
                match method {
                    0x100 => {}
                    0x1528 if value <= 1 => self.compute.shader_exceptions = Some(value != 0),
                    0x2608 if value <= 7 => self.compute.bindless_constant_slot = Some(value as u8),
                    0x077c => self.compute.local_memory_window = Some(value),
                    0x0214 => self.compute.shared_memory_window = Some(value),
                    0x1608 if value <= 255 => self.compute.program_upper = Some(value as u8),
                    0x160c => self.compute.program_lower = Some(value),
                    0x155c if value <= 255 => self.compute.sampler_pool.upper = Some(value as u8),
                    0x1560 => self.compute.sampler_pool.lower = Some(value),
                    0x1564 if value <= 0xfffff => {
                        self.compute.sampler_pool.maximum_index = Some(value)
                    }
                    0x1574 if value <= 255 => self.compute.header_pool.upper = Some(value as u8),
                    0x1578 => self.compute.header_pool.lower = Some(value),
                    0x157c if value <= 0x3fffff => {
                        self.compute.header_pool.maximum_index = Some(value)
                    }
                    0x0790 if value <= 255 => self.compute.local_memory_upper = Some(value as u8),
                    0x0794 => self.compute.local_memory_lower = Some(value),
                    0x02e4 if value <= 255 => self.compute.non_throttled.upper = Some(value as u8),
                    0x02e8 => self.compute.non_throttled.lower = Some(value),
                    0x02ec if value <= 511 => {
                        self.compute.non_throttled.max_sm_count = Some(value as u16)
                    }
                    0x02f0 if value <= 255 => self.compute.throttled.upper = Some(value as u8),
                    0x02f4 => self.compute.throttled.lower = Some(value),
                    0x02f8 if value <= 511 => {
                        self.compute.throttled.max_sm_count = Some(value as u16)
                    }
                    0x0310 if value == 0x503 => self.compute.spa_version = Some(value as u16),
                    0x1528 | 0x2608 | 0x1608 | 0x0310 | 0x155c | 0x1564 | 0x1574 | 0x157c
                    | 0x0790 | 0x02e4 | 0x02ec | 0x02f0 | 0x02f8 => return Err(bad()),
                    _ => {
                        return Err(MethodError::UnsupportedEngineMethod {
                            class: class as u32,
                            method,
                        });
                    }
                }
                return Ok(MethodEffect::State);
            }
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
            0x1b00..=0x1b0c => return self.graphics_report.apply(method, value, true),
            0x07a4 | 0x07a8 => {
                // All 15 hardware ZBC slots disabled. Compressed ZBC surfaces
                // and enabling an individual slot remain unsupported.
                if value != 0x7fff {
                    return Err(bad());
                }
                self.auxiliary_raster.zero_bandwidth_clear_disable_masks
                    [usize::from(method == 0x7a8)] = Some(value as u16);
            }
            0x0210 | 0x0f1c | 0x0f40 | 0x1224 | 0x11f0 => {
                if value != 0 {
                    return Err(bad());
                }
                match method {
                    0x0210 => self.auxiliary_raster.api_mandated_early_z = Some(false),
                    0x0f1c => self.auxiliary_raster.post_z_pixel_sample_mask = Some(false),
                    0x0f40 => self.auxiliary_raster.constant_color_rendering = Some(false),
                    0x1224 => self.auxiliary_raster.pixel_shader_interlock = Some(0),
                    _ => self.auxiliary_raster.offset_render_target_index = Some(0),
                }
            }
            0x0754 => {
                // One hybrid-AA pass, centroid per fragment, no extended passes.
                if value != 1 {
                    return Err(bad());
                }
                self.auxiliary_raster.hybrid_anti_alias_control = Some(1);
            }
            0x2000 | 0x2080 | 0x20c0 | 0x2100 => {
                let stage = ((method - 0x2000) / 64) as u8;
                if value != u32::from(stage) << 4 {
                    return Err(bad());
                }
                let index = if stage == 0 {
                    0
                } else {
                    usize::from(stage - 1)
                };
                self.auxiliary_raster.disabled_pipeline_types[index] = Some(stage);
            }
            0x0360 => self.auxiliary_raster.subtile_performance[0] = Some(value),
            0x0364 => {
                if value > 255 {
                    return Err(bad());
                }
                self.auxiliary_raster.subtile_performance[1] = Some(value);
            }
            0x0de0 => {
                // SYSMEMBAR_ENABLE=false only. A full host queue wait is a
                // conservative implementation of the fragment barrier; it does
                // not imply guest RAM publication or a hardware-equivalent stall.
                if value != 0 {
                    return Err(bad());
                }
                return Ok(MethodEffect::WaitIdle);
            }
            0x0f74 => {
                // deko3d's all-lines invalidate with implicit WFI. Per-tag
                // invalidation is separately unsupported, not silently widened.
                if value != 0 {
                    return Err(bad());
                }
                return Ok(MethodEffect::TextureDataInvalidatedWaitIdle);
            }
            0x1288 => {
                // Whole-cache invalidation only; per-tag forms are unsupported.
                if value != 0 {
                    return Err(bad());
                }
                return Ok(MethodEffect::TextureDataInvalidated);
            }
            0x0da4 => {
                if value & !0x1011 != 0 {
                    return Err(bad());
                }
                return Ok(MethodEffect::ShaderCachesInvalidated(value));
            }
            0x0124 => {
                self.shadow.mode = Some(match value {
                    0 => ShadowMode::Track,
                    1 => ShadowMode::TrackWithFilter,
                    2 => ShadowMode::Passthrough,
                    3 => ShadowMode::Replay,
                    _ => return Err(bad()),
                });
            }
            0x0180..=0x018c => {
                if self.inline_upload.active.is_some() {
                    return Err(MethodError::InlineUploadActive);
                }
                match method {
                    // Supported budget, not a hardware field-width limit.
                    0x0180 if value <= 65536 && value % 4 == 0 => {
                        self.inline_upload.length = Some(value);
                    }
                    0x0184 if value == 1 => self.inline_upload.lines = Some(value),
                    0x0188 if value <= 0xff => self.inline_upload.upper = Some(value as u8),
                    0x018c => self.inline_upload.lower = Some(value),
                    _ => return Err(bad()),
                }
            }
            0x01b0 => {
                if self.inline_upload.active.is_some() {
                    return Err(MethodError::InlineUploadActive);
                }
                // PITCH, FLUSH_ONLY, no interrupt/reduction, one-word semaphore
                // size (unused without RELEASE_SEMAPHORE). No guessed flags.
                if value != 0x1011 {
                    return Err(bad());
                }
                let (Some(length), Some(1), Some(upper), Some(lower)) = (
                    self.inline_upload.length,
                    self.inline_upload.lines,
                    self.inline_upload.upper,
                    self.inline_upload.lower,
                ) else {
                    return Err(MethodError::IncompleteInlineUpload);
                };
                let address = (u64::from(upper) << 32) | u64::from(lower);
                if address % 4 != 0
                    || address
                        .checked_add(u64::from(length))
                        .is_none_or(|end| end > 1 << 40)
                {
                    return Err(bad());
                }
                self.inline_upload.active = (length != 0).then_some((GpuAddress(address), length));
                return Ok(MethodEffect::InlineUploadStart {
                    address: GpuAddress(address),
                    length,
                });
            }
            0x01b4 => {
                let Some((address, remaining)) = self.inline_upload.active else {
                    return Err(MethodError::IncompleteInlineUpload);
                };
                self.inline_upload.active = if remaining == 4 {
                    None
                } else {
                    Some((GpuAddress(address.0 + 4), remaining - 4))
                };
                return Ok(MethodEffect::InlineWrite { address, value });
            }
            // deko3d 373bfd4 engine_3d.def uses word offsets 0x3df/0x3e0.
            // Only its operand-zero tile barrier/flush forms are established.
            // Publish conservatively through the existing coherence boundary;
            // this does not emulate tile scheduling or the unknown flush-alt.
            0x0f7c | 0x0f80 => {
                if value != 0 {
                    return Err(bad());
                }
                return Ok(MethodEffect::HostCacheMaintenance);
            }
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
                // not this selector. Bound TEXS resolves its handle through this slot.
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
            0x131c..=0x1328 => {
                // Preserve the wire bits without clamping, specializing shaders
                // or using them with an unsupported constant blend factor.
                self.auxiliary_raster.blend_constant_bits[usize::from((method - 0x131c) / 4)] =
                    Some(value);
            }
            0x0dd0 => {
                // Color/alpha enables occupy bits 0/1. Active requests remain
                // unsupported, rather than being silently disabled.
                if value != 0 {
                    return Err(bad());
                }
                self.auxiliary_raster.iterated_blend = Some((false, false));
            }
            0x0dd4 => {
                if value > 0xff {
                    return Err(bad());
                }
                self.auxiliary_raster.iterated_blend_pass = Some(value as u8);
            }
            0x0fbc..=0x0fc8 => {
                if value > 0xffff {
                    return Err(bad());
                }
                self.auxiliary_raster.sample_masks[usize::from((method - 0xfbc) / 4)] =
                    Some(value as u16);
            }
            0x16b4 => {
                if value > 3 {
                    return Err(bad());
                }
                self.auxiliary_raster.alpha_coverage_qualifier = Some(value as u8);
            }
            0xf60 | 0xf64 | 0xf68 | 0xf6c | 0x1108 | 0xf70 | 0x1134 => {
                let (slot, expected) = match method {
                    0xf60 => (0, 0),
                    0xf64 => (1, 0x00800080),
                    0xf68 => (2, 0x1109),
                    0xf6c => (3, 0x08080202),
                    0x1108 => (4, 0x1f),
                    0xf70 => (5, 0x00080001),
                    _ => (6, 0),
                };
                if value != expected {
                    return Err(bad());
                }
                self.auxiliary_raster.tile_cache_profile[slot] = Some(value);
            }
            0x1148 | 0x02ec | 0xfb4 | 0xfd8 | 0x11f8 => {
                // Ordinary filled bounding-box rasterization with TIR and
                // coverage-to-color disabled. Enabled forms are not implemented.
                if value != 0 {
                    return Err(bad());
                }
                match method {
                    0x1148 => self.auxiliary_raster.conservative_raster = Some(false),
                    0x02ec => self.auxiliary_raster.raster_bounding_box = Some(0),
                    0xfb4 => self.auxiliary_raster.tir_mode = Some(0),
                    0xfd8 => self.auxiliary_raster.tir_modulation = Some(0),
                    _ => self.auxiliary_raster.coverage_to_color = Some(false),
                }
            }
            0x1394 | 0x1398 | 0x139c | 0xf54 | 0xf58 | 0xf5c => {
                if value > 255 {
                    return Err(bad());
                }
                let slot = match method {
                    0x1394 => 0,
                    0x1398 => 1,
                    0x139c => 2,
                    0xf54 => 3,
                    0xf58 => 4,
                    _ => 5,
                };
                self.auxiliary_raster.stencil_refs_masks[slot] = Some(value as u8);
            }
            0x1520 => {
                if value > 1 {
                    return Err(bad());
                }
                self.auxiliary_raster.point_sprite = Some(value != 0);
            }
            0x1604 => {
                if value & !0x1fff != 0 || value & 3 == 3 {
                    return Err(bad());
                }
                self.auxiliary_raster.point_sprite_select = Some(value as u16);
            }
            0x1910 => {
                if value & !0xff1 != 0 {
                    return Err(bad());
                }
                self.auxiliary_raster.point_size_attribute = Some(value as u16);
            }
            0x1518 => {
                let size = f32::from_bits(value);
                if !size.is_finite() || size <= 0. {
                    return Err(bad());
                }
                self.auxiliary_raster.point_size_bits = Some(value);
            }
            0x1510 => {
                if value > 255 {
                    return Err(bad());
                }
                // Current exact VTG SPH and AST subset exports no clip-distance
                // values. Enabling unused planes cannot add a clipping output.
                self.auxiliary_raster.user_clip_enable = Some(value as u8);
            }
            0x1514 => {
                if value > 1 {
                    return Err(bad());
                }
                self.auxiliary_raster.zpass_pixel_count = Some(value != 0);
            }
            0x15e4 => {
                if value > 1 {
                    return Err(bad());
                }
                // Edge marking affects polygon POINT/LINE rasterization. Both
                // polygon faces remain strictly FILL in the supported draw path.
                self.auxiliary_raster.edge_flag = Some(value != 0);
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
            0x0790 => {
                if value > 255 {
                    return Err(bad());
                }
                self.auxiliary_raster.local_memory.address_upper = Some(value as u8);
            }
            0x0794 => self.auxiliary_raster.local_memory.address_lower = Some(value),
            0x0798 => {
                if value > 63 {
                    return Err(bad());
                }
                self.auxiliary_raster.local_memory.size_upper = Some(value as u8);
            }
            0x079c => self.auxiliary_raster.local_memory.size_lower = Some(value),
            0x07a0 => {
                if value > 0x03ff_ffff {
                    return Err(bad());
                }
                self.auxiliary_raster.local_memory.default_size_per_warp = Some(value);
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
            0x2004 | 0x2084 | 0x20c4 | 0x2104 => {
                let stage = ((method - 0x2004) / 64) as usize;
                self.inactive_pipeline_programs[if stage == 0 { 0 } else { stage - 1 }] =
                    Some(value);
            }
            0x1c10..=0x1cf0 if method % 16 == 0 => {
                if value & !0x1fff != 0 {
                    return Err(bad());
                }
                self.inactive_vertex_formats[usize::from((method - 0x1c10) / 16)] =
                    Some(value as u16);
            }
            0x1168..=0x11dc => {
                // Exact inactive FLOAT/R32, stream zero, offset zero profile.
                // Active inputs beyond the supported interface remain rejected.
                if value != 0x3a400040 {
                    return Err(bad());
                }
                self.inactive_vertex_attributes[usize::from((method - 0x1168) / 4)] = Some(value);
            }
            0x1c14..=0x1cf8 if matches!(method % 16, 4 | 8) => {
                let slot = usize::from((method - 0x1c00) / 16 - 1);
                let half = usize::from((method % 16 - 4) / 4);
                if half == 0 && value > 255 {
                    return Err(bad());
                }
                self.inactive_vertex_addresses[slot][half] = Some(value);
            }
            0x1f08..=0x1f7c => {
                let slot = usize::from((method - 0x1f00) / 8 - 1);
                let half = usize::from((method % 8) / 4);
                if half == 0 && value > 255 {
                    return Err(bad());
                }
                self.inactive_vertex_limits[slot][half] = Some(value);
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
                // Frequency is inert in the supported non-instanced stream.
                // Both documented initialized zero and existing one are admitted.
                if value > 1 {
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
                    matches!(fields, 0x38200000 | 0x38400000 | 0x38600000)
                } else {
                    fields == 0x38800000 || value == 0x3a400040
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
            0x0100 | 0x1a2c => {} // documented NO_OPERATION and PIPE_NOP
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
    /// Retained B197 format selector; zero explicitly disables the target.
    /// A configured inactive format does not authorize another color output.
    pub fn color_target_format(&self, slot: usize) -> Option<u32> {
        self.raster.color_target_format(slot)
    }
    pub fn rasterization(
        &self,
    ) -> Result<(crate::raster::ColorTarget, crate::raster::Rasterization)> {
        if self.raster.sample_mask_enabled()
            && self.auxiliary_raster.sample_masks != [Some(0xffff); 4]
        {
            return Err(invalid(
                "enabled raster sample mask requires all-covered masks",
            ));
        }
        if self.auxiliary_raster.iterated_blend_pass.is_some()
            && self.auxiliary_raster.iterated_blend != Some((false, false))
        {
            return Err(invalid(
                "iterated blend passes require established disabled color and alpha enables",
            ));
        }
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
            Some(0 | 1),
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
        let position_format = match (a0 >> 21) & 0x3f {
            2 => PositionFormat::Float32x3,
            3 => PositionFormat::Float16x4,
            _ => PositionFormat::Float32x4,
        };
        let uv_active = a1 & 0x40 == 0;
        let address = (u64::from(hi) << 32) | u64::from(lo);
        let limit = (u64::from(lhi) << 32) | u64::from(llo);
        if format & 0x1000 == 0
            || stride == 0
            || stride % 4 != 0
            || offsets.iter().any(|v| v % 4 != 0)
            || u32::from(offsets[0]) + u32::from(position_format.byte_size()) > u32::from(stride)
            || uv_active && u32::from(offsets[1]) + 8 > u32::from(stride)
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
            uv_active,
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
            Some(method) => self.engine.apply(method).map_err(|e| {
                crate::Error(format!(
                    "{e}; subchannel={} method={:#06x} value={:#010x}",
                    method.subchannel, method.method, method.value
                ))
            }),
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
        if self.engine.inline_upload.active.is_some() {
            self.faulted = true;
            return Err(invalid("incomplete B197 inline upload at channel finish"));
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

/// Maxwell timer ratio corroborated by MIT Ryujinx GpuContext 4930dced:
/// 614,400,000 ticks/s. Wide intermediates preserve the full u64 ns domain.
pub fn timestamp_ticks(nanoseconds: u64) -> u64 {
    (u128::from(nanoseconds) * 384 / 625) as u64
}
