//! System services hosted by the runtime, reached through sm like services of other
//! processes, and the CMIF server protocol they speak (Atmosphère `libstratosphere/source/sf`,
//! `include/stratosphere/sf/impl/sf_impl_command_serialization.hpp`; 0e9cd964).
//!
//! A CMIF request is a HIPC message of type 4 (5 for control requests, 6 and 7 with an inline
//! context, 2 to close) whose raw data starts at the next 16-byte boundary with the `SFCI`
//! header (magic, version, command ID, token) and the arguments. The reply has `SFCO`, version
//! 0, the result and the interface ID at the same alignment; its raw data word count includes
//! 16 bytes of alignment padding, and moved handles and output objects (sessions) follow the
//! special header. A command whose result is a failure replies with the header alone.
//!
//! Hosted services:
//! - `lm` (log manager, `libstratosphere/source/lm`): ILogService 0 OpenLogger(pid) opens an
//!   ILogger; ILogger 0 Log(InAutoSelectBuffer) receives a log packet and 1
//!   SetDestination(u32) selects where packets go. Packets carry a 0x18-byte header (process
//!   ID, thread ID, head/tail flags, severity, verbosity, payload size) and chunks of ULEB128
//!   key, ULEB128 size and data; the text chunks of a message, joined from its head packet to
//!   its tail packet, become one line of the process's debug output.
//!
//! Requests the model does not cover (domains, unknown commands, a request layout that
//! differs from the command's) are explicit faults rather than replies.
use crate::horizon::{Kernel, result};
use crate::ipc::{Endpoint, Request};
use crate::{Memory, ipc};

/// An object behind a session to a hosted service.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum HostObject {
    /// `lm` ILogService.
    LogService,
    /// ILogger opened by `process_id`.
    Logger { process_id: u64 },
}

/// Size of `lm`'s pointer buffer (`lm_ipc_server.cpp`, `PointerBufferSize`).
const LM_POINTER_BUFFER_SIZE: u16 = 0x400;

/// `lm::LogDestination_TargetManager`, the destination before any SetDestination.
pub const LOG_DESTINATION_TARGET_MANAGER: u32 = 1;

/// State of the hosted services.
#[derive(Clone, Debug, Default)]
pub struct HostState {
    /// Destination set by ILogger::SetDestination; `None` before the first call.
    pub log_destination: Option<u32>,
    /// Log text of messages whose tail packet has not arrived, by thread ID.
    pub partial_logs: Vec<(u64, String)>,
    /// Log packets received.
    pub log_packets: u64,
    /// CMIF domains, indexed by `Session::domain`.
    pub domains: Vec<Domain>,
    /// Domain object IDs of the `lm` server.
    pub lm_domain_ids: DomainIds,
}

/// The hosted service registered under `name`.
pub fn lookup(name: &[u8; 8]) -> Option<HostObject> {
    (name == b"lm\0\0\0\0\0\0").then_some(HostObject::LogService)
}

impl HostObject {
    fn interface(&self) -> (&'static str, u32) {
        match self {
            HostObject::LogService => ("lm ILogService", 0xE1D7_F748),
            HostObject::Logger { .. } => ("lm ILogger", 0x3E81_DAD2),
        }
    }
    fn pointer_buffer_size(&self) -> u16 {
        LM_POINTER_BUFFER_SIZE
    }
}

/// What a hosted service does with a request.
pub(crate) enum Outcome {
    Reply(Vec<u8>),
    /// The server closes the session; the client sees SessionClosed.
    Close,
}

/// A CMIF domain: objects addressed by ID through one session.
#[derive(Clone, Debug, Default)]
pub struct Domain {
    pub objects: Vec<(u32, HostObject)>,
}
/// `lm`'s `MaxDomainObjects`.
const MAX_DOMAIN_OBJECTS: u32 = 61;

/// The domain object IDs of one server (`ServerDomainManager::EntryManager`): entries in
/// order from ID 1, and freed IDs reused most recently freed first.
#[derive(Clone, Debug, Default)]
pub struct DomainIds {
    next: u32,
    freed: Vec<u32>,
}
impl DomainIds {
    fn allocate(&mut self) -> Result<u32, String> {
        if let Some(id) = self.freed.pop() {
            return Ok(id);
        }
        if self.next >= MAX_DOMAIN_OBJECTS {
            return Err("unsupported Horizon IPC: CMIF domain entries exhausted".into());
        }
        self.next += 1;
        Ok(self.next)
    }
}

impl Domain {
    fn get(&self, id: u32) -> Option<HostObject> {
        self.objects.iter().find(|(i, _)| *i == id).map(|&(_, o)| o)
    }
}
impl HostState {
    /// Registers `object` in domain `d` under a newly allocated ID.
    fn add(&mut self, d: usize, object: HostObject) -> Result<u32, String> {
        let id = self.lm_domain_ids.allocate()?;
        self.domains[d].objects.push((id, object));
        Ok(id)
    }
    /// Removes object `id` from domain `d`, freeing its ID.
    fn remove(&mut self, d: usize, id: u32) {
        let objects = &mut self.domains[d].objects;
        if let Some(i) = objects.iter().position(|(o, _)| *o == id) {
            objects.remove(i);
            self.lm_domain_ids.freed.push(id);
        }
    }
}

const CMIF_IN_MAGIC: u32 = u32::from_le_bytes(*b"SFCI");
const CMIF_OUT_MAGIC: u32 = u32::from_le_bytes(*b"SFCO");
const DOMAIN_SEND_MESSAGE: u8 = 1;
const DOMAIN_CLOSE: u8 = 2;
/// `sf::hipc` results (module 11).
const TARGET_NOT_DOMAIN: u32 = 11 | 491 << 9;
const DOMAIN_OBJECT_NOT_FOUND: u32 = 11 | 492 << 9;

/// A CMIF reply. Outside a domain, output objects are sessions moved in the special header
/// after `handles`; in a domain (`domain_ids`), a domain out header with the object count
/// precedes `SFCO`, and the object IDs follow the output data.
fn reply(
    interface_id: u32,
    code: u32,
    data: &[u8],
    handles: &[u32],
    domain_ids: Option<&[u32]>,
) -> Vec<u8> {
    let domain_header = if domain_ids.is_some() { 0x10 } else { 0 };
    let ids = domain_ids.unwrap_or_default();
    let raw = domain_header + 0x10 + data.len() + 4 * ids.len();
    let words = (raw.div_ceil(4) * 4 + 0x10) / 4;
    let mut out = vec![0u8; 8];
    out[4..8].copy_from_slice(
        &(words as u32 | if handles.is_empty() { 0 } else { 1 << 31 }).to_le_bytes(),
    );
    if !handles.is_empty() {
        out.extend(((handles.len() as u32) << 5).to_le_bytes());
        for h in handles {
            out.extend(h.to_le_bytes());
        }
    }
    let mut at = out.len().next_multiple_of(16);
    out.resize(out.len() + words * 4, 0);
    let mut put = |at: &mut usize, bytes: &[u8]| {
        out[*at..*at + bytes.len()].copy_from_slice(bytes);
        *at += bytes.len();
    };
    if domain_ids.is_some() {
        put(&mut at, &(ids.len() as u32).to_le_bytes());
        at += 12;
    }
    for w in [CMIF_OUT_MAGIC, 0, code, interface_id] {
        put(&mut at, &w.to_le_bytes());
    }
    put(&mut at, data);
    for id in ids {
        put(&mut at, &id.to_le_bytes());
    }
    out
}

/// The command's expected request shape, checked as `HipcCommandProcessor::PrepareForProcess`.
struct Shape {
    pid: bool,
    statics: usize,
    sends: usize,
    in_data: usize,
}

/// A command's result: on success, output data and output objects.
struct Done {
    code: u32,
    data: Vec<u8>,
    objects: Vec<HostObject>,
}
impl Done {
    fn ok() -> Self {
        Done {
            code: result::SUCCESS,
            data: Vec::new(),
            objects: Vec::new(),
        }
    }
}

/// One request on a session to a hosted service.
pub(crate) fn request(
    memory: &Memory,
    k: &mut Kernel,
    s: usize,
    object: HostObject,
    req: &Request,
    message: &[u8; ipc::MESSAGE_BUFFER_SIZE],
) -> Result<Outcome, String> {
    if req.kind == 2 {
        return Ok(Outcome::Close);
    }
    let control = matches!(req.kind, 5 | 7);
    if !matches!(req.kind, 4..=7) {
        // sf::hipc::ResultUnknownCommandType: the session is closed.
        return Ok(Outcome::Close);
    }
    // CMIF requires 16 bytes of padding in the raw data and 16-byte alignment.
    let raw_start = req.data.next_multiple_of(16);
    let raw_end = req.data + 4 * req.data_words;
    if 4 * req.data_words < 0x10 || raw_end - raw_start < 0x10 {
        return Ok(Outcome::Close);
    }
    let domain = k.sessions[s].domain;
    let (target, cmif) = match domain {
        Some(d) if !control => {
            // Domain in header: type, input object count, data size, object ID.
            let raw = &message[raw_start..raw_end];
            let (kind, in_objects) = (raw[0], raw[1]);
            let size = usize::from(u16::from_le_bytes([raw[2], raw[3]]));
            let id = ipc::read_u32(raw, 4);
            let body = &raw[0x10..];
            match kind {
                DOMAIN_SEND_MESSAGE => {
                    let Some(target) = k.host.domains[d].get(id) else {
                        // sf::cmif::ResultTargetNotFound: the session is closed.
                        return Ok(Outcome::Close);
                    };
                    if size + 4 * usize::from(in_objects) > body.len() {
                        return Ok(Outcome::Close);
                    }
                    if in_objects != 0 {
                        return Err(format!(
                            "unsupported Horizon IPC: input objects sent to {target:?}"
                        ));
                    }
                    (target, raw_start + 0x10..raw_start + 0x10 + size)
                }
                DOMAIN_CLOSE => {
                    k.host.remove(d, id);
                    // No reply is written: the message buffer is returned as sent.
                    return Ok(Outcome::Reply(Vec::new()));
                }
                _ => return Ok(Outcome::Close),
            }
        }
        _ => (object, raw_start..raw_end),
    };
    let (name, interface_id) = target.interface();
    if cmif.len() < 0x10 {
        return Ok(Outcome::Close);
    }
    let word = |i: usize| ipc::read_u32(message, cmif.start + 4 * i);
    if word(0) != CMIF_IN_MAGIC || word(1) > 1 {
        return Ok(Outcome::Close);
    }
    let command = word(2);
    let args = &message[cmif.start + 0x10..cmif.end];
    let unsupported = |what: &str| {
        Err(format!(
            "unsupported Horizon IPC: {what} (command {command} of {name}{})",
            if control { " control" } else { "" }
        ))
    };
    let check = |shape: Shape| {
        let valid = req.pid.is_some() == shape.pid
            && req.statics.len() == shape.statics
            && req.sends.len() == shape.sends
            && req.recvs.is_empty()
            && req.exchanges == 0
            && req.copy_handles == 0
            && req.move_handles == 0
            && args.len() >= shape.in_data;
        if valid {
            Ok(())
        } else {
            Err(format!(
                "unsupported Horizon IPC: the request layout differs from command {command} of {name} ({req:?})"
            ))
        }
    };
    let no_arguments = Shape {
        pid: false,
        statics: 0,
        sends: 0,
        in_data: 0,
    };
    if control {
        return match command {
            // QueryPointerBufferSize: out u16.
            3 => {
                check(no_arguments)?;
                let size = object.pointer_buffer_size().to_le_bytes();
                Ok(Outcome::Reply(reply(0, result::SUCCESS, &size, &[], None)))
            }
            // ConvertCurrentObjectToDomain: out the object's ID in the new domain.
            0 => {
                check(no_arguments)?;
                if domain.is_some() {
                    return unsupported("converting a domain session to a domain");
                }
                k.host.domains.push(Domain::default());
                let d = k.host.domains.len() - 1;
                let id = k.host.add(d, object)?;
                k.sessions[s].domain = Some(d);
                Ok(Outcome::Reply(reply(
                    0,
                    result::SUCCESS,
                    &id.to_le_bytes(),
                    &[],
                    None,
                )))
            }
            // CopyFromCurrentDomain(object ID): a session to that object, outside the domain.
            1 => {
                check(Shape {
                    in_data: 4,
                    ..no_arguments
                })?;
                let id = ipc::read_u32(args, 0);
                let Some(d) = domain else {
                    return Ok(Outcome::Reply(reply(0, TARGET_NOT_DOMAIN, &[], &[], None)));
                };
                let Some(found) = k.host.domains[d].get(id) else {
                    return Ok(Outcome::Reply(reply(
                        0,
                        DOMAIN_OBJECT_NOT_FOUND,
                        &[],
                        &[],
                        None,
                    )));
                };
                Ok(Outcome::Reply(
                    match ipc::open_session(k, Endpoint::Host(found)) {
                        Ok(h) => reply(0, result::SUCCESS, &[], &[h], None),
                        Err(e) => reply(0, e, &[], &[], None),
                    },
                ))
            }
            // CloneCurrentObject, CloneCurrentObjectEx(u32 tag): a new session to the object.
            2 | 4 => {
                check(Shape {
                    in_data: if command == 4 { 4 } else { 0 },
                    ..no_arguments
                })?;
                if domain.is_some() {
                    return unsupported("cloning a domain session");
                }
                let endpoint = k.sessions[s].endpoint;
                Ok(Outcome::Reply(match ipc::open_session(k, endpoint) {
                    Ok(h) => reply(0, result::SUCCESS, &[], &[h], None),
                    Err(e) => reply(0, e, &[], &[], None),
                }))
            }
            _ => unsupported("an unknown control command"),
        };
    }
    let done = match (target, command) {
        (HostObject::LogService, 0) => {
            // OpenLogger(ClientProcessId): the process ID is a u64 placeholder in the data,
            // replaced by the one the kernel wrote into the special header.
            check(Shape {
                pid: true,
                in_data: 8,
                ..no_arguments
            })?;
            let process_id = req.pid.unwrap_or_default();
            Done {
                objects: vec![HostObject::Logger { process_id }],
                ..Done::ok()
            }
        }
        (HostObject::Logger { process_id }, 0) => {
            // Log(InAutoSelectBuffer): the mapped buffer when its address is non-zero, else
            // the pointer buffer.
            check(Shape {
                statics: 1,
                sends: 1,
                ..no_arguments
            })?;
            let (address, size) = if req.sends[0].0 != 0 {
                req.sends[0]
            } else {
                req.statics[0]
            };
            let mut packet = vec![0u8; size as usize];
            memory
                .read_into(address, &mut packet)
                .map_err(|e| format!("lm ILogger::Log buffer unreadable: {e}"))?;
            log(k, process_id, &packet);
            Done::ok()
        }
        (HostObject::Logger { .. }, 1) => {
            check(Shape {
                in_data: 4,
                ..no_arguments
            })?;
            k.host.log_destination = Some(ipc::read_u32(args, 0));
            Done::ok()
        }
        _ => return unsupported("an unknown command"),
    };
    if done.code != result::SUCCESS {
        let ids: &[u32] = &[];
        return Ok(Outcome::Reply(reply(
            interface_id,
            done.code,
            &[],
            &[],
            domain.map(|_| ids),
        )));
    }
    Ok(Outcome::Reply(match domain {
        Some(d) => {
            let mut ids = Vec::new();
            for o in done.objects {
                ids.push(k.host.add(d, o)?);
            }
            reply(interface_id, done.code, &done.data, &[], Some(&ids))
        }
        None => {
            let mut handles = Vec::new();
            for o in done.objects {
                match ipc::open_session(k, Endpoint::Host(o)) {
                    Ok(h) => handles.push(h),
                    Err(e) => return Ok(Outcome::Reply(reply(interface_id, e, &[], &[], None))),
                }
            }
            reply(interface_id, done.code, &done.data, &handles, None)
        }
    }))
}

const LOG_PACKET_HEADER_SIZE: usize = 0x18;
const LOG_CHUNK_TEXT: u64 = 2;

fn uleb128(bytes: &[u8], at: &mut usize) -> Option<u64> {
    let mut value = 0u64;
    for shift in (0..64).step_by(7) {
        let b = *bytes.get(*at)?;
        *at += 1;
        value |= u64::from(b & 0x7f) << shift;
        if b & 0x80 == 0 {
            return Some(value);
        }
    }
    None
}

/// ILogger::Log: a packet whose size disagrees with its header is ignored with success, as
/// in `lm`.
fn log(k: &mut Kernel, _process_id: u64, packet: &[u8]) {
    if packet.len() < LOG_PACKET_HEADER_SIZE {
        return;
    }
    let payload = ipc::read_u32(packet, 0x14) as usize;
    if LOG_PACKET_HEADER_SIZE + payload != packet.len() {
        return;
    }
    k.host.log_packets += 1;
    let thread_id = u64::from_le_bytes(packet[8..16].try_into().unwrap_or_default());
    let flags = packet[0x10];
    let (head, tail) = (flags & 1 != 0, flags & 2 != 0);
    let mut text = String::new();
    let body = &packet[LOG_PACKET_HEADER_SIZE..];
    let mut at = 0;
    while at < body.len() {
        let (Some(key), Some(size)) = (uleb128(body, &mut at), uleb128(body, &mut at)) else {
            break;
        };
        let Some(data) = body.get(at..at.saturating_add(size as usize)) else {
            break;
        };
        at += data.len();
        if key == LOG_CHUNK_TEXT {
            text.push_str(&String::from_utf8_lossy(data));
        }
    }
    let partial = &mut k.host.partial_logs;
    let slot = partial.iter().position(|(t, _)| *t == thread_id);
    let mut message = match slot {
        Some(i) if !head => partial.remove(i).1,
        Some(i) => {
            partial.remove(i);
            String::new()
        }
        None => String::new(),
    };
    message.push_str(&text);
    if tail {
        let line = message.trim_end_matches(['\n', '\0']);
        if !line.is_empty() {
            k.debug_output.push(line.to_string());
        }
    } else {
        partial.push((thread_id, message));
    }
}
