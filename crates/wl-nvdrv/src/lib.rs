//! Process-local NVIDIA device adapter over the existing GPU driver.
//! All requests use explicit little-endian wire decoding. This is a bounded,
//! synchronous subset, not a Horizon handle manager or a second GPU interpreter.
use wl_gpu::{driver::*, driver_backend::GpuDriver, memory::*};
mod address_space;
mod channel;
mod control;
mod mapping;
mod nvmap;
mod remap;

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
    /// Success consumes the pin; failure retains it for a checked retry.
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
    references: u32,
    size: u64,
    storage: Option<Storage<P>>,
}
#[derive(Debug, Clone, Copy)]
struct Storage<P> {
    backing: BackingId,
    pin: P,
    alignment: u32,
}
#[derive(Debug)]
struct Reservation {
    address: u64,
    size: u64,
    page_size: u32,
    id: ReservationId,
    mappings: Vec<BufferMapping>,
    automatic: bool,
    sparse: bool,
}
#[derive(Debug)]
struct BufferMapping {
    address: u64,
    size: u64,
    id: MappingId,
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
    priority: u32,
    timeslice_us: u32,
    user_data: u64,
    notifier_enabled: bool,
    error_event: bool,
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
                    priority: 0x64,
                    timeslice_us: 2600,
                    user_data: 0,
                    notifier_enabled: false,
                    error_event: false,
                })
            }
            _ => return Err(NvError::NotSupported.into()),
        };
        let fd = self.next_fd;
        self.next_fd += 1;
        self.files.push(File { fd, node });
        Ok(fd)
    }
    /// Channel error notification only, not a completion fence. The synchronous
    /// execution adapter latches this event on an accepted execution/publication
    /// fault while notifications are enabled. Diagnostic queries survive faults.
    pub fn query_event(&self, fd: u32, event: u32) -> Result<bool> {
        let f = self.file(fd)?;
        let Node::Channel(channel) = &self.files[f].node else {
            return Err(NvError::NotSupported.into());
        };
        if event != 3 || channel.id.is_none() || !channel.object {
            return Err(NvError::NotSupported.into());
        }
        Ok(channel.error_event)
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
            0xc0180105 => {
                let index = self.allocation(word(r.input, 0))?;
                let allocation = &self.allocations[index];
                let remaining = allocation
                    .references
                    .checked_sub(1)
                    .ok_or(NvError::InvalidState)?;
                let size = u32::try_from(allocation.size).map_err(|_| NvError::InvalidSize)?;
                if remaining == 0 {
                    if let Some(storage) = allocation.storage {
                        checked(self.gpu.validate_release_backing(storage.backing))?;
                        memory
                            .unpin(storage.pin)
                            .map_err(|_| NvError::InvalidState)?;
                        // Exclusive ownership prevents a mapping/submission
                        // between the release preflight and this mutation.
                        if let Err(error) = self.gpu.release_backing(storage.backing) {
                            let message =
                                format!("backing release changed after preflight: {error}");
                            self.fatal = Some(message.clone());
                            return Err(Error::Fatal(message));
                        }
                    }
                    self.allocations.remove(index);
                } else {
                    self.allocations[index].references = remaining;
                }
                r.output.copy_from_slice(r.input);
                out64(r.output, 8, u64::from(remaining));
                out32(r.output, 16, size);
                out32(r.output, 20, u32::from(remaining != 0));
                Ok(())
            }
            0xc008010e => {
                // GetId's input handle is the SECOND word, unlike FromId.
                // IDs are opaque and scoped to this process-local session.
                let allocation = &self.allocations[self.allocation(word(r.input, 4))?];
                r.output.copy_from_slice(r.input);
                out32(r.output, 0, allocation.handle);
                Ok(())
            }
            0xc0080103 => {
                let index = self.allocation(word(r.input, 0))?;
                let allocation = &mut self.allocations[index];
                let references = nvmap::imported_references(
                    allocation.references,
                    allocation.storage.is_some(),
                )?;
                // An existing client reference is retained, not a second
                // allocation, backing registration or runtime pin.
                allocation.references = references;
                r.output.copy_from_slice(r.input);
                out32(r.output, 4, allocation.handle);
                Ok(())
            }
            0xc00c0109 => {
                let allocation = &self.allocations[self.allocation(word(r.input, 0))?];
                let value = nvmap::parameter(
                    word(r.input, 4),
                    allocation.size,
                    allocation.storage.as_ref().map(|s| s.alignment),
                )?;
                r.output.copy_from_slice(r.input);
                out32(r.output, 8, value);
                Ok(())
            }
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
                    references: 1,
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
                let selected = nvmap::user_backing(r.input, size)?;
                let address = selected.address;
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
                self.allocations[index].storage = Some(Storage {
                    backing,
                    pin,
                    alignment: selected.alignment,
                });
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
        if r.request & 0xc000ffff == 0xc0004114 {
            let entries = remap::decode(r.input)?;
            let mut changes = Vec::with_capacity(entries.len());
            for entry in entries {
                let end = entry
                    .address
                    .checked_add(entry.size)
                    .ok_or(NvError::InvalidSize)?;
                let reservation = space
                    .reservations
                    .iter()
                    .find(|v| v.sparse && entry.address >= v.address && end <= v.address + v.size)
                    .ok_or(NvError::InvalidAddress)?;
                configuration.validate_range(entry.address, entry.size, 65536)?;
                let backing = if entry.handle == 0 {
                    None
                } else {
                    let allocation = self
                        .allocations
                        .iter()
                        .find(|a| a.handle == entry.handle)
                        .ok_or(NvError::BadParameter)?;
                    if entry.size > allocation.size {
                        return Err(NvError::InvalidSize.into());
                    }
                    Some((
                        allocation
                            .storage
                            .as_ref()
                            .ok_or(NvError::InvalidState)?
                            .backing,
                        0,
                    ))
                };
                changes.push(MapChange {
                    reservation: reservation.id,
                    address: GpuAddress(entry.address),
                    size: entry.size,
                    backing,
                });
            }
            checked(self.gpu.apply_map_changes(space.id, &changes))?;
            for reservation in &mut space.reservations {
                if reservation.sparse {
                    reservation.mappings = checked(self.gpu.mappings(space.id, reservation.id))?
                        .into_iter()
                        .map(|m| BufferMapping {
                            address: m.address.0,
                            size: m.size,
                            id: m.id,
                        })
                        .collect();
                }
            }
            r.output.copy_from_slice(r.input);
            return Ok(());
        }
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
                if flags & !3 != 0 || word(r.input, 12) != 0 {
                    return Err(NvError::NotSupported.into());
                }
                let sparse = flags & 2 != 0;
                if sparse && page_size != 65536 {
                    return Err(NvError::NotSupported.into());
                }
                let size = pages
                    .checked_mul(u64::from(page_size))
                    .filter(|n| *n != 0)
                    .ok_or(NvError::InvalidSize)?;
                let address = if flags & 1 != 0 {
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
                    mappings: Vec::new(),
                    automatic: false,
                    sparse,
                });
                r.output.copy_from_slice(r.input);
                out64(r.output, 16, address);
                Ok(())
            }
            0xc0284106 => {
                let request = mapping::Request::decode(r.input)?;
                let handle = request.handle;
                let allocation = self
                    .allocations
                    .iter()
                    .find(|a| a.handle == handle)
                    .ok_or(NvError::BadParameter)?;
                let storage = allocation.storage.as_ref().ok_or(NvError::InvalidState)?;
                let extent = request.extent(allocation.size)?;
                let size = extent.resident;
                let span = extent.span;
                let offset = request.offset;
                // request.extent checked the entire canonical allocation extent;
                // the existing driver remains authoritative for mapping identity.
                if !request.fixed {
                    let alignment = request.alignment(storage.alignment)?;
                    let address = configuration.allocate(
                        size,
                        4096,
                        alignment,
                        space.reservations.iter().map(|v| (v.address, v.size)),
                    )?;
                    let id = checked(self.gpu.reserve_fixed(space.id, GpuAddress(address), size))?;
                    let mapping = match self.gpu.map(space.id, id, storage.backing, offset) {
                        Ok(mapping) => mapping,
                        Err(error) => {
                            // Nothing has executed; release this request's VA
                            // without disturbing any previous mapping.
                            checked(self.gpu.free_reservation(space.id, id))?;
                            return checked(Err(error));
                        }
                    };
                    space.reservations.push(Reservation {
                        address,
                        size,
                        page_size: 4096,
                        id,
                        mappings: vec![BufferMapping {
                            address,
                            size,
                            id: mapping,
                        }],
                        automatic: true,
                        sparse: false,
                    });
                    r.output.copy_from_slice(r.input);
                    out64(r.output, 32, address);
                    return Ok(());
                }
                let address = request.address;
                let end = address.checked_add(span).ok_or(NvError::InvalidSize)?;
                let reservation = space
                    .reservations
                    .iter_mut()
                    .find(|v| address >= v.address && end <= v.address + v.size)
                    .ok_or(NvError::InvalidAddress)?;
                if reservation
                    .mappings
                    .iter()
                    .any(|m| address < m.address + m.size && m.address < end)
                {
                    return Err(NvError::AlreadyAllocated.into());
                }
                configuration.validate_range(address, span, reservation.page_size)?;
                let mapping = checked(self.gpu.map_subrange(
                    space.id,
                    reservation.id,
                    storage.backing,
                    offset,
                    address - reservation.address,
                    size,
                ))?;
                reservation.mappings.push(BufferMapping {
                    address,
                    size: span,
                    id: mapping,
                });
                r.output.copy_from_slice(r.input);
                Ok(())
            }
            0xc0084105 => {
                let index = space
                    .reservations
                    .iter()
                    .position(|v| v.mappings.iter().any(|m| m.address == long(r.input, 0)))
                    .ok_or(NvError::InvalidAddress)?;
                let reservation = &mut space.reservations[index];
                let index_mapping = reservation
                    .mappings
                    .iter()
                    .position(|m| m.address == long(r.input, 0))
                    .ok_or(NvError::InvalidState)?;
                checked(
                    self.gpu
                        .unmap(space.id, reservation.mappings[index_mapping].id),
                )?;
                reservation.mappings.remove(index_mapping);
                if reservation.automatic {
                    checked(self.gpu.free_reservation(space.id, reservation.id))?;
                    space.reservations.remove(index);
                }
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
                if !space.reservations[index].mappings.is_empty() {
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
            0x40084805 | channel::ALLOC_GPFIFO_EX2 => {
                if state.gpfifo {
                    return Err(NvError::AlreadyAllocated.into());
                }
                let entries = channel::capacity(r.request, r.input)?;
                state.gpfifo = true;
                state.entries = entries;
                if r.request == channel::ALLOC_GPFIFO_EX2 {
                    r.output.copy_from_slice(r.input);
                    out32(r.output, 12, state.syncpoint);
                    out32(r.output, 16, state.threshold);
                }
                Ok(())
            }
            0xc018480c => {
                if !state.object {
                    return Err(NvError::InvalidState.into());
                }
                state.notifier_enabled = channel::notifier_enabled(r.input)?;
                r.output.copy_from_slice(r.input);
                Ok(())
            }
            0x4004480d => {
                let priority = word(r.input, 0);
                if !matches!(priority, 0x32 | 0x64 | 0x96) {
                    return Err(NvError::NotSupported.into());
                }
                // Requests execute serially in this adapter. No competing GPU
                // jobs remain to preempt; retain the policy for channel identity.
                state.priority = priority;
                state.timeslice_us = match priority {
                    0x32 => 1300,
                    0x64 => 2600,
                    0x96 => 5200,
                    _ => unreachable!(),
                };
                Ok(())
            }
            0xc004481d => {
                // Serialized execution has no competing runlist to preempt.
                // Retain validated policy without claiming a hardware scheduler.
                state.timeslice_us = channel::timeslice_us(r.input)?;
                r.output.copy_from_slice(r.input);
                Ok(())
            }
            0x40044803 => checked(self.gpu.set_completion_timeout(channel, word(r.input, 0))),
            0x40084714 => {
                // Opaque channel metadata, never dereferenced as a host pointer.
                state.user_data = long(r.input, 0);
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
                let increments = channel::driver_increments(flags, word(r.input, 20))?;
                if word(r.input, 8) > state.entries {
                    return Err(NvError::InvalidSize.into());
                }
                let threshold = state
                    .threshold
                    .checked_add(increments)
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
                // Publication can succeed before a later accepted-prefix fault.
                // Retain its actual image even though the request must halt.
                self.frame = completed_frame;
                if let Err(error) = finish {
                    let message = error.to_string();
                    state.error_event |= state.notifier_enabled;
                    self.fatal = Some(message.clone());
                    return Err(Error::Fatal(message));
                }
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
                if self.files.iter().any(|f|matches!(&f.node,Node::Space(s) if s.reservations.iter().any(|r|!r.mappings.is_empty()))) {
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
