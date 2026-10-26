//! Nvmap wire ABI and shared allocation lifetime through the public device API.
use wl_gpu::memory::{GuestAddress, GuestMemory, GuestMemoryWrite};
use wl_nvdrv::{Driver, Error, Ioctl, IoctlKind, Memory, NvError};

struct Backing {
    bytes: [u8; 4096],
    pins: usize,
    unpins: usize,
}
impl Default for Backing {
    fn default() -> Self {
        Self {
            bytes: [0; 4096],
            pins: 0,
            unpins: 0,
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
        self.unpins += 1;
        Ok(())
    }
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
