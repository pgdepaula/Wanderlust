# Validation methodology

## Rust checks

```sh
cargo fmt --all --check
cargo clippy --workspace --all-targets --locked -- -D warnings
cargo test --workspace --locked
```

The CLI module tests require Python 3, an LLD linker and, for the LLVM-compiled kernel, the `aarch64-unknown-none` Rust target (the test reports a skip without it; CI installs it). They need an LLD linker (`ld.lld`, or the `rust-lld` shipped with every rustup toolchain; `WL_LLD` and `WL_PYTHON` override discovery). They generate their inputs at test time with [`modules.py`](../../tools/python/modules.py).

The crate tests exercise NSO reconstruction, every truncation of a compressed fixture, malformed magic/version/flags, file/memory/BSS overlaps, size limits, address overflow, LZ4 failure, decoded-size mismatch, hash mismatch, metadata bounds and MOD0 dynamic-table bounds. Decoder tests distinguish SP/zero, W/X width, backward targets, reserved encodings, unsupported SIMD and writeback overlap. CFG tests cover loops, unreachable data, direct calls and explicit indirect roots, plus assembled GCC- and Clang-style jump tables (resolved with every case as a leader, table data never decoded), an unbounded dispatch that must stay unresolved, and a code address formed by ADRP/ADD and stored.

Runtime tests check byte order, widths, unaligned accesses, permissions, endpoint overflow, overlapping mapping rejection, copy overlap, arithmetic flags, all conditions, shifts and budget/service behavior. [IPC tests](../../crates/wl-runtime/tests/ipc.rs) cover named ports, HIPC validation and the sm: server. [Thread tests](../../crates/wl-runtime/tests/threads.rs) cover thread lifecycle, TLS, waits, lock arbitration, condition variables, stack aliases and the scheduler. [Horizon tests](../../crates/wl-runtime/tests/horizon.rs) cover the address-space layout, handle encoding, process start state, memory queries, every implemented SVC with its errors and memory effects, region splitting, accesses spanning regions, the SVC register contract and SVC access flags. CLI integration tests invoke the actual compiler and launch generated executables; they include compressed NSO execution, memory faults, unsupported SVCs, ExitProcess, a bounded native infinite loop, unsupported opcodes, input overwrite protection and unknown indirect targets.

Loader tests ([linking](../../crates/wl-loader/tests/linking.rs), [dynamic tables](../../crates/wl-loader/tests/dynamic_tables.rs)) build synthetic MOD0 modules with exact dynamic tables and check relocation semantics against the formulas in [module loading](../platform/module-loading.md): RELA/REL relative relocation independent of the stored word, module-base changes, the count prefix, load-order precedence, hidden visibility, REL jump slots, ABS32 width and overflow, unresolved strong/weak symbols, unsupported/unmapped/read-only targets, layout, rtld blocking, guest mode and initializer order, and DT_RELR address and bitmap entries (applied by host linking, left for the guest in guest mode, malformed bitmap order rejected); fourteen malformed tables must be rejected. Intake tests build PFS0 packages with encrypted-looking and plaintext NCA headers, CNMT XML and a ticket whose title-key block holds a canary that must not appear in the report. Decoder tests add the bit-mask decoder's architectural examples, decoding of each new family, and rejection of unallocated, ARMv8.1 and constrained-unpredictable encodings, each cross-checked with Capstone. CLI module tests link LLD-built modules in REL and RELA form and execute them natively, check that unrelocated execution is refused or faults, and check precise runtime faults for unsupported instructions, BRK and misaligned exclusives.

Floating-point unit tests pin values derived from IEEE 754 and the Arm pseudocode (see [SIMD and FP](../platform/aarch64-simd-fp.md)). Build tests check that rebuilding an unchanged program hits the cache for every unit and that changing one guest instruction recompiles exactly one unit, and that a program whose loop, direct callee and indirect callee lie in three translation units gives the same registers and instruction count as a single-unit build. The LLVM kernel test compiles [`tests/programs/guest.rs`](../../tests/programs/guest.rs) for AArch64 and [`host.rs`](../../tests/programs/host.rs) for the host and requires identical results.

The complete workspace suite passed on Linux x86-64 with rustc 1.98.1: 217 tests, including Horizon runtime 12, threads 13, IPC 5, CFG/survey 11, linking 16 and CLI Horizon 5, with 5 tests that require a GPU adapter ignored. Counts should be taken from actual test output rather than from this document.

## Independent differential execution

[The fixture generator](../../tools/python/fixtures.py) constructs original little-endian A64 machine words with independent encoding helpers. It also generates raw-LZ4 literal-only NSOs and hashes using Python's standard library. No fixture contains commercial-game instructions or assets.

[The harness](../../tests/differential/run.py) uses Unicorn 2.1.4 as an independent execution reference. Capstone 5.0.7 disassembles failure reproductions; it does not supply execution semantics. Neither dependency is linked into the native runtime. Initial states use a fixed PRNG seed, integer boundary operands, arbitrary high register bits, every NZCV value and 64 KiB of nonzero memory.

```sh
python3 -m venv .venv
.venv/bin/python -m pip install -r tests/differential/requirements.txt
cargo build -p wl-cli
.venv/bin/python tests/differential/run.py
```

The 70-program corpus compares 16 states each (1,120 comparisons): 49 integer programs and 21 SIMD/FP programs (`simd_programs.py`, 400 distinct instructions). It covers arithmetic flags at both widths, shifted and extended operands (including SP forms), all logical variants with shifted and bitmask-immediate operands, move-wide preservation, bitfield and extract operations, conditional select and compare (flags captured after each step through `MRS NZCV`), division by zero and signed overflow, variable shifts, all multiply forms, add/subtract with carry, bit/byte reversal and counting, address formation, scalar/pair memory including register offsets with every extend, sign-extending loads, literal loads, LDPSW, non-temporal pairs and prefetches, exclusive success/failure/CLREX sequences for byte to pair sizes, load-acquire/store-release, barriers and hints, TPIDR_EL0/TPIDRRO_EL0/NZCV/CTR_EL0/DCZID_EL0 accesses and DC ZVA, stack frames, nested calls, explicit BLR targets and conditional/compare/test branches. The mixed program executes a real 3-iteration loop and BL/RET. Initial scratch base is `0x10000000`; raw text base is `0x10000`.

For each execution the harness compares X0–X30, V0–V31, SP, NZCV, FPCR, FPSR, final PC, instruction count and all scratch bytes, bit for bit. Initial V registers, FPCR and FPSR vary per sample as described in [SIMD and FP](../platform/aarch64-simd-fp.md#validation); SIMD/FP programs are assembly encoded with Keystone and accepted only when Capstone round-trips each word ([`encode_simd.py`](../../tools/python/encode_simd.py)), and the committed encoding table means test runs need neither tool. Success writes `report.json`; mismatch writes `mismatch.json` with initial registers, flags, differing fields, memory hashes/addresses and disassembly. Input and dump files remain alongside the reproduction. `--case NAME` and `--samples N` permit focused reruns. Native and reference execution have explicit time/instruction limits.

All 1,120 comparisons passed on Linux x86-64, as did 4,480 with `--samples 64`, using the separately compiled translation units of [ADR-0004](../decisions/ADR-0004-separate-translation-units.md). During development the corpus caught a decoder error (LDRSW literal decoded as an 8-byte load), a SIMD&FP pair mask that captured 128-bit vector instructions, FRINT of already-integral values, and unmasked FPCR/FPSR writes. This is evidence for the tested instructions/states, not an ISA-conformance proof. SIMD/FP state, exceptions and concurrency are absent. Only scratch is compared; runtime permissions prohibit writes to raw text. Generated source sizes, executable sizes and individual compilation times are recorded in reports; these small programs are not a whole-game performance baseline.

## Horizon process execution

A libnx application ([`probe.c`](../../tests/horizon/probe.c)) built with devkitA64 runs through the complete pipeline in process mode. It covers libnx startup with DT_RELR self-relocation, RELRO reprotection, heap and stack setup, newlib `printf`, and SVC and memory checks, then exits through `svcExitProcess`. The [CLI test](../../tools/wl-cli/tests/horizon.rs) requires exact output lines derived independently of the runtime; the fixture, expected values and results are in [Horizon process environment](../platform/horizon-process.md#validation). Linux and Windows (`x86_64-pc-windows-gnu`, run through WSL interoperability) executables gave identical output, 64,767 guest instructions and exit status 0. Peak resident memory on Linux was 16 MiB with a 508 MiB guest heap. A second libnx program ([`threads.c`](../../tests/horizon/threads.c)) runs two guest threads with a contended libnx mutex, a condition variable, a timed wait and a join. It produced identical output and scheduler counters (1,041,282 instructions, 156 context switches) on Linux and Windows. A third program ([`ipc.c`](../../tests/horizon/ipc.c)) exchanges CMIF messages with the service manager through libnx, including a deferred lookup across threads. It also matched on both systems.

## Windows evidence and limits

A Windows GNU x86-64 workspace build completed. A generated PE executable from `mixed-compressed.nso` executed on Windows through WSL interoperability and reported `status=returned`, X0=26, X3=X4=21, NZCV=6 and 21 executed instructions. The fixture input SHA-256 was `532e82a4efd4fdab2669402392164c291dc26edbdfc41e2e6beae3acc51201d4`.

The complete corpus (70 programs, 1,120 comparisons) passed with executables built for `x86_64-pc-windows-gnu` and run on Windows (version 10.0.26300) through WSL interoperability, compared with the same Unicorn references as on Linux. An earlier attempt failed with WSL `UtilAcceptVsock` process-launch errors; that failure was environmental and did not recur. Windows MSVC builds and remote CI were not executed locally.

## Windows

The differential harness accepts `--target x86_64-pc-windows-gnu`; native executables are then built for Windows and launched through WSL interoperability. With the toolchain setup of [build environment](../development/build-environment.md), the integer and SIMD/FP programs were run this way against the same Unicorn references (results below).

### Relocatable and compiled programs on Windows

The LLD-linked two-module program (RELA form) was recompiled for `x86_64-pc-windows-gnu` and executed on Windows: PE32+ x86-64, `status=returned`, X0 = 1681, X9 = 1001, 43 guest instructions — identical to the Linux result. The LLVM-compiled SIMD/FP kernel returned the host oracle's hashes on Windows for each tested seed (18,522 guest instructions).

## Commercial-game boundary

Tears of the Kingdom base and update packages were supplied and verified (content IDs and CNMT hashes), but all NCA headers are encrypted and no executable module is readable ([supplied content](../game/content-layout.md)). No game instruction has been decoded or executed; there is no game entry-point execution, framebuffer or gameplay evidence. The strongest demonstrated boundary is original guest code — LLD-linked, dynamically relocated multi-module programs, LLVM-compiled floating-point/SIMD code, and a libnx application started as a Horizon process — executing through compiled host operations.

Game-dependent checks stay local and opt-in: commands take the user's paths as arguments, and no test or CI configuration references game files.
