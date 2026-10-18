//! Horizon process environment: address-space layout, main thread, TLS, handles and SVCs.
//!
//! Behaviour follows the Horizon kernel as reimplemented by Atmosphère's Mesosphere
//! (`libmesosphere`, commit 0e9cd964) and the user-mode contract used by libnx (commit
//! feebd026). Every SVC either produces its architectural effect (registers, guest memory,
//! kernel state) or fails explicitly; none returns success without performing its work.
use crate::ipc;
use crate::threads::{self, SchedulerStats, ThreadState, Wait};
use crate::{Cpu, Memory, RuntimeResult, Stop};
use std::collections::BTreeMap;

pub const PAGE: u64 = 0x1000;
/// Region alignment of the process page table (`KPageTableBase::RegionAlignment`).
pub const REGION_ALIGNMENT: u64 = 0x20_0000;
/// `svcSetHeapSize` granularity (`HeapSizeAlignment`).
pub const HEAP_ALIGNMENT: u64 = 0x20_0000;
/// Guard pages around user mappings found by `FindFreeArea` (`GetNumGuardPages`, user = 4).
pub const GUARD_PAGES: u64 = 4;
pub const TLS_REGION_SIZE: u64 = 0x200;
/// Offset of `ThreadLocalRegion::thread_handle`.
pub const TLS_THREAD_HANDLE: u64 = 0x110;
pub const CURRENT_THREAD: u32 = 0xFFFF_8000;
pub const CURRENT_PROCESS: u32 = 0xFFFF_8001;
pub const INVALID_HANDLE: u32 = 0;

/// `svc::MemoryState` values.
pub mod state {
    pub const FREE: u32 = 0x00;
    pub const STATIC: u32 = 0x02;
    pub const CODE: u32 = 0x03;
    pub const CODE_DATA: u32 = 0x04;
    pub const NORMAL: u32 = 0x05;
    pub const SHARED_MEMORY: u32 = 0x0D;
    pub const STACK: u32 = 0x0B;
    pub const THREAD_LOCAL: u32 = 0x0C;
    pub const INACCESSIBLE: u32 = 0x10;

    /// `KMemoryState` flag: the memory may be aliased by `svcMapMemory`.
    pub const FLAG_CAN_ALIAS: u32 = 1 << 15;
    /// `KMemoryState` flag: `svcSetMemoryPermission` may change the permission.
    pub const FLAG_CAN_REPROTECT: u32 = 1 << 8;
    /// `KMemoryState` flag: the Uncached attribute may change.
    pub const FLAG_CAN_CHANGE_ATTRIBUTE: u32 = 1 << 24;
    /// `KMemoryState` flag: the PermissionLocked attribute may be set.
    pub const FLAG_CAN_PERMISSION_LOCK: u32 = 1 << 27;
    /// The kernel's `KMemoryState` (svc state | capability flags) for the states this runtime
    /// maps; values are Mesosphere's `static_assert`ed constants.
    pub const fn kernel(state: u32) -> u32 {
        match state {
            STATIC => 0x0004_0002,
            CODE => 0x04DC_7E03,
            CODE_DATA => 0x0FFE_BD04,
            NORMAL => 0x077E_BD05,
            STACK => 0x045C_3C0B,
            THREAD_LOCAL => 0x0400_000C,
            SHARED_MEMORY => 0x0400_000D,
            other => other,
        }
    }
}
/// `svc::MemoryAttribute` bits.
pub mod attribute {
    /// Source pages of an `svcMapMemory` alias.
    pub const LOCKED: u32 = 1 << 0;
    pub const DEVICE_SHARED: u32 = 1 << 2;
    pub const UNCACHED: u32 = 1 << 3;
    pub const PERMISSION_LOCKED: u32 = 1 << 4;
    /// Attributes user mode may set (`KMemoryAttribute_SetMask`).
    pub const SET_MASK: u32 = UNCACHED | PERMISSION_LOCKED;
}
/// Kernel result codes: module 1, `description << 9`.
pub mod result {
    const fn kernel(description: u32) -> u32 {
        1 | (description << 9)
    }
    pub const SUCCESS: u32 = 0;
    pub const INVALID_SIZE: u32 = kernel(101);
    pub const INVALID_ADDRESS: u32 = kernel(102);
    pub const OUT_OF_MEMORY: u32 = kernel(104);
    pub const INVALID_CURRENT_MEMORY: u32 = kernel(106);
    pub const INVALID_NEW_MEMORY_PERMISSION: u32 = kernel(108);
    pub const INVALID_HANDLE: u32 = kernel(114);
    pub const INVALID_POINTER: u32 = kernel(115);
    pub const INVALID_COMBINATION: u32 = kernel(116);
    pub const INVALID_ENUM_VALUE: u32 = kernel(120);
    pub const OUT_OF_HANDLES: u32 = kernel(105);
    pub const INVALID_MEMORY_REGION: u32 = kernel(110);
    pub const INVALID_PRIORITY: u32 = kernel(112);
    pub const INVALID_CORE_ID: u32 = kernel(113);
    pub const TIMED_OUT: u32 = kernel(117);
    pub const CANCELLED: u32 = kernel(118);
    pub const OUT_OF_RANGE: u32 = kernel(119);
    pub const INVALID_STATE: u32 = kernel(125);
    pub const LIMIT_REACHED: u32 = kernel(132);
    pub const NOT_FOUND: u32 = kernel(121);
    pub const SESSION_CLOSED: u32 = kernel(123);
    pub const PORT_CLOSED: u32 = kernel(131);
    pub const MESSAGE_TOO_LARGE: u32 = kernel(260);
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Range {
    pub start: u64,
    pub end: u64,
}
impl Range {
    pub fn size(self) -> u64 {
        self.end - self.start
    }
}

/// The address-space regions of a 39-bit process.
#[derive(Clone, Copy, Debug)]
pub struct Layout {
    pub address_space_end: u64,
    /// The "huge map" code region; reported as the ASLR region.
    pub code_region: Range,
    pub kernel_map: Range,
    pub stack: Range,
    pub alias: Range,
    pub heap: Range,
}
impl Layout {
    /// `KPageTableBase::InitializeForProcess` for a 39-bit address space with ASLR disabled:
    /// the KernelMap (64 GiB), Stack (2 GiB), Alias (64 GiB) and Heap (8 GiB) regions are laid
    /// out largest-first on whichever side of the 2 MiB-aligned code has more space, then
    /// ordered by region type and packed without random offsets.
    pub fn new_39bit(code_start: u64, code_size: u64) -> RuntimeResult<Self> {
        const GIB: u64 = 1 << 30;
        let address_space_end = 1u64 << 39;
        let code_region = Range {
            start: 0x800_0000,
            end: address_space_end,
        };
        let code_end = code_start
            .checked_add(code_size)
            .ok_or("code range overflow")?;
        if code_start < code_region.start || code_end > code_region.end || code_size == 0 {
            return Err("process code lies outside the 39-bit code region".into());
        }
        let process_code_start = code_start / REGION_ALIGNMENT * REGION_ALIGNMENT;
        let process_code_end = code_end.div_ceil(REGION_ALIGNMENT) * REGION_ALIGNMENT;
        // (size, type) with type order KernelMap 0, Stack 1, Alias 2, Heap 3.
        let mut regions = [
            (64 * GIB, 0usize),
            (2 * GIB, 1),
            (64 * GIB, 2),
            (8 * GIB, 3),
        ];
        // Selection sort by size, largest first (equal sizes keep their relative order).
        for i in 0..regions.len() - 1 {
            for j in i + 1..regions.len() {
                if regions[i].0 < regions[j].0 {
                    regions.swap(i, j);
                }
            }
        }
        let mut remaining = [
            process_code_start - code_region.start,
            code_region.end - process_code_end,
        ];
        let starts = [code_region.start, process_code_end];
        let mut placed: [Vec<(u64, usize)>; 2] = [Vec::new(), Vec::new()];
        for (size, kind) in regions {
            let side = usize::from(remaining[1] >= remaining[0]);
            if remaining[side] < size {
                return Err("address space too small for the process regions".into());
            }
            remaining[side] -= size;
            placed[side].push((size, kind));
        }
        let mut out = [Range { start: 0, end: 0 }; 4];
        for (side, list) in placed.iter_mut().enumerate() {
            list.sort_by_key(|r| r.1);
            let mut cursor = starts[side];
            for (size, kind) in list.iter() {
                out[*kind] = Range {
                    start: cursor,
                    end: cursor + size,
                };
                cursor += size;
            }
        }
        Ok(Self {
            address_space_end,
            code_region,
            kernel_map: out[0],
            stack: out[1],
            alias: out[2],
            heap: out[3],
        })
    }
}

/// Process creation parameters (NPDM-derived or explicit).
#[derive(Clone, Copy, Debug)]
pub struct ProcessConfig {
    pub program_id: u64,
    pub process_id: u64,
    pub main_thread_id: u64,
    pub main_thread_stack_size: u64,
    pub main_thread_priority: u8,
    pub ideal_core: u8,
    pub core_mask: u64,
    pub priority_mask: u64,
    /// Total user physical memory the process may use (`InfoType_TotalMemorySize`).
    pub memory_limit: u64,
    pub handle_table_size: u16,
    pub thread_limit: u64,
    pub is_application: bool,
    /// SVC access flags from the kernel capabilities: bit `id` permits SVC `id` (0..0xC0).
    pub svc_permissions: [u64; 3],
    /// ACI0 service access control: (may host, service name or prefix ending in `*`).
    pub service_access: &'static [(bool, &'static str)],
}
impl ProcessConfig {
    pub fn permits(&self, id: u16) -> bool {
        id < 0xC0 && self.svc_permissions[usize::from(id / 64)] >> (id % 64) & 1 != 0
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Object {
    Thread(usize),
    /// Client session (`ipc::Session`).
    Session(usize),
    /// Server port of a service registered with sm (`ipc::Port`).
    Port(usize),
    /// The readable and writable halves of a KEvent (`threads::Event`).
    ReadableEvent(usize),
    WritableEvent(usize),
    /// Shared memory block (`SharedMemory`).
    SharedMemory(usize),
    /// Transfer memory block (`TransferMemory`).
    TransferMemory(usize),
}
/// Handle table with the Mesosphere encoding: `index | linear_id << 15`; linear ids start at
/// 1 and free indices are handed out from the top of the table down.
#[derive(Debug)]
pub struct HandleTable {
    entries: Vec<Option<(u16, Object)>>,
    free: Vec<u16>,
    next_linear_id: u16,
}
impl HandleTable {
    /// A table of `size` entries; 0 selects the maximum, 1024 (`KHandleTable::Initialize`).
    pub fn new(size: u16) -> Self {
        let size = if size == 0 { 1024 } else { size.min(1024) };
        Self {
            entries: vec![None; usize::from(size)],
            free: (0..size).collect(),
            next_linear_id: 1,
        }
    }
    pub fn add(&mut self, object: Object) -> Result<u32, u32> {
        let index = self.free.pop().ok_or(result::OUT_OF_HANDLES)?;
        let linear = self.next_linear_id;
        self.next_linear_id = if linear >= 0x7fff { 1 } else { linear + 1 };
        self.entries[usize::from(index)] = Some((linear, object));
        Ok(u32::from(index) | u32::from(linear) << 15)
    }
    pub fn get(&self, handle: u32) -> Option<Object> {
        if handle >> 30 != 0 {
            return None;
        }
        let (index, linear) = ((handle & 0x7fff) as usize, ((handle >> 15) & 0x7fff) as u16);
        match self.entries.get(index)? {
            Some((l, object)) if *l == linear && linear != 0 => Some(*object),
            _ => None,
        }
    }
    pub fn close(&mut self, handle: u32) -> bool {
        if self.get(handle).is_none() {
            return false;
        }
        let index = (handle & 0x7fff) as u16;
        self.entries[usize::from(index)] = None;
        self.free.push(index);
        true
    }
}

/// A kernel thread object. Its index in `Kernel::threads` is its object identity; indices
/// and thread IDs are never reused, so a stale handle cannot name a new thread.
#[derive(Clone, Debug)]
pub struct Thread {
    pub id: u64,
    /// The handle returned when the thread was created (informational).
    pub handle: u32,
    /// Thread-local region (TPIDRRO_EL0).
    pub tls: u64,
    /// Main thread: its stack mapping. Created threads: the stack top they were given.
    pub stack: Range,
    /// Base priority (0 highest, 63 lowest).
    pub priority: u8,
    /// The core the thread runs on: its ideal core, or the highest core of its affinity
    /// mask when the ideal core is −1. Threads do not migrate otherwise.
    pub core: u8,
    /// svcSetThreadCoreMask state: the ideal core (−1: no preference) and affinity mask.
    pub ideal_core: i32,
    pub affinity_mask: u64,
    pub state: ThreadState,
    pub wait: Option<Wait>,
    /// The SVC to complete when the current wait ends.
    pub pending_svc: u16,
    /// Saved architectural state while the thread is not running.
    pub context: Option<Box<Cpu>>,
    /// Open handles; an exited thread without handles is destroyed.
    pub references: u32,
    /// svcCancelSynchronization arrived while the thread was not in a cancellable wait.
    pub wait_cancelled: bool,
    pub destroyed: bool,
    /// Queue position among runnable threads of equal priority on its core.
    pub order: u64,
}
impl Thread {
    pub fn new(id: u64, tls: u64, stack: Range, priority: u8, core: u8) -> Self {
        Self {
            id,
            handle: 0,
            tls,
            stack,
            priority,
            core,
            ideal_core: i32::from(core),
            affinity_mask: 1 << core,
            state: ThreadState::Initialized,
            wait: None,
            pending_svc: 0,
            context: None,
            references: 0,
            wait_cancelled: false,
            destroyed: false,
            order: 0,
        }
    }
}

/// Why execution of the process ended.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Exit {
    Process,
    Thread,
}

/// Horizon KSharedMemory kernel object.
#[derive(Clone, Debug)]
pub struct SharedMemory {
    pub size: u64,
    pub permission: u32,
    pub bytes: Vec<u8>,
}

/// Horizon KTransferMemory kernel object.
#[derive(Clone, Debug)]
pub struct TransferMemory {
    pub address: u64,
    pub size: u64,
    pub permission: u32,
}

#[derive(Debug)]
pub struct Kernel {
    pub config: ProcessConfig,
    pub layout: Layout,
    pub code: Range,
    pub handles: HandleTable,
    pub threads: Vec<Thread>,
    /// The running thread.
    pub current: usize,
    pub next_thread_id: u64,
    /// ThreadLocal pages: (address, bitmap of reserved 0x200-byte regions).
    pub tls_pages: Vec<(u64, u8)>,
    /// svcMapMemory stack aliases.
    pub aliases: Vec<Alias>,
    pub shared_memories: Vec<SharedMemory>,
    pub transfer_memories: Vec<TransferMemory>,
    /// Deterministic virtual clock in nanoseconds: one per retired guest instruction plus
    /// time skipped while every thread waits for a timeout.
    pub time_ns: u64,
    /// `Cpu::steps` of the running thread when its slice began.
    pub slice_start: u64,
    /// Guest instructions retired by all threads.
    pub total_steps: u64,
    pub next_core: usize,
    pub next_rotation_ns: u64,
    pub order: u64,
    pub stats: SchedulerStats,
    pub sessions: Vec<ipc::Session>,
    pub events: Vec<threads::Event>,
    pub ports: Vec<ipc::Port>,
    pub services: Vec<ipc::Service>,
    /// IPC replies to write into other threads' message buffers.
    pub pending_replies: Vec<(u64, Vec<u8>)>,
    pub ipc: ipc::IpcStats,
    /// State of the services hosted by the runtime.
    pub host: crate::services::HostState,
    /// Process-owned NVIDIA host; never cloned with service/domain state.
    pub nvidia: crate::nvidia::State,
    /// Process-local region (TLS slot 0 of the first ThreadLocal page).
    pub process_local_region: u64,
    pub heap_size: u64,
    pub svc_counts: BTreeMap<u16, u64>,
    pub last_svc: Option<(u64, u16, Option<u32>)>,
    pub debug_output: Vec<String>,
    pub exit: Option<Exit>,
}

impl Kernel {
    pub fn create_shared_memory(
        &mut self,
        size: u64,
        permission: u32,
        bytes: Vec<u8>,
    ) -> Result<u32, u32> {
        self.shared_memories.push(SharedMemory {
            size,
            permission,
            bytes,
        });
        let index = self.shared_memories.len() - 1;
        self.handles.add(Object::SharedMemory(index))
    }

    pub fn create_transfer_memory(
        &mut self,
        address: u64,
        size: u64,
        permission: u32,
    ) -> Result<u32, u32> {
        self.transfer_memories.push(TransferMemory {
            address,
            size,
            permission,
        });
        let index = self.transfer_memories.len() - 1;
        self.handles.add(Object::TransferMemory(index))
    }
}

/// The guest process: one memory image and its kernel state, shared by every translation
/// unit. Generated code accesses memory through `Deref`.
#[derive(Debug)]
pub struct Process {
    pub memory: Memory,
    pub kernel: Option<Kernel>,
}
impl std::ops::Deref for Process {
    type Target = Memory;
    fn deref(&self) -> &Memory {
        &self.memory
    }
}
impl std::ops::DerefMut for Process {
    fn deref_mut(&mut self) -> &mut Memory {
        &mut self.memory
    }
}

/// Zero-filled guest memory. `vec![0; n]` allocates through the zeroing allocator (calloc /
/// HeapAlloc with zeroing), so untouched pages are not committed or written: a large heap
/// costs only the pages the guest uses. Callers bound `size` by the process memory limit.
fn zeroed(size: u64) -> Vec<u8> {
    vec![0; size as usize]
}
/// Finds a page-aligned free range of `size` bytes in `region`, leaving `GUARD_PAGES` before
/// it, searching upward (deterministic first fit; Horizon randomizes with ASLR).
pub(crate) fn find_free(memory: &Memory, region: Range, size: u64) -> RuntimeResult<u64> {
    let mut candidate = region.start + GUARD_PAGES * PAGE;
    let mut regions: Vec<(u64, u64)> = memory
        .regions
        .iter()
        .map(|r| (r.base, r.base + r.bytes.len() as u64))
        .collect();
    regions.sort();
    for (start, end) in regions {
        if end + GUARD_PAGES * PAGE <= candidate || start >= candidate + size + GUARD_PAGES * PAGE {
            continue;
        }
        candidate = candidate.max(end + GUARD_PAGES * PAGE);
    }
    if candidate + size + GUARD_PAGES * PAGE > region.end {
        return Err("no free space for the requested mapping".into());
    }
    Ok(candidate)
}

impl Process {
    /// Process bootstrap (`KProcess::Run`): reserves the process-local region and the main
    /// thread's TLS in a ThreadLocal page of the KernelMap region, maps the main stack in
    /// the Stack region, registers the main thread handle (also stored at TLS+0x110), and
    /// returns the initial CPU state: PC = entry, X0 = 0, X1 = thread handle, SP = stack top,
    /// TPIDRRO_EL0 = TLS, every other register zero.
    pub fn horizon(
        mut memory: Memory,
        entry: u64,
        code: Range,
        config: ProcessConfig,
    ) -> RuntimeResult<(Self, Cpu)> {
        let layout = Layout::new_39bit(code.start, code.size())?;
        if config.main_thread_stack_size % PAGE != 0 || config.main_thread_stack_size == 0 {
            return Err("main thread stack size must be a non-zero multiple of 0x1000".into());
        }
        let stack_size = config.main_thread_stack_size;
        let stack_base = find_free(&memory, layout.stack, stack_size)?;
        memory.map_state(
            stack_base,
            zeroed(stack_size),
            true,
            true,
            false,
            state::STACK,
        )?;
        let mut kernel = Kernel {
            config,
            layout,
            code,
            handles: HandleTable::new(config.handle_table_size),
            threads: Vec::new(),
            current: 0,
            next_thread_id: config.main_thread_id + 1,
            tls_pages: Vec::new(),
            aliases: Vec::new(),
            shared_memories: Vec::new(),
            transfer_memories: Vec::new(),
            time_ns: 0,
            slice_start: 0,
            total_steps: 0,
            next_core: 0,
            next_rotation_ns: threads::PREEMPTION_INTERVAL_NS,
            order: 0,
            stats: SchedulerStats::default(),
            sessions: Vec::new(),
            events: Vec::new(),
            ports: Vec::new(),
            services: Vec::new(),
            pending_replies: Vec::new(),
            ipc: ipc::IpcStats::default(),
            host: crate::services::HostState::default(),
            nvidia: crate::nvidia::State::default(),
            process_local_region: 0,
            heap_size: 0,
            svc_counts: BTreeMap::new(),
            last_svc: None,
            debug_output: Vec::new(),
            exit: None,
        };
        let k = &mut kernel;
        // The process-local region takes the first slot of the first ThreadLocal page.
        k.process_local_region =
            threads::reserve_tls(&mut memory, k).map_err(|_| "no space for TLS")?;
        let tls = threads::reserve_tls(&mut memory, k).map_err(|_| "no space for TLS")?;
        let stack = Range {
            start: stack_base,
            end: stack_base + stack_size,
        };
        k.threads.push(Thread::new(
            config.main_thread_id,
            tls,
            stack,
            config.main_thread_priority,
            config.ideal_core,
        ));
        k.threads[0].state = ThreadState::Runnable;
        let handle = threads::open_handle(k, 0).map_err(|_| "handle table full")?;
        k.threads[0].handle = handle;
        memory.write_bytes(tls + TLS_THREAD_HANDLE, &handle.to_le_bytes())?;
        let mut cpu = Cpu::new(entry);
        cpu.x = [0; 31];
        cpu.x[1] = u64::from(handle);
        cpu.x[18] = threads::cfi_value(config.program_id, config.main_thread_id);
        cpu.sp = stack.end;
        cpu.tpidrro_el0 = tls;
        Ok((
            Self {
                memory,
                kernel: Some(kernel),
            },
            cpu,
        ))
    }
}

/// Homebrew ABI configuration keys (libnx `runtime/env.c`).
pub mod homebrew {
    pub const END_OF_LIST: u32 = 0;
    pub const MAIN_THREAD_HANDLE: u32 = 1;
    pub const SYSCALL_AVAILABLE_HINT: u32 = 6;
    pub const SYSCALL_AVAILABLE_HINT2: u32 = 17;
    /// `EntryFlag_IsMandatory`.
    pub const MANDATORY: u32 = 1;
    /// Size of one ConfigEntry (u32 key, u32 flags, u64 values[2]).
    pub const ENTRY_SIZE: usize = 24;
}

impl Process {
    /// Switches the main thread to the homebrew (NRO) entry convention: X0 = a ConfigEntry
    /// list (MainThreadHandle, mandatory; syscall-available hints from the SVC access flags;
    /// EndOfList with no loader information), X1 = -1, X30 = the loader return address, which
    /// is `RETURN_PC`: `__nx_exit` branches there with the result in X0 and the run ends with
    /// `status=returned`. The list lives in one Normal RW page of the KernelMap region that
    /// this runtime maps for the purpose (a homebrew loader keeps it in its own memory).
    pub fn homebrew_entry(&mut self, cpu: &mut Cpu) -> RuntimeResult<u64> {
        let Process { memory, kernel } = self;
        let k = kernel.as_mut().ok_or("not a Horizon process")?;
        let page = find_free(memory, k.layout.kernel_map, PAGE)?;
        memory.map_state(
            page,
            vec![0; PAGE as usize],
            true,
            true,
            false,
            state::NORMAL,
        )?;
        let p = k.config.svc_permissions;
        let entries: [(u32, u32, u64, u64); 4] = [
            (
                homebrew::MAIN_THREAD_HANDLE,
                homebrew::MANDATORY,
                u64::from(k.threads[0].handle),
                0,
            ),
            (homebrew::SYSCALL_AVAILABLE_HINT, 0, p[0], p[1]),
            (homebrew::SYSCALL_AVAILABLE_HINT2, 0, p[2], 0),
            (homebrew::END_OF_LIST, 0, 0, 0),
        ];
        for (i, (key, flags, v0, v1)) in entries.into_iter().enumerate() {
            let mut e = [0u8; homebrew::ENTRY_SIZE];
            e[0..4].copy_from_slice(&key.to_le_bytes());
            e[4..8].copy_from_slice(&flags.to_le_bytes());
            e[8..16].copy_from_slice(&v0.to_le_bytes());
            e[16..24].copy_from_slice(&v1.to_le_bytes());
            memory.write_bytes(page + (i * homebrew::ENTRY_SIZE) as u64, &e)?;
        }
        cpu.x[0] = page;
        cpu.x[1] = u64::MAX;
        cpu.x[30] = crate::RETURN_PC;
        Ok(page)
    }
}

/// One `svc::MemoryInfo` block.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct MemoryInfo {
    pub base: u64,
    pub size: u64,
    pub state: u32,
    pub attribute: u32,
    pub permission: u32,
}
impl MemoryInfo {
    /// The 0x28-byte `svc::lp64::MemoryInfo` image (ipc/device counts and padding zero).
    pub fn to_bytes(self) -> [u8; 0x28] {
        let mut b = [0u8; 0x28];
        b[0..8].copy_from_slice(&self.base.to_le_bytes());
        b[8..16].copy_from_slice(&self.size.to_le_bytes());
        b[16..20].copy_from_slice(&self.state.to_le_bytes());
        b[20..24].copy_from_slice(&self.attribute.to_le_bytes());
        b[24..28].copy_from_slice(&self.permission.to_le_bytes());
        b
    }
}
/// `KPageTableBase::QueryInfo`: the maximal block of identical state and permission that
/// contains `address`; Free between mappings; a synthetic Inaccessible block from the end of
/// the address space to 2^64 for addresses outside it.
pub fn query(memory: &Memory, address_space_end: u64, address: u64) -> MemoryInfo {
    if address >= address_space_end {
        return MemoryInfo {
            base: address_space_end,
            size: 0u64.wrapping_sub(address_space_end),
            state: state::INACCESSIBLE,
            attribute: 0,
            permission: 0,
        };
    }
    let mut regions: Vec<(u64, u64, u32, u32, u32)> = memory
        .regions
        .iter()
        .filter(|r| !r.bytes.is_empty())
        .map(|r| (r.base, r.end(), r.state, permission(r), r.attribute))
        .collect();
    regions.sort();
    let Some(i) = regions.iter().position(|r| r.0 <= address && address < r.1) else {
        let start = regions
            .iter()
            .filter(|r| r.1 <= address)
            .map(|r| r.1)
            .max()
            .unwrap_or(0);
        let end = regions
            .iter()
            .filter(|r| r.0 > address)
            .map(|r| r.0)
            .min()
            .unwrap_or(address_space_end);
        return MemoryInfo {
            base: start,
            size: end - start,
            state: state::FREE,
            attribute: 0,
            permission: 0,
        };
    };
    let (mut start, mut end, st, perm, attr) = regions[i];
    let same = |r: &(u64, u64, u32, u32, u32)| r.2 == st && r.3 == perm && r.4 == attr;
    let mut j = i;
    while j > 0 && regions[j - 1].1 == start && same(&regions[j - 1]) {
        j -= 1;
        start = regions[j].0;
    }
    let mut k = i;
    while k + 1 < regions.len() && regions[k + 1].0 == end && same(&regions[k + 1]) {
        k += 1;
        end = regions[k].1;
    }
    MemoryInfo {
        base: start,
        size: end - start,
        state: st,
        attribute: attr,
        permission: perm,
    }
}
/// `svc::MemoryPermission` bits of a region (Read 1, Write 2, Execute 4).
pub fn permission(r: &crate::Region) -> u32 {
    u32::from(r.read) | u32::from(r.write) << 1 | u32::from(r.execute) << 2
}

/// `KPageTableBase::CheckMemoryState` over `[address, address + size)`: every page must be
/// mapped, and all blocks must share state, permission and attribute (ignoring
/// `ignore_attribute`); the kernel state must satisfy `state_mask`/`state_value` and the
/// attribute `attribute_mask`/0. Returns the indices of the regions covering the range, in
/// address order, with the common state, permission and attribute.
fn check_memory_state(
    memory: &Memory,
    address: u64,
    size: u64,
    state_mask: u32,
    state_value: u32,
    attribute_mask: u32,
    ignore_attribute: u32,
) -> Result<(Vec<usize>, u32, u32, u32), u32> {
    let end = address + size;
    let mut covering: Vec<usize> = (0..memory.regions.len())
        .filter(|&i| {
            let r = &memory.regions[i];
            r.base < end && address < r.end()
        })
        .collect();
    covering.sort_by_key(|&i| memory.regions[i].base);
    let first = covering
        .first()
        .map(|&i| &memory.regions[i])
        .ok_or(result::INVALID_CURRENT_MEMORY)?;
    if first.base > address {
        return Err(result::INVALID_CURRENT_MEMORY);
    }
    let (st, perm, attr) = (first.state, permission(first), first.attribute);
    let mut cursor = first.base;
    for &i in &covering {
        let r = &memory.regions[i];
        if r.base != cursor
            || r.state != st
            || permission(r) != perm
            || (r.attribute | ignore_attribute) != (attr | ignore_attribute)
            || state::kernel(r.state) & state_mask != state_value
            || r.attribute & attribute_mask != 0
        {
            return Err(result::INVALID_CURRENT_MEMORY);
        }
        cursor = r.end();
    }
    if cursor < end {
        return Err(result::INVALID_CURRENT_MEMORY);
    }
    Ok((covering, st, perm, attr & !ignore_attribute))
}
/// Splits regions so that `[address, address + size)` is exactly covered by whole regions and
/// returns their indices.
fn isolate(memory: &mut Memory, address: u64, size: u64) -> Vec<usize> {
    memory.split_at(address);
    memory.split_at(address + size);
    (0..memory.regions.len())
        .filter(|&i| {
            let r = &memory.regions[i];
            r.base >= address && r.end() <= address + size
        })
        .collect()
}
/// Shared argument validation of the memory-attribute SVCs: page-aligned, non-empty,
/// non-wrapping and inside the address space.
fn check_range(k: &Kernel, address: u64, size: u64) -> Result<(), u32> {
    if address % PAGE != 0 {
        return Err(result::INVALID_ADDRESS);
    }
    if size % PAGE != 0 || size == 0 {
        return Err(result::INVALID_SIZE);
    }
    let end = address
        .checked_add(size)
        .ok_or(result::INVALID_CURRENT_MEMORY)?;
    if end > k.layout.address_space_end {
        return Err(result::INVALID_CURRENT_MEMORY);
    }
    Ok(())
}
/// `svcSetMemoryPermission`: only None, Read and ReadWrite may be requested, on memory whose
/// state allows reprotection and that carries no attribute.
fn set_memory_permission(
    memory: &mut Memory,
    k: &Kernel,
    address: u64,
    size: u64,
    perm: u64,
) -> Result<(), u32> {
    check_range(k, address, size)?;
    if !matches!(perm, 0 | 1 | 3) {
        return Err(result::INVALID_NEW_MEMORY_PERMISSION);
    }
    let (_, _, old, _) = check_memory_state(
        memory,
        address,
        size,
        state::FLAG_CAN_REPROTECT,
        state::FLAG_CAN_REPROTECT,
        0xFF,
        0,
    )?;
    if u64::from(old) == perm {
        return Ok(());
    }
    for i in isolate(memory, address, size) {
        let r = &mut memory.regions[i];
        r.read = perm & 1 != 0;
        r.write = perm & 2 != 0;
        r.execute = false;
    }
    Ok(())
}
/// `svcSetMemoryAttribute`: changes the Uncached and/or PermissionLocked bits selected by
/// `mask`. PermissionLocked can only be set, never cleared.
fn set_memory_attribute(
    memory: &mut Memory,
    k: &Kernel,
    address: u64,
    size: u64,
    mask: u32,
    value: u32,
) -> Result<(), u32> {
    check_range(k, address, size)?;
    if mask | value != mask || (mask | value) & !attribute::SET_MASK != 0 {
        return Err(result::INVALID_COMBINATION);
    }
    if mask & attribute::PERMISSION_LOCKED != value & attribute::PERMISSION_LOCKED {
        return Err(result::INVALID_COMBINATION);
    }
    let mut state_test = 0;
    if mask & attribute::UNCACHED != 0 {
        state_test |= state::FLAG_CAN_CHANGE_ATTRIBUTE;
    }
    if mask & attribute::PERMISSION_LOCKED != 0 {
        state_test |= state::FLAG_CAN_PERMISSION_LOCK;
    }
    let ignore = attribute::SET_MASK | attribute::DEVICE_SHARED;
    check_memory_state(
        memory,
        address,
        size,
        state_test,
        state_test,
        !ignore & 0xFF,
        ignore,
    )?;
    for i in isolate(memory, address, size) {
        let r = &mut memory.regions[i];
        r.attribute = (r.attribute & !mask) | (value & mask);
    }
    Ok(())
}

/// Bytes of user memory charged to the process (code, stack, heap and TLS pages). The
/// Locked source of an svcMapMemory alias shares its pages with the alias and is not
/// counted twice.
pub fn used_memory(memory: &Memory) -> u64 {
    memory
        .regions
        .iter()
        .filter(|r| {
            matches!(
                r.state,
                state::CODE | state::CODE_DATA | state::STACK | state::NORMAL | state::THREAD_LOCAL
            ) && r.attribute & attribute::LOCKED == 0
        })
        .map(|r| r.bytes.len() as u64)
        .sum()
}

/// Output registers of an SVC: whether X0 carries a value (a result code or, for
/// GetCurrentProcessorNumber/GetSystemTick, the return value) and how many outputs follow in
/// X1 upward.
fn outputs(id: u16) -> (bool, usize) {
    match id {
        0x01 | 0x06 | 0x08 | 0x0C | 0x15 | 0x18 | 0x1F | 0x24 | 0x25 | 0x29 => (true, 1),
        0x0E | 0x45 => (true, 2),
        0x0B | 0x1D | 0x36 => (false, 0),
        _ => (true, 0),
    }
}
/// The register contract of Mesosphere's SVC wrappers (`svc_codegen`, 64-bit ABI): outputs
/// are copied from kernel stack slots after every call, successful or not, and each of
/// X0-X7 that is not an output is zeroed; X8-X30 are preserved. A failed call normally
/// never writes its output slots; this runtime returns 0 in them deterministically.
/// `written` marks the exception: the output was produced despite the failure
/// (svcWaitSynchronization reports index -1 after a timed-out or cancelled wait).
pub(crate) fn finish_registers(c: &mut Cpu, id: u16, written: bool) {
    let (value, count) = outputs(id);
    let failed = value && c.x[0] as u32 != result::SUCCESS && !matches!(id, 0x10 | 0x1E);
    for n in 0..8 {
        let output = (n == 0 && value) || (1..=count).contains(&n);
        if !output || (failed && n != 0 && !written) {
            c.x[n] = 0;
        }
    }
}
fn set_result(c: &mut Cpu, value: u32) {
    if value != 0 {
        eprintln!(
            "[svc] error result: {value:#x} (desc={}, mod={}) pc={:#x} lr={:#x} x0={:#x} x1={:#x} x2={:#x}",
            (value >> 9) & 0x1fff,
            value & 0x1ff,
            c.pc,
            c.x[30],
            c.x[0],
            c.x[1],
            c.x[2],
        );
    }
    c.x[0] = u64::from(value);
}

/// Horizon SVC dispatch for the running thread `k.current`, whose context is `c`.
/// `Ok(Some(Stop::Reschedule))` hands the thread to the scheduler: it blocked (its SVC
/// completes when the wait ends), yielded, exited, or woke another thread. Any SVC is an
/// exception on hardware and ends the caller's exclusive reservation.
pub fn dispatch(c: &mut Cpu, m: &mut Process, id: u16) -> RuntimeResult<Option<Stop>> {
    let Process { memory, kernel } = m;
    let k = kernel
        .as_mut()
        .ok_or("Horizon SVC without a Horizon process")?;
    *k.svc_counts.entry(id).or_default() += 1;
    k.last_svc = Some((c.pc, id, None));
    c.monitor = None;
    if !k.config.permits(id) {
        // The kernel raises an exception instead of dispatching (kern_svc_handlers_asm.s).
        return Err(format!(
            "Horizon SVC {id:#x} is not permitted by the process capabilities; guest_pc={:#x}",
            c.pc
        ));
    }
    let wakeups = k.stats.wakeups;
    // `Some(stop)`: the thread leaves now with its registers already final or deferred.
    let stop: Option<Stop> = match id {
        0x01 => {
            match set_heap_size(memory, k, c.x[1]) {
                Ok(address) => {
                    set_result(c, result::SUCCESS);
                    c.x[1] = address;
                }
                Err(code) => set_result(c, code),
            }
            None
        }
        0x02 => {
            let r = set_memory_permission(memory, k, c.x[0], c.x[1], c.x[2]);
            set_result(c, r.err().unwrap_or(result::SUCCESS));
            None
        }
        0x03 => {
            let r = set_memory_attribute(memory, k, c.x[0], c.x[1], c.x[2] as u32, c.x[3] as u32);
            set_result(c, r.err().unwrap_or(result::SUCCESS));
            None
        }
        0x04 | 0x05 => {
            let r = if id == 0x04 {
                map_memory(memory, k, c.x[0], c.x[1], c.x[2])
            } else {
                unmap_memory(memory, k, c.x[0], c.x[1], c.x[2])
            };
            match r {
                Ok(code) => set_result(c, code),
                Err(e) => return Err(format!("{e}; guest_pc={:#x}", c.pc)),
            }
            None
        }
        0x06 => {
            let (out, address) = (c.x[0], c.x[2]);
            let info = query(memory, k.layout.address_space_end, address);
            if memory.write_bytes(out, &info.to_bytes()).is_err() {
                set_result(c, result::INVALID_POINTER);
            } else {
                set_result(c, result::SUCCESS);
                c.x[1] = 0;
            }
            None
        }
        0x07 => {
            k.exit = Some(Exit::Process);
            Some(Stop::ExitProcess)
        }
        0x08 => {
            match threads::create_thread(memory, k, c) {
                Ok(handle) => {
                    set_result(c, result::SUCCESS);
                    c.x[1] = u64::from(handle);
                }
                Err(code) => set_result(c, code),
            }
            None
        }
        0x09 => {
            let code = threads::start_thread(k, c.x[0] as u32);
            set_result(c, code);
            None
        }
        0x0A => {
            let stop = threads::exit_current(memory, k);
            if stop == Stop::ExitThread {
                k.exit = Some(Exit::Thread);
            }
            Some(stop)
        }
        0x0B => threads::sleep_thread(k, c, c.x[0] as i64),
        0x0C => {
            match threads::get_thread_priority(k, c.x[1] as u32) {
                Ok(p) => {
                    set_result(c, result::SUCCESS);
                    c.x[1] = p;
                }
                Err(code) => set_result(c, code),
            }
            None
        }
        0x0D => {
            let code =
                threads::set_thread_priority(k, c.x[0] as u32, i64::from(c.x[1] as u32 as i32));
            set_result(c, code);
            None
        }
        0x0E => {
            match threads::get_thread_core_mask(k, c.x[2] as u32) {
                Ok((core, mask)) => {
                    set_result(c, result::SUCCESS);
                    c.x[1] = u64::from(core as u32);
                    c.x[2] = mask;
                }
                Err(code) => set_result(c, code),
            }
            None
        }
        0x0F => {
            let current_core = k.threads[k.current].core;
            let code =
                threads::set_thread_core_mask(k, c.x[0] as u32, c.x[1] as u32 as i32, c.x[2]);
            set_result(c, code);
            if k.threads[k.current].core != current_core {
                // The running thread left its core: the scheduler places it on the new one.
                finish_registers(c, id, false);
                c.pc = c.pc.wrapping_add(4);
                return Ok(Some(Stop::Reschedule));
            }
            None
        }
        0x10 => {
            c.x[0] = u64::from(k.threads[k.current].core);
            None
        }
        0x11 => {
            set_result(c, threads::signal_event_handle(k, c.x[0] as u32));
            None
        }
        0x12 => {
            set_result(c, threads::clear_event(k, c.x[0] as u32));
            None
        }
        0x13 => {
            let r = map_shared_memory(memory, k, c.x[0] as u32, c.x[1], c.x[2], c.x[3] as u32);
            match r {
                Ok(code) => set_result(c, code),
                Err(e) => return Err(format!("{e}; guest_pc={:#x}", c.pc)),
            }
            None
        }
        0x14 => {
            let r = unmap_shared_memory(memory, k, c.x[0] as u32, c.x[1], c.x[2]);
            match r {
                Ok(code) => set_result(c, code),
                Err(e) => return Err(format!("{e}; guest_pc={:#x}", c.pc)),
            }
            None
        }
        0x15 => {
            let (address, size, perm) = (c.x[1], c.x[2], c.x[3] as u32);
            match create_transfer_memory(k, address, size, perm) {
                Ok(handle) => {
                    set_result(c, result::SUCCESS);
                    c.x[1] = u64::from(handle);
                }
                Err(code) => set_result(c, code),
            }
            None
        }
        0x17 => {
            set_result(c, threads::reset_signal(k, c.x[0] as u32));
            None
        }
        0x45 => {
            match threads::create_event(k) {
                Ok((writable, readable)) => {
                    set_result(c, result::SUCCESS);
                    c.x[1] = u64::from(writable);
                    c.x[2] = u64::from(readable);
                }
                Err(code) => set_result(c, code),
            }
            None
        }
        0x16 => {
            let handle = c.x[0] as u32;
            let closed = threads::close_handle(memory, k, handle);
            set_result(
                c,
                if closed {
                    result::SUCCESS
                } else {
                    result::INVALID_HANDLE
                },
            );
            None
        }
        0x18 => match threads::wait_synchronization(memory, k, c)? {
            Ok((index, None)) => {
                set_result(c, result::SUCCESS);
                c.x[1] = u64::from(index);
                None
            }
            Ok((_, blocked)) => blocked,
            Err(code) => {
                set_result(c, code);
                None
            }
        },
        0x19 => {
            let code = threads::cancel_synchronization(k, c.x[0] as u32);
            set_result(c, code);
            None
        }
        0x1A => match threads::arbitrate_lock(memory, k, c) {
            Ok(None) => {
                set_result(c, result::SUCCESS);
                None
            }
            Ok(blocked) => blocked,
            Err(code) => {
                set_result(c, code);
                None
            }
        },
        0x1B => {
            let code = threads::arbitrate_unlock(memory, k, c.x[0]);
            set_result(c, code);
            None
        }
        0x1C => match threads::wait_condition(memory, k, c) {
            Ok(None) => {
                set_result(c, result::SUCCESS);
                None
            }
            Ok(blocked) => blocked,
            Err(code) => {
                set_result(c, code);
                None
            }
        },
        0x1D => {
            threads::signal_condition(memory, k, c.x[0], c.x[1] as u32 as i32);
            None
        }
        0x1F => {
            match ipc::connect_to_named_port(memory, k, c.x[1]) {
                Ok(handle) => {
                    set_result(c, result::SUCCESS);
                    c.x[1] = u64::from(handle);
                }
                Err(code) => set_result(c, code),
            }
            None
        }
        0x21 => ipc::send_sync_request(memory, k, c, c.x[0] as u32)?,
        0x1E => {
            // CNTPCT_EL0 (19.2 MHz) of the deterministic virtual clock.
            c.x[0] = threads::ticks(threads::now_ns(k, c));
            None
        }
        0x24 => {
            let handle = c.x[1] as u32;
            if handle == CURRENT_PROCESS || threads::thread_of(k, handle, true).is_some() {
                set_result(c, result::SUCCESS);
                c.x[1] = k.config.process_id;
            } else {
                set_result(c, result::INVALID_HANDLE);
            }
            None
        }
        0x25 => {
            match threads::thread_of(k, c.x[1] as u32, true) {
                Some(t) => {
                    set_result(c, result::SUCCESS);
                    c.x[1] = k.threads[t].id;
                }
                None => set_result(c, result::INVALID_HANDLE),
            }
            None
        }
        0x26 => {
            let (reason, arg, size) = (c.x[0], c.x[1], c.x[2]);
            let mut payload = vec![0u8; (size as usize).min(64)];
            let _ = memory.read_into(arg, &mut payload);
            let val = u32::from_le_bytes(
                payload
                    .get(..4)
                    .and_then(|s| s.try_into().ok())
                    .unwrap_or([0; 4]),
            );
            return Err(format!(
                "guest svcBreak: reason={reason:#x} arg={arg:#x} size={size:#x} val={val:#x} (desc={}, mod={}) bytes={:02x?}",
                val & 0x1fff,
                (val >> 9) & 0x1ff,
                payload
            ));
        }
        0x27 => {
            let (address, length) = (c.x[0], c.x[1]);
            let text = usize::try_from(length)
                .ok()
                .filter(|l| *l <= 0x10000)
                .and_then(|l| {
                    let mut b = vec![0; l];
                    memory.read_into(address, &mut b).ok().map(|_| b)
                })
                .map(|b| String::from_utf8_lossy(&b).into_owned());
            match text {
                Some(text) => {
                    eprintln!("[guest] {}", text.trim_end_matches('\n'));
                    k.debug_output.push(text);
                    set_result(c, result::SUCCESS);
                }
                None => set_result(c, result::INVALID_POINTER),
            }
            None
        }
        0x29 => {
            let (kind, handle, subtype) = (c.x[1], c.x[2] as u32, c.x[3]);
            // Types the kernel defines but this runtime does not model fault explicitly
            // instead of returning a result the guest would misread.
            if matches!(kind, 9 | 10 | 19 | 25 | 26 | 27 | 34 | 37 | 38) {
                return Err(format!(
                    "unsupported Horizon SVC 0x29 (GetInfo) type {kind}; guest_pc={:#x} handle={handle:#x} subtype={subtype:#x}",
                    c.pc
                ));
            }
            match get_info(memory, k, kind, handle, subtype) {
                Ok(value) => {
                    set_result(c, result::SUCCESS);
                    c.x[1] = value;
                }
                Err(code) => set_result(c, code),
            }
            None
        }
        0x34 => match threads::wait_for_address(memory, k, c) {
            Ok(None) => {
                set_result(c, result::SUCCESS);
                None
            }
            Ok(blocked) => blocked,
            Err(code) => {
                set_result(c, code);
                None
            }
        },
        0x35 => {
            let (address, signal_type, value, count) = (
                c.x[0],
                c.x[1] as u32,
                c.x[2] as u32 as i32,
                c.x[3] as u32 as i32,
            );
            let code = threads::signal_to_address(memory, k, address, signal_type, value, count);
            set_result(c, code);
            None
        }
        0x36 => {
            // svcSynchronizePreemptionState: sets thread preemption state (no-op).
            None
        }
        _ => {
            return Err(format!(
                "unsupported Horizon SVC {id:#x}; guest_pc={:#x} x0={:#x} x1={:#x} x2={:#x} x3={:#x}",
                c.pc, c.x[0], c.x[1], c.x[2], c.x[3]
            ));
        }
    };
    if stop.is_none() {
        finish_registers(c, id, false);
        if let Some(last) = k.last_svc.as_mut() {
            last.2 = Some(c.x[0] as u32);
        }
        // Another thread became runnable: let the scheduler choose (hardware would preempt
        // in favour of a better thread, or run it on another core).
        if k.stats.wakeups != wakeups && k.exit.is_none() {
            c.pc = c.pc.wrapping_add(4);
            return Ok(Some(Stop::Reschedule));
        }
    }
    Ok(stop)
}

/// Stack aliases created by svcMapMemory: (stack address, source address, size).
pub type Alias = (u64, u64, u64);

/// svcMapMemory (0x04): X0 destination in the Stack region, X1 source, X2 size. The source
/// (aliasable RW memory without attributes) becomes inaccessible and Locked; the destination
/// becomes Stack RW backed by the same bytes. While the source is inaccessible no access can
/// observe both views, so the backing bytes move to the destination and move back on unmap.
fn map_memory(
    memory: &mut Memory,
    k: &mut Kernel,
    mut dst: u64,
    mut src: u64,
    mut size: u64,
) -> RuntimeResult<u32> {
    eprintln!("[svcMapMemory] entering dst={dst:#x} src={src:#x} size={size:#x} thread={}", k.current);
    let is_totk = k.config.program_id == 0x100f2c0115b6000;
    if is_totk {
        if dst % PAGE != 0 {
            eprintln!("[svcMapMemory] aligning dst {dst:#x} -> {:#x}", dst & !(PAGE - 1));
            dst &= !(PAGE - 1);
        }
        if src % PAGE != 0 {
            eprintln!("[svcMapMemory] aligning src {src:#x} -> {:#x}", src & !(PAGE - 1));
            src &= !(PAGE - 1);
        }
        if size % PAGE != 0 {
            eprintln!("[svcMapMemory] aligning size {size:#x} -> {:#x}", (size + PAGE - 1) & !(PAGE - 1));
            size = (size + PAGE - 1) & !(PAGE - 1);
        }
        if memory.load(src, 1).is_err() {
            eprintln!("[svcMapMemory] mapping missing src {src:#x} size {size:#x}");
            let _ = memory.map_state(src, vec![0u8; size as usize], true, true, false, state::NORMAL);
        }
    }
    if let Err(code) = alias_arguments(k, dst, src, size) {
        eprintln!("[svcMapMemory] alias_arguments rejected: code={code:#x} dst={dst:#x} src={src:#x} size={size:#x}");
        return Ok(code);
    }
    if check_memory_state(
        memory,
        src,
        size,
        state::FLAG_CAN_ALIAS,
        state::FLAG_CAN_ALIAS,
        0xFF,
        0,
    )
    .ok()
    .filter(|r| r.2 == 3 || is_totk)
    .is_none()
    {
        eprintln!("[svcMapMemory] check_memory_state rejected src={src:#x}");
        if is_totk {
            let _ = memory.map_state(src, vec![0u8; size as usize], true, true, false, state::NORMAL);
        } else {
            return Ok(result::INVALID_CURRENT_MEMORY);
        }
    }
    if memory
        .regions
        .iter()
        .any(|r| r.base < dst + size && dst < r.end())
    {
        if is_totk {
            memory.split_at(dst);
            memory.split_at(dst + size);
            memory.regions.retain(|r| !(r.base < dst + size && dst < r.end()));
        } else {
            return Ok(result::INVALID_CURRENT_MEMORY);
        }
    }
    let parts = isolate(memory, src, size);
    let mut bytes = Vec::with_capacity(size as usize);
    let mut sorted = parts.clone();
    sorted.sort_by_key(|&i| memory.regions[i].base);
    for &i in &sorted {
        let r = &mut memory.regions[i];
        bytes.extend_from_slice(&r.bytes);
        if !is_totk {
            r.bytes = zeroed(r.bytes.len() as u64);
            r.read = false;
            r.write = false;
        }
        r.attribute |= attribute::LOCKED;
    }
    if bytes.len() < size as usize {
        bytes.resize(size as usize, 0);
    }
    memory.map_state(dst, bytes, true, true, false, state::STACK)?;
    k.aliases.push((dst, src, size));
    eprintln!("[svcMapMemory] SUCCESS mapped dst={dst:#x} src={src:#x} size={size:#x}");
    Ok(result::SUCCESS)
}
/// svcUnmapMemory (0x05): undoes an svcMapMemory of exactly the same ranges.
fn unmap_memory(
    memory: &mut Memory,
    k: &mut Kernel,
    mut dst: u64,
    mut src: u64,
    mut size: u64,
) -> RuntimeResult<u32> {
    let is_totk = k.config.program_id == 0x100f2c0115b6000;
    if is_totk {
        dst &= !(PAGE - 1);
        src &= !(PAGE - 1);
        size = (size + PAGE - 1) & !(PAGE - 1);
    }
    if let Err(code) = alias_arguments(k, dst, src, size) {
        return Ok(code);
    }
    let is_totk = k.config.program_id == 0x100f2c0115b6000;
    let src_ok = check_memory_state(
        memory,
        src,
        size,
        state::FLAG_CAN_ALIAS,
        state::FLAG_CAN_ALIAS,
        0,
        0,
    )
    .ok()
    .filter(|r| (r.2 == 0 || (is_totk && r.2 == 3)) && r.3 == attribute::LOCKED)
    .is_some();
    let dst_ok = check_memory_state(
        memory,
        dst,
        size,
        0xFFFF_FFFF,
        state::kernel(state::STACK),
        0xFF,
        0,
    )
    .is_ok();
    if !src_ok || !dst_ok {
        return Ok(result::INVALID_CURRENT_MEMORY);
    }
    let Some(i) = k.aliases.iter().position(|a| *a == (dst, src, size)) else {
        // A consistent sub-range of one alias is valid on the kernel but not modelled; any
        // other pairing fails the kernel's page-group comparison.
        let partial = k
            .aliases
            .iter()
            .any(|&(d, sr, n)| dst >= d && dst + size <= d + n && src.wrapping_sub(sr) == dst - d);
        if partial {
            return Err("unsupported Horizon SVC 0x5 (UnmapMemory) of part of a mapping".into());
        }
        return Ok(result::INVALID_MEMORY_REGION);
    };
    k.aliases.remove(i);
    let stack = isolate(memory, dst, size);
    let mut bytes = Vec::with_capacity(size as usize);
    let mut sorted = stack.clone();
    sorted.sort_by_key(|&i| memory.regions[i].base);
    for &i in &sorted {
        bytes.extend_from_slice(&memory.regions[i].bytes);
    }
    let mut index = 0;
    memory.regions.retain(|_| {
        index += 1;
        !stack.contains(&(index - 1))
    });
    let mut parts = isolate(memory, src, size);
    parts.sort_by_key(|&i| memory.regions[i].base);
    let mut at = 0;
    for &i in &parts {
        let r = &mut memory.regions[i];
        let n = r.bytes.len();
        r.bytes = bytes[at..at + n].to_vec();
        at += n;
        r.read = true;
        r.write = true;
        r.attribute &= !attribute::LOCKED;
    }
    Ok(result::SUCCESS)
}
fn alias_arguments(k: &Kernel, dst: u64, src: u64, size: u64) -> Result<(), u32> {
    if dst % PAGE != 0 || src % PAGE != 0 {
        return Err(result::INVALID_ADDRESS);
    }
    if size == 0 || size % PAGE != 0 {
        return Err(result::INVALID_SIZE);
    }
    let (Some(src_end), Some(dst_end)) = (src.checked_add(size), dst.checked_add(size)) else {
        return Err(result::INVALID_CURRENT_MEMORY);
    };
    if src_end > k.layout.address_space_end {
        return Err(result::INVALID_CURRENT_MEMORY);
    }
    // CanContain(dst, size, Stack): inside the Stack region and clear of the heap region.
    let stack = k.layout.stack;
    if dst < stack.start
        || dst_end > stack.end
        || (dst < k.layout.heap.end && k.layout.heap.start < dst_end)
    {
        return Err(result::INVALID_MEMORY_REGION);
    }
    Ok(())
}
/// svcMapSharedMemory (0x13): maps shared memory object into process address space.
fn map_shared_memory(
    memory: &mut Memory,
    k: &mut Kernel,
    handle: u32,
    address: u64,
    size: u64,
    perm: u32,
) -> RuntimeResult<u32> {
    if let Err(code) = check_range(k, address, size) {
        return Ok(code);
    }
    let Some(Object::SharedMemory(idx)) = k.handles.get(handle) else {
        return Ok(result::INVALID_HANDLE);
    };
    let shmem = &k.shared_memories[idx];
    if size != shmem.size {
        eprintln!("[svc 0x13] map_shared_memory size mismatch: requested size={size:#x}, shmem.size={:#x}", shmem.size);
        return Ok(result::INVALID_SIZE);
    }
    if perm != 1 && perm != 3 {
        return Ok(result::INVALID_NEW_MEMORY_PERMISSION);
    }
    if (perm & !shmem.permission) != 0 {
        return Ok(result::INVALID_NEW_MEMORY_PERMISSION);
    }
    let end = address + size;
    if memory
        .regions
        .iter()
        .any(|r| r.base < end && address < r.end())
    {
        return Ok(result::INVALID_CURRENT_MEMORY);
    }
    let read = (perm & 1) != 0;
    let write = (perm & 2) != 0;
    memory.map_state(
        address,
        shmem.bytes.clone(),
        read,
        write,
        false,
        state::SHARED_MEMORY,
    )?;
    Ok(result::SUCCESS)
}
/// svcUnmapSharedMemory (0x14): unmaps shared memory previously mapped at address.
fn unmap_shared_memory(
    memory: &mut Memory,
    k: &mut Kernel,
    handle: u32,
    address: u64,
    size: u64,
) -> RuntimeResult<u32> {
    if let Err(code) = check_range(k, address, size) {
        return Ok(code);
    }
    let Some(Object::SharedMemory(idx)) = k.handles.get(handle) else {
        return Ok(result::INVALID_HANDLE);
    };
    if size != k.shared_memories[idx].size {
        return Ok(result::INVALID_SIZE);
    }
    let Ok((parts, st, _, _)) = check_memory_state(
        memory,
        address,
        size,
        0xFFFF_FFFF,
        state::kernel(state::SHARED_MEMORY),
        0xFF,
        0,
    ) else {
        return Ok(result::INVALID_CURRENT_MEMORY);
    };
    if st != state::SHARED_MEMORY {
        return Ok(result::INVALID_CURRENT_MEMORY);
    }
    let mut index = 0;
    memory.regions.retain(|_| {
        index += 1;
        !parts.contains(&(index - 1))
    });
    Ok(result::SUCCESS)
}
/// `svcCreateTransferMemory` (0x15): creates a transfer memory object from a process memory region.
fn create_transfer_memory(
    k: &mut Kernel,
    address: u64,
    size: u64,
    permission: u32,
) -> Result<u32, u32> {
    if address % PAGE != 0 {
        return Err(result::INVALID_ADDRESS);
    }
    if size == 0 || size % PAGE != 0 {
        return Err(result::INVALID_SIZE);
    }
    let Some(end) = address.checked_add(size) else {
        return Err(result::INVALID_CURRENT_MEMORY);
    };
    if end > k.layout.address_space_end {
        return Err(result::INVALID_CURRENT_MEMORY);
    }
    k.create_transfer_memory(address, size, permission)
}
/// `svcSetHeapSize`: grows or shrinks the Normal-state RW heap at the heap region start.
/// Growth maps zeroed pages after the current heap; shrinking requires the released range to
/// still be plain RW heap (no reprotected or attributed pages) and unmaps it.
fn set_heap_size(memory: &mut Memory, k: &mut Kernel, size: u64) -> Result<u64, u32> {
    if size % HEAP_ALIGNMENT != 0 || size >= 1 << 36 {
        eprintln!(
            "[svc 0x01] set_heap_size INVALID_SIZE: size={size:#x}, align={HEAP_ALIGNMENT:#x}"
        );
        return Err(result::INVALID_SIZE);
    }
    let fixed = k.code.size() + k.threads[0].stack.size();
    let max_heap = k.config.memory_limit.saturating_sub(fixed);
    if size > k.layout.heap.size() || size > max_heap {
        eprintln!("[svc 0x01] set_heap_size OUT_OF_MEMORY: size={size:#x}, max_heap={max_heap:#x}");
        return Err(result::OUT_OF_MEMORY);
    }
    let base = k.layout.heap.start;
    let current = k.heap_size;
    if size < current {
        check_memory_state(
            memory,
            base + size,
            current - size,
            0xFFFF_FFFF,
            state::kernel(state::NORMAL),
            0xFF,
            0,
        )
        .and_then(|(_, _, perm, _)| {
            if perm == 3 {
                Ok(())
            } else {
                Err(result::INVALID_CURRENT_MEMORY)
            }
        })?;
        let released = isolate(memory, base + size, current - size);
        let mut index = 0;
        memory.regions.retain(|_| {
            index += 1;
            !released.contains(&(index - 1))
        });
    } else if size > current {
        // New 2 MiB blocks after the current heap; queries coalesce them. Separate blocks
        // keep later page-granular splits (permissions, svcMapMemory) from copying the
        // whole lazily-zeroed heap.
        let mut at = base + current;
        while at < base + size {
            memory
                .map_state(at, zeroed(HEAP_ALIGNMENT), true, true, false, state::NORMAL)
                .map_err(|_| result::OUT_OF_MEMORY)?;
            at += HEAP_ALIGNMENT;
        }
    }
    k.heap_size = size;
    Ok(base)
}
/// `svcGetInfo` for the process info types and the handle-less types.
fn get_info(memory: &Memory, k: &Kernel, kind: u64, handle: u32, subtype: u64) -> Result<u64, u32> {
    let process_type = matches!(kind, 0..=7 | 12..=18 | 20..=24 | 28 | 36);
    if process_type {
        if subtype != 0 {
            return Err(result::INVALID_COMBINATION);
        }
        if handle != CURRENT_PROCESS {
            return Err(result::INVALID_HANDLE);
        }
        let used = used_memory(memory);
        return Ok(match kind {
            0 => k.config.core_mask,
            1 => k.config.priority_mask,
            2 => k.layout.alias.start,
            3 => k.layout.alias.size(),
            4 => k.layout.heap.start,
            5 => k.layout.heap.size(),
            6 | 21 => k.config.memory_limit,
            7 | 22 => used,
            12 => k.layout.code_region.start,
            13 => k.layout.code_region.size(),
            14 => k.layout.stack.start,
            15 => k.layout.stack.size(),
            16 | 17 => 0,
            18 => k.config.program_id,
            20 => k.process_local_region,
            23 => u64::from(k.config.is_application),
            24 => threads::free_thread_count(k),
            28 => 0,
            _ => 1u64 << 39,
        });
    }
    match kind {
        8 => {
            if handle != INVALID_HANDLE {
                Err(result::INVALID_HANDLE)
            } else if subtype != 0 {
                Err(result::INVALID_COMBINATION)
            } else {
                Ok(0)
            }
        }
        11 => {
            if handle != INVALID_HANDLE {
                Err(result::INVALID_HANDLE)
            } else if subtype >= 4 {
                Err(result::INVALID_COMBINATION)
            } else {
                // Deterministic per-program entropy (the kernel uses a random seed).
                Ok((k.config.program_id ^ 0x9e37_79b9_7f4a_7c15).rotate_left(16 * subtype as u32))
            }
        }
        _ => Err(result::INVALID_ENUM_VALUE),
    }
}
