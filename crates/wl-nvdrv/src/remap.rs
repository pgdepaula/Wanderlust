//! Bounded Switch AS Remap entries, 20-byte little-endian inline array.
//! Reserved physical-offset word must be zero in this supported subset.
use crate::{NvError, Result, word};

// Remap uses 64 KiB pages. NVIDIA's kind/compression setup downgrades these
// kinds to their uncompressed storage when pages are below the 128 KiB compression
// granularity. No compressed payload or compression tags are imported here.
fn uncompressed_kind(kind: u16) -> Option<u16> {
    // PTE storage identity is distinct from B197/TIC format and sample count.
    // Registering an alias does not enable raster formats or multisampling.
    match kind {
        0 | 0xfe | 1 | 0x46 | 0x7b | 0xc3 => Some(kind),
        7..=10 => Some(1),         // Z16_2Z and MS2/MS4/MS8 storage aliases.
        0x51..=0x54 => Some(0x46), // Z24S8_2CZ.
        0x86..=0x89 => Some(0x7b), // ZF32_2CZ.
        0xce..=0xd1 => Some(0xc3), // ZF32_X24S8_2CSZV.
        0xcc..=0xcd
        | 0xd8
        | 0xda..=0xdc
        | 0xe0
        | 0xe5..=0xea
        | 0xee
        | 0xf3..=0xf5
        | 0xf7
        | 0xf9
        | 0xfb => Some(0xfe),
        _ => None,
    }
}

pub(super) struct Entry {
    pub handle: u32,
    pub address: u64,
    pub size: u64,
}
pub(super) fn decode(bytes: &[u8]) -> Result<Vec<Entry>> {
    if bytes.is_empty() || bytes.len() % 20 != 0 {
        return Err(NvError::InvalidSize.into());
    }
    if bytes.len() / 20 > wl_gpu::driver::MAX_OBJECTS {
        return Err(NvError::ResourceError.into());
    }
    bytes
        .chunks_exact(20)
        .map(|b| {
            let flags = u16::from_le_bytes(b[..2].try_into().unwrap());
            let kind = u16::from_le_bytes(b[2..4].try_into().unwrap());
            if !matches!(flags, 0 | 4) || uncompressed_kind(kind).is_none() || word(b, 8) != 0 {
                return Err(NvError::NotSupported.into());
            }
            let address = u64::from(word(b, 12)) << 16;
            let size = u64::from(word(b, 16)) << 16;
            if size == 0 || address == 0 || address.checked_add(size).is_none_or(|v| v > 1 << 40) {
                return Err(NvError::InvalidSize.into());
            }
            Ok(Entry {
                handle: word(b, 4),
                address,
                size,
            })
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    fn wire() -> Vec<u8> {
        [0x00fe0004u32, 19, 0, 0x40000, 7]
            .into_iter()
            .flat_map(u32::to_le_bytes)
            .collect()
    }
    #[test]
    fn literal_entry_fields_are_not_byte_offsets_or_host_words() {
        let e = decode(&wire()).unwrap();
        assert_eq!(e.len(), 1);
        assert_eq!(
            (e[0].handle, e[0].address, e[0].size),
            (19, 0x400000000, 7 * 65536)
        );
    }
    #[test]
    fn unsupported_forms_and_overflowing_gpu_extents_are_rejected() {
        for n in [0, 1, 19, 21, 39] {
            assert!(decode(&vec![0; n]).is_err());
        }
        for (at, value) in [
            (0, 0x00fe0001),
            (0, 0x00ff0000),
            (8, 1),
            (12, 0),
            (12, 0x1000000),
            (12, u32::MAX),
            (16, 0),
            (16, u32::MAX),
        ] {
            let mut b = wire();
            b[at..at + 4].copy_from_slice(&value.to_le_bytes());
            assert!(decode(&b).is_err());
        }
        assert!(decode(&wire().repeat(wl_gpu::driver::MAX_OBJECTS + 1)).is_err());
    }
    #[test]
    fn small_page_compression_fallback_is_an_explicit_kind_subset() {
        for kind in [
            0, 0xfe, 0xd8, 0xda, 0xdb, 0xdc, 0xe6, 0xe7, 0xe8, 0xe9, 0xea, 0xf4, 0xf5,
        ] {
            let mut b = wire();
            b[2..4].copy_from_slice(&(kind as u16).to_le_bytes());
            assert!(decode(&b).is_ok());
        }
        for (kind, expected) in [
            (7, 1),
            (0x51, 0x46),
            (0x86, 0x7b),
            (0xce, 0xc3),
            (0xcc, 0xfe),
            (0xf7, 0xfe),
            (0xf9, 0xfe),
            (0xfb, 0xfe),
        ] {
            assert_eq!(uncompressed_kind(kind), Some(expected));
            let mut b = wire();
            b[2..4].copy_from_slice(&kind.to_le_bytes());
            assert!(decode(&b).is_ok());
        }
        for kind in [0xd9, 0xdd, 0xeb, 0xf6, 0xff, 0xffff] {
            let mut b = wire();
            b[2..4].copy_from_slice(&(kind as u16).to_le_bytes());
            assert!(decode(&b).is_err());
        }
    }
}
