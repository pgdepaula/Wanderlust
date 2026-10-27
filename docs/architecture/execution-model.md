# Execution model

## Implemented pipeline

```text
package / ExeFS directory / NSO / raw A64
   │  wl-package  (intake: identify, verify; read-only)
   │  wl-nso      (NSO0 segments, MOD0, dynamic tables)
   ▼
wl-loader  ── ldr layout, rtld-equivalent linking ──▶ Process (mappings, diagnostics, code pointers)
   ▼
wl-analysis ── traversal from required + speculative roots, proven jump tables / code pointers ──▶ Program (blocks)
   ▼
wl-codegen-rust ── one Rust function per block, one crate per guest-address window ──▶ cached rlibs (parallel rustc)
   ▼                                                                                   │
wl-runtime (rlib): CPU and SIMD&FP state, guest memory, dispatch, FP semantics, Horizon process + SVCs ◀──┘──▶ executable crate ──▶ native executable
```

`FileOffset`, `ModuleOffset` and `GuestAddr` distinguish on-disk positions, module-relative positions and process virtual addresses throughout.

**Intake.** [`wl-package`](../../crates/wl-package/src/lib.rs) identifies PFS0/NSP archives, NCA headers (plaintext or encrypted), tickets (non-secret fields only), CNMT XML, NPDM, NSO files and ExeFS directories, and correlates them (content records, application/patch relationship). It never decrypts and never modifies inputs. See [supplied content](../game/content-layout.md).

**Single-module images.** `wl-analysis::Image::nso` maps one NSO at an explicit base and joins zero-filled BSS to data; `Image::raw` maps original A64 words. A MOD0 module whose dynamic table holds relocations is marked `RelocationState::NotApplied`, and code generation refuses it.

**Processes.** A directory input is loaded by [`wl-loader`](../../crates/wl-loader/src/lib.rs) as a process: `ldr` layout from `0x8000000`, page-granular segment permissions, and linking in `host` or `guest` mode, with structured diagnostics and an `execution_blocker` when the image must not run. The process entry is the start of the first module's text. See [module loading](../platform/module-loading.md).

**Analysis.** `wl-analysis::discover_roots` decodes recursively from two root classes. *Required* roots (the entry and `--seed` addresses) must be traversable; a failure is an analysis error. *Speculative* roots (relocation-derived code pointers, dynamic function symbols, initializers, and `--roots-file` addresses) are each traversed into a private set and merged only if traversal never leaves executable memory or overflows; otherwise the root and everything reached only through it are discarded and reported in `rejected_roots`. Direct branch destinations, conditional fallthrough, call destinations and return sites are traversed; instructions after unconditional transfers are not assumed to be code. Leaders are roots and the successors of transfers. A one-million-instruction limit bounds analysis.

Indirect branches are resolved only where a forward constant and bounds propagation over the decoded graph proves their targets: bounded jump tables and code addresses formed by `ADR`/`ADRP`/`ADD` that are stored, passed to calls or branched to. Proposed targets are traversed as speculative roots; jump-table entries also become block leaders and graph edges. Analysis and traversal alternate until no new code appears ([ADR-0005](../decisions/ADR-0005-proven-indirect-targets.md)). Reports include `jump_tables` and `flow_roots`.

Unsigned masked operands can bound a multiply-add when the product and accumulator do not overflow the instruction width. Subsequent immediate addition or subtraction retains a bounded modular interval, including W-register underflow. Mapped entries of a partly known table become speculative candidates; unreadable entries and invalid targets prevent reporting a complete table. Unknown upper bits, overflowing products and nonconstant subtract-products do not establish an unsigned bound. Original loads and indirect branches remain unchanged.

`wanderlust dispatch-scan INPUT` examines every BR/BLR in executable mappings using bounded local control-flow windows of at most 64 instructions. It reuses the decoder, graph construction and propagation, preserving conditional-edge joins and call clobbers. JSON separates candidate addresses, complete local tables and unresolved sites. The scan is bounded to 128 million executable words, 262,144 indirect sites and 262,144 candidate addresses; exceeding a budget fails explicitly. Candidates can be supplied through `--roots-file` for ordinary traversal before emission. An advisory scan may enumerate the high-word-zero slice of an X-indexed table guarded by a W comparison. Such a slice always remains unresolved: the low-word bound is never promoted to a full-register proof. Ordinary recursive propagation retains its strict index bounds; original loads and branches are unchanged. This inventory does not prove runtime-generated virtual-call targets or global control-flow completeness.

`wanderlust function-scan INPUT` recovers function-entry candidates from MOD0's unwind-index bounds. The supported [LSB 4.1 exception-frame layout](https://refspecs.linuxfoundation.org/LSB_4.1.0/LSB-Core-generic/LSB-Core-generic/ehframechpt.html) uses version 1, a signed32 PC-relative frame pointer, an unsigned32 entry count and signed32 data-relative table entries. Every location must agree with its FDE and cover a nonempty aligned range in immutable executable memory. Complete metadata ranges, backward CIE references, augmentation lengths and supported version-one `zR`/`zPLR` fields are checked. Personality and LSDA values are not dereferenced or executed. Limits are 1,048,576 functions per process, 4,096 CIEs per index, 4 KiB declared CIE bodies and 1 MiB declared FDE bodies. Absent indexes remain null; malformed or unsupported indexes fail explicitly. The output supplies speculative roots for ordinary traversal; it does not implement exception unwinding or prove arbitrary runtime-generated targets.

By default the first reached unsupported encoding is an error. With `--allow-unsupported` it terminates its block and becomes a recorded *unsupported site*. The output keeps distinct counts: supported instructions reached, unsupported sites reached, blocks, indirect sites, accepted and rejected speculative roots. `survey` is a separate linear sweep that also counts data and padding, with unsupported words grouped by Arm ARM encoding class; it is never a reachability metric.

**Code generation.** `wl-codegen-rust::emit` writes one host function per recovered block. Blocks starting in the same aligned 4 KiB guest window (`--unit-shift`) form a translation-unit crate exporting its sorted `(guest address, function)` table; the executable crate lists the unit tables, the module map and the memory images ([ADR-0004](../decisions/ADR-0004-separate-translation-units.md)). Each block contains instruction-specific Rust with constant guest addresses and operands; direct transfers assign the destination PC, BL/BLR set X30, and RET/BR/BLR use the same tables. An unsupported site compiles to a fault reporting its opcode, encoding class and PC. Guest memory images are written beside the executable crate as `EXECUTABLE.rs.<address>.bin` and embedded with `include_bytes!`, with trailing zero bytes restored at startup. No instruction is fetched or decoded during execution and no interpreter or JIT exists.

**Native build.** [`build`](../../crates/wl-codegen-rust/src/build.rs) compiles the runtime and every unit as rlibs (`opt-level=2`, overflow checks, `-D warnings`) with `--jobs` parallel rustc processes, then the executable crate. Each rlib is cached under the SHA-256 of its source, the runtime key, `rustc -vV`, target and flags (default cache `.wanderlust-cache` beside the executable, or `--cache-dir`/`WL_CACHE_DIR`); rebuilding an unchanged program reuses every rlib and changing one guest instruction recompiles one unit. The report lists hits, misses and per-unit compile time.

Application launch configurations use the 96-thread base resource budget documented by [Atmosphère `pm_spec.cpp` at 0e9cd964](https://github.com/Atmosphere-NX/Atmosphere/blob/0e9cd964c094946df14926b6c81a631b603a30f0/stratosphere/pm/source/impl/pm_spec.cpp). This process-manager policy is separate from NPDM permissions and includes the main thread. Non-application standalone launches retain a conservative 32-thread implementation limit. Creation beyond the configured live-thread budget still returns the kernel resource-limit error; increased application capacity does not bypass priority, affinity, memory, handle or SVC checks.

## Runtime contract

The dependency-free [`wl-runtime`](../../crates/wl-runtime/src/lib.rs) is linked into each program as an rlib. CPU state: X0–X30, V0–V31 (128-bit), SP, PC, NZCV (four bits), an instruction counter and budget, TPIDR_EL0, TPIDRRO_EL0, FPCR, FPSR and a local exclusive monitor; SIMD and floating-point semantics are in [`fp`](../../crates/wl-runtime/src/fp.rs) and [`simd`](../../crates/wl-runtime/src/simd.rs) ([specification](../platform/aarch64-simd-fp.md)). Harness mode runs one thread. In process mode every guest thread has its own saved CPU state and a deterministic scheduler interleaves them on one host thread, switching only at block boundaries ([threads and scheduling](../platform/horizon-process.md#threads-and-scheduling)).

An executable runs in one of two modes, fixed at translation time. **Process mode** (`recompile DIR --horizon`) starts the loaded process as Horizon starts an application's main thread and serves Horizon SVCs; see [Horizon process environment](../platform/horizon-process.md). **Harness mode** (the default) is the register-level test environment below, used by the differential suite and single-module tests.

In harness mode, scratch/stack memory is a 64 KiB RW mapping at `0x10000000`; initial SP is its upper bound. X30 starts at `0xfffffffffffffffc`, a host-return token; reaching it ends execution with `status=returned`. Other registers start at zero. These are execution-harness conventions, not Horizon's thread-entry ABI (rtld expects `X0 = 0`, `X1 =` main thread handle, an NPDM-sized stack and a TLS region addressed by TPIDRRO_EL0). Process mode implements that ABI. Because the harness scratch region sits at `0x10000000`, a harness-mode process image based at `0x8000000` larger than 128 MiB collides with it and is rejected at translation time; process mode maps no scratch region.

Each block accounts for all its instructions on entry (`Cpu::enter`); the default budget is one million instructions, checked at block entry, so a block that would exceed it is not started. Instructions that can fault (memory accesses, SVC, traps, conversions with FPCR-dependent faults) store their PC first; after a fault the dispatcher recomputes the exact number of retired instructions, including the faulting one. A fault prints the JSON state and a diagnostic with category (`memory-unmapped`, `unsupported-instruction`, `unresolved-control-flow`, `unsupported-service`, `alignment`, `trap`, `budget`, …), module and module offset of the PC, the instruction word and the step count. Exit codes: configuration error 2, guest fault 1, controlled completion 0. The runtime accepts initial state through `--reg`, `--vreg`, `--sp`, `--nzcv`, `--fpcr`, `--fpsr`, `--steps` and `--memory`.

In harness mode the only implemented SVC is 0x07, `ExitProcess` (`status=exit_process`); every other SVC fails with its ID, PC and X0–X3. Process mode implements the [listed SVCs](../platform/horizon-process.md#svcs) and fails explicitly on the rest. BRK/HLT fault with their immediate.

## Memory invariants

Region lookup retains two recent region indices. Every hit checks the current
region's bounds; permissions are checked by the access operation. The cache retains
neither host pointers nor permission snapshots, so insertion, removal, splitting and
backing replacement cannot authorize an access using stale metadata. Relaxed atomic
indices permit concurrent read-only lookups without changing ownership of memory.

`Memory` owns disjoint `Vec<u8>` regions; guest addresses are never host pointers. Mapping and access endpoints use checked addition; accesses validate the complete range and permissions before reading or writing little-endian bytes. SIMD&FP register and structure accesses use the same checked accessors (`load_vector`, `store_vector`), never host pointers. An access spanning adjacent regions succeeds only if every byte is mapped and grants it; it is checked completely before any byte moves. Each region carries a Horizon memory state and attribute; process mode splits regions at page boundaries when permissions or attributes change. Executable memory is immutable: self-modifying code and runtime code generation are unsupported. Exclusive and ordered accesses require natural alignment. No aliasing, shared memory, MMIO, guest exception delivery or multi-threaded memory ordering is implemented.

## Compatibility limits

Execution is limited to the [implemented A64 set](../platform/aarch64.md) and to control flow recovered statically or from metadata. Indirect branches reach only recovered blocks. Horizon services have explicit supported subsets ([runtime contract](../platform/horizon-process.md)); `--native-gpu` connects the bounded NVIDIA channel and Maxwell backend described in [graphics execution](../graphics/execution.md). These interfaces do not establish original-game frame compatibility. Translation cost and compiler working sets remain scaling constraints; unchanged runtime and unit identities can reuse native archives ([ADR-0004](../decisions/ADR-0004-separate-translation-units.md)).

Implementation: [intake](../../crates/wl-package/src/lib.rs), [NSO/MOD0](../../crates/wl-nso/src/lib.rs), [dynamic metadata](../../crates/wl-nso/src/dynamic.rs), [loader](../../crates/wl-loader/src/lib.rs), [decoder](../../crates/wl-aarch64/src/lib.rs), [analysis](../../crates/wl-analysis/src/lib.rs), [code generator](../../crates/wl-codegen-rust/src/lib.rs), [runtime](../../crates/wl-runtime/src/lib.rs), [Horizon environment](../../crates/wl-runtime/src/horizon.rs), [indirect-target analysis](../../crates/wl-analysis/src/flow.rs). Validation: [methodology](../testing/validation-methodology.md).

### File-backed native compilation

The CLI publishes generated Rust units to a temporary sidecar directory and releases
its in-memory source collection before compilation. `build::build_files` validates
source SHA-256 values before cache population and reacquires each bounded unit in its
worker. Unit text is retained only until that unit's compiler process completes;
worker count, rather than the entire program's source size, determines acquisition
memory. Each generated source file is limited to 64 MiB. Source files must remain
immutable for the duration of the build; changed or truncated bytes fail validation.

The runtime identity, generated source bytes, compiler identity, target and flags use
the same cache keys as the in-memory builder. Tests execute both builds and verify
identical runtime/unit keys and a complete cache hit on the second path. The main
source digest is checked again before linking. Successful CLI builds remove only
unchanged staging units, retaining the keyed cached sources; failed builds preserve
the staging files for diagnosis. This trades a bounded staging write and reread for
lower compiler-parent memory use; it is not a zero-copy compilation path.

Hosted [audio initialization](audio.md) uses a runtime callback contract with its
concrete implementation included in launcher source. Core contract changes still
invalidate all dependent native units; backend-only changes require a launcher
rebuild and link while preserving compatible CPU-unit archives.
