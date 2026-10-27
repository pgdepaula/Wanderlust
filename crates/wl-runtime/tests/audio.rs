extern crate wl_runtime as wl_guest_runtime;
#[path = "../hosted/audio.rs"]
mod audio;
#[path = "../hosted/fs_cache.rs"]
mod host_fs;
#[path = "../hosted/opus.rs"]
mod host_opus;
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
            service_access: &[(false, "audren:u"), (false, "hwopus"), (false, "fsp-srv")],
        },
    )
    .unwrap()
}
fn svc(p: &mut Process, c: &mut Cpu, id: u16, args: &[u64]) -> Option<Stop> {
    c.x[..args.len()].copy_from_slice(args);
    audio::service_hook(c, p, id).unwrap()
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
    named_manager(p, c, b"audren:u")
}
fn named_manager(p: &mut Process, c: &mut Cpu, name: &[u8; 8]) -> u32 {
    p.write_bytes(DATA, b"sm:\0").unwrap();
    svc(p, c, 0x1f, &[0, DATA]);
    assert_eq!(c.x[0], 0);
    let sm = c.x[1] as u32;
    wire(p, 0, true, &[], &[0; 8]);
    call(p, c, sm);
    wire(p, 1, false, &[], name);
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
    open_renderer(p, c, manager, parameters(8))
}
fn open_renderer(p: &mut Process, c: &mut Cpu, manager: u32, mut args: Vec<u8>) -> u32 {
    svc(p, c, 0x15, &[0, DATA + 0x10000, 0x10000, 0]);
    assert_eq!(c.x[0], 0);
    let transfer = c.x[1] as u32;
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

fn splitter_parameters() -> Vec<u8> {
    let mut p = parameters(11);
    p[36..40].copy_from_slice(&2u32.to_le_bytes());
    p[40..44].copy_from_slice(&3u32.to_le_bytes());
    p
}
fn splitter_section() -> Vec<u8> {
    // Independent wire fixture: two variable SNDI records followed by three
    // 112-byte SNDD records, with deliberately nonzero unused/padding bytes.
    let mut b = vec![0xa5; 32];
    b[..4].copy_from_slice(b"SNDH");
    b[4..8].copy_from_slice(&2u32.to_le_bytes());
    b[8..12].copy_from_slice(&3u32.to_le_bytes());
    for (id, destinations) in [(0u32, &[2u32, 0][..]), (1, &[1][..])] {
        b.extend(b"SNDI");
        b.extend(id.to_le_bytes());
        b.extend(48000u32.to_le_bytes());
        b.extend((destinations.len() as u32).to_le_bytes());
        for id in destinations {
            b.extend(id.to_le_bytes());
        }
        b.extend([0xa5; 12]);
    }
    assert_eq!(b.len(), 100);
    for id in [2u32, 1, 0] {
        let mut d = [0u8; 112];
        d[..4].copy_from_slice(b"SNDD");
        d[4..8].copy_from_slice(&id.to_le_bytes());
        d[8..12].copy_from_slice(&1f32.to_bits().to_le_bytes());
        d[12..16].copy_from_slice(&0.125f32.to_bits().to_le_bytes());
        d[108] = 1;
        d[109..].fill(0xa5); // Explicit reset is not a REV11 feature.
        b.extend(d);
    }
    assert_eq!(b.len(), 436);
    b.extend([0xbc; 12]);
    b
}
fn input_with_splitters(section: &[u8]) -> Vec<u8> {
    let mut input = silent_input();
    input[..4].copy_from_slice(b"REV;"); // REV11 is REV0 + (11 << 24).
    input[64..68].copy_from_slice(b"REV;");
    input[36..40].copy_from_slice(&(section.len() as u32).to_le_bytes());
    input.splice(576..576, section.iter().copied());
    let total = input.len() as u32;
    input[60..64].copy_from_slice(&total.to_le_bytes());
    input
}

#[test]
fn silent_rev11_splitter_updates_accept_variable_ids_and_opaque_padding() {
    let r = audio::Renderer::new(audio::Parameters::parse(&splitter_parameters()).unwrap(), 0);
    let input = input_with_splitters(&splitter_section());
    let result = r.update(&input, &Memory::default()).unwrap();
    assert_eq!(result.len(), 384);
    assert_eq!(&result[..4], b"REV;");
    assert!(result[128..].iter().all(|b| *b == 0));
    let mut voice = input;
    voice[208 + 9] = 1;
    assert!(
        r.update(&voice, &Memory::default())
            .unwrap_err()
            .contains("active voice")
    );
}

#[test]
fn malformed_splitter_ranges_and_ids_are_rejected_before_output() {
    let r = audio::Renderer::new(audio::Parameters::parse(&splitter_parameters()).unwrap(), 0);
    let memory = Memory::default();
    for (at, bits) in [
        (0, 0u32),
        (4, 3),
        (8, 4),
        (36, 2),
        (44, u32::MAX),
        (48, 3),
        (52, 2),
        (100, 0),
        (104, 3),
        (204, 1),
        (108, f32::NAN.to_bits()),
    ] {
        let mut section = splitter_section();
        section[at..at + 4].copy_from_slice(&bits.to_le_bytes());
        assert!(
            r.update(&input_with_splitters(&section), &memory).is_err(),
            "offset {at}"
        );
    }
    let mut section = splitter_section();
    section[208] = 2;
    assert!(r.update(&input_with_splitters(&section), &memory).is_err());
    section = splitter_section();
    section.truncate(432);
    assert!(r.update(&input_with_splitters(&section), &memory).is_err());
    section = splitter_section();
    section.extend([0; 16]);
    assert!(r.update(&input_with_splitters(&section), &memory).is_err());
    assert!(r.update(&input_with_splitters(&[]), &memory).is_err());
    for (offset, bits) in [
        (36, 1025u32),
        (40, 8193),
        (36, 0),
        (40, 0),
        (48, 0x34564552),
        (48, 0x3c564552),
    ] {
        let mut p = splitter_parameters();
        p[offset..offset + 4].copy_from_slice(&bits.to_le_bytes());
        assert!(audio::Parameters::parse(&p).is_err());
    }
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
    for revision in [1, 4, 5, 8, 11] {
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
fn disabled_effect_results_use_wire_state_four_in_both_revision_layouts() {
    for (revision, result_bytes) in [(8, 16), (11, 144)] {
        let mut params = parameters(revision);
        params[24..28].copy_from_slice(&1u32.to_le_bytes());
        let r = audio::Renderer::new(audio::Parameters::parse(&params).unwrap(), 0);
        // Literal ABI sizes: behavior, five pools, one voice, one effect,
        // final mix, sink, performance; channel resources are empty.
        let sizes = [16u32, 160, 368, 0, 192, 2352, 320, 16, 0, 0];
        let mut input = vec![0; 64 + sizes.iter().sum::<u32>() as usize];
        let magic = 0x00564552 | (u32::from(b'0' + revision) << 24);
        input[..4].copy_from_slice(&magic.to_le_bytes());
        for (i, size) in sizes.iter().enumerate() {
            input[4 + 4 * i..8 + 4 * i].copy_from_slice(&size.to_le_bytes());
        }
        let total = input.len() as u32;
        input[60..64].copy_from_slice(&total.to_le_bytes());
        input[64..68].copy_from_slice(&magic.to_le_bytes());
        let output = r.update(&input, &Memory::default()).unwrap();
        assert_eq!(
            u32::from_le_bytes(output[20..24].try_into().unwrap()),
            result_bytes as u32
        );
        let effect_out = 64 + 80 + 16;
        // EffectState::Disabled = 4 on the wire. UsageState::Disabled = 3
        // belongs to the server's separate internal state machine.
        assert_eq!(output[effect_out], 4);
        assert!(
            output[effect_out + 1..effect_out + result_bytes]
                .iter()
                .all(|v| *v == 0)
        );
        let effect_in = 64 + 16 + 160 + 368;
        input[effect_in + 2] = 1;
        assert!(
            r.update(&input, &Memory::default())
                .unwrap_err()
                .contains("DSP effect")
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
    update_wire_input(p, performance, &silent_input());
}
fn update_wire_input(p: &mut Process, performance: u64, input_data: &[u8]) {
    let input = DATA + 0x4000;
    let output = DATA + 0x6000;
    p.write_bytes(input, input_data).unwrap();
    let mut b = [0u8; 256];
    // One A input, two B output slots and two C output slots. Status uses
    // C[0], performance uses B[1]; filtering zero B slots would swap them.
    b[..4].copy_from_slice(&(4u32 | 1 << 20 | 2 << 24).to_le_bytes());
    b[4..8].copy_from_slice(&(8u32 | 4 << 10).to_le_bytes());
    for (at, address, size) in [
        (8, input, input_data.len() as u32),
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
fn splitter_cmif_update_validates_every_record_before_guest_publication() {
    let (mut p, mut c) = process();
    let m = manager(&mut p, &mut c);
    let r = open_renderer(&mut p, &mut c, m, splitter_parameters());
    let output = DATA + 0x6000;
    let perf = DATA + 0x8000;
    let input = input_with_splitters(&splitter_section());
    update_wire_input(&mut p, perf, &input);
    call(&mut p, &mut c, r);
    assert_eq!(p.read_bytes(output, 4).unwrap(), b"REV;");
    assert_eq!(p.load(output + 60, 4).unwrap(), 384);
    p.write_bytes(output, &[0x55; 512]).unwrap();
    p.write_bytes(perf, &[0x66; 32]).unwrap();
    let mut bad = splitter_section();
    bad[104..108].copy_from_slice(&3u32.to_le_bytes());
    update_wire_input(&mut p, perf, &input_with_splitters(&bad));
    c.x[0] = u64::from(r);
    assert!(dispatch(&mut c, &mut p, 0x21).unwrap_err().contains("SNDD"));
    assert_eq!(p.read_bytes(output, 512).unwrap(), &[0x55; 512]);
    assert_eq!(p.read_bytes(perf, 32).unwrap(), &[0x66; 32]);
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

#[test]
fn native_codec_binding_restores_the_port_after_rejected_requests() {
    let (mut p, mut c) = process();
    let h = named_manager(&mut p, &mut c, b"hwopus\0\0");
    let k = p.kernel.as_ref().unwrap();
    let Object::Session(session) = k.handles.get(h).unwrap() else {
        panic!("session")
    };
    let original = k.sessions[session].endpoint;
    let wl_runtime::ipc::Endpoint::Port(port) = original else {
        panic!("native port")
    };
    assert_eq!(k.ports[port].sessions, 1);
    wire(&mut p, 999, false, &[], &[]);
    c.x[0] = u64::from(h);
    assert!(
        audio::service_hook(&mut c, &mut p, 0x21)
            .unwrap_err()
            .contains("manager command")
    );
    assert_eq!(
        p.kernel.as_ref().unwrap().sessions[session].endpoint,
        original
    );
    wire(&mut p, 3, false, &[], &[]);
    let address = tls(&p);
    p.store(address, 4, 5).unwrap(); // CMIF control: QueryPointerBufferSize.
    call(&mut p, &mut c, h);
    assert_eq!(reply(&p, 0x20), 4096);
    svc(&mut p, &mut c, 0x16, &[u64::from(h)]);
    assert_eq!(p.kernel.as_ref().unwrap().ports[port].sessions, 0);
}

#[test]
fn native_codec_registration_does_not_bypass_service_access_permissions() {
    let (mut p, mut c) = process();
    p.kernel.as_mut().unwrap().config.service_access = &[(false, "audren:u")];
    p.write_bytes(DATA, b"sm:\0").unwrap();
    svc(&mut p, &mut c, 0x1f, &[0, DATA]);
    let sm = c.x[1] as u32;
    wire(&mut p, 0, true, &[], &[0; 8]);
    call(&mut p, &mut c, sm);
    wire(&mut p, 1, false, &[], b"hwopus\0\0");
    svc(&mut p, &mut c, 0x21, &[u64::from(sm)]);
    assert_eq!(reply(&p, 0x18), wl_runtime::ipc::sm_result::NOT_ALLOWED);
    assert_eq!(p.kernel.as_ref().unwrap().sessions.len(), 1);
}

fn opus_decode_wire(p: &mut Process, channels: u32, range: u32, command: u32) -> (u64, usize) {
    let input = DATA + 0x4000;
    let output = DATA + 0x6000;
    let mut packet = vec![0, 0, 0, 3];
    packet.extend(range.to_be_bytes());
    packet.extend([0xf8, 0xff, 0xfe]);
    p.write_bytes(input, &packet).unwrap();
    let bytes = 960 * channels as usize * 2;
    p.write_bytes(output, &vec![0x55; bytes + 16]).unwrap();
    let mut b = [0u8; 256];
    b[..4].copy_from_slice(&(4u32 | 1 << 20 | 1 << 24).to_le_bytes());
    b[4..8].copy_from_slice(&8u32.to_le_bytes());
    for (at, address, size) in [(8, input, 11u32), (20, output, (bytes + 16) as u32)] {
        b[at..at + 4].copy_from_slice(&size.to_le_bytes());
        b[at + 4..at + 8].copy_from_slice(&(address as u32).to_le_bytes());
    }
    b[32..36].copy_from_slice(b"SFCI");
    b[40..44].copy_from_slice(&command.to_le_bytes());
    b[48] = u8::from(matches!(command, 6 | 8));
    let address = tls(p);
    p.write_bytes(address, &b).unwrap();
    (output, bytes)
}

#[test]
#[ignore = "requires system libopus; executes actual C decoder through CMIF"]
fn hwopus_decodes_silence_validates_big_endian_headers_and_rolls_back_failed_packets() {
    for channels in [1u32, 2] {
        let (mut p, mut c) = process();
        let m = named_manager(&mut p, &mut c, b"hwopus\0\0");
        let args = [48000u32.to_le_bytes(), channels.to_le_bytes()].concat();
        wire(&mut p, 1, false, &[], &args);
        call(&mut p, &mut c, m);
        let required = reply(&p, 0x20);
        // Decoder sizes vary across libopus builds (including optional neural state).
        assert!(required > 4096 && required <= 0x30000);
        let size = required.next_multiple_of(4096);
        svc(
            &mut p,
            &mut c,
            0x15,
            &[0, DATA + 0x10000, u64::from(size), 0],
        );
        assert_eq!(c.x[0], 0);
        let transfer = c.x[1] as u32;
        let mut open = args;
        open.extend(size.to_le_bytes());
        wire(&mut p, 0, false, &[transfer], &open);
        call(&mut p, &mut c, m);
        let d = reply(&p, 12);
        svc(&mut p, &mut c, 0x16, &[u64::from(m)]);
        assert_eq!(c.x[0], 0);
        for command in [0u32, 4, 6, 8] {
            let (output, bytes) = opus_decode_wire(&mut p, channels, 0, command);
            call(&mut p, &mut c, d);
            assert_eq!(reply(&p, 0x20), 11);
            assert_eq!(reply(&p, 0x24), 960);
            assert!(p.read_bytes(output, bytes).unwrap().iter().all(|v| *v == 0));
            assert_eq!(
                p.read_bytes(output + bytes as u64, 16).unwrap(),
                &[0x55; 16]
            );
        }
        let (output, bytes) = opus_decode_wire(&mut p, channels, 1, 6);
        c.x[0] = u64::from(d);
        assert!(
            audio::service_hook(&mut c, &mut p, 0x21)
                .unwrap_err()
                .contains("final range")
        );
        assert_eq!(
            p.read_bytes(output, bytes + 16).unwrap(),
            &vec![0x55; bytes + 16]
        );
        opus_decode_wire(&mut p, channels, 0, 0);
        call(&mut p, &mut c, d);
        assert_eq!(reply(&p, 0x24), 960);
        svc(&mut p, &mut c, 0x16, &[u64::from(d)]);
        svc(&mut p, &mut c, 0x16, &[u64::from(transfer)]);
    }
}

#[test]
#[ignore = "requires system libopus; validates extended CMIF opening and real decode"]
fn hwopus_extended_manager_validates_flags_and_inline_work_size() {
    for flags in [0u32, 1] {
        let (mut p, mut c) = process();
        let m = named_manager(&mut p, &mut c, b"hwopus\0\0");
        let mut args = [48000u32, 2, flags, 0]
            .into_iter()
            .flat_map(u32::to_le_bytes)
            .collect::<Vec<_>>();
        wire(&mut p, 5, false, &[], &args);
        call(&mut p, &mut c, m);
        let size = reply(&p, 0x20).next_multiple_of(4096);
        svc(
            &mut p,
            &mut c,
            0x15,
            &[0, DATA + 0x10000, u64::from(size), 0],
        );
        assert_eq!(c.x[0], 0);
        let transfer = c.x[1] as u32;
        args.extend(size.to_le_bytes());
        wire(&mut p, 4, false, &[transfer], &args);
        call(&mut p, &mut c, m);
        let decoder = reply(&p, 12);
        let (output, bytes) = opus_decode_wire(&mut p, 2, 0, 8);
        call(&mut p, &mut c, decoder);
        assert_eq!(reply(&p, 0x20), 11);
        assert_eq!(reply(&p, 0x24), 960);
        assert!(p.read_bytes(output, bytes).unwrap().iter().all(|v| *v == 0));
        // RFC 6716 code-3 CBR packet: three 20ms DTX frames, independently
        // specified instead of obtaining expected PCM from the service itself.
        opus_decode_wire(&mut p, 2, 0, 8);
        let mut packet = 8u32.to_be_bytes().to_vec();
        packet.extend(0u32.to_be_bytes());
        packet.extend([0xfb, 3, 0xff, 0xfe, 0xff, 0xfe, 0xff, 0xfe]);
        p.write_bytes(DATA + 0x4000, &packet).unwrap();
        let address = tls(&p);
        p.store(address + 8, 4, packet.len() as u64).unwrap();
        let output_bytes = 2880 * 2 * 2;
        p.store(address + 20, 4, (output_bytes + 16) as u64)
            .unwrap();
        p.write_bytes(DATA + 0x6000, &vec![0x55; output_bytes + 16])
            .unwrap();
        if flags == 1 {
            call(&mut p, &mut c, decoder);
            assert_eq!(reply(&p, 0x20), 16);
            assert_eq!(reply(&p, 0x24), 2880);
            assert!(
                p.read_bytes(DATA + 0x6000, output_bytes)
                    .unwrap()
                    .iter()
                    .all(|v| *v == 0)
            );
        } else {
            c.x[0] = u64::from(decoder);
            assert!(
                audio::service_hook(&mut c, &mut p, 0x21)
                    .unwrap_err()
                    .contains("decode failed")
            );
            assert_eq!(
                p.read_bytes(DATA + 0x6000, output_bytes).unwrap(),
                &vec![0x55; output_bytes]
            );
        }
        assert_eq!(
            p.read_bytes(DATA + 0x6000 + output_bytes as u64, 16)
                .unwrap(),
            &[0x55; 16]
        );
        svc(&mut p, &mut c, 0x16, &[u64::from(decoder)]);
        for (bits, reserved) in [(2u32, 0u32), (0, 1)] {
            let invalid = [48000u32, 2, bits, reserved]
                .into_iter()
                .flat_map(u32::to_le_bytes)
                .collect::<Vec<_>>();
            wire(&mut p, 5, false, &[], &invalid);
            c.x[0] = u64::from(m);
            assert!(
                audio::service_hook(&mut c, &mut p, 0x21)
                    .unwrap_err()
                    .contains("extended flags")
            );
        }
        svc(&mut p, &mut c, 0x16, &[u64::from(m)]);
        svc(&mut p, &mut c, 0x16, &[u64::from(transfer)]);
    }
}

fn device_volume_wire(p: &mut Process, command: u32, bits: u32) {
    let name_address = DATA + 0x8000;
    let mut name = [0u8; 256];
    name[..13].copy_from_slice(b"AudioTvOutput");
    p.write_bytes(name_address, &name).unwrap();
    let mut wire = [0u8; 256];
    wire[..4].copy_from_slice(&(4u32 | 1 << 20).to_le_bytes());
    wire[4..8].copy_from_slice(&8u32.to_le_bytes());
    wire[8..12].copy_from_slice(&256u32.to_le_bytes());
    wire[12..16].copy_from_slice(&(name_address as u32).to_le_bytes());
    wire[32..36].copy_from_slice(b"SFCI");
    wire[40..44].copy_from_slice(&command.to_le_bytes());
    wire[48..52].copy_from_slice(&bits.to_le_bytes());
    let address = tls(p);
    p.write_bytes(address, &wire).unwrap();
}
#[test]
fn audio_device_preserves_finite_gain_bits_without_an_invented_unit_interval() {
    let (mut p, mut c) = process();
    let m = manager(&mut p, &mut c);
    wire(&mut p, 2, false, &[], &0u64.to_le_bytes());
    call(&mut p, &mut c, m);
    let device = reply(&p, 12);
    for volume in [2.5f32, -0.5, -0.0, 1.0001, f32::MAX] {
        device_volume_wire(&mut p, 7, volume.to_bits());
        call(&mut p, &mut c, device);
        device_volume_wire(&mut p, 8, 0);
        call(&mut p, &mut c, device);
        assert_eq!(reply(&p, 0x20), volume.to_bits());
    }
    for bits in [
        f32::NAN.to_bits(),
        f32::INFINITY.to_bits(),
        f32::NEG_INFINITY.to_bits(),
    ] {
        device_volume_wire(&mut p, 7, bits);
        c.x[0] = u64::from(device);
        assert!(
            audio::service_hook(&mut c, &mut p, 0x21)
                .unwrap_err()
                .contains("nonfinite audio device volume")
        );
        device_volume_wire(&mut p, 8, 0);
        call(&mut p, &mut c, device);
        assert_eq!(reply(&p, 0x20), f32::MAX.to_bits());
    }
}

#[test]
fn unknown_audio_device_gain_does_not_mutate_the_advertised_output() {
    let (mut p, mut c) = process();
    let m = manager(&mut p, &mut c);
    wire(&mut p, 2, false, &[], &0u64.to_le_bytes());
    call(&mut p, &mut c, m);
    let device = reply(&p, 12);
    device_volume_wire(&mut p, 7, 0.75f32.to_bits());
    call(&mut p, &mut c, device);
    for command in [7u32, 8] {
        device_volume_wire(&mut p, command, 2.5f32.to_bits());
        let mut name = [0u8; 256];
        name[..21].copy_from_slice(b"AudioStereoJackOutput");
        p.write_bytes(DATA + 0x8000, &name).unwrap();
        call(&mut p, &mut c, device);
        if command == 8 {
            assert_eq!(reply(&p, 0x20), 0.0f32.to_bits());
        }
    }
    device_volume_wire(&mut p, 8, 0);
    call(&mut p, &mut c, device);
    assert_eq!(reply(&p, 0x20), 0.75f32.to_bits());
    device_volume_wire(&mut p, 7, 1.0f32.to_bits());
    p.write_bytes(DATA + 0x8000, &[b'x'; 256]).unwrap();
    c.x[0] = u64::from(device);
    assert!(
        audio::service_hook(&mut c, &mut p, 0x21)
            .unwrap_err()
            .contains("unterminated")
    );
    device_volume_wire(&mut p, 8, 0);
    call(&mut p, &mut c, device);
    assert_eq!(reply(&p, 0x20), 0.75f32.to_bits());
}

fn fs_reader_wire(p: &mut Process, domain: Option<u32>, command: u32, output: bool) {
    let mut b = [0u8; 256];
    let descriptors = if output { 1u32 << 24 } else { 0 };
    b[..4].copy_from_slice(&(4 | descriptors).to_le_bytes());
    let at = if output {
        b[8..12].copy_from_slice(&192u32.to_le_bytes());
        b[12..16].copy_from_slice(&((DATA + 0x9000) as u32).to_le_bytes());
        20usize
    } else {
        8
    };
    let raw = (at + 15) & !15;
    let cmif = if let Some(id) = domain {
        b[raw] = 1;
        b[raw + 2..raw + 4].copy_from_slice(&16u16.to_le_bytes());
        b[raw + 4..raw + 8].copy_from_slice(&id.to_le_bytes());
        raw + 16
    } else {
        raw
    };
    b[4..8].copy_from_slice(&(((cmif + 16 - at) / 4) as u32).to_le_bytes());
    b[cmif..cmif + 4].copy_from_slice(b"SFCI");
    b[cmif + 8..cmif + 12].copy_from_slice(&command.to_le_bytes());
    let address = tls(p);
    p.write_bytes(address, &b).unwrap();
}
#[test]
fn cache_save_reader_returns_a_real_session_and_empty_count_without_overwriting_output() {
    let (mut p, mut c) = process();
    let fs = named_manager(&mut p, &mut c, b"fsp-srv\0");
    fs_reader_wire(&mut p, None, 62, false);
    call(&mut p, &mut c, fs);
    let reader = reply(&p, 12);
    let Object::Session(s) = p.kernel.as_ref().unwrap().handles.get(fs).unwrap() else {
        panic!()
    };
    assert_eq!(
        p.kernel.as_ref().unwrap().sessions[s].endpoint,
        wl_runtime::ipc::Endpoint::Host(wl_runtime::services::HostObject::FileSystemProxy)
    );
    p.write_bytes(DATA + 0x9000, &[0x55; 192]).unwrap();
    for _ in 0..2 {
        fs_reader_wire(&mut p, None, 0, true);
        call(&mut p, &mut c, reader);
        assert_eq!(p.load(tls(&p) + 0x20, 8).unwrap(), 0);
        assert_eq!(p.read_bytes(DATA + 0x9000, 192).unwrap(), &[0x55; 192]);
    }
    svc(&mut p, &mut c, 0x16, &[u64::from(reader)]);
}
#[test]
fn cache_reader_domain_objects_use_fs_ids_without_colliding_with_audio_ids() {
    let (mut p, mut c) = process();
    let audio = manager(&mut p, &mut c);
    wire(&mut p, 0, false, &[], &[]);
    let address = tls(&p);
    p.store(address, 4, 5).unwrap();
    call(&mut p, &mut c, audio);
    let audio_root = reply(&p, 0x20);
    let fs = named_manager(&mut p, &mut c, b"fsp-srv\0");
    wire(&mut p, 0, false, &[], &[]);
    p.store(address, 4, 5).unwrap();
    call(&mut p, &mut c, fs);
    let root = reply(&p, 0x20);
    assert_eq!((audio_root, root), (1, 1));
    // Invalid opening must restore both the FS target and its ID allocator.
    fs_reader_wire(&mut p, Some(root), 62, true);
    c.x[0] = u64::from(fs);
    assert!(audio::service_hook(&mut c, &mut p, 0x21).is_err());
    fs_reader_wire(&mut p, Some(root), 62, false);
    svc(&mut p, &mut c, 0x21, &[u64::from(fs)]);
    assert_eq!(reply(&p, 0x28), 0);
    let reader = reply(&p, 0x30);
    assert_ne!(reader, root);
    p.write_bytes(DATA + 0x9000, &[0x55; 192]).unwrap();
    fs_reader_wire(&mut p, Some(reader), 0, true);
    svc(&mut p, &mut c, 0x21, &[u64::from(fs)]);
    assert_eq!(reply(&p, 0x28), 0);
    assert_eq!(p.load(address + 0x30, 8).unwrap(), 0);
    assert_eq!(p.read_bytes(DATA + 0x9000, 192).unwrap(), &[0x55; 192]);
    let k = p.kernel.as_ref().unwrap();
    let Object::Session(s) = k.handles.get(fs).unwrap() else {
        panic!()
    };
    let d = k.sessions[s].domain.unwrap();
    let objects = &k.host.domains[d].objects;
    assert_eq!(objects.len(), 2);
    assert_ne!(objects[0].0, objects[1].0);
    assert_eq!(
        objects.iter().find(|(id, _)| *id == root).unwrap().1,
        wl_runtime::services::HostObject::FileSystemProxy
    );
    fs_reader_wire(&mut p, Some(reader), 0, false);
    p.store(address + 16, 1, 2).unwrap(); // Domain CloseObject.
    p.store(address + 18, 2, 0).unwrap();
    svc(&mut p, &mut c, 0x21, &[u64::from(fs)]);
    fs_reader_wire(&mut p, Some(root), 62, false);
    svc(&mut p, &mut c, 0x21, &[u64::from(fs)]);
    assert_eq!(reply(&p, 0x30), reader); // The FS server recycles its own ID.
    fs_reader_wire(&mut p, Some(audio_root), 2, false);
    p.store(address + 4, 4, 12).unwrap();
    p.store(address + 18, 2, 24).unwrap();
    svc(&mut p, &mut c, 0x21, &[u64::from(audio)]);
    assert_eq!(reply(&p, 0x28), 0);
    assert_eq!(reply(&p, 0x30), 2); // Audio's independent allocator was not changed.
}

#[test]
fn unconfigured_save_mount_returns_target_not_found_without_a_null_success_object() {
    for domain in [false, true] {
        let (mut p, mut c) = process();
        let fs = named_manager(&mut p, &mut c, b"fsp-srv\0");
        let root = if domain {
            wire(&mut p, 0, false, &[], &[]);
            let address = tls(&p);
            p.store(address, 4, 5).unwrap();
            call(&mut p, &mut c, fs);
            Some(reply(&p, 0x20))
        } else {
            None
        };
        for command in [51, 53] {
            fs_reader_wire(&mut p, root, command, false);
            let address = tls(&p);
            let cmif = if domain { 32 } else { 16 };
            p.store(address + 4, 4, (cmif + 16 + 72 - 8) / 4).unwrap();
            if domain {
                p.store(address + 18, 2, 88).unwrap();
            }
            let mut args = [0; 72];
            args[0] = 1;
            args[40] = 1;
            args[16] = 1;
            p.write_bytes(address + cmif + 16, &args).unwrap();
            svc(&mut p, &mut c, 0x21, &[u64::from(fs)]);
            assert_eq!(reply(&p, if domain { 0x28 } else { 0x18 }), 0x7d402);
            assert_eq!(reply(&p, 4) >> 31, 0);
            // A truncated mount cannot fall through to a success-only stub.
            fs_reader_wire(&mut p, root, command, false);
            c.x[0] = u64::from(fs);
            assert!(audio::service_hook(&mut c, &mut p, 0x21).is_err());
        }
    }
}

#[test]
fn ram_cache_quota_faults_permissions_and_lifetimes_preserve_file_contents() {
    use wl_runtime::{ipc::Request, services::HostObject};
    let (mut p, _) = process();
    let kernel = p.kernel.as_ref().unwrap();
    let mut cache = host_fs::Service::default();
    let plain = Request::default();
    let mut create = [0u8; 24];
    create[..2].copy_from_slice(&7u16.to_le_bytes());
    create[8..16].copy_from_slice(&16u64.to_le_bytes());
    for bad in [&create[..23], &[0xff; 24][..]] {
        assert!(
            cache
                .request(&mut p.memory, kernel, host_fs::MANAGER + 3, 27, &plain, bad)
                .is_err()
        );
    }
    assert_eq!(
        cache
            .request(
                &mut p.memory,
                kernel,
                host_fs::MANAGER + 3,
                27,
                &plain,
                &create
            )
            .unwrap()
            .data,
        [1, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0]
    );
    assert_eq!(
        cache
            .request(
                &mut p.memory,
                kernel,
                host_fs::MANAGER + 3,
                27,
                &plain,
                &create
            )
            .unwrap()
            .code,
        0x402
    );
    let object = |reply: wl_runtime::audio::Reply| match reply.objects[0] {
        HostObject::AudioRenderer { index } => index,
        _ => panic!(),
    };
    let mut attr = [0u8; 72];
    attr[0] = 1;
    attr[40] = 5;
    attr[42] = 7;
    let mount = object(
        cache
            .request(&mut p.memory, kernel, host_fs::MANAGER, 51, &plain, &attr)
            .unwrap(),
    );
    attr[8..16].copy_from_slice(&99u64.to_le_bytes());
    assert!(
        cache
            .request(&mut p.memory, kernel, host_fs::MANAGER, 51, &plain, &attr)
            .is_err()
    );
    attr[8..16].fill(0);
    let mut other = host_fs::Service::default();
    assert_eq!(
        other
            .request(&mut p.memory, kernel, host_fs::MANAGER, 51, &plain, &attr)
            .unwrap()
            .code,
        (1002 << 9) | 2
    );
    p.memory.write_bytes(DATA + 0x9000, b"/a\0").unwrap();
    let mut path = Request::default();
    path.statics.push((DATA + 0x9000, 0x301));
    let mut size = [0u8; 16];
    size[8..16].copy_from_slice(&8u64.to_le_bytes());
    assert_eq!(
        cache
            .request(&mut p.memory, kernel, mount, 0, &path, &size)
            .unwrap()
            .code,
        0
    );
    let file = object(
        cache
            .request(&mut p.memory, kernel, mount, 8, &path, &3u32.to_le_bytes())
            .unwrap(),
    );
    assert_eq!(
        cache
            .request(&mut p.memory, kernel, mount, 1, &path, &[])
            .unwrap()
            .code,
        0xe02
    );
    assert_eq!(
        cache
            .request(&mut p.memory, kernel, mount, 8, &path, &3u32.to_le_bytes())
            .unwrap()
            .code,
        0xe02
    );
    let mut transfer = [0u8; 24];
    transfer[16..24].copy_from_slice(&8u64.to_le_bytes());
    let mut input = Request::default();
    input.sends.push((0, 8));
    assert!(
        cache
            .request(&mut p.memory, kernel, file, 1, &input, &transfer)
            .is_err()
    );
    p.memory
        .write_bytes(DATA + 0xa000, &[1, 3, 7, 15, 31, 63, 127, 255])
        .unwrap();
    input.sends[0].0 = DATA + 0xa000;
    cache
        .request(&mut p.memory, kernel, file, 1, &input, &transfer)
        .unwrap();
    assert!(
        cache
            .request(&mut p.memory, kernel, file, 3, &plain, &17u64.to_le_bytes())
            .is_err()
    );
    assert_eq!(
        cache
            .request(&mut p.memory, kernel, file, 4, &plain, &[])
            .unwrap()
            .data,
        8i64.to_le_bytes()
    );
    let mut output = Request::default();
    output.recvs.push((0, 8));
    assert!(
        cache
            .request(&mut p.memory, kernel, file, 0, &output, &transfer)
            .is_err()
    );
    output.recvs[0].0 = DATA + 0xb000;
    cache
        .request(&mut p.memory, kernel, file, 0, &output, &transfer)
        .unwrap();
    let mut actual = [0; 8];
    p.memory.read_into(DATA + 0xb000, &mut actual).unwrap();
    assert_eq!(actual, [1, 3, 7, 15, 31, 63, 127, 255]);
    cache.close(file);
    assert!(
        cache
            .request(&mut p.memory, kernel, file, 4, &plain, &[])
            .is_err()
    );
    let readonly = object(
        cache
            .request(&mut p.memory, kernel, host_fs::MANAGER, 53, &plain, &attr)
            .unwrap(),
    );
    assert!(
        cache
            .request(
                &mut p.memory,
                kernel,
                readonly,
                8,
                &path,
                &2u32.to_le_bytes()
            )
            .is_err()
    );
    assert_eq!(
        cache
            .request(&mut p.memory, kernel, mount, 1, &path, &[])
            .unwrap()
            .code,
        0
    );
    assert_eq!(
        cache
            .request(&mut p.memory, kernel, mount, 8, &path, &1u32.to_le_bytes())
            .unwrap()
            .code,
        0x202
    );
}

#[test]
fn volatile_save_mounts_share_only_the_exact_application_user_and_type() {
    use wl_runtime::{ipc::Request, services::HostObject};
    let (mut p, _) = process();
    let k = p.kernel.as_ref().unwrap();
    let mut fs = host_fs::Service::with_volatile_saves();
    let plain = Request::default();
    let object = |reply: wl_runtime::audio::Reply| match reply.objects[0] {
        HostObject::AudioRenderer { index } => index,
        _ => panic!(),
    };
    let mut attr = [0; 72];
    attr[0] = 1;
    attr[40] = 1;
    attr[16] = 1;
    let mount = object(
        fs.request(&mut p.memory, k, host_fs::MANAGER, 51, &plain, &attr)
            .unwrap(),
    );
    p.memory.write_bytes(DATA + 0x9000, b"/entry\0").unwrap();
    let mut path = Request::default();
    path.statics.push((DATA + 0x9000, 0x301));
    let mut create = [0; 16];
    create[8..16].copy_from_slice(&3u64.to_le_bytes());
    fs.request(&mut p.memory, k, mount, 0, &path, &create)
        .unwrap();
    let alias = object(
        fs.request(&mut p.memory, k, host_fs::MANAGER, 53, &plain, &attr)
            .unwrap(),
    );
    assert_eq!(
        fs.request(&mut p.memory, k, alias, 7, &path, &[])
            .unwrap()
            .data,
        1u32.to_le_bytes()
    );
    assert!(fs.request(&mut p.memory, k, alias, 1, &path, &[]).is_err());
    attr[16] = 2;
    assert_eq!(
        fs.request(&mut p.memory, k, host_fs::MANAGER, 53, &plain, &attr)
            .unwrap()
            .code,
        0x7d402
    );
    let second = object(
        fs.request(&mut p.memory, k, host_fs::MANAGER, 51, &plain, &attr)
            .unwrap(),
    );
    assert_eq!(
        fs.request(&mut p.memory, k, second, 7, &path, &[])
            .unwrap()
            .code,
        0x202
    );
    attr[16] = 0;
    attr[40] = 3;
    let device = object(
        fs.request(&mut p.memory, k, host_fs::MANAGER, 51, &plain, &attr)
            .unwrap(),
    );
    assert_eq!(
        fs.request(&mut p.memory, k, device, 7, &path, &[])
            .unwrap()
            .code,
        0x202
    );
    attr[8..16].copy_from_slice(&0xffffu64.to_le_bytes());
    assert_eq!(
        fs.request(&mut p.memory, k, host_fs::MANAGER, 53, &plain, &attr)
            .unwrap()
            .code,
        0x7d402
    );
    // A fresh process's store never inherits these volatile contents.
    let mut fresh = host_fs::Service::default();
    attr[8..16].fill(0);
    assert_eq!(
        fresh
            .request(&mut p.memory, k, host_fs::MANAGER, 51, &plain, &attr)
            .unwrap()
            .code,
        0x7d402
    );
    // Failed two-path renames and open-file deletion preserve the source entry.
    p.memory.write_bytes(DATA + 0xa000, b"/else\0").unwrap();
    let mut rename = Request::default();
    rename
        .statics
        .extend_from_slice(&[(DATA + 0x9000, 0x301), (DATA + 0xa000, 0x301)]);
    let file = object(
        fs.request(&mut p.memory, k, mount, 8, &path, &1u32.to_le_bytes())
            .unwrap(),
    );
    assert_eq!(
        fs.request(&mut p.memory, k, mount, 5, &rename, &[])
            .unwrap()
            .code,
        0xe02
    );
    fs.close(file);
    rename.statics[1] = (0, 0x301);
    assert!(
        fs.request(&mut p.memory, k, mount, 5, &rename, &[])
            .is_err()
    );
    assert_eq!(
        fs.request(&mut p.memory, k, mount, 7, &path, &[])
            .unwrap()
            .data,
        1u32.to_le_bytes()
    );
}

#[test]
fn temporary_save_uses_space_three_type_four_without_enabling_durable_save_policy() {
    use wl_runtime::{ipc::Request, services::HostObject};
    let (mut p, _) = process();
    let k = p.kernel.as_ref().unwrap();
    let mut fs = host_fs::Service::default();
    let plain = Request::default();
    let mut attr = [0; 72];
    attr[0] = 3;
    attr[40] = 4;
    let obj = |reply: wl_runtime::audio::Reply| match reply.objects[0] {
        HostObject::AudioRenderer { index } => index,
        _ => panic!(),
    };
    assert_eq!(
        fs.request(&mut p.memory, k, host_fs::MANAGER, 53, &plain, &attr)
            .unwrap()
            .code,
        0x7d402
    );
    let temp = obj(fs
        .request(&mut p.memory, k, host_fs::MANAGER, 51, &plain, &attr)
        .unwrap());
    p.memory.write_bytes(DATA + 0x9000, b"/test\0").unwrap();
    let mut path = Request::default();
    path.statics.push((DATA + 0x9000, 0x301));
    let mut size = [0; 16];
    size[8..16].copy_from_slice(&4u64.to_le_bytes());
    fs.request(&mut p.memory, k, temp, 0, &path, &size).unwrap();
    let alias = obj(fs
        .request(&mut p.memory, k, host_fs::MANAGER, 53, &plain, &attr)
        .unwrap());
    assert_eq!(
        fs.request(&mut p.memory, k, alias, 7, &path, &[])
            .unwrap()
            .data,
        1u32.to_le_bytes()
    );
    assert!(
        fs.request(&mut p.memory, k, alias, 0, &path, &size)
            .is_err()
    );
    let mut fresh = host_fs::Service::default();
    assert_eq!(
        fresh
            .request(&mut p.memory, k, host_fs::MANAGER, 53, &plain, &attr)
            .unwrap()
            .code,
        0x7d402
    );
    attr[0] = 1;
    assert!(
        fs.request(&mut p.memory, k, host_fs::MANAGER, 51, &plain, &attr)
            .is_err()
    );
    attr[40] = 1;
    attr[16] = 1;
    assert_eq!(
        fs.request(&mut p.memory, k, host_fs::MANAGER, 51, &plain, &attr)
            .unwrap()
            .code,
        0x7d402
    );
    // SD is a different storage medium, never an alias of an internal volume.
    attr[0] = 4;
    attr[40] = 5;
    attr[16] = 0;
    assert_eq!(
        fs.request(&mut p.memory, k, host_fs::MANAGER, 51, &plain, &attr)
            .unwrap()
            .code,
        0x7d402
    );
}
