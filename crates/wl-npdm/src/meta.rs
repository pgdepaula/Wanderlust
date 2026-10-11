//! Header definitions and raw structures for NPDM META, ACID, and ACI0.
//!
//! Layout matches Atmosphère's `stratosphere/ldr/ldr_types.hpp` and Switchbrew specs.

use crate::error::NpdmError;
use serde::Serialize;

pub const MAGIC_META: [u8; 4] = *b"META";
pub const MAGIC_ACID: [u8; 4] = *b"ACID";
pub const MAGIC_ACI0: [u8; 4] = *b"ACI0";

pub const META_HEADER_SIZE: usize = 0x80;
pub const ACID_HEADER_SIZE: usize = 0x240;
pub const ACI0_HEADER_SIZE: usize = 0x40;

pub const PAGE_SIZE: u32 = 0x1000;
pub const MAX_SYSTEM_RESOURCE_SIZE: u32 = 0x1FE0_0000;

/// Process address space configuration (from META flags0 bits 1..=3).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
pub enum ProcessAddressSpace {
    /// 32-bit address space with 1GB reserved region.
    AddressSpace32Bit = 0,
    /// 64-bit address space with 36-bit range (64 GB).
    AddressSpace64Bit36 = 1,
    /// 32-bit address space with no reserved region.
    AddressSpace32BitNoReserved = 2,
    /// 64-bit address space with 39-bit range (512 GB) - standard application layout.
    AddressSpace64Bit39 = 3,
    /// 64-bit address space with 42-bit range (4 TB).
    AddressSpace64Bit42 = 4,
}

impl ProcessAddressSpace {
    pub fn from_raw(raw: u8) -> Result<Self, NpdmError> {
        match raw {
            0 => Ok(Self::AddressSpace32Bit),
            1 => Ok(Self::AddressSpace64Bit36),
            2 => Ok(Self::AddressSpace32BitNoReserved),
            3 => Ok(Self::AddressSpace64Bit39),
            4 => Ok(Self::AddressSpace64Bit42),
            other => Err(NpdmError::InvalidAddressSpace { raw: other }),
        }
    }

    pub fn is_64_bit(&self) -> bool {
        matches!(
            self,
            Self::AddressSpace64Bit36 | Self::AddressSpace64Bit39 | Self::AddressSpace64Bit42
        )
    }
}

/// Parsed NPDM META header (0x80 bytes), borrowed without heap allocation.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct MetaHeader<'a> {
    pub signature_key_generation: u32,
    pub is_64_bit_instruction: bool,
    pub address_space: ProcessAddressSpace,
    pub optimize_memory_allocation: bool,
    pub disable_device_address_space_merge: bool,
    pub enable_address_sanitizer: bool,
    pub prevent_code_reads: bool,
    pub enable_shadow_stack: bool,
    pub main_thread_priority: u8,
    pub default_cpu_id: u8,
    pub system_resource_size: u32,
    pub version: u32,
    /// Initial stack size. If 0, the kernel starts the initial process thread with `sp = 0`
    /// (no kernel-allocated stack; userland startup / rtld establishes its own stack).
    pub main_thread_stack_size: u32,
    pub program_name: &'a str,
    pub product_code: &'a str,
    pub aci_offset: u32,
    pub aci_size: u32,
    pub acid_offset: u32,
    pub acid_size: u32,
}

/// Parsed ACID header and extents (0x240 bytes base).
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct AcidHeader {
    pub size: u32,
    pub version: u8,
    pub unknown_209: u8,
    pub is_retail: bool,
    pub unqualified_approval: bool,
    pub pool_partition: u8,
    pub load_browser_core_dll: bool,
    pub program_id_min: u64,
    pub program_id_max: u64,
    pub fac_offset: u32,
    pub fac_size: u32,
    pub sac_offset: u32,
    pub sac_size: u32,
    pub kac_offset: u32,
    pub kac_size: u32,
}

/// Parsed ACI0 header and extents (0x40 bytes base).
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct Aci0Header {
    pub program_id: u64,
    pub fah_offset: u32,
    pub fah_size: u32,
    pub sac_offset: u32,
    pub sac_size: u32,
    pub kac_offset: u32,
    pub kac_size: u32,
}
