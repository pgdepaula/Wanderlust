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
/// They must not mutate the CPU mapping layout during this scoped request.
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
    fn initialize(&mut self, transfer: TransferBuffer) -> NvResult<()>;
    fn set_client_pid(&mut self, aruid: u64, process_id: u64) -> NvResult<()>;
    fn open(&mut self, path: &str) -> NvResult<u32>;
    fn close(&mut self, fd: u32, memory: &mut Memory) -> NvResult<()>;
    fn ioctl(&mut self, ioctl: Ioctl<'_>, memory: &mut Memory) -> NvResult<()>;
}

#[derive(Debug, Default)]
pub struct State {
    host: Option<Box<dyn Host>>,
    initialized: bool,
}
impl State {
    pub fn install(&mut self, host: Box<dyn Host>) -> NvResult<()> {
        if self.host.is_some() || self.initialized {
            return Err(NvError::InvalidState);
        }
        self.host = Some(host);
        Ok(())
    }
    pub fn is_initialized(&self) -> bool {
        self.initialized
    }
    fn ready(&mut self) -> NvResult<&mut (dyn Host + '_)> {
        if !self.initialized {
            return Err(NvError::NotInitialized);
        }
        match self.host.as_mut() {
            Some(host) => Ok(host.as_mut()),
            None => Err(NvError::NotInitialized),
        }
    }
}

pub(crate) struct Reply {
    pub data: Vec<u8>,
}
fn scalar(result: NvResult<()>) -> Reply {
    Reply {
        data: result.err().map_or(0, |e| e as u32).to_le_bytes().to_vec(),
    }
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
) -> NvResult<()> {
    let (kind, nin, nout) = match command {
        1 => (IoctlKind::Ioctl, 1, 1),
        11 => (IoctlKind::Ioctl2, 2, 1),
        12 => (IoctlKind::Ioctl3, 1, 2),
        _ => return Err(NvError::NotSupported),
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
        return Err(NvError::BadParameter);
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
            return Err(NvError::InvalidSize);
        }
        total = total
            .checked_add(usize::try_from(buffer.1).map_err(|_| NvError::InvalidSize)?)
            .ok_or(NvError::InvalidSize)?;
        inputs.push(buffer);
    }
    for i in 0..nout {
        let buffer = pair(req.recvs[i], req.recvs[nout + i])?;
        if i == 0 && buffer.1 != if direction & 2 != 0 { size } else { 0 } {
            return Err(NvError::InvalidSize);
        }
        total = total
            .checked_add(usize::try_from(buffer.1).map_err(|_| NvError::InvalidSize)?)
            .ok_or(NvError::InvalidSize)?;
        outputs.push(buffer);
    }
    if total > MAX_METADATA_BYTES {
        return Err(NvError::InvalidSize);
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
            return Err(NvError::BadParameter);
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
    state.ready()?.ioctl(
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
    )?;
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
            // The currently supported seam requires one readable/writable region.
            // Validation is scoped to this call: no kernel pin/lifetime is implied.
            memory
                .read_bytes(transfer.address, size as usize)
                .map_err(|_| "unsupported NVIDIA transfer memory backing")?;
            memory
                .validate_write(transfer.address, size as usize)
                .map_err(|_| "NVIDIA transfer memory is not writable")?;
            if k.nvidia.initialized {
                return Err("NVIDIA driver already initialized".into());
            }
            let host = k
                .nvidia
                .host
                .as_mut()
                .ok_or("unsupported NVIDIA driver: no host backend installed")?;
            host.initialize(TransferBuffer {
                address: transfer.address,
                size,
            })
            .map_err(|e| format!("NVIDIA host initialization failed: {e:?}"))?;
            k.nvidia.initialized = true;
            Ok(Reply { data: Vec::new() })
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
                    return Err(NvError::BadParameter);
                }
                let buffer = req.sends[0];
                if buffer.1 == 0 || buffer.1 > 4096 {
                    return Err(NvError::InvalidSize);
                }
                let bytes = read_metadata(memory, buffer)?;
                let bytes = bytes.strip_suffix(&[0]).unwrap_or(&bytes);
                let path = std::str::from_utf8(bytes).map_err(|_| NvError::BadParameter)?;
                if path.is_empty() || path.as_bytes().contains(&0) {
                    return Err(NvError::BadParameter);
                }
                k.nvidia.ready()?.open(path)
            })();
            let (fd, error) = match result {
                Ok(fd) => (fd, 0),
                Err(e) => (u32::MAX, e as u32),
            };
            let mut data = fd.to_le_bytes().to_vec();
            data.extend(error.to_le_bytes());
            Ok(Reply { data })
        }
        1 | 11 | 12 => {
            let tls = k.threads[k.current].tls;
            Ok(scalar(ioctl(
                memory,
                &mut k.nvidia,
                req,
                command,
                args,
                tls,
            )))
        }
        2 => {
            let result = (|| {
                if !no_buffers
                    || req.pid.is_some()
                    || req.copy_handles != 0
                    || req.move_handles != 0
                {
                    return Err(NvError::BadParameter);
                }
                k.nvidia.ready()?.close(u32_at(args, 0)?, memory)
            })();
            Ok(scalar(result))
        }
        4 => {
            // No invented readable event or permanently signalled completion.
            // Real syncpoint/event support needs an explicit backend/kernel contract.
            Ok(scalar(Err(NvError::NotSupported)))
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
                .ready()
                .and_then(|host| host.set_client_pid(aruid, k.config.process_id))
                .map_err(|e| format!("NVIDIA SetClientPID failed: {e:?}"))?;
            Ok(Reply { data: Vec::new() })
        }
        _ => Err(format!("unsupported NVIDIA service command {command}")),
    }
}
