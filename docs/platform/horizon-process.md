# Horizon process environment

`wanderlust recompile DIR --horizon` builds an executable that starts the loaded process the way Horizon starts an application's main thread and serves its supervisor calls (SVCs) from the runtime. Implementation: [`wl-runtime::horizon`](../../crates/wl-runtime/src/horizon.rs). Without `--horizon` the executable is the register-level test harness described in the [execution model](../architecture/execution-model.md#runtime-contract); both modes share the translated code, CPU state and memory model.

## Sources

| Source | Revision | Used for | Licence handling |
|---|---|---|---|
| Atmosphère `libmesosphere` (`kern_k_page_table_base.cpp`, `kern_k_process.cpp`, `kern_k_handle_table.hpp`, `kern_k_memory_block.hpp`, `kern_k_capabilities.*`, `svc/kern_svc_*.cpp`) | [0e9cd964](https://github.com/Atmosphere-NX/Atmosphere/tree/0e9cd964c094946df14926b6c81a631b603a30f0) | address-space layout, memory states and flags, SVC argument checks and results, handle encoding, thread start | GPL-2.0: behaviour restated, no code copied |
| Atmosphère `libvapours` (`svc/svc_types_common.hpp`, `results/svc_results.hpp`, `svc/codegen/*`) | same | SVC numbers, `MemoryInfo`, result values, SVC register contract | same |
| Atmosphère `libstratosphere` (`source/sf/hipc/sf_hipc_server_session_manager.cpp`, `source/sf/cmif/sf_cmif_service_dispatch.cpp`, `include/stratosphere/sf/impl/sf_impl_command_serialization.hpp`, `include/stratosphere/tipc/*`, `source/lm/*`) and `stratosphere/sm` | same | CMIF and TIPC server message handling, sm and lm behaviour | same |
| SwitchBrew wiki (PTM services, Applet Manager services, IPC Marshalling) | pages as of 2026-10-14 | apm and am command tables and values, NoMessage | documentation, restated |
| am sysmodule of firmware 22.1.0 (program 0100000000000023), analysed statically from a local copy | firmware 22.1.0 | am's pointer-buffer size | values only; no code or binaries reproduced |
| libnx (`nx/source/runtime/*`, `nx/source/kernel/svc.s`, `nx/include/switch/arm/tls.h`) | [feebd026](https://github.com/switchbrew/libnx/tree/feebd026ca0f5dcc2119f46ad8e0d16ad3dd4973) | user-mode expectations: crt0, self-relocation, TLS layout, heap and stack setup | ISC |

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
4. The CPU starts at the first module's text with `X0 = 0`, `X1 =` the thread handle, `SP =` the stack top, `TPIDRRO_EL0 =` the TLS address, X18 the thread's CFI value (see [threads](#threads-and-scheduling)), and every other register (including X30) zero.

An NRO input (`recompile FILE.nro --horizon`) is parsed and planned by [`wl-nro`](nro.md) (`HomebrewImagePlan`). Its text, rodata and data become one guest-linked module at `0x8000000` with BSS after data (`wl_nso::Module::from_segments`, so MOD0 and dynamic-table discovery are those of NSO), and libnx's crt0 relocates itself. It starts with the homebrew ABI that libnx's crt0 and `envSetup` implement: `X0 =` a ConfigEntry list, `X1 = −1`, `X30 =` the loader return address. The list (24-byte entries: key, flags, two values) holds MainThreadHandle (mandatory), SyscallAvailableHint and SyscallAvailableHint2 (from the SVC access flags) and EndOfList without loader information. It lives in a Normal RW page of the KernelMap region mapped by the runtime; a homebrew loader keeps it in its own memory. The return address is the runtime's return token: when libnx's `__nx_exit` branches there, the run ends with `status=returned` and the result in X0. NROs carry no NPDM, so the defaults above apply.

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
| 0x0E | GetThreadCoreMask | W2 thread handle → W1 ideal core, X2 affinity mask (initially the creation core and that core alone) | InvalidHandle |
| 0x0F | SetThreadCoreMask | W0 thread handle, W1 ideal core (−1 no preference, −2 the process's ideal core with only that core, −3 keep the ideal core), X2 affinity mask. A thread whose core leaves the mask moves to its ideal core, or to the highest core of the mask; the running thread's slice then ends | InvalidHandle, InvalidCoreId (mask outside the process's cores, or a core ID that is neither 0–63 nor −1/−3), InvalidCombination (empty mask, ideal core outside the mask) |
| 0x10 | GetCurrentProcessorNumber | the core the thread runs on | — |
| 0x11 | SignalEvent | W0 writable-event handle: the event becomes signalled and its waiters wake; signalling a signalled event does nothing | InvalidHandle (any other object) |
| 0x12 | ClearEvent | W0 handle to either half of an event: it stops being signalled | InvalidHandle |
| 0x13 | MapSharedMemory | W0 shmem handle, X1 address, X2 size, W3 permission (Read 1, ReadWrite 3); maps the shared memory with state SharedMemory (0xD) and requested permission | InvalidHandle, InvalidAddress, InvalidSize, InvalidCurrentMemory, InvalidNewMemoryPermission |
| 0x14 | UnmapSharedMemory | W0 shmem handle, X1 address, X2 size; unmaps shared memory previously mapped at address | InvalidHandle, InvalidAddress, InvalidSize, InvalidCurrentMemory |
| 0x16 | CloseHandle | frees a table handle; pseudo-handles are not closable; an exited thread without handles is destroyed | InvalidHandle (114) |
| 0x17 | ResetSignal | W0 readable-event handle: clears a signalled event | InvalidState (not signalled), InvalidHandle |
| 0x18 | WaitSynchronization | X1 handle array, W2 count (0–64), X3 timeout (0: poll, < 0: infinite); W1 index of the first signalled object (after a timed-out or cancelled wait, −1). Thread objects are signalled once exited; readable events while signalled (a wait does not clear them) | OutOfRange (119), InvalidPointer, InvalidHandle (pseudo-handles included), TimedOut (117), Cancelled (118) |
| 0x19 | CancelSynchronization | ends a thread's WaitSynchronization with Cancelled, or makes its next one return Cancelled | InvalidHandle |
| 0x1A | ArbitrateLock | W0 owner handle, X1 lock word, W2 caller tag: if the word still equals owner \| 0x40000000, waits until the owner hands the lock over; otherwise returns at once | InvalidAddress (alignment), InvalidCurrentMemory, InvalidHandle |
| 0x1B | ArbitrateUnlock | X0 lock word: the best waiter (priority, then arrival) becomes owner, inheriting the other waiters; the word becomes its tag, with 0x40000000 if others still wait, or 0 | InvalidAddress, InvalidCurrentMemory |
| 0x1C | WaitProcessWideKeyAtomic | X0 lock word, X1 key, W2 tag, X3 timeout: releases the lock as ArbitrateUnlock, sets the key word to 1 and waits to be signalled and to reacquire the lock | InvalidPointer, InvalidAddress, InvalidCurrentMemory, TimedOut |
| 0x1D | SignalProcessWideKey | X0 key, W1 count (≤ 0: all): signalled waiters (priority, then arrival) take the lock if its word is 0, else queue on its owner with the wait bit set; the key word becomes 0 when no waiter remains; no return value | — |
| 0x1F | ConnectToNamedPort | X1 name (at most 11 characters): W1 client session; the only named port is `sm:` | OutOfRange (no terminator within 12 bytes), InvalidCurrentMemory, NotFound (121) |
| 0x21 | SendSyncRequest | W0 client session: the request in the thread's message buffer, answered in place (see [IPC](#ipc)) | InvalidHandle, SessionClosed (123), MessageTooLarge (260) |
| 0x1E | GetSystemTick | the deterministic virtual clock at 19.2 MHz | — |
| 0x24 | GetProcessId | process ID for the current-process pseudo-handle or a thread handle | InvalidHandle |
| 0x25 | GetThreadId | thread ID for the current-thread pseudo-handle or a thread handle | InvalidHandle |
| 0x26 | Break | fault, category `guest-break`, with reason and arguments | — |
| 0x27 | OutputDebugString | text to stderr as `[guest] …` and to `debug_output` (up to 64 KiB) | InvalidPointer |
| 0x29 | GetInfo | see below | InvalidCombination, InvalidHandle, InvalidEnumValue (120) |
| 0x45 | CreateEvent | W1 writable-event handle, W2 readable-event handle of a new unsignalled event | OutOfHandles |

An SVC whose bit is clear in the process's SVC access flags (or whose ID is 0xC0 or above) is a fault of category `svc-not-permitted`, where the kernel raises an exception instead of dispatching. Every other SVC not listed is a fault of category `unsupported-service` reporting the ID, PC and X0–X3; no SVC returns success without its effect.

`svcGetInfo` process types (0–7, 12–18, 20–24, 28, 36) require subtype 0 and the current-process pseudo-handle. Values: core mask (0), priority mask (1), Alias region (2, 3), Heap region (4, 5), memory limit (6, 21), used memory (7, 22: code, stack, heap and TLS pages), code region (12, 13), Stack region (14, 15), system resource 0 (16, 17), program ID (18), user exception context address (20; the process-local region, as in Mesosphere), is-application (23), free thread slots (24), 0 for 28 and 2^39 for 36. DebuggerAttached (8) requires handle 0 and returns 0. RandomEntropy (11) requires handle 0 and subtype < 4 and returns a value derived from the program ID (*deterministic choice*). Types the kernel defines but this runtime does not model (resource limit 9, idle and thread tick counts 10 and 25, initial process ID range 19, SVC permission 26, I/O and transfer-memory hints 27 and 34, shadow stack 37 and 38) fault with category `unsupported-service`; undefined types are InvalidEnumValue.

## Threads and scheduling

Implementation: [`threads.rs`](../../crates/wl-runtime/src/threads.rs).

A created thread starts like a Mesosphere user thread: PC = entry (bit 0 cleared), X0 = argument, SP = stack top aligned down to 16, X18 = an odd value (the kernel's random CFI value; a value derived from program and thread ID, *deterministic choice*), FPCR and FPSR copied from the creator, every other register zero, and TPIDRRO_EL0 = its own zeroed 0x200-byte thread-local region, with its handle at offset 0x110. Thread-local regions are taken from the first ThreadLocal page with a free slot (eight per page), or from a new page in the KernelMap region. A thread object, and its region, is released when it has exited and its last handle is closed. Thread IDs continue from the main thread's (`0x72`, `0x73`, …) and are never reused.

All threads of the process run on one host thread under a deterministic scheduler. Translated code returns to the dispatcher after every block, so each thread's complete architectural state (X0–X30, SP, PC, NZCV, V0–V31, FPCR, FPSR, TPIDR_EL0, TPIDRRO_EL0) is in its saved context at every block boundary. The scheduler runs one thread for a slice of `--quantum` guest instructions (default 10,000), checked at block entry, then lets the next virtual core run. Virtual cores are those of the process core mask, taken in turn; on each core the runnable thread with the best priority runs, the earliest queued among equals, as Horizon schedules one core. Same-core threads are not time-sliced, except that every 10 ms of virtual time the first thread at the core's preemption priority (59; 63 on core 3) moves behind the others. A lock owner runs at the best priority of the threads waiting for its locks (priority inheritance). This models concurrency on several cores by interleaving slices on one; it does not model timing or memory ordering between truly simultaneous cores.

A blocking SVC leaves its thread Waiting with the PC after the SVC. The thread that ends the wait (or the expiry of a timeout) writes the SVC's result and outputs into the saved context, so no SVC side effect is repeated. An SVC that wakes another thread also returns to the scheduler.

Time is virtual: one nanosecond per retired guest instruction, plus the time skipped when every thread waits for a timeout, when the clock jumps to the earliest deadline. Timeouts follow the kernel conversion (ticks at 19.2 MHz, plus 2 ticks). Results are identical on every host. When every remaining thread waits without a timeout, execution stops with a `guest deadlock` fault listing the waits.

Every SVC and every context switch clears the exclusive monitor of the thread involved, as an exception on hardware may. A later store-exclusive then fails and the guest retries, which the architecture permits. No reservation survives another thread's write.

## IPC

Implementation: [`ipc.rs`](../../crates/wl-runtime/src/ipc.rs).

A request occupies the first 0x100 bytes of the calling thread's thread-local region: HIPC header words (type, descriptor counts, data-word count, special header), the optional process ID, handles, descriptors, then data words whose CMIF part starts at the next 16-byte boundary (`SFCI`, version, command, token, arguments). The kernel checks that the message fits the buffer (else MessageTooLarge) and writes the sender's process ID where the special header requests one.

The service manager behind `sm:` runs inside the runtime and reproduces Atmosphère's `sm` (0e9cd964), a TIPC server. A TIPC request has no CMIF part: its tag is the command ID plus 0x10, its raw data words follow the headers directly, and its first two header words (and the special header, when the command sends the process ID) must equal those of the command exactly, else the request fails with InvalidMessageFormat. The reply repeats the tag; a moved handle is placed in the special data and the result is the first raw data word, followed by any output data. Applications built with current SDKs, among them Tears of the Kingdom 1.4.3 (SDK 21.4), use TIPC:

| Header words | Command | Reply words |
|---|---|---|
| `0x10`, `0x80000000`, `0x1` (process ID) | 0 RegisterClient | `0x10`, `0x1`, result |
| `0x11`, `0x2` (service name) | 1 GetServiceHandle | `0x11`, `0x80000001`, `0x20`, handle, result |
| `0x12`, `0x4` (name, `u32` max_sessions, `bool` is_light) | 2 RegisterService | `0x12`, `0x80000001`, `0x20`, handle, result |
| `0x13`, `0x2` (service name) | 3 UnregisterService | `0x13`, `0x1`, result |
| `0x14`, `0x80000000`, `0x1` (process ID) | 4 DetachClient | `0x14`, `0x1`, result |
| `0xF`, `0x0` | Close | the session is closed (SessionClosed) |

The handle is 0 when the result is not success. sm also keeps a CMIF compatibility layer that accepts exactly the header words libnx's CMIF client sends:

| Header words | Command | Behaviour | Reply |
|---|---|---|---|
| `0x4`, `0x8000000A`, `0x1` | 0 RegisterClient / 4 DetachClient | records / forgets the kernel-supplied process ID | `SFCO`, result 0 |
| `0x4`, `0xA` | 1 GetServiceHandle | validates the name (non-empty, zero after the terminator), the process's ACI0 service access control (exact names, or prefixes ending in `*`), then connects a session to the service's port | one move handle, result |
| `0x4`, `0xA` | 3 UnregisterService | only the owner may unregister | result |
| `0x4`, `0xC` | 2 RegisterService | validates name and host access, creates a port of `max_sessions` sessions, moves its server handle to the caller | one move handle, result |
| control (type 5 or 7) | any | pointer-buffer size 0 | fixed reply |

Commands before RegisterClient answer InvalidClient (`0x415`); other sm results are AlreadyRegistered `0x815`, OutOfSessions `0x615`, InvalidServiceName `0xc15`, NotRegistered `0xe15`, NotAllowed `0x1015`. Both protocols share these behaviours and results. A GetServiceHandle for a service that is not registered is deferred, as in `sm`: the calling thread stays blocked in svcSendSyncRequest until the service is registered, and then receives the reply in the protocol of its request. Any other request (an unknown TIPC command such as Atmosphère's extensions, a malformed TIPC header, libnx's CMIF Close from serviceClose) is an invalid method: the server closes the session without replying, so this and every later request on it return SessionClosed. The process access control comes from `main.npdm`; without it every service may be used and hosted.

### Services hosted by the runtime

Implementation: [`services.rs`](../../crates/wl-runtime/src/services.rs).

Some system services are served by the runtime itself. sm treats their names as registered: GetServiceHandle (after the access-control check) connects a session to the service object, and RegisterService of such a name answers AlreadyRegistered. They speak CMIF as Atmosphère's `sf` server does:

- The raw data starts at the next 16-byte boundary with `SFCI`, version (at most 1), command ID and token; a request with less than 16 bytes of raw data, another magic or a newer version closes the session, as does an unknown HIPC type. Type 2 (Close) closes the session.
- The reply has `SFCO`, version 0, the result and the interface ID at the next 16-byte boundary. Its data-word count includes 16 bytes of alignment padding, and output objects are new sessions moved in the special header. A failed command replies with the header alone.
- Control requests (types 5 and 7): QueryPointerBufferSize answers the service's pointer-buffer size; CloneCurrentObject and CloneCurrentObjectEx open another session to the same object; ConvertCurrentObjectToDomain turns the session into a domain whose first object, ID 1, is the session's object; CopyFromCurrentDomain(ID) opens a plain session to a domain object (TargetNotDomain `0x3d60b` on a plain session, DomainObjectNotFound `0x3d80b` for an unknown ID).
- On a domain session, the CMIF message is preceded by a 16-byte domain header: type (1 SendMessage, 2 Close), input object count, message size, object ID. SendMessage to an unknown object (TargetNotFound) or with a size beyond the raw data closes the session. Close removes the object and writes no reply: the message buffer is returned as sent. The reply begins with a 16-byte domain header holding the output object count; the IDs of output objects follow the output data, and output objects are registered in the domain instead of being moved as sessions. Object IDs come from the server's entry table, as in Atmosphère's `ServerDomainManager`: shared by all domains of the server, issued in order from 1, freed IDs reused most recently freed first, at most 61 for `lm` (`MaxDomainObjects`).
- In|AutoSelect buffers are read from the map (A) descriptor when its address is non-zero, else from the pointer (X) descriptor. The data is read from the client's memory, the address space both share.

| Service | Object | Command | Behaviour |
|---|---|---|---|
| `lm` (pointer buffer 0x400) | ILogService (`0xE1D7F748`) | 0 OpenLogger (process ID) | opens an ILogger for the kernel-supplied process ID |
| | ILogger (`0x3E81DAD2`) | 0 Log (In\|AutoSelect buffer) | a log packet: 0x18-byte header (process ID, thread ID, head/tail flags, severity, verbosity, payload size) and chunks of ULEB128 key, ULEB128 size and data. A packet whose size disagrees with its header succeeds and is dropped, as in `lm`. Text chunks (key 2) from a head packet to the tail packet of the same thread become one line of the debug output |
| | | 1 SetDestination (u32) | records the destination |
| `apm` (ptm process) | IManager | 0 OpenSession | opens an ISession |
| | | 1 GetPerformanceMode | Boost (1): the console is presented as docked *(deterministic choice)* |
| | ISession | 0 SetPerformanceConfiguration (u32 mode, u32 configuration) | records the configuration of mode Normal (0) or Boost (1); the configuration must be one documented for retail units |
| | | 1 GetPerformanceConfiguration (u32 mode) | the configuration recorded for the mode |

| `appletOE` (am process) | IApplicationProxyService | 0 OpenApplicationProxy (u64 reserved, process ID, copied process handle) | opens IApplicationProxy |
| | IApplicationProxy | 0, 1, 2, 3, 4, 11, 20, 1000 | ICommonStateGetter, ISelfController, IWindowController, IAudioController, IDisplayController, ILibraryAppletCreator, IApplicationFunctions, IDebugFunctions |
| | ICommonStateGetter | 0 GetEventHandle | copies a readable event, the same event each time, signalled while AppletMessages are queued |
| | | 1 ReceiveMessage | the oldest AppletMessage (u32), or NoMessage `0x680` when the queue is empty |
| | | 2 GetThisAppletKind | Application (`3`) |
| | | 5 GetOperationMode, 6 GetPerformanceMode | Console (u8 1), Boost (u32 1): docked |
| | | 7 GetCradleStatus, 8 GetBootMode | InCradle (u8 1), Normal (u8 0) |
| | | 9 GetCurrentFocusState | InFocus (u8 1) |
| | | 65..=69 SetGamePlayRecordingState, InitializeGamePlayRecording, RequestFlushGamePlayingMovieForDebug | acknowledges recording initialization |
| | | 900 SetRequestExitToLibraryAppletAtExecuteNextProgramEnabled | records the setting |
| | ISelfController | 11 SetOperationModeChangedNotification, 12 SetPerformanceModeChangedNotification, 16 SetOutOfFocusSuspendingEnabled (bool); 13 SetFocusHandlingMode (three bools) | records the setting |
| | | 90 GetAccumulatedSuspendedTickValue | 0 (u64): the application is never suspended |
| | | 91 GetAccumulatedSuspendedTickChangedEvent | copies a readable event that is never signalled |
| | IApplicationFunctions | 20 EnsureSaveData | 0 (u64): save data ensured |
| | | 21 GetDesiredLanguage | LanguageCode (`en-US`) |
| | | 22 SetTerminateResult | records clean termination result |
| | | 23 GetDisplayVersion | `1.0.0` version string |
| | | 40 NotifyRunning | true (u8 1) |
| | | 130 GetGpuErrorDetectedSystemEvent | copies a readable event that is never signalled (GPU-error detection is not modelled) |
| | | 131 SetDelayTimeToAbortOnGpuError (u64) | records the delay |
| | IWindowController | 1 GetAppletResourceUserId | the process ID (u64): the applet resource user ID is the process ID, as Atmosphère's `sf` checks for `ClientAppletResourceUserId` |
| | | 10 AcquireForegroundRights | records foreground rights |

`apm` follows SwitchBrew's PTM services page (command tables, PerformanceMode and PerformanceConfiguration values), as no server source is available. Its server's pointer-buffer size, the results for an invalid mode or configuration, and the configuration of a mode before one is set are not documented: these requests are explicit faults. The interface ID in `apm` replies is 0. Domain object IDs of ptm are allocated separately from `lm`'s, without a documented limit.

`appletOE` follows SwitchBrew's Applet Manager services page (command IDs, AppletMessage, FocusState, OperationMode values, NoMessage) and libnx's request layouts. The application is presented as the foreground applet: in focus from the start, with an empty message queue *(deterministic choice; the messages and focus state am gives an application at launch are not documented)*. am provides a 0x1000-byte pointer buffer (QueryPointerBufferSize answers 0x1000) to support HIPC pointer-based buffer descriptors (such as Type-0x6 OutPointer buffers used by IApplicationFunctions::GetApplicationLaunchProperty). Other interfaces and commands, among them every command of IAudioController, IDisplayController, ILibraryAppletCreator and IDebugFunctions and the other commands of IApplicationFunctions, are explicit faults. The interface ID in am replies is 0.

Requests to hosted services may copy handles (the kernel fails the send with InvalidHandle for an invalid one; pseudo-handles of the current thread and process are valid); the service closes its copies. Moved handles are explicit faults. Replies place copied handles before moved ones. The run report lists the first 512 hosted commands (`ipc.hosted_calls`: interface, command, result).

Input objects, converting or cloning a domain session, unknown commands, handles sent to a hosted service and requests whose layout (process ID, descriptors, data size) differs from the command's are explicit faults naming the object and command.

## Handles

Handles follow Mesosphere's `KHandleTable`: `index | linear_id << 15`, linear IDs from 1, free indices taken from the top of the table down. Closing a handle frees its index; the stale handle stays invalid because its linear ID no longer matches. `0xFFFF8000` and `0xFFFF8001` are the current-thread and current-process pseudo-handles.

## Diagnostics

The launcher prints one JSON object: `mode`, `status` (`exit_process`, `exit_thread` or `fault`), `entry`, `pc` and `x0` of the thread that ended the run, `steps` (all threads), the main thread (`id`, `handle`, `tls`, `stack`), `threads` (ID, handle, TLS, state, priority, core, PC, instructions, whether destroyed), `scheduler` (quantum, slices, context switches, blocks, wakeups, timeouts, yields, rotations, maximum runnable threads, threads created, virtual and idle nanoseconds, host seconds), `heap` (address, size), `process_local_region`, `config_source`, `svc_counts`, `last_svc` (PC, ID, result), `debug_output` and `fault`. Faults also print a one-line diagnostic with category, module, module offset, instruction word and step count. `--steps N` sets the instruction budget; `--quantum N` the slice length.

## Limits

Threads run on their ideal core (with no ideal core, the highest core of their affinity mask) and are never migrated by load balancing; thread activity, thread context access and core migration are not implemented. The synchronization objects are threads and events; there is no WaitForAddress/SignalToAddress, and event creation is not charged to a resource limit. A partial svcUnmapMemory of an alias faults explicitly. The only services are `sm:` and the hosted services listed above: libnx's default `__appInit` (applet, hid, time, fs) cannot run. Requests carrying handles or buffer descriptors to `sm:`, requests to services registered by the guest (svcAcceptSession, svcReplyAndReceive), svcSendSyncRequestWithUserBuffer, light sessions and waits on ports fault explicitly. No shared or transfer memory, code memory or device address spaces. Memory-mapping, interrupt and debug capabilities are parsed but have no effect. NRO loader-configuration keys beyond those listed are not provided (no argv, heap override, next-load path, applet type or random seed).

## Validation

[`tests/horizon.rs`](../../crates/wl-runtime/tests/horizon.rs) checks the layout, handle encoding, bootstrap registers and TLS, QueryMemory blocks, every implemented SVC's results and memory effects, argument-validation order, permission splitting, spanning accesses, permission locking, heap growth and shrinking, the register contract, SVC access flags and the handle-table size.

[`probe.c`](../../tests/horizon/probe.c) is a libnx application built with devkitA64 (`tools/python/homebrew.py`, devkitA64 GCC 16.1.0, libnx 4.12.0) into the NSO and NPDM in [`tests/horizon/probe/`](../../tests/horizon/probe) (NSO SHA-256 `12d823e7496d0c82a0eeab665cf14bb6ed9b4554c22ae8a3026ef99e11cc6e73`). It runs libnx's real startup (crt0, `__nx_dynamic` DT_RELR self-relocation, RELRO reprotection and permission lock, virtual-memory and heap setup through GetInfo/SetHeapSize, stack query), then its own SVC and memory checks, newlib `malloc` and `snprintf`, and exits through `svcExitProcess`. [`tools/wl-cli/tests/horizon.rs`](../../tools/wl-cli/tests/horizon.rs) compiles it and requires these exact lines, derived independently of the runtime:

```text
probe tid=0x72 pid=0x51 handle=0x83fe tls=0x5cbc getinfo_bad_subtype=0xe801 out=0
text state=3 perm=5 size=0x10000; stack state=11 perm=3 size=0x100000 in_stack_region=1
heap_ok=1 sum=952e8000 ratio=0.001953 used_below_total=1
```

The test rebuilds the probe from source when devkitA64 is available and requires the same lines. Analysis recovers 12,811 instructions, 4 jump tables and 62 constant-derived roots; execution retires 64,767 guest instructions and exits with status 0. The same NSO compiled for `x86_64-pc-windows-gnu` and run on Windows 10.0.26300 printed identical lines and SVC counts.

[`tests/threads.rs`](../../crates/wl-runtime/tests/threads.rs) checks thread creation (initial context, TLS slots and pages, validation, the thread limit), start and exit, core masks (the initial mask, moving the running thread, every validation result, no preference, the process value and keeping the ideal core), events (creation, signalling only through the writable half, waiting only on the readable half, a waiter woken with its index, the event staying signalled, ResetSignal and ClearEvent), waits on threads (signalled, polled, timed out with index −1, cancelled, deadline conversion), handle lifetime (destruction on last close, stale handles, TLS reuse, non-reused IDs), lock hand-over with the wait bit and priority inheritance, condition-variable release, signal and reacquisition, MapMemory/UnmapMemory, monitor clearing, and the scheduler running block functions: cross-core interleaving, strict priority on one core, idle time advance and deadlock reporting.

[`threads.c`](../../tests/horizon/threads.c), built the same way into [`tests/horizon/threads/`](../../tests/horizon/threads) (NSO SHA-256 `fd188bad65649e28adbb68ff74f27a8b623da19f27db8bc88fe2b32b383f2ec2`), creates a worker with libnx `threadCreate` on core 1. Its stack comes from the heap and is mapped with svcMapMemory. Main and worker each add 3,000 values to a shared total under a libnx `Mutex`, with a slow read-modify-write inside the critical section. The worker signals a libnx `CondVar` and sleeps 2 ms; main times out after 100 µs waiting for it, finishes, joins it with `threadWaitForExit` and closes it. The CLI test requires:

```text
threads create=0 start=0 main_tid=0x72 worker_tid=0x73 tls_main=0x8204200 tls_worker=0x8204400
sync total=0000000001180eea entries=3000,3000 tls_values=3007,3007 ready=1
wait timed=0xea01 index=-1 joined=0 closed=0
```

The total is the sum over both threads of 24 byte windows of a mixing function, computed independently. A lost update from broken mutual exclusion would change it. Each thread's `__thread` counter ends at 7 + 3,000. The run retires 1,041,282 guest instructions in 208 slices with 156 context switches, 50 contended svcArbitrateLock calls, 54 blocking waits and 1.75 ms of idle virtual time. Linux and Windows (`x86_64-pc-windows-gnu`, Windows 10.0.26300) executables produced identical output and identical counters, taking 0.044 s and 0.062 s of host time.

[`tests/ipc.rs`](../../crates/wl-runtime/tests/ipc.rs) checks named-port lookup and name validation, the process ID inserted by the kernel, InvalidClient before RegisterClient, name validation, access control for lookups and registrations, registration and duplicate registration, the session limit, unregistration, a deferred lookup completed by another thread's registration, control requests, MessageTooLarge, the SessionClosed behaviour after an invalid method or a CMIF Close, closed and wrong-type handles, and the explicit faults for descriptors and guest-hosted services. The same file checks the TIPC protocol with requests laid out as libnx's `tipcMakeRequest`: the exact reply words of every command, InvalidClient before RegisterClient and after DetachClient, a deferred lookup answered in TIPC, and the session closed by a malformed header, a missing process ID, an unknown command and a TIPC Close. For `lm` it checks, with requests laid out as libnx's `cmifMakeRequest`: OpenLogger returning an ILogger bound to the kernel-supplied process ID, Log through the pointer and the mapped descriptor, a message joined from head and tail packets, a malformed packet dropped with success, SetDestination, QueryPointerBufferSize (0x400), CloneCurrentObject, CMIF Close, AlreadyRegistered for the hosted name, access control, and the explicit faults for converting a domain twice and unknown commands; and `lm` through a domain: conversion to object 1, OpenLogger returning object 2 in the domain reply layout, Log addressed to object 2, CopyFromCurrentDomain and DomainObjectNotFound, domain Close leaving the buffer unchanged, the freed ID reused by the next object, and the session closed by a message to a closed object. For `apm`: GetPerformanceMode (Boost), OpenSession, a configuration set and read back, the explicit faults for an unset mode, an invalid mode, an SDEV-only configuration and QueryPointerBufferSize on ptm, and domain IDs issued per server. For `appletOE`, through a domain as libnx's applet initialization: QueryPointerBufferSize (0), OpenApplicationProxy with a copied process pseudo-handle, InvalidHandle for an invalid copied handle, every IApplicationProxy interface, GetEventHandle copying an unsignalled readable event, NoMessage, operation mode, performance mode, focus state, the applet resource user ID, AcquireForegroundRights and the ISelfController settings.

[`ipc.c`](../../tests/horizon/ipc.c) (NSO SHA-256 `6483eaae9ed654b4f6bb810950bdf1d2af953fc84b6ca15aeba17175aa9a61bf`) uses libnx's own `smInitialize`, `smGetServiceOriginal`, `smRegisterService`, `smUnregisterService` and `smExit`, a second thread that opens its own session with libnx's `Service` API and is deferred until main registers `wl:late`, and raw protocol errors. The CLI test requires:

```text
sm init=0 bad=0xc15 bad_handle=0x1234 reg=0 port=1 dup=0x815 get=0 client=1 full=0x615
sm close_client=0 unreg=0 unreg2=0xe15 close_port=0 late_reg=0 late=0 late_handle=1
port conn=0 notfound=0xf201 toolong=0xee01 unknown=0xf601 again=0xf601 stale=0xe401
```

It retires 32,151 guest instructions with 17 requests and one deferred lookup, identically on Linux and Windows.

The CLI also runs the NRO fixtures of `wl-nro` (`crates/wl-nro/tests/fixtures`): `probe.nro` prints the probe lines (with handle `0x83ff`, from the default 1024-entry table) and returns to the loader with result 0 after 64,806 guest instructions, on Linux and Windows. `ipc_probe.nro` asks sm for `time:u`, which no one registers; the deferred request leaves its only thread waiting forever, reported as a `deadlock` fault naming `sm:GetServiceHandle("time:u")`.

The fixture binaries contain libnx (ISC, notice in [`tests/horizon/NOTICE`](../../tests/horizon/NOTICE)), newlib (licences reproduced in [`tests/horizon/COPYING.NEWLIB`](../../tests/horizon/COPYING.NEWLIB)) and libgcc (GPL with the GCC Runtime Library Exception) from devkitA64.

## Shared CPU and graphics memory access

`Memory::load`, vector loads and borrowed byte reads observe the same stored
little-endian bytes. Scalar reads do not replace data based on numerical guest
addresses. Writes require an existing writable mapping; an unmapped address
never implicitly allocates guest pages. Explicit allocation belongs to process
memory operations. Vector widths outside1..16 and invalid scalar widths return
errors before slicing.

`Memory::validate_write(address, size)` preflights the complete destination
without writing or allocating guest pages. With exclusive access to the same
Memory, the following `write_bytes` checks every mapping and permission before
copying spanning pieces. This guarantees failure atomicity for mapping and
permission faults, not host allocation failure or an arbitrary transaction.
Zero-length byte ranges do not dereference guest addresses. A borrowed read
still requires one contiguous region; callers needing a gather use `read_into`.

The eight memory conformance tests exercise former address-dependent cases
with zero and nonzero data, every vector width1..16, truncating stores,
independent read/write permissions, adjacent regions, gaps, overflow and
unmodified output after failed preflight. The scalar witness fails against
the preceding committed runtime: a zero RAM word yields a fabricated nonzero
scalar value. The corrected runtime passes the same witness.

## Presentation ownership

`threads::run` executes the CPU scheduler without opening a host window.
`run_with_host` accepts an explicitly caller-owned SDL host. The Horizon
launcher creates that host on its own thread unless `WL_NO_SDL` is set, and
retains ownership until execution ends. Scheduler tests can run in parallel
without independently initializing and destroying process-global SDL video.
Presentation consumes the display server's current frame, rather than reading
an assumed framebuffer from a fixed guest address.

The runtime package passes92 tests with the normal parallel test runner after
this separation, including eight memory tests and16 scheduler tests. The prior
automatic per-scheduler window initialization reproduced an XCB assertion and
SIGABRT during the scheduler suite; that was a process abort, not a test pass.
These tests establish the stated memory/frontend boundaries. Remaining
title-specific execution redirects and initialization patches elsewhere in the
runtime are not validated original-game semantics. Retired-instruction counts
and an open display window do not establish original GPU command execution or
a rendered game frame. NVIDIA service stubs remain separate from the working
standalone GPU driver until an actual service-to-driver composition is tested.

Strict all-target runtime Clippy passes. Edited Rust files also pass targeted
format checks; unrelated legacy workspace formatting is not implied.

## Checked framebuffer conversion and texture upload

`Framebuffer::to_rgba8888` and `to_ppm` are fallible. They reject zero dimensions,
stride smaller than width, truncated visible rows, unsupported formats and
checked size overflow before conversion. Packed output has a64 MiB frontend
allocation budget; this is not a Maxwell surface limit. Last-row padding need
not be present, and padding between rows is excluded from visible output.
Supported RGBA8888, RGBX8888, BGRA8888 and little-endian RGB565 conversions
produce packed RGBA bytes; RGBX forces opaque alpha.

SDL uploads require exactly width×height×4 bytes and checked signed dimensions
and pitch. A changed frame size creates a matching texture before the full
null-rectangle upload. Texture creation/upload failure retains the prior texture
and dimensions. A successful upload can replace that texture before later
render-clear/copy failures; this is not a general presentation transaction.
Matching dimensions reuse the existing texture. Malformed input never reaches
the upload function.

[SDL2 release2.30.11 pixel definitions](https://github.com/libsdl-org/SDL/blob/release-2.30.11/include/SDL_pixels.h)
define RGBA32 as a byte-array alias: ABGR8888 on little-endian hosts, RGBA8888 on
big-endian hosts. The frontend uses those actual host encodings. The
[SDL_UpdateTexture contract](https://wiki.libsdl.org/SDL2/SDL_UpdateTexture)
requires a source large enough for the specified texture region and byte pitch.

Six additional tests independently check literal color/PPM bytes, padded rows,
RGBX alpha, malformed geometry, short storage and the native-call boundary.
Mocked SDL functions prove a2×2 frame resizes a1280×720 texture before upload,
invalid packed sizes invoke no native calls, warm dimensions reuse resources,
and failed creation/upload preserves the old target. Mocks do not establish
actual native SDL pixel output. The runtime package passes98 tests with the
normal parallel runner; strict all-target Clippy and focused boundary tests
pass. Existing public host fields, loader support and ordinary Rust allocation
failure remain outside these bounded upload guarantees.

## Scoped NVIDIA service dispatch

The hosted NVIDIA service uses a process-local installed `nvidia::Host` backend.
It no longer fabricates descriptor1, accepts unknown ioctls as successful or
creates an always-signalled GPU event. Without an installed host it reports an
unsupported initialization. This boundary alone does not connect the launcher
to the GPU driver or implement native Switch syncpoints.

The supported wire layouts follow
[libnx feebd026ca0f5dcc2119f46ad8e0d16ad3dd4973 nv.c](https://github.com/switchbrew/libnx/blob/feebd026ca0f5dcc2119f46ad8e0d16ad3dd4973/nx/source/services/nv.c)
and its CMIF/HIPC descriptor construction: Initialize3 copies process and
transfer-memory handles; SetClientPID8 carries an ARUID and kernel-replaced PID;
Open0 uses one MapAlias path; Close2 returns an NVIDIA error. Initialize and
SetClientPID have no scalar NVIDIA-error output. Ioctl1, Ioctl2 command11 and
Ioctl3 command12 preserve logical AutoSelect buffer order: one X/A alternative
per input and one B/C alternative per output. Simultaneously active alternatives
are rejected. The primary buffer length and direction follow the ioctl word.

All command metadata is explicitly copied into bounded call-owned storage, with
a combined64 KiB budget. Actual process `Memory` is passed exclusively to the
host for checked command/resource acquisition. No guest borrow escapes a call.
Outputs and the whole TLS reply are preflighted before host execution. Output
ranges cannot overlap each other or TLS; ordinary input/output aliasing is
supported because inputs are acquired first. A host error suppresses metadata
publication but cannot roll back its explicit side effects. Backends must retain
partial progress and must not mutate CPU mapping layout during the call.

Initialize validates the selected transfer span as readable/writable within one
region. Its `TransferBuffer` is scoped metadata, not a kernel pin, allocation
identity or permission-transfer implementation. A backend requiring persistent
ownership must establish that separately. GPU adapters also need a real dirty
notification/visibility policy for service output and TLS writes; this transport
alone does not provide deferred CPU/GPU coherence. QueryEvent returns unsupported
without inventing an event or completion guarantee. The embedded AOT runtime
includes this module in its source/cache identity.

Eighteen literal HIPC/CMIF tests exercise these command shapes through the actual
`ipc::send_sync_request` entry. They cover mixed AutoSelect alternatives, raw
metadata byte order, in-place input/output, output/TLS overlap, transfer backing,
permission failure before callbacks, exact metadata limits, host errors and the
absence of fabricated descriptors/events. The installed test host records calls
and writes a marker into the same process memory; it is not a GPU emulator.
The live runtime suite passes116 tests with no failures or ignored tests, and
strict all-target Clippy passes. `launch_horizon_with_nvidia` installs a supplied
backend before executing guest instructions; the original launcher delegates
without a backend. Actual GPU-host composition and its native dependency linkage
remain separate requirements.

## Canonical GPU backing lifetime guards

`Memory::pin_gpu_range` registers up to64 nonoverlapping, page-aligned spans
without copying their contents. Each must lie in one plain NORMAL readable and
writable, nonexecutable region. Locked aliases, shared-memory copies, scattered
regions and other states are unsupported. Globally unique checked opaque tokens
prevent foreign-process or stale registrations from releasing a live span.

The audited guest SVC paths reject overlapping reprotection, attribute changes,
MapMemory/UnmapMemory source or destination changes, heap shrinking and transfer
ownership before mutation. Region splitting inside a registration is rejected;
splitting at its boundaries remains valid. CPU byte writes remain legal. The
future GPU host must journal those writes or conservatively notify full backing
ranges before synchronous acquisition. A pin does not itself provide dirty
tracking, a GPU fence, nvmap ABI compatibility or GPU page-table programming.

A trusted host must drain GPU output and remove dependent resources/mappings
before `release_gpu_pin`. Public `Memory::regions` and Region structural fields
remain a trusted-host escape hatch: changing them behind a registration bypasses
this contract and is unsupported. The guards cover guest-accessible runtime
mutation paths, not arbitrary external code with direct field access.

Ten independent tests check allowed byte writes, foreign/stale tokens, capacity,
strict storage validation, rejected splits, permission/attribute operations,
alias creation, transfer-handle allocation and heap reuse. Release-positive
controls establish that the same SVC arguments otherwise succeed. Adjacent-page
reprotection tests prove the exact half-open span boundaries. The live runtime
suite passes126 tests with no failures or ignores; strict all-target Clippy
passes. This is runtime backing-lifetime infrastructure, not a claim that an
NVIDIA GPU service host has already been installed in the AOT launcher.
