//! Bounded Switch AS Remap entries, 20-byte little-endian inline array.
//! Reserved physical-offset word must be zero in this supported subset.
use crate::{NvError, Result, word};

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
            if !matches!(flags, 0 | 4) || !matches!(kind, 0 | 0xfe) || word(b, 8) != 0 {
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
            (0, 0x00010000),
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
}
