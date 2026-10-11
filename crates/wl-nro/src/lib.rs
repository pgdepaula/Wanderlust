//! Authoritative, zero-allocation NRO0 executable format ingestion and validation.
//!
//! NRO0 is the Nintendo Switch executable container format for non-ExeFS binaries and
//! homebrew applications. This crate provides safe, bounded parsing of NRO0 headers,
//! segment boundaries, MOD0 module pointers, optional ASET asset trailers, and
//! Homebrew ABI entry contracts.

pub mod abi;
pub mod asset;
pub mod error;
pub mod header;
pub mod image;
pub mod plan;

pub use abi::{
    EntryConvention, HOMEBREW_LOADER_SENTINEL, LoaderConfig, LoaderConfigEntry, LoaderConfigKey,
};
pub use asset::{ASET_HEADER_SIZE, MAGIC_ASET, NroAssetHeader, NroAssetSectionHeader, NroAssets};
pub use error::NroError;
pub use header::{
    EmbeddedInfo, MAGIC_MOD0, MAGIC_NRO, NRO_BASE_HEADER_SIZE, NRO_HEADER_SIZE, NRO_START_SIZE,
    NroFileOffset, NroHeader, NroSegmentHeader, NroStart,
};
pub use image::{NroGuestMapping, NroImage, NroSegment, PAGE_SIZE, SegmentKind};
pub use plan::{
    HomebrewImagePlan, HomebrewStartupRequirements, MappableRange, RelocationRequirement,
    ZeroFillRange,
};

use serde::Serialize;
use std::fmt;
use wl_common::{GuestAddr, ModuleOffset};

/// Top-level parsed NRO0 executable view borrowing the underlying bytes.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Nro<'a> {
    pub start: NroStart,
    pub header: NroHeader,
    pub raw_bytes: &'a [u8],
    pub assets: Option<NroAssets<'a>>,
}

impl<'a> Nro<'a> {
    /// Parses and rigorously validates an NRO0 binary from an in-memory buffer.
    pub fn parse(bytes: &'a [u8]) -> Result<Self, NroError> {
        if bytes.len() < NRO_BASE_HEADER_SIZE {
            return Err(NroError::BufferTooSmall {
                required: NRO_BASE_HEADER_SIZE,
                actual: bytes.len(),
            });
        }

        // 1. Parse NroStart (0x00..0x10) and NroHeader (0x10..0x80)
        let start = NroStart::parse(bytes)?;
        let header = NroHeader::parse(bytes)?;

        // 2. Validate version
        if header.version > 1 {
            return Err(NroError::UnsupportedVersion(header.version));
        }

        // 3. Validate declared file size
        if header.size < NRO_BASE_HEADER_SIZE as u32 {
            return Err(NroError::InvalidDeclaredSize {
                declared: header.size,
                file_size: bytes.len(),
            });
        }
        if bytes.len() < header.size as usize {
            return Err(NroError::InvalidDeclaredSize {
                declared: header.size,
                file_size: bytes.len(),
            });
        }

        let nro_container_size = header.size as usize;

        // 4. Validate segment extents within declared NRO size
        validate_segment_extent("text", &header.text, nro_container_size)?;
        validate_segment_extent("rodata", &header.ro, nro_container_size)?;
        validate_segment_extent("data", &header.data, nro_container_size)?;

        // 5. Alignment validation
        if header.text.size % 4 != 0 {
            return Err(NroError::AlignmentViolation {
                field: "text_size",
                offset: header.text.size,
                required: 4,
            });
        }
        if header.ro.file_offset.0 % 4 != 0 {
            return Err(NroError::AlignmentViolation {
                field: "ro_offset",
                offset: header.ro.file_offset.0,
                required: 4,
            });
        }
        if header.ro.size % 4 != 0 {
            return Err(NroError::AlignmentViolation {
                field: "ro_size",
                offset: header.ro.size,
                required: 4,
            });
        }
        if header.data.file_offset.0 % 4 != 0 {
            return Err(NroError::AlignmentViolation {
                field: "data_offset",
                offset: header.data.file_offset.0,
                required: 4,
            });
        }
        if header.data.size % 4 != 0 {
            return Err(NroError::AlignmentViolation {
                field: "data_size",
                offset: header.data.size,
                required: 4,
            });
        }
        if header.bss_size % 4 != 0 {
            return Err(NroError::AlignmentViolation {
                field: "bss_size",
                offset: header.bss_size,
                required: 4,
            });
        }

        // 6. Section non-overlap validation
        // In standard elf2nro output, text.file_offset is 0 (embedding the 0x80 header at the start of text).
        // RoData and Data must be disjoint from text and from each other.
        check_segment_overlaps(&[
            ("text", &header.text),
            ("rodata", &header.ro),
            ("data", &header.data),
        ])?;

        // 7. Validate MOD0 offset if present
        let mod0_off = start.mod0_offset.0 as usize;
        if mod0_off != 0 {
            if mod0_off + 4 > nro_container_size {
                return Err(NroError::InvalidMod0Offset {
                    offset: start.mod0_offset.0,
                    file_size: nro_container_size,
                });
            }
            let mod0_magic: [u8; 4] = bytes[mod0_off..mod0_off + 4].try_into().unwrap();
            if mod0_magic != MAGIC_MOD0 {
                return Err(NroError::MissingMod0Magic {
                    offset: start.mod0_offset.0,
                    found: mod0_magic,
                });
            }
        }

        // 8. Parse optional ASET asset section if present beyond declared NRO size
        let assets = if bytes.len() > nro_container_size {
            let trailer_slice = &bytes[nro_container_size..];
            if trailer_slice.len() >= ASET_HEADER_SIZE {
                let trailer_magic: [u8; 4] = trailer_slice[0..4].try_into().unwrap();
                if trailer_magic == MAGIC_ASET {
                    Some(NroAssets::parse(trailer_slice)?)
                } else {
                    return Err(NroError::InvalidAssetMagic {
                        expected: MAGIC_ASET,
                        found: trailer_magic,
                    });
                }
            } else {
                return Err(NroError::BufferTooSmall {
                    required: ASET_HEADER_SIZE,
                    actual: trailer_slice.len(),
                });
            }
        } else {
            None
        };

        Ok(Self {
            start,
            header,
            raw_bytes: bytes,
            assets,
        })
    }

    /// Borrows the executable text segment bytes.
    pub fn text_slice(&self) -> &'a [u8] {
        let off = self.header.text.file_offset.0 as usize;
        let sz = self.header.text.size as usize;
        &self.raw_bytes[off..off + sz]
    }

    /// Borrows the read-only data segment bytes.
    pub fn ro_slice(&self) -> &'a [u8] {
        let off = self.header.ro.file_offset.0 as usize;
        let sz = self.header.ro.size as usize;
        &self.raw_bytes[off..off + sz]
    }

    /// Borrows the writable data segment bytes.
    pub fn data_slice(&self) -> &'a [u8] {
        let off = self.header.data.file_offset.0 as usize;
        let sz = self.header.data.size as usize;
        &self.raw_bytes[off..off + sz]
    }

    /// Returns the optional parsed ASET asset section, if present.
    pub fn assets(&self) -> Option<&NroAssets<'a>> {
        self.assets.as_ref()
    }

    /// Returns hexadecimal representation of the 32-byte Module ID (GNU build ID).
    pub fn module_id_hex(&self) -> String {
        self.header
            .module_id
            .iter()
            .map(|b| format!("{b:02x}"))
            .collect()
    }

    /// Produces a structured `NroImage` representation for guest memory layout and AOT recompilation.
    pub fn to_image(&self) -> Result<NroImage<'a>, NroError> {
        let text_mem_off = ModuleOffset(0);
        let ro_mem_off = ModuleOffset(self.header.ro.file_offset.0 as u64);
        let data_mem_off = ModuleOffset(self.header.data.file_offset.0 as u64);
        let bss_mem_off = ModuleOffset(data_mem_off.0 + self.header.data.size as u64);

        let data_end = bss_mem_off
            .0
            .checked_add(self.header.bss_size as u64)
            .ok_or(NroError::ArithmeticOverflow("bss_end"))?;

        let total_memory_size = align_up_u64(data_end, PAGE_SIZE)
            .ok_or(NroError::ArithmeticOverflow("total_memory_size"))?;

        let text = NroSegment {
            name: "text",
            kind: SegmentKind::Text,
            file_offset: self.header.text.file_offset,
            memory_offset: text_mem_off,
            file_size: self.header.text.size,
            memory_size: self.header.text.size,
            readable: true,
            writable: false,
            executable: true,
            bytes: self.text_slice(),
        };

        let rodata = NroSegment {
            name: "rodata",
            kind: SegmentKind::RoData,
            file_offset: self.header.ro.file_offset,
            memory_offset: ro_mem_off,
            file_size: self.header.ro.size,
            memory_size: self.header.ro.size,
            readable: true,
            writable: false,
            executable: false,
            bytes: self.ro_slice(),
        };

        let data = NroSegment {
            name: "data",
            kind: SegmentKind::Data,
            file_offset: self.header.data.file_offset,
            memory_offset: data_mem_off,
            file_size: self.header.data.size,
            memory_size: self.header.data.size,
            readable: true,
            writable: true,
            executable: false,
            bytes: self.data_slice(),
        };

        Ok(NroImage {
            text,
            rodata,
            data,
            bss_offset: bss_mem_off,
            bss_size: self.header.bss_size as u64,
            total_memory_size,
            module_id: self.header.module_id,
            module_id_hex: self.module_id_hex(),
            entrypoint_offset: ModuleOffset(0),
            entry_convention: EntryConvention::default_homebrew(GuestAddr(0)),
        })
    }

    /// Performs preflight inspection and returns a diagnostic summary.
    pub fn inspect(&self) -> NroInspectReport {
        let (icon_size, nacp_size, romfs_size) = match &self.assets {
            Some(a) => (a.header.icon.size, a.header.nacp.size, a.header.romfs.size),
            None => (0, 0, 0),
        };

        let data_end = (self.header.data.file_offset.0 as u64)
            + (self.header.data.size as u64)
            + (self.header.bss_size as u64);
        let total_memory_size = align_up_u64(data_end, PAGE_SIZE).unwrap_or(0);

        NroInspectReport {
            magic: "NRO0".to_string(),
            version: self.header.version,
            nro_size: self.header.size,
            file_size: self.raw_bytes.len(),
            text_offset: self.header.text.file_offset.0,
            text_size: self.header.text.size,
            ro_offset: self.header.ro.file_offset.0,
            ro_size: self.header.ro.size,
            data_offset: self.header.data.file_offset.0,
            data_size: self.header.data.size,
            bss_size: self.header.bss_size,
            total_memory_size,
            module_id: self.module_id_hex(),
            mod0_offset: self.start.mod0_offset.0,
            has_mod0: self.start.mod0_offset.0 != 0,
            has_aset: self.assets.is_some(),
            icon_size,
            nacp_size,
            romfs_size,
            entry_convention: "Homebrew ABI (X0=loader_config, X1=0xFFFFFFFFFFFFFFFF, returnable LR)",
        }
    }
}

fn validate_segment_extent(
    name: &'static str,
    seg: &NroSegmentHeader,
    container_size: usize,
) -> Result<(), NroError> {
    if seg.size == 0 {
        return Ok(());
    }
    let end = seg
        .file_offset
        .0
        .checked_add(seg.size)
        .ok_or(NroError::ArithmeticOverflow(name))?;
    if end as usize > container_size {
        return Err(NroError::SectionOutOfBounds {
            section: name,
            offset: seg.file_offset.0,
            size: seg.size,
            container_size,
        });
    }
    Ok(())
}

fn check_segment_overlaps(segments: &[(&'static str, &NroSegmentHeader)]) -> Result<(), NroError> {
    for i in 0..segments.len() {
        let (name_a, seg_a) = segments[i];
        if seg_a.size == 0 {
            continue;
        }
        let end_a = seg_a.file_offset.0 + seg_a.size;

        for &(name_b, seg_b) in &segments[(i + 1)..] {
            if seg_b.size == 0 {
                continue;
            }
            let end_b = seg_b.file_offset.0 + seg_b.size;

            if ranges_overlap_u32(seg_a.file_offset.0, end_a, seg_b.file_offset.0, end_b) {
                return Err(NroError::SectionOverlap {
                    first: name_a,
                    second: name_b,
                });
            }
        }
    }
    Ok(())
}

fn ranges_overlap_u32(start_a: u32, end_a: u32, start_b: u32, end_b: u32) -> bool {
    start_a < end_b && start_b < end_a
}

fn align_up_u64(value: u64, align: u64) -> Option<u64> {
    let remainder = value % align;
    if remainder == 0 {
        Some(value)
    } else {
        value.checked_add(align - remainder)
    }
}

/// Diagnostic inspection summary of an NRO executable.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct NroInspectReport {
    pub magic: String,
    pub version: u32,
    pub nro_size: u32,
    pub file_size: usize,
    pub text_offset: u32,
    pub text_size: u32,
    pub ro_offset: u32,
    pub ro_size: u32,
    pub data_offset: u32,
    pub data_size: u32,
    pub bss_size: u32,
    pub total_memory_size: u64,
    pub module_id: String,
    pub mod0_offset: u32,
    pub has_mod0: bool,
    pub has_aset: bool,
    pub icon_size: u64,
    pub nacp_size: u64,
    pub romfs_size: u64,
    pub entry_convention: &'static str,
}

impl fmt::Display for NroInspectReport {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        writeln!(f, "NRO Format: {}", self.magic)?;
        writeln!(f, "Format Version: {}", self.version)?;
        writeln!(
            f,
            "NRO Declared Size: {:#x} ({} bytes)",
            self.nro_size, self.nro_size
        )?;
        writeln!(
            f,
            "Total File Size: {:#x} ({} bytes)",
            self.file_size, self.file_size
        )?;
        writeln!(f, "Module ID: {}", self.module_id)?;
        writeln!(f, "Entry Convention: {}", self.entry_convention)?;
        writeln!(
            f,
            "Text Segment: file {:#x}..{:#x} (size {:#x})",
            self.text_offset,
            self.text_offset + self.text_size,
            self.text_size
        )?;
        writeln!(
            f,
            "RoData Segment: file {:#x}..{:#x} (size {:#x})",
            self.ro_offset,
            self.ro_offset + self.ro_size,
            self.ro_size
        )?;
        writeln!(
            f,
            "Data Segment: file {:#x}..{:#x} (size {:#x})",
            self.data_offset,
            self.data_offset + self.data_size,
            self.data_size
        )?;
        writeln!(
            f,
            "BSS Size: {:#x} ({} bytes)",
            self.bss_size, self.bss_size
        )?;
        writeln!(
            f,
            "Total Mapped Memory: {:#x} ({} bytes)",
            self.total_memory_size, self.total_memory_size
        )?;
        if self.has_mod0 {
            writeln!(
                f,
                "MOD0 Dynamic Header: present at offset {:#x}",
                self.mod0_offset
            )?;
        } else {
            writeln!(f, "MOD0 Dynamic Header: none")?;
        }
        if self.has_aset {
            writeln!(
                f,
                "ASET Assets: present (Icon: {} B, NACP: {} B, RomFS: {} B)",
                self.icon_size, self.nacp_size, self.romfs_size
            )?;
        } else {
            writeln!(f, "ASET Assets: none")?;
        }
        Ok(())
    }
}
