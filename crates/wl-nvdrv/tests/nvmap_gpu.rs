//! Nvmap wire ABI and shared allocation lifetime through the public device API.
use wl_gpu::memory::{GuestAddress, GuestMemory, GuestMemoryWrite};
use wl_nvdrv::{Driver, Error, Ioctl, IoctlKind, Memory, NvError};

struct Backing {
    bytes: [u8; 4096],
    pins: usize,
    unpins: usize,
    fail_unpin: bool,
}
impl Default for Backing {
    fn default() -> Self {
        Self {
            bytes: [0; 4096],
            pins: 0,
            unpins: 0,
            fail_unpin: false,
        }
    }
}
impl GuestMemory for Backing {
    fn read(&self, address: GuestAddress, length: usize) -> wl_gpu::Result<&[u8]> {
        let start = address
            .0
            .checked_sub(0x21000)
            .and_then(|v| usize::try_from(v).ok())
            .ok_or_else(|| wl_gpu::Error("invalid backing address".into()))?;
        let end = start
            .checked_add(length)
            .ok_or_else(|| wl_gpu::Error("backing range overflow".into()))?;
        self.bytes
            .get(start..end)
            .ok_or_else(|| wl_gpu::Error("invalid backing range".into()))
    }
}
impl GuestMemoryWrite for Backing {
    fn validate_write(&self, address: GuestAddress, length: usize) -> wl_gpu::Result<()> {
        self.read(address, length).map(|_| ())
    }
    fn write(&mut self, _: GuestAddress, _: &[u8]) -> wl_gpu::Result<()> {
        panic!("allocation identity operations must not publish guest memory")
    }
}
impl Memory for Backing {
    type Pin = u32;
    fn pin(&mut self, address: GuestAddress, size: u64) -> wl_gpu::Result<u32> {
        self.read(address, size as usize)?;
        self.pins += 1;
        Ok(37)
    }
    fn unpin(&mut self, pin: u32) -> wl_gpu::Result<()> {
        assert_eq!(pin, 37);
        if self.fail_unpin {
            return Err(wl_gpu::Error("injected pin release failure".into()));
        }
        self.unpins += 1;
        Ok(())
    }
}

#[test]
#[ignore = "checks actual GPU registration and runtime pin lifetime"]
fn nvmap_free_retains_imports_and_retries_without_losing_backing() {
    let mut d = Driver::new().unwrap();
    let mut m = Backing::default();
    let fd = d.open("/dev/nvmap").unwrap();
    let counters = d.graphics_counters();
    let mut created = [0; 8];
    call(
        &mut d,
        &mut m,
        fd,
        0xc0080101,
        &words(&[4096, 0]),
        &mut created,
    )
    .unwrap();
    let handle = u32::from_le_bytes(created[4..].try_into().unwrap());
    let mut allocation = words(&[handle, 0, 0, 131072, 0, 0]);
    allocation.extend(0x21000u64.to_le_bytes());
    call(&mut d, &mut m, fd, 0xc0200104, &allocation, &mut [0; 32]).unwrap();
    call(
        &mut d,
        &mut m,
        fd,
        0xc0080103,
        &words(&[handle, 0]),
        &mut [0; 8],
    )
    .unwrap();
    let request = words(&[handle, 0x12345678, u32::MAX, u32::MAX, u32::MAX, u32::MAX]);
    let mut result = [0xcc; 24];
    call(&mut d, &mut m, fd, 0xc0180105, &request, &mut result).unwrap();
    assert_eq!(&result[8..16], &1u64.to_le_bytes());
    assert_eq!(&result[16..20], &4096u32.to_le_bytes());
    assert_eq!(&result[20..], &1u32.to_le_bytes());
    assert_eq!((m.pins, m.unpins), (1, 0));
    let space = d.open("/dev/nvhost-as-gpu").unwrap();
    let mut init = words(&[1, 0, 65536, 0]);
    for address in [0x4000000u64, 0x2000000000, 0x400000000] {
        init.extend(address.to_le_bytes());
    }
    call(&mut d, &mut m, space, 0x40284109, &init, &mut []).unwrap();
    let mut reserve = words(&[1, 65536, 1, 0]);
    reserve.extend(0x400000000u64.to_le_bytes());
    call(&mut d, &mut m, space, 0xc0184102, &reserve, &mut [0; 24]).unwrap();
    // Only 4 KiB can be borrowed from Backing. The padded 64 KiB VA window
    // must not extend that backing registration or require an oversized pin.
    // C32_2CRA (0xdb) falls back to generic uncompressed storage below the
    // 128 KiB compression granularity. This must still pin only real bytes.
    let mut map = words(&[5, 0xdb, handle, 65536]);
    map.extend(0u64.to_le_bytes());
    map.extend(65536u64.to_le_bytes());
    map.extend(0x400000000u64.to_le_bytes());
    let mut invalid = map.clone();
    invalid[12..16].copy_from_slice(&131072u32.to_le_bytes());
    let mut unchanged = [0xcc; 40];
    error(
        call(&mut d, &mut m, space, 0xc0284106, &invalid, &mut unchanged),
        NvError::NotSupported,
    );
    assert_eq!(unchanged, [0xcc; 40]);
    call(&mut d, &mut m, space, 0xc0284106, &map, &mut [0; 40]).unwrap();
    let mut retag = words(&[0x100, 0xdb, 0, 0]);
    retag.extend(0u64.to_le_bytes());
    retag.extend(4096u64.to_le_bytes());
    retag.extend(0x400000000u64.to_le_bytes());
    let mut retagged = [0; 40];
    call(&mut d, &mut m, space, 0xc0284106, &retag, &mut retagged).unwrap();
    assert_eq!(&retagged, retag.as_slice());
    retag[4..8].copy_from_slice(&7u32.to_le_bytes());
    call(&mut d, &mut m, space, 0xc0284106, &retag, &mut retagged).unwrap();
    assert_eq!(&retagged, retag.as_slice());
    // The reserved tail is not backing: changing its kind must not make it
    // resident. Failed changes preserve the live mapping and wire output.
    let mut padded = retag.clone();
    padded[24..32].copy_from_slice(&65536u64.to_le_bytes());
    unchanged.fill(0xcc);
    error(
        call(&mut d, &mut m, space, 0xc0284106, &padded, &mut unchanged),
        NvError::InvalidAddress,
    );
    assert_eq!(unchanged, [0xcc; 40]);
    call(&mut d, &mut m, space, 0xc0284106, &retag, &mut retagged).unwrap();
    result.fill(0xcc);
    error(
        call(&mut d, &mut m, fd, 0xc0180105, &request, &mut result),
        NvError::InvalidState,
    );
    assert_eq!(result, [0xcc; 24]);
    assert_eq!((m.pins, m.unpins), (1, 0));
    call(
        &mut d,
        &mut m,
        space,
        0xc0084105,
        &0x400000000u64.to_le_bytes(),
        &mut [0; 8],
    )
    .unwrap();
    let mut free = 0x400000000u64.to_le_bytes().to_vec();
    unchanged.fill(0xcc);
    error(
        call(&mut d, &mut m, space, 0xc0284106, &retag, &mut unchanged),
        NvError::InvalidAddress,
    );
    assert_eq!(unchanged, [0xcc; 40]);
    free.extend(words(&[1, 65536]));
    call(&mut d, &mut m, space, 0xc0104103, &free, &mut [0; 16]).unwrap();
    d.close(space, &mut m).unwrap();
    m.fail_unpin = true;
    result.fill(0xcc);
    error(
        call(&mut d, &mut m, fd, 0xc0180105, &request, &mut result),
        NvError::InvalidState,
    );
    assert_eq!(result, [0xcc; 24]);
    assert_eq!((m.pins, m.unpins), (1, 0));
    call(
        &mut d,
        &mut m,
        fd,
        0xc008010e,
        &words(&[0, handle]),
        &mut [0; 8],
    )
    .unwrap();
    m.fail_unpin = false;
    call(&mut d, &mut m, fd, 0xc0180105, &request, &mut result).unwrap();
    assert_eq!(&result[8..16], &[0; 8]);
    assert_eq!(&result[16..20], &4096u32.to_le_bytes());
    assert_eq!(&result[20..], &[0; 4]);
    assert_eq!((m.pins, m.unpins), (1, 1));
    result.fill(0xcc);
    error(
        call(&mut d, &mut m, fd, 0xc0180105, &request, &mut result),
        NvError::BadParameter,
    );
    assert_eq!(result, [0xcc; 24]);
    // Free unallocated objects repeatedly: released slots cannot exhaust the
    // bounded allocation registry or reissue a stale handle.
    for _ in 0..128 {
        call(
            &mut d,
            &mut m,
            fd,
            0xc0080101,
            &words(&[4096, 0]),
            &mut created,
        )
        .unwrap();
        let next = u32::from_le_bytes(created[4..].try_into().unwrap());
        assert!(next > handle);
        call(
            &mut d,
            &mut m,
            fd,
            0xc0180105,
            &words(&[next, 0, 0, 0, 0, 0]),
            &mut result,
        )
        .unwrap();
    }
    d.close(fd, &mut m).unwrap();
    assert_eq!((m.pins, m.unpins), (1, 1));
    assert_eq!(d.graphics_counters(), counters);
}
fn call(
    d: &mut Driver<u32>,
    m: &mut Backing,
    fd: u32,
    request: u32,
    input: &[u8],
    output: &mut [u8],
) -> wl_nvdrv::Result<()> {
    d.ioctl(
        Ioctl {
            fd,
            request,
            kind: IoctlKind::Ioctl,
            input,
            output,
            extra_input: &[],
            extra_output: &mut [],
        },
        m,
    )
}
fn words(values: &[u32]) -> Vec<u8> {
    values.iter().flat_map(|v| v.to_le_bytes()).collect()
}
#[test]
#[ignore = "initializes the GPU driver for bounded handle-registry validation"]
fn live_allocation_capacity_rejects_without_replacing_existing_handles() {
    let mut d = Driver::new().unwrap();
    let mut m = Backing::default();
    let fd = d.open("/dev/nvmap").unwrap();
    let counters = d.graphics_counters();
    let mut handles = Vec::new();
    let mut created = [0; 8];
    for _ in 0..1024 {
        call(
            &mut d,
            &mut m,
            fd,
            0xc0080101,
            &words(&[4096, 0]),
            &mut created,
        )
        .unwrap();
        handles.push(u32::from_le_bytes(created[4..].try_into().unwrap()));
    }
    created.fill(0xcc);
    error(
        call(
            &mut d,
            &mut m,
            fd,
            0xc0080101,
            &words(&[4096, 0]),
            &mut created,
        ),
        NvError::ResourceError,
    );
    assert_eq!(created, [0xcc; 8]);
    call(
        &mut d,
        &mut m,
        fd,
        0xc0180105,
        &words(&[handles[0], 0, 0, 0, 0, 0]),
        &mut [0; 24],
    )
    .unwrap();
    call(
        &mut d,
        &mut m,
        fd,
        0xc0080101,
        &words(&[4096, 0]),
        &mut created,
    )
    .unwrap();
    assert!(u32::from_le_bytes(created[4..].try_into().unwrap()) > *handles.last().unwrap());
    for handle in &handles[1..] {
        call(
            &mut d,
            &mut m,
            fd,
            0xc008010e,
            &words(&[0, *handle]),
            &mut created,
        )
        .unwrap();
        assert_eq!(&created[..4], &handle.to_le_bytes());
    }
    d.close(fd, &mut m).unwrap();
    assert_eq!((m.pins, m.unpins), (0, 0));
    assert_eq!(d.graphics_counters(), counters);
}
fn error(result: wl_nvdrv::Result<()>, expected: NvError) {
    assert!(matches!(result, Err(Error::Driver(e)) if e == expected));
}
#[test]
#[ignore = "initializes an actual wgpu device; execute explicitly"]
fn nvmap_shared_id_retains_one_backing_and_rejects_stale_ids() {
    let mut d = Driver::new().unwrap();
    let mut m = Backing::default();
    let fd = d.open("/dev/nvmap").unwrap();
    let counters = d.graphics_counters();
    let mut output = [0xcc; 8];
    call(
        &mut d,
        &mut m,
        fd,
        0xc0080101,
        &words(&[4096, 0xfeedface]),
        &mut output,
    )
    .unwrap();
    let handle = u32::from_le_bytes(output[4..8].try_into().unwrap());
    // Literal libnx ABI: GetId outputs word zero and consumes word one.
    call(
        &mut d,
        &mut m,
        fd,
        0xc008010e,
        &words(&[0xdeadbeef, handle]),
        &mut output,
    )
    .unwrap();
    let id = u32::from_le_bytes(output[..4].try_into().unwrap());
    assert_ne!(id, 0);
    assert_eq!(&output[4..], &handle.to_le_bytes());
    output.fill(0xcc);
    error(
        call(
            &mut d,
            &mut m,
            fd,
            0xc0080103,
            &words(&[id, 0]),
            &mut output,
        ),
        NvError::InvalidState,
    );
    assert_eq!(output, [0xcc; 8]);
    let mut allocation = words(&[handle, 0, 0, 4096, 0, 0]);
    allocation.extend(0x21000u64.to_le_bytes());
    let mut allocated = [0xcc; 32];
    call(&mut d, &mut m, fd, 0xc0200104, &allocation, &mut allocated).unwrap();
    for _ in 0..128 {
        call(
            &mut d,
            &mut m,
            fd,
            0xc0080103,
            &words(&[id, 0xdeadbeef]),
            &mut output,
        )
        .unwrap();
        assert_eq!(&output[..4], &id.to_le_bytes());
        assert_eq!(&output[4..], &handle.to_le_bytes());
    }
    assert_eq!((m.pins, m.unpins), (1, 0));
    let mut parameter = [0xcc; 12];
    call(
        &mut d,
        &mut m,
        fd,
        0xc00c0109,
        &words(&[handle, 1, 0]),
        &mut parameter,
    )
    .unwrap();
    assert_eq!(&parameter[8..], &4096u32.to_le_bytes());
    // Neither import nor export can make a second backing allocation.
    error(
        call(&mut d, &mut m, fd, 0xc0200104, &allocation, &mut allocated),
        NvError::AlreadyAllocated,
    );
    for (request, input, expected) in [
        (0xc008010e, words(&[handle, 0]), NvError::BadParameter),
        (0xc0080103, words(&[0, handle]), NvError::BadParameter),
        (
            0xc0080103,
            words(&[u32::MAX, handle]),
            NvError::BadParameter,
        ),
        (0xc008010e, vec![0; 7], NvError::InvalidSize),
        (0xc0080103, vec![0; 9], NvError::InvalidSize),
    ] {
        output.fill(0xcc);
        error(
            call(&mut d, &mut m, fd, request, &input, &mut output),
            expected,
        );
        assert_eq!(output, [0xcc; 8]);
    }
    assert_eq!(d.graphics_counters(), counters);
    d.close(fd, &mut m).unwrap();
    assert_eq!((m.pins, m.unpins), (1, 1));
    let fd = d.open("/dev/nvmap").unwrap();
    output.fill(0xcc);
    error(
        call(
            &mut d,
            &mut m,
            fd,
            0xc0080103,
            &words(&[id, 0]),
            &mut output,
        ),
        NvError::BadParameter,
    );
    assert_eq!(output, [0xcc; 8]);
    call(
        &mut d,
        &mut m,
        fd,
        0xc0080101,
        &words(&[4096, 0]),
        &mut output,
    )
    .unwrap();
    assert_ne!(&output[4..], &handle.to_le_bytes());
    d.close(fd, &mut m).unwrap();
    assert_eq!((m.pins, m.unpins), (1, 1));
}
