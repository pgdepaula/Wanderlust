use crate::error::NroError;
use serde::Serialize;

pub const MAGIC_ASET: [u8; 4] = *b"ASET";
pub const ASET_HEADER_SIZE: usize = 0x38;

/// Descriptor for an embedded asset within the ASET section.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
pub struct NroAssetSectionHeader {
    /// Byte offset relative to the start of the ASET header.
    pub offset: u64,
    /// Byte length of the asset section.
    pub size: u64,
}

impl NroAssetSectionHeader {
    pub fn is_present(&self) -> bool {
        self.size > 0
    }
}

/// Parsed ASET asset header (`0x00..0x38` of the asset trailer).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
pub struct NroAssetHeader {
    pub magic: [u8; 4],
    pub version: u32,
    pub icon: NroAssetSectionHeader,
    pub nacp: NroAssetSectionHeader,
    pub romfs: NroAssetSectionHeader,
}

/// Borrowed views into embedded NRO assets (icon, NACP, RomFS).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct NroAssets<'a> {
    pub header: NroAssetHeader,
    pub icon_bytes: Option<&'a [u8]>,
    pub nacp_bytes: Option<&'a [u8]>,
    pub romfs_bytes: Option<&'a [u8]>,
}

impl<'a> NroAssets<'a> {
    /// Parses and validates the ASET asset trailer from bytes located at or beyond `nro_size`.
    pub fn parse(aset_bytes: &'a [u8]) -> Result<Self, NroError> {
        if aset_bytes.len() < ASET_HEADER_SIZE {
            return Err(NroError::BufferTooSmall {
                required: ASET_HEADER_SIZE,
                actual: aset_bytes.len(),
            });
        }

        let magic: [u8; 4] = aset_bytes[0x00..0x04].try_into().unwrap();
        if magic != MAGIC_ASET {
            return Err(NroError::InvalidAssetMagic {
                expected: MAGIC_ASET,
                found: magic,
            });
        }

        let version = u32::from_le_bytes(aset_bytes[0x04..0x08].try_into().unwrap());

        let icon_offset = u64::from_le_bytes(aset_bytes[0x08..0x10].try_into().unwrap());
        let icon_size = u64::from_le_bytes(aset_bytes[0x10..0x18].try_into().unwrap());

        let nacp_offset = u64::from_le_bytes(aset_bytes[0x18..0x20].try_into().unwrap());
        let nacp_size = u64::from_le_bytes(aset_bytes[0x20..0x28].try_into().unwrap());

        let romfs_offset = u64::from_le_bytes(aset_bytes[0x28..0x30].try_into().unwrap());
        let romfs_size = u64::from_le_bytes(aset_bytes[0x30..0x38].try_into().unwrap());

        let icon = NroAssetSectionHeader {
            offset: icon_offset,
            size: icon_size,
        };
        let nacp = NroAssetSectionHeader {
            offset: nacp_offset,
            size: nacp_size,
        };
        let romfs = NroAssetSectionHeader {
            offset: romfs_offset,
            size: romfs_size,
        };

        let container_size = aset_bytes.len() as u64;

        // Validate each section extent
        validate_asset_extent("icon", &icon, container_size)?;
        validate_asset_extent("nacp", &nacp, container_size)?;
        validate_asset_extent("romfs", &romfs, container_size)?;

        // Validate that non-empty asset sections do not overlap
        check_asset_overlaps(&[("icon", &icon), ("nacp", &nacp), ("romfs", &romfs)])?;

        let icon_bytes = if icon.is_present() {
            Some(&aset_bytes[icon.offset as usize..(icon.offset + icon.size) as usize])
        } else {
            None
        };

        let nacp_bytes = if nacp.is_present() {
            Some(&aset_bytes[nacp.offset as usize..(nacp.offset + nacp.size) as usize])
        } else {
            None
        };

        let romfs_bytes = if romfs.is_present() {
            Some(&aset_bytes[romfs.offset as usize..(romfs.offset + romfs.size) as usize])
        } else {
            None
        };

        Ok(Self {
            header: NroAssetHeader {
                magic,
                version,
                icon,
                nacp,
                romfs,
            },
            icon_bytes,
            nacp_bytes,
            romfs_bytes,
        })
    }
}

fn validate_asset_extent(
    name: &'static str,
    sec: &NroAssetSectionHeader,
    container_size: u64,
) -> Result<(), NroError> {
    if sec.size == 0 {
        return Ok(());
    }
    if sec.offset < ASET_HEADER_SIZE as u64 {
        return Err(NroError::AssetOutOfBounds {
            asset: name,
            offset: sec.offset,
            size: sec.size,
            container_size,
        });
    }
    let end = sec
        .offset
        .checked_add(sec.size)
        .ok_or(NroError::ArithmeticOverflow(name))?;
    if end > container_size {
        return Err(NroError::AssetOutOfBounds {
            asset: name,
            offset: sec.offset,
            size: sec.size,
            container_size,
        });
    }
    Ok(())
}

fn check_asset_overlaps(
    sections: &[(&'static str, &NroAssetSectionHeader)],
) -> Result<(), NroError> {
    for i in 0..sections.len() {
        let (name_a, sec_a) = sections[i];
        if !sec_a.is_present() {
            continue;
        }
        let end_a = sec_a.offset + sec_a.size;

        for &(name_b, sec_b) in &sections[(i + 1)..] {
            if !sec_b.is_present() {
                continue;
            }
            let end_b = sec_b.offset + sec_b.size;

            if ranges_overlap_u64(sec_a.offset, end_a, sec_b.offset, end_b) {
                return Err(NroError::AssetOverlap {
                    first: name_a,
                    second: name_b,
                });
            }
        }
    }
    Ok(())
}

fn ranges_overlap_u64(start_a: u64, end_a: u64, start_b: u64, end_b: u64) -> bool {
    start_a < end_b && start_b < end_a
}
