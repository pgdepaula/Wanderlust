//! # wl-npdm: Authoritative Nintendo Process Descriptor Meta (NPDM) Parser
//!
//! Independent, zero-allocation parser for Nintendo Switch NPDM files (`main.npdm`).
//!
//! Follows Switchbrew specification, libnx runtime conventions, and Atmosphère's
//! `libstratosphere` implementation (`ldr_meta`, `ldr_capabilities`).

pub mod access_control;
pub mod capability;
pub mod error;
pub mod meta;

pub use access_control::{FsAccessControl, SacEntry, SacIterator};
pub use capability::{Capabilities, Capability, CapabilityId, SyscallBitmap};
pub use error::NpdmError;
pub use meta::{
    ACI0_HEADER_SIZE, ACID_HEADER_SIZE, Aci0Header, AcidHeader, MAGIC_ACI0, MAGIC_ACID, MAGIC_META,
    MAX_SYSTEM_RESOURCE_SIZE, META_HEADER_SIZE, MetaHeader, PAGE_SIZE, ProcessAddressSpace,
};
use serde::Serialize;
use wl_common::Result as WlResult;

/// Process metadata summary for configuring Horizon execution environments.
///
/// Designed to map cleanly to `ProcessConfig` in `wl-runtime`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct ProcessMetadata {
    pub program_id: u64,
    pub main_thread_priority: u8,
    pub ideal_core: u8,
    pub main_thread_stack_size: u64,
    pub system_resource_size: u64,
    pub handle_table_size: u16,
    pub core_mask: u64,
    pub priority_mask: u64,
    pub is_application: bool,
    pub address_space: ProcessAddressSpace,
    pub syscall_bitmap: SyscallBitmap,
    pub effective_fs_permissions: u64,
}

/// Parsed, zero-allocation borrowed NPDM file structure.
#[derive(Clone, Debug)]
pub struct Npdm<'a> {
    bytes: &'a [u8],
    meta: MetaHeader<'a>,
    acid: AcidHeader,
    aci0: Aci0Header,
}

impl<'a> Npdm<'a> {
    /// Parses and validates an NPDM image from a raw byte buffer without heap allocation.
    ///
    /// Checks magic headers, bounds, arithmetic overflows, alignments, section overlaps,
    /// and process field constraints.
    pub fn parse(bytes: &'a [u8]) -> Result<Self, NpdmError> {
        if bytes.len() < META_HEADER_SIZE {
            return Err(NpdmError::BufferTooSmall {
                expected: META_HEADER_SIZE,
                actual: bytes.len(),
            });
        }

        // Validate META magic
        let meta_magic: [u8; 4] = bytes[0..4].try_into().unwrap();
        if meta_magic != MAGIC_META {
            return Err(NpdmError::InvalidMetaMagic { found: meta_magic });
        }

        let signature_key_generation = read_u32_le(bytes, 0x04)?;
        let flags0 = bytes[0x0C];
        let flags1 = bytes[0x0D];

        let is_64_bit_instruction = (flags0 & 1) != 0;
        let address_space_raw = (flags0 >> 1) & 0x07;
        let address_space = ProcessAddressSpace::from_raw(address_space_raw)?;
        let optimize_memory_allocation = (flags0 & (1 << 4)) != 0;
        let disable_device_address_space_merge = (flags0 & (1 << 5)) != 0;
        let enable_address_sanitizer = (flags0 & (1 << 6)) != 0;
        let prevent_code_reads = (flags0 & (1 << 7)) != 0;

        let enable_shadow_stack = (flags1 & 1) != 0;

        let main_thread_priority = bytes[0x0E];
        if main_thread_priority > 63 {
            return Err(NpdmError::InvalidPriority {
                priority: main_thread_priority,
            });
        }

        let default_cpu_id = bytes[0x0F];
        if default_cpu_id > 3 {
            return Err(NpdmError::InvalidCpuId {
                cpu_id: default_cpu_id,
            });
        }

        let system_resource_size = read_u32_le(bytes, 0x14)?;
        if (system_resource_size % PAGE_SIZE) != 0
            || system_resource_size > MAX_SYSTEM_RESOURCE_SIZE
        {
            return Err(NpdmError::InvalidSystemResourceSize {
                size: system_resource_size,
            });
        }

        let version = read_u32_le(bytes, 0x18)?;
        let main_thread_stack_size = read_u32_le(bytes, 0x1C)?;
        if (main_thread_stack_size % PAGE_SIZE) != 0 {
            return Err(NpdmError::InvalidStackSize {
                size: main_thread_stack_size,
            });
        }

        // Zero-allocation borrowed string parsing
        let program_name = read_borrowed_string(&bytes[0x20..0x30]);
        let product_code = read_borrowed_string(&bytes[0x30..0x40]);

        let aci_offset = read_u32_le(bytes, 0x70)?;
        let aci_size = read_u32_le(bytes, 0x74)?;
        let acid_offset = read_u32_le(bytes, 0x78)?;
        let acid_size = read_u32_le(bytes, 0x7C)?;

        // Validate ACID and ACI0 outer extents
        check_extent(
            "ACID",
            acid_offset,
            acid_size,
            ACID_HEADER_SIZE,
            bytes.len(),
        )?;
        check_extent("ACI0", aci_offset, aci_size, ACI0_HEADER_SIZE, bytes.len())?;

        // Validate that ACID and ACI0 sections do not overlap
        let acid_start = acid_offset as usize;
        let acid_end = acid_start + acid_size as usize;
        let aci_start = aci_offset as usize;
        let aci_end = aci_start + aci_size as usize;

        if ranges_overlap(acid_start, acid_end, aci_start, aci_end) {
            return Err(NpdmError::SectionOverlap {
                first: "ACID",
                second: "ACI0",
            });
        }

        // Parse and validate ACID
        let acid_slice = &bytes[acid_start..acid_end];
        let acid_magic: [u8; 4] = acid_slice[0x200..0x204].try_into().unwrap();
        if acid_magic != MAGIC_ACID {
            return Err(NpdmError::InvalidAcidMagic { found: acid_magic });
        }

        let acid_payload_size = read_u32_le(acid_slice, 0x204)?;
        // Signed payload starts at +0x100 (modulus) and covers at least header (0x140 bytes)
        if acid_payload_size < (ACID_HEADER_SIZE as u32 - 0x100) {
            return Err(NpdmError::InvalidAcidPayloadSize {
                payload_size: acid_payload_size,
                acid_size,
            });
        }
        let signed_end =
            0x100u32
                .checked_add(acid_payload_size)
                .ok_or(NpdmError::OffsetOverflow {
                    section: "ACID signed payload",
                    offset: 0x100,
                    size: acid_payload_size,
                })?;
        if signed_end > acid_size {
            return Err(NpdmError::InvalidAcidPayloadSize {
                payload_size: acid_payload_size,
                acid_size,
            });
        }

        let acid_version = acid_slice[0x208];
        let acid_unknown_209 = acid_slice[0x209];
        let acid_flags = read_u32_le(acid_slice, 0x20C)?;
        let is_retail = (acid_flags & 1) != 0;
        let unqualified_approval = (acid_flags & (1 << 1)) != 0;
        let pool_partition = ((acid_flags >> 2) & 0x0F) as u8;
        let load_browser_core_dll = (acid_flags & (1 << 7)) != 0;

        let program_id_min = read_u64_le(acid_slice, 0x210)?;
        let program_id_max = read_u64_le(acid_slice, 0x218)?;
        if program_id_min > program_id_max {
            return Err(NpdmError::InvalidProgramIdRange {
                min: program_id_min,
                max: program_id_max,
            });
        }

        let acid_fac_offset = read_u32_le(acid_slice, 0x220)?;
        let acid_fac_size = read_u32_le(acid_slice, 0x224)?;
        let acid_sac_offset = read_u32_le(acid_slice, 0x228)?;
        let acid_sac_size = read_u32_le(acid_slice, 0x22C)?;
        let acid_kac_offset = read_u32_le(acid_slice, 0x230)?;
        let acid_kac_size = read_u32_le(acid_slice, 0x234)?;

        // Validate ACID inner subregions within the signed envelope
        validate_subregion(
            "ACID FAC",
            acid_fac_offset,
            acid_fac_size,
            ACID_HEADER_SIZE,
            signed_end,
        )?;
        validate_subregion(
            "ACID SAC",
            acid_sac_offset,
            acid_sac_size,
            ACID_HEADER_SIZE,
            signed_end,
        )?;
        validate_subregion(
            "ACID KAC",
            acid_kac_offset,
            acid_kac_size,
            ACID_HEADER_SIZE,
            signed_end,
        )?;
        if acid_kac_size > 0 {
            if (acid_kac_offset % 4) != 0 {
                return Err(NpdmError::UnalignedSection {
                    section: "ACID KAC",
                    offset: acid_kac_offset,
                    alignment: 4,
                });
            }
            if (acid_kac_size % 4) != 0 {
                return Err(NpdmError::UnalignedSection {
                    section: "ACID KAC",
                    offset: acid_kac_size,
                    alignment: 4,
                });
            }
        }
        check_inner_overlaps([
            ("ACID FAC", acid_fac_offset, acid_fac_size),
            ("ACID SAC", acid_sac_offset, acid_sac_size),
            ("ACID KAC", acid_kac_offset, acid_kac_size),
        ])?;

        // Parse and validate ACI0
        let aci_slice = &bytes[aci_start..aci_end];
        let aci_magic: [u8; 4] = aci_slice[0x00..0x04].try_into().unwrap();
        if aci_magic != MAGIC_ACI0 {
            return Err(NpdmError::InvalidAci0Magic { found: aci_magic });
        }

        let program_id = read_u64_le(aci_slice, 0x10)?;
        // If ACID specifies an authorized program ID range, enforce containment
        if (program_id_min != 0 || program_id_max != 0)
            && (program_id < program_id_min || program_id > program_id_max)
        {
            return Err(NpdmError::ProgramIdOutOfAcidRange {
                program_id,
                min: program_id_min,
                max: program_id_max,
            });
        }

        let aci_fah_offset = read_u32_le(aci_slice, 0x20)?;
        let aci_fah_size = read_u32_le(aci_slice, 0x24)?;
        let aci_sac_offset = read_u32_le(aci_slice, 0x28)?;
        let aci_sac_size = read_u32_le(aci_slice, 0x2C)?;
        let aci_kac_offset = read_u32_le(aci_slice, 0x30)?;
        let aci_kac_size = read_u32_le(aci_slice, 0x34)?;

        // Validate ACI0 inner subregions
        validate_subregion(
            "ACI0 FAH",
            aci_fah_offset,
            aci_fah_size,
            ACI0_HEADER_SIZE,
            aci_size,
        )?;
        validate_subregion(
            "ACI0 SAC",
            aci_sac_offset,
            aci_sac_size,
            ACI0_HEADER_SIZE,
            aci_size,
        )?;
        validate_subregion(
            "ACI0 KAC",
            aci_kac_offset,
            aci_kac_size,
            ACI0_HEADER_SIZE,
            aci_size,
        )?;
        if aci_kac_size > 0 {
            if (aci_kac_offset % 4) != 0 {
                return Err(NpdmError::UnalignedSection {
                    section: "ACI0 KAC",
                    offset: aci_kac_offset,
                    alignment: 4,
                });
            }
            if (aci_kac_size % 4) != 0 {
                return Err(NpdmError::UnalignedSection {
                    section: "ACI0 KAC",
                    offset: aci_kac_size,
                    alignment: 4,
                });
            }
        }
        check_inner_overlaps([
            ("ACI0 FAH", aci_fah_offset, aci_fah_size),
            ("ACI0 SAC", aci_sac_offset, aci_sac_size),
            ("ACI0 KAC", aci_kac_offset, aci_kac_size),
        ])?;

        let meta = MetaHeader {
            signature_key_generation,
            is_64_bit_instruction,
            address_space,
            optimize_memory_allocation,
            disable_device_address_space_merge,
            enable_address_sanitizer,
            prevent_code_reads,
            enable_shadow_stack,
            main_thread_priority,
            default_cpu_id,
            system_resource_size,
            version,
            main_thread_stack_size,
            program_name,
            product_code,
            aci_offset,
            aci_size,
            acid_offset,
            acid_size,
        };

        let acid = AcidHeader {
            size: acid_payload_size,
            version: acid_version,
            unknown_209: acid_unknown_209,
            is_retail,
            unqualified_approval,
            pool_partition,
            load_browser_core_dll,
            program_id_min,
            program_id_max,
            fac_offset: acid_fac_offset,
            fac_size: acid_fac_size,
            sac_offset: acid_sac_offset,
            sac_size: acid_sac_size,
            kac_offset: acid_kac_offset,
            kac_size: acid_kac_size,
        };

        let aci0 = Aci0Header {
            program_id,
            fah_offset: aci_fah_offset,
            fah_size: aci_fah_size,
            sac_offset: aci_sac_offset,
            sac_size: aci_sac_size,
            kac_offset: aci_kac_offset,
            kac_size: aci_kac_size,
        };

        let npdm = Self {
            bytes,
            meta,
            acid,
            aci0,
        };

        // Validate descriptors and access records
        for cap in npdm.aci0_capabilities() {
            cap?;
        }
        for cap in npdm.acid_capabilities() {
            cap?;
        }
        for svc in npdm.aci0_services() {
            svc?;
        }
        for svc in npdm.acid_services() {
            svc?;
        }
        let _ = npdm.filesystem_access()?;

        Ok(npdm)
    }

    /// Returns the META header.
    pub fn meta(&self) -> &MetaHeader<'a> {
        &self.meta
    }

    /// Returns the ACID header.
    pub fn acid(&self) -> &AcidHeader {
        &self.acid
    }

    /// Returns the ACI0 header.
    pub fn aci0(&self) -> &Aci0Header {
        &self.aci0
    }

    /// Borrowed 256-byte RSA-2048 signature located at ACID + 0x000.
    pub fn signature(&self) -> &'a [u8] {
        let acid_start = self.meta.acid_offset as usize;
        &self.bytes[acid_start..acid_start + 0x100]
    }

    /// Borrowed 256-byte RSA-2048 public modulus located at ACID + 0x100.
    pub fn modulus(&self) -> &'a [u8] {
        let acid_start = self.meta.acid_offset as usize;
        &self.bytes[acid_start + 0x100..acid_start + 0x200]
    }

    /// Zero-allocation iterator over ACID kernel capability descriptors.
    pub fn acid_capabilities(&self) -> Capabilities<'a> {
        if self.acid.kac_size == 0 {
            return Capabilities::new(&[]);
        }
        let acid_start = self.meta.acid_offset as usize;
        let kac_start = acid_start + self.acid.kac_offset as usize;
        let kac_end = kac_start + self.acid.kac_size as usize;
        Capabilities::new(&self.bytes[kac_start..kac_end])
    }

    /// Zero-allocation iterator over ACI0 kernel capability descriptors.
    pub fn aci0_capabilities(&self) -> Capabilities<'a> {
        if self.aci0.kac_size == 0 {
            return Capabilities::new(&[]);
        }
        let aci_start = self.meta.aci_offset as usize;
        let kac_start = aci_start + self.aci0.kac_offset as usize;
        let kac_end = kac_start + self.aci0.kac_size as usize;
        Capabilities::new(&self.bytes[kac_start..kac_end])
    }

    /// Zero-allocation iterator over ACID Service Access Control entries.
    pub fn acid_services(&self) -> SacIterator<'a> {
        if self.acid.sac_size == 0 {
            return SacIterator::new(&[]);
        }
        let acid_start = self.meta.acid_offset as usize;
        let sac_start = acid_start + self.acid.sac_offset as usize;
        let sac_end = sac_start + self.acid.sac_size as usize;
        SacIterator::new(&self.bytes[sac_start..sac_end])
    }

    /// Zero-allocation iterator over ACI0 Service Access Control entries.
    pub fn aci0_services(&self) -> SacIterator<'a> {
        if self.aci0.sac_size == 0 {
            return SacIterator::new(&[]);
        }
        let aci_start = self.meta.aci_offset as usize;
        let sac_start = aci_start + self.aci0.sac_offset as usize;
        let sac_end = sac_start + self.aci0.sac_size as usize;
        SacIterator::new(&self.bytes[sac_start..sac_end])
    }

    /// Reads and validates combined filesystem access control from ACID FAC and ACI0 FAH.
    pub fn filesystem_access(&self) -> Result<Option<FsAccessControl<'a>>, NpdmError> {
        let fac_slice = if self.acid.fac_size == 0 {
            &[][..]
        } else {
            let acid_start = self.meta.acid_offset as usize;
            let fac_start = acid_start + self.acid.fac_offset as usize;
            let fac_end = fac_start + self.acid.fac_size as usize;
            &self.bytes[fac_start..fac_end]
        };

        let fah_slice = if self.aci0.fah_size == 0 {
            &[][..]
        } else {
            let aci_start = self.meta.aci_offset as usize;
            let fah_start = aci_start + self.aci0.fah_offset as usize;
            let fah_end = fah_start + self.aci0.fah_size as usize;
            &self.bytes[fah_start..fah_end]
        };

        FsAccessControl::parse(fac_slice, fah_slice)
    }

    /// Constructs the bridge metadata summary for configuring a Horizon process.
    pub fn to_process_metadata(&self) -> Result<ProcessMetadata, NpdmError> {
        let mut handle_table_size = 1024u16;
        let mut core_mask = 1u64 << self.meta.default_cpu_id;
        let mut priority_mask = 1u64 << self.meta.main_thread_priority;
        let mut is_application = self.acid.pool_partition == 0;
        let mut syscall_bitmap = SyscallBitmap::default();

        for cap in self.aci0_capabilities() {
            match cap? {
                Capability::KernelFlags {
                    highest_thread_priority,
                    lowest_thread_priority,
                    lowest_cpu_id,
                    highest_cpu_id,
                } => {
                    core_mask = 0;
                    for cpu in lowest_cpu_id..=highest_cpu_id {
                        core_mask |= 1u64 << cpu;
                    }
                    priority_mask = 0;
                    for prio in highest_thread_priority..=lowest_thread_priority {
                        priority_mask |= 1u64 << prio;
                    }
                }
                Capability::SyscallMask { slot, mask } => {
                    syscall_bitmap.apply_mask(slot, mask);
                }
                Capability::HandleTable { size } => {
                    handle_table_size = size;
                }
                Capability::ApplicationType { application_type } => {
                    is_application = application_type == 1;
                }
                Capability::Unknown { id, raw } => {
                    return Err(NpdmError::UnsupportedCapability { id, raw });
                }
                Capability::MapRange { .. }
                | Capability::IoMemoryMap { .. }
                | Capability::MapRegion { .. }
                | Capability::InterruptPair { .. }
                | Capability::KernelVersion { .. }
                | Capability::DebugFlags { .. } => {}
            }
        }

        // ACID capabilities must also not contain unknown/unsupported descriptors for bootstrap
        for cap in self.acid_capabilities() {
            if let Capability::Unknown { id, raw } = cap? {
                return Err(NpdmError::UnsupportedCapability { id, raw });
            }
        }

        let effective_fs_permissions = self
            .filesystem_access()?
            .map(|fs| fs.effective_permissions)
            .unwrap_or(0);

        Ok(ProcessMetadata {
            program_id: self.aci0.program_id,
            main_thread_priority: self.meta.main_thread_priority,
            ideal_core: self.meta.default_cpu_id,
            main_thread_stack_size: self.meta.main_thread_stack_size as u64,
            system_resource_size: self.meta.system_resource_size as u64,
            handle_table_size,
            core_mask,
            priority_mask,
            is_application,
            address_space: self.meta.address_space,
            syscall_bitmap,
            effective_fs_permissions,
        })
    }

    /// Performs read-only executable preflight inspection of process metadata and capabilities.
    pub fn preflight(&self) -> NpdmPreflight<'a> {
        let mut known_cap_count = 0;
        let mut unknown_caps = Vec::new();
        for cap in self.aci0_capabilities().flatten() {
            match cap {
                Capability::Unknown { id, raw } => {
                    unknown_caps.push(UnknownCapabilityInfo { id, raw });
                }
                _ => {
                    known_cap_count += 1;
                }
            }
        }

        let mut allowed_services = Vec::new();
        for svc in self.aci0_services().flatten() {
            allowed_services.push(svc.name);
        }

        let effective_fs_permissions = self
            .filesystem_access()
            .ok()
            .flatten()
            .map(|f| f.effective_permissions);

        NpdmPreflight {
            structural_validation_passed: true,
            program_id: self.aci0.program_id,
            program_name: self.meta.program_name,
            product_code: self.meta.product_code,
            main_thread_priority: self.meta.main_thread_priority,
            default_cpu_id: self.meta.default_cpu_id,
            main_thread_stack_size: self.meta.main_thread_stack_size,
            system_resource_size: self.meta.system_resource_size,
            address_space: self.meta.address_space,
            is_64_bit_instruction: self.meta.is_64_bit_instruction,
            known_capability_count: known_cap_count,
            unknown_capabilities: unknown_caps,
            allowed_services,
            effective_fs_permissions,
            signature_verification: "not performed",
            horizon_runtime_compatibility: if self.meta.is_64_bit_instruction {
                "AArch64 candidate"
            } else {
                "unsupported 32-bit instruction mode"
            },
        }
    }
}

/// Information on an unrecognized capability descriptor found during preflight.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct UnknownCapabilityInfo {
    pub id: u32,
    pub raw: u32,
}

/// Read-only preflight diagnostic summary of process configuration and capabilities.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct NpdmPreflight<'a> {
    pub structural_validation_passed: bool,
    pub program_id: u64,
    pub program_name: &'a str,
    pub product_code: &'a str,
    pub main_thread_priority: u8,
    pub default_cpu_id: u8,
    pub main_thread_stack_size: u32,
    pub system_resource_size: u32,
    pub address_space: ProcessAddressSpace,
    pub is_64_bit_instruction: bool,
    pub known_capability_count: usize,
    pub unknown_capabilities: Vec<UnknownCapabilityInfo>,
    pub allowed_services: Vec<&'a str>,
    pub effective_fs_permissions: Option<u64>,
    pub signature_verification: &'static str,
    pub horizon_runtime_compatibility: &'static str,
}

impl<'a> std::fmt::Display for NpdmPreflight<'a> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        writeln!(f, "NPDM structural validation: passed")?;
        writeln!(f, "Program ID: {:#018x}", self.program_id)?;
        writeln!(f, "Program name: \"{}\"", self.program_name)?;
        writeln!(f, "Product code: \"{}\"", self.product_code)?;
        writeln!(f, "Main thread priority: {}", self.main_thread_priority)?;
        writeln!(f, "Main thread core: {}", self.default_cpu_id)?;
        writeln!(
            f,
            "Main thread stack size: {:#x}",
            self.main_thread_stack_size
        )?;
        writeln!(f, "System resource size: {:#x}", self.system_resource_size)?;
        writeln!(f, "Address space: {:?}", self.address_space)?;
        writeln!(
            f,
            "Instruction mode: {}",
            if self.is_64_bit_instruction {
                "64-bit"
            } else {
                "32-bit"
            }
        )?;
        writeln!(f, "Known capabilities: {}", self.known_capability_count)?;
        if self.unknown_capabilities.is_empty() {
            writeln!(f, "Unknown capabilities: none")?;
        } else {
            write!(f, "Unknown capabilities: [")?;
            for (i, c) in self.unknown_capabilities.iter().enumerate() {
                if i > 0 {
                    write!(f, ", ")?;
                }
                write!(f, "{:#010x} (type id {})", c.raw, c.id)?;
            }
            writeln!(f, "]")?;
        }
        if let Some(perms) = self.effective_fs_permissions {
            writeln!(f, "Filesystem flags: {:#018x}", perms)?;
        } else {
            writeln!(f, "Filesystem flags: none")?;
        }
        writeln!(
            f,
            "Service access declarations: {} allowed",
            self.allowed_services.len()
        )?;
        writeln!(f, "Signature verification: {}", self.signature_verification)?;
        write!(
            f,
            "Horizon runtime compatibility: {}",
            self.horizon_runtime_compatibility
        )
    }
}

/// Convenience function to perform read-only preflight inspection of an NPDM buffer.
pub fn preflight_npdm(bytes: &[u8]) -> WlResult<NpdmPreflight<'_>> {
    let npdm = Npdm::parse(bytes).map_err(|e| e.to_string())?;
    Ok(npdm.preflight())
}

/// Convenience parser returning standard Wanderlust `Result<Npdm>`.
pub fn parse_npdm(bytes: &[u8]) -> WlResult<Npdm<'_>> {
    Npdm::parse(bytes).map_err(|e| e.to_string())
}

fn read_u32_le(bytes: &[u8], offset: usize) -> Result<u32, NpdmError> {
    let end = offset.checked_add(4).ok_or(NpdmError::OffsetOverflow {
        section: "header",
        offset: offset as u32,
        size: 4,
    })?;
    let slice = bytes.get(offset..end).ok_or(NpdmError::BufferTooSmall {
        expected: end,
        actual: bytes.len(),
    })?;
    Ok(u32::from_le_bytes(slice.try_into().unwrap()))
}

fn read_u64_le(bytes: &[u8], offset: usize) -> Result<u64, NpdmError> {
    let end = offset.checked_add(8).ok_or(NpdmError::OffsetOverflow {
        section: "header",
        offset: offset as u32,
        size: 8,
    })?;
    let slice = bytes.get(offset..end).ok_or(NpdmError::BufferTooSmall {
        expected: end,
        actual: bytes.len(),
    })?;
    Ok(u64::from_le_bytes(slice.try_into().unwrap()))
}

fn read_borrowed_string(bytes: &[u8]) -> &str {
    let len = bytes.iter().position(|&b| b == 0).unwrap_or(bytes.len());
    std::str::from_utf8(&bytes[..len]).unwrap_or("")
}

fn check_extent(
    section: &'static str,
    offset: u32,
    size: u32,
    min_size: usize,
    total_len: usize,
) -> Result<(), NpdmError> {
    if (size as usize) < min_size {
        return Err(NpdmError::BufferTooSmall {
            expected: min_size,
            actual: size as usize,
        });
    }
    let end = offset.checked_add(size).ok_or(NpdmError::OffsetOverflow {
        section,
        offset,
        size,
    })? as usize;
    if end > total_len {
        return Err(NpdmError::SectionOutOfBounds {
            section,
            end,
            total: total_len,
        });
    }
    if (offset as usize) < META_HEADER_SIZE {
        return Err(NpdmError::SectionOutOfBounds {
            section,
            end: offset as usize,
            total: total_len,
        });
    }
    Ok(())
}

fn validate_subregion(
    section: &'static str,
    offset: u32,
    size: u32,
    min_start: usize,
    parent_size: u32,
) -> Result<(), NpdmError> {
    if size == 0 {
        // If empty, offset can be 0 or within parent bounds
        if offset != 0
            && ((offset as usize) < min_start || (offset as usize) > parent_size as usize)
        {
            return Err(NpdmError::SectionOutOfBounds {
                section,
                end: offset as usize,
                total: parent_size as usize,
            });
        }
        return Ok(());
    }
    if (offset as usize) < min_start {
        return Err(NpdmError::SectionOutOfBounds {
            section,
            end: offset as usize,
            total: parent_size as usize,
        });
    }
    let end = offset.checked_add(size).ok_or(NpdmError::OffsetOverflow {
        section,
        offset,
        size,
    })? as usize;
    if end > (parent_size as usize) {
        return Err(NpdmError::SectionOutOfBounds {
            section,
            end,
            total: parent_size as usize,
        });
    }
    Ok(())
}

fn ranges_overlap(start1: usize, end1: usize, start2: usize, end2: usize) -> bool {
    start1 < end2 && start2 < end1
}

fn check_inner_overlaps(regions: [(&'static str, u32, u32); 3]) -> Result<(), NpdmError> {
    for i in 0..regions.len() {
        let (name_a, off_a, size_a) = regions[i];
        if size_a == 0 {
            continue;
        }
        let end_a = off_a as usize + size_a as usize;
        for j in (i + 1)..regions.len() {
            let (name_b, off_b, size_b) = regions[j];
            if size_b == 0 {
                continue;
            }
            let end_b = off_b as usize + size_b as usize;
            if ranges_overlap(off_a as usize, end_a, off_b as usize, end_b) {
                return Err(NpdmError::SectionOverlap {
                    first: name_a,
                    second: name_b,
                });
            }
        }
    }
    Ok(())
}
