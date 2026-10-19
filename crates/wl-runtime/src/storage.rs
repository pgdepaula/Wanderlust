//! Read-only, explicitly mounted RomFS storage. No archive decryption or host path traversal.
use crate::{Memory, RuntimeResult};
use std::{
    fs::File,
    io::{Read, Seek, SeekFrom},
    path::Path,
    sync::{Arc, Mutex},
};

/// Conservative staging bound, independent of the file's potentially large size.
pub const MAX_READ: u64 = 64 * 1024 * 1024;

/// One already-open immutable image. Clones share the same file and serialize seeks.
#[derive(Clone, Debug)]
pub struct RomFs {
    file: Arc<Mutex<File>>,
    size: u64,
}
impl RomFs {
    pub fn open(path: impl AsRef<Path>) -> RuntimeResult<Self> {
        let mut file = File::open(path).map_err(|e| format!("RomFS open: {e}"))?;
        let metadata = file
            .metadata()
            .map_err(|e| format!("RomFS metadata: {e}"))?;
        if !metadata.is_file() || metadata.len() > i64::MAX as u64 {
            return Err("RomFS requires a regular file with a signed 64-bit size".into());
        }
        let mut header = [0; 0x50];
        file.read_exact(&mut header)
            .map_err(|e| format!("RomFS header: {e}"))?;
        Self::validate_header(&header, metadata.len())?;
        Ok(Self {
            file: Arc::new(Mutex::new(file)),
            size: metadata.len(),
        })
    }

    fn validate_header(header: &[u8; 0x50], size: u64) -> RuntimeResult<()> {
        let word = |n: usize| u64::from_le_bytes(header[n * 8..n * 8 + 8].try_into().unwrap());
        if word(0) != 0x50 || word(9) < 0x50 || word(9) > size {
            return Err("invalid RomFS header/data offset".into());
        }
        let mut ranges = Vec::with_capacity(4);
        for n in [1, 3, 5, 7] {
            let (offset, length) = (word(n), word(n + 1));
            let end = offset.checked_add(length).ok_or("RomFS table overflow")?;
            if offset < 0x50 || end > size {
                return Err("RomFS metadata outside file".into());
            }
            if length != 0 {
                if ranges.iter().any(|&(a, b)| offset < b && a < end) {
                    return Err("overlapping RomFS metadata".into());
                }
                ranges.push((offset, end));
            }
        }
        if word(2) == 0 || word(2) % 4 != 0 || word(6) == 0 || word(6) % 4 != 0 || word(4) < 24 {
            return Err("invalid RomFS table geometry".into());
        }
        // Tables may follow file data. This validates the container ranges,
        // not every directory/file record; the guest filesystem interprets them.
        Ok(())
    }

    pub fn size(&self) -> u64 {
        self.size
    }

    /// Full bounded read before one checked guest write. Host IO failures leave guest RAM intact.
    pub fn read_into_guest(
        &self,
        offset: i64,
        size: u64,
        destination: u64,
        buffer_size: u64,
        memory: &mut Memory,
    ) -> RuntimeResult<()> {
        let offset = u64::try_from(offset).map_err(|_| "negative IStorage read offset")?;
        if size > MAX_READ {
            return Err(format!(
                "IStorage read {size} exceeds staging limit {MAX_READ}"
            ));
        }
        if size > buffer_size {
            return Err(format!(
                "IStorage read {size} exceeds output buffer {buffer_size}"
            ));
        }
        let end = offset.checked_add(size).ok_or("IStorage range overflow")?;
        if end > self.size {
            return Err("IStorage read outside mounted image".into());
        }
        let length = usize::try_from(size).map_err(|_| "IStorage size exceeds host usize")?;
        memory.validate_write(destination, length)?;
        let mut file = self.file.lock().map_err(|_| "RomFS lock poisoned")?;
        if file
            .metadata()
            .map_err(|e| format!("RomFS metadata: {e}"))?
            .len()
            != self.size
        {
            return Err("mounted RomFS size changed".into());
        }
        let mut bytes = Vec::new();
        bytes
            .try_reserve_exact(length)
            .map_err(|e| format!("IStorage staging allocation: {e}"))?;
        bytes.resize(length, 0);
        file.seek(SeekFrom::Start(offset))
            .and_then(|_| file.read_exact(&mut bytes))
            .map_err(|e| format!("RomFS read: {e}"))?;
        memory.write_bytes(destination, &bytes)
    }
}
