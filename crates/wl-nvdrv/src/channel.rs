//! Switch channel allocation wire formats, libnx feebd026ca0f5dcc2119f46ad8e0d16ad3dd4973.
//! Queue capacity is distinct from the bounded number of entries executed per call.
use crate::{NvError, Result, word};

pub(super) const ALLOC_GPFIFO_EX2: u32 = 0xc020481a;

pub(super) fn capacity(request: u32, input: &[u8]) -> Result<u32> {
    let limit = match request {
        0x40084805 if input.len() == 8 && word(input, 4) == 0 => 2048,
        ALLOC_GPFIFO_EX2
            if input.len() == 32
                && word(input, 4) == 1
                && [8, 20, 24, 28].into_iter().all(|at| word(input, at) == 0) =>
        {
            32768
        }
        _ => return Err(NvError::NotSupported.into()),
    };
    let count = word(input, 0);
    if count == 0 || count > limit {
        return Err(NvError::InvalidSize.into());
    }
    // No command payload is allocated here. Output fence bytes 12..20 in Ex2
    // are deliberately not inputs. Unknown allocation modes remain unsupported.
    Ok(count)
}

/// Switch uses the legacy `mem` word as a nonzero enable switch. Offset,
/// size and reserved are ignored, not pointers to map or buffer extents.
/// https://switchbrew.org/wiki/NV_services#NVGPU_IOCTL_CHANNEL_SET_ERROR_NOTIFIER
pub(super) fn notifier_enabled(input: &[u8]) -> Result<bool> {
    if input.len() != 24 {
        return Err(NvError::InvalidSize.into());
    }
    Ok(word(input, 16) != 0)
}

#[cfg(test)]
mod tests {
    use super::*;
    fn input() -> [u8; 32] {
        let mut b = [0; 32];
        b[..4].copy_from_slice(&32768u32.to_le_bytes());
        b[4..8].copy_from_slice(&1u32.to_le_bytes());
        b
    }
    #[test]
    fn ex2_fields_and_capacity_are_not_submission_work_or_input_fences() {
        let mut b = input();
        b[12..20].fill(0xff);
        assert_eq!(capacity(ALLOC_GPFIFO_EX2, &b).unwrap(), 32768);
        for at in [4, 8, 20, 24, 28] {
            let mut invalid = b;
            invalid[at] = 2;
            assert!(capacity(ALLOC_GPFIFO_EX2, &invalid).is_err());
        }
        for count in [0u32, 32769, u32::MAX] {
            b[..4].copy_from_slice(&count.to_le_bytes());
            assert!(capacity(ALLOC_GPFIFO_EX2, &b).is_err());
        }
        for size in 0..32 {
            assert!(capacity(ALLOC_GPFIFO_EX2, &input()[..size]).is_err());
        }
        assert!(capacity(0x40084805, &input()).is_err());
        assert_eq!(capacity(0x40084805, &[1, 0, 0, 0, 0, 0, 0, 0]).unwrap(), 1);
    }
}

#[cfg(test)]
mod notifier_tests {
    use super::*;
    #[test]
    fn legacy_fields_are_ignored_and_every_nonzero_mem_enables_notification() {
        let mut input = [0xff; 24];
        for value in [0u32, 1, 2, 4, u32::MAX] {
            input[16..20].copy_from_slice(&value.to_le_bytes());
            assert_eq!(notifier_enabled(&input).unwrap(), value != 0);
        }
        for length in 0..24 {
            assert!(notifier_enabled(&input[..length]).is_err());
        }
        assert!(notifier_enabled(&[0; 25]).is_err());
    }
}
