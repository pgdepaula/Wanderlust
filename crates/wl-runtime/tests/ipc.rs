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
    assert!(
        dispatch(&mut c, &mut p, 0x21)
            .unwrap_err()
            .contains("hosted by the guest")
    );
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
    assert!(
        dispatch(&mut c, &mut p, 0x21)
            .unwrap_err()
            .contains("descriptors")
    );
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
