//! Empty cache-save enumeration, for a process without a configured cache store.
//! libnx feebd026 fsSaveDataInfoReaderRead; Ryujinx 4930dced command 62.
use wl_guest_runtime::{
    Cpu, Memory, Process, RuntimeResult,
    audio::Reply,
    horizon::Object,
    ipc::{Endpoint, Request},
    services::HostObject,
};
pub const MANAGER: usize = 1 << 29;
const READER: usize = MANAGER + 1;
const EMPTY_FILESYSTEM: usize = MANAGER + 2;
const APPLICATION: usize = MANAGER + 3;
const LIMIT: usize = MANAGER + 0x10000;
pub fn owns(index: usize) -> bool {
    (MANAGER..LIMIT).contains(&index)
}
pub fn request(
    memory: &Memory,
    index: usize,
    command: u32,
    req: &Request,
    args: &[u8],
) -> RuntimeResult<Reply> {
    if index == EMPTY_FILESYSTEM {
        return empty_open(memory, command, req, args);
    }
    if req.pid.is_some()
        || req.copy_handles != 0
        || req.move_handles != 0
        || req.exchanges != 0
        || !req.statics.is_empty()
        || !req.sends.is_empty()
        || req.recv_static_mode != 0
    {
        return Err("unsupported cache-save reader metadata".into());
    }
    match (index, command) {
        (MANAGER, 62) if req.recvs.is_empty() => {
            Ok(Reply::object(HostObject::AudioRenderer { index: READER }))
        }
        (READER, 0) if req.recvs.len() == 1 => {
            let (address, size) = req.recvs[0];
            if size > 1024 * 1024 || size % 0x60 != 0 {
                return Err("invalid cache-save info output size".into());
            }
            memory.validate_write(address, size as usize)?;
            // No cache store is mounted, so the real list is empty. Preserve all
            // destination bytes: no entries were returned, not zero-filled entries.
            Ok(Reply::data(&0i64.to_le_bytes()))
        }
        _ => Err(format!("unsupported cache-save reader command {command}")),
    }
}
// Existing FileSystem objects represent unconfigured save/SD stores, never the
// mounted RomFS (which has its own RomFsStorage object). Missing paths must return
// the FS error, not successful replies with null IFile/IDirectory objects.
fn empty_open(memory: &Memory, command: u32, req: &Request, args: &[u8]) -> RuntimeResult<Reply> {
    if !matches!(command, 7..=9)
        || req.pid.is_some()
        || req.copy_handles != 0
        || req.move_handles != 0
        || req.exchanges != 0
        || req.statics.len() != 1
        || !req.sends.is_empty()
        || !req.recvs.is_empty()
        || req.recv_static_mode != 0
    {
        return Err("unsupported unconfigured filesystem request".into());
    }
    if command != 7 {
        let mode = u32_at(args, 0).ok_or("truncated filesystem open mode")?;
        let allowed = if command == 8 { 7 } else { 0x80000003 };
        if mode == 0 || mode & !allowed != 0 {
            return Err("invalid filesystem open mode".into());
        }
    }
    let (address, size) = req.statics[0];
    if size == 0 || size > 0x301 {
        return Err("invalid filesystem path buffer size".into());
    }
    let mut bytes = vec![0; size as usize];
    memory.read_into(address, &mut bytes)?;
    let end = bytes
        .iter()
        .position(|b| *b == 0)
        .ok_or("unterminated filesystem path")?;
    if end == 0 || bytes[0] != b'/' {
        return Err("filesystem path is not absolute".into());
    }
    if std::env::var_os("WL_FS_TRACE").is_some() {
        eprintln!(
            "[fs] unconfigured store cmd={command} path={}",
            String::from_utf8_lossy(&bytes[..end])
        );
    }
    // libnx/Ryujinx ResultFs.PathNotFound: module 2, description 1.
    Ok(Reply::failure((1 << 9) | 2))
}

enum Location {
    Session(usize, Endpoint),
    Domain(usize, usize, HostObject),
}
pub struct Binding {
    location: Option<Location>,
    swap_ids: bool,
}
fn u32_at(b: &[u8], at: usize) -> Option<u32> {
    Some(u32::from_le_bytes(b.get(at..at + 4)?.try_into().ok()?))
}
/// Classify only this extension's calls. The stock kernel still validates HIPC,
/// CMIF, buffers, domain framing and handles before executing a hosted request.
pub fn bind(cpu: &Cpu, process: &mut Process, svc: u16) -> Option<Binding> {
    if svc != 0x21 {
        return None;
    }
    let k = process.kernel.as_ref()?;
    let Object::Session(session) = k.handles.get(cpu.x[0] as u32)? else {
        return None;
    };
    let s = k.sessions.get(session)?;
    if s.closed || s.references == 0 {
        return None;
    }
    let candidate = match s.endpoint {
        Endpoint::Host(
            HostObject::FileSystemProxy
            | HostObject::FileSystem
            | HostObject::ApplicationFunctions
            | HostObject::AppletOe
            | HostObject::ApplicationProxy,
        ) => true,
        Endpoint::Host(HostObject::AudioRenderer { index }) => owns(index),
        _ => false,
    };
    if !candidate {
        return None;
    }
    let mut b = [0u8; 256];
    process
        .memory
        .read_into(k.threads.get(k.current)?.tls, &mut b)
        .ok()?;
    let w0 = u32_at(&b, 0)?;
    let w1 = u32_at(&b, 4)?;
    let kind = w0 as u16;
    if !matches!(kind, 4..=7) {
        return None;
    }
    let control = matches!(kind, 5 | 7);
    let mut at = 8usize;
    if w1 >> 31 != 0 {
        let h = u32_at(&b, 8)?;
        at = 12 + if h & 1 != 0 { 8 } else { 0 } + 4 * (((h >> 1) & 15) + ((h >> 5) & 15)) as usize;
    }
    at += 8 * ((w0 >> 16) & 15) as usize
        + 12 * (((w0 >> 20) & 15) + ((w0 >> 24) & 15) + (w0 >> 28)) as usize;
    let end = at.checked_add(4 * (w1 & 0x3ff) as usize)?;
    let raw = at.checked_add(15)? & !15;
    if end > 256 || raw + 16 > end {
        return None;
    }
    let (target, cmif, location, closing) = if let Some(domain) = s.domain.filter(|_| !control) {
        if !matches!(b[raw], 1 | 2) || b[raw + 1] != 0 {
            return None;
        }
        let id = u32_at(&b, raw + 4)?;
        let objects = &k.host.domains.get(domain)?.objects;
        let index = objects.iter().position(|(found, _)| *found == id)?;
        (
            objects[index].1,
            raw + 16,
            Location::Domain(domain, index, objects[index].1),
            b[raw] == 2,
        )
    } else {
        let Endpoint::Host(target) = s.endpoint else {
            return None;
        };
        (target, raw, Location::Session(session, s.endpoint), false)
    };
    let reader = matches!(target, HostObject::AudioRenderer { index } if owns(index));
    let opening = !control
        && !closing
        && target == HostObject::FileSystemProxy
        && cmif + 16 <= end
        && u32_at(&b, cmif)? == u32::from_le_bytes(*b"SFCI")
        && (u32_at(&b, cmif + 8)? == 62
            || (matches!(u32_at(&b, cmif + 8)?, 51 | 53)
                && cmif + 16 + 72 <= end
                && b[cmif + 16 + 40] == 5));
    let filesystem = !control
        && !closing
        && target == HostObject::FileSystem
        && cmif + 16 <= end
        && u32_at(&b, cmif)? == u32::from_le_bytes(*b"SFCI")
        && matches!(u32_at(&b, cmif + 8)?, 7..=9);
    let application = !control
        && !closing
        && target == HostObject::ApplicationFunctions
        && cmif + 16 <= end
        && u32_at(&b, cmif)? == u32::from_le_bytes(*b"SFCI")
        && u32_at(&b, cmif + 8)? == 27;
    if !reader && !opening && !filesystem && !application {
        return None;
    }
    let k = process.kernel.as_mut()?;
    // The opaque callback transport uses an audio tag, but these objects belong
    // to FS. Scope the existing ID allocator to FS for creation and domain close,
    // preventing collisions and avoiding a second domain/handle implementation.
    if !application {
        std::mem::swap(&mut k.host.fs_domain_ids, &mut k.host.audio_domain_ids);
    }
    let location = if opening || filesystem || application {
        let target = HostObject::AudioRenderer {
            index: if application {
                APPLICATION
            } else if filesystem {
                EMPTY_FILESYSTEM
            } else {
                MANAGER
            },
        };
        match location {
            Location::Session(s, _) => k.sessions[s].endpoint = Endpoint::Host(target),
            Location::Domain(d, i, _) => k.host.domains[d].objects[i].1 = target,
        }
        Some(location)
    } else {
        None
    };
    Some(Binding {
        location,
        swap_ids: !application,
    })
}
pub fn restore(process: &mut Process, binding: Option<Binding>) {
    let Some(binding) = binding else { return };
    let Some(k) = process.kernel.as_mut() else {
        return;
    };
    if binding.swap_ids {
        std::mem::swap(&mut k.host.fs_domain_ids, &mut k.host.audio_domain_ids);
    }
    match binding.location {
        Some(Location::Session(s, original)) => {
            if let Some(s) = k.sessions.get_mut(s) {
                s.endpoint = original
            }
        }
        Some(Location::Domain(d, i, original)) => {
            if let Some(d) = k.host.domains.get_mut(d)
                && let Some((_, object)) = d.objects.get_mut(i)
            {
                *object = original
            }
        }
        None => {}
    }
}

use std::collections::{BTreeMap, BTreeSet};
use wl_guest_runtime::horizon::Kernel;
const MAX_CACHE_BYTES: u64 = 128 * 1024 * 1024;
const MAX_TRANSFER: u64 = 16 * 1024 * 1024;
#[derive(Debug)]
struct Volume {
    quota: u64,
    journal: u64,
    files: BTreeMap<String, Vec<u8>>,
    directories: BTreeSet<String>,
}
impl Volume {
    fn used(&self) -> u64 {
        self.files.values().map(|b| b.len() as u64).sum()
    }
}
#[derive(Clone, Debug)]
enum CacheObject {
    Mount {
        volume: u16,
        readonly: bool,
    },
    File {
        volume: u16,
        path: String,
        mode: u32,
    },
    Directory {
        entries: Vec<Vec<u8>>,
        cursor: usize,
    },
    Reader {
        indices: Vec<u16>,
        cursor: usize,
    },
}
/// Re-creatable per-process RAM cache, not durable save storage or NAND journaling.
/// Limits are implementation budgets; no host path or host pointer is exposed.
#[derive(Debug)]
pub struct Service {
    volumes: BTreeMap<u16, Volume>,
    objects: BTreeMap<usize, CacheObject>,
    next: usize,
}
impl Default for Service {
    fn default() -> Self {
        Self {
            volumes: BTreeMap::new(),
            objects: BTreeMap::new(),
            next: MANAGER + 0x100,
        }
    }
}
fn u64_at(b: &[u8], at: usize) -> RuntimeResult<u64> {
    Ok(u64::from_le_bytes(
        b.get(at..at + 8)
            .ok_or("truncated cache scalar")?
            .try_into()
            .unwrap(),
    ))
}
fn simple(req: &Request) -> RuntimeResult<()> {
    if req.pid.is_some()
        || req.copy_handles != 0
        || req.move_handles != 0
        || req.exchanges != 0
        || req.recv_static_mode != 0
    {
        Err("unsupported cache request metadata".into())
    } else {
        Ok(())
    }
}
fn no_buffers(req: &Request) -> RuntimeResult<()> {
    simple(req)?;
    if !req.statics.is_empty() || !req.sends.is_empty() || !req.recvs.is_empty() {
        Err("unexpected cache buffers".into())
    } else {
        Ok(())
    }
}
fn path(memory: &Memory, req: &Request) -> RuntimeResult<String> {
    simple(req)?;
    if req.statics.len() != 1 || !req.sends.is_empty() || !req.recvs.is_empty() {
        return Err("invalid cache path descriptors".into());
    }
    let (address, size) = req.statics[0];
    if size == 0 || size > 0x301 {
        return Err("invalid cache path size".into());
    }
    let mut b = vec![0; size as usize];
    memory.read_into(address, &mut b)?;
    let end = b
        .iter()
        .position(|b| *b == 0)
        .ok_or("unterminated cache path")?;
    let p = std::str::from_utf8(&b[..end]).map_err(|_| "cache path is not UTF-8")?;
    if !p.starts_with('/') || p.contains('\\') || p.split('/').any(|c| c == ".." || c == ".") {
        return Err("invalid absolute cache path".into());
    }
    Ok(if p == "/" {
        p.into()
    } else {
        p.trim_end_matches('/').into()
    })
}
fn parent(p: &str) -> &str {
    let at = p.rfind('/').unwrap_or(0);
    if at == 0 { "/" } else { &p[..at] }
}
fn resize(data: &mut Vec<u8>, size: usize) -> RuntimeResult<()> {
    if size > data.len() {
        data.try_reserve_exact(size - data.len())
            .map_err(|_| "cache allocation failed")?;
    }
    data.resize(size, 0);
    Ok(())
}
impl Service {
    fn object(&mut self, object: CacheObject) -> RuntimeResult<Reply> {
        if self.next >= LIMIT || self.objects.len() >= 1024 {
            return Err("cache object lifetime budget".into());
        }
        let index = self.next;
        self.next += 1;
        self.objects.insert(index, object);
        Ok(Reply::object(HostObject::AudioRenderer { index }))
    }
    pub fn close(&mut self, index: usize) {
        self.objects.remove(&index);
    }
    pub fn request(
        &mut self,
        memory: &mut Memory,
        kernel: &Kernel,
        index: usize,
        command: u32,
        req: &Request,
        args: &[u8],
    ) -> RuntimeResult<Reply> {
        if index == APPLICATION {
            no_buffers(req)?;
            if command != 27 || args.len() < 24 {
                return Err("invalid CreateCacheStorage request".into());
            }
            let number = u16::from_le_bytes(args[..2].try_into().unwrap());
            let size = u64_at(args, 8)?;
            let journal = u64_at(args, 16)?;
            let required = size.checked_add(journal).ok_or("cache size overflow")?;
            let reserved: u64 = self.volumes.values().map(|v| v.quota + v.journal).sum();
            if size == 0
                || required > MAX_CACHE_BYTES
                || self.volumes.len() >= 16
                || reserved
                    .checked_add(required)
                    .is_none_or(|n| n > MAX_CACHE_BYTES)
            {
                return Err("CreateCacheStorage exceeds RAM-cache budget".into());
            }
            if self.volumes.contains_key(&number) {
                return Ok(Reply::failure(0x402));
            }
            self.volumes.insert(
                number,
                Volume {
                    quota: size,
                    journal,
                    files: BTreeMap::new(),
                    directories: BTreeSet::from(["/".into()]),
                },
            );
            let mut out = [0; 16];
            out[..4].copy_from_slice(&1u32.to_le_bytes());
            return Ok(Reply::data(&out));
        }
        if index == MANAGER {
            no_buffers(req)?;
            return match command {
                62 => self.object(CacheObject::Reader {
                    indices: self.volumes.keys().copied().collect(),
                    cursor: 0,
                }),
                51 | 53 => {
                    if args.len() < 72
                        || args[40] != 5
                        || args[41] != 0
                        || !matches!(args[0], 1 | 4)
                    {
                        return Err("unsupported cache mount attribute".into());
                    }
                    let application = u64_at(args, 8)?;
                    if application != 0 && application != kernel.config.program_id {
                        return Err("cache belongs to another application".into());
                    }
                    if args[16..40].iter().any(|b| *b != 0) {
                        return Err("unsupported cache UID/system ID".into());
                    }
                    let volume = u16::from_le_bytes(args[42..44].try_into().unwrap());
                    if !self.volumes.contains_key(&volume) {
                        Ok(Reply::failure((1002 << 9) | 2))
                    } else {
                        self.object(CacheObject::Mount {
                            volume,
                            readonly: command == 53,
                        })
                    }
                }
                _ => Err(format!("unsupported cache manager command {command}")),
            };
        }
        if index == EMPTY_FILESYSTEM {
            return request(memory, index, command, req, args);
        }
        let object = self
            .objects
            .get(&index)
            .cloned()
            .ok_or("closed cache object")?;
        match object {
            CacheObject::Mount { volume, readonly } => {
                self.mount(memory, volume, readonly, command, req, args)
            }
            CacheObject::File { volume, path, mode } => Self::file(
                memory,
                self.volumes
                    .get_mut(&volume)
                    .ok_or("unmounted cache volume")?,
                &path,
                mode,
                command,
                req,
                args,
            ),
            CacheObject::Reader { indices, cursor } => {
                simple(req)?;
                if command != 0
                    || req.recvs.len() != 1
                    || !req.sends.is_empty()
                    || !req.statics.is_empty()
                {
                    return Err("invalid cache reader request".into());
                }
                let (address, size) = req.recvs[0];
                if size > 1024 * 1024 || size % 0x60 != 0 {
                    return Err("invalid cache reader output size".into());
                }
                memory.validate_write(address, size as usize)?;
                let n = (size as usize / 0x60).min(indices.len() - cursor);
                let mut out = vec![0; n * 0x60];
                for (dst, number) in out.chunks_exact_mut(0x60).zip(&indices[cursor..cursor + n]) {
                    let v = &self.volumes[number];
                    dst[..8].copy_from_slice(&(u64::from(*number) + 1).to_le_bytes());
                    dst[8] = 1;
                    dst[9] = 5;
                    dst[40..48].copy_from_slice(&kernel.config.program_id.to_le_bytes());
                    dst[48..56].copy_from_slice(&(v.quota + v.journal).to_le_bytes());
                    dst[56..58].copy_from_slice(&number.to_le_bytes());
                }
                memory.write_bytes(address, &out)?;
                if let Some(CacheObject::Reader { cursor, .. }) = self.objects.get_mut(&index) {
                    *cursor += n;
                }
                Ok(Reply::data(&(n as i64).to_le_bytes()))
            }
            CacheObject::Directory { entries, cursor } => {
                if command == 1 {
                    no_buffers(req)?;
                    return Ok(Reply::data(&(entries.len() as i64).to_le_bytes()));
                }
                simple(req)?;
                if command != 0
                    || req.recvs.len() != 1
                    || !req.sends.is_empty()
                    || !req.statics.is_empty()
                {
                    return Err("invalid directory read".into());
                }
                let (address, size) = req.recvs[0];
                if size > MAX_TRANSFER || size % 0x310 != 0 {
                    return Err("invalid directory output size".into());
                }
                memory.validate_write(address, size as usize)?;
                let n = (size as usize / 0x310).min(entries.len() - cursor);
                let mut out = Vec::with_capacity(n * 0x310);
                for entry in &entries[cursor..cursor + n] {
                    out.extend_from_slice(entry);
                }
                memory.write_bytes(address, &out)?;
                if let Some(CacheObject::Directory { cursor, .. }) = self.objects.get_mut(&index) {
                    *cursor += n;
                }
                Ok(Reply::data(&(n as i64).to_le_bytes()))
            }
        }
    }
    fn mount(
        &mut self,
        memory: &Memory,
        volume: u16,
        readonly: bool,
        command: u32,
        req: &Request,
        args: &[u8],
    ) -> RuntimeResult<Reply> {
        if command == 10 {
            no_buffers(req)?;
            return Ok(Reply::ok());
        } // RAM cache has no deferred host writes.
        let p = path(memory, req)?;
        if std::env::var_os("WL_FS_TRACE").is_some() {
            eprintln!("[fs] RAM-cache index={volume} cmd={command} path={p}");
        }
        let v = self
            .volumes
            .get_mut(&volume)
            .ok_or("unmounted cache volume")?;
        match command {
            0 => {
                if readonly {
                    return Err("read-only cache mount".into());
                }
                if u32_at(args, 0) != Some(0) {
                    return Err("unsupported cache CreateFile options".into());
                }
                let size = u64_at(args, 8)?;
                if v.files.contains_key(&p) || v.directories.contains(&p) {
                    return Ok(Reply::failure(0x402));
                }
                if !v.directories.contains(parent(&p)) {
                    return Ok(Reply::failure(0x202));
                }
                if v.files.len() + v.directories.len() >= 1024
                    || size > v.quota.saturating_sub(v.used())
                {
                    return Err("cache file quota exceeded".into());
                }
                let mut bytes = Vec::new();
                resize(&mut bytes, size as usize)?;
                v.files.insert(p, bytes);
                Ok(Reply::ok())
            }
            1 => {
                if self.objects.values().any(|o| matches!(o, CacheObject::File { volume:v,path:n,.. } if *v==volume && *n==p)) {
                    return Ok(Reply::failure(0xe02));
                }
                if readonly {
                    return Err("read-only cache mount".into());
                }
                Ok(if v.files.remove(&p).is_some() {
                    Reply::ok()
                } else {
                    Reply::failure(0x202)
                })
            }
            2 => {
                if readonly {
                    return Err("read-only cache mount".into());
                }
                if v.files.contains_key(&p) || v.directories.contains(&p) {
                    return Ok(Reply::failure(0x402));
                }
                if !v.directories.contains(parent(&p)) {
                    return Ok(Reply::failure(0x202));
                }
                if v.files.len() + v.directories.len() >= 1024 {
                    return Err("cache entry budget".into());
                }
                v.directories.insert(p);
                Ok(Reply::ok())
            }
            7 => {
                if v.files.contains_key(&p) {
                    Ok(Reply::data(&1u32.to_le_bytes()))
                } else if v.directories.contains(&p) {
                    Ok(Reply::data(&0u32.to_le_bytes()))
                } else {
                    Ok(Reply::failure(0x202))
                }
            }
            8 => {
                let mode = u32_at(args, 0).ok_or("truncated cache file mode")?;
                if mode & 3 == 0
                    || mode & !7 != 0
                    || (mode & 4 != 0 && mode & 2 == 0)
                    || (readonly && mode & 2 != 0)
                {
                    return Err("unsupported cache file mode".into());
                }
                if !v.files.contains_key(&p) {
                    return Ok(Reply::failure(0x202));
                }
                if self.objects.values().any(|o| matches!(o, CacheObject::File { volume:v,path:n,mode:m } if *v==volume && *n==p && (mode & 2 != 0 || *m & 2 != 0))) {
                    return Ok(Reply::failure(0xe02));
                }
                self.object(CacheObject::File {
                    volume,
                    path: p,
                    mode,
                })
            }
            9 => {
                let mode = u32_at(args, 0).ok_or("truncated directory mode")?;
                if mode & 3 == 0 || mode & !0x80000003 != 0 {
                    return Err("unsupported directory mode".into());
                }
                if !v.directories.contains(&p) {
                    return Ok(Reply::failure(0x202));
                }
                let mut entries = Vec::new();
                for (name, kind, size) in v
                    .directories
                    .iter()
                    .map(|n| (n, 0, 0))
                    .chain(v.files.iter().map(|(n, b)| (n, 1, b.len() as u64)))
                {
                    if name == &p || parent(name) != p || mode & (1 << kind) == 0 {
                        continue;
                    }
                    let leaf = name.rsplit('/').next().unwrap().as_bytes();
                    let mut e = vec![0; 0x310];
                    e[..leaf.len()].copy_from_slice(leaf);
                    e[0x304] = kind as u8;
                    if mode & 0x80000000 == 0 {
                        e[0x308..].copy_from_slice(&size.to_le_bytes());
                    }
                    entries.push(e);
                }
                entries.sort_by(|a, b| a[..0x301].cmp(&b[..0x301]));
                self.object(CacheObject::Directory { entries, cursor: 0 })
            }
            11 => Ok(Reply::data(&((v.quota - v.used()) as i64).to_le_bytes())),
            12 => Ok(Reply::data(&(v.quota as i64).to_le_bytes())),
            _ => Err(format!(
                "unsupported RAM-cache filesystem command {command}"
            )),
        }
    }
    fn file(
        memory: &mut Memory,
        v: &mut Volume,
        path: &str,
        mode: u32,
        command: u32,
        req: &Request,
        args: &[u8],
    ) -> RuntimeResult<Reply> {
        let used = v.used();
        let quota = v.quota;
        let bytes = v.files.get_mut(path).ok_or("deleted cache file")?;
        match command {
            0 | 1 => {
                simple(req)?;
                if !req.statics.is_empty() || args.len() < 24 {
                    return Err("invalid cache file transfer".into());
                }
                let option = u32_at(args, 0).unwrap();
                if (command == 0 && option != 0) || (command == 1 && option & !1 != 0) {
                    return Err("unsupported file transfer option".into());
                }
                let offset = u64_at(args, 8)?;
                let size = u64_at(args, 16)?;
                let end = offset.checked_add(size).ok_or("cache transfer overflow")?;
                if size > MAX_TRANSFER || offset > MAX_CACHE_BYTES || end > MAX_CACHE_BYTES {
                    return Err("cache transfer budget".into());
                }
                if command == 0 {
                    if mode & 1 == 0 || req.recvs.len() != 1 || !req.sends.is_empty() {
                        return Err("invalid cache read mode/buffers".into());
                    }
                    let (address, capacity) = req.recvs[0];
                    if size > capacity {
                        return Err("short cache read buffer".into());
                    }
                    memory.validate_write(address, size as usize)?;
                    let n = size.min((bytes.len() as u64).saturating_sub(offset));
                    if n != 0 {
                        memory
                            .write_bytes(address, &bytes[offset as usize..(offset + n) as usize])?;
                    }
                    Ok(Reply::data(&n.to_le_bytes()))
                } else {
                    if mode & 2 == 0 || req.sends.len() != 1 || !req.recvs.is_empty() {
                        return Err("invalid cache write mode/buffers".into());
                    }
                    let (address, capacity) = req.sends[0];
                    if size > capacity {
                        return Err("short cache write buffer".into());
                    }
                    if end > bytes.len() as u64 && mode & 4 == 0 {
                        return Err("cache write requires append permission".into());
                    }
                    let growth = end.saturating_sub(bytes.len() as u64);
                    if growth > quota - used {
                        return Err("cache write quota exceeded".into());
                    }
                    let mut source = vec![0; size as usize];
                    memory.read_into(address, &mut source)?;
                    if end > bytes.len() as u64 {
                        resize(bytes, end as usize)?;
                    }
                    bytes[offset as usize..end as usize].copy_from_slice(&source);
                    Ok(Reply::ok())
                }
            }
            2 => {
                no_buffers(req)?;
                Ok(Reply::ok())
            }
            3 => {
                no_buffers(req)?;
                if mode & 2 == 0 {
                    return Err("cache resize requires write access".into());
                }
                let size = u64_at(args, 0)?;
                if size > quota || size.saturating_sub(bytes.len() as u64) > quota - used {
                    return Err("cache resize quota exceeded".into());
                }
                resize(bytes, size as usize)?;
                Ok(Reply::ok())
            }
            4 => {
                no_buffers(req)?;
                Ok(Reply::data(&(bytes.len() as i64).to_le_bytes()))
            }
            _ => Err(format!("unsupported RAM-cache file command {command}")),
        }
    }
}
