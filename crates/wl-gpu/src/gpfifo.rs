//! B06F USER/MAIN/PROCEED GPFIFO entries, distinct from nvdrv ioctl flags.
//! Sources and unsupported control-entry policy are recorded in graphics sources.
use crate::{Result, invalid, memory::GpuAddress};

pub const MAX_ENTRIES: usize = 2048;
pub const MAX_WORDS: u64 = 1_048_576;
pub const WIRE_MAX_WORDS: u32 = (1 << 21) - 1;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EntryError {
    ConditionalFetch,
    ReservedBit,
    Privileged,
    Subroutine,
    Wait,
    ControlEntry,
    AddressRange,
}
impl std::fmt::Display for EntryError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::ConditionalFetch => "unsupported GPFIFO conditional fetch",
            Self::ReservedBit => "reserved GPFIFO entry0 bit 1",
            Self::Privileged => "unsupported GPFIFO kernel privilege",
            Self::Subroutine => "unsupported GPFIFO subroutine level",
            Self::Wait => "unsupported GPFIFO SYNC_WAIT",
            Self::ControlEntry => "unsupported zero-length GPFIFO control entry",
            Self::AddressRange => "GPFIFO command range exceeds 40-bit GPU address space",
        })
    }
}
impl std::error::Error for EntryError {}

/// Validated nonempty command range. Length is in words, never bytes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Entry {
    address: GpuAddress,
    words: u32,
}
impl Entry {
    pub fn decode(bytes: [u8; 8]) -> std::result::Result<Self, EntryError> {
        let lo = u32::from_le_bytes(bytes[..4].try_into().unwrap());
        let hi = u32::from_le_bytes(bytes[4..].try_into().unwrap());
        if lo & 1 != 0 {
            return Err(EntryError::ConditionalFetch);
        }
        if lo & 2 != 0 {
            return Err(EntryError::ReservedBit);
        }
        if hi & 0x100 != 0 {
            return Err(EntryError::Privileged);
        }
        if hi & 0x200 != 0 {
            return Err(EntryError::Subroutine);
        }
        if hi & 0x8000_0000 != 0 {
            return Err(EntryError::Wait);
        }
        let words = (hi >> 10) & WIRE_MAX_WORDS;
        if words == 0 {
            return Err(EntryError::ControlEntry);
        }
        let address = u64::from(lo) | (u64::from(hi & 255) << 32);
        let end = address
            .checked_add(u64::from(words) * 4)
            .ok_or(EntryError::AddressRange)?;
        if end > 1 << 40 {
            return Err(EntryError::AddressRange);
        }
        Ok(Self {
            address: GpuAddress(address),
            words,
        })
    }
    pub fn address(self) -> GpuAddress {
        self.address
    }
    pub fn words(self) -> u32 {
        self.words
    }
    pub fn byte_len(self) -> usize {
        self.words as usize * 4
    }
}

/// Metadata-only preflight. No command memory or channel state is touched.
/// Entry decoding is repeated during execution to avoid an owned entry array.
#[derive(Debug)]
pub struct Submission<'a> {
    bytes: &'a [u8],
    words: u64,
}
impl<'a> Submission<'a> {
    pub fn new(bytes: &'a [u8]) -> Result<Self> {
        Self::new_counted(bytes, &mut 0)
    }
    pub(crate) fn new_counted(bytes: &'a [u8], decodes: &mut u64) -> Result<Self> {
        if bytes.len() % 8 != 0 {
            return Err(invalid(
                "GPFIFO entry array must contain complete eight-byte entries",
            ));
        }
        if bytes.len() / 8 > MAX_ENTRIES {
            return Err(invalid(
                "unsupported GPFIFO submission exceeds 2048 entries",
            ));
        }
        let mut words = 0u64;
        for raw in bytes.chunks_exact(8) {
            *decodes += 1;
            let entry =
                Entry::decode(raw.try_into().unwrap()).map_err(|e| crate::Error(e.to_string()))?;
            words = words
                .checked_add(u64::from(entry.words()))
                .ok_or_else(|| invalid("GPFIFO work size overflow"))?;
            if words > MAX_WORDS {
                return Err(invalid(
                    "unsupported GPFIFO submission exceeds 1048576 command words",
                ));
            }
        }
        Ok(Self { bytes, words })
    }
    pub fn entries(&self) -> impl Iterator<Item = Entry> + '_ {
        self.bytes
            .chunks_exact(8)
            .map(|b| Entry::decode(b.try_into().unwrap()).unwrap())
    }
    pub fn entry_count(&self) -> usize {
        self.bytes.len() / 8
    }
    pub fn word_count(&self) -> u64 {
        self.words
    }
}

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct Progress {
    pub entries_completed: usize,
    /// Includes a word consumed by the decoder even if its method faults.
    pub words_consumed: u64,
    /// Queue-accepted draws, not GPU completion or guest-visible writes.
    pub draws_submitted: u64,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FaultPhase {
    Preflight,
    Fetch,
    Execute,
}
#[derive(Debug)]
pub struct SubmissionFault {
    pub phase: FaultPhase,
    pub progress: Progress,
    pub cause: crate::Error,
}
impl std::fmt::Display for SubmissionFault {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "GPFIFO {:?} fault after {} entries/{} words/{} draws: {}",
            self.phase,
            self.progress.entries_completed,
            self.progress.words_consumed,
            self.progress.draws_submitted,
            self.cause
        )
    }
}
impl std::error::Error for SubmissionFault {}
