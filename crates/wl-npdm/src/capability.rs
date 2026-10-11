//! Authoritative Kernel Capability (KAC) descriptor types and iterator.
//!
//! Encodings strictly follow Atmosphère (`libstratosphere/ldr_capabilities`) and
//! Switchbrew authoritative definitions. All parsing is zero-allocation and bounds-checked.

use crate::error::NpdmError;
use serde::Serialize;

/// Kernel capability descriptor identifier based on low delimiter bit pattern.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
pub enum CapabilityId {
    KernelFlags = 3,
    SyscallMask = 4,
    MapRange = 6,
    IoMemoryMap = 7,
    MapRegion = 10,
    InterruptPair = 11,
    ApplicationType = 13,
    KernelVersion = 14,
    HandleTable = 15,
    DebugFlags = 16,
    Empty = 32,
}

impl CapabilityId {
    pub fn from_descriptor(desc: u32) -> Option<Self> {
        let trailing_ones = (!desc).trailing_zeros();
        match trailing_ones {
            3 => Some(Self::KernelFlags),
            4 => Some(Self::SyscallMask),
            6 => Some(Self::MapRange),
            7 => Some(Self::IoMemoryMap),
            10 => Some(Self::MapRegion),
            11 => Some(Self::InterruptPair),
            13 => Some(Self::ApplicationType),
            14 => Some(Self::KernelVersion),
            15 => Some(Self::HandleTable),
            16 => Some(Self::DebugFlags),
            32 => Some(Self::Empty),
            _ => None,
        }
    }
}

/// Strongly typed kernel capability descriptors.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
pub enum Capability {
    /// Thread priority limits and allowed core range.
    KernelFlags {
        highest_thread_priority: u8,
        lowest_thread_priority: u8,
        lowest_cpu_id: u8,
        highest_cpu_id: u8,
    },
    /// Mask of 24 syscalls at `slot * 24`.
    SyscallMask { slot: u8, mask: u32 },
    /// Memory mapping range for IO or Normal physical memory.
    MapRange {
        address: u64,
        size: u64,
        is_read_only: bool,
        is_io: bool,
    },
    /// Single mapped physical IO page (Switchbrew #IoMemoryMap).
    IoMemoryMap { address: u64 },
    /// Memory region mappings (KernelTraceBuffer, OnMemoryBootImage, DTB).
    MapRegion {
        region0: u8,
        read_only0: bool,
        region1: u8,
        read_only1: bool,
        region2: u8,
        read_only2: bool,
    },
    /// Pair of allowed hardware interrupts (0x3FF represents unset/none).
    InterruptPair {
        irq0: Option<u16>,
        irq1: Option<u16>,
    },
    /// Application type (0 = System, 1 = Application, 2 = Applet).
    ApplicationType { application_type: u8 },
    /// Intended Horizon kernel release version.
    KernelVersion { major: u16, minor: u8 },
    /// Process handle table size limit.
    HandleTable { size: u16 },
    /// Debug control flags.
    DebugFlags {
        allow_debug: bool,
        force_debug_prod: bool,
        force_debug: bool,
    },
    /// Unrecognized or newer firmware capability preserved in diagnostic form.
    Unknown { id: u32, raw: u32 },
}

/// Zero-allocation borrowed iterator yielding parsed kernel capabilities.
#[derive(Clone, Debug)]
pub struct Capabilities<'a> {
    words: &'a [u8],
    offset: usize,
}

impl<'a> Capabilities<'a> {
    pub(crate) fn new(words: &'a [u8]) -> Self {
        Self { words, offset: 0 }
    }

    /// Total number of 32-bit descriptor words in the slice.
    pub fn word_count(&self) -> usize {
        self.words.len() / 4
    }

    fn read_u32(&self, at: usize) -> Option<u32> {
        let chunk = self.words.get(at..at.checked_add(4)?)?;
        Some(u32::from_le_bytes(chunk.try_into().ok()?))
    }
}

impl<'a> Iterator for Capabilities<'a> {
    type Item = Result<Capability, NpdmError>;

    fn next(&mut self) -> Option<Self::Item> {
        while self.offset < self.words.len() {
            let desc = match self.read_u32(self.offset) {
                Some(w) => w,
                None => return None,
            };
            self.offset += 4;

            let trailing_ones = (!desc).trailing_zeros();
            if trailing_ones == 32 {
                // Empty padding (0xFFFFFFFF)
                continue;
            }

            let id = match CapabilityId::from_descriptor(desc) {
                Some(id) => id,
                None => {
                    // Preserve unknown capability in diagnostic form without failing parser
                    return Some(Ok(Capability::Unknown {
                        id: trailing_ones,
                        raw: desc,
                    }));
                }
            };

            let item = match id {
                CapabilityId::KernelFlags => {
                    let highest_thread_priority = ((desc >> 4) & 0x3F) as u8;
                    let lowest_thread_priority = ((desc >> 10) & 0x3F) as u8;
                    let lowest_cpu_id = ((desc >> 16) & 0xFF) as u8;
                    let highest_cpu_id = ((desc >> 24) & 0xFF) as u8;

                    if highest_thread_priority > lowest_thread_priority {
                        return Some(Err(NpdmError::InvalidThreadPriorityBounds {
                            highest: highest_thread_priority,
                            lowest: lowest_thread_priority,
                        }));
                    }
                    if lowest_cpu_id > highest_cpu_id || highest_cpu_id > 3 {
                        return Some(Err(NpdmError::InvalidCpuIdBounds {
                            lowest: lowest_cpu_id,
                            highest: highest_cpu_id,
                        }));
                    }

                    Capability::KernelFlags {
                        highest_thread_priority,
                        lowest_thread_priority,
                        lowest_cpu_id,
                        highest_cpu_id,
                    }
                }
                CapabilityId::SyscallMask => {
                    let mask = (desc >> 5) & 0x00FF_FFFF;
                    let slot = ((desc >> 29) & 0x07) as u8;
                    Capability::SyscallMask { slot, mask }
                }
                CapabilityId::MapRange => {
                    // MapRange descriptors always come in pairs.
                    let addr_shifted = (desc >> 7) & 0x00FF_FFFF;
                    let is_read_only = ((desc >> 31) & 1) != 0;

                    let next_desc = match self.read_u32(self.offset) {
                        Some(w) => w,
                        None => {
                            return Some(Err(NpdmError::UnpairedMapRangeDescriptor { desc }));
                        }
                    };

                    if CapabilityId::from_descriptor(next_desc) != Some(CapabilityId::MapRange) {
                        return Some(Err(NpdmError::UnpairedMapRangeDescriptor {
                            desc: next_desc,
                        }));
                    }
                    self.offset += 4;

                    let size_shifted = (next_desc >> 7) & 0x00FF_FFFF;
                    let is_io = ((next_desc >> 31) & 1) == 0;

                    Capability::MapRange {
                        address: (addr_shifted as u64) << 12,
                        size: (size_shifted as u64) << 12,
                        is_read_only,
                        is_io,
                    }
                }
                CapabilityId::IoMemoryMap => {
                    let addr_shifted = (desc >> 8) & 0x00FF_FFFF;
                    Capability::IoMemoryMap {
                        address: (addr_shifted as u64) << 12,
                    }
                }
                CapabilityId::MapRegion => {
                    let region0 = ((desc >> 11) & 0x3F) as u8;
                    let read_only0 = ((desc >> 17) & 1) != 0;
                    let region1 = ((desc >> 18) & 0x3F) as u8;
                    let read_only1 = ((desc >> 24) & 1) != 0;
                    let region2 = ((desc >> 25) & 0x3F) as u8;
                    let read_only2 = ((desc >> 31) & 1) != 0;

                    Capability::MapRegion {
                        region0,
                        read_only0,
                        region1,
                        read_only1,
                        region2,
                        read_only2,
                    }
                }
                CapabilityId::InterruptPair => {
                    let raw_irq0 = ((desc >> 12) & 0x3FF) as u16;
                    let raw_irq1 = ((desc >> 22) & 0x3FF) as u16;
                    let irq0 = if raw_irq0 == 0x3FF {
                        None
                    } else {
                        Some(raw_irq0)
                    };
                    let irq1 = if raw_irq1 == 0x3FF {
                        None
                    } else {
                        Some(raw_irq1)
                    };
                    Capability::InterruptPair { irq0, irq1 }
                }
                CapabilityId::ApplicationType => {
                    let application_type = ((desc >> 14) & 0x07) as u8;
                    Capability::ApplicationType { application_type }
                }
                CapabilityId::KernelVersion => {
                    let minor = ((desc >> 15) & 0x0F) as u8;
                    let major = ((desc >> 19) & 0x1FFF) as u16;
                    Capability::KernelVersion { major, minor }
                }
                CapabilityId::HandleTable => {
                    let size = (desc >> 16) as u16;
                    if size > 1024 {
                        return Some(Err(NpdmError::InvalidHandleTableSize { size: size as u32 }));
                    }
                    Capability::HandleTable { size }
                }
                CapabilityId::DebugFlags => {
                    let allow_debug = ((desc >> 17) & 1) != 0;
                    let force_debug_prod = ((desc >> 18) & 1) != 0;
                    let force_debug = ((desc >> 19) & 1) != 0;
                    Capability::DebugFlags {
                        allow_debug,
                        force_debug_prod,
                        force_debug,
                    }
                }
                CapabilityId::Empty => unreachable!(),
            };

            return Some(Ok(item));
        }

        None
    }
}

/// Bitset covering allowed Horizon SVCs (0x00 through 0xBF).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default, Serialize)]
pub struct SyscallBitmap {
    words: [u32; 6],
}

impl SyscallBitmap {
    pub const TOTAL_SVCS: usize = 0xC0;

    /// Checks if a specific system call ID is permitted.
    pub fn is_allowed(&self, svc_id: u8) -> bool {
        let idx = svc_id as usize;
        if idx >= Self::TOTAL_SVCS {
            return false;
        }
        let word_idx = idx / 32;
        let bit_idx = idx % 32;
        (self.words[word_idx] & (1 << bit_idx)) != 0
    }

    /// Enables a range from a 24-bit slot mask.
    pub fn apply_mask(&mut self, slot: u8, mask24: u32) {
        if slot > 7 {
            return;
        }
        let base_svc = slot as usize * 24;
        for bit in 0..24 {
            if (mask24 & (1 << bit)) != 0 {
                let svc = base_svc + bit;
                if svc < Self::TOTAL_SVCS {
                    let word_idx = svc / 32;
                    let bit_idx = svc % 32;
                    self.words[word_idx] |= 1 << bit_idx;
                }
            }
        }
    }
}
