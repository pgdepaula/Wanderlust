//! Guest threads of a Horizon process: thread objects and TLS, the deterministic scheduler,
//! waits, and user-mode synchronization.
//!
//! Semantics follow Mesosphere (`KThread`, `KScheduler`, `KSynchronizationObject`,
//! `KConditionVariable`, `kern_svc_thread.cpp`, `kern_svc_synchronization.cpp`,
//! `kern_svc_lock.cpp`, `kern_svc_condition_variable.cpp`; Atmosphère 0e9cd964) and the
//! user-mode protocol of libnx (`kernel/thread.c`, `kernel/mutex.c`, `kernel/condvar.c`).
//!
//! All guest threads run on one host thread. Each thread owns a saved `Cpu`; the scheduler
//! moves it in and out of `execute_until`, which returns at block boundaries, so a thread
//! always resumes from its complete architectural state. A blocking SVC leaves its thread
//! `Waiting` with the PC after the SVC; whoever ends the wait writes the SVC's result and
//! outputs into the saved context, so no SVC side effect is ever repeated.
use crate::horizon::{
    self, CURRENT_THREAD, Kernel, Object, PAGE, Process, Range, TLS_REGION_SIZE, TLS_THREAD_HANDLE,
    result, state,
};
use crate::{BlockTable, Cpu, Memory, RuntimeResult, Stop, execute_until};

/// Bit 30 of a user mutex word: other threads wait for it (`svc::HandleWaitMask`).
pub const HANDLE_WAIT_MASK: u32 = 0x4000_0000;
/// `svc::ArgumentHandleCountMax`.
pub const ARGUMENT_HANDLE_COUNT_MAX: u64 = 0x40;
/// `svc::IdealCoreUseProcessValue`.
pub const IDEAL_CORE_USE_PROCESS_VALUE: i32 = -2;
/// `svc::LowestThreadPriority` (numerically largest).
pub const LOWEST_PRIORITY: i64 = 63;
/// Default time slice in guest instructions between scheduling decisions.
pub const DEFAULT_QUANTUM: u64 = 10_000;
/// The kernel rotates the queue of the preemption priority every 10 ms
/// (`kern_k_dpc_manager.cpp`).
pub const PREEMPTION_INTERVAL_NS: u64 = 10_000_000;
/// Number of virtual cores (`cpu::NumVirtualCores`).
pub const VIRTUAL_CORES: u64 = 64;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ThreadState {
    /// Created by svcCreateThread, not yet started.
    Initialized,
    Runnable,
    Waiting,
    Exited,
}

/// What a waiting thread waits for. Deadlines are in virtual ticks (19.2 MHz).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Wait {
    /// svcWaitSynchronization on these objects (cancellable).
    Objects {
        objects: Vec<Object>,
        deadline: Option<u64>,
    },
    /// svcSleepThread with a positive duration.
    Sleep { deadline: u64 },
    /// Waiting for `owner` to hand over the user lock at `address` (svcArbitrateLock, or a
    /// condition-variable waiter whose lock was taken). `tag` is the value written to the
    /// lock word when this thread becomes the owner.
    Lock {
        owner: usize,
        address: u64,
        tag: u32,
        order: u64,
        deadline: Option<u64>,
    },
    /// svcSendSyncRequest to sm:GetServiceHandle for a service not registered yet.
    Service { session: usize, name: [u8; 8] },
    /// svcWaitProcessWideKeyAtomic before being signalled.
    CondVar {
        key: u64,
        address: u64,
        tag: u32,
        order: u64,
        deadline: Option<u64>,
    },
}
impl std::fmt::Display for Wait {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Wait::Objects { objects, .. } => write!(f, "svcWaitSynchronization on {objects:?}"),
            Wait::Sleep { .. } => write!(f, "svcSleepThread"),
            Wait::Lock { address, .. } => write!(f, "a user lock at {address:#x}"),
            Wait::CondVar { key, .. } => write!(f, "a condition variable at {key:#x}"),
            Wait::Service { name, .. } => {
                let text: String = name
                    .iter()
                    .take_while(|&&b| b != 0)
                    .map(|&b| char::from(b))
                    .collect();
                write!(
                    f,
                    "sm:GetServiceHandle({text:?}), deferred until the service is registered"
                )
            }
        }
    }
}
impl Wait {
    fn deadline(&self) -> Option<u64> {
        match *self {
            Wait::Objects { deadline, .. }
            | Wait::Lock { deadline, .. }
            | Wait::CondVar { deadline, .. } => deadline,
            Wait::Sleep { deadline } => Some(deadline),
            Wait::Service { .. } => None,
        }
    }
}

/// Scheduler counters reported by the launcher.
#[derive(Clone, Copy, Debug, Default)]
pub struct SchedulerStats {
    pub slices: u64,
    pub context_switches: u64,
    pub blocks: u64,
    pub wakeups: u64,
    pub timeouts: u64,
    pub yields: u64,
    pub rotations: u64,
    /// Virtual nanoseconds skipped while every thread waited for a timeout.
    pub idle_ns: u64,
    pub max_runnable: u64,
    pub threads_created: u64,
}

/// Virtual ticks (19.2 MHz) at virtual time `ns`.
pub fn ticks(ns: u64) -> u64 {
    (u128::from(ns) * 192 / 10_000) as u64
}
/// The first virtual nanosecond at which `ticks(ns) >= tick`.
fn ns_at_tick(tick: u64) -> u64 {
    (u128::from(tick) * 10_000)
        .div_ceil(192)
        .min(u128::from(u64::MAX)) as u64
}
/// Kernel timeout conversion: now + ticks(ns) + 2; a non-positive tick offset never fires.
fn deadline_after(now_tick: u64, ns: i64) -> Option<u64> {
    let offset = ticks(ns as u64);
    (offset > 0).then(|| now_tick.saturating_add(offset).saturating_add(2))
}
/// Deterministic stand-in for the kernel's random odd X18 (CFI) value of a new thread.
pub fn cfi_value(program_id: u64, thread_id: u64) -> u64 {
    let mut z = program_id ^ thread_id.wrapping_mul(0x9e37_79b9_7f4a_7c15);
    z = (z ^ (z >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
    (z ^ (z >> 31)) | 1
}

/// Virtual time of the running thread: the kernel clock plus the instructions it retired in
/// the current slice (one instruction = 1 ns).
pub fn now_ns(k: &Kernel, c: &Cpu) -> u64 {
    k.time_ns + c.steps.saturating_sub(k.slice_start)
}

// ---------------------------------------------------------------- thread-local regions

/// Reserves a 0x200-byte thread-local region: the first free slot of the first page with
/// one (`KProcess::CreateThreadLocalRegion`), else a new ThreadLocal page in the KernelMap
/// region. The region is zeroed.
pub fn reserve_tls(memory: &mut Memory, k: &mut Kernel) -> Result<u64, u32> {
    let slots = (PAGE / TLS_REGION_SIZE) as u32;
    let address = if let Some(page) = k.tls_pages.iter_mut().find(|p| p.1.count_ones() < slots) {
        let slot = (!page.1).trailing_zeros();
        page.1 |= 1 << slot;
        page.0 + u64::from(slot) * TLS_REGION_SIZE
    } else {
        let page = horizon::find_free(memory, k.layout.kernel_map, PAGE)
            .map_err(|_| result::OUT_OF_MEMORY)?;
        memory
            .map_state(
                page,
                vec![0; PAGE as usize],
                true,
                true,
                false,
                state::THREAD_LOCAL,
            )
            .map_err(|_| result::OUT_OF_MEMORY)?;
        k.tls_pages.push((page, 1));
        page
    };
    memory
        .write_bytes(address, &[0; TLS_REGION_SIZE as usize])
        .map_err(|_| result::OUT_OF_MEMORY)?;
    Ok(address)
}
/// Releases a thread-local region; a page without regions is unmapped.
pub fn release_tls(memory: &mut Memory, k: &mut Kernel, address: u64) {
    let page = address & !(PAGE - 1);
    let slot = ((address - page) / TLS_REGION_SIZE) as u32;
    if let Some(i) = k.tls_pages.iter().position(|p| p.0 == page) {
        k.tls_pages[i].1 &= !(1 << slot);
        if k.tls_pages[i].1 == 0 {
            k.tls_pages.remove(i);
            memory
                .regions
                .retain(|r| !(r.base == page && r.state == state::THREAD_LOCAL));
        }
    }
}

// ---------------------------------------------------------------- thread objects

/// The thread object a handle names; `CURRENT_THREAD` is the running thread when allowed.
pub fn thread_of(k: &Kernel, handle: u32, pseudo: bool) -> Option<usize> {
    if handle == CURRENT_THREAD {
        return pseudo.then_some(k.current);
    }
    match k.handles.get(handle)? {
        Object::Thread(t) => Some(t),
        _ => None,
    }
}
/// Adds a handle to a thread object.
pub fn open_handle(k: &mut Kernel, t: usize) -> Result<u32, u32> {
    let handle = k.handles.add(Object::Thread(t))?;
    k.threads[t].references += 1;
    Ok(handle)
}
/// Closes a handle; an exited thread without handles is destroyed and its TLS released.
pub fn close_handle(memory: &mut Memory, k: &mut Kernel, handle: u32) -> bool {
    let Some(object) = k.handles.get(handle) else {
        return false;
    };
    k.handles.close(handle);
    match object {
        Object::Thread(t) => {
            k.threads[t].references -= 1;
            destroy_if_unreferenced(memory, k, t);
        }
        other => crate::ipc::close_handle(k, other),
    }
    true
}
fn destroy_if_unreferenced(memory: &mut Memory, k: &mut Kernel, t: usize) {
    let th = &k.threads[t];
    if th.state == ThreadState::Exited && th.references == 0 && !th.destroyed {
        let tls = th.tls;
        k.threads[t].destroyed = true;
        k.threads[t].context = None;
        release_tls(memory, k, tls);
    }
}
fn live_threads(k: &Kernel) -> u64 {
    k.threads.iter().filter(|t| !t.destroyed).count() as u64
}
/// Threads that have not exited.
pub fn running_threads(k: &Kernel) -> usize {
    k.threads
        .iter()
        .filter(|t| t.state != ThreadState::Exited)
        .count()
}
/// Free thread slots of the process resource limit (`InfoType_FreeThreadCount`).
pub fn free_thread_count(k: &Kernel) -> u64 {
    k.config.thread_limit.saturating_sub(live_threads(k))
}

/// Priority with inheritance: a lock owner runs at the best priority of the threads waiting
/// for its locks (`KThread::RestorePriority`).
pub fn effective_priority(k: &Kernel, t: usize) -> u8 {
    fn walk(k: &Kernel, t: usize, depth: u32) -> u8 {
        let mut p = k.threads[t].priority;
        if depth < 16 {
            for (w, th) in k.threads.iter().enumerate() {
                if let Some(Wait::Lock { owner, .. }) = th.wait {
                    if owner == t && th.state == ThreadState::Waiting && w != t {
                        p = p.min(walk(k, w, depth + 1));
                    }
                }
            }
        }
        p
    }
    walk(k, t, 0)
}

fn next_order(k: &mut Kernel) -> u64 {
    k.order += 1;
    k.order
}
fn make_runnable(k: &mut Kernel, t: usize) {
    let order = next_order(k);
    let th = &mut k.threads[t];
    th.state = ThreadState::Runnable;
    th.order = order;
}

/// Ends the wait of thread `t`: completes its pending SVC in its saved context with
/// `result` (and the synchronization index for svcWaitSynchronization) and makes it
/// runnable.
pub fn end_wait(k: &mut Kernel, t: usize, result_code: u32, index: Option<i32>) {
    let svc = k.threads[t].pending_svc;
    k.threads[t].wait = None;
    make_runnable(k, t);
    k.stats.wakeups += 1;
    if let Some(cpu) = k.threads[t].context.as_mut() {
        cpu.x[0] = u64::from(result_code);
        if let Some(i) = index {
            cpu.x[1] = u64::from(i as u32);
        }
        horizon::finish_registers(cpu, svc, index.is_some());
    }
}

/// svcExitThread for the running thread: it becomes signalled and its waiters wake.
pub fn exit_current(memory: &mut Memory, k: &mut Kernel) -> Stop {
    let t = k.current;
    k.threads[t].state = ThreadState::Exited;
    let waiters: Vec<(usize, i32)> = k
        .threads
        .iter()
        .enumerate()
        .filter_map(|(w, th)| match &th.wait {
            Some(Wait::Objects { objects, .. }) if th.state == ThreadState::Waiting => objects
                .iter()
                .position(|o| *o == Object::Thread(t))
                .map(|i| (w, i as i32)),
            _ => None,
        })
        .collect();
    for (w, i) in waiters {
        end_wait(k, w, result::SUCCESS, Some(i));
    }
    destroy_if_unreferenced(memory, k, t);
    if running_threads(k) == 0 {
        Stop::ExitThread
    } else {
        Stop::Reschedule
    }
}

/// Puts the running thread to sleep in `wait`; its SVC completes when the wait ends.
pub fn block_for(k: &mut Kernel, c: &mut Cpu, id: u16, wait: Wait) -> Option<Stop> {
    block(k, c, id, wait)
}
fn block(k: &mut Kernel, c: &mut Cpu, id: u16, wait: Wait) -> Option<Stop> {
    let t = k.current;
    k.threads[t].state = ThreadState::Waiting;
    k.threads[t].wait = Some(wait);
    k.threads[t].pending_svc = id;
    k.stats.blocks += 1;
    c.pc = c.pc.wrapping_add(4);
    Some(Stop::Reschedule)
}

// ---------------------------------------------------------------- SVC handlers

/// Result of an SVC handled here: `Done(result)` completes now; `Stop(s)` leaves the
/// running thread to the scheduler (its registers are completed later or already final).
pub enum Outcome {
    Done(u32),
    Stop(Stop),
}

/// svcCreateThread (0x08): X1 entry, X2 argument, X3 stack top, W4 priority, W5 core.
pub fn create_thread(memory: &mut Memory, k: &mut Kernel, c: &mut Cpu) -> Result<u32, u32> {
    let (entry, argument, stack_top) = (c.x[1], c.x[2], c.x[3]);
    let priority = i64::from(c.x[4] as u32 as i32);
    let mut core = c.x[5] as u32 as i32;
    if core == IDEAL_CORE_USE_PROCESS_VALUE {
        core = i32::from(k.config.ideal_core);
    }
    if !(0..VIRTUAL_CORES as i32).contains(&core) || k.config.core_mask >> core & 1 == 0 {
        return Err(result::INVALID_CORE_ID);
    }
    if !(0..=LOWEST_PRIORITY).contains(&priority) || k.config.priority_mask >> priority & 1 == 0 {
        return Err(result::INVALID_PRIORITY);
    }
    if live_threads(k) >= k.config.thread_limit {
        return Err(result::LIMIT_REACHED);
    }
    let tls = reserve_tls(memory, k)?;
    let t = k.threads.len();
    let id = k.next_thread_id;
    k.threads.push(horizon::Thread::new(
        id,
        tls,
        Range {
            start: stack_top,
            end: stack_top,
        },
        priority as u8,
        core as u8,
    ));
    let handle = match open_handle(k, t) {
        Ok(h) => h,
        Err(e) => {
            k.threads.pop();
            release_tls(memory, k, tls);
            return Err(e);
        }
    };
    k.next_thread_id += 1;
    k.stats.threads_created += 1;
    k.threads[t].handle = handle;
    let _ = memory.write_bytes(tls + TLS_THREAD_HANDLE, &handle.to_le_bytes());
    // KThreadContext: X0 = argument, X18 = CFI value, SP = stack top aligned down to 16,
    // FPCR/FPSR cloned from the creator, every other register zero.
    let mut cpu = Cpu::new(entry & !1);
    cpu.x = [0; 31];
    cpu.x[0] = argument;
    cpu.x[18] = cfi_value(k.config.program_id, id);
    cpu.sp = stack_top & !15;
    cpu.fpcr = c.fpcr;
    cpu.fpsr = c.fpsr;
    cpu.tpidrro_el0 = tls;
    k.threads[t].context = Some(Box::new(cpu));
    Ok(handle)
}

/// svcStartThread (0x09): W0 handle. Only an Initialized thread can start.
pub fn start_thread(k: &mut Kernel, handle: u32) -> u32 {
    let Some(t) = thread_of(k, handle, true) else {
        return result::INVALID_HANDLE;
    };
    if k.threads[t].state != ThreadState::Initialized {
        return result::INVALID_STATE;
    }
    make_runnable(k, t);
    result::SUCCESS
}

/// svcSleepThread (0x0B): positive durations sleep; 0, -1 and -2 yield; other values do
/// nothing (`kern_svc_thread.cpp`).
pub fn sleep_thread(k: &mut Kernel, c: &mut Cpu, ns: i64) -> Option<Stop> {
    if ns > 0 {
        let deadline = deadline_after(ticks(now_ns(k, c)), ns).unwrap_or(u64::MAX);
        return block(k, c, 0x0B, Wait::Sleep { deadline });
    }
    if (-2..=0).contains(&ns) {
        // Yield: to the back of the queue of its priority on its core.
        let t = k.current;
        make_runnable(k, t);
        k.stats.yields += 1;
        horizon::finish_registers(c, 0x0B, false);
        c.pc = c.pc.wrapping_add(4);
        return Some(Stop::Reschedule);
    }
    None
}

/// svcGetThreadPriority (0x0C): W1 handle -> W1 priority.
pub fn get_thread_priority(k: &Kernel, handle: u32) -> Result<u64, u32> {
    let t = thread_of(k, handle, true).ok_or(result::INVALID_HANDLE)?;
    Ok(u64::from(effective_priority(k, t)))
}
/// svcSetThreadPriority (0x0D): W0 handle, W1 priority.
pub fn set_thread_priority(k: &mut Kernel, handle: u32, priority: i64) -> u32 {
    let Some(t) = thread_of(k, handle, true) else {
        return result::INVALID_HANDLE;
    };
    if !(0..=LOWEST_PRIORITY).contains(&priority) || k.config.priority_mask >> priority & 1 == 0 {
        return result::INVALID_PRIORITY;
    }
    k.threads[t].priority = priority as u8;
    result::SUCCESS
}

/// svcWaitSynchronization (0x18): X1 handle array, W2 count, X3 timeout (ns) -> W1 index.
/// The outer `Err` is an unsupported case the runtime reports as a fault.
pub fn wait_synchronization(
    memory: &Memory,
    k: &mut Kernel,
    c: &mut Cpu,
) -> RuntimeResult<Result<(u32, Option<Stop>), u32>> {
    let (pointer, count, timeout) = (c.x[1], c.x[2] as u32 as i32 as i64, c.x[3] as i64);
    if !(0..=ARGUMENT_HANDLE_COUNT_MAX as i64).contains(&count) {
        return Ok(Err(result::OUT_OF_RANGE));
    }
    let mut raw = vec![0u8; count as usize * 4];
    if count > 0 && memory.read_into(pointer, &mut raw).is_err() {
        return Ok(Err(result::INVALID_POINTER));
    }
    let mut objects = Vec::with_capacity(count as usize);
    for chunk in raw.chunks_exact(4) {
        let handle = u32::from_le_bytes(chunk.try_into().unwrap_or_default());
        // GetMultipleObjects does not accept pseudo-handles; client sessions are not
        // synchronization objects.
        match (handle != CURRENT_THREAD)
            .then(|| k.handles.get(handle))
            .flatten()
        {
            Some(Object::Thread(t)) => objects.push(Object::Thread(t)),
            Some(Object::Port(_)) => {
                return Err(
                    "unsupported Horizon SVC 0x18 (WaitSynchronization) on a server port".into(),
                );
            }
            _ => return Ok(Err(result::INVALID_HANDLE)),
        }
    }
    let signalled =
        |o: &Object| matches!(o, Object::Thread(t) if k.threads[*t].state == ThreadState::Exited);
    if let Some(i) = objects.iter().position(signalled) {
        return Ok(Ok((i as u32, None)));
    }
    if timeout == 0 {
        return Ok(Err(result::TIMED_OUT));
    }
    let t = k.current;
    if k.threads[t].wait_cancelled {
        k.threads[t].wait_cancelled = false;
        return Ok(Err(result::CANCELLED));
    }
    let deadline = if timeout > 0 {
        deadline_after(ticks(now_ns(k, c)), timeout)
    } else {
        None
    };
    Ok(Ok((
        0,
        block(k, c, 0x18, Wait::Objects { objects, deadline }),
    )))
}

/// svcCancelSynchronization (0x19): W0 thread handle.
pub fn cancel_synchronization(k: &mut Kernel, handle: u32) -> u32 {
    let Some(t) = thread_of(k, handle, true) else {
        return result::INVALID_HANDLE;
    };
    if k.threads[t].state == ThreadState::Waiting
        && matches!(k.threads[t].wait, Some(Wait::Objects { .. }))
    {
        k.threads[t].wait_cancelled = false;
        end_wait(k, t, result::CANCELLED, Some(-1));
    } else {
        k.threads[t].wait_cancelled = true;
    }
    result::SUCCESS
}

const KERNEL_ADDRESS_BASE: u64 = 0xFFFF_FF80_0000_0000;
fn read_u32(memory: &Memory, address: u64) -> Option<u32> {
    memory.load(address, 4).ok().map(|v| v as u32)
}
fn write_u32(memory: &mut Memory, address: u64, value: u32) -> bool {
    memory.store(address, 4, u64::from(value)).is_ok()
}

/// Lock waiters of `owner` for `address`, best first: effective priority, then arrival.
fn lock_waiters(k: &Kernel, owner: usize, address: u64) -> Vec<usize> {
    let mut w: Vec<(u8, u64, usize)> = k
        .threads
        .iter()
        .enumerate()
        .filter_map(|(i, th)| match th.wait {
            Some(Wait::Lock {
                owner: o,
                address: a,
                order,
                ..
            }) if o == owner && a == address && th.state == ThreadState::Waiting => {
                Some((effective_priority(k, i), order, i))
            }
            _ => None,
        })
        .collect();
    w.sort();
    w.into_iter().map(|x| x.2).collect()
}
/// `KThread::RemoveWaiterByKey`: the best waiter for `address` becomes the next owner and
/// inherits the remaining waiters. Returns it and whether others still wait.
fn remove_waiter_by_key(k: &mut Kernel, owner: usize, address: u64) -> Option<(usize, bool)> {
    let waiters = lock_waiters(k, owner, address);
    let (&next, rest) = waiters.split_first()?;
    for &w in rest {
        if let Some(Wait::Lock { owner, .. }) = k.threads[w].wait.as_mut() {
            *owner = next;
        }
    }
    Some((next, !rest.is_empty()))
}
fn lock_tag(k: &Kernel, t: usize) -> u32 {
    match k.threads[t].wait {
        Some(Wait::Lock { tag, .. }) | Some(Wait::CondVar { tag, .. }) => tag,
        _ => 0,
    }
}

/// svcArbitrateLock (0x1A): W0 owner handle, X1 lock address, W2 tag of the caller.
pub fn arbitrate_lock(memory: &Memory, k: &mut Kernel, c: &mut Cpu) -> Result<Option<Stop>, u32> {
    let (handle, address, tag) = (c.x[0] as u32, c.x[1], c.x[2] as u32);
    if address >= KERNEL_ADDRESS_BASE {
        return Err(result::INVALID_CURRENT_MEMORY);
    }
    if address % 4 != 0 {
        return Err(result::INVALID_ADDRESS);
    }
    let value = read_u32(memory, address).ok_or(result::INVALID_CURRENT_MEMORY)?;
    if value != handle | HANDLE_WAIT_MASK {
        return Ok(None);
    }
    let owner = thread_of(k, handle, false).ok_or(result::INVALID_HANDLE)?;
    let order = next_order(k);
    Ok(block(
        k,
        c,
        0x1A,
        Wait::Lock {
            owner,
            address,
            tag,
            order,
            deadline: None,
        },
    ))
}

/// svcArbitrateUnlock (0x1B): X0 lock address, released by the running thread.
pub fn arbitrate_unlock(memory: &mut Memory, k: &mut Kernel, address: u64) -> u32 {
    if address >= KERNEL_ADDRESS_BASE {
        return result::INVALID_CURRENT_MEMORY;
    }
    if address % 4 != 0 {
        return result::INVALID_ADDRESS;
    }
    let next = remove_waiter_by_key(k, k.current, address);
    let value = next.map_or(0, |(n, more)| {
        lock_tag(k, n) | if more { HANDLE_WAIT_MASK } else { 0 }
    });
    let code = if write_u32(memory, address, value) {
        result::SUCCESS
    } else {
        result::INVALID_CURRENT_MEMORY
    };
    if let Some((n, _)) = next {
        end_wait(k, n, code, None);
    }
    code
}

/// svcWaitProcessWideKeyAtomic (0x1C): X0 lock address, X1 condition key, W2 tag,
/// X3 timeout. Releases the lock, then waits to be signalled and to reacquire it.
pub fn wait_condition(
    memory: &mut Memory,
    k: &mut Kernel,
    c: &mut Cpu,
) -> Result<Option<Stop>, u32> {
    let (address, key, tag, timeout) = (c.x[0], c.x[1] & !3, c.x[2] as u32, c.x[3] as i64);
    if address >= KERNEL_ADDRESS_BASE {
        return Err(result::INVALID_POINTER);
    }
    if address % 4 != 0 {
        return Err(result::INVALID_ADDRESS);
    }
    let deadline = if timeout > 0 {
        Some(deadline_after(ticks(now_ns(k, c)), timeout).unwrap_or(u64::MAX))
    } else {
        None
    };
    let next = remove_waiter_by_key(k, k.current, address);
    let value = next.map_or(0, |(n, more)| {
        lock_tag(k, n) | if more { HANDLE_WAIT_MASK } else { 0 }
    });
    if let Some((n, _)) = next {
        end_wait(k, n, result::SUCCESS, None);
    }
    let _ = write_u32(memory, key, 1);
    if !write_u32(memory, address, value) {
        return Err(result::INVALID_CURRENT_MEMORY);
    }
    if timeout == 0 {
        return Err(result::TIMED_OUT);
    }
    let order = next_order(k);
    Ok(block(
        k,
        c,
        0x1C,
        Wait::CondVar {
            key,
            address,
            tag,
            order,
            deadline,
        },
    ))
}

/// svcSignalProcessWideKey (0x1D): X0 condition key, W1 count (<= 0: all waiters).
pub fn signal_condition(memory: &mut Memory, k: &mut Kernel, key: u64, count: i32) {
    let key = key & !3;
    let mut waiters: Vec<(u8, u64, usize)> = k
        .threads
        .iter()
        .enumerate()
        .filter_map(|(i, th)| match th.wait {
            Some(Wait::CondVar { key: wk, order, .. })
                if wk == key && th.state == ThreadState::Waiting =>
            {
                Some((effective_priority(k, i), order, i))
            }
            _ => None,
        })
        .collect();
    waiters.sort();
    let take = if count <= 0 {
        waiters.len()
    } else {
        (count as usize).min(waiters.len())
    };
    for &(_, _, t) in &waiters[..take] {
        let Some(Wait::CondVar {
            address,
            tag,
            deadline,
            ..
        }) = k.threads[t].wait.clone()
        else {
            continue;
        };
        // KConditionVariable::SignalImpl: UpdateLockAtomic(if 0 -> tag, else |= wait mask).
        match read_u32(memory, address) {
            Some(prev) => {
                let new = if prev == 0 {
                    tag
                } else {
                    prev | HANDLE_WAIT_MASK
                };
                if !write_u32(memory, address, new) {
                    end_wait(k, t, result::INVALID_CURRENT_MEMORY, None);
                } else if prev == 0 {
                    end_wait(k, t, result::SUCCESS, None);
                } else {
                    match thread_of(k, prev & !HANDLE_WAIT_MASK, false) {
                        Some(owner) => {
                            let order = next_order(k);
                            k.threads[t].wait = Some(Wait::Lock {
                                owner,
                                address,
                                tag,
                                order,
                                deadline,
                            });
                        }
                        None => end_wait(k, t, result::INVALID_STATE, None),
                    }
                }
            }
            None => end_wait(k, t, result::INVALID_CURRENT_MEMORY, None),
        }
    }
    if take == waiters.len() {
        let _ = write_u32(memory, key, 0);
    }
}

// ---------------------------------------------------------------- scheduler

/// Wakes every waiter whose deadline has passed at `tick`.
fn expire(k: &mut Kernel, tick: u64) {
    let due: Vec<usize> = k
        .threads
        .iter()
        .enumerate()
        .filter(|(_, th)| {
            th.state == ThreadState::Waiting
                && th
                    .wait
                    .as_ref()
                    .and_then(Wait::deadline)
                    .is_some_and(|d| d <= tick)
        })
        .map(|(i, _)| i)
        .collect();
    for t in due {
        k.stats.timeouts += 1;
        match k.threads[t].wait {
            Some(Wait::Sleep { .. }) => end_wait(k, t, result::SUCCESS, None),
            Some(Wait::Objects { .. }) => end_wait(k, t, result::TIMED_OUT, Some(-1)),
            _ => end_wait(k, t, result::TIMED_OUT, None),
        }
    }
}

/// The preemption priority of a core (59 on cores 0-2, 63 on core 3).
fn preemption_priority(core: u8) -> u8 {
    if core == 3 { 63 } else { 59 }
}
/// Every 10 ms of virtual time, the first runnable thread at each core's preemption
/// priority moves behind the others of that priority (`RotateScheduledQueue`).
fn rotate(k: &mut Kernel) {
    while k.time_ns >= k.next_rotation_ns {
        k.next_rotation_ns += PREEMPTION_INTERVAL_NS;
        for core in 0..4u8 {
            let p = preemption_priority(core);
            let first = (0..k.threads.len())
                .filter(|&t| {
                    k.threads[t].state == ThreadState::Runnable
                        && k.threads[t].core == core
                        && effective_priority(k, t) == p
                })
                .min_by_key(|&t| k.threads[t].order);
            if let Some(t) = first {
                make_runnable(k, t);
                k.stats.rotations += 1;
            }
        }
    }
}

/// Chooses the next thread: virtual cores take turns; on each core the runnable thread with
/// the best effective priority runs, the earliest-queued among equals. Threads on the same
/// core are never time-sliced except by the preemption rotation.
fn pick(k: &mut Kernel) -> Option<usize> {
    let cores: Vec<u8> = (0..VIRTUAL_CORES as u8)
        .filter(|c| k.config.core_mask >> c & 1 != 0)
        .collect();
    let runnable = k
        .threads
        .iter()
        .filter(|t| t.state == ThreadState::Runnable)
        .count() as u64;
    k.stats.max_runnable = k.stats.max_runnable.max(runnable);
    for i in 0..cores.len() {
        let index = (k.next_core + i) % cores.len();
        let core = cores[index];
        let best = (0..k.threads.len())
            .filter(|&t| k.threads[t].state == ThreadState::Runnable && k.threads[t].core == core)
            .min_by_key(|&t| (effective_priority(k, t), k.threads[t].order));
        if let Some(t) = best {
            k.next_core = (index + 1) % cores.len();
            return Some(t);
        }
    }
    None
}

/// How a process run ended.
pub struct RunResult {
    pub result: RuntimeResult<Stop>,
    /// The thread that faulted or ended the process.
    pub thread: usize,
}

/// Runs the process's threads until the process exits, every thread exits, the
/// instruction budget is exhausted, a fault occurs, or every thread waits forever.
pub fn run(p: &mut Process, units: &[(u64, BlockTable)], limit: u64, quantum: u64) -> RunResult {
    let mut last: Option<usize> = None;
    loop {
        let Process { memory: _, kernel } = p;
        let k = kernel.as_mut().expect("Horizon process");
        expire(k, ticks(k.time_ns));
        rotate(k);
        let Some(t) = pick(k) else {
            if running_threads(k) == 0 {
                return RunResult {
                    result: Ok(Stop::ExitThread),
                    thread: k.current,
                };
            }
            let next = k
                .threads
                .iter()
                .filter(|th| th.state == ThreadState::Waiting)
                .filter_map(|th| th.wait.as_ref().and_then(Wait::deadline))
                .min();
            match next {
                Some(deadline) if deadline < u64::MAX => {
                    let at = ns_at_tick(deadline).max(k.time_ns);
                    k.stats.idle_ns += at - k.time_ns;
                    k.time_ns = at;
                    continue;
                }
                _ => {
                    let waiting: Vec<String> = k
                        .threads
                        .iter()
                        .filter(|th| th.state == ThreadState::Waiting)
                        .map(|th| {
                            let wait = th
                                .wait
                                .as_ref()
                                .map_or("nothing".to_string(), |w| w.to_string());
                            format!("thread {:#x} waits for {wait}", th.id)
                        })
                        .collect();
                    return RunResult {
                        result: Err(format!(
                            "guest deadlock: every thread waits without a timeout ({})",
                            waiting.join("; ")
                        )),
                        thread: k.current,
                    };
                }
            }
        };
        if last != Some(t) {
            k.stats.context_switches += u64::from(last.is_some());
            last = Some(t);
        }
        k.stats.slices += 1;
        k.current = t;
        let mut cpu = k.threads[t]
            .context
            .take()
            .expect("runnable thread has a context");
        k.slice_start = cpu.steps;
        cpu.limit = cpu.steps + limit.saturating_sub(k.total_steps);
        let slice_end = cpu.steps.saturating_add(quantum);
        let result = execute_until(&mut cpu, p, units, slice_end);
        let k = p.kernel.as_mut().expect("Horizon process");
        let ran = cpu.steps - k.slice_start;
        k.total_steps += ran;
        k.time_ns += ran;
        // A context switch (an exception on hardware) ends any exclusive reservation.
        cpu.monitor = None;
        k.threads[t].context = Some(cpu);
        match result {
            Ok(Stop::Reschedule) => {}
            Ok(stop) => {
                return RunResult {
                    result: Ok(stop),
                    thread: t,
                };
            }
            Err(e) => {
                return RunResult {
                    result: Err(e),
                    thread: t,
                };
            }
        }
    }
}
