//! Bounded ControllerSupport applet for an already connected FullKey Npad.
//! libnx feebd026: applet.c, libapplet.h, hid_la.c/h (ISC wire definitions).
use std::{
    collections::BTreeMap,
    sync::{Arc, Mutex},
};
use wl_guest_runtime::{
    Cpu, Memory, Process, RuntimeResult,
    audio::Reply,
    horizon::{Kernel, Object},
    ipc::{Endpoint, Request},
    services::HostObject,
    threads,
};
pub const CREATOR: usize = 1 << 23;
const LIMIT: usize = CREATOR + 8192;
const MAX_OBJECTS: usize = 128;
const MAX_STORAGE: usize = 4096;
pub fn owns(index: usize) -> bool {
    (CREATOR..=LIMIT).contains(&index)
}
fn w(b: &[u8], n: usize) -> u32 {
    u32::from_le_bytes(b[n..n + 4].try_into().unwrap())
}
fn q(b: &[u8], n: usize) -> u64 {
    u64::from_le_bytes(b[n..n + 8].try_into().unwrap())
}
type Bytes = Arc<Mutex<Vec<u8>>>;
#[derive(Clone, Debug)]
enum Kind {
    Storage(Bytes),
    Accessor(Bytes),
    Controller,
}
#[derive(Debug, Default)]
struct Controller {
    inputs: Vec<Vec<u8>>,
    event: Option<usize>,
    output: Option<Vec<u8>>,
    finished: bool,
    suspend: bool,
}
#[derive(Default)]
pub struct Service {
    next: usize,
    objects: BTreeMap<usize, Kind>,
    controllers: BTreeMap<usize, Controller>,
    launchable: Option<usize>,
    // Process-owned policy only: the hosted profile does not suspend on idle.
    idle_time_detection_extension: u32,
}
impl std::fmt::Debug for Service {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("NativeApplets")
            .field("objects", &self.objects.len())
            .field("controllers", &self.controllers.len())
            .finish()
    }
}
pub struct Call<'a> {
    pub command: u32,
    pub inputs: &'a [HostObject],
}
impl Service {
    pub fn idle_time_detection_extension(&mut self, value: Option<u32>) -> RuntimeResult<Reply> {
        if let Some(value) = value {
            if value > 2 {
                return Err("unsupported applet idle-time detection extension".into());
            }
            self.idle_time_detection_extension = value;
            Ok(Reply::ok())
        } else {
            Ok(Reply::data(
                &self.idle_time_detection_extension.to_le_bytes(),
            ))
        }
    }
    fn add(&mut self, kind: Kind) -> RuntimeResult<Reply> {
        if self.objects.len() >= MAX_OBJECTS || self.next >= 8192 {
            return Err("native applet object capacity".into());
        }
        self.next += 1;
        let index = CREATOR + self.next;
        self.objects.insert(index, kind);
        Ok(Reply::object(HostObject::AudioRenderer { index }))
    }
    pub fn close(&mut self, index: usize) {
        self.objects.remove(&index);
        self.controllers.remove(&index);
    }
    pub fn launchable(&mut self, k: &mut Kernel) -> Reply {
        let e = *self.launchable.get_or_insert_with(|| {
            let e = k.events.len();
            k.events.push(threads::Event {
                signaled: true,
                ..Default::default()
            });
            e
        });
        event_reply(k, e)
    }
    pub fn request(
        &mut self,
        m: &mut Memory,
        k: &mut Kernel,
        index: usize,
        req: &Request,
        args: &[u8],
        call: Call<'_>,
    ) -> RuntimeResult<Reply> {
        let Call { command, inputs } = call;
        if req.pid.is_some()
            || req.copy_handles != 0
            || req.move_handles != 0
            || req.exchanges != 0
            || !req.copies.is_empty()
            || args.len() > 32
        {
            return Err("unsupported applet request metadata".into());
        }
        let plain = req.statics.is_empty() && req.sends.is_empty() && req.recvs.is_empty();
        if !inputs.is_empty() && command != 100 {
            return Err("unexpected applet input objects".into());
        }
        if index == CREATOR {
            if !plain || !inputs.is_empty() {
                return Err("unsupported applet creator descriptors".into());
            }
            return match command {
                0 if args.len() >= 8 && w(args, 0) == 0xc && w(args, 4) == 0 => {
                    let reply = self.add(Kind::Controller)?;
                    let HostObject::AudioRenderer { index } = reply.objects[0] else {
                        unreachable!()
                    };
                    self.controllers.insert(index, Controller::default());
                    Ok(reply)
                }
                10 if args.len() >= 8 && (1..=MAX_STORAGE as u64).contains(&q(args, 0)) => self
                    .add(Kind::Storage(Arc::new(Mutex::new(vec![
                        0;
                        q(args, 0) as usize
                    ])))),
                _ => Err(format!(
                    "unsupported library applet creation/storage command {command}"
                )),
            };
        }
        let kind = self
            .objects
            .get(&index)
            .cloned()
            .ok_or("closed or unknown native applet object")?;
        match kind {
            Kind::Storage(data) if command == 0 && plain && inputs.is_empty() => {
                self.add(Kind::Accessor(data))
            }
            Kind::Accessor(data) => {
                if !inputs.is_empty() {
                    return Err("input objects sent to storage accessor".into());
                }
                if command == 0 && plain {
                    return Ok(Reply::data(
                        &(data.lock().map_err(|_| "poisoned applet storage")?.len() as u64)
                            .to_le_bytes(),
                    ));
                }
                if args.len() < 8 || q(args, 0) > i64::MAX as u64 || !matches!(command, 10 | 11) {
                    return Err("unsupported applet storage access".into());
                }
                let offset =
                    usize::try_from(q(args, 0)).map_err(|_| "applet storage offset overflow")?;
                let buffers: Vec<_> = if command == 10 {
                    if !req.recvs.is_empty() {
                        return Err("output descriptor on storage write".into());
                    }
                    req.statics
                        .iter()
                        .chain(&req.sends)
                        .copied()
                        .filter(|b| b.1 != 0)
                        .collect()
                } else {
                    if !req.statics.is_empty() || !req.sends.is_empty() {
                        return Err("input descriptor on storage read".into());
                    }
                    req.recvs.iter().copied().filter(|b| b.1 != 0).collect()
                };
                if buffers.len() != 1 || buffers[0].1 > MAX_STORAGE as u64 {
                    return Err("unsupported applet storage buffer".into());
                }
                let (address, size) = buffers[0];
                let size = size as usize;
                let end = offset
                    .checked_add(size)
                    .ok_or("applet storage extent overflow")?;
                let mut data = data.lock().map_err(|_| "poisoned applet storage")?;
                let target = data
                    .get_mut(offset..end)
                    .ok_or("applet storage extent outside allocation")?;
                if command == 10 {
                    let mut new = vec![0; size];
                    m.read_into(address, &mut new)?;
                    target.copy_from_slice(&new);
                } else {
                    m.validate_write(address, size)?;
                    m.write_bytes(address, target)?;
                }
                Ok(Reply::ok())
            }
            Kind::Controller if plain => {
                if command == 100 {
                    if inputs.len() != 1 {
                        return Err("PushInData requires one checked domain storage object".into());
                    }
                    let HostObject::AudioRenderer { index: storage } = inputs[0] else {
                        return Err("non-storage input object".into());
                    };
                    let Some(Kind::Storage(data)) = self.objects.get(&storage) else {
                        return Err("input is not a live applet storage".into());
                    };
                    let bytes = data.lock().map_err(|_| "poisoned applet storage")?.clone();
                    let c = self
                        .controllers
                        .get_mut(&index)
                        .ok_or("missing controller applet state")?;
                    if c.finished || c.inputs.len() >= 3 {
                        return Err("invalid controller input ordering".into());
                    }
                    c.inputs.push(bytes);
                    return Ok(Reply::ok());
                }
                if !inputs.is_empty() {
                    return Err("unexpected controller applet input objects".into());
                }
                match command {
                    0 => {
                        let c = self
                            .controllers
                            .get_mut(&index)
                            .ok_or("missing controller applet")?;
                        let e = *c.event.get_or_insert_with(|| {
                            let e = k.events.len();
                            k.events.push(threads::Event {
                                signaled: c.finished,
                                ..Default::default()
                            });
                            e
                        });
                        Ok(event_reply(k, e))
                    }
                    10 => {
                        let c = self
                            .controllers
                            .get(&index)
                            .ok_or("missing controller applet")?;
                        if c.finished {
                            return Err("controller applet already started".into());
                        }
                        let output = controller_result(k, &c.inputs)?;
                        let c = self.controllers.get_mut(&index).unwrap();
                        c.output = Some(output);
                        c.finished = true;
                        if let Some(e) = c.event {
                            threads::signal_event(k, e);
                        }
                        Ok(Reply::ok())
                    }
                    30 if self.controllers[&index].finished => Ok(Reply::ok()),
                    101 => {
                        let output = self.controllers[&index]
                            .output
                            .as_ref()
                            .ok_or("controller applet has no pending output")?
                            .clone();
                        let reply = self.add(Kind::Storage(Arc::new(Mutex::new(output))))?;
                        self.controllers.get_mut(&index).unwrap().output = None;
                        Ok(reply)
                    }
                    50 if args.first().is_some_and(|b| *b <= 1) => {
                        self.controllers.get_mut(&index).unwrap().suspend = args[0] != 0;
                        Ok(Reply::ok())
                    }
                    60 => Ok(Reply::ok()), // This applet performs no GPU work.
                    _ => Err(format!("unsupported controller applet command {command}")),
                }
            }
            _ => Err(format!("unsupported applet object command {command}")),
        }
    }
}
fn event_reply(k: &mut Kernel, e: usize) -> Reply {
    match threads::readable_event_handle(k, e) {
        Ok(h) => Reply {
            copies: vec![h],
            ..Reply::ok()
        },
        Err(code) => Reply::failure(code),
    }
}
/// Pure validation plus the configured HID sample, not host-injected game data.
fn controller_result(k: &Kernel, inputs: &[Vec<u8>]) -> RuntimeResult<Vec<u8>> {
    if inputs.len() != 3 {
        return Err("controller applet requires common/private/support arguments".into());
    }
    let (common, private, support) = (&inputs[0], &inputs[1], &inputs[2]);
    if common.len() != 32
        || w(common, 0) != 1
        || w(common, 4) != 32
        || !matches!(w(common, 8), 3 | 4 | 5 | 7 | 8)
        || common[16] > 1
        || common[17..24] != [0; 7]
    {
        return Err("unsupported controller common arguments".into());
    }
    if private.len() != 20
        || w(private, 0) != 20
        || w(private, 4) as usize != support.len()
        || private[8..12] != [0; 4]
        || w(private, 12) & 1 == 0
        || w(private, 16) > 1
    {
        return Err("unsupported controller support mode/style".into());
    }
    let modern = w(common, 8) >= 7;
    let expected = if modern { 1072 } else { 540 };
    if support.len() != expected
        || support[0] > 1
        || !(1..=if modern { 8 } else { 4 }).contains(&support[1])
        || support[0] > support[1]
        || support[2] != 1
        || support[2..7].iter().any(|b| *b > 1)
    {
        return Err("unsupported controller player configuration".into());
    }
    // Only the already-connected FullKey profile is auto-completed. No pairing,
    // firmware changes, button press, dialog dismissal or user choice is invented.
    let id = k
        .host
        .hid_shared_memory
        .ok_or("controller support requires HID shared memory")?;
    let shm = &k
        .shared_memories
        .get(id)
        .ok_or("missing HID backing")?
        .bytes;
    let b = shm
        .get(0x9a00..0x9a00 + 0x5000)
        .ok_or("short HID Npad memory")?;
    if w(b, 0) & 1 == 0 {
        return Err("FullKey Npad is not connected".into());
    }
    let lifo = 0x28;
    let tail = q(b, lifo + 16);
    let count = q(b, lifo + 24);
    if q(b, lifo + 8) != 17
        || tail >= 17
        || count == 0
        || count > 17
        || q(b, lifo + 32 + tail as usize * 48 + 40) & 1 == 0
    {
        return Err("FullKey Npad has no connected HID sample".into());
    }
    let mut result = vec![0; 12];
    result[0] = 1; // one player, NpadId Player1=0, result=0
    Ok(result)
}

#[derive(Clone, Copy)]
enum Location {
    Session(usize, Endpoint),
    Domain(usize, usize, HostObject),
}
pub struct Binding {
    location: Option<Location>,
}
/// Route AM-owned objects for exactly one existing kernel IPC dispatch. Input
/// object validation remains in the kernel; no guest wire bytes are rewritten.
pub fn bind(cpu: &Cpu, p: &mut Process, svc: u16) -> Option<Binding> {
    if svc != 0x21 {
        return None;
    }
    let k = p.kernel.as_ref()?;
    let Object::Session(s) = k.handles.get(cpu.x[0] as u32)? else {
        return None;
    };
    let session = k.sessions.get(s)?;
    if session.closed || session.references == 0 {
        return None;
    }
    let mut b = [0; 256];
    p.memory
        .read_into(k.threads.get(k.current)?.tls, &mut b)
        .ok()?;
    let w0 = w(&b, 0);
    let w1 = w(&b, 4);
    if !matches!(w0 as u16, 4..=7) {
        return None;
    }
    let control = matches!(w0 as u16, 5 | 7);
    let mut at = 8usize;
    if w1 >> 31 != 0 {
        let h = w(&b, 8);
        at = 12 + if h & 1 != 0 { 8 } else { 0 } + 4 * (((h >> 1) & 15) + ((h >> 5) & 15)) as usize;
    }
    at += 8 * ((w0 >> 16) & 15) as usize
        + 12 * (((w0 >> 20) & 15) + ((w0 >> 24) & 15) + (w0 >> 28)) as usize;
    let end = at.checked_add(4 * (w1 & 0x3ff) as usize)?;
    let raw = at.checked_add(15)? & !15;
    if end > 256 || raw + 16 > end {
        return None;
    }
    let (target, location) = if let Some(d) = session.domain.filter(|_| !control) {
        if !matches!(b[raw], 1 | 2) {
            return None;
        }
        let objects = &k.host.domains.get(d)?.objects;
        let i = objects.iter().position(|(id, _)| *id == w(&b, raw + 4))?;
        (objects[i].1, Location::Domain(d, i, objects[i].1))
    } else {
        let Endpoint::Host(target) = session.endpoint else {
            return None;
        };
        (target, Location::Session(s, session.endpoint))
    };
    let native = matches!(target,HostObject::AudioRenderer{index}if owns(index));
    let closing = session.domain.is_some() && !control && b[raw] == 2;
    let creator = target == HostObject::LibraryAppletCreator && !control && !closing;
    if !native && !creator {
        return None;
    }
    let k = p.kernel.as_mut()?;
    std::mem::swap(&mut k.host.am_domain_ids, &mut k.host.audio_domain_ids);
    if creator {
        let target = HostObject::AudioRenderer { index: CREATOR };
        match location {
            Location::Session(s, _) => k.sessions[s].endpoint = Endpoint::Host(target),
            Location::Domain(d, i, _) => k.host.domains[d].objects[i].1 = target,
        };
    }
    Some(Binding {
        location: creator.then_some(location),
    })
}
pub fn restore(p: &mut Process, b: Option<Binding>) {
    if let Some(b) = b
        && let Some(k) = p.kernel.as_mut()
    {
        std::mem::swap(&mut k.host.am_domain_ids, &mut k.host.audio_domain_ids);
        match b.location {
            Some(Location::Session(s, e)) => {
                if let Some(s) = k.sessions.get_mut(s) {
                    s.endpoint = e;
                }
            }
            Some(Location::Domain(d, i, o)) => {
                if let Some((_, target)) =
                    k.host.domains.get_mut(d).and_then(|d| d.objects.get_mut(i))
                {
                    *target = o;
                }
            }
            None => {}
        }
    }
}
