//! nfp:user with no connected NFC readers. Wire shapes: libnx feebd026 nfc.c/h.
use std::collections::BTreeMap;
use wl_guest_runtime::{
    Memory, RuntimeResult, audio::Reply, horizon::Kernel, ipc::Request, services::HostObject,
    threads,
};
pub const MANAGER: usize = 1 << 25;
pub const NAME: [u8; 8] = *b"nfp:user";
const LAST: usize = MANAGER + 4096;
const DEVICE_NOT_FOUND: u32 = 115 | (64 << 9);
#[derive(Debug, Default)]
struct Interface {
    initialized: bool,
    event: Option<usize>,
}
#[derive(Debug, Default)]
pub struct Service {
    next: usize,
    interfaces: BTreeMap<usize, Interface>,
}
pub fn owns(index: usize) -> bool {
    (MANAGER..=LAST).contains(&index)
}
impl Service {
    pub fn close(&mut self, index: usize) {
        self.interfaces.remove(&index);
    }
    pub fn request(
        &mut self,
        memory: &Memory,
        k: &mut Kernel,
        index: usize,
        command: u32,
        req: &Request,
        args: &[u8],
    ) -> RuntimeResult<Reply> {
        if req.copy_handles != 0
            || req.move_handles != 0
            || !req.copies.is_empty()
            || req.exchanges != 0
            || !req.statics.is_empty()
            || args.len() > 32
        {
            return Err("unsupported nfp request shape".into());
        }
        let no_buffers = req.sends.is_empty() && req.recvs.is_empty();
        if index == MANAGER {
            if command != 0
                || req.pid.is_some()
                || !no_buffers
                || self.next >= 4096
                || self.interfaces.len() >= 64
            {
                return Err("unsupported nfp manager request or capacity".into());
            }
            self.next += 1;
            let index = MANAGER + self.next;
            self.interfaces.insert(index, Interface::default());
            return Ok(Reply::object(HostObject::AudioRenderer { index }));
        }
        let interface = self
            .interfaces
            .get_mut(&index)
            .ok_or("closed or unknown nfp interface")?;
        if !interface.initialized && matches!(command, 2 | 23) {
            return Err("nfp device operation before Initialize".into());
        }
        match command {
            0 => {
                if req.pid != Some(k.config.process_id)
                    || args.len() < 16
                    || req.sends.len() != 1
                    || !req.recvs.is_empty()
                {
                    return Err("unsupported nfp Initialize layout or caller".into());
                }
                let aruid = u64::from_le_bytes(args[..8].try_into().unwrap());
                if aruid != k.config.process_id {
                    return Err("nfp Initialize ARUID does not identify this process".into());
                }
                let (address, size) = req.sends[0];
                if size == 0 || size > 256 || size % 32 != 0 {
                    return Err("unsupported nfp MCU version array".into());
                }
                let mut versions = [0u8; 256];
                memory.read_into(address, &mut versions[..size as usize])?;
                for entry in versions[..size as usize].chunks_exact(32) {
                    if entry[..8] == [0; 8] || entry[8..].iter().any(|b| *b != 0) {
                        return Err("unsupported nfp MCU version record".into());
                    }
                }
                interface.initialized = true;
                Ok(Reply::ok())
            }
            1 if no_buffers && req.pid.is_none() => {
                interface.initialized = false;
                Ok(Reply::ok())
            }
            19 if no_buffers && req.pid.is_none() => {
                Ok(Reply::data(&u32::from(interface.initialized).to_le_bytes()))
            }
            2 if req.pid.is_none() && req.sends.is_empty() && req.recvs.len() == 1 => {
                let (address, size) = req.recvs[0];
                if size == 0 || size > 9 * 8 || size % 8 != 0 {
                    return Err("unsupported nfp device-list capacity".into());
                }
                memory.validate_write(address, size as usize)?;
                // No reader exists: report the documented absence result and
                // leave the guest's output array untouched.
                Ok(Reply::failure(DEVICE_NOT_FOUND))
            }
            23 if no_buffers && req.pid.is_none() => {
                let event = match interface.event {
                    Some(e) => e,
                    None => {
                        let e = k.events.len();
                        k.events.push(threads::Event::default());
                        interface.event = Some(e);
                        e
                    }
                };
                match threads::readable_event_handle(k, event) {
                    Ok(handle) => Ok(Reply {
                        copies: vec![handle],
                        ..Reply::ok()
                    }),
                    Err(code) => Ok(Reply::failure(code)),
                }
            }
            _ => Err(format!("unsupported nfp:user command {command}")),
        }
    }
}
