//! Switch InitializeEx wire fields, following pinned libnx rather than the
//! differently ordered desktop nvgpu allocation structure. No page tables or
//! guest pointers are retained: mappings remain owned by GpuDriver.
use crate::{NvError, Result, long, word};

#[derive(Debug)]
pub(super) struct Configuration {
    big_page_size: u32,
    range: Option<(u64, u64, u64)>,
}
impl Configuration {
    pub(super) fn decode(input: &[u8]) -> Result<Self> {
        if input.len() != 40 {
            return Err(NvError::InvalidSize.into());
        }
        let flags = word(input, 0);
        let page = word(input, 8);
        let start = long(input, 16);
        let end = long(input, 24);
        let split = long(input, 32);
        if std::env::var("WL_NVDRV_TRACE").as_deref() == Ok("1") {
            eprintln!(
                "NVIDIA address-space flags={flags:#x} big_page_size={page:#x} range_start={start:#x} range_end={end:#x} range_split={split:#x}"
            );
        }
        // Existing contexts, unknown policy flags and reserved fields are not
        // interpreted as a request to share another address space.
        if flags > 1 || word(input, 4) != 0 || word(input, 12) != 0 {
            return Err(NvError::NotSupported.into());
        }
        let big_page_size = match page {
            0 => 0x20000,
            0x10000 | 0x20000 => page,
            _ => return Err(NvError::NotSupported.into()),
        };
        let range = if (start, end, split) == (0, 0, 0) {
            None
        } else {
            let alignment = u64::from(big_page_size);
            if start == 0
                || start >= split
                || split >= end
                || end > 1 << 40
                || [start, end, split].iter().any(|v| v % alignment != 0)
            {
                return Err(NvError::InvalidSize.into());
            }
            Some((start, end, split))
        };
        Ok(Self {
            big_page_size,
            range,
        })
    }
    pub(super) fn validate_range(&self, address: u64, size: u64, page: u32) -> Result<()> {
        if page != 4096 && page != self.big_page_size {
            return Err(NvError::NotSupported.into());
        }
        let end = address.checked_add(size).ok_or(NvError::InvalidSize)?;
        if size == 0
            || address == 0
            || end > 1 << 40
            || address % u64::from(page) != 0
            || size % u64::from(page) != 0
        {
            return Err(NvError::InvalidSize.into());
        }
        if let Some((start, limit, split)) = self.range {
            let (low, high) = if page == 4096 {
                (start, split)
            } else {
                (split, limit)
            };
            if address < low || end > high {
                return Err(NvError::InvalidAddress.into());
            }
        }
        Ok(())
    }

    /// The two regions are reported from the same bounds used for reservation
    /// checks. No invented default VA allocator is exposed to the guest.
    pub(super) fn regions(&self) -> Result<[u8; 48]> {
        let (start, end, split) = self.range.ok_or(NvError::NotSupported)?;
        let mut bytes = [0; 48];
        for (i, (base, limit, page)) in [(start, split, 4096u32), (split, end, self.big_page_size)]
            .into_iter()
            .enumerate()
        {
            let at = i * 24;
            bytes[at..at + 8].copy_from_slice(&base.to_le_bytes());
            bytes[at + 8..at + 12].copy_from_slice(&page.to_le_bytes());
            bytes[at + 16..at + 24]
                .copy_from_slice(&((limit - base) / u64::from(page)).to_le_bytes());
        }
        Ok(bytes)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn request() -> [u8; 40] {
        let mut b = [0; 40];
        for (offset, value) in [(0, 1u32), (8, 0x10000)] {
            b[offset..offset + 4].copy_from_slice(&value.to_le_bytes());
        }
        for (offset, value) in [(16, 0x4000000u64), (24, 0x2000000000), (32, 0x400000000)] {
            b[offset..offset + 8].copy_from_slice(&value.to_le_bytes());
        }
        b
    }
    #[test]
    fn explicit_wire_order_and_region_edges() {
        let c = Configuration::decode(&request()).unwrap();
        assert_eq!(c.big_page_size, 65536);
        for (a, n, p) in [
            (0x4000000, 4096, 4096),
            (0x3fffff000, 4096, 4096),
            (0x400000000, 65536, 65536),
            (0x1fffff0000, 65536, 65536),
        ] {
            c.validate_range(a, n, p).unwrap();
        }
        for (a, n, p) in [
            (0x3fff000, 4096, 4096),
            (0x3fffff000, 8192, 4096),
            (0x3ffff0000, 65536, 65536),
            (0x2000000000, 65536, 65536),
            (u64::MAX, 1, 4096),
            (0x4000000, 0, 4096),
            (0x4000001, 4096, 4096),
            (0x4000000, 4096, 65536),
        ] {
            assert!(c.validate_range(a, n, p).is_err());
        }
        assert!(c.validate_range(0x400000000, 0x20000, 0x20000).is_err());
    }
    #[test]
    fn malformed_configuration_is_rejected() {
        assert!(Configuration::decode(&[0; 39]).is_err());
        for (offset, value) in [(0, 2u32), (4, u32::MAX), (8, 4096), (12, 1)] {
            let mut b = request();
            b[offset..offset + 4].copy_from_slice(&value.to_le_bytes());
            assert!(Configuration::decode(&b).is_err());
        }
        for (offset, value) in [
            (16, 0u64),
            (16, 0x4000001),
            (24, 1u64 << 41),
            (32, 0x4000000),
            (32, 0x2000000000),
        ] {
            let mut b = request();
            b[offset..offset + 8].copy_from_slice(&value.to_le_bytes());
            assert!(Configuration::decode(&b).is_err());
        }
    }
    #[test]
    fn default_request_retains_existing_fixed_mapping_subset() {
        let c = Configuration::decode(&[0; 40]).unwrap();
        c.validate_range(0x10000, 4096, 4096).unwrap();
        c.validate_range(0x20000, 0x20000, 0x20000).unwrap();
        assert!(c.regions().is_err());
    }
    #[test]
    fn regions_describe_exactly_the_validated_ranges() {
        let b = Configuration::decode(&request())
            .unwrap()
            .regions()
            .unwrap();
        assert_eq!(long(&b, 0), 0x4000000);
        assert_eq!(word(&b, 8), 4096);
        assert_eq!(word(&b, 12), 0);
        assert_eq!(long(&b, 16), 0x3fc000);
        assert_eq!(long(&b, 24), 0x400000000);
        assert_eq!(word(&b, 32), 65536);
        assert_eq!(word(&b, 36), 0);
        assert_eq!(long(&b, 40), 0x1c0000);
    }
}
