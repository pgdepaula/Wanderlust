//! Horizon threads: lifecycle, TLS, waits, user-mode lock arbitration, condition variables,
//! stack aliases, and the scheduler running real block functions.
use wl_runtime::horizon::*;
use wl_runtime::threads::{self, ThreadState, Wait};
use wl_runtime::{BlockTable, Cpu, Memory, RuntimeResult, Stop};

const CODE: u64 = 0x800_0000;
const ENTRY: u64 = CODE + 0x100;

fn config() -> ProcessConfig {
    ProcessConfig {
        program_id: 0x0100_0000_0000_2000,
        process_id: 0x51,
        main_thread_id: 0x72,
        main_thread_stack_size: 0x10_0000,
        main_thread_priority: 44,
        ideal_core: 0,
        core_mask: 0b111,
        priority_mask: 0xffff_ffff_f000_0000, // priorities 28..=63
        memory_limit: 0x2000_0000,
        handle_table_size: 64,
        thread_limit: 8,
        is_application: true,
        svc_permissions: [u64::MAX; 3],
        service_access: &[(false, "*"), (true, "*")],
    }
}
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
    Process::horizon(
        m,
        CODE,
        Range {
            start: CODE,
            end: CODE + 0x4000,
        },
        config(),
    )
    .unwrap()
}
fn k(p: &mut Process) -> &mut Kernel {
    p.kernel.as_mut().unwrap()
}
fn svc(p: &mut Process, c: &mut Cpu, id: u16, args: &[u64]) -> (u32, Option<Stop>) {
    c.x[..args.len()].copy_from_slice(args);
    let stop = dispatch(c, p, id).unwrap();
    (c.x[0] as u32, stop)
}
/// Switches the running thread: saves `c` into the current thread and loads thread `t`.
fn switch(p: &mut Process, c: &mut Cpu, t: usize) {
    let k = k(p);
    let current = k.current;
    k.threads[current].context = Some(Box::new(c.clone()));
    *c = *k.threads[t].context.take().unwrap();
    k.current = t;
}
/// Creates and starts a thread with entry ENTRY on `core`; returns (index, handle).
fn spawn(p: &mut Process, c: &mut Cpu, priority: u64, core: u64) -> (usize, u32) {
    let stack = k(p).threads[0].stack.end - 0x8000;
    let (r, _) = svc(p, c, 0x08, &[0, ENTRY, 0x1234, stack, priority, core]);
    assert_eq!(r, result::SUCCESS);
    let handle = c.x[1] as u32;
    assert_eq!(svc(p, c, 0x09, &[u64::from(handle)]).0, result::SUCCESS);
    (k(p).threads.len() - 1, handle)
}

#[test]
fn create_thread_initial_context_tls_and_handles() {
    let (mut p, mut c) = process();
    c.fpcr = 0x0300_0000;
    let stack_top = k(&mut p).threads[0].stack.end - 0x1008;
    let (r, _) = svc(
        &mut p,
        &mut c,
        0x08,
        &[9, ENTRY | 1, 0xfeed, stack_top, 30, (-2i64) as u64],
    );
    assert_eq!(r, result::SUCCESS);
    let handle = c.x[1] as u32;
    // X2-X7 are not outputs of CreateThread and are cleared.
    assert_eq!(&c.x[2..8], &[0; 6]);
    let kk = k(&mut p);
    let t = &kk.threads[1];
    assert_eq!(
        (t.id, t.priority, t.core, t.state),
        (0x73, 30, 0, ThreadState::Initialized)
    );
    // Slot 0: process-local region, slot 1: main thread, slot 2: this thread.
    assert_eq!(t.tls, kk.process_local_region + 2 * TLS_REGION_SIZE);
    let ctx = t.context.as_ref().unwrap();
    assert_eq!(
        (ctx.pc, ctx.x[0], ctx.sp, ctx.tpidrro_el0),
        (ENTRY, 0xfeed, stack_top & !15, t.tls)
    );
    assert_eq!(ctx.x[18] & 1, 1);
    assert!(ctx
        .x
        .iter()
        .enumerate()
        .all(|(i, &x)| matches!(i, 0 | 18) || x == 0));
    assert_eq!(ctx.fpcr, 0x0300_0000);
    let tls = t.tls;
    assert_eq!(
        p.load(tls + TLS_THREAD_HANDLE, 4).unwrap(),
        u64::from(handle)
    );
    // Thread IDs come from GetThreadId on the new handle.
    assert_eq!(
        svc(&mut p, &mut c, 0x25, &[0, u64::from(handle)]).0,
        result::SUCCESS
    );
    assert_eq!(c.x[1], 0x73);
    // Starting twice is an invalid state; a bad handle is invalid.
    assert_eq!(
        svc(&mut p, &mut c, 0x09, &[u64::from(handle)]).0,
        result::SUCCESS
    );
    assert_eq!(
        svc(&mut p, &mut c, 0x09, &[u64::from(handle)]).0,
        result::INVALID_STATE
    );
    assert_eq!(
        svc(&mut p, &mut c, 0x09, &[0x1234]).0,
        result::INVALID_HANDLE
    );
}

#[test]
fn create_thread_validation() {
    let (mut p, mut c) = process();
    let sp = k(&mut p).threads[0].stack.end;
    // Core 3 is outside the core mask; core 64 is not a virtual core.
    assert_eq!(
        svc(&mut p, &mut c, 0x08, &[0, ENTRY, 0, sp, 44, 3]).0,
        result::INVALID_CORE_ID
    );
    assert_eq!(
        svc(&mut p, &mut c, 0x08, &[0, ENTRY, 0, sp, 44, 64]).0,
        result::INVALID_CORE_ID
    );
    // Priority 10 is outside the priority mask; 64 is not a priority.
    assert_eq!(
        svc(&mut p, &mut c, 0x08, &[0, ENTRY, 0, sp, 10, 0]).0,
        result::INVALID_PRIORITY
    );
    assert_eq!(
        svc(&mut p, &mut c, 0x08, &[0, ENTRY, 0, sp, 64, 0]).0,
        result::INVALID_PRIORITY
    );
    assert_eq!(c.x[1], 0, "no handle on failure");
    // The thread limit (8) counts the main thread; the ninth thread fails.
    for _ in 0..7 {
        assert_eq!(
            svc(&mut p, &mut c, 0x08, &[0, ENTRY, 0, sp, 44, 0]).0,
            result::SUCCESS
        );
    }
    assert_eq!(
        svc(&mut p, &mut c, 0x08, &[0, ENTRY, 0, sp, 44, 0]).0,
        result::LIMIT_REACHED
    );
    // 8 threads + process-local region = 9 regions: a second ThreadLocal page was mapped.
    let kk = k(&mut p);
    assert_eq!(kk.tls_pages.len(), 2);
    let mut tls: Vec<u64> = kk.threads.iter().map(|t| t.tls).collect();
    tls.sort();
    tls.dedup();
    assert_eq!(tls.len(), 8, "distinct TLS per thread");
    assert_eq!(
        svc(
            &mut p,
            &mut c,
            0x29,
            &[0, 24, u64::from(CURRENT_PROCESS), 0]
        )
        .0,
        result::SUCCESS
    );
    assert_eq!(c.x[1], 0, "no free thread slots");
}

#[test]
fn exit_wakes_waiters_and_handles_control_lifetime() {
    let (mut p, mut c) = process();
    let (worker, handle) = spawn(&mut p, &mut c, 44, 1);
    let tls = k(&mut p).threads[worker].tls;
    // Main waits on [main-thread handle, worker]; neither is signalled.
    let main_handle = k(&mut p).threads[0].handle;
    let list = c.sp - 0x40;
    p.write_bytes(list, &main_handle.to_le_bytes()).unwrap();
    p.write_bytes(list + 4, &handle.to_le_bytes()).unwrap();
    let pc = c.pc;
    let (_, stop) = svc(&mut p, &mut c, 0x18, &[0, list, 2, u64::MAX]);
    assert_eq!(stop, Some(Stop::Reschedule));
    assert_eq!(c.pc, pc + 4, "resumes after the SVC");
    assert_eq!(k(&mut p).threads[0].state, ThreadState::Waiting);
    switch(&mut p, &mut c, worker);
    assert_eq!(
        dispatch(&mut c, &mut p, 0x0A).unwrap(),
        Some(Stop::Reschedule)
    );
    let kk = k(&mut p);
    assert_eq!(kk.threads[worker].state, ThreadState::Exited);
    assert_eq!(kk.threads[0].state, ThreadState::Runnable);
    let main = kk.threads[0].context.as_ref().unwrap();
    assert_eq!(
        (main.x[0], main.x[1]),
        (0, 1),
        "success, index of the worker"
    );
    assert!(main.x[2..8].iter().all(|&x| x == 0));
    // The exited worker keeps its TLS while a handle exists; waiting on it returns at once.
    assert!(!kk.threads[worker].destroyed);
    switch(&mut p, &mut c, 0);
    p.write_bytes(list, &handle.to_le_bytes()).unwrap();
    assert_eq!(
        svc(&mut p, &mut c, 0x18, &[0, list, 1, 0]),
        (result::SUCCESS, None)
    );
    assert_eq!(c.x[1], 0);
    assert_eq!(
        svc(&mut p, &mut c, 0x16, &[u64::from(handle)]).0,
        result::SUCCESS
    );
    assert!(k(&mut p).threads[worker].destroyed);
    // The stale handle is invalid, and the TLS slot is reused with a fresh linear id.
    assert_eq!(
        svc(&mut p, &mut c, 0x25, &[0, u64::from(handle)]).0,
        result::INVALID_HANDLE
    );
    let (next, next_handle) = spawn(&mut p, &mut c, 44, 1);
    assert_eq!(k(&mut p).threads[next].tls, tls);
    assert_ne!(next_handle, handle);
    assert_eq!(
        k(&mut p).threads[next].id,
        0x74,
        "thread ids are not reused"
    );
}

#[test]
fn wait_synchronization_errors_and_timeouts() {
    let (mut p, mut c) = process();
    let (_, handle) = spawn(&mut p, &mut c, 44, 1);
    let list = c.sp - 0x40;
    p.write_bytes(list, &handle.to_le_bytes()).unwrap();
    assert_eq!(
        svc(&mut p, &mut c, 0x18, &[0, list, 0x41, 0]).0,
        result::OUT_OF_RANGE
    );
    assert_eq!(
        svc(&mut p, &mut c, 0x18, &[0, 0x10, 1, 0]).0,
        result::INVALID_POINTER
    );
    // Pseudo-handles are not accepted; unknown handles are invalid.
    p.write_bytes(list + 4, &CURRENT_THREAD.to_le_bytes())
        .unwrap();
    assert_eq!(
        svc(&mut p, &mut c, 0x18, &[0, list, 2, 0]).0,
        result::INVALID_HANDLE
    );
    // Timeout 0 on an unsignalled object: TimedOut immediately, output zero.
    assert_eq!(
        svc(&mut p, &mut c, 0x18, &[0, list, 1, 0]),
        (result::TIMED_OUT, None)
    );
    assert_eq!(c.x[1], 0);
    // A pending cancellation makes the next wait return Cancelled at once.
    assert_eq!(
        svc(&mut p, &mut c, 0x19, &[u64::from(CURRENT_THREAD)]).0,
        result::SUCCESS
    );
    assert_eq!(
        svc(&mut p, &mut c, 0x18, &[0, list, 1, u64::MAX]),
        (result::CANCELLED, None)
    );
    // A finite timeout blocks; the scheduler's expiry ends it with TimedOut and index -1.
    let (_, stop) = svc(&mut p, &mut c, 0x18, &[0, list, 1, 1_000_000]);
    assert_eq!(stop, Some(Stop::Reschedule));
    let kk = k(&mut p);
    kk.threads[0].context = Some(Box::new(c.clone()));
    let Some(Wait::Objects {
        deadline: Some(deadline),
        ..
    }) = kk.threads[0].wait.clone()
    else {
        panic!("not waiting");
    };
    // 1 ms at 19.2 MHz = 19200 ticks, plus the kernel's 2.
    assert_eq!(deadline, threads::ticks(0) + 19_200 + 2);
    threads::end_wait(kk, 0, result::TIMED_OUT, Some(-1));
    let main = kk.threads[0].context.as_ref().unwrap();
    assert_eq!(
        (main.x[0] as u32, main.x[1]),
        (result::TIMED_OUT, 0xffff_ffff)
    );
}

#[test]
fn cancel_synchronization_ends_a_wait() {
    let (mut p, mut c) = process();
    let (worker, handle) = spawn(&mut p, &mut c, 44, 1);
    let list = c.sp - 0x40;
    p.write_bytes(list, &handle.to_le_bytes()).unwrap();
    let main_handle = k(&mut p).threads[0].handle;
    svc(&mut p, &mut c, 0x18, &[0, list, 1, u64::MAX]);
    switch(&mut p, &mut c, worker);
    assert_eq!(
        svc(&mut p, &mut c, 0x19, &[u64::from(main_handle)]).0,
        result::SUCCESS
    );
    let main = k(&mut p).threads[0].context.clone().unwrap();
    assert_eq!(
        (main.x[0] as u32, main.x[1]),
        (result::CANCELLED, 0xffff_ffff)
    );
    assert!(!k(&mut p).threads[0].wait_cancelled);
}

#[test]
fn mutex_arbitration_hands_over_ownership() {
    let (mut p, mut c) = process();
    let (a, ha) = spawn(&mut p, &mut c, 44, 1);
    let (b, hb) = spawn(&mut p, &mut c, 40, 2);
    let main_handle = k(&mut p).threads[0].handle;
    let lock = CODE + 0x1000;
    // Main owns the lock; A and B find it held and register as waiters (libnx mutexLock).
    p.store(lock, 4, u64::from(main_handle | threads::HANDLE_WAIT_MASK))
        .unwrap();
    switch(&mut p, &mut c, a);
    // Alignment and owner validation.
    assert_eq!(
        svc(
            &mut p,
            &mut c,
            0x1A,
            &[u64::from(main_handle), lock + 2, u64::from(ha)]
        )
        .0,
        result::INVALID_ADDRESS
    );
    assert_eq!(
        svc(&mut p, &mut c, 0x1A, &[0x77, CODE - 0x1000, u64::from(ha)]).0,
        result::INVALID_CURRENT_MEMORY
    );
    // A word that no longer names the owner with the wait bit returns at once.
    assert_eq!(
        svc(&mut p, &mut c, 0x1A, &[u64::from(hb), lock, u64::from(ha)]),
        (result::SUCCESS, None)
    );
    let (_, stop) = svc(
        &mut p,
        &mut c,
        0x1A,
        &[u64::from(main_handle), lock, u64::from(ha)],
    );
    assert_eq!(stop, Some(Stop::Reschedule));
    switch(&mut p, &mut c, b);
    svc(
        &mut p,
        &mut c,
        0x1A,
        &[u64::from(main_handle), lock, u64::from(hb)],
    );
    // Main inherits the best waiter priority (B, 40) while it holds the lock.
    assert_eq!(threads::effective_priority(k(&mut p), 0), 40);
    switch(&mut p, &mut c, 0);
    // Unlock: B (higher priority) becomes owner; A still waits, so the wait bit stays.
    assert_eq!(svc(&mut p, &mut c, 0x1B, &[lock]).0, result::SUCCESS);
    assert_eq!(
        p.load(lock, 4).unwrap(),
        u64::from(hb | threads::HANDLE_WAIT_MASK)
    );
    assert_eq!(k(&mut p).threads[b].state, ThreadState::Runnable);
    assert_eq!(k(&mut p).threads[b].context.as_ref().unwrap().x[0], 0);
    assert_eq!(threads::effective_priority(k(&mut p), 0), 44);
    assert_eq!(threads::effective_priority(k(&mut p), b), 40);
    // A now waits for B; B's unlock hands the lock to A without the wait bit.
    assert!(matches!(k(&mut p).threads[a].wait, Some(Wait::Lock { owner, .. }) if owner == b));
    switch(&mut p, &mut c, b);
    assert_eq!(svc(&mut p, &mut c, 0x1B, &[lock]).0, result::SUCCESS);
    assert_eq!(p.load(lock, 4).unwrap(), u64::from(ha));
    assert_eq!(k(&mut p).threads[a].state, ThreadState::Runnable);
    // Unlocking with no waiters writes 0.
    switch(&mut p, &mut c, a);
    assert_eq!(svc(&mut p, &mut c, 0x1B, &[lock]).0, result::SUCCESS);
    assert_eq!(p.load(lock, 4).unwrap(), 0);
    assert_eq!(
        svc(&mut p, &mut c, 0x1B, &[lock + 1]).0,
        result::INVALID_ADDRESS
    );
}

#[test]
fn condition_variable_wait_signal_and_reacquire() {
    let (mut p, mut c) = process();
    let (a, ha) = spawn(&mut p, &mut c, 44, 1);
    let main_handle = k(&mut p).threads[0].handle;
    let (lock, key) = (CODE + 0x1000, CODE + 0x1004);
    // A holds the lock and waits on the condition variable: the lock is released.
    switch(&mut p, &mut c, a);
    p.store(lock, 4, u64::from(ha)).unwrap();
    let (_, stop) = svc(
        &mut p,
        &mut c,
        0x1C,
        &[lock, key | 3, u64::from(ha), u64::MAX],
    );
    assert_eq!(stop, Some(Stop::Reschedule));
    assert_eq!(p.load(lock, 4).unwrap(), 0);
    assert_eq!(p.load(key, 4).unwrap(), 1, "has-waiter flag");
    // Main takes the lock, then signals: A must wait for main's lock (wait bit set).
    switch(&mut p, &mut c, 0);
    p.store(lock, 4, u64::from(main_handle)).unwrap();
    let (_, stop) = svc(&mut p, &mut c, 0x1D, &[key, 1]);
    assert_eq!(stop, None);
    assert_eq!(c.x[0], 0, "SignalProcessWideKey returns nothing");
    assert_eq!(
        p.load(lock, 4).unwrap(),
        u64::from(main_handle | threads::HANDLE_WAIT_MASK)
    );
    assert_eq!(p.load(key, 4).unwrap(), 0, "no waiters left");
    assert!(matches!(
        k(&mut p).threads[a].wait,
        Some(Wait::Lock { owner: 0, .. })
    ));
    // Main unlocks: A reacquires and its WaitProcessWideKeyAtomic returns success.
    assert_eq!(svc(&mut p, &mut c, 0x1B, &[lock]).0, result::SUCCESS);
    assert_eq!(p.load(lock, 4).unwrap(), u64::from(ha));
    assert_eq!(k(&mut p).threads[a].state, ThreadState::Runnable);
    // A zero timeout releases the lock and reports TimedOut.
    switch(&mut p, &mut c, a);
    assert_eq!(
        svc(&mut p, &mut c, 0x1C, &[lock, key, u64::from(ha), 0]).0,
        result::TIMED_OUT
    );
    assert_eq!(p.load(lock, 4).unwrap(), 0);
    assert_eq!(
        svc(&mut p, &mut c, 0x1C, &[lock + 1, key, u64::from(ha), 0]).0,
        result::INVALID_ADDRESS
    );
}

#[test]
fn map_memory_aliases_heap_into_the_stack_region() {
    let (mut p, mut c) = process();
    assert_eq!(
        svc(&mut p, &mut c, 0x01, &[0, 0x20_0000]).0,
        result::SUCCESS
    );
    let heap = c.x[1];
    p.store(heap + 0x1008, 8, 0xabcd).unwrap();
    let l = k(&mut p).layout;
    let dst = l.stack.start + 0x40_0000;
    // Destination outside the stack region, misaligned, and a non-RW source.
    assert_eq!(
        svc(&mut p, &mut c, 0x04, &[heap, heap + 0x10000, 0x2000]).0,
        result::INVALID_MEMORY_REGION
    );
    assert_eq!(
        svc(&mut p, &mut c, 0x04, &[dst + 1, heap, 0x2000]).0,
        result::INVALID_ADDRESS
    );
    assert_eq!(
        svc(&mut p, &mut c, 0x04, &[dst, CODE, 0x1000]).0,
        result::INVALID_CURRENT_MEMORY
    );
    assert_eq!(
        svc(&mut p, &mut c, 0x04, &[dst, heap + 0x1000, 0x2000]).0,
        result::SUCCESS
    );
    // The alias sees the bytes; the source is inaccessible and Locked.
    assert_eq!(p.load(dst + 8, 8).unwrap(), 0xabcd);
    assert!(p.load(heap + 0x1008, 8).is_err());
    let src = query(&p, 1 << 39, heap + 0x1000);
    assert_eq!(
        (src.state, src.permission, src.attribute),
        (state::NORMAL, 0, attribute::LOCKED)
    );
    let alias = query(&p, 1 << 39, dst);
    assert_eq!(
        (alias.state, alias.permission, alias.size),
        (state::STACK, 3, 0x2000)
    );
    p.store(dst + 0x10, 8, 0x77).unwrap();
    // Used memory does not count the aliased pages twice.
    svc(&mut p, &mut c, 0x29, &[0, 7, u64::from(CURRENT_PROCESS), 0]);
    let used = c.x[1];
    // A destination whose pages are not the source's fails the page-group check.
    assert_eq!(
        svc(&mut p, &mut c, 0x05, &[dst, heap + 0x2000, 0x1000]).0,
        result::INVALID_MEMORY_REGION
    );
    // A consistent part of the alias is valid on the kernel but not modelled: explicit fault.
    c.x[..3].copy_from_slice(&[dst, heap + 0x1000, 0x1000]);
    assert!(dispatch(&mut c, &mut p, 0x05)
        .unwrap_err()
        .contains("part of a mapping"));
    assert_eq!(
        svc(&mut p, &mut c, 0x05, &[dst, heap + 0x1000, 0x2000]).0,
        result::SUCCESS
    );
    assert_eq!(
        p.load(heap + 0x1010, 8).unwrap(),
        0x77,
        "writes through the alias persist"
    );
    assert!(p.load(dst, 1).is_err());
    svc(&mut p, &mut c, 0x29, &[0, 7, u64::from(CURRENT_PROCESS), 0]);
    assert_eq!(c.x[1], used);
}

#[test]
fn svc_ends_the_exclusive_reservation() {
    let (mut p, mut c) = process();
    c.monitor = Some((CODE + 0x1000, 4));
    svc(&mut p, &mut c, 0x25, &[0, u64::from(CURRENT_THREAD)]);
    assert_eq!(c.monitor, None);
}

// ---------------------------------------------------------------- scheduler with blocks

/// Loop body at ENTRY: X19 += 1, branch to itself.
fn spin(c: &mut Cpu, _: &mut Process) -> RuntimeResult<Option<Stop>> {
    c.enter(ENTRY, 1)?;
    c.x[19] += 1;
    c.monitor = Some((CODE + 0x1000, 4));
    c.pc = ENTRY;
    Ok(None)
}
/// At ENTRY + 0x10: svcExitThread.
fn exit_thread(c: &mut Cpu, m: &mut Process) -> RuntimeResult<Option<Stop>> {
    c.enter(ENTRY + 0x10, 1)?;
    c.pc = ENTRY + 0x10;
    if let Some(s) = wl_runtime::service(c, m, 0x0A)? {
        return Ok(Some(s));
    }
    Err("returned from ExitThread".into())
}
/// At ENTRY + 0x20: svcSleepThread(X0) then branch to ENTRY + 0x10.
fn sleep_then_exit(c: &mut Cpu, m: &mut Process) -> RuntimeResult<Option<Stop>> {
    c.enter(ENTRY + 0x20, 1)?;
    c.pc = ENTRY + 0x20;
    if let Some(s) = wl_runtime::service(c, m, 0x0B)? {
        return Ok(Some(s));
    }
    c.pc = ENTRY + 0x24;
    Ok(None)
}
/// At ENTRY + 0x24 and ENTRY + 0x34 (after an SVC): branch to ENTRY + 0x10.
fn to_exit(c: &mut Cpu, _: &mut Process) -> RuntimeResult<Option<Stop>> {
    c.enter(c.pc, 1)?;
    c.pc = ENTRY + 0x10;
    Ok(None)
}
/// At ENTRY + 0x30: svcWaitSynchronization(X1 list, X2 count, X3 timeout), then exit.
fn wait_then_exit(c: &mut Cpu, m: &mut Process) -> RuntimeResult<Option<Stop>> {
    c.enter(ENTRY + 0x30, 1)?;
    c.pc = ENTRY + 0x30;
    if let Some(s) = wl_runtime::service(c, m, 0x18)? {
        return Ok(Some(s));
    }
    c.pc = ENTRY + 0x34;
    Ok(None)
}
static BLOCKS: [(u64, wl_runtime::BlockFn); 6] = [
    (ENTRY, spin),
    (ENTRY + 0x10, exit_thread),
    (ENTRY + 0x20, sleep_then_exit),
    (ENTRY + 0x24, to_exit),
    (ENTRY + 0x30, wait_then_exit),
    (ENTRY + 0x34, to_exit),
];
static UNITS: [(u64, BlockTable); 1] = [(CODE, &BLOCKS)];

fn scheduled(core: u64, priority: u64) -> (Process, usize) {
    let (mut p, mut c) = process();
    let (t, _) = spawn(&mut p, &mut c, priority, core);
    c.pc = ENTRY;
    k(&mut p).threads[0].context = Some(Box::new(c));
    (p, t)
}

#[test]
fn threads_on_different_cores_interleave() {
    let (mut p, t) = scheduled(1, 44);
    let run = threads::run(&mut p, &UNITS, 50_000, 1000);
    assert!(run.result.unwrap_err().contains("budget"));
    let kk = k(&mut p);
    let main = kk.threads[0].context.as_ref().unwrap().x[19];
    let worker = kk.threads[t].context.as_ref().unwrap().x[19];
    assert_eq!(main + worker, kk.total_steps);
    assert!(main >= 24_000 && worker >= 24_000, "{main} {worker}");
    assert!(kk.stats.context_switches >= 48);
    // Each switch ended the exclusive reservation of the thread switched out.
    assert_eq!(kk.threads[t].context.as_ref().unwrap().monitor, None);
}

#[test]
fn same_core_runs_the_better_priority_only() {
    let (mut p, t) = scheduled(0, 40);
    let run = threads::run(&mut p, &UNITS, 20_000, 1000);
    assert!(run.result.is_err());
    let kk = k(&mut p);
    assert_eq!(kk.threads[0].context.as_ref().unwrap().x[19], 0);
    assert_eq!(kk.threads[t].context.as_ref().unwrap().x[19], 20_000);
}

#[test]
fn sleeping_threads_advance_virtual_time_and_waiters_wake_on_exit() {
    let (mut p, mut c) = process();
    let (t, handle) = spawn(&mut p, &mut c, 44, 1);
    // Worker sleeps 3 ms then exits; main waits for it without a timeout, then exits.
    k(&mut p).threads[t].context.as_mut().unwrap().pc = ENTRY + 0x20;
    k(&mut p).threads[t].context.as_mut().unwrap().x[0] = 3_000_000;
    let list = c.sp - 0x40;
    p.write_bytes(list, &handle.to_le_bytes()).unwrap();
    c.pc = ENTRY + 0x30;
    c.x[1] = list;
    c.x[2] = 1;
    c.x[3] = u64::MAX;
    k(&mut p).threads[0].context = Some(Box::new(c));
    let run = threads::run(&mut p, &UNITS, 1_000_000, 1000);
    assert_eq!(run.result.unwrap(), Stop::ExitThread);
    let kk = k(&mut p);
    assert!(kk.time_ns >= 3_000_000);
    assert!(kk.stats.idle_ns > 2_900_000);
    assert_eq!(kk.stats.blocks, 2);
    assert_eq!(kk.stats.timeouts, 1, "the sleep");
    assert!(kk.threads.iter().all(|t| t.state == ThreadState::Exited));
}

#[test]
fn waiting_forever_is_reported_as_deadlock() {
    let (mut p, mut c) = process();
    let (t, handle) = spawn(&mut p, &mut c, 44, 1);
    // Both threads wait on each other's handles forever.
    let main_handle = k(&mut p).threads[0].handle;
    let list = c.sp - 0x40;
    p.write_bytes(list, &handle.to_le_bytes()).unwrap();
    p.write_bytes(list + 4, &main_handle.to_le_bytes()).unwrap();
    let ctx = k(&mut p).threads[t].context.as_mut().unwrap();
    (ctx.pc, ctx.x[1], ctx.x[2], ctx.x[3]) = (ENTRY + 0x30, list + 4, 1, u64::MAX);
    (c.pc, c.x[1], c.x[2], c.x[3]) = (ENTRY + 0x30, list, 1, u64::MAX);
    k(&mut p).threads[0].context = Some(Box::new(c));
    let run = threads::run(&mut p, &UNITS, 1_000_000, 1000);
    assert!(run.result.unwrap_err().contains("guest deadlock"));
}

#[test]
fn events_signal_clear_reset_and_wake_waiters() {
    let (mut p, mut c) = process();
    let (r, _) = svc(&mut p, &mut c, 0x45, &[]);
    assert_eq!(r, result::SUCCESS);
    let (writable, readable) = (c.x[1] as u32, c.x[2] as u32);
    assert!(c.x[3..8].iter().all(|&x| x == 0));
    // Unsignalled: a zero-timeout wait times out; ResetSignal is InvalidState.
    let list = c.sp - 0x40;
    p.write_bytes(list, &readable.to_le_bytes()).unwrap();
    assert_eq!(
        svc(&mut p, &mut c, 0x18, &[0, list, 1, 0]).0,
        result::TIMED_OUT
    );
    assert_eq!(
        svc(&mut p, &mut c, 0x17, &[u64::from(readable)]).0,
        result::INVALID_STATE
    );
    // Only the writable half signals; only the readable half is waitable or reset.
    assert_eq!(
        svc(&mut p, &mut c, 0x11, &[u64::from(readable)]).0,
        result::INVALID_HANDLE
    );
    assert_eq!(
        svc(&mut p, &mut c, 0x17, &[u64::from(writable)]).0,
        result::INVALID_HANDLE
    );
    p.write_bytes(list, &writable.to_le_bytes()).unwrap();
    assert_eq!(
        svc(&mut p, &mut c, 0x18, &[0, list, 1, 0]).0,
        result::INVALID_HANDLE
    );
    // A worker waits; signalling wakes it with its index and the event stays signalled.
    let (worker, _) = spawn(&mut p, &mut c, 44, 1);
    switch(&mut p, &mut c, worker);
    p.write_bytes(list, &readable.to_le_bytes()).unwrap();
    assert_eq!(
        svc(&mut p, &mut c, 0x18, &[0, list, 1, u64::MAX]).1,
        Some(Stop::Reschedule)
    );
    switch(&mut p, &mut c, 0);
    assert_eq!(
        svc(&mut p, &mut c, 0x11, &[u64::from(writable)]),
        (result::SUCCESS, Some(Stop::Reschedule))
    );
    let w = &k(&mut p).threads[worker];
    assert_eq!(w.state, ThreadState::Runnable);
    assert_eq!(w.context.as_ref().unwrap().x[..2], [0, 0]);
    assert_eq!(svc(&mut p, &mut c, 0x18, &[0, list, 1, 0]), (0, None));
    // ResetSignal clears once; ClearEvent works on either half.
    assert_eq!(svc(&mut p, &mut c, 0x17, &[u64::from(readable)]).0, 0);
    assert_eq!(
        svc(&mut p, &mut c, 0x18, &[0, list, 1, 0]).0,
        result::TIMED_OUT
    );
    svc(&mut p, &mut c, 0x11, &[u64::from(writable)]);
    assert_eq!(svc(&mut p, &mut c, 0x12, &[u64::from(writable)]).0, 0);
    assert_eq!(
        svc(&mut p, &mut c, 0x18, &[0, list, 1, 0]).0,
        result::TIMED_OUT
    );
    assert_eq!(svc(&mut p, &mut c, 0x12, &[u64::from(readable)]).0, 0);
    // Closing both halves.
    assert_eq!(svc(&mut p, &mut c, 0x16, &[u64::from(writable)]).0, 0);
    assert_eq!(svc(&mut p, &mut c, 0x16, &[u64::from(readable)]).0, 0);
    let e = &k(&mut p).events[0];
    assert_eq!((e.readable_references, e.writable_references), (0, 0));
}

#[test]
fn thread_core_masks() {
    let (mut p, mut c) = process();
    let me = u64::from(CURRENT_THREAD);
    let get = |p: &mut Process, c: &mut Cpu| {
        let (r, _) = svc(p, c, 0x0E, &[0, 0, me]);
        (r, c.x[1] as u32 as i32, c.x[2])
    };
    assert_eq!(get(&mut p, &mut c), (0, 0, 1));
    // Moving the running thread to core 2 ends its slice.
    let (r, stop) = svc(&mut p, &mut c, 0x0F, &[me, 2, 0b100]);
    assert_eq!((r, stop), (0, Some(Stop::Reschedule)));
    assert_eq!(k(&mut p).threads[0].core, 2);
    assert_eq!(get(&mut p, &mut c), (0, 2, 0b100));
    // Validation, in Mesosphere's order.
    for (core, mask, expected) in [
        (3u64, 0b1000u64, result::INVALID_CORE_ID),
        (0, 0, result::INVALID_COMBINATION),
        (1, 0b100, result::INVALID_COMBINATION),
        (7, 0b1, result::INVALID_COMBINATION),
        (64, 0b1, result::INVALID_CORE_ID),
    ] {
        assert_eq!(svc(&mut p, &mut c, 0x0F, &[me, core, mask]).0, expected);
    }
    assert_eq!(
        svc(&mut p, &mut c, 0x0F, &[0x1234, 0, 1]).0,
        result::INVALID_HANDLE
    );
    // No preference: the thread leaves core 2 for the highest core of the mask.
    let dont_care = u64::from((-1i32) as u32);
    assert_eq!(svc(&mut p, &mut c, 0x0F, &[me, dont_care, 0b011]).0, 0);
    assert_eq!(k(&mut p).threads[0].core, 1);
    // NoUpdate needs an ideal core inside the mask.
    let no_update = u64::from((-3i32) as u32);
    assert_eq!(
        svc(&mut p, &mut c, 0x0F, &[me, no_update, 0b001]).0,
        result::INVALID_COMBINATION
    );
    // The process value: ideal core 0, mask {0}.
    let process_value = u64::from((-2i32) as u32);
    assert_eq!(svc(&mut p, &mut c, 0x0F, &[me, process_value, 0]).0, 0);
    assert_eq!(get(&mut p, &mut c), (0, 0, 1));
    assert_eq!(svc(&mut p, &mut c, 0x0F, &[me, no_update, 0b011]).0, 0);
    assert_eq!(get(&mut p, &mut c), (0, 0, 0b011));
}
