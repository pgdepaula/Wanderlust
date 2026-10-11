//! Error types for NPDM parsing and validation.

use std::fmt;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum NpdmError {
    /// Buffer is smaller than the minimum required size.
    BufferTooSmall { expected: usize, actual: usize },
    /// META magic does not match `META`.
    InvalidMetaMagic { found: [u8; 4] },
    /// ACID magic does not match `ACID`.
    InvalidAcidMagic { found: [u8; 4] },
    /// ACI0 magic does not match `ACI0`.
    InvalidAci0Magic { found: [u8; 4] },
    /// Offset and size addition overflowed.
    OffsetOverflow {
        section: &'static str,
        offset: u32,
        size: u32,
    },
    /// Section extends beyond file buffer bounds.
    SectionOutOfBounds {
        section: &'static str,
        end: usize,
        total: usize,
    },
    /// Two sections overlap in the file.
    SectionOverlap {
        first: &'static str,
        second: &'static str,
    },
    /// Section offset or size violates required alignment.
    UnalignedSection {
        section: &'static str,
        offset: u32,
        alignment: usize,
    },
    /// Main thread stack size is not page-aligned (0x1000).
    InvalidStackSize { size: u32 },
    /// System resource size is not page-aligned or exceeds 0x1FE00000.
    InvalidSystemResourceSize { size: u32 },
    /// Main thread priority exceeds 63.
    InvalidPriority { priority: u8 },
    /// Default CPU ID exceeds 3.
    InvalidCpuId { cpu_id: u8 },
    /// Address space flags value is unknown (must be 0..=4).
    InvalidAddressSpace { raw: u8 },
    /// ACID program ID minimum exceeds maximum.
    InvalidProgramIdRange { min: u64, max: u64 },
    /// Program ID in ACI0 is outside ACID [program_id_min, program_id_max].
    ProgramIdOutOfAcidRange { program_id: u64, min: u64, max: u64 },
    /// ACID signed payload size field is invalid or exceeds section bounds.
    InvalidAcidPayloadSize { payload_size: u32, acid_size: u32 },
    /// Malformed capability descriptor bits.
    MalformedCapabilityDescriptor { desc: u32 },
    /// MapRange capability descriptor must be immediately followed by a size descriptor.
    UnpairedMapRangeDescriptor { desc: u32 },
    /// Syscall mask index exceeds maximum table index (0..=7).
    InvalidSyscallIndex { index: u32 },
    /// Kernel flags lowest priority exceeds highest priority (in Horizon 0 is highest priority).
    InvalidThreadPriorityBounds { highest: u8, lowest: u8 },
    /// Kernel flags core ID out of bounds.
    InvalidCpuIdBounds { lowest: u8, highest: u8 },
    /// Handle table size exceeds 1024.
    InvalidHandleTableSize { size: u32 },
    /// Malformed Service Access Control record.
    MalformedServiceAccessControl { offset: usize },
    /// Malformed or unsupported Filesystem Access Control record.
    MalformedFsAccessControl {
        section: &'static str,
        reason: &'static str,
    },
    /// Unsupported capability descriptor cannot be granted for process bootstrap.
    UnsupportedCapability { id: u32, raw: u32 },
    /// CorePriority capability was missing from ACI0 descriptors for process bootstrap.
    MissingCorePriorityCapability,
}

impl fmt::Display for NpdmError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::BufferTooSmall { expected, actual } => {
                write!(
                    f,
                    "buffer too small: expected at least {expected} bytes, got {actual}"
                )
            }
            Self::InvalidMetaMagic { found } => {
                write!(f, "invalid META magic: {found:02x?}")
            }
            Self::InvalidAcidMagic { found } => {
                write!(f, "invalid ACID magic: {found:02x?}")
            }
            Self::InvalidAci0Magic { found } => {
                write!(f, "invalid ACI0 magic: {found:02x?}")
            }
            Self::OffsetOverflow {
                section,
                offset,
                size,
            } => {
                write!(f, "{section} offset {offset:#x} + size {size:#x} overflows")
            }
            Self::SectionOutOfBounds {
                section,
                end,
                total,
            } => {
                write!(
                    f,
                    "{section} end {end:#x} exceeds total file size {total:#x}"
                )
            }
            Self::SectionOverlap { first, second } => {
                write!(f, "{first} and {second} sections overlap")
            }
            Self::UnalignedSection {
                section,
                offset,
                alignment,
            } => {
                write!(
                    f,
                    "{section} offset {offset:#x} is not aligned to {alignment} bytes"
                )
            }
            Self::InvalidStackSize { size } => {
                write!(
                    f,
                    "main thread stack size {size:#x} is not 0x1000 page-aligned"
                )
            }
            Self::InvalidSystemResourceSize { size } => {
                write!(
                    f,
                    "system resource size {size:#x} is invalid (must be page-aligned and <= 0x1FE00000)"
                )
            }
            Self::InvalidPriority { priority } => {
                write!(
                    f,
                    "main thread priority {priority} exceeds valid maximum of 63"
                )
            }
            Self::InvalidCpuId { cpu_id } => {
                write!(f, "default cpu id {cpu_id} exceeds valid core maximum of 3")
            }
            Self::InvalidAddressSpace { raw } => {
                write!(f, "unknown address space type {raw} (expected 0..=4)")
            }
            Self::InvalidProgramIdRange { min, max } => {
                write!(
                    f,
                    "ACID program id range invalid: min {min:#018x} > max {max:#018x}"
                )
            }
            Self::ProgramIdOutOfAcidRange {
                program_id,
                min,
                max,
            } => {
                write!(
                    f,
                    "ACI0 program id {program_id:#018x} outside ACID range [{min:#018x}, {max:#018x}]"
                )
            }
            Self::InvalidAcidPayloadSize {
                payload_size,
                acid_size,
            } => {
                write!(
                    f,
                    "ACID payload size {payload_size:#x} is invalid for section size {acid_size:#x}"
                )
            }
            Self::MalformedCapabilityDescriptor { desc } => {
                write!(f, "malformed capability descriptor {desc:#010x}")
            }
            Self::UnpairedMapRangeDescriptor { desc } => {
                write!(f, "unpaired MapRange capability descriptor {desc:#010x}")
            }
            Self::InvalidSyscallIndex { index } => {
                write!(f, "syscall mask index {index} exceeds maximum index of 7")
            }
            Self::InvalidThreadPriorityBounds { highest, lowest } => {
                write!(
                    f,
                    "kernel flags priority bounds invalid: highest {highest} > lowest {lowest}"
                )
            }
            Self::InvalidCpuIdBounds { lowest, highest } => {
                write!(
                    f,
                    "kernel flags core bounds invalid: lowest {lowest} > highest {highest} or > 3"
                )
            }
            Self::InvalidHandleTableSize { size } => {
                write!(f, "handle table size {size} exceeds maximum 1024")
            }
            Self::MalformedServiceAccessControl { offset } => {
                write!(f, "malformed SAC record at offset {offset:#x}")
            }
            Self::MalformedFsAccessControl { section, reason } => {
                write!(f, "malformed {section}: {reason}")
            }
            Self::UnsupportedCapability { id, raw } => {
                write!(
                    f,
                    "unsupported capability descriptor {raw:#010x} (type id {id}) cannot be granted for process bootstrap"
                )
            }
            Self::MissingCorePriorityCapability => {
                write!(
                    f,
                    "missing required CorePriority capability for process creation"
                )
            }
        }
    }
}

impl std::error::Error for NpdmError {}

impl From<NpdmError> for String {
    fn from(e: NpdmError) -> Self {
        e.to_string()
    }
}
