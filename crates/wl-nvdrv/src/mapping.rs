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
impl Request {
    pub fn decode(input: &[u8]) -> Result<Self> {
        if input.len() != 40 {
            return Err(NvError::InvalidSize.into());
        }
        let flags = word(input, 0);
        let hint = word(input, 12);
        if flags & !5 != 0
            || !matches!(word(input, 4), 0 | u32::MAX)
            || !matches!(hint, 0 | 4096 | 65536 | 131072)
        {
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
    pub fn size(&self, backing: u64) -> Result<u64> {
        let available = backing
            .checked_sub(self.offset)
            .ok_or(NvError::InvalidSize)?;
        let size = if self.length == 0 {
            available
        } else {
            self.length
        };
        if size == 0 || size > available || size % 4096 != 0 || self.offset % 4096 != 0 {
            return Err(NvError::InvalidSize.into());
        }
        Ok(size)
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

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn whole_buffer_hint_does_not_extend_canonical_backing() {
        let mut input = [0; 40];
        input[8] = 1;
        input[12..16].copy_from_slice(&65536u32.to_le_bytes());
        let request = Request::decode(&input).unwrap();
        assert!(!request.fixed);
        assert_eq!(request.size(4096).unwrap(), 4096);
        assert_eq!(request.alignment(131072).unwrap(), 65536);
        for (at, value) in [(0, 2u32), (4, 1), (12, 8192)] {
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
            assert!(Request::decode(&invalid).unwrap().size(4096).is_err());
        }
        assert!(Request::decode(&input[..39]).is_err());
    }
}
