//! NSO0 version-zero loader. Guest offsets remain distinct from file offsets.
pub mod dynamic;
use serde::Serialize;
use sha2::{Digest, Sha256};
use std::{fs::File, io::Read, path::Path};
use wl_common::{FileOffset, ModuleOffset, Result};

pub const MAX_INPUT: usize = 512 * 1024 * 1024;
pub const MAX_IMAGE: u64 = 512 * 1024 * 1024;

#[derive(Clone, Debug, Serialize)]
pub struct Segment {
    pub name: &'static str,
    pub file_offset: FileOffset,
    pub guest_offset: ModuleOffset,
    pub file_size: u32,
    pub memory_size: u32,
    pub compressed: bool,
    pub hash_verified: bool,
    pub readable: bool,
    pub writable: bool,
    pub executable: bool,
    #[serde(skip)]
    pub bytes: Vec<u8>,
}
#[derive(Clone, Debug, Serialize)]
pub struct RoRange {
    pub name: &'static str,
    pub offset: u32,
    pub size: u32,
}
#[derive(Clone, Debug, Serialize)]
pub struct Module {
    pub format: &'static str,
    pub sha256: String,
    pub module_id: String,
    pub flags: u32,
    pub module_name_offset: FileOffset,
    pub module_name_size: u32,
    pub segments: Vec<Segment>,
    pub bss_offset: ModuleOffset,
    pub bss_size: u32,
    pub ro_metadata: Vec<RoRange>,
    pub mod0: Option<Mod0>,
    pub constraints: &'static str,
}
#[derive(Clone, Debug, Serialize)]
pub struct Mod0 {
    pub offset: ModuleOffset,
    pub dynamic_offset: ModuleOffset,
    pub bss_start: ModuleOffset,
    pub bss_end: ModuleOffset,
    /// `.eh_frame_hdr` bounds (MOD0 words 4 and 5).
    pub unwind_start: ModuleOffset,
    pub unwind_end: ModuleOffset,
    /// Storage reserved for rtld's per-module object (MOD0 word 6); normally inside BSS.
    pub module_object: ModuleOffset,
    pub dynamic_entries: Vec<DynamicEntry>,
    pub dynamic: dynamic::Dynamic,
}
#[derive(Clone, Debug, Serialize)]
pub struct DynamicEntry {
    pub tag: i64,
    pub value: u64,
}

pub fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}
pub fn read_input(path: &Path) -> Result<Vec<u8>> {
    let mut file = File::open(path).map_err(|e| format!("{}: {e}", path.display()))?;
    let size = file.metadata().map_err(|e| e.to_string())?.len();
    if size > MAX_INPUT as u64 {
        return Err(format!("input exceeds {MAX_INPUT} byte limit"));
    }
    let mut bytes = Vec::new();
    file.by_ref()
        .take(MAX_INPUT as u64 + 1)
        .read_to_end(&mut bytes)
        .map_err(|e| e.to_string())?;
    if bytes.len() > MAX_INPUT {
        return Err("input grew beyond size limit".into());
    }
    Ok(bytes)
}
pub(crate) fn u32_at(bytes: &[u8], at: usize) -> Result<u32> {
    let end = at.checked_add(4).ok_or("header offset overflow")?;
    let word = bytes
        .get(at..end)
        .ok_or_else(|| format!("truncated u32 at file offset {at:#x}"))?;
    Ok(u32::from_le_bytes(
        word.try_into().map_err(|_| "invalid u32")?,
    ))
}
fn overlap(a: (u64, u64), b: (u64, u64)) -> bool {
    a.0 < b.1 && b.0 < a.1 && a.0 != a.1 && b.0 != b.1
}

impl Module {
    pub fn parse(bytes: &[u8]) -> Result<Self> {
        if bytes.len() > MAX_INPUT {
            return Err("input exceeds size limit".into());
        }
        if bytes.len() < 0x100 {
            return Err("NSO header truncated: requires 0x100 bytes".into());
        }
        if &bytes[..4] != b"NSO0" {
            return Err(
                "invalid NSO magic (expected NSO0); NRO/encrypted containers are unsupported"
                    .into(),
            );
        }
        let version = u32_at(bytes, 4)?;
        if version != 0 {
            return Err(format!("unsupported NSO version {version}"));
        }
        let flags = u32_at(bytes, 0xc)?;
        if flags & !0x7f != 0 {
            return Err(format!(
                "unsupported NSO flags {flags:#x} (ZBIC/zstd is not supported)"
            ));
        }
        let bss_size = u32_at(bytes, 0x3c)?;
        let module_name_offset = FileOffset(u32_at(bytes, 0x1c)?);
        let module_name_size = u32_at(bytes, 0x2c)?;
        let name_end = u64::from(module_name_offset.0) + u64::from(module_name_size);
        if module_name_size != 0 && (module_name_offset.0 < 0x100 || name_end > bytes.len() as u64)
        {
            return Err("module name file range is invalid".into());
        }
        let mut segments = Vec::new();
        let mut file_ranges = Vec::new();
        let mut memory_ranges = Vec::new();
        let mut total = u64::from(bss_size);
        for (i, name) in ["text", "rodata", "data"].iter().enumerate() {
            let at = 0x10 + i * 0x10;
            let file_offset = u32_at(bytes, at)?;
            let guest_offset = u32_at(bytes, at + 4)?;
            let memory_size = u32_at(bytes, at + 8)?;
            let compressed = flags & (1 << i) != 0;
            let stored_size = u32_at(bytes, 0x60 + i * 4)?;
            let file_size = if compressed { stored_size } else { memory_size };
            let file_end = u64::from(file_offset) + u64::from(file_size);
            let memory_end = u64::from(guest_offset) + u64::from(memory_size);
            if memory_end > u64::from(u32::MAX) {
                return Err(format!("{name}: guest-relative address overflow"));
            }
            if memory_size != 0 && (file_offset < 0x100 || file_end > bytes.len() as u64) {
                return Err(format!(
                    "{name}: file range {file_offset:#x}..{file_end:#x} is outside input"
                ));
            }
            if name == &"text" && (guest_offset % 4 != 0 || memory_size % 4 != 0) {
                return Err("text: guest offset/size must be instruction aligned".into());
            }
            total = total
                .checked_add(u64::from(memory_size))
                .ok_or("image size overflow")?;
            if total > MAX_IMAGE || memory_end > MAX_IMAGE {
                return Err("NSO exceeds 512 MiB reconstructed-image limit".into());
            }
            let fr = (u64::from(file_offset), file_end);
            let mr = (u64::from(guest_offset), memory_end);
            if file_ranges.iter().any(|r| overlap(*r, fr)) {
                return Err(format!("{name}: overlapping segment file ranges"));
            }
            if module_name_size > 0 && overlap(fr, (u64::from(module_name_offset.0), name_end)) {
                return Err(format!("{name}: overlaps module name"));
            }
            if memory_ranges.iter().any(|r| overlap(*r, mr)) {
                return Err(format!("{name}: overlapping segment memory ranges"));
            }
            file_ranges.push(fr);
            memory_ranges.push(mr);
            segments.push(Segment {
                name,
                file_offset: FileOffset(file_offset),
                guest_offset: ModuleOffset(u64::from(guest_offset)),
                file_size,
                memory_size,
                compressed,
                hash_verified: false,
                readable: i != 0 || flags & 0x40 == 0,
                writable: i == 2,
                executable: i == 0,
                bytes: Vec::new(),
            });
        }
        let bss_offset =
            ModuleOffset(segments[2].guest_offset.0 + u64::from(segments[2].memory_size));
        let bss_end = bss_offset.0 + u64::from(bss_size);
        if bss_end > MAX_IMAGE {
            return Err("BSS exceeds reconstructed-image limit".into());
        }
        if memory_ranges
            .iter()
            .any(|r| overlap(*r, (bss_offset.0, bss_end)))
        {
            return Err("BSS overlaps another segment".into());
        }
        let mut ro_metadata = Vec::new();
        for (at, name) in [(0x88, "embedded"), (0x90, "dynstr"), (0x98, "dynsym")] {
            let offset = u32_at(bytes, at)?;
            let size = u32_at(bytes, at + 4)?;
            if size != 0 && u64::from(offset) + u64::from(size) > u64::from(segments[1].memory_size)
            {
                return Err(format!("{name}: metadata exceeds rodata"));
            }
            ro_metadata.push(RoRange { name, offset, size });
        }
        // All ranges and allocation limits are checked before decompression.
        for (i, segment) in segments.iter_mut().enumerate() {
            let begin = segment.file_offset.0 as usize;
            let stored = if segment.file_size == 0 {
                &[][..]
            } else {
                &bytes[begin..begin + segment.file_size as usize]
            };
            let mut decoded = Vec::new();
            decoded
                .try_reserve_exact(segment.memory_size as usize)
                .map_err(|e| format!("{} allocation: {e}", segment.name))?;
            decoded.resize(segment.memory_size as usize, 0);
            if segment.compressed {
                let size = lz4_flex::block::decompress_into(stored, &mut decoded)
                    .map_err(|e| format!("{}: invalid LZ4 block: {e}", segment.name))?;
                if size != decoded.len() {
                    return Err(format!(
                        "{}: decompressed size {size} differs from declared {}",
                        segment.name,
                        decoded.len()
                    ));
                }
            } else {
                decoded.copy_from_slice(stored);
            }
            if flags & (1 << (i + 3)) != 0 {
                if Sha256::digest(&decoded)[..] != bytes[0xa0 + i * 32..0xc0 + i * 32] {
                    return Err(format!("{}: SHA-256 segment hash mismatch", segment.name));
                }
                segment.hash_verified = true;
            }
            segment.bytes = decoded;
        }
        let mut module = Self {
            format: "NSO0",
            sha256: hex(&Sha256::digest(bytes)),
            module_id: hex(&bytes[0x40..0x60]),
            flags,
            module_name_offset,
            module_name_size,
            segments,
            bss_offset,
            bss_size,
            ro_metadata,
            mod0: None,
            constraints: "Offsets are module-relative, not process virtual addresses. NSO has no entry-point field. MOD0 dynamic metadata is parsed here; placement and relocation are performed by wl-loader. No Horizon process initialization.",
        };
        module.mod0 = module.find_mod0()?;
        Ok(module)
    }
    fn bytes_at(&self, address: u64, size: usize) -> Result<&[u8]> {
        for s in &self.segments {
            if let Some(offset) = address
                .checked_sub(s.guest_offset.0)
                .and_then(|v| usize::try_from(v).ok())
            {
                if let Some(slice) = offset
                    .checked_add(size)
                    .and_then(|end| s.bytes.get(offset..end))
                {
                    return Ok(slice);
                }
            }
        }
        Err(format!(
            "module-relative range {address:#x} + {size:#x} is unmapped"
        ))
    }
    fn find_mod0(&self) -> Result<Option<Mod0>> {
        let text = &self.segments[0];
        if text.bytes.len() < 8 {
            return Ok(None);
        }
        let offset = u64::from(u32_at(&text.bytes, 4)?);
        // An arbitrary instruction stream may have no module header; do not infer one from word 1 alone.
        if self.bytes_at(offset, 4).ok() != Some(&b"MOD0"[..]) {
            return Ok(None);
        }
        let header = self.bytes_at(offset, 0x1c)?;
        let relative = |at| -> Result<ModuleOffset> {
            let delta = u32_at(header, at)? as i32 as i64;
            Ok(ModuleOffset(
                offset
                    .checked_add_signed(delta)
                    .ok_or("MOD0 relative address overflow")?,
            ))
        };
        let dynamic_offset = relative(4)?;
        let bss_start = relative(8)?;
        let bss_end = relative(12)?;
        let unwind_start = relative(16)?;
        let unwind_end = relative(20)?;
        let module_object = relative(24)?;
        if bss_start.0 > bss_end.0 {
            return Err("MOD0 BSS range is reversed".into());
        }
        let mut dynamic_entries = Vec::new();
        for i in 0..4096u64 {
            let at = dynamic_offset
                .0
                .checked_add(i * 16)
                .ok_or("dynamic table overflow")?;
            let entry = self.bytes_at(at, 16)?;
            let tag =
                i64::from_le_bytes(entry[..8].try_into().map_err(|_| "dynamic tag truncated")?);
            let value = u64::from_le_bytes(
                entry[8..]
                    .try_into()
                    .map_err(|_| "dynamic value truncated")?,
            );
            if tag == 0 {
                let dynamic = dynamic::Dynamic::parse(self, &dynamic_entries)?;
                return Ok(Some(Mod0 {
                    offset: ModuleOffset(offset),
                    dynamic_offset,
                    bss_start,
                    bss_end,
                    unwind_start,
                    unwind_end,
                    module_object,
                    dynamic_entries,
                    dynamic,
                }));
            }
            dynamic_entries.push(DynamicEntry { tag, value });
        }
        Err("MOD0 dynamic table lacks DT_NULL within 4096 entries".into())
    }
}
