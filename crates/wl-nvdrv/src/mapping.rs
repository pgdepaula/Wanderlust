//! MapBufferEx metadata, separate from GPU mapping ownership. libnx's address
//! space page field also accompanies whole buffers smaller than that page.
//! This adapter maps only actual 4 KiB backing extents, with the larger field
//! constraining automatic VA alignment; it does not expose physical GMMU pages.
use crate::{NvError, Result, long, word};

pub(super) struct Request {
    pub fixed: bool,
    pub handle: u32,
    pub offset: u64,
    pub address: u64,
    hint: u32,
    length: u64,
}
pub(super) struct Extent {
    /// Only these bytes have canonical backing and can be fetched by the GPU.
    pub resident: u64,
    /// Requested VA window, including inaccessible final-page padding.
    pub span: u64,
}
impl Request {
    pub fn small_page_kinds(&self) -> bool {
        self.hint == 65536
    }
    pub fn decode(input: &[u8]) -> Result<Self> {
        if input.len() != 40 {
            return Err(NvError::InvalidSize.into());
        }
        let flags = word(input, 0);
        let hint = word(input, 12);
        let kind = word(input, 4);
        // A verified 64 KiB page request cannot carry 128 KiB compression
        // tags. Apply the same explicit uncompressed-kind fallback as Remap;
        // an unspecified or 128 KiB page request cannot use this guarantee.
        let supported_kind = matches!(kind, 0 | 0xfe | u32::MAX)
            || (hint == 65536
                && u16::try_from(kind)
                    .ok()
                    .and_then(super::remap::uncompressed_kind)
                    .is_some());
        if flags & !5 != 0 || !supported_kind || !matches!(hint, 0 | 4096 | 65536 | 131072) {
            return Err(NvError::NotSupported.into());
        }
        let fixed = flags & 1 != 0;
        let address = long(input, 32);
        if !fixed && address != 0 {
            // Alignment in this final union is not implemented. The verified
            // libnx whole-buffer caller supplies zero and uses page_size.
            return Err(NvError::NotSupported.into());
        }
        Ok(Self {
            fixed,
            handle: word(input, 8),
            hint,
            address,
            offset: long(input, 16),
            length: long(input, 24),
        })
    }
    pub fn extent(&self, backing: u64) -> Result<Extent> {
        let available = backing
            .checked_sub(self.offset)
            .ok_or(NvError::InvalidSize)?;
        let span = if self.length == 0 {
            available
        } else {
            self.length
        };
        if span > available {
            let rounded = available.checked_add(65535).ok_or(NvError::InvalidSize)? & !65535;
            if !self.fixed || self.hint != 65536 || self.offset % 65536 != 0 || span != rounded {
                return Err(NvError::InvalidSize.into());
            }
        }
        let resident = span.min(available);
        if resident == 0 || resident % 4096 != 0 || span % 4096 != 0 || self.offset % 4096 != 0 {
            return Err(NvError::InvalidSize.into());
        }
        Ok(Extent { resident, span })
    }
    pub fn alignment(&self, allocation: u32) -> Result<u64> {
        let value = if self.hint == 0 {
            allocation
        } else {
            self.hint
        };
        if value < 4096 || !value.is_power_of_two() {
            return Err(NvError::InvalidSize.into());
        }
        Ok(u64::from(value))
    }
}

/// Switch MapBufferEx bit 8 changes the kind of an existing mapping, without
/// replacing its physical backing. Only aliases of already-uncompressed
/// storage are supported here; no compression tags are created.
pub(super) fn uncompressed_retag(input: &[u8], small_pages: bool) -> Result<(u64, usize)> {
    if input.len() != 40 {
        return Err(NvError::InvalidSize.into());
    }
    let kind = word(input, 4);
    let uncompressed = matches!(kind, 0 | 0xfe | u32::MAX)
        || (small_pages
            && u16::try_from(kind)
                .ok()
                .and_then(super::remap::uncompressed_kind)
                .is_some());
    if word(input, 0) != 0x100 || word(input, 8) != 0 || word(input, 12) != 0 || !uncompressed {
        return Err(NvError::NotSupported.into());
    }
    let start = long(input, 32)
        .checked_add(long(input, 16))
        .ok_or(NvError::InvalidSize)?;
    let size = long(input, 24);
    if size == 0
        || start % 4096 != 0
        || size % 4096 != 0
        || start.checked_add(size).is_none_or(|end| end > 1 << 40)
    {
        return Err(NvError::InvalidSize.into());
    }
    Ok((
        start,
        usize::try_from(size).map_err(|_| NvError::InvalidSize)?,
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn retag_checks_wire_fields_page_policy_and_address_arithmetic() {
        let mut input = [0; 40];
        input[..4].copy_from_slice(&0x100u32.to_le_bytes());
        input[4] = 0xdb;
        input[16..24].copy_from_slice(&4096u64.to_le_bytes());
        input[24..32].copy_from_slice(&8192u64.to_le_bytes());
        input[32..40].copy_from_slice(&0x400000000u64.to_le_bytes());
        assert_eq!(
            uncompressed_retag(&input, true).unwrap(),
            (0x400001000, 8192)
        );
        assert!(uncompressed_retag(&input, false).is_err());
        for (offset, value) in [(0, 0x104u32), (4, 0xd9), (4, 0xff), (8, 1), (12, 65536)] {
            let mut invalid = input;
            invalid[offset..offset + 4].copy_from_slice(&value.to_le_bytes());
            assert!(uncompressed_retag(&invalid, true).is_err());
        }
        for (offset, value) in [
            (16, u64::MAX),
            (16, 1),
            (24, 0),
            (24, u64::MAX),
            (32, 1 << 40),
        ] {
            let mut invalid = input;
            invalid[offset..offset + 8].copy_from_slice(&value.to_le_bytes());
            assert!(uncompressed_retag(&invalid, true).is_err());
        }
        assert!(uncompressed_retag(&input[..39], true).is_err());
        input[4] = 0xfe;
        assert_eq!(
            uncompressed_retag(&input, false).unwrap(),
            (0x400001000, 8192)
        );
        // GM20B Z16_2Z uses Z16 storage when compression is unavailable.
        input[4] = 7;
        assert_eq!(
            uncompressed_retag(&input, true).unwrap(),
            (0x400001000, 8192)
        );
        assert!(uncompressed_retag(&input, false).is_err());
    }
    #[test]
    fn compressed_kind_requires_explicit_small_page_fallback() {
        let mut input = [0; 40];
        input[0] = 4;
        input[4] = 0xdb;
        input[8..12].copy_from_slice(&185u32.to_le_bytes());
        input[12..16].copy_from_slice(&65536u32.to_le_bytes());
        let request = Request::decode(&input).unwrap();
        assert_eq!(request.handle, 185);
        assert_eq!(request.extent(0x20000).unwrap().resident, 0x20000);
        for page in [0u32, 4096, 131072] {
            input[12..16].copy_from_slice(&page.to_le_bytes());
            assert!(Request::decode(&input).is_err());
        }
        input[12..16].copy_from_slice(&65536u32.to_le_bytes());
        for kind in [0xebu32, 0xff, 0x100db] {
            input[4..8].copy_from_slice(&kind.to_le_bytes());
            assert!(Request::decode(&input).is_err());
        }
    }
    #[test]
    fn whole_buffer_hint_does_not_extend_canonical_backing() {
        let mut input = [0; 40];
        input[8] = 1;
        input[12..16].copy_from_slice(&65536u32.to_le_bytes());
        let request = Request::decode(&input).unwrap();
        assert!(!request.fixed);
        assert_eq!(request.extent(4096).unwrap().resident, 4096);
        assert_eq!(request.alignment(131072).unwrap(), 65536);
        input[4] = 0xfe; // libnx NvKind_Generic_16BX2, uncompressed.
        assert_eq!(
            Request::decode(&input)
                .unwrap()
                .extent(4096)
                .unwrap()
                .resident,
            4096
        );
        for (at, value) in [(0, 2u32), (4, 0xff), (12, 8192)] {
            let mut invalid = input;
            invalid[at..at + 4].copy_from_slice(&value.to_le_bytes());
            assert!(Request::decode(&invalid).is_err());
        }
        for (offset, length) in [
            (4096, 4096),
            (0, 8192),
            (1, 4096),
            (u64::MAX, 0),
            (0, u64::MAX),
        ] {
            let mut invalid = input;
            invalid[16..24].copy_from_slice(&offset.to_le_bytes());
            invalid[24..32].copy_from_slice(&length.to_le_bytes());
            assert!(Request::decode(&invalid).unwrap().extent(4096).is_err());
        }
        assert!(Request::decode(&input[..39]).is_err());
    }
    #[test]
    fn fixed_page_window_does_not_make_padding_canonical_backing() {
        let mut input = [0; 40];
        input[0] = 5;
        input[12..16].copy_from_slice(&65536u32.to_le_bytes());
        input[24..32].copy_from_slice(&0x30000u64.to_le_bytes());
        input[32..40].copy_from_slice(&0x400000000u64.to_le_bytes());
        let e = Request::decode(&input).unwrap().extent(0x26000).unwrap();
        assert_eq!((e.resident, e.span), (0x26000, 0x30000));
        for (backing, span) in [(0x26000, 0x40000u64), (0x26001, 0x30000), (0, 0x10000)] {
            input[24..32].copy_from_slice(&span.to_le_bytes());
            assert!(Request::decode(&input).unwrap().extent(backing).is_err());
        }
    }
}
