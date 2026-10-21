//! Scoped NVIDIA service-to-host boundary. Command metadata is owned only for
//! this call; large GPU pushbuffers remain in the actual process Memory.
use crate::{
    Memory,
    horizon::{CURRENT_PROCESS, Kernel, Object, PAGE},
    ipc::Request,
};

pub const MAX_METADATA_BYTES: usize = 64 * 1024;
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u32)]
pub enum NvError {
    NotImplemented = 1,
    NotSupported = 2,
    NotInitialized = 3,
    BadParameter = 4,
    InvalidState = 8,
    InvalidAddress = 9,
    InvalidSize = 10,
    BadValue = 11,
    AlreadyAllocated = 13,
    ResourceError = 15,
    CountMismatch = 16,
    IoctlFailed = 0x3000f,
}
pub type NvResult<T> = Result<T, NvError>;
/// Ordinary NVIDIA errors return to the caller. Fatal failures stop guest
/// execution, for example when accepted GPU output cannot safely be published.
#[derive(Debug, PartialEq, Eq)]
pub enum HostError {
    Driver(NvError),
    Fatal(String),
}
impl From<NvError> for HostError {
    fn from(error: NvError) -> Self {
        Self::Driver(error)
    }
}
pub type HostResult<T> = Result<T, HostError>;
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum IoctlKind {
    Ioctl,
    Ioctl2,
    Ioctl3,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
/// Scoped, validated transfer-region metadata; this does not pin guest memory.
/// Hosts must not retain a borrow or infer mapping lifetime from these numbers.
pub struct TransferBuffer {
    pub address: u64,
    pub size: u32,
}

/// Output slices are bounded owned metadata, never borrowed guest storage.
/// Implementations must preserve accepted progress after a partial GPU fault.
/// CPU virtual mappings and permissions must remain stable during this call.
/// Internal owned storage may be consolidated without changing guest addresses.
pub struct Ioctl<'a> {
    pub fd: u32,
    pub request: u32,
    pub kind: IoctlKind,
    pub input: &'a [u8],
    pub extra_input: &'a [u8],
    pub output: &'a mut [u8],
    pub extra_output: &'a mut [u8],
}

/// Installed on one process's Kernel. No guest borrow may outlive a call.
/// A successful ioctl means this backend performed its defined operation; it
/// is not a synthesized GPU fence or implicit promise of deferred visibility.
pub trait Host: std::fmt::Debug {
    fn initialize(&mut self, transfer: TransferBuffer) -> HostResult<()>;
    fn set_client_pid(&mut self, aruid: u64, process_id: u64) -> HostResult<()>;
    fn open(&mut self, path: &str) -> HostResult<u32>;
    fn close(&mut self, fd: u32, memory: &mut Memory) -> HostResult<()>;
    fn ioctl(&mut self, ioctl: Ioctl<'_>, memory: &mut Memory) -> HostResult<()>;
    /// Optional host-only UI update. Retired CPU instructions and elapsed host
    /// time are observations, not the guest's configured clock frequency.
    fn update_progress(&mut self, _instructions: u64, _elapsed_seconds: f64) {}
    /// Validate a device event and report its latched signal state. This is not
    /// a completion query. Implementations may serve diagnostics after a fault.
    fn query_event(&mut self, _fd: u32, _event: u32) -> HostResult<bool> {
        Err(NvError::NotSupported.into())
    }
}

/// Service policy request under the supported, disabled firmware-margin setting.
/// This is not a GPU allocation or a submission-completion event.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct FirmwareMemoryMargin {
    pub requested: bool,
    pub enabled: bool,
}

#[derive(Debug, Default)]
pub struct State {
    host: Option<Box<dyn Host>>,
    initialized: bool,
    fatal: Option<String>,
    firmware_margin: Option<FirmwareMemoryMargin>,
    events: Vec<(u32, u32, usize)>,
}
impl State {
    pub fn install(&mut self, host: Box<dyn Host>) -> NvResult<()> {
        if self.host.is_some() || self.initialized {
            return Err(NvError::InvalidState);
        }
        self.host = Some(host);
        Ok(())
    }
    pub(crate) fn update_progress(&mut self, instructions: u64, elapsed: f64) {
        if let Some(host) = &mut self.host {
            host.update_progress(instructions, elapsed);
        }
    }
    pub fn is_initialized(&self) -> bool {
        self.initialized
    }
    pub fn firmware_memory_margin(&self) -> Option<FirmwareMemoryMargin> {
        self.firmware_margin
    }
    fn invoke<T>(
        &mut self,
        require_initialized: bool,
        operation: impl FnOnce(&mut dyn Host) -> HostResult<T>,
    ) -> HostResult<T> {
        if let Some(message) = &self.fatal {
            return Err(HostError::Fatal(message.clone()));
        }
        if require_initialized && !self.initialized {
            return Err(NvError::NotInitialized.into());
        }
        let host = self.host.as_mut().ok_or(NvError::NotInitialized)?;
        let result = operation(host.as_mut());
        if let Err(HostError::Fatal(message)) = &result {
            self.fatal = Some(message.clone());
        }
        result
    }
    pub fn is_faulted(&self) -> bool {
        self.fatal.is_some()
    }
}

pub(crate) struct Reply {
    pub data: Vec<u8>,
    pub copies: Vec<u32>,
}
fn scalar(result: HostResult<()>) -> Result<Reply, String> {
    let error = match result {
        Ok(()) => 0,
        Err(HostError::Driver(error)) => error as u32,
        Err(HostError::Fatal(message)) => return Err(format!("NVIDIA backend halted: {message}")),
    };
    Ok(Reply {
        data: error.to_le_bytes().to_vec(),
        copies: Vec::new(),
    })
}

fn u32_at(args: &[u8], at: usize) -> NvResult<u32> {
    let bytes = args.get(at..at + 4).ok_or(NvError::InvalidSize)?;
    Ok(u32::from_le_bytes(bytes.try_into().unwrap()))
}
fn pair(a: (u64, u64), b: (u64, u64)) -> NvResult<(u64, u64)> {
    match (a.1 != 0, b.1 != 0) {
        (true, true) => Err(NvError::CountMismatch),
        (true, false) if a.0 != 0 => Ok(a),
        (false, true) if b.0 != 0 => Ok(b),
        (false, false) => Ok((0, 0)),
        _ => Err(NvError::InvalidAddress),
    }
}
fn read_metadata(memory: &Memory, buffer: (u64, u64)) -> NvResult<Vec<u8>> {
    let size = usize::try_from(buffer.1).map_err(|_| NvError::InvalidSize)?;
    if size > MAX_METADATA_BYTES {
        return Err(NvError::InvalidSize);
    }
    let mut bytes = vec![0; size];
    memory
        .read_into(buffer.0, &mut bytes)
        .map_err(|_| NvError::InvalidAddress)?;
    Ok(bytes)
}

fn ioctl(
    memory: &mut Memory,
    state: &mut State,
    req: &Request,
    command: u32,
    args: &[u8],
    tls: u64,
) -> HostResult<()> {
    let (kind, nin, nout) = match command {
        1 => (IoctlKind::Ioctl, 1, 1),
        11 => (IoctlKind::Ioctl2, 2, 1),
        12 => (IoctlKind::Ioctl3, 1, 2),
        _ => return Err(NvError::NotSupported.into()),
    };
    if req.pid.is_some()
        || req.copy_handles != 0
        || req.move_handles != 0
        || req.exchanges != 0
        || req.statics.len() != nin
        || req.sends.len() != nin
        || req.recvs.len() != 2 * nout
        || req.recv_static_mode != nout as u32 + 2
    {
        return Err(NvError::BadParameter.into());
    }
    let fd = u32_at(args, 0)?;
    let request = u32_at(args, 4)?;
    let size = u64::from((request >> 16) & 0x3fff);
    let direction = request >> 30;
    let mut inputs = Vec::with_capacity(nin);
    let mut outputs = Vec::with_capacity(nout);
    let mut total = 0usize;
    for i in 0..nin {
        let buffer = pair(req.statics[i], req.sends[i])?;
        if i == 0 && buffer.1 != if direction & 1 != 0 { size } else { 0 } {
            return Err(NvError::InvalidSize.into());
        }
        total = total
            .checked_add(usize::try_from(buffer.1).map_err(|_| NvError::InvalidSize)?)
            .ok_or(NvError::InvalidSize)?;
        inputs.push(buffer);
    }
    for i in 0..nout {
        let buffer = pair(req.recvs[i], req.recvs[nout + i])?;
        if i == 0 && buffer.1 != if direction & 2 != 0 { size } else { 0 } {
            return Err(NvError::InvalidSize.into());
        }
        total = total
            .checked_add(usize::try_from(buffer.1).map_err(|_| NvError::InvalidSize)?)
            .ok_or(NvError::InvalidSize)?;
        outputs.push(buffer);
    }
    if total > MAX_METADATA_BYTES {
        return Err(NvError::InvalidSize.into());
    }
    // Inputs may alias outputs: all inputs are copied before the host call.
    // Two outputs, or an output and the TLS reply, have no supported ordering.
    let overlaps = |a: (u64, u64), b: (u64, u64)| -> NvResult<bool> {
        let ae = a.0.checked_add(a.1).ok_or(NvError::InvalidAddress)?;
        let be = b.0.checked_add(b.1).ok_or(NvError::InvalidAddress)?;
        Ok(a.1 != 0 && b.1 != 0 && a.0 < be && b.0 < ae)
    };
    for (i, &output) in outputs.iter().enumerate() {
        if overlaps(output, (tls, crate::ipc::MESSAGE_BUFFER_SIZE as u64))?
            || outputs[..i]
                .iter()
                .any(|&other| overlaps(output, other).unwrap_or(true))
        {
            return Err(NvError::BadParameter.into());
        }
    }
    // Validate every destination before any state-changing host operation.
    for &(address, size) in &outputs {
        memory
            .validate_write(address, size as usize)
            .map_err(|_| NvError::InvalidAddress)?;
    }
    let input = read_metadata(memory, inputs[0])?;
    let extra_input = if nin == 2 {
        read_metadata(memory, inputs[1])?
    } else {
        Vec::new()
    };
    let mut output = vec![0; outputs[0].1 as usize];
    let mut extra_output = vec![0; if nout == 2 { outputs[1].1 as usize } else { 0 }];
    state.invoke(true, |host| {
        host.ioctl(
            Ioctl {
                fd,
                request,
                kind,
                input: &input,
                extra_input: &extra_input,
                output: &mut output,
                extra_output: &mut extra_output,
            },
            memory,
        )
    })?;
    // A trusted host must retain mappings; recheck all ranges before any copy.
    for &(address, size) in &outputs {
        memory
            .validate_write(address, size as usize)
            .map_err(|_| NvError::InvalidAddress)?;
    }
    memory
        .write_bytes(outputs[0].0, &output)
        .map_err(|_| NvError::InvalidAddress)?;
    if nout == 2 {
        memory
            .write_bytes(outputs[1].0, &extra_output)
            .map_err(|_| NvError::InvalidAddress)?;
    }
    Ok(())
}

pub(crate) fn request(
    memory: &mut Memory,
    k: &mut Kernel,
    req: &Request,
    command: u32,
    args: &[u8],
) -> Result<Reply, String> {
    if let Some(message) = &k.nvidia.fatal {
        return Err(format!("NVIDIA backend halted: {message}"));
    }
    let no_buffers = req.statics.is_empty()
        && req.sends.is_empty()
        && req.recvs.is_empty()
        && req.exchanges == 0
        && req.recv_static_mode == 0;
    match command {
        3 => {
            if !no_buffers
                || req.pid.is_some()
                || req.copy_handles != 2
                || req.move_handles != 0
                || req.copies.len() != 2
            {
                return Err("unsupported NVIDIA Initialize request layout".into());
            }
            let size = u32_at(args, 0).map_err(|_| "short NVIDIA Initialize payload")?;
            if req.copies[0] != CURRENT_PROCESS {
                return Err("unsupported NVIDIA process handle".into());
            }
            let Some(Object::TransferMemory(index)) = k.handles.get(req.copies[1]) else {
                return Err("invalid NVIDIA transfer memory handle".into());
            };
            let transfer = &k.transfer_memories[index];
            if size == 0 || u64::from(size) % PAGE != 0 || u64::from(size) > transfer.size {
                return Err("invalid NVIDIA transfer memory size".into());
            }
            // Validate the entire guest range, including adjacent host regions.
            // Validation is scoped to this call: no kernel pin/lifetime is implied.
            memory
                .pieces(transfer.address, size as usize, false)
                .map_err(|_| "unsupported NVIDIA transfer memory backing")?;
            memory
                .validate_write(transfer.address, size as usize)
                .map_err(|_| "NVIDIA transfer memory is not writable")?;
            if k.nvidia.initialized {
                return Err("NVIDIA driver already initialized".into());
            }
            if k.nvidia.host.is_none() {
                return Err("unsupported NVIDIA driver: no host backend installed".into());
            }
            let transfer = TransferBuffer {
                address: transfer.address,
                size,
            };
            k.nvidia
                .invoke(false, |host| host.initialize(transfer))
                .map_err(|e| format!("NVIDIA host initialization failed: {e:?}"))?;
            k.nvidia.initialized = true;
            Ok(Reply {
                data: Vec::new(),
                copies: Vec::new(),
            })
        }
        0 => {
            let result = (|| {
                if req.pid.is_some()
                    || req.copy_handles != 0
                    || req.move_handles != 0
                    || req.exchanges != 0
                    || req.recv_static_mode != 0
                    || !req.statics.is_empty()
                    || req.sends.len() != 1
                    || !req.recvs.is_empty()
                {
                    return Err(NvError::BadParameter.into());
                }
                let buffer = req.sends[0];
                if buffer.1 == 0 || buffer.1 > 4096 {
                    return Err(NvError::InvalidSize.into());
                }
                let bytes = read_metadata(memory, buffer)?;
                let bytes = bytes.strip_suffix(&[0]).unwrap_or(&bytes);
                let path = std::str::from_utf8(bytes).map_err(|_| NvError::BadParameter)?;
                if path.is_empty() || path.as_bytes().contains(&0) {
                    return Err(NvError::BadParameter.into());
                }
                k.nvidia.invoke(true, |host| host.open(path))
            })();
            let (fd, error) = match result {
                Ok(fd) => (fd, 0),
                Err(HostError::Driver(e)) => (u32::MAX, e as u32),
                Err(HostError::Fatal(message)) => {
                    return Err(format!("NVIDIA backend halted: {message}"));
                }
            };
            let mut data = fd.to_le_bytes().to_vec();
            data.extend(error.to_le_bytes());
            Ok(Reply {
                data,
                copies: Vec::new(),
            })
        }
        1 | 11 | 12 => {
            let tls = k.threads[k.current].tls;
            let result = ioctl(memory, &mut k.nvidia, req, command, args, tls);
            // Poll only registered diagnostic events, including after a fatal
            // accepted prefix. Use the normal kernel signal path to wake waiters.
            for index in 0..k.nvidia.events.len() {
                let (fd, event, e) = k.nvidia.events[index];
                if k.nvidia
                    .host
                    .as_mut()
                    .is_some_and(|h| h.query_event(fd, event) == Ok(true))
                {
                    crate::threads::signal_event(k, e);
                }
            }
            scalar(result)
        }
        2 => {
            let result = (|| {
                if !no_buffers
                    || req.pid.is_some()
                    || req.copy_handles != 0
                    || req.move_handles != 0
                {
                    return Err(NvError::BadParameter.into());
                }
                let fd = u32_at(args, 0)?;
                k.nvidia.invoke(true, |host| host.close(fd, memory))
            })();
            scalar(result)
        }
        4 => {
            let result = (|| -> HostResult<(usize, bool)> {
                if !no_buffers
                    || req.pid.is_some()
                    || req.copy_handles != 0
                    || req.move_handles != 0
                    || args.len() < 8
                {
                    return Err(NvError::BadParameter.into());
                }
                let fd = u32_at(args, 0)?;
                let event = u32_at(args, 4)?;
                let existing = k
                    .nvidia
                    .events
                    .iter()
                    .position(|&(f, id, _)| f == fd && id == event);
                if existing.is_none() && k.nvidia.events.len() >= 32 {
                    return Err(NvError::ResourceError.into());
                }
                let signaled = k.nvidia.invoke(true, |host| host.query_event(fd, event))?;
                let e = if let Some(index) = existing {
                    k.nvidia.events[index].2
                } else {
                    let e = k.events.len();
                    k.events.push(crate::threads::Event::default());
                    k.nvidia.events.push((fd, event, e));
                    e
                };
                Ok((e, signaled))
            })();
            match result {
                Ok((e, signaled)) => {
                    if signaled {
                        crate::threads::signal_event(k, e);
                    }
                    match crate::threads::readable_event_handle(k, e) {
                        Ok(handle) => Ok(Reply {
                            data: 0u32.to_le_bytes().to_vec(),
                            copies: vec![handle],
                        }),
                        Err(_) => scalar(Err(NvError::ResourceError.into())),
                    }
                }
                Err(e) => scalar(Err(e)),
            }
        }
        8 => {
            if !no_buffers
                || req.pid.is_none()
                || req.copy_handles != 0
                || req.move_handles != 0
                || args.len() < 8
            {
                return Err("unsupported NVIDIA SetClientPID request layout".into());
            }
            let aruid = u64::from_le_bytes(args[..8].try_into().unwrap());
            k.nvidia
                .invoke(true, |host| host.set_client_pid(aruid, k.config.process_id))
                .map_err(|e| format!("NVIDIA SetClientPID failed: {e:?}"))?;
            Ok(Reply {
                data: Vec::new(),
                copies: Vec::new(),
            })
        }
        13 => {
            if !no_buffers
                || req.pid.is_some()
                || req.copy_handles != 0
                || req.move_handles != 0
                || args.len() < 8
            {
                return Err("unsupported NVIDIA firmware-margin request layout".into());
            }
            if !k.nvidia.initialized {
                return Err("NVIDIA firmware-margin request before initialization".into());
            }
            let requested = match u64::from_le_bytes(args[..8].try_into().unwrap()) {
                0 => false,
                1 => true,
                _ => return Err("unsupported NVIDIA firmware-margin value".into()),
            };
            // NV_services oldid=14790: command 13 applies the request through
            // nv!nv_graphics_firmware_memory_margin. System_Settings oldid=14990
            // specifies false by default. Only that disabled system policy is
            // supported here; this does not allocate firmware or complete work.
            k.nvidia.firmware_margin = Some(FirmwareMemoryMargin {
                requested,
                enabled: false,
            });
            Ok(Reply {
                data: Vec::new(),
                copies: Vec::new(),
            })
        }
        _ => Err(format!("unsupported NVIDIA service command {command}")),
    }
}
