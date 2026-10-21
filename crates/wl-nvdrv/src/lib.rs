//! Process-local NVIDIA device adapter over the existing GPU driver.
//! All requests use explicit little-endian wire decoding. This is a bounded,
//! synchronous subset, not a Horizon handle manager or a second GPU interpreter.
use wl_gpu::{driver::*, driver_backend::GpuDriver, memory::*};
mod address_space;
mod control;
mod nvmap;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u32)]
pub enum NvError {
    NotSupported = 2,
    BadParameter = 4,
    InvalidState = 8,
    InvalidAddress = 9,
    InvalidSize = 10,
    AlreadyAllocated = 13,
    ResourceError = 15,
    IoctlFailed = 0x3000f,
}
#[derive(Debug)]
pub enum Error {
    Driver(NvError),
    Fatal(String),
}
pub type Result<T> = std::result::Result<T, Error>;
impl From<NvError> for Error {
    fn from(e: NvError) -> Self {
        Self::Driver(e)
    }
}
fn checked<T>(r: wl_gpu::Result<T>) -> Result<T> {
    r.map_err(|_| NvError::InvalidState.into())
}
fn fatal<T>(r: wl_gpu::Result<T>) -> Result<T> {
    r.map_err(|e| Error::Fatal(e.to_string()))
}

/// Runtime-owned pinning, not host pointers or a second allocation manager.
/// Ordinary CPU writes may occur only between these synchronous calls.
pub trait Memory: GuestMemoryWrite {
    type Pin: Copy;
    fn pin(&mut self, base: GuestAddress, size: u64) -> wl_gpu::Result<Self::Pin>;
    fn unpin(&mut self, pin: Self::Pin) -> wl_gpu::Result<()>;
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum IoctlKind {
    Ioctl,
    Ioctl2,
    Ioctl3,
}
pub struct Ioctl<'a> {
    pub fd: u32,
    pub request: u32,
    pub kind: IoctlKind,
    pub input: &'a [u8],
    pub extra_input: &'a [u8],
    pub output: &'a mut [u8],
    pub extra_output: &'a mut [u8],
}
const MAX_FILES: usize = 32;
const MAX_ALLOCS: usize = 64;
#[derive(Debug)]
struct Allocation<P> {
    handle: u32,
    size: u64,
    storage: Option<Storage<P>>,
}
#[derive(Debug, Clone, Copy)]
struct Storage<P> {
    backing: BackingId,
    pin: P,
}
#[derive(Debug)]
struct Reservation {
    address: u64,
    size: u64,
    page_size: u32,
    id: ReservationId,
    mapping: Option<MappingId>,
    allocation: Option<u32>,
}
#[derive(Debug)]
struct Space {
    id: AddressSpaceId,
    configuration: Option<address_space::Configuration>,
    reservations: Vec<Reservation>,
}
#[derive(Debug)]
struct Channel {
    id: Option<ChannelId>,
    nvmap: Option<u32>,
    gpfifo: bool,
    object: bool,
    syncpoint: u32,
    threshold: u32,
    entries: u32,
}
#[derive(Debug)]
enum Node {
    ControlGpu,
    Nvmap,
    Space(Space),
    Channel(Channel),
}
#[derive(Debug)]
struct File {
    fd: u32,
    node: Node,
}
/// One process-local device session. Every accepted GPU prefix is completed and
/// published before success OR an ordinary driver error returns to guest code.
/// A post-acceptance publication failure is fatal and must halt guest execution.
pub struct Driver<P: Copy> {
    gpu: GpuDriver,
    files: Vec<File>,
    allocations: Vec<Allocation<P>>,
    next_fd: u32,
    next_handle: u32,
    next_syncpoint: u32,
    fatal: Option<String>,
    frame: Option<wl_gpu::backend::Frame>,
}
impl<P: Copy> std::fmt::Debug for Driver<P> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("NvidiaDriver")
            .field("files", &self.files.len())
            .field("allocations", &self.allocations.len())
            .finish_non_exhaustive()
    }
}
fn word(b: &[u8], at: usize) -> u32 {
    u32::from_le_bytes(b[at..at + 4].try_into().unwrap())
}
fn long(b: &[u8], at: usize) -> u64 {
    u64::from_le_bytes(b[at..at + 8].try_into().unwrap())
}
fn out32(b: &mut [u8], at: usize, v: u32) {
    b[at..at + 4].copy_from_slice(&v.to_le_bytes());
}
fn out64(b: &mut [u8], at: usize, v: u64) {
    b[at..at + 8].copy_from_slice(&v.to_le_bytes());
}
impl<P: Copy> Driver<P> {
    pub fn new() -> wl_gpu::Result<Self> {
        Ok(Self {
            gpu: GpuDriver::new()?,
            files: Vec::new(),
            allocations: Vec::new(),
            next_fd: 1,
            next_handle: 1,
            next_syncpoint: 1,
            fatal: None,
            frame: None,
        })
    }
    pub fn capabilities(&self) -> &wl_gpu::backend::Capabilities {
        self.gpu.capabilities()
    }
    pub fn graphics_counters(&self) -> wl_gpu::Counters {
        self.gpu.graphics_counters()
    }
    pub fn driver_counters(&self) -> wl_gpu::driver_backend::DriverCounters {
        self.gpu.counters
    }
    pub fn has_pending_output(&self) -> bool {
        self.gpu.has_pending_output()
    }
    /// Latest explicitly published color output. This is a diagnostic image,
    /// not a guest VI queue or a replacement for display-buffer ownership.
    pub fn take_frame(&mut self) -> Option<wl_gpu::backend::Frame> {
        self.frame.take()
    }
    fn file(&self, fd: u32) -> Result<usize> {
        self.files
            .iter()
            .position(|f| f.fd == fd)
            .ok_or(NvError::BadParameter.into())
    }
    fn allocation(&self, handle: u32) -> Result<usize> {
        self.allocations
            .iter()
            .position(|a| a.handle == handle)
            .ok_or(NvError::BadParameter.into())
    }
    fn healthy(&self) -> Result<()> {
        if let Some(message) = &self.fatal {
            return Err(Error::Fatal(message.clone()));
        }
        Ok(())
    }
    pub fn open(&mut self, path: &str) -> Result<u32> {
        self.healthy()?;
        if self.files.len() == MAX_FILES || self.next_fd == u32::MAX {
            return Err(NvError::ResourceError.into());
        }
        let node = match path {
            "/dev/nvhost-ctrl-gpu" => Node::ControlGpu,
            "/dev/nvmap" => {
                if self.files.iter().any(|f| matches!(f.node, Node::Nvmap)) {
                    return Err(NvError::NotSupported.into());
                }
                Node::Nvmap
            }
            "/dev/nvhost-as-gpu" => Node::Space(Space {
                id: checked(self.gpu.create_space())?,
                configuration: None,
                reservations: Vec::new(),
            }),
            "/dev/nvhost-gpu" => {
                if self.next_syncpoint > 16 {
                    return Err(NvError::ResourceError.into());
                }
                let id = self.next_syncpoint;
                self.next_syncpoint += 1;
                Node::Channel(Channel {
                    id: None,
                    nvmap: None,
                    gpfifo: false,
                    object: false,
                    syncpoint: id,
                    threshold: 0,
                    entries: 0,
                })
            }
            _ => return Err(NvError::NotSupported.into()),
        };
        let fd = self.next_fd;
        self.next_fd += 1;
        self.files.push(File { fd, node });
        Ok(fd)
    }
    pub fn ioctl(&mut self, r: Ioctl<'_>, memory: &mut impl Memory<Pin = P>) -> Result<()> {
        self.healthy()?;
        let size = ((r.request >> 16) & 0x3fff) as usize;
        let direction = r.request >> 30;
        if r.input.len() != if direction & 1 != 0 { size } else { 0 }
            || r.output.len() != if direction & 2 != 0 { size } else { 0 }
        {
            return Err(NvError::InvalidSize.into());
        }
        if [
            r.input.len(),
            r.output.len(),
            r.extra_input.len(),
            r.extra_output.len(),
        ]
        .into_iter()
        .try_fold(0usize, |total, size| total.checked_add(size))
        .is_none_or(|total| total > 65536)
        {
            return Err(NvError::InvalidSize.into());
        }
        let f = self.file(r.fd)?;
        let inline_query = match self.files[f].node {
            Node::ControlGpu => control::is_inline_query(&r),
            Node::Space(_) => r.request == 0xc0404108 && r.kind == IoctlKind::Ioctl3,
            _ => false,
        };
        if r.request != KICKOFF_PB
            && !inline_query
            && (r.kind != IoctlKind::Ioctl
                || !r.extra_input.is_empty()
                || !r.extra_output.is_empty())
        {
            return Err(NvError::NotSupported.into());
        }
        match &self.files[f].node {
            Node::ControlGpu => control::ioctl(r),
            Node::Nvmap => self.nvmap(r, memory),
            Node::Space(_) => self.space(f, r),
            Node::Channel(_) => self.channel(f, r, memory),
        }
    }
    fn nvmap(&mut self, r: Ioctl<'_>, memory: &mut impl Memory<Pin = P>) -> Result<()> {
        match r.request {
            0xc0080101 => {
                let size = u64::from(word(r.input, 0));
                if size == 0 || size % PAGE_SIZE != 0 {
                    return Err(NvError::InvalidSize.into());
                }
                if self.allocations.len() == MAX_ALLOCS || self.next_handle == u32::MAX {
                    return Err(NvError::ResourceError.into());
                }
                let handle = self.next_handle;
                self.next_handle += 1;
                self.allocations.push(Allocation {
                    handle,
                    size,
                    storage: None,
                });
                r.output.copy_from_slice(r.input);
                out32(r.output, 4, handle);
                Ok(())
            }
            0xc0200104 => {
                let index = self.allocation(word(r.input, 0))?;
                if self.allocations[index].storage.is_some() {
                    return Err(NvError::AlreadyAllocated.into());
                }
                let size = self.allocations[index].size;
                let address = nvmap::user_backing(r.input, size)?;
                let pin = memory
                    .pin(address, size)
                    .map_err(|_| NvError::InvalidAddress)?;
                let backing = match self.gpu.register_backing(address, size) {
                    Ok(id) => id,
                    Err(_) => {
                        fatal(memory.unpin(pin))?;
                        return Err(NvError::InvalidState.into());
                    }
                };
                self.allocations[index].storage = Some(Storage { backing, pin });
                r.output.copy_from_slice(r.input);
                Ok(())
            }
            // Free semantics have conflicting public field descriptions. Do not
            // fabricate a refcount/CPU address response or release mapped storage.
            _ => Err(NvError::NotSupported.into()),
        }
    }
    fn space(&mut self, f: usize, r: Ioctl<'_>) -> Result<()> {
        if r.request == 0x40044101 {
            let channel = self.file(word(r.input, 0))?;
            let Node::Space(space) = &self.files[f].node else {
                unreachable!()
            };
            if space.configuration.is_none() {
                return Err(NvError::InvalidState.into());
            }
            let space_id = space.id;
            let Node::Channel(state) = &self.files[channel].node else {
                return Err(NvError::BadParameter.into());
            };
            if state.id.is_some() {
                return Err(NvError::AlreadyAllocated.into());
            }
            let id = checked(self.gpu.create_channel(space_id))?;
            let Node::Channel(state) = &mut self.files[channel].node else {
                unreachable!()
            };
            state.id = Some(id);
            return Ok(());
        }
        let Node::Space(space) = &mut self.files[f].node else {
            unreachable!()
        };
        if r.request == 0x40284109 {
            if space.configuration.is_some() {
                return Err(NvError::AlreadyAllocated.into());
            }
            space.configuration = Some(address_space::Configuration::decode(r.input)?);
            return Ok(());
        }
        if space.configuration.is_none() {
            return Err(NvError::InvalidState.into());
        }
        let configuration = space.configuration.as_ref().ok_or(NvError::InvalidState)?;
        match r.request {
            0xc0404108 => {
                if !matches!(word(r.input, 8), 0 | 48) || word(r.input, 12) != 0 {
                    return Err(NvError::InvalidSize.into());
                }
                if !r.extra_input.is_empty()
                    || (r.kind == IoctlKind::Ioctl3 && r.extra_output.len() != 48)
                {
                    return Err(NvError::InvalidSize.into());
                }
                let regions = configuration.regions()?;
                r.output.copy_from_slice(r.input);
                out32(r.output, 8, 48);
                r.output[16..].copy_from_slice(&regions);
                if r.kind == IoctlKind::Ioctl3 {
                    r.extra_output.copy_from_slice(&regions);
                }
                Ok(())
            }
            0xc0184102 => {
                let pages = u64::from(word(r.input, 0));
                let page_size = word(r.input, 4);
                let flags = word(r.input, 8);
                if flags > 1 || word(r.input, 12) != 0 {
                    return Err(NvError::NotSupported.into());
                }
                let size = pages
                    .checked_mul(u64::from(page_size))
                    .filter(|n| *n != 0)
                    .ok_or(NvError::InvalidSize)?;
                let address = if flags == 1 {
                    long(r.input, 16)
                } else {
                    configuration.allocate(
                        size,
                        page_size,
                        long(r.input, 16),
                        space.reservations.iter().map(|v| (v.address, v.size)),
                    )?
                };
                configuration.validate_range(address, size, page_size)?;
                let id = checked(self.gpu.reserve_fixed(space.id, GpuAddress(address), size))?;
                space.reservations.push(Reservation {
                    address,
                    size,
                    page_size,
                    id,
                    mapping: None,
                    allocation: None,
                });
                r.output.copy_from_slice(r.input);
                out64(r.output, 16, address);
                Ok(())
            }
            0xc0284106 => {
                if !matches!(word(r.input, 0), 1 | 5)
                    || !matches!(word(r.input, 4), 0 | u32::MAX)
                    || word(r.input, 12) != 0
                {
                    return Err(NvError::NotSupported.into());
                }
                let handle = word(r.input, 8);
                let allocation = self
                    .allocations
                    .iter()
                    .find(|a| a.handle == handle)
                    .ok_or(NvError::BadParameter)?;
                let storage = allocation.storage.as_ref().ok_or(NvError::InvalidState)?;
                let address = long(r.input, 32);
                let reservation = space
                    .reservations
                    .iter_mut()
                    .find(|v| v.address == address)
                    .ok_or(NvError::InvalidAddress)?;
                if reservation.mapping.is_some() {
                    return Err(NvError::AlreadyAllocated.into());
                }
                let offset = long(r.input, 16);
                let requested = long(r.input, 24);
                let size = if requested == 0 {
                    allocation
                        .size
                        .checked_sub(offset)
                        .ok_or(NvError::InvalidSize)?
                } else {
                    requested
                };
                if size != reservation.size {
                    return Err(NvError::NotSupported.into());
                }
                let mapping =
                    checked(
                        self.gpu
                            .map(space.id, reservation.id, storage.backing, offset),
                    )?;
                reservation.mapping = Some(mapping);
                reservation.allocation = Some(handle);
                r.output.copy_from_slice(r.input);
                Ok(())
            }
            0xc0084105 => {
                let reservation = space
                    .reservations
                    .iter_mut()
                    .find(|v| v.address == long(r.input, 0))
                    .ok_or(NvError::InvalidAddress)?;
                let mapping = reservation.mapping.ok_or(NvError::InvalidState)?;
                checked(self.gpu.unmap(space.id, mapping))?;
                reservation.mapping = None;
                reservation.allocation = None;
                r.output.copy_from_slice(r.input);
                Ok(())
            }
            0xc0104103 => {
                let size = u64::from(word(r.input, 8))
                    .checked_mul(u64::from(word(r.input, 12)))
                    .ok_or(NvError::InvalidSize)?;
                configuration.validate_range(long(r.input, 0), size, word(r.input, 12))?;
                let index = space
                    .reservations
                    .iter()
                    .position(|v| {
                        v.address == long(r.input, 0)
                            && v.size == size
                            && v.page_size == word(r.input, 12)
                    })
                    .ok_or(NvError::InvalidAddress)?;
                if space.reservations[index].mapping.is_some() {
                    return Err(NvError::InvalidState.into());
                }
                checked(
                    self.gpu
                        .free_reservation(space.id, space.reservations[index].id),
                )?;
                space.reservations.remove(index);
                r.output.copy_from_slice(r.input);
                Ok(())
            }
            _ => Err(NvError::NotSupported.into()),
        }
    }
    fn channel(&mut self, f: usize, r: Ioctl<'_>, memory: &mut impl Memory<Pin = P>) -> Result<()> {
        if r.request == 0x40044801 {
            let nvmap = word(r.input, 0);
            let n = self.file(nvmap)?;
            if !matches!(self.files[n].node, Node::Nvmap) {
                return Err(NvError::BadParameter.into());
            }
            let Node::Channel(state) = &mut self.files[f].node else {
                unreachable!()
            };
            if state.nvmap.is_some() {
                return Err(NvError::AlreadyAllocated.into());
            }
            state.nvmap = Some(nvmap);
            return Ok(());
        }
        let Node::Channel(state) = &mut self.files[f].node else {
            unreachable!()
        };
        let channel = state.id.ok_or(NvError::InvalidState)?;
        if state.nvmap.is_none() {
            return Err(NvError::InvalidState.into());
        }
        match r.request {
            0x40084805 => {
                if state.gpfifo {
                    return Err(NvError::AlreadyAllocated.into());
                }
                if word(r.input, 0) == 0 || word(r.input, 0) > 2048 || word(r.input, 4) != 0 {
                    return Err(NvError::NotSupported.into());
                }
                state.gpfifo = true;
                state.entries = word(r.input, 0);
                Ok(())
            }
            0xc0104809 => {
                if state.object {
                    return Err(NvError::AlreadyAllocated.into());
                }
                if word(r.input, 0) != 0xb197 || word(r.input, 4) != 0 {
                    return Err(NvError::NotSupported.into());
                }
                state.object = true;
                r.output.copy_from_slice(r.input);
                out64(r.output, 8, (u64::from(r.fd) << 32) | 0xb197);
                Ok(())
            }
            KICKOFF_PB => {
                if !state.object || !state.gpfifo {
                    return Err(NvError::InvalidState.into());
                }
                if r.kind != IoctlKind::Ioctl2 || !r.extra_output.is_empty() {
                    return Err(NvError::NotSupported.into());
                }
                // Driver-generated fence increments are completed only after a
                // real host GPU wait/publication. No token sequence is exposed as
                // a syncpoint. The non-incrementing form returns the existing
                // threshold and is not a new completion guarantee.
                let flags = word(r.input, 12);
                if !matches!(flags, 4 | 6) {
                    return Err(NvError::NotSupported.into());
                }
                if word(r.input, 8) > state.entries {
                    return Err(NvError::InvalidSize.into());
                }
                let threshold = state
                    .threshold
                    .checked_add(if flags & 2 != 0 { 2 } else { 0 })
                    .ok_or(NvError::NotSupported)?;
                let mut header = [0u8; 24];
                header.copy_from_slice(r.input);
                out32(&mut header, 12, 4);
                out32(&mut header, 16, 0);
                out32(&mut header, 20, 0);
                // Command metadata is validated before dirty notifications or
                // acceptance. The existing decoder alone interprets pushbuffers.
                SubmitRequest::decode(KICKOFF_PB, &header, Some(r.extra_input))
                    .map_err(|_| NvError::BadParameter)?;
                for allocation in &self.allocations {
                    if let Some(storage) = allocation.storage {
                        checked(self.gpu.notify_cpu_write(
                            storage.backing,
                            0,
                            allocation.size as usize,
                        ))?;
                    }
                }
                let report = checked(self.gpu.submit(
                    channel,
                    KICKOFF_PB,
                    &header,
                    Some(r.extra_input),
                    memory,
                ))?;
                let mut completed_frame = None;
                let finish = (|| -> wl_gpu::Result<()> {
                    completed_frame = self.gpu.publish(channel, memory)?;
                    let status = self.gpu.wait(report.token)?;
                    if !status.host_completed || !status.guest_visible {
                        return Err(wl_gpu::Error("GPU completion/publication invariant".into()));
                    }
                    self.gpu.retire(report.token)?;
                    if let Some(fault) = report.fault {
                        return Err(wl_gpu::Error(format!(
                            "accepted NVIDIA submission fault: {fault:?}"
                        )));
                    }
                    Ok(())
                })();
                if let Err(error) = finish {
                    let message = error.to_string();
                    self.fatal = Some(message.clone());
                    return Err(Error::Fatal(message));
                }
                self.frame = completed_frame;
                state.threshold = threshold;
                r.output.copy_from_slice(r.input);
                out32(r.output, 12, 0);
                out32(r.output, 16, state.syncpoint);
                out32(r.output, 20, threshold);
                Ok(())
            }
            _ => Err(NvError::NotSupported.into()),
        }
    }
    pub fn close(&mut self, fd: u32, memory: &mut impl Memory<Pin = P>) -> Result<()> {
        let f = self.file(fd)?;
        match &self.files[f].node {
            Node::ControlGpu => {}
            Node::Channel(state) => {
                if let Some(channel) = state.id {
                    checked(self.gpu.close(channel, memory))?;
                }
            }
            Node::Space(space) => {
                if !space.reservations.is_empty() {
                    return Err(NvError::InvalidState.into());
                }
                checked(self.gpu.destroy_space(space.id))?;
            }
            Node::Nvmap => {
                if self.files.iter().any(|file| matches!(&file.node, Node::Channel(channel) if channel.nvmap == Some(fd))) {
                    return Err(NvError::InvalidState.into());
                }
                if self.files.iter().any(|f|matches!(&f.node,Node::Space(s) if s.reservations.iter().any(|r|r.mapping.is_some()))) {
                    return Err(NvError::InvalidState.into());
                }
                for allocation in &mut self.allocations {
                    if let Some(storage) = allocation.storage {
                        checked(self.gpu.release_backing(storage.backing))?;
                        fatal(memory.unpin(storage.pin))?;
                        allocation.storage = None;
                    }
                }
                self.allocations.clear();
            }
        }
        self.files.remove(f);
        Ok(())
    }
}

/// Module source compiled against the exact embedded `wl_guest_runtime` identity
/// by the native executable; Cargo runtime types are not substituted for it.
pub const RUNTIME_ADAPTER: &str = include_str!("runtime_adapter.rs");
