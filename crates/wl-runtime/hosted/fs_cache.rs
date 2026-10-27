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
pub fn owns(index: usize) -> bool {
    matches!(index, MANAGER | READER | EMPTY_FILESYSTEM)
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
    if !matches!(
        s.endpoint,
        Endpoint::Host(HostObject::FileSystemProxy)
            | Endpoint::Host(HostObject::FileSystem)
            | Endpoint::Host(HostObject::AudioRenderer { index: READER })
    ) {
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
    let reader = target == HostObject::AudioRenderer { index: READER };
    let opening = !control
        && !closing
        && target == HostObject::FileSystemProxy
        && cmif + 16 <= end
        && u32_at(&b, cmif)? == u32::from_le_bytes(*b"SFCI")
        && u32_at(&b, cmif + 8)? == 62;
    let filesystem = !control
        && !closing
        && target == HostObject::FileSystem
        && cmif + 16 <= end
        && u32_at(&b, cmif)? == u32::from_le_bytes(*b"SFCI")
        && matches!(u32_at(&b, cmif + 8)?, 7..=9);
    if !reader && !opening && !filesystem {
        return None;
    }
    let k = process.kernel.as_mut()?;
    // The opaque callback transport uses an audio tag, but these objects belong
    // to FS. Scope the existing ID allocator to FS for creation and domain close,
    // preventing collisions and avoiding a second domain/handle implementation.
    std::mem::swap(&mut k.host.fs_domain_ids, &mut k.host.audio_domain_ids);
    let location = if opening || filesystem {
        let target = HostObject::AudioRenderer {
            index: if filesystem {
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
        swap_ids: true,
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
