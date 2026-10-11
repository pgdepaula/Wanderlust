# Horizon process environment

`wanderlust recompile DIR --horizon` builds an executable that starts the loaded process the way Horizon starts an application's main thread and serves its supervisor calls (SVCs) from the runtime. Implementation: [`wl-runtime::horizon`](../../crates/wl-runtime/src/horizon.rs). Without `--horizon` the executable is the register-level test harness described in the [execution model](../architecture/execution-model.md#runtime-contract); both modes share the translated code, CPU state and memory model.

## Sources

| Source | Revision | Used for | Licence handling |
|---|---|---|---|
| Atmosphère `libmesosphere` (`kern_k_page_table_base.cpp`, `kern_k_process.cpp`, `kern_k_handle_table.hpp`, `kern_k_memory_block.hpp`, `kern_k_capabilities.*`, `svc/kern_svc_*.cpp`) | [0e9cd964](https://github.com/Atmosphere-NX/Atmosphere/tree/0e9cd964c094946df14926b6c81a631b603a30f0) | address-space layout, memory states and flags, SVC argument checks and results, handle encoding, thread start | GPL-2.0: behaviour restated, no code copied |
| Atmosphère `libvapours` (`svc/svc_types_common.hpp`, `results/svc_results.hpp`, `svc/codegen/*`) | same | SVC numbers, `MemoryInfo`, result values, SVC register contract | same |
| libnx (`nx/source/runtime/*`, `nx/source/kernel/svc.s`, `nx/include/switch/arm/tls.h`) | [feebd026](https://github.com/switchbrew/libnx/tree/feebd0263b2fed8fdc6fdf14b5ef46efd85e5d72) | user-mode expectations: crt0, self-relocation, TLS layout, heap and stack setup | ISC |

Values marked *deterministic choice* below are where Horizon is random or indeterminate and the runtime fixes one value.

## Address space

Only 64-bit processes with a 39-bit address space (NPDM `address_space_type` 3) are accepted. The code region (`MapHuge`/ASLR region) is `[0x8000000, 2^39)`. `Layout::new_39bit` reproduces `KPageTableBase::InitializeForProcess` without ASLR: the process code range is widened to 2 MiB alignment, the KernelMap (64 GiB), Stack (2 GiB), Alias (64 GiB) and Heap (8 GiB) regions are taken largest first and placed on whichever side of the code has more remaining space, then laid out in type order (KernelMap, Stack, Alias, Heap) without random offsets. For a process at `0x8000000` with less than 2 MiB of code:

| Region | Start | Size |
|---|---|---|
| code | `0x8000000` | to 2^39 |
| KernelMap | `0x8200000` | 64 GiB |
| Stack | `0x1008200000` | 2 GiB |
| Alias | `0x1088200000` | 64 GiB |
| Heap | `0x2088200000` | 8 GiB |

Mappings found inside a region (`FindFreeArea`) keep four guard pages on each side and are placed first-fit from the region start (*deterministic choice*; Horizon randomizes within the region).

## Process start

`Process::horizon` performs the steps of `KProcess::Run` that are visible to user mode:

1. One ThreadLocal page is mapped in the KernelMap region. Its first 0x200-byte slot is the process-local region (`InfoType_ProcessLocalRegionAddress`), the second is the main thread's TLS. The thread handle is stored at TLS + 0x110 (`ThreadLocalRegion::thread_handle`).
2. The main stack, `main_thread_stack_size` bytes from the NPDM (non-zero, page-multiple), is mapped as `Stack` RW in the Stack region.
3. The main thread is added to the handle table.
4. The CPU starts at the first module's text with `X0 = 0`, `X1 =` the thread handle, `SP =` the stack top, `TPIDRRO_EL0 =` the TLS address, and every other register (including X30) zero.

Process parameters come from `main.npdm`, parsed and validated by [`wl-npdm`](npdm.md) (`ProcessMetadata`): program ID, main-thread stack size, priority and ideal core from META; core mask, priority mask, handle-table size, program type and the SVC access flags from the ACI0 kernel capabilities. Without `main.npdm`, defaults are used (1 MiB stack, priority 44, core 0, core mask 1, every priority and SVC permitted, program ID 0) and `config_source` says so. A handle-table size of 0 selects the kernel maximum of 1024 entries (`KHandleTable::Initialize`). The process memory limit (`InfoType_TotalMemorySize`) is `--memory-limit` (default 512 MiB). Process ID `0x51` and main thread ID `0x72` are *deterministic choices*.

## Memory model

Every mapping is a region with Horizon `MemoryState`, `MemoryPermission` (R 1, W 2, X 4) and `MemoryAttribute`:

| Mapping | State | Permission |
|---|---|---|
| module text | Code (3) | R-X |
| module rodata, data, BSS | CodeData (4) | R / RW |
| heap | Normal (5) | RW |
| main stack | Stack (0xB) | RW |
| TLS page | ThreadLocal (0xC) | RW |

The kernel state used for capability checks is Mesosphere's `KMemoryState` value for each state (for example CodeData `0x0FFEBD04`: may be reprotected and permission-locked; Code `0x04DC7E03`: neither).

`svcSetMemoryPermission` and `svcSetMemoryAttribute` change part of a mapping by splitting its region at page boundaries. Memory accesses that span adjacent regions are legal if every byte is mapped and grants the access, as on hardware; the whole access is checked before any byte moves, so a fault leaves memory unchanged. Executable memory is never writable.

`svcQueryMemory` reports the maximal run of adjacent regions with equal state, permission and attribute containing the address; unmapped ranges are `Free` blocks spanning to the neighbouring mappings; addresses at or above 2^39 report one `Inaccessible` block from 2^39 to 2^64.

Heap and stack memory are allocated through Rust's zeroing allocator (`vec![0; n]`), so pages are committed only when the guest touches them: the 508 MiB heap of the probe costs 16 MiB peak RSS. Allocation failure aborts the process.

## SVCs

Arguments are in X0–X7. Results are in W0 and outputs from X1 upward. After every SVC, each of X0–X7 that is not an output of that SVC is zeroed and X8–X30 are preserved (Mesosphere's `svc_codegen` wrapper for the 64-bit ABI). Outputs are copied back even when the call fails; the kernel never writes them in that case, so their value is indeterminate. The runtime returns 0 (*deterministic choice*). Result codes are `1 | description << 9`.

| ID | SVC | Implemented behaviour | Errors |
|---|---|---|---|
| 0x01 | SetHeapSize | size multiple of 2 MiB; grows with zeroed Normal RW pages at the heap region start, shrinks by unmapping; returns the heap address | InvalidSize (101), OutOfMemory (104) above the region or memory limit, InvalidCurrentMemory (106) when shrinking over pages that are no longer plain RW heap |
| 0x02 | SetMemoryPermission | None, R or RW on memory whose state allows reprotection, with no attribute set | InvalidAddress (102), InvalidSize (101), InvalidCurrentMemory (106), InvalidNewMemoryPermission (108) |
| 0x03 | SetMemoryAttribute | Uncached / PermissionLocked bits; PermissionLocked can only be set and requires a lockable state | InvalidAddress, InvalidSize, InvalidCombination (116), InvalidCurrentMemory |
| 0x04 | MapMemory | aliases aliasable RW memory without attributes (heap, CodeData) at a free destination in the Stack region (outside the Heap region); the source becomes inaccessible and Locked, the destination Stack RW over the same bytes | InvalidAddress, InvalidSize, InvalidCurrentMemory, InvalidMemoryRegion (110) |
| 0x05 | UnmapMemory | undoes a MapMemory of the same ranges; writes through the alias are kept | as MapMemory; InvalidMemoryRegion when the destination is not an alias of the source |
| 0x06 | QueryMemory | writes the 0x28-byte `MemoryInfo`; PageInfo 0 | InvalidPointer (115) if the output is not writable |
| 0x07 | ExitProcess | ends execution, `status=exit_process` | — |
| 0x08 | CreateThread | X1 entry, X2 argument, X3 stack top, W4 priority, W5 core (−2: process ideal core); W1 handle. Allocates a thread object, TLS and handle; the thread is not started | InvalidCoreId (113), InvalidPriority (112), LimitReached (132), OutOfMemory, OutOfHandles |
| 0x09 | StartThread | makes a created thread runnable | InvalidHandle, InvalidState (125) if already started |
| 0x0A | ExitThread | ends the calling thread; it becomes signalled and its waiters wake; when no thread remains, `status=exit_thread` | — |
| 0x0B | SleepThread | positive: sleeps for the duration; 0, −1, −2: yields; other values: nothing; no return value | — |
| 0x0C | GetThreadPriority | W1 handle → W1 current priority (with inheritance) | InvalidHandle |
| 0x0D | SetThreadPriority | W0 handle, W1 priority | InvalidHandle, InvalidPriority |
| 0x10 | GetCurrentProcessorNumber | the thread's ideal core | — |
| 0x16 | CloseHandle | frees a table handle; pseudo-handles are not closable; an exited thread without handles is destroyed | InvalidHandle (114) |
| 0x18 | WaitSynchronization | X1 handle array, W2 count (0–64), X3 timeout (0: poll, < 0: infinite); W1 index of the first signalled object (after a timed-out or cancelled wait, −1). Thread objects are signalled once exited | OutOfRange (119), InvalidPointer, InvalidHandle (pseudo-handles included), TimedOut (117), Cancelled (118) |
| 0x19 | CancelSynchronization | ends a thread's WaitSynchronization with Cancelled, or makes its next one return Cancelled | InvalidHandle |
| 0x1A | ArbitrateLock | W0 owner handle, X1 lock word, W2 caller tag: if the word still equals owner \| 0x40000000, waits until the owner hands the lock over; otherwise returns at once | InvalidAddress (alignment), InvalidCurrentMemory, InvalidHandle |
| 0x1B | ArbitrateUnlock | X0 lock word: the best waiter (priority, then arrival) becomes owner, inheriting the other waiters; the word becomes its tag, with 0x40000000 if others still wait, or 0 | InvalidAddress, InvalidCurrentMemory |
| 0x1C | WaitProcessWideKeyAtomic | X0 lock word, X1 key, W2 tag, X3 timeout: releases the lock as ArbitrateUnlock, sets the key word to 1 and waits to be signalled and to reacquire the lock | InvalidPointer, InvalidAddress, InvalidCurrentMemory, TimedOut |
| 0x1D | SignalProcessWideKey | X0 key, W1 count (≤ 0: all): signalled waiters (priority, then arrival) take the lock if its word is 0, else queue on its owner with the wait bit set; the key word becomes 0 when no waiter remains; no return value | — |
| 0x1E | GetSystemTick | the deterministic virtual clock at 19.2 MHz | — |
| 0x24 | GetProcessId | process ID for the current-process pseudo-handle or a thread handle | InvalidHandle |
| 0x25 | GetThreadId | thread ID for the current-thread pseudo-handle or a thread handle | InvalidHandle |
| 0x26 | Break | fault, category `guest-break`, with reason and arguments | — |
| 0x27 | OutputDebugString | text to stderr as `[guest] …` and to `debug_output` (up to 64 KiB) | InvalidPointer |
| 0x29 | GetInfo | see below | InvalidCombination, InvalidHandle, InvalidEnumValue (120) |

An SVC whose bit is clear in the process's SVC access flags (or whose ID is 0xC0 or above) is a fault of category `svc-not-permitted`, where the kernel raises an exception instead of dispatching. Every other SVC not listed is a fault of category `unsupported-service` reporting the ID, PC and X0–X3; no SVC returns success without its effect.

`svcGetInfo` process types (0–7, 12–18, 20–24, 28, 36) require subtype 0 and the current-process pseudo-handle. Values: core mask (0), priority mask (1), Alias region (2, 3), Heap region (4, 5), memory limit (6, 21), used memory (7, 22: code, stack, heap and TLS pages), code region (12, 13), Stack region (14, 15), system resource 0 (16, 17), program ID (18), user exception context address (20; the process-local region, as in Mesosphere), is-application (23), free thread slots (24), 0 for 28 and 2^39 for 36. DebuggerAttached (8) requires handle 0 and returns 0. RandomEntropy (11) requires handle 0 and subtype < 4 and returns a value derived from the program ID (*deterministic choice*). Types the kernel defines but this runtime does not model (resource limit 9, idle and thread tick counts 10 and 25, initial process ID range 19, SVC permission 26, I/O and transfer-memory hints 27 and 34, shadow stack 37 and 38) fault with category `unsupported-service`; undefined types are InvalidEnumValue.

## Threads and scheduling

Implementation: [`threads.rs`](../../crates/wl-runtime/src/threads.rs).

A created thread starts like a Mesosphere user thread: PC = entry (bit 0 cleared), X0 = argument, SP = stack top aligned down to 16, X18 = an odd value (the kernel's random CFI value; a value derived from program and thread ID, *deterministic choice*), FPCR and FPSR copied from the creator, every other register zero, and TPIDRRO_EL0 = its own zeroed 0x200-byte thread-local region, with its handle at offset 0x110. Thread-local regions are taken from the first ThreadLocal page with a free slot (eight per page), or from a new page in the KernelMap region. A thread object, and its region, is released when it has exited and its last handle is closed. Thread IDs continue from the main thread's (`0x72`, `0x73`, …) and are never reused.

All threads of the process run on one host thread under a deterministic scheduler. Translated code returns to the dispatcher after every block, so each thread's complete architectural state (X0–X30, SP, PC, NZCV, V0–V31, FPCR, FPSR, TPIDR_EL0, TPIDRRO_EL0) is in its saved context at every block boundary. The scheduler runs one thread for a slice of `--quantum` guest instructions (default 10,000), checked at block entry, then lets the next virtual core run. Virtual cores are those of the process core mask, taken in turn; on each core the runnable thread with the best priority runs, the earliest queued among equals, as Horizon schedules one core. Same-core threads are not time-sliced, except that every 10 ms of virtual time the first thread at the core's preemption priority (59; 63 on core 3) moves behind the others. A lock owner runs at the best priority of the threads waiting for its locks (priority inheritance). This models concurrency on several cores by interleaving slices on one; it does not model timing or memory ordering between truly simultaneous cores.

A blocking SVC leaves its thread Waiting with the PC after the SVC. The thread that ends the wait (or the expiry of a timeout) writes the SVC's result and outputs into the saved context, so no SVC side effect is repeated. An SVC that wakes another thread also returns to the scheduler.

Time is virtual: one nanosecond per retired guest instruction, plus the time skipped when every thread waits for a timeout, when the clock jumps to the earliest deadline. Timeouts follow the kernel conversion (ticks at 19.2 MHz, plus 2 ticks). Results are identical on every host. When every remaining thread waits without a timeout, execution stops with a `guest deadlock` fault listing the waits.

Every SVC and every context switch clears the exclusive monitor of the thread involved, as an exception on hardware may. A later store-exclusive then fails and the guest retries, which the architecture permits. No reservation survives another thread's write.

## Handles

Handles follow Mesosphere's `KHandleTable`: `index | linear_id << 15`, linear IDs from 1, free indices taken from the top of the table down. Closing a handle frees its index; the stale handle stays invalid because its linear ID no longer matches. `0xFFFF8000` and `0xFFFF8001` are the current-thread and current-process pseudo-handles.

## Diagnostics

The launcher prints one JSON object: `mode`, `status` (`exit_process`, `exit_thread` or `fault`), `entry`, `pc` and `x0` of the thread that ended the run, `steps` (all threads), the main thread (`id`, `handle`, `tls`, `stack`), `threads` (ID, handle, TLS, state, priority, core, PC, instructions, whether destroyed), `scheduler` (quantum, slices, context switches, blocks, wakeups, timeouts, yields, rotations, maximum runnable threads, threads created, virtual and idle nanoseconds, host seconds), `heap` (address, size), `process_local_region`, `config_source`, `svc_counts`, `last_svc` (PC, ID, result), `debug_output` and `fault`. Faults also print a one-line diagnostic with category, module, module offset, instruction word and step count. `--steps N` sets the instruction budget; `--quantum N` the slice length.

## Limits

Threads run on their ideal core; svcGetThreadCoreMask/svcSetThreadCoreMask, thread activity, thread context access and core migration are not implemented. The only synchronization objects are threads: no events (svcCreateEvent, SignalEvent, ClearEvent, ResetSignal), no WaitForAddress/SignalToAddress. A partial svcUnmapMemory of an alias faults explicitly. No IPC (`ConnectToNamedPort`, `SendSyncRequest`) and therefore no services: applications must not depend on `__appInit` service initialization. No shared or transfer memory, code memory or device address spaces. Memory-mapping, interrupt and debug capabilities are parsed but have no effect. Only NSO entry semantics are implemented (NRO loader entry, with X0 = loader configuration and X1 = −1, is not).

## Validation

[`tests/horizon.rs`](../../crates/wl-runtime/tests/horizon.rs) checks the layout, handle encoding, bootstrap registers and TLS, QueryMemory blocks, every implemented SVC's results and memory effects, argument-validation order, permission splitting, spanning accesses, permission locking, heap growth and shrinking, the register contract, SVC access flags and the handle-table size.

[`probe.c`](../../tests/horizon/probe.c) is a libnx application built with devkitA64 (`tools/python/homebrew.py`, devkitA64 GCC 16.1.0, libnx 4.12.0) into the NSO and NPDM in [`tests/horizon/probe/`](../../tests/horizon/probe) (NSO SHA-256 `12d823e7496d0c82a0eeab665cf14bb6ed9b4554c22ae8a3026ef99e11cc6e73`). It runs libnx's real startup (crt0, `__nx_dynamic` DT_RELR self-relocation, RELRO reprotection and permission lock, virtual-memory and heap setup through GetInfo/SetHeapSize, stack query), then its own SVC and memory checks, newlib `malloc` and `snprintf`, and exits through `svcExitProcess`. [`tools/wl-cli/tests/horizon.rs`](../../tools/wl-cli/tests/horizon.rs) compiles it and requires these exact lines, derived independently of the runtime:

```text
probe tid=0x72 pid=0x51 handle=0x83fe tls=0x5cbc getinfo_bad_subtype=0xe801 out=0
text state=3 perm=5 size=0x10000; stack state=11 perm=3 size=0x100000 in_stack_region=1
heap_ok=1 sum=952e8000 ratio=0.001953 used_below_total=1
```

The test rebuilds the probe from source when devkitA64 is available and requires the same lines. Analysis recovers 12,811 instructions, 4 jump tables and 62 constant-derived roots; execution retires 64,767 guest instructions and exits with status 0. The same NSO compiled for `x86_64-pc-windows-gnu` and run on Windows 10.0.26300 printed identical lines and SVC counts.

[`tests/threads.rs`](../../crates/wl-runtime/tests/threads.rs) checks thread creation (initial context, TLS slots and pages, validation, the thread limit), start and exit, waits on threads (signalled, polled, timed out with index −1, cancelled, deadline conversion), handle lifetime (destruction on last close, stale handles, TLS reuse, non-reused IDs), lock hand-over with the wait bit and priority inheritance, condition-variable release, signal and reacquisition, MapMemory/UnmapMemory, monitor clearing, and the scheduler running block functions: cross-core interleaving, strict priority on one core, idle time advance and deadlock reporting.

[`threads.c`](../../tests/horizon/threads.c), built the same way into [`tests/horizon/threads/`](../../tests/horizon/threads) (NSO SHA-256 `fd188bad65649e28adbb68ff74f27a8b623da19f27db8bc88fe2b32b383f2ec2`), creates a worker with libnx `threadCreate` on core 1. Its stack comes from the heap and is mapped with svcMapMemory. Main and worker each add 3,000 values to a shared total under a libnx `Mutex`, with a slow read-modify-write inside the critical section. The worker signals a libnx `CondVar` and sleeps 2 ms; main times out after 100 µs waiting for it, finishes, joins it with `threadWaitForExit` and closes it. The CLI test requires:

```text
threads create=0 start=0 main_tid=0x72 worker_tid=0x73 tls_main=0x8204200 tls_worker=0x8204400
sync total=0000000001180eea entries=3000,3000 tls_values=3007,3007 ready=1
wait timed=0xea01 index=-1 joined=0 closed=0
```

The total is the sum over both threads of 24 byte windows of a mixing function, computed independently. A lost update from broken mutual exclusion would change it. Each thread's `__thread` counter ends at 7 + 3,000. The run retires 1,041,282 guest instructions in 208 slices with 156 context switches, 50 contended svcArbitrateLock calls, 54 blocking waits and 1.75 ms of idle virtual time. Linux and Windows (`x86_64-pc-windows-gnu`, Windows 10.0.26300) executables produced identical output and identical counters, taking 0.044 s and 0.062 s of host time.

The fixture binaries contain libnx (ISC, notice in [`tests/horizon/NOTICE`](../../tests/horizon/NOTICE)), newlib (licences reproduced in [`tests/horizon/COPYING.NEWLIB`](../../tests/horizon/COPYING.NEWLIB)) and libgcc (GPL with the GCC Runtime Library Exception) from devkitA64.
