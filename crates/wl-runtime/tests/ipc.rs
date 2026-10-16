//! Horizon IPC: named ports, client sessions, HIPC validation and the sm: server.
use wl_runtime::horizon::*;
use wl_runtime::ipc::sm_result;
use wl_runtime::threads::{ThreadState, Wait};
use wl_runtime::{Cpu, Memory, Stop};

const CODE: u64 = 0x800_0000;
const NAME: u64 = CODE + 0x1000;

fn config(access: &'static [(bool, &'static str)]) -> ProcessConfig {
    ProcessConfig {
        program_id: 0x0100_0000_0000_2000,
        process_id: 0x51,
        main_thread_id: 0x72,
        main_thread_stack_size: 0x10_0000,
        main_thread_priority: 44,
        ideal_core: 0,
        core_mask: 0b11,
        priority_mask: u64::MAX,
        memory_limit: 0x2000_0000,
        handle_table_size: 64,
        thread_limit: 8,
        is_application: true,
        svc_permissions: [u64::MAX; 3],
        service_access: access,
    }
}
fn process(access: &'static [(bool, &'static str)]) -> (Process, Cpu) {
    let mut m = Memory::default();
    m.map_state(CODE, vec![0; 0x1000], true, false, true, state::CODE)
        .unwrap();
    m.map_state(
        CODE + 0x1000,
        vec![0; 0x3000],
        true,
        true,
        false,
        state::CODE_DATA,
    )
    .unwrap();
    Process::horizon(
        m,
        CODE,
        Range {
            start: CODE,
            end: CODE + 0x4000,
        },
        config(access),
    )
    .unwrap()
}
fn svc(p: &mut Process, c: &mut Cpu, id: u16, args: &[u64]) -> (u32, Option<Stop>) {
    c.x[..args.len()].copy_from_slice(args);
    let stop = dispatch(c, p, id).unwrap();
    (c.x[0] as u32, stop)
}
fn tls(p: &Process) -> u64 {
    let k = p.kernel.as_ref().unwrap();
    k.threads[k.current].tls
}
fn connect(p: &mut Process, c: &mut Cpu) -> u32 {
    p.write_bytes(NAME, b"sm:\0").unwrap();
    assert_eq!(svc(p, c, 0x1F, &[0, NAME]).0, result::SUCCESS);
    c.x[1] as u32
}
/// Writes a CMIF request as libnx's cmifMakeRequest lays it out (no domain, no buffers).
fn request(p: &mut Process, command: u32, send_pid: bool, data: &[u8]) {
    let mut m = vec![0u8; 0x100];
    let words = (16 + 16 + data.len()).div_ceil(4) as u32;
    m[0..4].copy_from_slice(&4u32.to_le_bytes());
    m[4..8].copy_from_slice(&(words | if send_pid { 1 << 31 } else { 0 }).to_le_bytes());
    let mut at = 8;
    if send_pid {
        m[8..12].copy_from_slice(&1u32.to_le_bytes());
        at = 20;
    }
    let start = (at + 15) & !15;
    m[start..start + 4].copy_from_slice(b"SFCI");
    m[start + 8..start + 12].copy_from_slice(&command.to_le_bytes());
    m[start + 16..start + 16 + data.len()].copy_from_slice(data);
    let t = tls(p);
    p.write_bytes(t, &m).unwrap();
}
fn send(p: &mut Process, c: &mut Cpu, session: u32) -> (u32, Option<Stop>) {
    svc(p, c, 0x21, &[u64::from(session)])
}
fn reply(p: &Process) -> Vec<u8> {
    p.read_bytes(tls(p), 0x40).unwrap().to_vec()
}
fn reply_result(p: &Process) -> u32 {
    u32::from_le_bytes(reply(p)[0x18..0x1C].try_into().unwrap())
}
fn reply_handle(p: &Process) -> u32 {
    u32::from_le_bytes(reply(p)[0x0C..0x10].try_into().unwrap())
}
fn name(s: &str) -> [u8; 8] {
    let mut n = [0u8; 8];
    n[..s.len()].copy_from_slice(s.as_bytes());
    n
}
fn register_client(p: &mut Process, c: &mut Cpu, session: u32) {
    request(p, 0, true, &[0; 8]);
    assert_eq!(send(p, c, session), (result::SUCCESS, None));
    assert_eq!(&reply(p)[0x10..0x14], b"SFCO");
}

#[test]
fn connect_to_named_port_validates_the_name() {
    let (mut p, mut c) = process(&[(false, "*")]);
    let session = connect(&mut p, &mut c);
    assert_ne!(session, 0);
    p.write_bytes(NAME, b"fs\0").unwrap();
    assert_eq!(svc(&mut p, &mut c, 0x1F, &[0, NAME]).0, result::NOT_FOUND);
    assert_eq!(c.x[1], 0, "no handle on failure");
    p.write_bytes(NAME, b"abcdefghijkl").unwrap();
    assert_eq!(
        svc(&mut p, &mut c, 0x1F, &[0, NAME]).0,
        result::OUT_OF_RANGE
    );
    assert_eq!(
        svc(&mut p, &mut c, 0x1F, &[0, 0x10]).0,
        result::INVALID_CURRENT_MEMORY
    );
}

#[test]
fn register_client_then_lookup_and_registration() {
    let (mut p, mut c) = process(&[(false, "*"), (true, "wl:*")]);
    let sm = connect(&mut p, &mut c);
    // Before RegisterClient every command answers InvalidClient.
    request(&mut p, 1, false, &name("wl:echo"));
    send(&mut p, &mut c, sm);
    assert_eq!(reply_result(&p), sm_result::INVALID_CLIENT);
    register_client(&mut p, &mut c, sm);
    // The kernel supplied the process ID in the request's special header.
    let k = p.kernel.as_ref().unwrap();
    assert!(matches!(
        k.sessions[0].endpoint,
        wl_runtime::ipc::Endpoint::Sm { client: Some(0x51) }
    ));
    // Malformed name.
    let mut bad = [0u8; 8];
    bad[0] = b'a';
    bad[2] = b'b';
    request(&mut p, 1, false, &bad);
    send(&mut p, &mut c, sm);
    assert_eq!(
        (reply_result(&p), reply_handle(&p)),
        (sm_result::INVALID_SERVICE_NAME, 0)
    );
    // Hosting outside the access control is not allowed; "wl:*" is.
    let mut register = name("xx:svc").to_vec();
    register.extend([0, 0, 0, 0, 1, 0, 0, 0]);
    request(&mut p, 2, false, &register);
    send(&mut p, &mut c, sm);
    assert_eq!(reply_result(&p), sm_result::NOT_ALLOWED);
    let mut register = name("wl:svc").to_vec();
    register.extend([0, 0, 0, 0, 1, 0, 0, 0]);
    request(&mut p, 2, false, &register);
    send(&mut p, &mut c, sm);
    assert_eq!(reply_result(&p), result::SUCCESS);
    let port = reply_handle(&p);
    assert!(matches!(
        p.kernel.as_ref().unwrap().handles.get(port),
        Some(Object::Port(_))
    ));
    request(&mut p, 2, false, &register);
    send(&mut p, &mut c, sm);
    assert_eq!(reply_result(&p), sm_result::ALREADY_REGISTERED);
    // One session allowed: the second lookup fails with sm OutOfSessions.
    request(&mut p, 1, false, &name("wl:svc"));
    send(&mut p, &mut c, sm);
    let client = reply_handle(&p);
    assert!(matches!(
        p.kernel.as_ref().unwrap().handles.get(client),
        Some(Object::Session(_))
    ));
    request(&mut p, 1, false, &name("wl:svc"));
    send(&mut p, &mut c, sm);
    assert_eq!(reply_result(&p), sm_result::OUT_OF_SESSIONS);
    // Requests to a guest-hosted service are not modelled: explicit fault.
    c.x[0] = u64::from(client);
    assert!(dispatch(&mut c, &mut p, 0x21)
        .unwrap_err()
        .contains("hosted by the guest"));
    // Unregister: success, then NotRegistered.
    request(&mut p, 3, false, &name("wl:svc"));
    send(&mut p, &mut c, sm);
    assert_eq!(reply_result(&p), result::SUCCESS);
    request(&mut p, 3, false, &name("wl:svc"));
    send(&mut p, &mut c, sm);
    assert_eq!(reply_result(&p), sm_result::NOT_REGISTERED);
}

#[test]
fn access_control_restricts_lookups() {
    let (mut p, mut c) = process(&[(false, "fsp-srv")]);
    let sm = connect(&mut p, &mut c);
    register_client(&mut p, &mut c, sm);
    request(&mut p, 1, false, &name("vi:u"));
    send(&mut p, &mut c, sm);
    assert_eq!(reply_result(&p), sm_result::NOT_ALLOWED);
}

#[test]
fn deferred_lookup_completes_on_registration() {
    let (mut p, mut c) = process(&[(false, "*"), (true, "*")]);
    let sm = connect(&mut p, &mut c);
    register_client(&mut p, &mut c, sm);
    request(&mut p, 1, false, &name("wl:late"));
    let pc = c.pc;
    assert_eq!(send(&mut p, &mut c, sm).1, Some(Stop::Reschedule));
    assert_eq!(c.pc, pc + 4);
    let k = p.kernel.as_mut().unwrap();
    assert!(matches!(k.threads[0].wait, Some(Wait::Service { .. })));
    // A second thread registers the service on its own session; the waiter completes.
    k.threads[0].context = Some(Box::new(c.clone()));
    let stack = k.threads[0].stack.end - 0x8000;
    let mut c2 = c.clone();
    c2.x[..6].copy_from_slice(&[0, CODE, 0, stack, 44, 1]);
    dispatch(&mut c2, &mut p, 0x08).unwrap();
    let k = p.kernel.as_mut().unwrap();
    let t = k.threads.len() - 1;
    let mut c2 = *k.threads[t].context.take().unwrap();
    k.current = t;
    let sm2 = connect(&mut p, &mut c2);
    register_client(&mut p, &mut c2, sm2);
    let mut register = name("wl:late").to_vec();
    register.extend([0, 0, 0, 0, 4, 0, 0, 0]);
    request(&mut p, 2, false, &register);
    // Waking the deferred client hands control back to the scheduler.
    c2.x[0] = u64::from(sm2);
    assert_eq!(
        dispatch(&mut c2, &mut p, 0x21).unwrap(),
        Some(Stop::Reschedule)
    );
    let k = p.kernel.as_ref().unwrap();
    assert_eq!(k.threads[0].state, ThreadState::Runnable);
    assert_eq!(k.threads[0].context.as_ref().unwrap().x[0], 0);
    let main_tls = k.threads[0].tls;
    let reply = p.read_bytes(main_tls, 0x20).unwrap();
    assert_eq!(&reply[0x10..0x14], b"SFCO");
    let handle = u32::from_le_bytes(reply[0x0C..0x10].try_into().unwrap());
    assert!(matches!(k.handles.get(handle), Some(Object::Session(_))));
    assert_eq!(k.ipc.deferred, 1);
}

#[test]
fn control_close_and_invalid_requests() {
    let (mut p, mut c) = process(&[(false, "*")]);
    let sm = connect(&mut p, &mut c);
    // Control requests get the pointer-buffer size (zero) reply.
    let t = tls(&p);
    p.write_bytes(t, &[5, 0, 0, 0, 8, 0, 0, 0]).unwrap();
    assert_eq!(send(&mut p, &mut c, sm), (result::SUCCESS, None));
    assert_eq!(reply(&p)[4], 7);
    // A message larger than the buffer is rejected by the kernel.
    p.write_bytes(t, &[4, 0, 0, 0, 0xff, 0x03, 0, 0]).unwrap();
    assert_eq!(send(&mut p, &mut c, sm).0, result::MESSAGE_TOO_LARGE);
    // Buffer descriptors to sm are not modelled: explicit fault.
    p.write_bytes(t, &[4, 0, 0x10, 0, 0x0A, 0, 0, 0]).unwrap();
    c.x[0] = u64::from(sm);
    assert!(dispatch(&mut c, &mut p, 0x21)
        .unwrap_err()
        .contains("descriptors"));
    // An unknown command: the server closes the session; every later request fails.
    request(&mut p, 99, false, &[0; 8]);
    assert_eq!(send(&mut p, &mut c, sm).0, result::SESSION_CLOSED);
    register_client_fails(&mut p, &mut c, sm);
    // CMIF Close (libnx serviceClose) also ends in SessionClosed.
    let sm2 = connect(&mut p, &mut c);
    p.write_bytes(t, &[2, 0, 0, 0, 0, 0, 0, 0]).unwrap();
    assert_eq!(send(&mut p, &mut c, sm2).0, result::SESSION_CLOSED);
    // Closed and unknown handles.
    assert_eq!(
        svc(&mut p, &mut c, 0x16, &[u64::from(sm2)]).0,
        result::SUCCESS
    );
    assert_eq!(send(&mut p, &mut c, sm2).0, result::INVALID_HANDLE);
    let thread = p.kernel.as_ref().unwrap().threads[0].handle;
    assert_eq!(send(&mut p, &mut c, thread).0, result::INVALID_HANDLE);
    assert_eq!(&c.x[1..8], &[0; 7]);
}
fn register_client_fails(p: &mut Process, c: &mut Cpu, sm: u32) {
    request(p, 0, true, &[0; 8]);
    assert_eq!(send(p, c, sm).0, result::SESSION_CLOSED);
}

/// Writes a TIPC request as libnx's tipcMakeRequest lays it out: tag = command + 16, the
/// raw data directly after the headers, the process ID slot filled in by the kernel.
fn tipc_request(p: &mut Process, command: u16, send_pid: bool, data: &[u8]) {
    let mut m = vec![0u8; 0x100];
    let words = data.len().div_ceil(4) as u32;
    m[0..4].copy_from_slice(&u32::from(command + 16).to_le_bytes());
    m[4..8].copy_from_slice(&(words | if send_pid { 1 << 31 } else { 0 }).to_le_bytes());
    let mut at = 8;
    if send_pid {
        m[8..12].copy_from_slice(&1u32.to_le_bytes());
        at = 20;
    }
    m[at..at + data.len()].copy_from_slice(data);
    let t = tls(p);
    p.write_bytes(t, &m).unwrap();
}
fn words(p: &Process, n: usize) -> Vec<u32> {
    reply(p)[..4 * n]
        .chunks(4)
        .map(|w| u32::from_le_bytes(w.try_into().unwrap()))
        .collect()
}
fn tipc_register_client(p: &mut Process, c: &mut Cpu, session: u32) {
    tipc_request(p, 0, true, &[]);
    assert_eq!(send(p, c, session), (result::SUCCESS, None));
    assert_eq!(words(p, 3), [0x10, 1, result::SUCCESS]);
}

#[test]
fn tipc_register_client_lookup_and_registration() {
    let (mut p, mut c) = process(&[(false, "*"), (true, "wl:*")]);
    let sm = connect(&mut p, &mut c);
    // Before RegisterClient: InvalidClient, the handle slot zero.
    tipc_request(&mut p, 1, false, &name("wl:echo"));
    assert_eq!(send(&mut p, &mut c, sm), (result::SUCCESS, None));
    assert_eq!(
        words(&p, 5),
        [0x11, 1 | 1 << 31, 1 << 5, 0, sm_result::INVALID_CLIENT]
    );
    tipc_register_client(&mut p, &mut c, sm);
    assert!(matches!(
        p.kernel.as_ref().unwrap().sessions[0].endpoint,
        wl_runtime::ipc::Endpoint::Sm { client: Some(0x51) }
    ));
    // RegisterService(name, max_sessions = 1, is_light = false).
    let mut register = name("wl:svc").to_vec();
    register.extend([1, 0, 0, 0, 0]);
    tipc_request(&mut p, 2, false, &register);
    send(&mut p, &mut c, sm);
    let w = words(&p, 5);
    assert_eq!((w[0], w[1], w[2], w[4]), (0x12, 1 | 1 << 31, 1 << 5, 0));
    assert!(matches!(
        p.kernel.as_ref().unwrap().handles.get(w[3]),
        Some(Object::Port(_))
    ));
    tipc_request(&mut p, 2, false, &register);
    send(&mut p, &mut c, sm);
    assert_eq!(words(&p, 5)[4], sm_result::ALREADY_REGISTERED);
    tipc_request(&mut p, 1, false, &name("wl:svc"));
    send(&mut p, &mut c, sm);
    let w = words(&p, 5);
    assert_eq!(w[4], result::SUCCESS);
    assert!(matches!(
        p.kernel.as_ref().unwrap().handles.get(w[3]),
        Some(Object::Session(_))
    ));
    tipc_request(&mut p, 1, false, &name("wl:svc"));
    send(&mut p, &mut c, sm);
    assert_eq!(words(&p, 5)[3..], [0, sm_result::OUT_OF_SESSIONS]);
    // UnregisterService: result-only replies.
    tipc_request(&mut p, 3, false, &name("wl:svc"));
    send(&mut p, &mut c, sm);
    assert_eq!(words(&p, 3), [0x13, 1, result::SUCCESS]);
    tipc_request(&mut p, 3, false, &name("wl:svc"));
    send(&mut p, &mut c, sm);
    assert_eq!(words(&p, 3), [0x13, 1, sm_result::NOT_REGISTERED]);
    // DetachClient: later lookups are InvalidClient again.
    tipc_request(&mut p, 4, true, &[]);
    send(&mut p, &mut c, sm);
    assert_eq!(words(&p, 3), [0x14, 1, result::SUCCESS]);
    tipc_request(&mut p, 1, false, &name("wl:svc"));
    send(&mut p, &mut c, sm);
    assert_eq!(words(&p, 5)[4], sm_result::INVALID_CLIENT);
}

#[test]
fn tipc_deferred_lookup_replies_in_tipc() {
    let (mut p, mut c) = process(&[(false, "*"), (true, "*")]);
    let sm = connect(&mut p, &mut c);
    tipc_register_client(&mut p, &mut c, sm);
    tipc_request(&mut p, 1, false, &name("wl:late"));
    assert_eq!(send(&mut p, &mut c, sm).1, Some(Stop::Reschedule));
    let k = p.kernel.as_mut().unwrap();
    assert!(matches!(
        k.threads[0].wait,
        Some(Wait::Service { tipc: true, .. })
    ));
    k.threads[0].context = Some(Box::new(c.clone()));
    let stack = k.threads[0].stack.end - 0x8000;
    let mut c2 = c.clone();
    c2.x[..6].copy_from_slice(&[0, CODE, 0, stack, 44, 1]);
    dispatch(&mut c2, &mut p, 0x08).unwrap();
    let k = p.kernel.as_mut().unwrap();
    let t = k.threads.len() - 1;
    let mut c2 = *k.threads[t].context.take().unwrap();
    k.current = t;
    let sm2 = connect(&mut p, &mut c2);
    tipc_register_client(&mut p, &mut c2, sm2);
    let mut register = name("wl:late").to_vec();
    register.extend([4, 0, 0, 0, 0]);
    tipc_request(&mut p, 2, false, &register);
    c2.x[0] = u64::from(sm2);
    assert_eq!(
        dispatch(&mut c2, &mut p, 0x21).unwrap(),
        Some(Stop::Reschedule)
    );
    let k = p.kernel.as_ref().unwrap();
    assert_eq!(k.threads[0].state, ThreadState::Runnable);
    let reply = p.read_bytes(k.threads[0].tls, 20).unwrap();
    let w: Vec<u32> = reply
        .chunks(4)
        .map(|w| u32::from_le_bytes(w.try_into().unwrap()))
        .collect();
    assert_eq!((w[0], w[1], w[2], w[4]), (0x11, 1 | 1 << 31, 1 << 5, 0));
    assert!(matches!(k.handles.get(w[3]), Some(Object::Session(_))));
}

#[test]
fn tipc_malformed_unknown_and_close_end_the_session() {
    let (mut p, mut c) = process(&[(false, "*")]);
    // GetServiceHandle with three data words: InvalidMessageFormat closes the session.
    let sm = connect(&mut p, &mut c);
    tipc_register_client(&mut p, &mut c, sm);
    tipc_request(
        &mut p,
        1,
        false,
        &[b'f', b's', 0, 0, 0, 0, 0, 0, 0, 0, 0, 0],
    );
    assert_eq!(send(&mut p, &mut c, sm).0, result::SESSION_CLOSED);
    tipc_request(&mut p, 0, true, &[]);
    assert_eq!(send(&mut p, &mut c, sm).0, result::SESSION_CLOSED);
    // RegisterClient without the process ID.
    let sm = connect(&mut p, &mut c);
    tipc_request(&mut p, 0, false, &[]);
    assert_eq!(send(&mut p, &mut c, sm).0, result::SESSION_CLOSED);
    // An unknown command (Atmosphère extensions are not Nintendo's sm).
    let sm = connect(&mut p, &mut c);
    tipc_request(&mut p, 65100, false, &name("fs"));
    assert_eq!(send(&mut p, &mut c, sm).0, result::SESSION_CLOSED);
    // TIPC Close (libnx tipcClose).
    let sm = connect(&mut p, &mut c);
    let t = tls(&p);
    p.write_bytes(t, &[15, 0, 0, 0, 0, 0, 0, 0]).unwrap();
    assert_eq!(send(&mut p, &mut c, sm).0, result::SESSION_CLOSED);
}

const PACKET: u64 = NAME + 0x200;

/// Opens `lm` through TIPC sm and its ILogger through ILogService::OpenLogger.
fn open_logger(p: &mut Process, c: &mut Cpu) -> u32 {
    let sm = connect(p, c);
    tipc_register_client(p, c, sm);
    tipc_request(p, 1, false, &name("lm"));
    send(p, c, sm);
    let w = words(p, 5);
    assert_eq!(w[4], result::SUCCESS);
    let lm = w[3];
    // OpenLogger: the process ID placeholder in the data, the real one from the kernel.
    request(p, 0, true, &[0; 8]);
    assert_eq!(send(p, c, lm), (result::SUCCESS, None));
    let r = reply(p);
    assert_eq!(&r[0x10..0x14], b"SFCO");
    assert_eq!(reply_result(p), result::SUCCESS);
    let logger = reply_handle(p);
    let k = p.kernel.as_ref().unwrap();
    let Some(Object::Session(session)) = k.handles.get(logger) else {
        panic!("OpenLogger moved no session handle");
    };
    assert!(matches!(
        k.sessions[session].endpoint,
        wl_runtime::ipc::Endpoint::Host(wl_runtime::services::HostObject::Logger {
            process_id: 0x51
        })
    ));
    logger
}
/// A log packet: header, then a text chunk (key 2).
fn packet(flags: u8, text: &str) -> Vec<u8> {
    let mut body = vec![2, text.len() as u8];
    body.extend(text.as_bytes());
    let mut m = vec![0u8; 0x18];
    m[8..16].copy_from_slice(&7u64.to_le_bytes());
    m[0x10] = flags | 4;
    m[0x14..0x18].copy_from_slice(&(body.len() as u32).to_le_bytes());
    m.extend(body);
    m
}
/// ILogger::Log with an In|AutoSelect buffer: a pointer (X) descriptor carrying the data and
/// an empty map (A) descriptor, or the reverse when `mapped`.
fn log(p: &mut Process, c: &mut Cpu, logger: u32, bytes: &[u8], mapped: bool) -> u32 {
    p.write_bytes(PACKET, bytes).unwrap();
    let (x, a) = if mapped {
        ((0, 0), (PACKET, bytes.len() as u32))
    } else {
        ((PACKET, bytes.len() as u32), (0, 0))
    };
    let mut m = vec![0u8; 0x100];
    m[0..4].copy_from_slice(&(4u32 | 1 << 16 | 1 << 20).to_le_bytes());
    m[4..8].copy_from_slice(&8u32.to_le_bytes());
    let xw0 = (x.1 << 16) | (((x.0 >> 32) as u32 & 0xf) << 12);
    m[8..12].copy_from_slice(&xw0.to_le_bytes());
    m[12..16].copy_from_slice(&(x.0 as u32).to_le_bytes());
    m[16..20].copy_from_slice(&a.1.to_le_bytes());
    m[20..24].copy_from_slice(&(a.0 as u32).to_le_bytes());
    m[24..28].copy_from_slice(&((((a.0 >> 32) as u32) & 0xf) << 28).to_le_bytes());
    m[32..36].copy_from_slice(b"SFCI");
    let t = tls(p);
    p.write_bytes(t, &m).unwrap();
    assert_eq!(send(p, c, logger), (result::SUCCESS, None));
    reply_result(p)
}

#[test]
fn lm_logger_records_log_packets() {
    let (mut p, mut c) = process(&[(false, "*")]);
    let logger = open_logger(&mut p, &mut c);
    assert_eq!(log(&mut p, &mut c, logger, &packet(3, "hello\n"), false), 0);
    // A message split over a head and a tail packet, the second through a mapped buffer.
    assert_eq!(
        log(&mut p, &mut c, logger, &packet(1, "part one, "), false),
        0
    );
    assert_eq!(log(&mut p, &mut c, logger, &packet(2, "part two"), true), 0);
    // A packet whose size disagrees with its header succeeds and is dropped.
    let mut bad = packet(3, "dropped");
    bad.push(0);
    assert_eq!(log(&mut p, &mut c, logger, &bad, false), 0);
    let k = p.kernel.as_ref().unwrap();
    assert_eq!(k.debug_output, ["hello", "part one, part two"]);
    assert_eq!(k.host.log_packets, 3);
    // SetDestination(u32).
    request(&mut p, 1, false, &2u32.to_le_bytes());
    send(&mut p, &mut c, logger);
    assert_eq!(reply_result(&p), result::SUCCESS);
    assert_eq!(p.kernel.as_ref().unwrap().host.log_destination, Some(2));
}

#[test]
fn hosted_service_control_requests_and_limits() {
    let (mut p, mut c) = process(&[(false, "*"), (true, "*")]);
    let logger = open_logger(&mut p, &mut c);
    let t = tls(&p);
    // QueryPointerBufferSize: lm's 0x400-byte pointer buffer.
    let mut m = [0u8; 0x30];
    m[0] = 5;
    m[4] = 8;
    m[0x10..0x14].copy_from_slice(b"SFCI");
    m[0x18] = 3;
    p.write_bytes(t, &m).unwrap();
    assert_eq!(send(&mut p, &mut c, logger), (result::SUCCESS, None));
    let r = reply(&p);
    assert_eq!(
        (&r[0x10..0x14], r[0x18], &r[0x20..0x22]),
        (&b"SFCO"[..], 0, &[0, 4][..])
    );
    // CloneCurrentObject: another session to the same logger.
    m[0x18] = 2;
    p.write_bytes(t, &m).unwrap();
    send(&mut p, &mut c, logger);
    let clone = reply_handle(&p);
    assert_eq!(
        log(&mut p, &mut c, clone, &packet(3, "from the clone"), false),
        0
    );
    // Converting twice and unknown commands are not modelled: explicit faults.
    m[0x18] = 0;
    p.write_bytes(t, &m).unwrap();
    assert_eq!(send(&mut p, &mut c, logger), (result::SUCCESS, None));
    p.write_bytes(t, &m).unwrap();
    c.x[0] = u64::from(logger);
    assert!(dispatch(&mut c, &mut p, 0x21)
        .unwrap_err()
        .contains("converting a domain session"));
    request(&mut p, 9, false, &[]);
    c.x[0] = u64::from(clone);
    assert!(dispatch(&mut c, &mut p, 0x21)
        .unwrap_err()
        .contains("unknown command"));
    // The name cannot be registered by the guest; CMIF Close ends the session.
    let sm = connect(&mut p, &mut c);
    tipc_register_client(&mut p, &mut c, sm);
    let mut register = name("lm").to_vec();
    register.extend([1, 0, 0, 0, 0]);
    tipc_request(&mut p, 2, false, &register);
    send(&mut p, &mut c, sm);
    assert_eq!(words(&p, 5)[4], sm_result::ALREADY_REGISTERED);
    p.write_bytes(t, &[2, 0, 0, 0, 0, 0, 0, 0]).unwrap();
    assert_eq!(send(&mut p, &mut c, clone).0, result::SESSION_CLOSED);
    assert_eq!(p.kernel.as_ref().unwrap().debug_output, ["from the clone"]);
}

#[test]
fn hosted_services_obey_access_control() {
    let (mut p, mut c) = process(&[(false, "fsp-srv")]);
    let sm = connect(&mut p, &mut c);
    tipc_register_client(&mut p, &mut c, sm);
    tipc_request(&mut p, 1, false, &name("lm"));
    send(&mut p, &mut c, sm);
    assert_eq!(words(&p, 5)[3..], [0, sm_result::NOT_ALLOWED]);
}

/// A CMIF request to object `id` of a domain, laid out as libnx's cmifMakeRequest: the domain
/// header (SendMessage, no input objects, message size, object ID) before `SFCI`.
fn domain_request(p: &mut Process, kind: u8, id: u32, command: u32, send_pid: bool, data: &[u8]) {
    let mut m = vec![0u8; 0x100];
    let words = (16 + 16 + 16 + data.len()).div_ceil(4) as u32;
    m[0..4].copy_from_slice(&4u32.to_le_bytes());
    m[4..8].copy_from_slice(&(words | if send_pid { 1 << 31 } else { 0 }).to_le_bytes());
    let mut at = 8;
    if send_pid {
        m[8..12].copy_from_slice(&1u32.to_le_bytes());
        at = 20;
    }
    let start = (at + 15) & !15;
    m[start] = kind;
    m[start + 2..start + 4].copy_from_slice(&((16 + data.len()) as u16).to_le_bytes());
    m[start + 4..start + 8].copy_from_slice(&id.to_le_bytes());
    m[start + 16..start + 20].copy_from_slice(b"SFCI");
    m[start + 24..start + 28].copy_from_slice(&command.to_le_bytes());
    m[start + 32..start + 32 + data.len()].copy_from_slice(data);
    let t = tls(p);
    p.write_bytes(t, &m).unwrap();
}
fn control(p: &mut Process, command: u32, data: &[u8]) {
    let mut m = [0u8; 0x40];
    m[0] = 5;
    m[4] = ((32 + data.len()).div_ceil(4)) as u8;
    m[0x10..0x14].copy_from_slice(b"SFCI");
    m[0x18..0x1C].copy_from_slice(&command.to_le_bytes());
    m[0x20..0x20 + data.len()].copy_from_slice(data);
    let t = tls(p);
    p.write_bytes(t, &m).unwrap();
}

#[test]
fn lm_through_a_cmif_domain() {
    let (mut p, mut c) = process(&[(false, "*")]);
    let sm = connect(&mut p, &mut c);
    tipc_register_client(&mut p, &mut c, sm);
    tipc_request(&mut p, 1, false, &name("lm"));
    send(&mut p, &mut c, sm);
    let lm = words(&p, 5)[3];
    // ConvertCurrentObjectToDomain: the service object becomes object 1.
    control(&mut p, 0, &[]);
    assert_eq!(send(&mut p, &mut c, lm), (result::SUCCESS, None));
    let r = reply(&p);
    assert_eq!(
        (&r[0x10..0x14], reply_result(&p), &r[0x20..0x24]),
        (&b"SFCO"[..], 0, &[1, 0, 0, 0][..])
    );
    // OpenLogger in the domain: one output object, ID 2, after the (empty) output data.
    domain_request(&mut p, 1, 1, 0, true, &[0; 8]);
    assert_eq!(send(&mut p, &mut c, lm), (result::SUCCESS, None));
    let r = reply(&p);
    let w = |at: usize| u32::from_le_bytes(r[at..at + 4].try_into().unwrap());
    // Header: no special header; raw data = domain header, SFCO header, one ID, padding.
    assert_eq!((w(0), w(4)), (0, (16 + 16 + 4 + 3 + 16) / 4));
    assert_eq!(
        (w(0x10), &r[0x20..0x24], w(0x28), w(0x30)),
        (1, &b"SFCO"[..], 0, 2)
    );
    // Log to object 2 through the domain.
    let bytes = packet(3, "domain log");
    p.write_bytes(PACKET, &bytes).unwrap();
    let mut m = vec![0u8; 0x100];
    m[0..4].copy_from_slice(&(4u32 | 1 << 16 | 1 << 20).to_le_bytes());
    m[4..8].copy_from_slice(&12u32.to_le_bytes());
    m[8..12].copy_from_slice(&((bytes.len() as u32) << 16).to_le_bytes());
    m[12..16].copy_from_slice(&(PACKET as u32).to_le_bytes());
    m[32] = 1;
    m[34] = 16;
    m[36] = 2;
    m[48..52].copy_from_slice(b"SFCI");
    let t = tls(&p);
    p.write_bytes(t, &m).unwrap();
    assert_eq!(send(&mut p, &mut c, lm), (result::SUCCESS, None));
    assert_eq!(&reply(&p)[0x20..0x24], b"SFCO");
    assert_eq!(p.kernel.as_ref().unwrap().debug_output, ["domain log"]);
    // CopyFromCurrentDomain(2): a plain session to the logger.
    control(&mut p, 1, &2u32.to_le_bytes());
    send(&mut p, &mut c, lm);
    let copy = reply_handle(&p);
    assert_eq!(log(&mut p, &mut c, copy, &packet(3, "copied"), false), 0);
    control(&mut p, 1, &9u32.to_le_bytes());
    send(&mut p, &mut c, lm);
    assert_eq!(
        u32::from_le_bytes(reply(&p)[0x18..0x1C].try_into().unwrap()),
        11 | 492 << 9
    );
    // Domain Close of object 2: no reply is written; later messages to it close the session.
    domain_request(&mut p, 2, 2, 0, false, &[]);
    let before = reply(&p);
    assert_eq!(send(&mut p, &mut c, lm), (result::SUCCESS, None));
    assert_eq!(reply(&p), before);
    // The freed ID 2 is reused by the next object.
    domain_request(&mut p, 1, 1, 0, true, &[0; 8]);
    send(&mut p, &mut c, lm);
    assert_eq!(&reply(&p)[0x30..0x34], &[2, 0, 0, 0]);
    domain_request(&mut p, 2, 2, 0, false, &[]);
    send(&mut p, &mut c, lm);
    // A message to the closed object: TargetNotFound closes the session.
    domain_request(&mut p, 1, 2, 1, false, &[0; 4]);
    assert_eq!(send(&mut p, &mut c, lm).0, result::SESSION_CLOSED);
    assert_eq!(send(&mut p, &mut c, lm).0, result::SESSION_CLOSED);
}

fn service(p: &mut Process, c: &mut Cpu, sm: u32, service: &str) -> u32 {
    tipc_request(p, 1, false, &name(service));
    send(p, c, sm);
    let w = words(p, 5);
    assert_eq!(w[4], result::SUCCESS);
    w[3]
}
fn reply_u32(p: &Process, at: usize) -> u32 {
    u32::from_le_bytes(reply(p)[at..at + 4].try_into().unwrap())
}
fn reply_u64(p: &Process, at: usize) -> u64 {
    u64::from_le_bytes(reply(p)[at..at + 8].try_into().unwrap())
}

#[test]
fn apm_performance_mode_and_configuration() {
    let (mut p, mut c) = process(&[(false, "*")]);
    let sm = connect(&mut p, &mut c);
    tipc_register_client(&mut p, &mut c, sm);
    let apm = service(&mut p, &mut c, sm, "apm");
    // GetPerformanceMode: docked, Boost.
    request(&mut p, 1, false, &[]);
    send(&mut p, &mut c, apm);
    assert_eq!((reply_result(&p), reply_u32(&p, 0x20)), (0, 1));
    // OpenSession, then a configuration set and read back.
    request(&mut p, 0, false, &[]);
    send(&mut p, &mut c, apm);
    let session = reply_handle(&p);
    let mut set = 1u32.to_le_bytes().to_vec();
    set.extend(0x0001_0000u32.to_le_bytes());
    request(&mut p, 0, false, &set);
    send(&mut p, &mut c, session);
    assert_eq!(reply_result(&p), 0);
    request(&mut p, 1, false, &1u32.to_le_bytes());
    send(&mut p, &mut c, session);
    assert_eq!((reply_result(&p), reply_u32(&p, 0x20)), (0, 0x0001_0000));
    // Undocumented outcomes are explicit faults.
    for (command, data) in [
        (1u32, 0u32.to_le_bytes().to_vec()),
        (
            0,
            [2u32.to_le_bytes(), 0x0001_0000u32.to_le_bytes()].concat(),
        ),
        (
            0,
            [0u32.to_le_bytes(), 0x0001_0002u32.to_le_bytes()].concat(),
        ),
    ] {
        request(&mut p, command, false, &data);
        c.x[0] = u64::from(session);
        assert!(dispatch(&mut c, &mut p, 0x21).is_err());
    }
    control(&mut p, 3, &[]);
    c.x[0] = u64::from(apm);
    assert!(dispatch(&mut c, &mut p, 0x21)
        .unwrap_err()
        .contains("pointer-buffer size"));
}

#[test]
fn domain_ids_are_allocated_per_server() {
    let (mut p, mut c) = process(&[(false, "*")]);
    let sm = connect(&mut p, &mut c);
    tipc_register_client(&mut p, &mut c, sm);
    let lm = service(&mut p, &mut c, sm, "lm");
    control(&mut p, 0, &[]);
    send(&mut p, &mut c, lm);
    assert_eq!(reply_u32(&p, 0x20), 1);
    domain_request(&mut p, 1, 1, 0, true, &[0; 8]);
    send(&mut p, &mut c, lm);
    assert_eq!(reply_u32(&p, 0x30), 2);
    // apm's server issues its own IDs from 1.
    let apm = service(&mut p, &mut c, sm, "apm");
    control(&mut p, 0, &[]);
    send(&mut p, &mut c, apm);
    assert_eq!(reply_u32(&p, 0x20), 1);
    domain_request(&mut p, 1, 1, 0, false, &[]);
    send(&mut p, &mut c, apm);
    assert_eq!((reply_u32(&p, 0x10), reply_u32(&p, 0x30)), (1, 2));
}

/// A domain request with copied handles, as libnx's cmifMakeRequest lays it out.
fn domain_request_with(
    p: &mut Process,
    id: u32,
    command: u32,
    send_pid: bool,
    copies: &[u32],
    data: &[u8],
) {
    let mut m = vec![0u8; 0x100];
    let words = (16 + 16 + 16 + data.len()).div_ceil(4) as u32;
    m[0..4].copy_from_slice(&4u32.to_le_bytes());
    m[4..8].copy_from_slice(&(words | 1 << 31).to_le_bytes());
    let special = u32::from(send_pid) | (copies.len() as u32) << 1;
    m[8..12].copy_from_slice(&special.to_le_bytes());
    let mut at = if send_pid { 20 } else { 12 };
    for h in copies {
        m[at..at + 4].copy_from_slice(&h.to_le_bytes());
        at += 4;
    }
    let start = (at + 15) & !15;
    m[start] = 1;
    m[start + 2..start + 4].copy_from_slice(&((16 + data.len()) as u16).to_le_bytes());
    m[start + 4..start + 8].copy_from_slice(&id.to_le_bytes());
    m[start + 16..start + 20].copy_from_slice(b"SFCI");
    m[start + 24..start + 28].copy_from_slice(&command.to_le_bytes());
    m[start + 32..start + 32 + data.len()].copy_from_slice(data);
    let t = tls(p);
    p.write_bytes(t, &m).unwrap();
}
/// Sends a domain request with no special header and returns (result, first data word,
/// first output object ID) of a reply without handles.
fn call(
    p: &mut Process,
    c: &mut Cpu,
    session: u32,
    id: u32,
    command: u32,
    data: &[u8],
) -> (u32, u32, u32) {
    domain_request(p, 1, id, command, false, data);
    assert_eq!(send(p, c, session), (result::SUCCESS, None));
    (reply_u32(p, 0x28), reply_u32(p, 0x30), reply_u32(p, 0x30))
}

#[test]
fn applet_oe_application_startup() {
    let (mut p, mut c) = process(&[(false, "*")]);
    let sm = connect(&mut p, &mut c);
    tipc_register_client(&mut p, &mut c, sm);
    let am = service(&mut p, &mut c, sm, "appletOE");
    control(&mut p, 0, &[]);
    send(&mut p, &mut c, am);
    assert_eq!(reply_u32(&p, 0x20), 1);
    // am has no pointer buffer.
    control(&mut p, 3, &[]);
    send(&mut p, &mut c, am);
    assert_eq!(
        (reply_u32(&p, 0x18), &reply(&p)[0x20..0x22]),
        (0, &[0, 0][..])
    );
    // OpenApplicationProxy: process ID, the current-process pseudo-handle copied.
    domain_request_with(&mut p, 1, 0, true, &[CURRENT_PROCESS], &[0; 8]);
    assert_eq!(send(&mut p, &mut c, am), (result::SUCCESS, None));
    assert_eq!(
        (
            reply_u32(&p, 0x10),
            reply_u32(&p, 0x28),
            reply_u32(&p, 0x30)
        ),
        (1, 0, 2)
    );
    // An invalid copied handle fails the send in the kernel.
    domain_request_with(&mut p, 1, 0, true, &[0x1234], &[0; 8]);
    assert_eq!(send(&mut p, &mut c, am).0, result::INVALID_HANDLE);
    // The proxy's interfaces.
    let getter = call(&mut p, &mut c, am, 2, 0, &[]).2;
    let controller = call(&mut p, &mut c, am, 2, 1, &[]).2;
    let window = call(&mut p, &mut c, am, 2, 2, &[]).2;
    assert_eq!((getter, controller, window), (3, 4, 5));
    for command in [3, 4, 11, 20, 1000] {
        assert_eq!(call(&mut p, &mut c, am, 2, command, &[]).0, 0);
    }
    // GetEventHandle: a copied readable event, signalled because startup messages are queued.
    domain_request(&mut p, 1, getter, 0, false, &[]);
    send(&mut p, &mut c, am);
    let r = reply(&p);
    assert_eq!(reply_u32(&p, 4) >> 31, 1);
    assert_eq!(reply_u32(&p, 8), 1 << 1);
    let event = reply_u32(&p, 0x0C);
    assert_eq!((&r[0x20..0x24], reply_u32(&p, 0x28)), (&b"SFCO"[..], 0));
    assert!(matches!(
        p.kernel.as_ref().unwrap().handles.get(event),
        Some(Object::ReadableEvent(_))
    ));
    let list = NAME + 0x80;
    p.write_bytes(list, &event.to_le_bytes()).unwrap();
    assert_eq!(
        svc(&mut p, &mut c, 0x18, &[0, list, 1, 0]).0,
        result::SUCCESS
    );
    // ReceiveMessage: first FocusStateChanged (15), then ChangeIntoForeground (1).
    assert_eq!(call(&mut p, &mut c, am, getter, 1, &[]), (0, 15, 15));
    assert_eq!(call(&mut p, &mut c, am, getter, 1, &[]), (0, 1, 1));
    // ReceiveMessage on now empty queue: am NoMessage (0x680), and the event resets.
    assert_eq!(call(&mut p, &mut c, am, getter, 1, &[]).0, 0x680);
    assert_eq!(
        svc(&mut p, &mut c, 0x18, &[0, list, 1, 0]).0,
        result::TIMED_OUT
    );
    // Operation mode Console, performance mode Boost, in focus.
    assert_eq!(call(&mut p, &mut c, am, getter, 5, &[]).0, 0);
    assert_eq!(reply(&p)[0x30], 1);
    assert_eq!(call(&mut p, &mut c, am, getter, 6, &[]).1, 1);
    call(&mut p, &mut c, am, getter, 9, &[]);
    assert_eq!(reply(&p)[0x30], 1);
    // SetRequestExitToLibraryAppletAtExecuteNextProgramEnabled.
    assert_eq!(call(&mut p, &mut c, am, getter, 900, &[]).0, 0);
    // GetThisAppletKind, GetCradleStatus, GetBootMode, InitializeGamePlayRecording.
    assert_eq!(call(&mut p, &mut c, am, getter, 2, &[]).1, 3);
    assert_eq!(call(&mut p, &mut c, am, getter, 7, &[]).0, 0);
    assert_eq!(reply(&p)[0x30], 1);
    assert_eq!(call(&mut p, &mut c, am, getter, 8, &[]).0, 0);
    assert_eq!(reply(&p)[0x30], 0);
    assert_eq!(call(&mut p, &mut c, am, getter, 66, &[]).0, 0);
    // Never suspended: tick value 0, a copied event that is not signalled.
    assert_eq!(call(&mut p, &mut c, am, controller, 90, &[]), (0, 0, 0));
    domain_request(&mut p, 1, controller, 91, false, &[]);
    send(&mut p, &mut c, am);
    let tick_event = reply_u32(&p, 0x0C);
    assert_eq!((reply_u32(&p, 8), reply_u32(&p, 0x28)), (1 << 1, 0));
    p.write_bytes(list, &tick_event.to_le_bytes()).unwrap();
    assert_eq!(
        svc(&mut p, &mut c, 0x18, &[0, list, 1, 0]).0,
        result::TIMED_OUT
    );
    // IApplicationFunctions (object 9): the GPU-error event, never signalled.
    domain_request(&mut p, 1, 9, 130, false, &[]);
    send(&mut p, &mut c, am);
    assert_eq!((reply_u32(&p, 8), reply_u32(&p, 0x28)), (1 << 1, 0));
    assert_eq!(call(&mut p, &mut c, am, 9, 131, &5u64.to_le_bytes()).0, 0);
    // IApplicationFunctions command 210: event handle, never signalled.
    domain_request(&mut p, 1, 9, 210, false, &[]);
    send(&mut p, &mut c, am);
    assert_eq!((reply_u32(&p, 8), reply_u32(&p, 0x28)), (1 << 1, 0));
    // IApplicationFunctions commands 20, 21, 22, 23, 40.
    assert_eq!(call(&mut p, &mut c, am, 9, 20, &[]).0, 0);
    assert_eq!(call(&mut p, &mut c, am, 9, 21, &[]).0, 0);
    assert_eq!(&reply(&p)[0x30..0x38], b"en-US\0\0\0");
    assert_eq!(call(&mut p, &mut c, am, 9, 22, &[0; 4]).0, 0);
    assert_eq!(call(&mut p, &mut c, am, 9, 23, &[]).0, 0);
    assert_eq!(call(&mut p, &mut c, am, 9, 40, &[]).0, 0);
    assert_eq!(reply(&p)[0x30], 1);
    // The applet resource user ID is the process ID.
    let (r, aruid, _) = call(&mut p, &mut c, am, window, 1, &[]);
    assert_eq!((r, aruid), (0, 0x51));
    assert_eq!(call(&mut p, &mut c, am, window, 10, &[]).0, 0);
    // ISelfController settings.
    assert_eq!(call(&mut p, &mut c, am, controller, 13, &[1, 0, 1]).0, 0);
    assert_eq!(call(&mut p, &mut c, am, controller, 11, &[1]).0, 0);
    assert_eq!(call(&mut p, &mut c, am, controller, 12, &[1]).0, 0);
    assert_eq!(call(&mut p, &mut c, am, controller, 14, &[1]).0, 0);
    assert_eq!(call(&mut p, &mut c, am, controller, 40, &[]).0, 0);
    assert_eq!(call(&mut p, &mut c, am, controller, 44, &[]).0, 0);
    assert_eq!(reply_u32(&p, 0x30), 1);
    assert_eq!(reply_u32(&p, 0x38), 2);
    let h = &p.kernel.as_ref().unwrap().host;
    assert_eq!(h.focus_handling_mode, Some([true, false, true]));
    assert_eq!(
        (
            h.operation_mode_notification,
            h.performance_mode_notification,
            h.restart_message_enabled,
        ),
        (Some(true), Some(true), Some(true))
    );
    assert!(h.foreground_rights && h.request_exit_to_library_applet_at_execute_next_program);
    assert_eq!(h.delay_time_to_abort_on_gpu_error, Some(5));
    assert_eq!(h.calls.last(), Some(&("am ISelfController", 44, 0)));
}

#[test]
fn aoc_u_queries_add_on_content() {
    let (mut p, mut c) = process(&[(false, "*")]);
    let sm = connect(&mut p, &mut c);
    tipc_register_client(&mut p, &mut c, sm);
    let aoc = service(&mut p, &mut c, sm, "aoc:u");
    // CountAddOnContent: out u32 count (0).
    request(&mut p, 2, true, &[]);
    assert_eq!(send(&mut p, &mut c, aoc), (result::SUCCESS, None));
    assert_eq!(reply_result(&p), result::SUCCESS);
    assert_eq!(reply_u32(&p, 0x20), 0);
}

#[test]
fn ectx_aw_creates_context_registrar() {
    let (mut p, mut c) = process(&[(false, "*")]);
    let sm = connect(&mut p, &mut c);
    tipc_register_client(&mut p, &mut c, sm);
    let ectx = service(&mut p, &mut c, sm, "ectx:aw");
    // CreateContextRegistrar (cmd 0): out move handle.
    request(&mut p, 0, false, &[]);
    assert_eq!(send(&mut p, &mut c, ectx), (result::SUCCESS, None));
    assert_eq!(reply_result(&p), result::SUCCESS);
    let registrar = reply_u32(&p, 0x0C);
    assert_ne!(registrar, 0);
    // Complete (cmd 0) on registrar.
    request(&mut p, 0, false, &[]);
    assert_eq!(send(&mut p, &mut c, registrar), (result::SUCCESS, None));
    assert_eq!(reply_result(&p), result::SUCCESS);
}

#[test]
fn pctl_creates_service_and_checks_restrictions() {
    let (mut p, mut c) = process(&[(false, "*")]);
    let sm = connect(&mut p, &mut c);
    tipc_register_client(&mut p, &mut c, sm);
    let pctl = service(&mut p, &mut c, sm, "pctl");
    // CreateService (cmd 0): out move handle.
    request(&mut p, 0, true, &[]);
    assert_eq!(send(&mut p, &mut c, pctl), (result::SUCCESS, None));
    assert_eq!(reply_result(&p), result::SUCCESS);
    let svc = reply_u32(&p, 0x0C);
    assert_ne!(svc, 0);
    // IsRestrictionEnabled (cmd 1011): out bool (false).
    request(&mut p, 1011, false, &[]);
    assert_eq!(send(&mut p, &mut c, svc), (result::SUCCESS, None));
    assert_eq!(reply_result(&p), result::SUCCESS);
    assert_eq!(reply(&p)[0x20], 0);
}

#[test]
fn time_and_acc_services() {
    let (mut p, mut c) = process(&[(false, "*")]);
    let sm = connect(&mut p, &mut c);
    tipc_register_client(&mut p, &mut c, sm);
    let time = service(&mut p, &mut c, sm, "time:u");
    // GetStandardUserSystemClock (cmd 0): out move handle.
    request(&mut p, 0, false, &[]);
    assert_eq!(send(&mut p, &mut c, time), (result::SUCCESS, None));
    assert_eq!(reply_result(&p), result::SUCCESS);
    let clock = reply_u32(&p, 0x0C);
    assert_ne!(clock, 0);
    // GetCurrentTime (cmd 0) on clock: out u64 posix seconds.
    request(&mut p, 0, false, &[]);
    assert_eq!(send(&mut p, &mut c, clock), (result::SUCCESS, None));
    assert_eq!(reply_result(&p), result::SUCCESS);
    assert_eq!(reply_u32(&p, 0x20), 1_700_000_000);

    // GetSharedMemoryNativeHandle (cmd 20): out copy handle of KSharedMemory.
    request(&mut p, 20, false, &[]);
    assert_eq!(send(&mut p, &mut c, time), (result::SUCCESS, None));
    assert_eq!(reply_result(&p), result::SUCCESS);
    let shmem = reply_u32(&p, 0x0C);
    assert_ne!(shmem, 0);
    assert!(matches!(
        p.kernel.as_ref().unwrap().handles.get(shmem),
        Some(Object::SharedMemory(_))
    ));

    // Map the shared memory via svcMapSharedMemory (0x13).
    let target = 0x20_0000_0000u64;
    c.x[0] = u64::from(shmem);
    c.x[1] = target;
    c.x[2] = 0x1000;
    c.x[3] = 1; // Read
    dispatch(&mut c, &mut p, 0x13).unwrap();
    assert_eq!(c.x[0], 0);

    // Verify steady clock index and local clock posix epoch offset in shared memory.
    let mut steady_idx = [0u8; 4];
    p.read_into(target, &mut steady_idx).unwrap();
    assert_eq!(u32::from_le_bytes(steady_idx), 2);
    let mut local_epoch = [0u8; 8];
    p.read_into(target + 0x40, &mut local_epoch).unwrap();
    assert_eq!(i64::from_le_bytes(local_epoch), 1_700_000_000);

    // Unmap via svcUnmapSharedMemory (0x14).
    c.x[0] = u64::from(shmem);
    c.x[1] = target;
    c.x[2] = 0x1000;
    dispatch(&mut c, &mut p, 0x14).unwrap();
    assert_eq!(c.x[0], 0);

    let acc = service(&mut p, &mut c, sm, "acc:u0");
    // GetUserCount (cmd 0): out u32 (1).
    request(&mut p, 0, false, &[]);
    assert_eq!(send(&mut p, &mut c, acc), (result::SUCCESS, None));
    assert_eq!(reply_result(&p), result::SUCCESS);
    assert_eq!(reply_u32(&p, 0x20), 1);
}

#[test]
fn fsp_srv_service_startup() {
    let (mut p, mut c) = process(&[(false, "*")]);
    let sm = connect(&mut p, &mut c);
    tipc_register_client(&mut p, &mut c, sm);
    let fs = service(&mut p, &mut c, sm, "fsp-srv");

    // Initialize (cmd 1).
    request(&mut p, 1, false, &[0; 8]);
    assert_eq!(send(&mut p, &mut c, fs), (result::SUCCESS, None));
    assert_eq!(reply_result(&p), result::SUCCESS);

    // GetGlobalAccessLogMode (cmd 1006): out u32 (0).
    request(&mut p, 1006, false, &[]);
    assert_eq!(send(&mut p, &mut c, fs), (result::SUCCESS, None));
    assert_eq!(reply_result(&p), result::SUCCESS);
    assert_eq!(reply_u32(&p, 0x20), 0);

    // OpenRomFileSystemByCurrentProcess (cmd 203): returns IFileSystem.
    request(&mut p, 203, false, &[]);
    assert_eq!(send(&mut p, &mut c, fs), (result::SUCCESS, None));
    assert_eq!(reply_result(&p), result::SUCCESS);
    let romfs = reply_handle(&p);
    assert_ne!(romfs, 0);

    // OpenDataStorageByCurrentProcess (cmd 200): returns IStorage.
    request(&mut p, 200, false, &[]);
    assert_eq!(send(&mut p, &mut c, fs), (result::SUCCESS, None));
    assert_eq!(reply_result(&p), result::SUCCESS);
    let storage = reply_handle(&p);
    assert_ne!(storage, 0);
}

#[test]
fn nvdrv_and_vi_service_startup() {
    let (mut p, mut c) = process(&[(false, "*")]);
    let sm = connect(&mut p, &mut c);
    tipc_register_client(&mut p, &mut c, sm);

    // nvdrv
    let nv = service(&mut p, &mut c, sm, "nvdrv");
    // Initialize (cmd 3)
    request(&mut p, 3, false, &[0; 4]);
    assert_eq!(send(&mut p, &mut c, nv), (result::SUCCESS, None));
    assert_eq!(reply_result(&p), result::SUCCESS);
    // Open (cmd 0) -> fd 1, err 0
    request(&mut p, 0, false, &[]);
    assert_eq!(send(&mut p, &mut c, nv), (result::SUCCESS, None));
    assert_eq!(reply_result(&p), result::SUCCESS);
    assert_eq!(reply_u32(&p, 0x20), 1); // fd
    assert_eq!(reply_u32(&p, 0x24), 0); // err

    // vi:u
    let vi = service(&mut p, &mut c, sm, "vi:u");
    // GetDisplayService (cmd 0) -> IApplicationDisplayService
    request(&mut p, 0, false, &[0; 4]);
    assert_eq!(send(&mut p, &mut c, vi), (result::SUCCESS, None));
    assert_eq!(reply_result(&p), result::SUCCESS);
    let disp = reply_handle(&p);
    assert_ne!(disp, 0);

    // GetRelayService (cmd 100) -> IHOSBinderDriver
    request(&mut p, 100, false, &[]);
    assert_eq!(send(&mut p, &mut c, disp), (result::SUCCESS, None));
    assert_eq!(reply_result(&p), result::SUCCESS);
    let binder = reply_handle(&p);
    assert_ne!(binder, 0);

    // OpenDisplay (cmd 1010) -> display_id = 1
    request(&mut p, 1010, false, &[]);
    assert_eq!(send(&mut p, &mut c, disp), (result::SUCCESS, None));
    assert_eq!(reply_result(&p), result::SUCCESS);
    assert_eq!(reply_u64(&p, 0x20), 1);

    // OpenLayer (cmd 2020) -> parcel_size = 60
    request(&mut p, 2020, false, &[]);
    assert_eq!(send(&mut p, &mut c, disp), (result::SUCCESS, None));
    assert_eq!(reply_result(&p), result::SUCCESS);
    assert_eq!(reply_u64(&p, 0x20), 60);

    // AdjustRefcount (cmd 1) on binder
    request(&mut p, 1, false, &[]);
    assert_eq!(send(&mut p, &mut c, binder), (result::SUCCESS, None));
    assert_eq!(reply_result(&p), result::SUCCESS);
}
