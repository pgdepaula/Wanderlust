//! Horizon IPC: the named port `sm:`, client sessions, the HIPC message in the calling
//! thread's thread-local region, and the service manager as a server hosted by the runtime.
//!
//! The service manager is a TIPC server, as Atmosphère's `sm`
//! (`stratosphere/sm/source/sm_user_service.{hpp,cpp}`, `impl/sm_service_manager.cpp`,
//! `libstratosphere/include/stratosphere/tipc`; 0e9cd964). A TIPC request's tag is the
//! command ID plus 0x10 and must carry the exact message header (and special header, when a
//! process ID is sent) of that command; the reply repeats the tag, places a move handle in the
//! special data and the result as the first raw data word. Tag 15 closes the session. sm also
//! keeps a CMIF compatibility layer for older clients: it recognises the exact HIPC header
//! words of RegisterClient/DetachClient, GetServiceHandle/UnregisterService and
//! RegisterService and answers control requests with its pointer-buffer size. Anything else is
//! an invalid method, and a failed request closes the session without a reply
//! (`tipc_server_manager.hpp`), so the client's svcSendSyncRequest ends with SessionClosed.
//! GetServiceHandle for a service that is not registered is deferred: the client stays
//! blocked until the service is registered, and then receives the reply in the protocol of
//! its request.
use crate::horizon::{Kernel, Object, result};
use crate::services::{self, HostObject, Outcome};
use crate::threads::{self, Wait};
use crate::{Cpu, Memory, Stop};

/// Size of the IPC message buffer at the start of the thread-local region.
pub const MESSAGE_BUFFER_SIZE: usize = 0x100;
/// `KObjectName::NameLengthMax`.
pub const PORT_NAME_MAX: usize = 12;

/// Service-manager results (module 21).
pub mod sm_result {
    const fn sm(description: u32) -> u32 {
        21 | (description << 9)
    }
    pub const INVALID_CLIENT: u32 = sm(2);
    pub const OUT_OF_SESSIONS: u32 = sm(3);
    pub const ALREADY_REGISTERED: u32 = sm(4);
    pub const INVALID_SERVICE_NAME: u32 = sm(6);
    pub const NOT_REGISTERED: u32 = sm(7);
    pub const NOT_ALLOWED: u32 = sm(8);
}

/// What a client session is connected to.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Endpoint {
    /// The service manager; `client` is the process ID given by RegisterClient.
    Sm { client: Option<u64> },
    /// A port registered through sm:RegisterService, served by the guest itself.
    Port(usize),
    /// An object of a service hosted by the runtime.
    Host(HostObject),
}
#[derive(Clone, Copy, Debug)]
pub struct Session {
    pub endpoint: Endpoint,
    /// The session was converted to a CMIF domain (index into `HostState::domains`).
    pub domain: Option<usize>,
    /// The server side closed the session (SessionClosed for every further request).
    pub closed: bool,
    pub references: u32,
}
/// The server side of a port created by sm:RegisterService.
#[derive(Clone, Copy, Debug)]
pub struct Port {
    pub name: [u8; 8],
    pub max_sessions: u32,
    pub sessions: u32,
    pub is_light: bool,
    /// The guest closed its server-port handle.
    pub server_closed: bool,
    pub references: u32,
}
/// A service registered with sm.
#[derive(Clone, Copy, Debug)]
pub struct Service {
    pub name: [u8; 8],
    pub owner: u64,
    pub port: usize,
}

/// IPC counters reported by the launcher.
#[derive(Clone, Copy, Debug, Default)]
pub struct IpcStats {
    pub requests: u64,
    pub replies: u64,
    pub sessions_closed_by_server: u64,
    pub deferred: u64,
}

pub(crate) fn read_u32(b: &[u8], at: usize) -> u32 {
    u32::from_le_bytes(b[at..at + 4].try_into().unwrap_or_default())
}
fn name_text(name: &[u8; 8]) -> String {
    name.iter()
        .take_while(|&&b| b != 0)
        .map(|&b| char::from(b))
        .collect()
}

/// svcConnectToNamedPort (0x1F): X1 name (at most 11 characters and a terminator) -> W1
/// client session handle.
pub fn connect_to_named_port(memory: &Memory, k: &mut Kernel, address: u64) -> Result<u32, u32> {
    let mut name = [0u8; PORT_NAME_MAX];
    for (i, byte) in name.iter_mut().enumerate() {
        match memory.load(address.wrapping_add(i as u64), 1) {
            Ok(b) => {
                *byte = b as u8;
                if b == 0 {
                    break;
                }
            }
            Err(_) if i == 0 => return Err(result::INVALID_CURRENT_MEMORY),
            Err(_) => return Err(result::INVALID_POINTER),
        }
    }
    if name[PORT_NAME_MAX - 1] != 0 {
        return Err(result::OUT_OF_RANGE);
    }
    if &name[..4] != b"sm:\0" {
        return Err(result::NOT_FOUND);
    }
    open_session(k, Endpoint::Sm { client: None })
}

pub(crate) fn open_session(k: &mut Kernel, endpoint: Endpoint) -> Result<u32, u32> {
    let s = k.sessions.len();
    k.sessions.push(Session {
        endpoint,
        domain: None,
        closed: false,
        references: 1,
    });
    k.handles.add(Object::Session(s)).inspect_err(|_| {
        k.sessions.pop();
    })
}

/// Closes a session or port handle; true if the handle named one.
pub fn close_handle(k: &mut Kernel, object: Object) {
    match object {
        Object::Session(s) => {
            let session = &mut k.sessions[s];
            session.references -= 1;
            if session.references == 0 {
                if let Endpoint::Port(p) = session.endpoint {
                    k.ports[p].sessions = k.ports[p].sessions.saturating_sub(1);
                }
            }
        }
        Object::Port(p) => {
            let port = &mut k.ports[p];
            port.references -= 1;
            if port.references == 0 {
                port.server_closed = true;
            }
        }
        Object::Thread(_) => {}
    }
}

/// The response bytes of Atmosphère sm's CMIF compatibility layer.
const RESPONSE_POINTER_BUFFER_SIZE: [u8; 36] = [
    0x00, 0x00, 0x00, 0x00, 0x07, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
    0x00, //
    0x53, 0x46, 0x43, 0x4F, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
    0x00, //
    0x00, 0x00, 0x00, 0x00,
];
const REQUEST_REGISTER_CLIENT: [u8; 12] = [0x04, 0, 0, 0, 0x0A, 0, 0, 0x80, 0x01, 0, 0, 0];
const RESPONSE_REGISTER_CLIENT: [u8; 32] = [
    0x00, 0x00, 0x00, 0x00, 0x06, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
    0x00, //
    0x53, 0x46, 0x43, 0x4F, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
];
const REQUEST_GET_SERVICE_HANDLE: [u8; 8] = [0x04, 0, 0, 0, 0x0A, 0, 0, 0];
/// One move handle (at 0x0C) and the result at 0x18.
const RESPONSE_WITH_HANDLE: [u8; 32] = [
    0x00, 0x00, 0x00, 0x00, 0x04, 0x00, 0x00, 0x80, 0x20, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
    0x00, //
    0x53, 0x46, 0x43, 0x4F, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
];
/// The result at 0x18.
const RESPONSE_UNREGISTER: [u8; 40] = [
    0x00, 0x00, 0x00, 0x00, 0x08, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
    0x00, //
    0x53, 0x46, 0x43, 0x4F, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
    0x00, //
    0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
];
const REQUEST_REGISTER_SERVICE: [u8; 8] = [0x04, 0, 0, 0, 0x0C, 0, 0, 0];

/// `sm::impl::ValidateServiceName`: non-empty and all zero after the terminator.
pub fn valid_service_name(name: &[u8; 8]) -> bool {
    if name[0] == 0 {
        return false;
    }
    let end = name.iter().position(|&b| b == 0).unwrap_or(8);
    name[end..].iter().all(|&b| b == 0)
}
/// `ValidateAccessControl` against the process's ACI0 service access control: an entry of
/// the same direction matches exactly, or by prefix when it ends in `*`.
fn allowed(k: &Kernel, name: &[u8; 8], host: bool) -> bool {
    k.config.service_access.iter().any(|&(is_host, pattern)| {
        let p = pattern.as_bytes();
        is_host == host
            && match p.split_last() {
                Some((b'*', prefix)) => name.starts_with(prefix),
                _ => {
                    let mut padded = [0u8; 8];
                    let n = p.len().min(8);
                    padded[..n].copy_from_slice(&p[..n]);
                    padded == *name && p.len() <= 8
                }
            }
    })
}

/// A HIPC request as `hipcParseRequest` reads it: the header, the special header, the
/// descriptors (address, size) and the raw data words.
#[derive(Clone, Debug, Default)]
pub struct Request {
    pub kind: u16,
    /// The process ID the kernel placed in the special header.
    pub pid: Option<u64>,
    pid_offset: Option<usize>,
    pub copy_handles: u32,
    pub move_handles: u32,
    /// Send statics (X), send (A), receive (B) and the count of exchange (W) buffers.
    pub statics: Vec<(u64, u64)>,
    pub sends: Vec<(u64, u64)>,
    pub recvs: Vec<(u64, u64)>,
    pub exchanges: u32,
    /// Byte offset and length in words of the raw data.
    pub data: usize,
    pub data_words: usize,
    pub recv_static_mode: u32,
    /// Bytes up to the end of the raw data.
    pub size: usize,
}

fn parse_request(m: &[u8; MESSAGE_BUFFER_SIZE]) -> Request {
    let (w0, w1) = (read_u32(m, 0), read_u32(m, 4));
    let mut r = Request {
        kind: w0 as u16,
        exchanges: w0 >> 28,
        data_words: (w1 & 0x3ff) as usize,
        recv_static_mode: (w1 >> 10) & 0xf,
        ..Request::default()
    };
    let (statics, sends, recvs) = ((w0 >> 16) & 0xf, (w0 >> 20) & 0xf, (w0 >> 24) & 0xf);
    let mut at = 8;
    if w1 >> 31 != 0 {
        let special = read_u32(m, 8);
        at += 4;
        if special & 1 != 0 {
            r.pid_offset = Some(at);
            at += 8;
        }
        r.copy_handles = (special >> 1) & 0xf;
        r.move_handles = (special >> 5) & 0xf;
        at += 4 * (r.copy_handles + r.move_handles) as usize;
    }
    let descriptors = at;
    at += 8 * statics as usize + 12 * (sends + recvs + r.exchanges) as usize;
    r.data = at;
    r.size = at + 4 * r.data_words;
    if r.size > MESSAGE_BUFFER_SIZE {
        return r;
    }
    let mut d = descriptors;
    for _ in 0..statics {
        let (a, b) = (read_u32(m, d), read_u32(m, d + 4));
        let address =
            u64::from(b) | u64::from((a >> 12) & 0xf) << 32 | u64::from((a >> 6) & 0x3f) << 36;
        r.statics.push((address, u64::from(a >> 16)));
        d += 8;
    }
    let buffer = |d: &mut usize| {
        let (size, low, c) = (read_u32(m, *d), read_u32(m, *d + 4), read_u32(m, *d + 8));
        *d += 12;
        let address = u64::from(low) | u64::from(c >> 28) << 32 | u64::from((c >> 2) & 7) << 36;
        (address, u64::from(size) | u64::from((c >> 24) & 0xf) << 32)
    };
    for _ in 0..sends {
        r.sends.push(buffer(&mut d));
    }
    for _ in 0..recvs {
        r.recvs.push(buffer(&mut d));
    }
    r
}

/// Result of a request: a reply to write (bytes, optional new handle object to place at
/// offset 0x0C), the server closing the session, or a deferral.
enum Reply {
    Bytes(Vec<u8>),
    Close,
    /// GetServiceHandle deferred; `tipc` selects the layout of the eventual reply.
    Defer {
        name: [u8; 8],
        tipc: bool,
    },
}

/// svcSendSyncRequest (0x21): W0 client session handle; the request and the reply occupy the
/// calling thread's message buffer.
pub fn send_sync_request(
    memory: &mut Memory,
    k: &mut Kernel,
    c: &mut Cpu,
    handle: u32,
) -> Result<Option<Stop>, String> {
    let Some(Object::Session(s)) = k.handles.get(handle) else {
        c.x[0] = u64::from(result::INVALID_HANDLE);
        return Ok(None);
    };
    k.ipc.requests += 1;
    if k.sessions[s].closed {
        c.x[0] = u64::from(result::SESSION_CLOSED);
        return Ok(None);
    }
    let endpoint = k.sessions[s].endpoint;
    if let Endpoint::Port(p) = endpoint {
        return Err(format!(
            "unsupported Horizon IPC: request to {:?}, a service hosted by the guest (svcAcceptSession/svcReplyAndReceive are not implemented)",
            name_text(&k.ports[p].name)
        ));
    }
    let tls = k.threads[k.current].tls;
    let mut message = [0u8; MESSAGE_BUFFER_SIZE];
    memory
        .read_into(tls, &mut message)
        .map_err(|e| format!("IPC message buffer unreadable: {e}"))?;
    // Kernel-side message validation: everything must fit the 0x100-byte buffer.
    let mut req = parse_request(&message);
    if req.size > MESSAGE_BUFFER_SIZE {
        c.x[0] = u64::from(result::MESSAGE_TOO_LARGE);
        return Ok(None);
    }
    // The kernel supplies the sender's process ID in the special header.
    if let Some(at) = req.pid_offset {
        message[at..at + 8].copy_from_slice(&k.config.process_id.to_le_bytes());
        req.pid = Some(k.config.process_id);
    }
    let reply = match endpoint {
        Endpoint::Sm { client } => {
            if req.copy_handles + req.move_handles != 0
                || !(req.statics.is_empty() && req.sends.is_empty() && req.recvs.is_empty())
                || req.exchanges != 0
                || req.recv_static_mode != 0
            {
                return Err(
                    "unsupported Horizon IPC: handles or buffer descriptors in a request to sm:"
                        .into(),
                );
            }
            sm_request(k, s, client, &mut message)
        }
        Endpoint::Host(object) => {
            if req.copy_handles + req.move_handles != 0 {
                return Err(format!(
                    "unsupported Horizon IPC: handles sent to a hosted service ({object:?})"
                ));
            }
            match services::request(memory, k, s, object, &req, &message)? {
                Outcome::Reply(bytes) => Reply::Bytes(bytes),
                Outcome::Close => Reply::Close,
            }
        }
        Endpoint::Port(_) => unreachable!("rejected above"),
    };
    // Replies completed for other threads by this request (deferred lookups).
    for (address, bytes) in std::mem::take(&mut k.pending_replies) {
        memory
            .write_bytes(address, &bytes)
            .map_err(|e| format!("IPC message buffer unwritable: {e}"))?;
    }
    match reply {
        Reply::Bytes(bytes) => {
            memory
                .write_bytes(tls, &bytes)
                .map_err(|e| format!("IPC message buffer unwritable: {e}"))?;
            k.ipc.replies += 1;
            c.x[0] = u64::from(result::SUCCESS);
            Ok(None)
        }
        Reply::Close => {
            k.sessions[s].closed = true;
            k.ipc.sessions_closed_by_server += 1;
            c.x[0] = u64::from(result::SESSION_CLOSED);
            Ok(None)
        }
        Reply::Defer { name, tipc } => {
            k.ipc.deferred += 1;
            Ok(threads::block_for(
                k,
                c,
                0x21,
                Wait::Service {
                    session: s,
                    name,
                    tipc,
                },
            ))
        }
    }
}

/// One request to sm, as Atmosphère's `UserService::ProcessDefaultServiceCommand`.
fn sm_request(
    k: &mut Kernel,
    s: usize,
    client: Option<u64>,
    message: &mut [u8; MESSAGE_BUFFER_SIZE],
) -> Reply {
    let tag = u16::from_le_bytes([message[0], message[1]]);
    if tag == 5 || tag == 7 {
        return Reply::Bytes(RESPONSE_POINTER_BUFFER_SIZE.to_vec());
    }
    if tag != 4 && tag != 6 {
        return tipc_request(k, s, client, tag, message);
    }
    message[0..2].copy_from_slice(&4u16.to_le_bytes());
    let name: [u8; 8] = message[0x20..0x28].try_into().unwrap_or_default();
    if message[..12] == REQUEST_REGISTER_CLIENT {
        let pid = u64::from_le_bytes(message[0xC..0x14].try_into().unwrap_or_default());
        match read_u32(message, 0x28) {
            0 => k.sessions[s].endpoint = Endpoint::Sm { client: Some(pid) },
            4 => k.sessions[s].endpoint = Endpoint::Sm { client: None },
            _ => return Reply::Close,
        }
        return Reply::Bytes(RESPONSE_REGISTER_CLIENT.to_vec());
    }
    if message[..8] == REQUEST_GET_SERVICE_HANDLE {
        return match read_u32(message, 0x18) {
            1 => match get_service_handle(k, client, &name) {
                Err(Deferred) => Reply::Defer { name, tipc: false },
                Ok(r) => Reply::Bytes(with_handle(r)),
            },
            3 => {
                let r = unregister_service(k, client, &name);
                let mut out = RESPONSE_UNREGISTER.to_vec();
                out[0x18..0x1C].copy_from_slice(&r.to_le_bytes());
                Reply::Bytes(out)
            }
            _ => Reply::Close,
        };
    }
    if message[..8] == REQUEST_REGISTER_SERVICE && read_u32(message, 0x18) == 2 {
        let is_light = message[0x28] & 1 != 0;
        let max_sessions = read_u32(message, 0x2C);
        return Reply::Bytes(with_handle(register_service(
            k,
            client,
            &name,
            max_sessions,
            is_light,
        )));
    }
    Reply::Close
}

/// TIPC tags: the command ID plus 0x10; 15 closes the session.
const TIPC_CLOSE: u16 = 15;
const TIPC_REGISTER_CLIENT: u16 = 0x10;
const TIPC_GET_SERVICE_HANDLE: u16 = 0x11;
const TIPC_REGISTER_SERVICE: u16 = 0x12;
const TIPC_UNREGISTER_SERVICE: u16 = 0x13;
const TIPC_DETACH_CLIENT: u16 = 0x14;
/// Special header of a request sending the process ID and no handles.
const SPECIAL_PROCESS_ID: u32 = 1;
/// Special header of a reply moving one handle.
const SPECIAL_ONE_MOVE_HANDLE: u32 = 1 << 5;

/// One TIPC request to sm (`tipc::impl::InvokeServiceCommandImpl`): the first two header
/// words must equal the command's (tag, raw data words, special-header flag), and its special
/// header must match; otherwise the request fails with InvalidMessageFormat and the session is
/// closed.
fn tipc_request(
    k: &mut Kernel,
    s: usize,
    client: Option<u64>,
    tag: u16,
    message: &[u8; MESSAGE_BUFFER_SIZE],
) -> Reply {
    let header = (read_u32(message, 0), read_u32(message, 4));
    let expect = |words: u32| header == (u32::from(tag), words);
    let name: [u8; 8] = message[8..16].try_into().unwrap_or_default();
    match tag {
        TIPC_REGISTER_CLIENT | TIPC_DETACH_CLIENT
            if expect(1 << 31) && read_u32(message, 8) == SPECIAL_PROCESS_ID =>
        {
            let pid = u64::from_le_bytes(message[12..20].try_into().unwrap_or_default());
            let client = (tag == TIPC_REGISTER_CLIENT).then_some(pid);
            k.sessions[s].endpoint = Endpoint::Sm { client };
            Reply::Bytes(tipc_result(tag, result::SUCCESS))
        }
        TIPC_GET_SERVICE_HANDLE if expect(2) => match get_service_handle(k, client, &name) {
            Err(Deferred) => Reply::Defer { name, tipc: true },
            Ok(r) => Reply::Bytes(tipc_with_handle(tag, r)),
        },
        // ServiceName, u32 max_sessions, bool is_light: 13 bytes in four words.
        TIPC_REGISTER_SERVICE if expect(4) => {
            let max_sessions = read_u32(message, 16);
            let is_light = message[20] & 1 != 0;
            Reply::Bytes(tipc_with_handle(
                tag,
                register_service(k, client, &name, max_sessions, is_light),
            ))
        }
        TIPC_UNREGISTER_SERVICE if expect(2) => {
            Reply::Bytes(tipc_result(tag, unregister_service(k, client, &name)))
        }
        // Close: the server closes its end, the client sees SessionClosed.
        TIPC_CLOSE if expect(0) => Reply::Close,
        // InvalidMessageFormat or InvalidMethod: the session is closed without a reply.
        _ => Reply::Close,
    }
}

/// A TIPC reply with only the result: one raw data word.
fn tipc_result(tag: u16, code: u32) -> Vec<u8> {
    [u32::from(tag), 1, code]
        .iter()
        .flat_map(|w| w.to_le_bytes())
        .collect()
}

/// A TIPC reply moving one handle (`Ok(handle)`, or handle 0 with `Err(result)`): the handle
/// in the special data, then the result.
fn tipc_with_handle(tag: u16, r: Result<u32, u32>) -> Vec<u8> {
    let (handle, code) = match r {
        Ok(h) => (h, result::SUCCESS),
        Err(e) => (0, e),
    };
    [
        u32::from(tag),
        1 | 1 << 31,
        SPECIAL_ONE_MOVE_HANDLE,
        handle,
        code,
    ]
    .iter()
    .flat_map(|w| w.to_le_bytes())
    .collect()
}

/// A reply carrying one move handle: `Ok(handle)` or `Err(result)` with handle 0.
fn with_handle(r: Result<u32, u32>) -> Vec<u8> {
    let mut out = RESPONSE_WITH_HANDLE.to_vec();
    let (handle, code) = match r {
        Ok(h) => (h, result::SUCCESS),
        Err(e) => (0, e),
    };
    out[0x0C..0x10].copy_from_slice(&handle.to_le_bytes());
    out[0x18..0x1C].copy_from_slice(&code.to_le_bytes());
    out
}

/// sm:GetServiceHandle could not complete yet: the service is not registered.
struct Deferred;

fn get_service_handle(
    k: &mut Kernel,
    client: Option<u64>,
    name: &[u8; 8],
) -> Result<Result<u32, u32>, Deferred> {
    if client.is_none() {
        return Ok(Err(sm_result::INVALID_CLIENT));
    }
    if !valid_service_name(name) {
        return Ok(Err(sm_result::INVALID_SERVICE_NAME));
    }
    if !allowed(k, name, false) {
        return Ok(Err(sm_result::NOT_ALLOWED));
    }
    let Some(service) = k.services.iter().find(|s| s.name == *name).copied() else {
        return match services::lookup(name) {
            Some(object) => Ok(open_session(k, Endpoint::Host(object))),
            None => Err(Deferred),
        };
    };
    let port = &mut k.ports[service.port];
    if port.server_closed {
        return Ok(Err(result::PORT_CLOSED));
    }
    if port.sessions >= port.max_sessions {
        return Ok(Err(sm_result::OUT_OF_SESSIONS));
    }
    port.sessions += 1;
    Ok(open_session(k, Endpoint::Port(service.port)))
}

fn register_service(
    k: &mut Kernel,
    client: Option<u64>,
    name: &[u8; 8],
    max_sessions: u32,
    is_light: bool,
) -> Result<u32, u32> {
    let owner = client.ok_or(sm_result::INVALID_CLIENT)?;
    if !valid_service_name(name) {
        return Err(sm_result::INVALID_SERVICE_NAME);
    }
    if !allowed(k, name, true) {
        return Err(sm_result::NOT_ALLOWED);
    }
    if k.services.iter().any(|s| s.name == *name) || services::lookup(name).is_some() {
        return Err(sm_result::ALREADY_REGISTERED);
    }
    let p = k.ports.len();
    k.ports.push(Port {
        name: *name,
        max_sessions,
        sessions: 0,
        is_light,
        server_closed: false,
        references: 1,
    });
    let handle = k.handles.add(Object::Port(p)).inspect_err(|_| {
        k.ports.pop();
    })?;
    k.services.push(Service {
        name: *name,
        owner,
        port: p,
    });
    retry_deferred(k);
    Ok(handle)
}

fn unregister_service(k: &mut Kernel, client: Option<u64>, name: &[u8; 8]) -> u32 {
    let Some(owner) = client else {
        return sm_result::INVALID_CLIENT;
    };
    if !valid_service_name(name) {
        return sm_result::INVALID_SERVICE_NAME;
    }
    let Some(i) = k.services.iter().position(|s| s.name == *name) else {
        return sm_result::NOT_REGISTERED;
    };
    if k.services[i].owner != owner {
        return sm_result::NOT_ALLOWED;
    }
    k.services.remove(i);
    result::SUCCESS
}

/// Re-runs deferred GetServiceHandle requests after a registration; completed ones write
/// their reply into the waiting thread's message buffer and wake it.
fn retry_deferred(k: &mut Kernel) {
    let waiting: Vec<(usize, usize, [u8; 8], bool)> = k
        .threads
        .iter()
        .enumerate()
        .filter_map(|(t, th)| match th.wait {
            Some(Wait::Service {
                session,
                name,
                tipc,
            }) => Some((t, session, name, tipc)),
            _ => None,
        })
        .collect();
    for (t, session, name, tipc) in waiting {
        let Endpoint::Sm { client } = k.sessions[session].endpoint else {
            continue;
        };
        if let Ok(r) = get_service_handle(k, client, &name) {
            let reply = if tipc {
                tipc_with_handle(TIPC_GET_SERVICE_HANDLE, r)
            } else {
                with_handle(r)
            };
            k.pending_replies.push((k.threads[t].tls, reply));
            k.ipc.replies += 1;
            threads::end_wait(k, t, result::SUCCESS, None);
        }
    }
}
