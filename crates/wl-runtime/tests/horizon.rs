//! Horizon process environment: layout, handles, bootstrap state, memory queries and SVCs.
use wl_runtime::horizon::*;
use wl_runtime::{Cpu, Memory};

const CODE: u64 = 0x800_0000;
const GIB: u64 = 1 << 30;

fn config() -> ProcessConfig {
    ProcessConfig {
        program_id: 0x0100_0000_0000_2000,
        process_id: 0x51,
        main_thread_id: 0x72,
        main_thread_stack_size: 0x10_0000,
        main_thread_priority: 44,
        ideal_core: 0,
        core_mask: 1,
        priority_mask: u64::MAX,
        memory_limit: 0x2000_0000,
        system_resource_size: 0,
        handle_table_size: 1023,
        thread_limit: 32,
        is_application: true,
        svc_permissions: [u64::MAX; 3],
        service_access: &[(false, "*"), (true, "*")],
    }
}
/// A process with RX code at CODE (one page), RW data after it and the main thread.
fn process() -> (Process, Cpu) {
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
    let code = Range {
        start: CODE,
        end: CODE + 0x4000,
    };
    Process::horizon(m, CODE, code, config()).unwrap()
}
fn svc(p: &mut Process, c: &mut Cpu, id: u16, args: &[u64]) -> u32 {
    c.x[..args.len()].copy_from_slice(args);
    assert_eq!(dispatch(c, p, id).unwrap(), None);
    c.x[0] as u32
}
fn info(p: &Process, address: u64) -> MemoryInfo {
    query(p, 1 << 39, address)
}

#[test]
fn layout_matches_mesosphere_without_aslr() {
    let l = Layout::new_39bit(CODE, 0x4000).unwrap();
    // Code rounds up to 2 MiB at 0x8200000; everything fits above it. Largest first
    // (KernelMap 64 GiB, Alias 64 GiB, Heap 8 GiB, Stack 2 GiB), then ordered by type.
    let start = 0x820_0000;
    assert_eq!(
        l.kernel_map,
        Range {
            start,
            end: start + 64 * GIB
        }
    );
    assert_eq!(l.stack.start, l.kernel_map.end);
    assert_eq!(l.stack.size(), 2 * GIB);
    assert_eq!(l.alias.start, l.stack.end);
    assert_eq!(l.alias.size(), 64 * GIB);
    assert_eq!(l.heap.start, l.alias.end);
    assert_eq!(l.heap.size(), 8 * GIB);
    assert_eq!(
        l.code_region,
        Range {
            start: CODE,
            end: 1 << 39
        }
    );
    assert!(Layout::new_39bit(0x1000, 0x1000).is_err());
    assert!(Layout::new_39bit(CODE, 0).is_err());
}

#[test]
fn handles_use_index_and_linear_id_top_down() {
    let mut t = HandleTable::new(4);
    let a = t.add(Object::Thread(0)).unwrap();
    let b = t.add(Object::Thread(1)).unwrap();
    assert_eq!((a, b), (3 | 1 << 15, 2 | 2 << 15));
    assert_eq!(t.get(a), Some(Object::Thread(0)));
    assert!(t.close(a));
    assert!(!t.close(a));
    assert_eq!(t.get(a), None);
    // The freed slot is reused with a new linear id, so the stale handle stays invalid.
    let c = t.add(Object::Thread(2)).unwrap();
    assert_eq!(c, 3 | 3 << 15);
    assert_eq!(t.get(a), None);
    t.add(Object::Thread(3)).unwrap();
    t.add(Object::Thread(4)).unwrap();
    assert_eq!(t.add(Object::Thread(5)), Err(result::OUT_OF_HANDLES));
    assert_eq!(t.get(0x4000_0000 | 3), None);
}

#[test]
fn bootstrap_state_follows_the_nso_entry_contract() {
    let (p, c) = process();
    let k = p.kernel.as_ref().unwrap();
    let t = &k.threads[0];
    assert_eq!(c.pc, CODE);
    assert_eq!(c.x[0], 0);
    assert_eq!(c.x[1], u64::from(t.handle));
    assert_eq!(t.handle, 1022 | 1 << 15); // 1023-entry table
    // Every other register is zero except X18, the kernel's (here deterministic) odd CFI value.
    assert!(c.x[2..18].iter().chain(&c.x[19..]).all(|&x| x == 0));
    assert_eq!(c.x[18] & 1, 1);
    assert_eq!(c.sp, t.stack.end);
    assert_eq!(c.sp % 16, 0);
    assert_eq!(c.tpidrro_el0, t.tls);
    assert_eq!(t.tls, k.process_local_region + TLS_REGION_SIZE);
    assert_eq!(
        p.load(t.tls + TLS_THREAD_HANDLE, 4).unwrap(),
        u64::from(t.handle)
    );
    assert!(k.layout.stack.start <= t.stack.start && t.stack.end <= k.layout.stack.end);
    assert!(k.layout.kernel_map.start <= t.tls && t.tls < k.layout.kernel_map.end);
    let stack = info(&p, t.stack.start);
    assert_eq!(
        (stack.base, stack.size, stack.state, stack.permission),
        (t.stack.start, 0x10_0000, state::STACK, 3)
    );
    let tls = info(&p, t.tls);
    assert_eq!(
        (tls.state, tls.permission, tls.size),
        (state::THREAD_LOCAL, 3, PAGE)
    );
    let mut bad = config();
    bad.main_thread_stack_size = 0x1234;
    let m = Memory::default();
    assert!(
        Process::horizon(
            m,
            CODE,
            Range {
                start: CODE,
                end: CODE + 0x1000
            },
            bad
        )
        .is_err()
    );
}

#[test]
fn query_reports_free_gaps_inaccessible_tail_and_coalesces() {
    let (mut p, _) = process();
    let code = info(&p, CODE + 4);
    assert_eq!(
        (code.base, code.size, code.state, code.permission),
        (CODE, 0x1000, state::CODE, 5)
    );
    let gap = info(&p, CODE - 1);
    assert_eq!((gap.base, gap.state, gap.permission), (0, state::FREE, 0));
    assert_eq!(gap.base + gap.size, CODE);
    let tail = info(&p, 1 << 39);
    assert_eq!(
        (tail.base, tail.size, tail.state),
        (1 << 39, 0u64.wrapping_sub(1 << 39), state::INACCESSIBLE)
    );
    // Adjacent regions with equal state, permission and attribute form one block.
    p.map_state(
        CODE + 0x4000,
        vec![0; 0x1000],
        true,
        true,
        false,
        state::CODE_DATA,
    )
    .unwrap();
    let data = info(&p, CODE + 0x4000);
    assert_eq!((data.base, data.size), (CODE + 0x1000, 0x4000));
    let bytes = data.to_bytes();
    assert_eq!(bytes.len(), 0x28);
    assert_eq!(
        u32::from_le_bytes(bytes[16..20].try_into().unwrap()),
        state::CODE_DATA
    );
}

#[test]
fn query_memory_svc_writes_memory_info_and_page_info() {
    let (mut p, mut c) = process();
    let out = c.sp - 0x100;
    assert_eq!(
        svc(&mut p, &mut c, 0x06, &[out, 0x5555, CODE + 8]),
        result::SUCCESS
    );
    assert_eq!(c.x[1], 0);
    assert_eq!(p.load(out, 8).unwrap(), CODE);
    assert_eq!(p.load(out + 8, 8).unwrap(), 0x1000);
    assert_eq!(p.load(out + 16, 4).unwrap(), u64::from(state::CODE));
    assert_eq!(p.load(out + 24, 4).unwrap(), 5);
    // An unwritable output pointer is reported, never a host fault.
    assert_eq!(
        svc(&mut p, &mut c, 0x06, &[CODE, 0, CODE]),
        result::INVALID_POINTER
    );
}

#[test]
fn get_info_values_and_errors() {
    let (mut p, mut c) = process();
    let l = p.kernel.as_ref().unwrap().layout;
    for (kind, expected) in [
        (0, 1),
        (2, l.alias.start),
        (3, l.alias.size()),
        (4, l.heap.start),
        (5, l.heap.size()),
        (6, 0x2000_0000),
        (12, l.code_region.start),
        (13, l.code_region.size()),
        (14, l.stack.start),
        (15, l.stack.size()),
        (18, 0x0100_0000_0000_2000),
        (23, 1),
    ] {
        assert_eq!(
            svc(
                &mut p,
                &mut c,
                0x29,
                &[0, kind, u64::from(CURRENT_PROCESS), 0]
            ),
            result::SUCCESS,
            "{kind}"
        );
        assert_eq!(c.x[1], expected, "info type {kind}");
    }
    // Used memory: code 0x4000 + stack + TLS page.
    svc(&mut p, &mut c, 0x29, &[0, 7, u64::from(CURRENT_PROCESS), 0]);
    assert_eq!(c.x[1], 0x4000 + 0x10_0000 + PAGE);
    let errors = [
        (
            6,
            u64::from(CURRENT_PROCESS),
            1,
            result::INVALID_COMBINATION,
        ),
        (6, 0x1234, 0, result::INVALID_HANDLE),
        (8, u64::from(CURRENT_PROCESS), 0, result::INVALID_HANDLE),
        (11, 0, 4, result::INVALID_COMBINATION),
        (0x7777, 0, 0, result::INVALID_ENUM_VALUE),
    ];
    for (kind, handle, sub, code) in errors {
        assert_eq!(
            svc(&mut p, &mut c, 0x29, &[0, kind, handle, sub]),
            code,
            "{kind}"
        );
        // Outputs of a failed call are zero; other argument registers are cleared.
        assert_eq!(&c.x[1..8], &[0; 7]);
    }
    // A type the kernel defines but the runtime does not model is a fault, not a result.
    c.x[1] = 25;
    let e = dispatch(&mut c, &mut p, 0x29).unwrap_err();
    assert!(e.contains("GetInfo) type 25"), "{e}");
    assert_eq!(svc(&mut p, &mut c, 0x29, &[0, 8, 0, 0]), result::SUCCESS);
    assert_eq!(c.x[1], 0);
}

#[test]
fn heap_grows_shrinks_and_validates() {
    let (mut p, mut c) = process();
    let heap = p.kernel.as_ref().unwrap().layout.heap.start;
    assert_eq!(
        svc(&mut p, &mut c, 0x01, &[0, 0x1000]),
        result::INVALID_SIZE
    );
    assert_eq!(
        svc(&mut p, &mut c, 0x01, &[0, 0x2000_0000]),
        result::OUT_OF_MEMORY
    );
    assert_eq!(svc(&mut p, &mut c, 0x01, &[0, 0x40_0000]), result::SUCCESS);
    assert_eq!(c.x[1], heap);
    p.store(heap + 0x3f_fff8, 8, 7).unwrap();
    let h = info(&p, heap);
    assert_eq!(
        (h.size, h.state, h.permission),
        (0x40_0000, state::NORMAL, 3)
    );
    assert_eq!(svc(&mut p, &mut c, 0x01, &[0, 0x60_0000]), result::SUCCESS);
    assert_eq!(p.load(heap + 0x3f_fff8, 8).unwrap(), 7);
    assert_eq!(p.load(heap + 0x5f_fff8, 8).unwrap(), 0);
    assert_eq!(svc(&mut p, &mut c, 0x01, &[0, 0x20_0000]), result::SUCCESS);
    assert!(p.load(heap + 0x20_0000, 1).is_err());
    assert_eq!(info(&p, heap).size, 0x20_0000);
    // Shrinking over reprotected heap pages is refused.
    assert_eq!(svc(&mut p, &mut c, 0x01, &[0, 0x40_0000]), result::SUCCESS);
    assert_eq!(
        svc(&mut p, &mut c, 0x02, &[heap + 0x30_0000, PAGE, 1]),
        result::SUCCESS
    );
    assert_eq!(
        svc(&mut p, &mut c, 0x01, &[0, 0x20_0000]),
        result::INVALID_CURRENT_MEMORY
    );
    assert_eq!(
        svc(&mut p, &mut c, 0x01, &[0, 0]),
        result::INVALID_CURRENT_MEMORY
    );
}

#[test]
fn memory_permission_splits_and_enforces() {
    let (mut p, mut c) = process();
    let data = CODE + 0x1000;
    // Argument validation order: address, size, permission, then state.
    assert_eq!(
        svc(&mut p, &mut c, 0x02, &[data + 1, PAGE, 1]),
        result::INVALID_ADDRESS
    );
    assert_eq!(
        svc(&mut p, &mut c, 0x02, &[data, 0, 1]),
        result::INVALID_SIZE
    );
    assert_eq!(
        svc(&mut p, &mut c, 0x02, &[data, PAGE, 5]),
        result::INVALID_NEW_MEMORY_PERMISSION
    );
    assert_eq!(
        svc(&mut p, &mut c, 0x02, &[CODE, PAGE, 1]),
        result::INVALID_CURRENT_MEMORY
    );
    assert_eq!(
        svc(&mut p, &mut c, 0x02, &[data + 0x2000, 0x2000, 1]),
        result::INVALID_CURRENT_MEMORY
    );
    p.store(data + 0x1ffc, 8, 0x1122_3344_5566_7788).unwrap();
    assert_eq!(
        svc(&mut p, &mut c, 0x02, &[data + PAGE, PAGE, 1]),
        result::SUCCESS
    );
    let ro = info(&p, data + PAGE);
    assert_eq!((ro.base, ro.size, ro.permission), (data + PAGE, PAGE, 1));
    assert_eq!(info(&p, data).size, PAGE);
    assert!(p.store(data + PAGE, 1, 0).is_err());
    // A read spanning the RW/R boundary succeeds; a write spanning it faults atomically.
    assert_eq!(p.load(data + 0x1ffc, 8).unwrap(), 0x1122_3344_5566_7788);
    assert!(p.store(data + 0xffc, 8, 0).is_err());
    assert_eq!(p.load(data + 0xffc, 4).unwrap(), 0);
    // Restoring RW keeps the split blocks but reports one coalesced block again.
    assert_eq!(
        svc(&mut p, &mut c, 0x02, &[data + PAGE, PAGE, 3]),
        result::SUCCESS
    );
    assert_eq!(info(&p, data).size, 0x3000);
    // Permission-locked memory cannot be reprotected.
    assert_eq!(
        svc(&mut p, &mut c, 0x03, &[data, PAGE, 0x10, 0x10]),
        result::SUCCESS
    );
    assert_eq!(info(&p, data).attribute, 0x10);
    assert_eq!(
        svc(&mut p, &mut c, 0x02, &[data, PAGE, 1]),
        result::INVALID_CURRENT_MEMORY
    );
}

#[test]
fn memory_attribute_validation() {
    let (mut p, mut c) = process();
    let data = CODE + 0x1000;
    assert_eq!(
        svc(&mut p, &mut c, 0x03, &[data, PAGE, 0x8, 0x18]),
        result::INVALID_COMBINATION
    );
    assert_eq!(
        svc(&mut p, &mut c, 0x03, &[data, PAGE, 0x1, 0x1]),
        result::INVALID_COMBINATION
    );
    // PermissionLocked can only be set, never cleared.
    assert_eq!(
        svc(&mut p, &mut c, 0x03, &[data, PAGE, 0x10, 0]),
        result::INVALID_COMBINATION
    );
    // Code memory cannot be permission-locked (no FlagCanPermissionLock).
    assert_eq!(
        svc(&mut p, &mut c, 0x03, &[CODE, PAGE, 0x10, 0x10]),
        result::INVALID_CURRENT_MEMORY
    );
    assert_eq!(
        svc(&mut p, &mut c, 0x03, &[data, PAGE, 0x8, 0x8]),
        result::SUCCESS
    );
    assert_eq!(info(&p, data).attribute, 0x8);
    assert_eq!(
        svc(&mut p, &mut c, 0x03, &[data, PAGE, 0x8, 0]),
        result::SUCCESS
    );
    assert_eq!(info(&p, data).attribute, 0);
}

#[test]
fn thread_process_and_handle_svcs() {
    let (mut p, mut c) = process();
    let handle = u64::from(p.kernel.as_ref().unwrap().threads[0].handle);
    assert_eq!(
        svc(&mut p, &mut c, 0x25, &[0, u64::from(CURRENT_THREAD)]),
        result::SUCCESS
    );
    assert_eq!(c.x[1], 0x72);
    assert_eq!(svc(&mut p, &mut c, 0x25, &[0, handle]), result::SUCCESS);
    assert_eq!(c.x[1], 0x72);
    assert_eq!(
        svc(&mut p, &mut c, 0x25, &[0, 0x1234]),
        result::INVALID_HANDLE
    );
    assert_eq!(
        svc(&mut p, &mut c, 0x24, &[0, u64::from(CURRENT_PROCESS)]),
        result::SUCCESS
    );
    assert_eq!(c.x[1], 0x51);
    assert_eq!(
        svc(&mut p, &mut c, 0x16, &[u64::from(CURRENT_THREAD)]),
        result::INVALID_HANDLE
    );
    assert_eq!(svc(&mut p, &mut c, 0x16, &[handle]), result::SUCCESS);
    assert_eq!(svc(&mut p, &mut c, 0x16, &[handle]), result::INVALID_HANDLE);
    // SleepThread(0) yields: it returns nothing (X0-X7 cleared, X8 and above preserved) and
    // hands the thread to the scheduler at the next instruction.
    c.x[8] = 0x88;
    c.x[..8].copy_from_slice(&[0, 1, 2, 3, 4, 5, 6, 7]);
    let pc = c.pc;
    assert_eq!(
        dispatch(&mut c, &mut p, 0x0B).unwrap(),
        Some(wl_runtime::Stop::Reschedule)
    );
    assert_eq!(&c.x[..9], &[0, 0, 0, 0, 0, 0, 0, 0, 0x88]);
    assert_eq!(c.pc, pc + 4);
    let text = b"hello";
    let at = c.sp - 16;
    p.write_bytes(at, text).unwrap();
    assert_eq!(svc(&mut p, &mut c, 0x27, &[at, 5]), result::SUCCESS);
    assert_eq!(p.kernel.as_ref().unwrap().debug_output, ["hello"]);
    assert_eq!(svc(&mut p, &mut c, 0x27, &[0, 5]), result::INVALID_POINTER);
    c.x[0] = 0;
    assert_eq!(
        dispatch(&mut c, &mut p, 0x07).unwrap(),
        Some(wl_runtime::Stop::ExitProcess)
    );
}

#[test]
fn capabilities_gate_svcs_and_size_the_handle_table() {
    let mut cfg = config();
    // Only ExitProcess (0x07) and GetInfo (0x29) are permitted.
    cfg.svc_permissions = [1 << 0x07 | 1 << 0x29, 0, 0];
    cfg.handle_table_size = 0;
    let mut m = Memory::default();
    m.map_state(CODE, vec![0; 0x1000], true, false, true, state::CODE)
        .unwrap();
    let (mut p, mut c) = Process::horizon(
        m,
        CODE,
        Range {
            start: CODE,
            end: CODE + 0x1000,
        },
        cfg,
    )
    .unwrap();
    // Size 0 is the kernel maximum: the first handle takes index 1023.
    assert_eq!(c.x[1], 1023 | 1 << 15);
    assert_eq!(
        svc(
            &mut p,
            &mut c,
            0x29,
            &[0, 18, u64::from(CURRENT_PROCESS), 0]
        ),
        result::SUCCESS
    );
    let e = dispatch(&mut c, &mut p, 0x25).unwrap_err();
    assert!(e.contains("SVC 0x25 is not permitted"), "{e}");
    assert_eq!(wl_runtime::fault_category(&e), "svc-not-permitted");
    assert!(dispatch(&mut c, &mut p, 0xC0).is_err());
}
#[test]
fn unsupported_and_break_svcs_fail_explicitly() {
    let (mut p, mut c) = process();
    let before = c.x;
    let e = dispatch(&mut c, &mut p, 0x22).unwrap_err();
    assert!(e.contains("unsupported Horizon SVC 0x22"), "{e}");
    assert_eq!(c.x, before);
    let e = dispatch(&mut c, &mut p, 0x26).unwrap_err();
    assert!(e.contains("guest svcBreak"), "{e}");
    let k = p.kernel.as_ref().unwrap();
    assert_eq!(k.svc_counts.get(&0x22), Some(&1));
}

#[test]
fn homebrew_entry_passes_a_loader_configuration() {
    let (mut p, mut c) = process();
    let config = p.homebrew_entry(&mut c).unwrap();
    assert_eq!(
        (c.x[0], c.x[1], c.x[30]),
        (config, u64::MAX, wl_runtime::RETURN_PC)
    );
    let handle = p.kernel.as_ref().unwrap().threads[0].handle;
    let entry = |i: u64, field: u64, size: u8| p.load(config + i * 24 + field, size).unwrap();
    // MainThreadHandle (mandatory), syscall hints from the SVC access flags, EndOfList.
    assert_eq!(
        (entry(0, 0, 4), entry(0, 4, 4), entry(0, 8, 8)),
        (1, 1, u64::from(handle))
    );
    assert_eq!(
        (entry(1, 0, 4), entry(1, 8, 8), entry(1, 16, 8)),
        (6, u64::MAX, u64::MAX)
    );
    assert_eq!((entry(2, 0, 4), entry(2, 8, 8)), (17, u64::MAX));
    assert_eq!((entry(3, 0, 4), entry(3, 8, 8), entry(3, 16, 8)), (0, 0, 0));
    let block = query(&p, 1 << 39, config);
    assert_eq!((block.state, block.permission), (state::NORMAL, 3));
}

#[test]
fn shared_memory_map_and_unmap() {
    let (mut p, mut c) = process();
    let k = p.kernel.as_mut().unwrap();
    let data = vec![0x5au8; PAGE as usize];
    let handle = u64::from(
        k.create_shared_memory(PAGE, 1, data)
            .expect("create shared memory"),
    );
    let target = 0x20_0000_0000u64; // free area

    // Invalid handle.
    assert_eq!(
        svc(&mut p, &mut c, 0x13, &[0x9999, target, PAGE, 1]),
        result::INVALID_HANDLE
    );
    // Unaligned address.
    assert_eq!(
        svc(&mut p, &mut c, 0x13, &[handle, target + 1, PAGE, 1]),
        result::INVALID_ADDRESS
    );
    // Invalid size.
    assert_eq!(
        svc(&mut p, &mut c, 0x13, &[handle, target, 0, 1]),
        result::INVALID_SIZE
    );
    assert_eq!(
        svc(&mut p, &mut c, 0x13, &[handle, target, 2 * PAGE, 1]),
        result::INVALID_SIZE
    );
    // Write permission on read-only shared memory.
    assert_eq!(
        svc(&mut p, &mut c, 0x13, &[handle, target, PAGE, 3]),
        result::INVALID_NEW_MEMORY_PERMISSION
    );
    // Overlapping existing code region.
    assert_eq!(
        svc(&mut p, &mut c, 0x13, &[handle, CODE, PAGE, 1]),
        result::INVALID_CURRENT_MEMORY
    );
    // Valid mapping.
    assert_eq!(
        svc(&mut p, &mut c, 0x13, &[handle, target, PAGE, 1]),
        result::SUCCESS
    );
    assert_eq!(info(&p, target).state, state::SHARED_MEMORY);
    assert_eq!(info(&p, target).permission, 1);
    let mut read_back = [0u8; 4];
    p.read_into(target, &mut read_back).unwrap();
    assert_eq!(read_back, [0x5a, 0x5a, 0x5a, 0x5a]);

    // Unmap validation:
    // Invalid handle.
    assert_eq!(
        svc(&mut p, &mut c, 0x14, &[0x9999, target, PAGE]),
        result::INVALID_HANDLE
    );
    // Unaligned address.
    assert_eq!(
        svc(&mut p, &mut c, 0x14, &[handle, target + 1, PAGE]),
        result::INVALID_ADDRESS
    );
    // Size mismatch.
    assert_eq!(
        svc(&mut p, &mut c, 0x14, &[handle, target, 2 * PAGE]),
        result::INVALID_SIZE
    );
    // Target is not shared memory (e.g. unmapped free area).
    assert_eq!(
        svc(&mut p, &mut c, 0x14, &[handle, target + PAGE, PAGE]),
        result::INVALID_CURRENT_MEMORY
    );
    // Valid unmap.
    assert_eq!(
        svc(&mut p, &mut c, 0x14, &[handle, target, PAGE]),
        result::SUCCESS
    );
    assert_eq!(info(&p, target).state, state::FREE);
}

#[test]
fn transfer_memory_creation_and_close() {
    let (mut p, mut c) = process();
    let target = 0x20_0000_0000u64;

    // Unaligned address.
    assert_eq!(
        svc(&mut p, &mut c, 0x15, &[0, target + 1, PAGE, 0]),
        result::INVALID_ADDRESS
    );
    // Zero size.
    assert_eq!(
        svc(&mut p, &mut c, 0x15, &[0, target, 0, 0]),
        result::INVALID_SIZE
    );
    // Unaligned size.
    assert_eq!(
        svc(&mut p, &mut c, 0x15, &[0, target, PAGE + 1, 0]),
        result::INVALID_SIZE
    );
    // Valid creation.
    assert_eq!(
        svc(&mut p, &mut c, 0x15, &[0, target, PAGE * 4, 0]),
        result::SUCCESS
    );
    let handle = c.x[1] as u32;
    assert_ne!(handle, 0);

    // Verify handle table and kernel state.
    let k = p.kernel.as_ref().unwrap();
    assert!(matches!(
        k.handles.get(handle),
        Some(Object::TransferMemory(_))
    ));

    // Close handle.
    assert_eq!(
        svc(&mut p, &mut c, 0x16, &[u64::from(handle)]),
        result::SUCCESS
    );
    let k = p.kernel.as_ref().unwrap();
    assert_eq!(k.handles.get(handle), None);
}

#[test]
fn memory_mapping_permissions_and_failures_do_not_depend_on_program_id() {
    for program_id in [0x0100_0000_0000_2000, 0x0100_f2c0_115b_6000] {
        let (mut p, mut c) = process();
        let k = p.kernel.as_mut().unwrap();
        k.config.program_id = program_id;
        let src = k.layout.heap.start;
        let dst = k.layout.stack.start + 0x40_0000;
        p.memory
            .map_state(
                src,
                vec![0x5a; (PAGE * 2) as usize],
                true,
                true,
                false,
                state::NORMAL,
            )
            .unwrap();
        for (destination, source, length, expected) in [
            (dst + 1, src, PAGE, result::INVALID_ADDRESS),
            (dst, src + 1, PAGE, result::INVALID_ADDRESS),
            (dst, src, PAGE + 1, result::INVALID_SIZE),
            (dst, src + PAGE * 16, PAGE, result::INVALID_CURRENT_MEMORY),
            (dst, CODE, PAGE, result::INVALID_CURRENT_MEMORY),
        ] {
            assert_eq!(
                svc(&mut p, &mut c, 4, &[destination, source, length]),
                expected,
                "program {program_id:#x}"
            );
            assert!(p.kernel.as_ref().unwrap().aliases.is_empty());
            assert_eq!(info(&p, src).permission, 3);
            assert_eq!(info(&p, dst).state, state::FREE);
            assert_eq!(p.load(src, 1).unwrap(), 0x5a);
        }
        // A registered GPU backing cannot be hidden by changing CPU mappings.
        let pin = p.memory.pin_gpu_range(src, PAGE).unwrap();
        assert_eq!(
            svc(&mut p, &mut c, 4, &[dst, src, PAGE]),
            result::INVALID_CURRENT_MEMORY
        );
        p.memory.release_gpu_pin(pin).unwrap();
        assert_eq!(svc(&mut p, &mut c, 4, &[dst, src, PAGE]), result::SUCCESS);
        assert!(
            p.load(src, 1).is_err(),
            "source must be inaccessible while aliased"
        );
        assert_eq!(
            (info(&p, src).permission, info(&p, src).attribute),
            (0, attribute::LOCKED)
        );
        assert_eq!(p.load(dst + 7, 1).unwrap(), 0x5a);
        p.store(dst + 7, 1, 0x91).unwrap();
        assert_eq!(
            svc(&mut p, &mut c, 5, &[dst + 1, src, PAGE]),
            result::INVALID_ADDRESS
        );
        assert_eq!(svc(&mut p, &mut c, 5, &[dst, src, PAGE]), result::SUCCESS);
        assert_eq!(p.load(src + 7, 1).unwrap(), 0x91);
        assert_eq!(info(&p, src).permission, 3);
        assert_eq!(info(&p, dst).state, state::FREE);
        // An occupied destination must survive rejection byte-for-byte.
        p.memory
            .map_state(
                dst,
                vec![0x7b; PAGE as usize],
                true,
                true,
                false,
                state::STACK,
            )
            .unwrap();
        assert_eq!(
            svc(&mut p, &mut c, 4, &[dst, src, PAGE]),
            result::INVALID_CURRENT_MEMORY
        );
        assert_eq!(p.load(dst, 1).unwrap(), 0x7b);
        assert_eq!(p.load(src + 7, 1).unwrap(), 0x91);
        assert!(p.kernel.as_ref().unwrap().aliases.is_empty());
    }
}

#[test]
fn physical_alias_memory_preserves_existing_pages_and_unmaps_holes() {
    let (mut p, mut c) = process();
    p.kernel.as_mut().unwrap().config.system_resource_size = 0x20_0000;
    let a = p.kernel.as_ref().unwrap().layout.alias.start;
    assert_eq!(
        svc(&mut p, &mut c, 0x2c, &[a + PAGE, PAGE]),
        result::SUCCESS
    );
    p.store(a + PAGE, 8, 0x0123_4567_89ab_cdef).unwrap();
    assert_eq!(svc(&mut p, &mut c, 0x2c, &[a, 3 * PAGE]), result::SUCCESS);
    assert_eq!(p.load(a, 8).unwrap(), 0);
    assert_eq!(p.load(a + PAGE, 8).unwrap(), 0x0123_4567_89ab_cdef);
    assert_eq!(p.load(a + 2 * PAGE, 8).unwrap(), 0);
    assert_eq!(info(&p, a).state, state::NORMAL);
    assert_eq!(info(&p, a).permission, 3);
    let before = used_memory(&p);
    assert_eq!(svc(&mut p, &mut c, 0x2c, &[a, 3 * PAGE]), result::SUCCESS);
    assert_eq!(used_memory(&p), before);
    assert_eq!(
        svc(&mut p, &mut c, 0x2d, &[a + PAGE, PAGE]),
        result::SUCCESS
    );
    assert!(p.load(a + PAGE, 1).is_err());
    assert_eq!(svc(&mut p, &mut c, 0x2d, &[a, 4 * PAGE]), result::SUCCESS);
    assert_eq!(used_memory(&p), before - 3 * PAGE);
    assert_eq!(svc(&mut p, &mut c, 0x2d, &[a, 4 * PAGE]), result::SUCCESS);
}

#[test]
fn physical_alias_memory_checks_state_quota_and_range_before_mutation() {
    let (mut p, mut c) = process();
    let a = p.kernel.as_ref().unwrap().layout.alias.start;
    assert_eq!(svc(&mut p, &mut c, 0x2c, &[a, PAGE]), result::INVALID_STATE);
    p.kernel.as_mut().unwrap().config.system_resource_size = PAGE;
    for (address, size, error) in [
        (a + 1, PAGE, result::INVALID_ADDRESS),
        (a, 0, result::INVALID_SIZE),
        (a, PAGE + 1, result::INVALID_SIZE),
        (u64::MAX - PAGE + 1, PAGE, result::INVALID_MEMORY_REGION),
        (a - PAGE, PAGE, result::INVALID_MEMORY_REGION),
        (
            p.kernel.as_ref().unwrap().layout.alias.end,
            PAGE,
            result::INVALID_MEMORY_REGION,
        ),
    ] {
        for id in [0x2c, 0x2d] {
            assert_eq!(svc(&mut p, &mut c, id, &[address, size]), error);
            assert!(p.load(a, 1).is_err());
        }
    }
    let used = used_memory(&p);
    p.kernel.as_mut().unwrap().config.memory_limit = used + PAGE + PAGE;
    assert_eq!(
        svc(&mut p, &mut c, 0x2c, &[a, 2 * PAGE]),
        result::LIMIT_REACHED
    );
    assert!(p.load(a, 1).is_err());
    assert_eq!(svc(&mut p, &mut c, 0x2c, &[a, PAGE]), result::SUCCESS);
    p.store(a, 8, 73).unwrap();
    assert_eq!(
        svc(&mut p, &mut c, 0x01, &[0, HEAP_ALIGNMENT]),
        result::OUT_OF_MEMORY
    );
    let pin = p.pin_gpu_range(a, PAGE).unwrap();
    assert_eq!(
        svc(&mut p, &mut c, 0x2d, &[a, 2 * PAGE]),
        result::INVALID_CURRENT_MEMORY
    );
    assert_eq!(p.load(a, 8).unwrap(), 73);
    p.release_gpu_pin(pin).unwrap();
    p.regions
        .iter_mut()
        .find(|r| r.base == a)
        .unwrap()
        .attribute = attribute::UNCACHED;
    assert_eq!(
        svc(&mut p, &mut c, 0x2d, &[a, 2 * PAGE]),
        result::INVALID_CURRENT_MEMORY
    );
    assert_eq!(p.load(a, 8).unwrap(), 73);
}

#[test]
fn system_resource_info_reserves_budget_and_validates_configuration() {
    let (mut p, mut c) = process();
    p.kernel.as_mut().unwrap().config.system_resource_size = 0x20_0000;
    let used = used_memory(&p);
    for (kind, expected) in [
        (16, 0x20_0000),
        (17, 0),
        (6, 0x2000_0000),
        (7, used + 0x20_0000),
        (21, 0x1fe0_0000),
        (22, used),
    ] {
        assert_eq!(
            svc(
                &mut p,
                &mut c,
                0x29,
                &[0, kind, u64::from(CURRENT_PROCESS), 0]
            ),
            result::SUCCESS
        );
        assert_eq!(c.x[1], expected);
    }
    for size in [1, 0x1fe0_1000] {
        let mut cfg = config();
        cfg.system_resource_size = size;
        assert!(
            Process::horizon(
                Memory::default(),
                CODE,
                Range {
                    start: CODE,
                    end: CODE + PAGE
                },
                cfg
            )
            .is_err()
        );
    }
}
