# ADR-0004: Compile translation units as separately cached rlibs, with block-level accounting

Status: accepted. Supersedes the single-crate emission of [ADR-0003](ADR-0003-partitioned-block-functions.md); per-block host functions and table dispatch are kept.

## Problem

ADR-0003 split guest blocks into one host function each, but all of them still formed one crate. Peak compiler memory then grows with the whole program, every rebuild recompiles everything, and parallelism is limited to rustc's codegen units within one crate.

## Measurements

Workload: the 42,105-instruction, 10,941-block proxy program of ADR-0003 (identical instruction stream in every row; the SIMD/FP decoder was not yet present). Host: WSL2, 12 logical CPUs, 3.7 GiB RAM, rustc 1.98.1, `opt-level=2`, target `x86_64-unknown-linux-gnu`. Peak RSS is that of the largest single compiler process, measured with `tools/python/benchmarks.py measure`. Cold cache unless stated.

| Configuration | Units | Jobs | Wall | Total compiler CPU | Peak RSS | Executable |
|---|---|---|---|---|---|---|
| ADR-0003 single crate | 1 | — | 68.8 s | 158.5 s | 2,190 MiB | 12.3 MB |
| Separate rlibs, 64 KiB windows | 5 | 4 | 45.2 s | 123.4 s | 876 MiB | 13.2 MB |
| Separate rlibs, 32 KiB windows | 9 | 4 | 38.4 s | 122.3 s | 519 MiB | 13.2 MB |
| Separate rlibs, 16 KiB windows | 18 | 1 / 2 / 4 / 8 | 114.3 / 53.1 / 30.6 / 24.4 s | 115.5 / 101.4 / 113.3 / 148.9 s | ~338 MiB | 13.2 MB |
| Separate rlibs, 8 KiB windows | 35 | 4 | 30.1 s | 109.3 s | 233 MiB | 13.2 MB |
| Separate rlibs, 4 KiB windows | 70 | 4 | 29.1 s | 108.6 s | 174 MiB | 13.3 MB |
| 4 KiB windows + block-level accounting and direct integer lowering | 70 | 4 | **21.1 s** | **77.5 s** | **169 MiB** | **11.7 MB** |
| same, warm cache, no change | 70 | 4 | 0.36 s | 0.4 s | — | — |
| same, one guest instruction changed | 70 | 4 | 2.24 s (1 unit rebuilt) | 2.3 s | — | — |

Observations: total compiler CPU depends on the volume of generated code, not on how it is split (≈ 110 s for 6.5–8.2 MB of source regardless of window size); peak memory scales with unit size; wall time scales with parallelism until hardware threads saturate. Reducing generated code per instruction cut CPU by 29 %.

Runtime cost was measured with a loop of 2,000,000 iterations of 11 instructions and three block transitions (`tools/python/benchmarks.py loop`): 346–414 M guest instructions per second with this design versus 256–406 M with ADR-0003, run to run; no regression is measurable.

## Decision

* **Units.** Blocks whose start address lies in the same aligned `2^unit_shift`-byte guest window form a crate `wlu_<window start>` (default 4 KiB, `--unit-shift`). Address windows were chosen over call-graph or function grouping because no block calls another directly — every transfer returns to the dispatcher — so grouping affects only compile cost and cache granularity, and fixed windows make a unit's identity stable under edits elsewhere.
* **Crate graph.** `wl_guest_runtime` (rlib, compiled once per toolchain/flags) ← each unit (rlib exporting `pub static BLOCKS: [(u64, BlockFn); n]`, sorted) ← the executable crate, which contains only the unit table, the module map, memory setup and `main`. Units never reference each other, so they compile in any order and in parallel (`--jobs`, default `min(CPUs, 4)`). Dispatch is a binary search over units by window start, then over the unit's blocks.
* **Cache.** An rlib is stored in `<cache>/<crate>-<key>` where `key = SHA-256(crate name, source, runtime key, rustc -vV, target, flags)`; the runtime key covers all runtime sources. The source fully encodes translation decisions, so no separate "semantics version" can drift. Population is atomic (build in a temporary directory, rename). Cache hits and misses are reported per build.
* **Instruction accounting.** A block adds its instruction count at entry (`Cpu::enter`). Only instructions that can fault record their PC; after a fault the dispatcher recomputes the exact number of retired instructions from the faulting PC, because a block's instructions are contiguous. The budget is checked at block entry: a block that would exceed it is not started, and the fault reports the block's start.

## Consequences

Guest-visible results are unchanged: the integer differential suite and all native tests pass, and cross-unit execution is tested with a program whose loop, direct callee and indirect callee lie in three units, against the single-unit build. Budget exhaustion is now block-granular. The `.deps` directory beside an executable holds hard links (or copies) of its rlibs so the final rustc command line stays short.

### Inlining of guest memory accessors

The runtime's memory accessors (`Memory::load`, `store`, `load_vector`, `store_vector` and the region lookup) are `#[inline]`, so their range and permission checks are inlined at every access site. Measured trade-off on the full-ISA proxy (71,198 instructions, 4 KiB units, 4 jobs) and on the memory benchmark (`tools/python/benchmarks.py memloop`: 16,000,003 guest instructions of LDR/STR/LDP/STP and arithmetic):

| Accessors | Compiler CPU | Wall | Executable | Memory benchmark |
|---|---|---|---|---|
| inlined (kept) | 128.2 s | 33.3 s | 14.9 MB | 321 M guest instructions/s |
| out of line | 110.9 s | 29.0 s | 13.8 MB | 177 M guest instructions/s |

Translated code executes many times while each unit compiles once and is then cached, so the 1.8× execution speed is preferred over a 13.5 % compile-time saving. Collapsing base-plus-offset accesses into single expressions reduced generated source by 5 % without a measurable compile-time change.

At the measured rate of the full SIMD/FP-capable translator (≈ 1.8 ms of compiler CPU and ≈ 139 bytes of source per guest instruction on the 71,198-instruction proxy), compiling an executable of millions of instructions remains hours of CPU time, now spread over parallel jobs with bounded memory and paid once per unit thanks to the cache. Further reduction of generated code volume, or a different code generator for cold units, is the next measurable lever.
