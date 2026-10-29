//! Opt-in, bounded pre-execution command snapshots for offline diagnostics.
//! A snapshot is not an execution trace: later GPU/CPU writes can change an
//! unread suffix. Execution must continue to fetch through the guarded engine.
use crate::{Result, gpfifo::Submission, invalid, memory::*};
use std::{
    fs::{self, File, OpenOptions},
    io::{Read, Write},
    path::{Path, PathBuf},
};

const MAGIC: &[u8; 8] = b"WLGPFC01";
const END: &[u8; 8] = b"WLGPFEND";
const MAX_FILE: usize = 8 * 1024 * 1024;
const MAX_TOTAL: u64 = 64 * 1024 * 1024;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SnapshotIdentity {
    pub process: u32,
    pub channel: u64,
    pub space: u64,
    pub sequence: u64,
    pub mapping_epoch: u64,
}
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct CaptureCounters {
    pub guest_read_calls: u64,
    pub guest_borrowed_bytes: u64,
    pub files: u64,
    pub file_bytes: u64,
    pub failures: u64,
}
/// At most 32 records / 64 MiB. Disabled after any error. Errors are diagnostic
/// only; callers must not use them to change channel acceptance or execution.
pub struct InitialCapture {
    directory: PathBuf,
    stopped: bool,
    counters: CaptureCounters,
}
impl InitialCapture {
    pub fn new(directory: impl Into<PathBuf>) -> Self {
        Self {
            directory: directory.into(),
            stopped: false,
            counters: CaptureCounters::default(),
        }
    }
    pub fn counters(&self) -> CaptureCounters {
        self.counters
    }
    pub fn record(
        &mut self,
        id: SnapshotIdentity,
        entries: &[u8],
        map: &AddressMap,
        memory: &impl GuestMemory,
    ) {
        if self.stopped || entries.is_empty() {
            return;
        }
        if self.write_record(id, entries, map, memory).is_err() {
            self.counters.failures += 1;
            self.stopped = true;
        }
    }
    fn write_record(
        &mut self,
        id: SnapshotIdentity,
        entries: &[u8],
        map: &AddressMap,
        memory: &impl GuestMemory,
    ) -> Result<()> {
        let submission = Submission::new(entries)?;
        let size = 56
            + entries.len()
            + submission.entry_count() * 4
            + submission.word_count() as usize * 4;
        if self.counters.files >= 32
            || size > MAX_FILE
            || self.counters.file_bytes + size as u64 > MAX_TOTAL
        {
            self.stopped = true;
            return Ok(());
        }
        fs::create_dir_all(&self.directory).map_err(io_error)?;
        let name = format!(
            "{:010}-{:020}-{:020}.gpfcap",
            id.process, id.channel, id.sequence
        );
        let path = self.directory.join(name);
        let temporary = path.with_extension("part");
        // Refuse replacement, including a pre-existing temporary file. Both names
        // include process, channel and sequence identities; no guest-controlled path.
        if path.exists() {
            return Err(invalid("command snapshot already exists"));
        }
        let mut options = OpenOptions::new();
        options.write(true).create_new(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        let mut file = options.open(&temporary).map_err(io_error)?;
        let result = (|| -> Result<()> {
            file.write_all(MAGIC).map_err(io_error)?;
            file.write_all(&id.process.to_le_bytes())
                .map_err(io_error)?;
            file.write_all(&(submission.entry_count() as u32).to_le_bytes())
                .map_err(io_error)?;
            for v in [id.channel, id.space, id.sequence, id.mapping_epoch] {
                file.write_all(&v.to_le_bytes()).map_err(io_error)?;
            }
            for (raw, entry) in entries.chunks_exact(8).zip(submission.entries()) {
                let address = map.resolve(entry.address(), entry.byte_len())?;
                self.counters.guest_read_calls += 1;
                let bytes = memory.read(address, entry.byte_len())?;
                if bytes.len() != entry.byte_len() {
                    return Err(invalid("short command snapshot read"));
                }
                self.counters.guest_borrowed_bytes += bytes.len() as u64;
                file.write_all(raw).map_err(io_error)?;
                file.write_all(&(bytes.len() as u32).to_le_bytes())
                    .map_err(io_error)?;
                file.write_all(bytes).map_err(io_error)?;
            }
            file.write_all(END).map_err(io_error)?;
            file.flush().map_err(io_error)?;
            // Hard-link publication is atomic and refuses to overwrite an existing
            // destination. Unlike rename it cannot silently replace another record.
            fs::hard_link(&temporary, &path).map_err(io_error)?;
            Ok(())
        })();
        drop(file);
        let _ = fs::remove_file(&temporary);
        result?;
        self.counters.files += 1;
        self.counters.file_bytes += size as u64;
        Ok(())
    }
}
fn io_error(e: std::io::Error) -> crate::Error {
    crate::Error(format!("command snapshot: {e}"))
}

/// One bounded owned diagnostic file, with borrowed entry payloads. No execution
/// or resource publication is performed when reading it.
pub struct CommandSnapshot {
    bytes: Vec<u8>,
    identity: SnapshotIdentity,
    ranges: Vec<(usize, usize)>,
}
impl CommandSnapshot {
    pub fn read(path: impl AsRef<Path>) -> Result<Self> {
        let file = File::open(path).map_err(io_error)?;
        if !file.metadata().map_err(io_error)?.is_file() {
            return Err(invalid("snapshot is not a regular file"));
        }
        let mut bytes = Vec::new();
        file.take(MAX_FILE as u64 + 1)
            .read_to_end(&mut bytes)
            .map_err(io_error)?;
        Self::decode(bytes)
    }
    pub fn decode(bytes: Vec<u8>) -> Result<Self> {
        if bytes.len() < 56 || bytes.len() > MAX_FILE || bytes.get(..8) != Some(MAGIC) {
            return Err(invalid("invalid command snapshot header/size"));
        }
        let u32_at = |i| u32::from_le_bytes(bytes[i..i + 4].try_into().unwrap());
        let u64_at = |i| u64::from_le_bytes(bytes[i..i + 8].try_into().unwrap());
        let count = u32_at(12) as usize;
        if count > crate::gpfifo::MAX_ENTRIES {
            return Err(invalid("snapshot entry budget"));
        }
        let identity = SnapshotIdentity {
            process: u32_at(8),
            channel: u64_at(16),
            space: u64_at(24),
            sequence: u64_at(32),
            mapping_epoch: u64_at(40),
        };
        let mut ranges = Vec::with_capacity(count);
        let mut offset = 48usize;
        let mut words = 0u64;
        for _ in 0..count {
            let header = bytes
                .get(offset..offset + 12)
                .ok_or_else(|| invalid("truncated snapshot entry"))?;
            let entry = crate::gpfifo::Entry::decode(header[..8].try_into().unwrap())
                .map_err(|e| crate::Error(e.to_string()))?;
            let length = u32::from_le_bytes(header[8..12].try_into().unwrap()) as usize;
            words += u64::from(entry.words());
            if length != entry.byte_len() || words > crate::gpfifo::MAX_WORDS {
                return Err(invalid("snapshot length/work budget"));
            }
            let end = offset
                .checked_add(12)
                .and_then(|v| v.checked_add(length))
                .ok_or_else(|| invalid("snapshot range overflow"))?;
            if end > bytes.len() {
                return Err(invalid("truncated snapshot payload"));
            }
            ranges.push((offset, end));
            offset = end;
        }
        if bytes.get(offset..) != Some(END) {
            return Err(invalid("snapshot footer or trailing data"));
        }
        Ok(Self {
            bytes,
            identity,
            ranges,
        })
    }
    pub fn identity(&self) -> SnapshotIdentity {
        self.identity
    }
    pub fn entries(&self) -> impl Iterator<Item = ([u8; 8], &[u8])> {
        self.ranges.iter().map(|&(start, end)| {
            (
                self.bytes[start..start + 8].try_into().unwrap(),
                &self.bytes[start + 12..end],
            )
        })
    }
}
