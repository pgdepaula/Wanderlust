//! Checked mono/stereo hwopus service, using the system libopus decoder.
//! IPC: libnx feebd026 hwopus.c/h. No guest address becomes a native pointer.
use std::{
    collections::BTreeMap,
    ffi::{c_char, c_int, c_void},
    ptr::NonNull,
    sync::OnceLock,
};
use wl_guest_runtime::{
    Memory, RuntimeResult,
    audio::Reply,
    horizon::{Kernel, Object},
    ipc::Request,
    services::HostObject,
};

pub const MANAGER: usize = 1 << 30;
const MAX_DECODERS: usize = 256;
const MAX_LIFETIMES: usize = 4096;
type GetSize = unsafe extern "C" fn(c_int) -> c_int;
type Create = unsafe extern "C" fn(c_int, c_int, *mut c_int) -> *mut c_void;
type Destroy = unsafe extern "C" fn(*mut c_void);
type Decode = unsafe extern "C" fn(*mut c_void, *const u8, c_int, *mut i16, c_int, c_int) -> c_int;
type Ctl = unsafe extern "C" fn(*mut c_void, c_int, ...) -> c_int;
struct Api {
    _library: usize,
    size: GetSize,
    create: Create,
    destroy: Destroy,
    decode: Decode,
    ctl: Ctl,
}
static API: OnceLock<RuntimeResult<Api>> = OnceLock::new();

#[cfg(unix)]
unsafe extern "C" {
    fn dlopen(name: *const c_char, flags: c_int) -> *mut c_void;
    fn dlsym(handle: *mut c_void, name: *const c_char) -> *mut c_void;
    fn dlclose(handle: *mut c_void) -> c_int;
}
#[cfg(windows)]
#[link(name = "kernel32")]
unsafe extern "system" {
    fn LoadLibraryExW(name: *const u16, file: *mut c_void, flags: u32) -> *mut c_void;
    fn GetProcAddress(handle: *mut c_void, name: *const c_char) -> *mut c_void;
    fn FreeLibrary(handle: *mut c_void) -> c_int;
}
fn open_library() -> RuntimeResult<usize> {
    #[cfg(unix)]
    for name in [c"libopus.so.0", c"libopus.dylib"] {
        // SAFETY: fixed NUL-terminated host library names, no guest input.
        let p = unsafe { dlopen(name.as_ptr(), 2) };
        if !p.is_null() {
            return Ok(p as usize);
        }
    }
    #[cfg(windows)]
    {
        use std::os::windows::ffi::OsStrExt;
        let executable = std::env::current_exe().map_err(|e| e.to_string())?;
        for name in ["opus.dll", "libopus-0.dll"] {
            let Some(directory) = executable.parent() else {
                break;
            };
            let Ok(path) = directory.join(name).canonicalize() else {
                continue;
            };
            let mut wide: Vec<u16> = path.as_os_str().encode_wide().collect();
            if wide.contains(&0) {
                continue;
            }
            wide.push(0);
            // SAFETY: absolute application-local path. CWD/PATH search is disabled.
            let p = unsafe { LoadLibraryExW(wide.as_ptr(), std::ptr::null_mut(), 0x100 | 0x1000) };
            if !p.is_null() {
                return Ok(p as usize);
            }
        }
    }
    Err("hwopus requires system libopus (Windows: application-local opus.dll)".into())
}
fn symbol(handle: usize, name: &std::ffi::CStr) -> RuntimeResult<*mut c_void> {
    #[cfg(unix)]
    // SAFETY: handle came from dlopen and stays live for the process lifetime.
    let p = unsafe { dlsym(handle as *mut c_void, name.as_ptr()) };
    #[cfg(windows)]
    // SAFETY: handle came from LoadLibraryExW and stays live for the process lifetime.
    let p = unsafe { GetProcAddress(handle as *mut c_void, name.as_ptr()) };
    #[cfg(not(any(unix, windows)))]
    let p = {
        let _ = (handle, name);
        std::ptr::null_mut::<c_void>()
    };
    if p.is_null() {
        Err(format!("libopus missing {}", name.to_string_lossy()))
    } else {
        Ok(p)
    }
}
fn api() -> RuntimeResult<&'static Api> {
    API.get_or_init(|| {
        let handle = open_library()?;
        let result = (|| {
            // SAFETY: these exact public libopus symbols have the declared C signatures.
            Ok(unsafe {
                Api {
                    _library: handle,
                    size: std::mem::transmute::<*mut c_void, GetSize>(symbol(
                        handle,
                        c"opus_decoder_get_size",
                    )?),
                    create: std::mem::transmute::<*mut c_void, Create>(symbol(
                        handle,
                        c"opus_decoder_create",
                    )?),
                    destroy: std::mem::transmute::<*mut c_void, Destroy>(symbol(
                        handle,
                        c"opus_decoder_destroy",
                    )?),
                    decode: std::mem::transmute::<*mut c_void, Decode>(symbol(
                        handle,
                        c"opus_decode",
                    )?),
                    ctl: std::mem::transmute::<*mut c_void, Ctl>(symbol(
                        handle,
                        c"opus_decoder_ctl",
                    )?),
                }
            })
        })();
        if result.is_err() {
            #[cfg(unix)]
            // SAFETY: initialization failed before any decoder could use the library.
            unsafe {
                dlclose(handle as *mut c_void);
            }
            #[cfg(windows)]
            // SAFETY: initialization failed before any decoder could use the library.
            unsafe {
                FreeLibrary(handle as *mut c_void);
            }
        }
        result
    })
    .as_ref()
    .map_err(Clone::clone)
}
fn word(b: &[u8], at: usize) -> u32 {
    u32::from_le_bytes(b[at..at + 4].try_into().unwrap())
}
fn parameters(args: &[u8]) -> RuntimeResult<(u32, u32)> {
    if args.len() < 8 {
        return Err("truncated hwopus parameters".into());
    }
    let rate = word(args, 0);
    let channels = word(args, 4);
    if !matches!(rate, 8000 | 12000 | 16000 | 24000 | 48000) || !matches!(channels, 1 | 2) {
        return Err("unsupported hwopus sample rate or channel count".into());
    }
    Ok((rate, channels))
}
pub fn work_size(channels: u32) -> RuntimeResult<u32> {
    if !matches!(channels, 1 | 2) {
        return Err("unsupported hwopus channel count".into());
    }
    // SAFETY: validated channel count; the API has the public libopus signature.
    let bytes = unsafe { (api()?.size)(channels as c_int) };
    if !(1..=1024 * 1024).contains(&bytes) {
        return Err("invalid libopus decoder size".into());
    }
    Ok(bytes as u32 + 4096)
}
pub struct Decoder {
    pointer: NonNull<c_void>,
    api: &'static Api,
    rate: u32,
    channels: u32,
    bytes: usize,
    frame_ms: u32,
}
// SAFETY: libopus decoders have no thread affinity. This decoder is exclusively
// owned; decoding/resetting requires &mut self and no native state pointer escapes.
unsafe impl Send for Decoder {}
impl std::fmt::Debug for Decoder {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("OpusDecoder")
            .field("rate", &self.rate)
            .field("channels", &self.channels)
            .finish()
    }
}
impl Decoder {
    pub fn new(rate: u32, channels: u32) -> RuntimeResult<Self> {
        parameters(&[rate.to_le_bytes(), channels.to_le_bytes()].concat())?;
        let api = api()?;
        let bytes = (work_size(channels)? - 4096) as usize;
        let mut error = 0;
        // SAFETY: validated arguments and initialized writable result storage.
        let raw = unsafe { (api.create)(rate as c_int, channels as c_int, &mut error) };
        let pointer =
            NonNull::new(raw).ok_or_else(|| format!("libopus initialization failed: {error}"))?;
        if error != 0 {
            // SAFETY: non-null state returned by this library, not exposed elsewhere.
            unsafe { (api.destroy)(pointer.as_ptr()) };
            return Err(format!("libopus initialization failed: {error}"));
        }
        Ok(Self {
            pointer,
            api,
            rate,
            channels,
            bytes,
            frame_ms: 40,
        })
    }
    pub fn decode_to(
        &mut self,
        memory: &mut Memory,
        input: (u64, u64),
        output: (u64, u64),
        reset: bool,
    ) -> RuntimeResult<(u32, u32)> {
        if !(8..=65544).contains(&input.1) || output.1 > 1024 * 1024 {
            return Err("hwopus buffer budget".into());
        }
        let capacity = (output.1 / (2 * u64::from(self.channels)))
            .min(u64::from(self.rate) * u64::from(self.frame_ms) / 1000)
            as usize;
        if capacity == 0 {
            return Err("hwopus PCM output is too short".into());
        }
        memory.validate_write(output.0, output.1 as usize)?;
        let mut packet = vec![0; input.1 as usize];
        memory.read_into(input.0, &mut packet)?;
        let bytes = u32::from_be_bytes(packet[..4].try_into().unwrap()) as usize;
        let range = u32::from_be_bytes(packet[4..8].try_into().unwrap());
        if bytes > packet.len() - 8 {
            return Err("truncated hwopus packet".into());
        }
        let mut snapshot = vec![std::mem::MaybeUninit::<u8>::uninit(); self.bytes];
        // SAFETY: libopus documents a contiguous, shallow-copyable decoder state
        // of exactly decoder_get_size bytes. MaybeUninit preserves any C padding
        // without asserting that every byte is an initialized Rust u8 value.
        // Source and destination are distinct live host allocations, never guest memory.
        unsafe {
            std::ptr::copy_nonoverlapping(
                self.pointer.as_ptr().cast::<std::mem::MaybeUninit<u8>>(),
                snapshot.as_mut_ptr(),
                self.bytes,
            );
        }
        let result = (|| {
            if reset {
                // SAFETY: exclusive live decoder; OPUS_RESET_STATE takes no variadic arguments.
                let result = unsafe { (self.api.ctl)(self.pointer.as_ptr(), 4028) };
                if result != 0 {
                    return Err(format!("libopus reset failed: {result}"));
                }
            }
            let mut pcm = vec![0i16; capacity * self.channels as usize];
            // SAFETY: owned packet/PCM allocations cover their declared extents;
            // the live decoder is exclusive and never points into guest memory.
            let samples = unsafe {
                (self.api.decode)(
                    self.pointer.as_ptr(),
                    packet[8..].as_ptr(),
                    bytes as c_int,
                    pcm.as_mut_ptr(),
                    capacity as c_int,
                    0,
                )
            };
            if samples < 0 || samples as usize > capacity {
                return Err(format!("libopus decode failed: {samples}"));
            }
            if bytes != 0 && range != 0 {
                let mut actual = 0u32;
                // SAFETY: OPUS_GET_FINAL_RANGE expects a writable uint32 pointer.
                let result =
                    unsafe { (self.api.ctl)(self.pointer.as_ptr(), 4031, &mut actual as *mut u32) };
                if result != 0 || actual != range {
                    return Err("hwopus packet final range mismatch".into());
                }
            }
            let mut encoded = Vec::with_capacity(samples as usize * self.channels as usize * 2);
            for value in &pcm[..samples as usize * self.channels as usize] {
                encoded.extend(value.to_le_bytes());
            }
            memory.write_bytes(output.0, &encoded)?;
            Ok(((8 + bytes) as u32, samples as u32))
        })();
        if result.is_err() {
            // SAFETY: restoring the same sized host state to its still-live allocation.
            unsafe {
                std::ptr::copy_nonoverlapping(
                    snapshot.as_ptr(),
                    self.pointer.as_ptr().cast::<std::mem::MaybeUninit<u8>>(),
                    self.bytes,
                )
            };
        }
        result
    }
}
impl Drop for Decoder {
    fn drop(&mut self) {
        // SAFETY: this exclusively owned allocation was created by the same API.
        unsafe { (self.api.destroy)(self.pointer.as_ptr()) };
    }
}

#[derive(Debug, Default)]
pub struct Service {
    decoders: BTreeMap<usize, Decoder>,
    next_id: usize,
}
impl Service {
    pub fn close(&mut self, index: usize) {
        self.decoders.remove(&index);
    }
    pub fn request(
        &mut self,
        memory: &mut Memory,
        k: &mut Kernel,
        index: usize,
        command: u32,
        req: &Request,
        args: &[u8],
    ) -> RuntimeResult<Reply> {
        if req.pid.is_some() || req.move_handles != 0 || req.exchanges != 0 {
            return Err("unsupported hwopus request metadata".into());
        }
        if index == MANAGER {
            if !matches!(command, 0 | 1 | 4 | 5) {
                return Err(format!("unsupported hwopus manager command {command}"));
            }
            if !req.sends.is_empty() || !req.statics.is_empty() || !req.recvs.is_empty() {
                return Err("unsupported hwopus manager buffers".into());
            }
            let (rate, channels) = parameters(args)?;
            let size_offset = if matches!(command, 4 | 5) {
                if args.len() < 16 || word(args, 8) & !1 != 0 || word(args, 12) != 0 {
                    return Err("unsupported hwopus extended flags or reserved fields".into());
                }
                16
            } else {
                8
            };
            let size = work_size(channels)?;
            return match command {
                1 | 5 if req.copy_handles == 0 => Ok(Reply::data(&size.to_le_bytes())),
                0 | 4 if req.copy_handles == 1 && args.len() >= size_offset + 4 => {
                    let Some(Object::TransferMemory(t)) = k.handles.get(req.copies[0]) else {
                        return Err("hwopus requires transfer memory".into());
                    };
                    let supplied = word(args, size_offset);
                    if supplied < size || u64::from(supplied) > k.transfer_memories[t].size {
                        return Err("hwopus work buffer is too short".into());
                    }
                    if self.decoders.len() >= MAX_DECODERS || self.next_id >= MAX_LIFETIMES {
                        return Err("hwopus decoder capacity".into());
                    }
                    let mut decoder = Decoder::new(rate, channels)?;
                    if size_offset == 16 && word(args, 8) == 1 {
                        decoder.frame_ms = 120;
                    }
                    self.next_id += 1;
                    let id = MANAGER + self.next_id;
                    self.decoders.insert(id, decoder);
                    Ok(Reply::object(HostObject::AudioRenderer { index: id }))
                }
                _ => Err(format!("unsupported hwopus manager command {command}")),
            };
        }
        if req.copy_handles != 0
            || req.sends.len() != 1
            || req.recvs.len() != 1
            || !req.statics.is_empty()
            || req.recv_static_mode != 0
        {
            return Err("unsupported hwopus decode descriptors".into());
        }
        if !matches!(command, 0 | 4 | 6 | 8) {
            return Err(format!("unsupported hwopus decoder command {command}"));
        }
        let reset = if matches!(command, 6 | 8) {
            match args.first() {
                Some(0) => false,
                Some(1) => true,
                _ => return Err("invalid hwopus reset flag".into()),
            }
        } else {
            false
        };
        let decoder = self
            .decoders
            .get_mut(&index)
            .ok_or("stale hwopus decoder")?;
        let (consumed, samples) = decoder.decode_to(memory, req.sends[0], req.recvs[0], reset)?;
        let mut out = Vec::with_capacity(16);
        out.extend(consumed.to_le_bytes());
        out.extend(samples.to_le_bytes());
        if command != 0 {
            out.extend(0u64.to_le_bytes());
        } // Software backend reports no DSP performance clock.
        Ok(Reply::data(&out))
    }
}
