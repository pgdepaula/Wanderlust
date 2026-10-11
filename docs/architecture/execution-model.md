# Execution model

## Implemented pipeline

```text
package / ExeFS directory / NSO / raw A64
   │  wl-package  (intake: identify, verify; read-only)
   │  wl-nso      (NSO0 segments, MOD0, dynamic tables)
   ▼
wl-loader  ── ldr layout, rtld-equivalent linking ──▶ Process (mappings, diagnostics, code pointers)
   ▼
wl-analysis ── recursive traversal from required + speculative roots ──▶ Program (blocks)
   ▼
wl-codegen-rust ── one Rust function per block, one crate per guest-address window ──▶ cached rlibs (parallel rustc)
   ▼                                                                                   │
wl-runtime (rlib): CPU and SIMD&FP state, guest memory, dispatch, FP semantics ◀──────┘──▶ executable crate ──▶ native executable
```

`FileOffset`, `ModuleOffset` and `GuestAddr` distinguish on-disk positions, module-relative positions and process virtual addresses throughout.

**Intake.** [`wl-package`](../../crates/wl-package/src/lib.rs) identifies PFS0/NSP archives, NCA headers (plaintext or encrypted), tickets (non-secret fields only), CNMT XML, NPDM, NSO files and ExeFS directories, and correlates them (content records, application/patch relationship). It never decrypts and never modifies inputs. See [supplied content](../game/content-layout.md).

**Single-module images.** `wl-analysis::Image::nso` maps one NSO at an explicit base and joins zero-filled BSS to data; `Image::raw` maps original A64 words. A MOD0 module whose dynamic table holds relocations is marked `RelocationState::NotApplied`, and code generation refuses it.

**Processes.** A directory input is loaded by [`wl-loader`](../../crates/wl-loader/src/lib.rs) as a process: `ldr` layout from `0x8000000`, page-granular segment permissions, and linking in `host` or `guest` mode, with structured diagnostics and an `execution_blocker` when the image must not run. The process entry is the start of the first module's text. See [module loading](../platform/module-loading.md).

**Analysis.** `wl-analysis::discover_roots` decodes recursively from two root classes. *Required* roots (the entry and `--seed` addresses) must be traversable; a failure is an analysis error. *Speculative* roots (relocation-derived code pointers, dynamic function symbols, initializers, and `--roots-file` addresses) are each traversed into a private set and merged only if traversal never leaves executable memory or overflows; otherwise the root and everything reached only through it are discarded and reported in `rejected_roots`. Direct branch destinations, conditional fallthrough, call destinations and return sites are traversed; instructions after unconditional transfers are not assumed to be code. Leaders are roots and the successors of transfers. A one-million-instruction limit bounds analysis.

By default the first reached unsupported encoding is an error. With `--allow-unsupported` it terminates its block and becomes a recorded *unsupported site*. The output keeps distinct counts: supported instructions reached, unsupported sites reached, blocks, indirect sites, accepted and rejected speculative roots. `survey` is a separate linear sweep that also counts data and padding, with unsupported words grouped by Arm ARM encoding class; it is never a reachability metric.

**Code generation.** `wl-codegen-rust::emit` writes one host function per recovered block. Blocks starting in the same aligned 4 KiB guest window (`--unit-shift`) form a translation-unit crate exporting its sorted `(guest address, function)` table; the executable crate lists the unit tables, the module map and the memory images ([ADR-0004](../decisions/ADR-0004-separate-translation-units.md)). Each block contains instruction-specific Rust with constant guest addresses and operands; direct transfers assign the destination PC, BL/BLR set X30, and RET/BR/BLR use the same tables. An unsupported site compiles to a fault reporting its opcode, encoding class and PC. Guest memory images are written beside the executable crate as `EXECUTABLE.rs.<address>.bin` and embedded with `include_bytes!`, with trailing zero bytes restored at startup. No instruction is fetched or decoded during execution and no interpreter or JIT exists.

**Native build.** [`build`](../../crates/wl-codegen-rust/src/build.rs) compiles the runtime and every unit as rlibs (`opt-level=2`, overflow checks, `-D warnings`) with `--jobs` parallel rustc processes, then the executable crate. Each rlib is cached under the SHA-256 of its source, the runtime key, `rustc -vV`, target and flags (default cache `.wanderlust-cache` beside the executable, or `--cache-dir`/`WL_CACHE_DIR`); rebuilding an unchanged program reuses every rlib and changing one guest instruction recompiles one unit. The report lists hits, misses and per-unit compile time.

## Runtime contract

The dependency-free [`wl-runtime`](../../crates/wl-runtime/src/lib.rs) is linked into each program as an rlib. CPU state: X0–X30, V0–V31 (128-bit), SP, PC, NZCV (four bits), an instruction counter and budget, TPIDR_EL0, TPIDRRO_EL0, FPCR, FPSR and a local exclusive monitor; SIMD and floating-point semantics are in [`fp`](../../crates/wl-runtime/src/fp.rs) and [`simd`](../../crates/wl-runtime/src/simd.rs) ([specification](../platform/aarch64-simd-fp.md)). There is one execution thread; diagnostics report thread 0.

Scratch/stack memory is a 64 KiB RW mapping at `0x10000000`; initial SP is its upper bound. X30 starts at `0xfffffffffffffffc`, a host-return token; reaching it ends execution with `status=returned`. Other registers start at zero. These are execution-harness conventions, not Horizon's thread-entry ABI (rtld expects `X0 = 0`, `X1 =` main thread handle, an NPDM-sized stack and a TLS region addressed by TPIDRRO_EL0). Because the scratch region sits at `0x10000000`, a process image based at `0x8000000` larger than 128 MiB currently collides with it and is rejected at translation time.

Each block accounts for all its instructions on entry (`Cpu::enter`); the default budget is one million instructions, checked at block entry, so a block that would exceed it is not started. Instructions that can fault (memory accesses, SVC, traps, conversions with FPCR-dependent faults) store their PC first; after a fault the dispatcher recomputes the exact number of retired instructions, including the faulting one. A fault prints the JSON state and a diagnostic with category (`memory-unmapped`, `unsupported-instruction`, `unresolved-control-flow`, `unsupported-service`, `alignment`, `trap`, `budget`, …), module and module offset of the PC, the instruction word and the step count. Exit codes: configuration error 2, guest fault 1, controlled completion 0. The runtime accepts initial state through `--reg`, `--vreg`, `--sp`, `--nzcv`, `--fpcr`, `--fpsr`, `--steps` and `--memory`.

The only implemented Horizon operation is SVC 0x07, `ExitProcess` (`status=exit_process`). Every other SVC fails with its ID, PC and X0–X3. BRK/HLT fault with their immediate.

## Memory invariants

`Memory` owns disjoint `Vec<u8>` regions; guest addresses are never host pointers. Mapping and access endpoints use checked addition; accesses validate the complete range and permissions before reading or writing little-endian bytes. SIMD&FP register and structure accesses use the same checked accessors (`load_vector`, `store_vector`), never host pointers. Accesses across region boundaries are rejected even when regions are adjacent. Executable memory is immutable: self-modifying code and runtime code generation are unsupported. Exclusive and ordered accesses require natural alignment. No page table, aliasing, shared memory, MMIO, guest exception delivery or multi-threaded memory ordering is implemented.

## Compatibility limits

Execution is limited to the [implemented A64 set](../platform/aarch64.md) and to control flow recovered statically or from metadata. Indirect branches reach only recovered blocks. No Horizon kernel objects, IPC services, threads, TLS setup, graphics or audio exist. Translation cost remains the principal scaling limit: about 1.8 ms of compiler CPU per guest instruction, in parallel units of bounded memory ([ADR-0004](../decisions/ADR-0004-separate-translation-units.md)).

Implementation: [intake](../../crates/wl-package/src/lib.rs), [NSO/MOD0](../../crates/wl-nso/src/lib.rs), [dynamic metadata](../../crates/wl-nso/src/dynamic.rs), [loader](../../crates/wl-loader/src/lib.rs), [decoder](../../crates/wl-aarch64/src/lib.rs), [analysis](../../crates/wl-analysis/src/lib.rs), [code generator](../../crates/wl-codegen-rust/src/lib.rs), [runtime](../../crates/wl-runtime/src/lib.rs). Validation: [methodology](../testing/validation-methodology.md).
