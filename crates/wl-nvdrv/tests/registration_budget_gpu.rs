use wl_gpu::memory::{GuestAddress, GuestMemory, GuestMemoryWrite};
use wl_nvdrv::{Driver, Error, Ioctl, IoctlKind, NvError};
use wl_runtime::{Memory, gpu_memory::GpuPin, horizon::state};

struct Backing(Memory);
impl GuestMemory for Backing {
    fn read(&self, address: GuestAddress, size: usize) -> wl_gpu::Result<&[u8]> {
        self.0.read_bytes(address.0, size).map_err(wl_gpu::Error)
    }
}
impl GuestMemoryWrite for Backing {
    fn validate_write(&self, address: GuestAddress, size: usize) -> wl_gpu::Result<()> {
        self.0
            .validate_write(address.0, size)
            .map_err(wl_gpu::Error)
    }
    fn write(&mut self, address: GuestAddress, bytes: &[u8]) -> wl_gpu::Result<()> {
        self.0.write_bytes(address.0, bytes).map_err(wl_gpu::Error)
    }
}
impl wl_nvdrv::Memory for Backing {
    type Pin = GpuPin;
    fn pin(&mut self, address: GuestAddress, size: u64) -> wl_gpu::Result<GpuPin> {
        self.0.pin_gpu_range(address.0, size).map_err(wl_gpu::Error)
    }
    fn unpin(&mut self, pin: GpuPin) -> wl_gpu::Result<()> {
        self.0.release_gpu_pin(pin).map_err(wl_gpu::Error)
    }
}
fn call(
    d: &mut Driver<GpuPin>,
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

#[test]
#[ignore = "requires GPU device; validates thousands of real runtime lifetime registrations"]
fn allocation_budget_preserves_independent_audio_pin_and_recovers_after_close() {
    const BASE: u64 = 0x10000000;
    let capacity = wl_gpu::driver::MAX_OBJECTS;
    assert_eq!(capacity, 4096);
    assert!(wl_runtime::gpu_memory::MAX_GPU_PINS > capacity);
    let mut m = Backing(Memory::default());
    m.0.map_state(
        BASE,
        vec![0x3a; (capacity + 2) * 4096],
        true,
        true,
        false,
        state::NORMAL,
    )
    .unwrap();
    let audio_pin = m.0.pin_gpu_range(BASE, 4096).unwrap();
    let mut d = Driver::new().unwrap();
    let fd = d.open("/dev/nvmap").unwrap();
    let input: Vec<u8> = [4096u32, 0]
        .into_iter()
        .flat_map(u32::to_le_bytes)
        .collect();
    for page in 0..capacity {
        let mut output = [0u8; 8];
        call(&mut d, &mut m, fd, 0xc0080101, &input, &mut output).unwrap();
        let handle = u32::from_le_bytes(output[4..].try_into().unwrap());
        let mut allocation: Vec<u8> = [handle, 0, 0, 4096, 0, 0]
            .into_iter()
            .flat_map(u32::to_le_bytes)
            .collect();
        allocation.extend((BASE + (page as u64 + 1) * 4096).to_le_bytes());
        call(&mut d, &mut m, fd, 0xc0200104, &allocation, &mut [0; 32]).unwrap();
    }
    let mut rejected = [0xa5; 8];
    assert!(matches!(
        call(&mut d, &mut m, fd, 0xc0080101, &input, &mut rejected),
        Err(Error::Driver(NvError::ResourceError))
    ));
    assert_eq!(rejected, [0xa5; 8]);
    assert!(
        m.0.ensure_gpu_unpinned(BASE + 4096, capacity as u64 * 4096)
            .is_err()
    );
    d.close(fd, &mut m).unwrap();
    assert!(
        m.0.ensure_gpu_unpinned(BASE + 4096, capacity as u64 * 4096)
            .is_ok()
    );
    assert!(m.0.ensure_gpu_unpinned(BASE, 4096).is_err());
    m.0.release_gpu_pin(audio_pin).unwrap();
    assert!(
        m.0.read_bytes(BASE, (capacity + 2) * 4096)
            .unwrap()
            .iter()
            .all(|b| *b == 0x3a)
    );
    assert!(
        m.0.ensure_gpu_unpinned(BASE, (capacity as u64 + 2) * 4096)
            .is_ok()
    );
}
