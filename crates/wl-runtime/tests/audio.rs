extern crate wl_runtime as wl_guest_runtime;
#[path = "../hosted/audio.rs"]
mod audio;
use wl_runtime::{Cpu, Memory, Stop, horizon::*};

const CODE: u64 = 0x8000000;
const DATA: u64 = CODE + 0x1000;
fn process() -> (Process, Cpu) {
    audio::install().unwrap();
    let mut memory = Memory::default();
    memory
        .map_state(CODE, vec![0; 0x1000], true, false, true, state::CODE)
        .unwrap();
    memory
        .map_state(DATA, vec![0; 0x40000], true, true, false, state::CODE_DATA)
        .unwrap();
    Process::horizon(
        memory,
        CODE,
        Range {
            start: CODE,
            end: DATA + 0x40000,
        },
        ProcessConfig {
            program_id: 1,
            process_id: 81,
            main_thread_id: 114,
            main_thread_stack_size: 0x100000,
            main_thread_priority: 44,
            ideal_core: 0,
            core_mask: 7,
            priority_mask: u64::MAX,
            memory_limit: 512 * 1024 * 1024,
            system_resource_size: 0,
            handle_table_size: 64,
            thread_limit: 8,
            is_application: true,
            svc_permissions: [u64::MAX; 3],
            service_access: &[(false, "audren:u")],
        },
    )
    .unwrap()
}
fn svc(p: &mut Process, c: &mut Cpu, id: u16, args: &[u64]) -> Option<Stop> {
    c.x[..args.len()].copy_from_slice(args);
    dispatch(c, p, id).unwrap()
}
fn tls(p: &Process) -> u64 {
    p.kernel.as_ref().unwrap().threads[0].tls
}
fn wire(p: &mut Process, command: u32, pid: bool, copies: &[u32], args: &[u8]) {
    let mut b = [0u8; 256];
    b[..4].copy_from_slice(&4u32.to_le_bytes());
    b[4..8].copy_from_slice(
        &(((32 + args.len()).div_ceil(4) as u32)
            | if pid || !copies.is_empty() {
                1 << 31
            } else {
                0
            })
        .to_le_bytes(),
    );
    let mut at: usize = 8;
    if pid || !copies.is_empty() {
        b[8..12].copy_from_slice(&(u32::from(pid) | (copies.len() as u32) << 1).to_le_bytes());
        at = 12;
        if pid {
            at += 8;
        }
        for h in copies {
            b[at..at + 4].copy_from_slice(&h.to_le_bytes());
            at += 4;
        }
    }
    at = at.next_multiple_of(16);
    b[at..at + 4].copy_from_slice(b"SFCI");
    b[at + 8..at + 12].copy_from_slice(&command.to_le_bytes());
    b[at + 16..at + 16 + args.len()].copy_from_slice(args);
    let address = tls(p);
    p.write_bytes(address, &b).unwrap();
}
fn reply(p: &Process, at: u64) -> u32 {
    p.load(tls(p) + at, 4).unwrap() as u32
}
fn call(p: &mut Process, c: &mut Cpu, handle: u32) {
    assert_eq!(svc(p, c, 0x21, &[u64::from(handle)]), None);
    assert_eq!(c.x[0], 0);
    assert_eq!(reply(p, 0x18), 0);
}
fn manager(p: &mut Process, c: &mut Cpu) -> u32 {
    p.write_bytes(DATA, b"sm:\0").unwrap();
    svc(p, c, 0x1f, &[0, DATA]);
    assert_eq!(c.x[0], 0);
    let sm = c.x[1] as u32;
    wire(p, 0, true, &[], &[0; 8]);
    call(p, c, sm);
    wire(p, 1, false, &[], b"audren:u");
    call(p, c, sm);
    reply(p, 12)
}
fn parameters(revision: u8) -> Vec<u8> {
    let mut b = vec![0; 0x34];
    for (at, v) in [
        (0, 48000u32),
        (4, 240),
        (8, 24),
        (16, 1),
        (20, 1),
        (48, 0x00564552 | (u32::from(b'0' + revision) << 24)),
    ] {
        b[at..at + 4].copy_from_slice(&v.to_le_bytes());
    }
    b
}
fn renderer(p: &mut Process, c: &mut Cpu, manager: u32) -> u32 {
    svc(p, c, 0x15, &[0, DATA + 0x10000, 0x10000, 0]);
    assert_eq!(c.x[0], 0);
    let transfer = c.x[1] as u32;
    let mut args = parameters(8);
    args.extend([0; 4]);
    args.extend(audio::WORK_BUFFER_BYTES.to_le_bytes());
    args.extend(0u64.to_le_bytes());
    wire(p, 0, true, &[transfer, CURRENT_PROCESS], &args);
    call(p, c, manager);
    reply(p, 12)
}
fn silent_input() -> Vec<u8> {
    // Literal section sizes from the public wire structures, not production sizing code.
    let sizes = [16u32, 128, 368, 0, 0, 2352, 320, 16, 0, 0];
    let total = 64 + sizes.iter().sum::<u32>();
    let mut b = vec![0; total as usize];
    b[..4].copy_from_slice(&0x38564552u32.to_le_bytes());
    for (i, v) in sizes.iter().enumerate() {
        b[4 + i * 4..8 + i * 4].copy_from_slice(&v.to_le_bytes());
    }
    b[60..64].copy_from_slice(&total.to_le_bytes());
    b[64..68].copy_from_slice(&0x38564552u32.to_le_bytes());
    for i in 0..4 {
        b[80 + i * 32 + 16..80 + i * 32 + 20].copy_from_slice(&1u32.to_le_bytes());
    }
    b
}

#[test]
fn service_discovery_queries_and_transfer_memory_open_use_real_ipc() {
    let (mut p, mut c) = process();
    let m = manager(&mut p, &mut c);
    wire(&mut p, 1, false, &[], &parameters(8));
    call(&mut p, &mut c, m);
    assert_eq!(p.load(tls(&p) + 0x20, 8).unwrap(), 0x10000);
    let r = renderer(&mut p, &mut c, m);
    for (cmd, expected) in [(0, 48000), (1, 240), (2, 24), (3, 1)] {
        wire(&mut p, cmd, false, &[], &[]);
        call(&mut p, &mut c, r);
        assert_eq!(reply(&p, 0x20), expected);
    }
    assert_eq!(p.kernel.as_ref().unwrap().events.len(), 1);
}
#[test]
fn renderer_event_requires_real_virtual_time_and_stop_disarms_it() {
    let (mut p, mut c) = process();
    let m = manager(&mut p, &mut c);
    let r = renderer(&mut p, &mut c, m);
    wire(&mut p, 7, false, &[], &[]);
    call(&mut p, &mut c, r);
    let handle = reply(&p, 12);
    let k = p.kernel.as_ref().unwrap();
    let Some(Object::ReadableEvent(event)) = k.handles.get(handle) else {
        panic!("not a readable event")
    };
    assert!(!k.events[event].signaled);
    wire(&mut p, 5, false, &[], &[]);
    call(&mut p, &mut c, r);
    let k = p.kernel.as_mut().unwrap();
    k.time_ns = 4_999_999;
    wl_runtime::audio::poll(&mut p.memory, k).unwrap();
    assert!(!k.events[event].signaled);
    k.time_ns = 5_000_000;
    wl_runtime::audio::poll(&mut p.memory, k).unwrap();
    assert!(k.events[event].signaled);

    assert_eq!(wl_runtime::threads::clear_event(k, handle), 0);
    wl_runtime::audio::poll(&mut p.memory, k).unwrap();
    assert!(!k.events[event].signaled);
    wire(&mut p, 6, false, &[], &[]);
    call(&mut p, &mut c, r);
    let k = p.kernel.as_mut().unwrap();
    k.time_ns = 50_000_000;
    wl_runtime::audio::poll(&mut p.memory, k).unwrap();
    assert!(!k.events[event].signaled);
    assert_eq!(wl_runtime::audio::next_deadline_tick(k), None);
}
#[test]
fn silent_update_has_independently_specified_status_bytes() {
    let params = audio::Parameters::parse(&parameters(8)).unwrap();
    let r = audio::Renderer::new(params, 0);
    let memory = Memory::default();
    let input = silent_input();
    let output = r.update(&input, &memory).unwrap();
    assert_eq!(output.len(), 384);
    assert_eq!(&output[..4], b"REV8");
    assert_eq!(u32::from_le_bytes(output[60..64].try_into().unwrap()), 384);
    for i in 0..4 {
        assert_eq!(
            u32::from_le_bytes(output[64 + i * 16..68 + i * 16].try_into().unwrap()),
            1
        );
    }
    assert!(output[128..].iter().all(|v| *v == 0));
}
#[test]
fn elapsed_render_info_begins_at_revision_five() {
    for revision in [1, 4, 5, 8] {
        let params = audio::Parameters::parse(&parameters(revision)).unwrap();
        let mut r = audio::Renderer::new(params, 0);
        r.elapsed_frames = 123;
        let mut input = silent_input();
        let magic = 0x00564552 | (u32::from(b'0' + revision) << 24);
        input[..4].copy_from_slice(&magic.to_le_bytes());
        input[64..68].copy_from_slice(&magic.to_le_bytes());
        let output = r.update(&input, &Memory::default()).unwrap();
        if revision >= 5 {
            assert_eq!(output.len(), 384);
            assert_eq!(&output[368..376], &123u64.to_le_bytes());
        } else {
            assert_eq!(output.len(), 368);
        }
        assert_eq!(
            u32::from_le_bytes(output[40..44].try_into().unwrap()),
            if revision >= 5 { 16 } else { 0 }
        );
    }
}
#[test]
fn active_playback_and_malformed_updates_are_explicit_errors() {
    let params = audio::Parameters::parse(&parameters(8)).unwrap();
    let r = audio::Renderer::new(params, 0);
    let mut input = silent_input();
    let memory = Memory::default();
    input[208 + 9] = 1;
    assert!(
        r.update(&input, &memory)
            .unwrap_err()
            .contains("active voice")
    );
    input = silent_input();
    input[60..64].copy_from_slice(&u32::MAX.to_le_bytes());
    assert!(r.update(&input, &memory).is_err());
    input = silent_input();
    input[36..40].copy_from_slice(&4u32.to_le_bytes());
    assert!(r.update(&input, &memory).is_err());
    for len in 0..64 {
        assert!(r.update(&input[..len], &memory).is_err());
    }
}
#[test]
fn parameters_reject_unknown_revision_execution_mode_and_overflow_counts() {
    for at in [0, 4, 8, 12, 16, 20, 24, 48] {
        let mut p = parameters(8);
        p[at..at + 4].copy_from_slice(&u32::MAX.to_le_bytes());
        assert!(audio::Parameters::parse(&p).is_err(), "offset{at}");
    }
    let mut p = parameters(8);
    p[35] = 1;
    assert!(audio::Parameters::parse(&p).is_err());
    for len in 0..52 {
        assert!(audio::Parameters::parse(&p[..len]).is_err());
    }
}

fn update_wire(p: &mut Process, performance: u64) {
    let input = DATA + 0x4000;
    let output = DATA + 0x6000;
    p.write_bytes(input, &silent_input()).unwrap();
    let mut b = [0u8; 256];
    // One A input, two B output slots and two C output slots. Status uses
    // C[0], performance uses B[1]; filtering zero B slots would swap them.
    b[..4].copy_from_slice(&(4u32 | 1 << 20 | 2 << 24).to_le_bytes());
    b[4..8].copy_from_slice(&(8u32 | 4 << 10).to_le_bytes());
    for (at, address, size) in [
        (8, input, silent_input().len() as u32),
        (20, 0, 0),
        (32, performance, 32),
    ] {
        b[at..at + 4].copy_from_slice(&size.to_le_bytes());
        b[at + 4..at + 8].copy_from_slice(&(address as u32).to_le_bytes());
    }
    b[48..52].copy_from_slice(b"SFCI");
    b[56..60].copy_from_slice(&10u32.to_le_bytes());
    b[76..80].copy_from_slice(&(output as u32).to_le_bytes());
    b[80..84].copy_from_slice(&(512u32 << 16).to_le_bytes());
    let address = tls(p);
    p.write_bytes(address, &b).unwrap();
}

#[test]
fn ipc_autoselect_preserves_output_slots_and_padding_bounds() {
    let (mut p, mut c) = process();
    let m = manager(&mut p, &mut c);
    let r = renderer(&mut p, &mut c, m);
    let output = DATA + 0x6000;
    let perf = DATA + 0x8000;
    p.write_bytes(output, &[0x55; 528]).unwrap();
    p.write_bytes(perf, &[0x66; 48]).unwrap();
    update_wire(&mut p, perf);
    call(&mut p, &mut c, r);
    assert_eq!(p.read_bytes(output, 4).unwrap(), b"REV8");
    assert_eq!(p.load(output + 60, 4).unwrap(), 384);
    for i in 0..4 {
        assert_eq!(p.load(output + 64 + i * 16, 4).unwrap(), 1);
    }
    assert_eq!(p.read_bytes(output + 512, 16).unwrap(), &[0x55; 16]);
    assert_eq!(p.read_bytes(perf, 32).unwrap(), &[0; 32]);
    assert_eq!(p.read_bytes(perf + 32, 16).unwrap(), &[0x66; 16]);
}

#[test]
fn invalid_performance_destination_does_not_partially_publish_status() {
    let (mut p, mut c) = process();
    let m = manager(&mut p, &mut c);
    let r = renderer(&mut p, &mut c, m);
    let output = DATA + 0x6000;
    p.write_bytes(output, &[0x55; 512]).unwrap();
    update_wire(&mut p, 0x7fffffffff);
    c.x[0] = u64::from(r);
    assert!(dispatch(&mut c, &mut p, 0x21).is_err());
    assert_eq!(p.read_bytes(output, 512).unwrap(), &[0x55; 512]);
}

#[test]
fn wrong_work_memory_object_and_unknown_commands_do_not_open_renderers() {
    let (mut p, mut c) = process();
    let m = manager(&mut p, &mut c);
    let mut args = parameters(8);
    args.extend([0; 4]);
    args.extend(0x10000u64.to_le_bytes());
    args.extend(0u64.to_le_bytes());
    wire(&mut p, 0, true, &[CURRENT_PROCESS, CURRENT_PROCESS], &args);
    c.x[0] = u64::from(m);
    assert!(
        dispatch(&mut c, &mut p, 0x21)
            .unwrap_err()
            .contains("transfer memory")
    );
    assert!(p.kernel.as_ref().unwrap().events.is_empty());
    wire(&mut p, 999, false, &[], &[]);
    c.x[0] = u64::from(m);
    assert!(
        dispatch(&mut c, &mut p, 0x21)
            .unwrap_err()
            .contains("unknown command")
    );
}

#[test]
fn cloned_renderer_sessions_keep_the_clock_until_the_last_close() {
    let (mut p, mut c) = process();
    let m = manager(&mut p, &mut c);
    let r = renderer(&mut p, &mut c, m);
    wire(&mut p, 5, false, &[], &[]);
    call(&mut p, &mut c, r);
    wire(&mut p, 2, false, &[], &[]);
    let address = tls(&p);
    p.store(address, 4, 5).unwrap();
    call(&mut p, &mut c, r);
    let clone = reply(&p, 12);
    svc(&mut p, &mut c, 0x16, &[u64::from(r)]);
    assert_eq!(c.x[0], 0);
    assert!(wl_runtime::audio::next_deadline_tick(p.kernel.as_ref().unwrap()).is_some());
    wire(&mut p, 3, false, &[], &[]);
    call(&mut p, &mut c, clone);
    assert_eq!(reply(&p, 0x20), 0);
    svc(&mut p, &mut c, 0x16, &[u64::from(clone)]);
    assert_eq!(c.x[0], 0);
    assert_eq!(
        wl_runtime::audio::next_deadline_tick(p.kernel.as_ref().unwrap()),
        None
    );
    // More than four renderer lifetimes work; no old instance can be revived
    // when a fresh session receives a reused kernel handle-table slot.
    for _ in 0..8 {
        let r = renderer(&mut p, &mut c, m);
        svc(&mut p, &mut c, 0x16, &[u64::from(r)]);
        assert_eq!(c.x[0], 0);
    }
}

#[test]
fn break_diagnostics_decode_horizon_result_module_and_description() {
    let (mut p, mut c) = process();
    p.store(DATA, 4, 0x1a03).unwrap();
    c.x[..3].copy_from_slice(&[0, DATA, 4]);
    let error = dispatch(&mut c, &mut p, 0x26).unwrap_err();
    assert!(error.contains("desc=13, mod=3"), "{error}");
}
