//! User-backed allocation metadata. The libnx map.c consumer selects flags
//! 0/1 by CPU cacheability; these bits are not guest memory access permissions.
use crate::{NvError, Result, long, word};
use wl_gpu::memory::GuestAddress;

pub(super) fn user_backing(input: &[u8], size: u64) -> Result<GuestAddress> {
    if input.len() != 32 {
        return Err(NvError::InvalidSize.into());
    }
    let flags = word(input, 8);
    let alignment = word(input, 12);
    if std::env::var("WL_NVDRV_TRACE").as_deref() == Ok("1") {
        eprintln!(
            "NVIDIA nvmap-user-backing heap_mask={:#x} cache_flags={flags:#x} alignment={alignment:#x} kind={} bytes={size}",
            word(input, 4),
            input[16]
        );
    }
    if word(input, 4) != 0 || flags > 1 || input[16..24] != [0; 8] {
        return Err(NvError::NotSupported.into());
    }
    if alignment < 4096 || !alignment.is_power_of_two() || size == 0 || size % 4096 != 0 {
        return Err(NvError::InvalidSize.into());
    }
    let address = long(input, 24);
    // The caller has already allocated the CPU storage. libnx validates only
    // CPU page alignment, independently of the requested GPU allocation align.
    if address == 0 || address % 4096 != 0 || address.checked_add(size).is_none() {
        return Err(NvError::InvalidAddress.into());
    }
    Ok(GuestAddress(address))
}

#[cfg(test)]
mod tests {
    use super::*;
    fn request() -> [u8; 32] {
        let mut input = [0; 32];
        input[12..16].copy_from_slice(&0x20000u32.to_le_bytes());
        input[24..].copy_from_slice(&0x21000u64.to_le_bytes());
        input
    }
    #[test]
    fn cacheability_and_gpu_alignment_do_not_change_cpu_permissions_or_address() {
        let mut input = request();
        for flags in [0u32, 1] {
            input[8..12].copy_from_slice(&flags.to_le_bytes());
            assert_eq!(user_backing(&input, 4096).unwrap(), GuestAddress(0x21000));
        }
    }
    #[test]
    fn unsupported_metadata_and_invalid_ranges_fail_before_pinning() {
        assert!(user_backing(&[0; 31], 4096).is_err());
        for (offset, value) in [
            (4, 1u32),
            (8, 2),
            (12, 0),
            (12, 3072),
            (12, 12288),
            (16, 1),
            (20, 1),
        ] {
            let mut input = request();
            input[offset..offset + 4].copy_from_slice(&value.to_le_bytes());
            assert!(user_backing(&input, 4096).is_err());
        }
        for address in [0u64, 0x21001, u64::MAX - 4095] {
            let mut input = request();
            input[24..].copy_from_slice(&address.to_le_bytes());
            assert!(user_backing(&input, 4096).is_err());
        }
        assert!(user_backing(&request(), 0).is_err());
        assert!(user_backing(&request(), 1).is_err());
    }
}
