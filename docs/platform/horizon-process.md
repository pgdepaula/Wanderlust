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
| 0x06 | QueryMemory | writes the 0x28-byte `MemoryInfo`; PageInfo 0 | InvalidPointer (115) if the output is not writable |
| 0x07 | ExitProcess | ends execution, `status=exit_process` | — |
| 0x0A | ExitThread | ends the only thread, `status=exit_thread` | — |
| 0x0B | SleepThread | returns immediately (one thread, no scheduler); no return value | — |
| 0x10 | GetCurrentProcessorNumber | the thread's ideal core | — |
| 0x16 | CloseHandle | frees a table handle; pseudo-handles are not closable | InvalidHandle (114) |
| 0x1E | GetSystemTick | host monotonic time since process start at 19.2 MHz | — |
| 0x24 | GetProcessId | process ID for the current-process pseudo-handle or a thread handle | InvalidHandle |
| 0x25 | GetThreadId | thread ID for the current-thread pseudo-handle or a thread handle | InvalidHandle |
| 0x26 | Break | fault, category `guest-break`, with reason and arguments | — |
| 0x27 | OutputDebugString | text to stderr as `[guest] …` and to `debug_output` (up to 64 KiB) | InvalidPointer |
| 0x29 | GetInfo | see below | InvalidCombination, InvalidHandle, InvalidEnumValue (120) |

An SVC whose bit is clear in the process's SVC access flags (or whose ID is 0xC0 or above) is a fault of category `svc-not-permitted`, where the kernel raises an exception instead of dispatching. Every other SVC not listed is a fault of category `unsupported-service` reporting the ID, PC and X0–X3; no SVC returns success without its effect.

`svcGetInfo` process types (0–7, 12–18, 20–24, 28, 36) require subtype 0 and the current-process pseudo-handle. Values: core mask (0), priority mask (1), Alias region (2, 3), Heap region (4, 5), memory limit (6, 21), used memory (7, 22: code, stack, heap and TLS pages), code region (12, 13), Stack region (14, 15), system resource 0 (16, 17), program ID (18), user exception context address (20; the process-local region, as in Mesosphere), is-application (23), free thread slots (24), 0 for 28 and 2^39 for 36. DebuggerAttached (8) requires handle 0 and returns 0. RandomEntropy (11) requires handle 0 and subtype < 4 and returns a value derived from the program ID (*deterministic choice*). Types the kernel defines but this runtime does not model (resource limit 9, idle and thread tick counts 10 and 25, initial process ID range 19, SVC permission 26, I/O and transfer-memory hints 27 and 34, shadow stack 37 and 38) fault with category `unsupported-service`; undefined types are InvalidEnumValue.

## Handles

Handles follow Mesosphere's `KHandleTable`: `index | linear_id << 15`, linear IDs from 1, free indices taken from the top of the table down. Closing a handle frees its index; the stale handle stays invalid because its linear ID no longer matches. `0xFFFF8000` and `0xFFFF8001` are the current-thread and current-process pseudo-handles.

## Diagnostics

The launcher prints one JSON object: `mode`, `status` (`exit_process`, `exit_thread` or `fault`), `entry`, `pc`, `steps`, `x0`, the main thread (`id`, `handle`, `tls`, `stack`), `heap` (address, size), `process_local_region`, `config_source`, `svc_counts`, `last_svc` (PC, ID, result), `debug_output` and `fault`. Faults also print a one-line diagnostic with category, module, module offset, instruction word and step count. `--steps N` sets the instruction budget.

## Limits

One thread; no scheduler, synchronization objects, events or `CreateThread`. No IPC (`ConnectToNamedPort`, `SendSyncRequest`) and therefore no services: applications must not depend on `__appInit` service initialization. No `MapMemory`, alias mappings, shared or transfer memory, code memory or device address spaces. Memory-mapping, interrupt and debug capabilities are parsed but have no effect. Only NSO entry semantics are implemented (NRO loader entry, with X0 = loader configuration and X1 = −1, is not).

## Validation

[`tests/horizon.rs`](../../crates/wl-runtime/tests/horizon.rs) checks the layout, handle encoding, bootstrap registers and TLS, QueryMemory blocks, every implemented SVC's results and memory effects, argument-validation order, permission splitting, spanning accesses, permission locking, heap growth and shrinking, the register contract, SVC access flags and the handle-table size.

[`probe.c`](../../tests/horizon/probe.c) is a libnx application built with devkitA64 (`tools/python/homebrew.py`, devkitA64 GCC 16.1.0, libnx 4.12.0) into the NSO and NPDM in [`tests/horizon/probe/`](../../tests/horizon/probe) (NSO SHA-256 `12d823e7496d0c82a0eeab665cf14bb6ed9b4554c22ae8a3026ef99e11cc6e73`). It runs libnx's real startup (crt0, `__nx_dynamic` DT_RELR self-relocation, RELRO reprotection and permission lock, virtual-memory and heap setup through GetInfo/SetHeapSize, stack query), then its own SVC and memory checks, newlib `malloc` and `snprintf`, and exits through `svcExitProcess`. [`tools/wl-cli/tests/horizon.rs`](../../tools/wl-cli/tests/horizon.rs) compiles it and requires these exact lines, derived independently of the runtime:

```text
probe tid=0x72 pid=0x51 handle=0x83fe tls=0x5cbc getinfo_bad_subtype=0xe801 out=0
text state=3 perm=5 size=0x10000; stack state=11 perm=3 size=0x100000 in_stack_region=1
heap_ok=1 sum=952e8000 ratio=0.001953 used_below_total=1
```

The test rebuilds the probe from source when devkitA64 is available and requires the same lines. Analysis recovers 12,811 instructions, 4 jump tables and 62 constant-derived roots; execution retires 64,767 guest instructions and exits with status 0. The same NSO compiled for `x86_64-pc-windows-gnu` and run on Windows 10.0.26300 printed identical lines and SVC counts.

The fixture binary contains libnx (ISC, notice in [`tests/horizon/NOTICE`](../../tests/horizon/NOTICE)), newlib and libgcc from devkitA64.
