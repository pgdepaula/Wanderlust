# ADR-0003: Emit one host function per guest block, dispatched through a sorted table

Status: accepted; the single-crate emission is superseded by [ADR-0004](ADR-0004-separate-translation-units.md), which keeps per-block functions and table dispatch. Refines [ADR-0001](ADR-0001-aot-source-backend.md).

## Evidence

ADR-0001 emitted one `execute` function containing a `match` arm per recovered basic block. It was measured on a 42,105-instruction, 10,941-block program: the executable sections of the precompiled `aarch64-unknown-none` Rust standard library, analyzed from its 1,479 function symbols (`elf_text.py --roots`, `--roots-file`, `--allow-unsupported`). Host: WSL2, 12 logical CPUs, 3.7 GiB RAM, rustc 1.98.1, `opt-level=2`.

| Emission | Generated source | rustc result | Wall / CPU | Peak RSS |
|---|---|---|---|---|
| Single `match` function | — | killed by the kernel OOM killer | 168 s / — | ≥ 3.0 GiB |
| One function per block, 1,024 per module | 8.2 MB | 12.3 MB executable | 71.7 s / 161 s | 2.19 GiB |
| Same source, `opt-level=0` | 8.2 MB | 25.3 MB executable | 44.3 s / 48 s | 2.00 GiB |
| Same source, `opt-level=1` | 8.2 MB | 14.4 MB executable | 55.0 s / 143 s | 2.06 GiB |

A single function concentrates every block in one LLVM function, where register allocation and CFG passes scale superlinearly. Splitting removes that failure. The remaining cost is dominated by per-crate front-end work over the volume of generated code: even unoptimized compilation needs about 2 GiB for 8.2 MB of source.

## Decision

`wl-codegen-rust` emits each block as `fn b<address>(&mut Cpu, &mut Memory) -> RuntimeResult<Option<&'static str>>` inside partition modules of 1,024 blocks, and a `static BLOCKS: [(u64, BlockFn); N]` table sorted by guest address. The dispatch loop checks the return token and execute permission, binary-searches the table, and calls the block. A block returns `Ok(None)` to continue at `c.pc`, `Ok(Some(status))` to end execution, or an error. Unknown targets still fault with the target address; no interpreter fallback exists. Guest memory images are written to sidecar files and embedded with `include_bytes!`, with trailing zero bytes (BSS) restored at startup rather than spelled out in source.

## Consequences

Guest-visible semantics are unchanged: the 784-comparison differential suite and the native integration tests pass with the new emission. Dispatch costs one binary search per block transition, `O(log N)`; no direct block chaining is performed.

Extrapolated linearly, the measured rate (≈ 3.8 ms of compiler CPU and ≈ 195 bytes of source per guest instruction, ≈ 50 KiB peak memory per instruction within one crate) makes a single crate infeasible for an executable with millions of instructions. The remaining options differ in cost and are to be decided by the same measurement on larger input: compiling partitions as separate crates in parallel, cached by module hash and compiler settings; reducing emitted code per instruction (per-block instruction accounting, shared lowering helpers); or a direct Cranelift or LLVM backend. None of them is implemented.
