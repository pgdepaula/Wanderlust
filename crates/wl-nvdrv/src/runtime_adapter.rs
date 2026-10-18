//! This module is compiled in the generated executable, not in wl-nvdrv.
use wl_gpu::memory::{GuestAddress, GuestMemory, GuestMemoryWrite};
use wl_guest_runtime::{Memory, gpu_memory::GpuPin, nvidia as service};
use wl_nvdrv as driver;
struct MemoryView<'a>(&'a mut Memory);
impl GuestMemory for MemoryView<'_> {
    fn read(&self, address: GuestAddress, length: usize) -> wl_gpu::Result<&[u8]> {
        self.0.read_bytes(address.0, length).map_err(wl_gpu::Error)
    }
}
impl GuestMemoryWrite for MemoryView<'_> {
    fn validate_write(&self, address: GuestAddress, length: usize) -> wl_gpu::Result<()> {
        self.0
            .validate_write(address.0, length)
            .map_err(wl_gpu::Error)
    }
    fn write(&mut self, address: GuestAddress, bytes: &[u8]) -> wl_gpu::Result<()> {
        self.0.write_bytes(address.0, bytes).map_err(wl_gpu::Error)
    }
}
impl driver::Memory for MemoryView<'_> {
    type Pin = GpuPin;
    fn pin(&mut self, base: GuestAddress, size: u64) -> wl_gpu::Result<GpuPin> {
        self.0.pin_gpu_range(base.0, size).map_err(wl_gpu::Error)
    }
    fn unpin(&mut self, pin: GpuPin) -> wl_gpu::Result<()> {
        self.0.release_gpu_pin(pin).map_err(wl_gpu::Error)
    }
}
fn error(e: driver::Error) -> service::HostError {
    match e {
        driver::Error::Fatal(message) => service::HostError::Fatal(message),
        driver::Error::Driver(e) => service::HostError::Driver(match e {
            driver::NvError::NotSupported => service::NvError::NotSupported,
            driver::NvError::BadParameter => service::NvError::BadParameter,
            driver::NvError::InvalidState => service::NvError::InvalidState,
            driver::NvError::InvalidAddress => service::NvError::InvalidAddress,
            driver::NvError::InvalidSize => service::NvError::InvalidSize,
            driver::NvError::AlreadyAllocated => service::NvError::AlreadyAllocated,
            driver::NvError::ResourceError => service::NvError::ResourceError,
            driver::NvError::IoctlFailed => service::NvError::IoctlFailed,
        }),
    }
}
#[derive(Debug)]
pub struct NativeHost {
    driver: driver::Driver<GpuPin>,
}
impl NativeHost {
    pub fn new() -> wl_gpu::Result<Self> {
        let driver = driver::Driver::new()?;
        eprintln!("NVIDIA adapter {:?}", driver.capabilities().adapter);
        Ok(Self { driver })
    }
    pub fn graphics_counters(&self) -> wl_gpu::Counters {
        self.driver.graphics_counters()
    }
    pub fn driver_counters(&self) -> wl_gpu::driver_backend::DriverCounters {
        self.driver.driver_counters()
    }
}
impl service::Host for NativeHost {
    fn initialize(&mut self, _transfer: service::TransferBuffer) -> service::HostResult<()> {
        Ok(())
    }
    fn set_client_pid(&mut self, _aruid: u64, _pid: u64) -> service::HostResult<()> {
        Ok(())
    }
    fn open(&mut self, path: &str) -> service::HostResult<u32> {
        self.driver.open(path).map_err(error)
    }
    fn close(&mut self, fd: u32, memory: &mut Memory) -> service::HostResult<()> {
        self.driver
            .close(fd, &mut MemoryView(memory))
            .map_err(error)
    }
    fn ioctl(&mut self, r: service::Ioctl<'_>, memory: &mut Memory) -> service::HostResult<()> {
        let kind = match r.kind {
            service::IoctlKind::Ioctl => driver::IoctlKind::Ioctl,
            service::IoctlKind::Ioctl2 => driver::IoctlKind::Ioctl2,
            service::IoctlKind::Ioctl3 => driver::IoctlKind::Ioctl3,
        };
        self.driver
            .ioctl(
                driver::Ioctl {
                    fd: r.fd,
                    request: r.request,
                    kind,
                    input: r.input,
                    extra_input: r.extra_input,
                    output: r.output,
                    extra_output: r.extra_output,
                },
                &mut MemoryView(memory),
            )
            .map_err(error)
    }
}
