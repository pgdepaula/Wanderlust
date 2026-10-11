# ADR-0005: Resolve indirect control flow only where constant propagation proves the targets

Status: accepted.

## Problem

Ahead-of-time translation must know every block that can execute. Recursive traversal from the entry, relocated code pointers and symbols misses two common forms of compiler output:

* **Jump tables.** `switch` statements compile to a bounds check, a table load and `BR`. In the libnx probe, newlib's `_svfprintf_r` dispatches its format characters through a 91-entry halfword table (`ldrh w1, [x26, w1, uxtw #1]; adr x2, L; add x1, x2, w1, sxth #2; br x1`). The table base is loaded into callee-saved X26 before the formatting loop, and the bound check sits in the preceding block. Without the cases, the first `printf` faults with `unresolved-control-flow`.
* **Code addresses formed in code.** `ADRP`/`ADD` pairs place function addresses in registers that are stored (libnx stores `&svcExitProcess` as its exit continuation), passed to calls or branched to. Position-independent code needs no relocation for these, so the loader cannot report them.

Runtime discovery (interpretation or a JIT) is excluded by design ([ADR-0001](ADR-0001-aot-source-backend.md)), and heuristics that treat arbitrary text as code create blocks for data.

## Decision

[`wl-analysis::flow`](../../crates/wl-analysis/src/flow.rs) runs a forward dataflow over the decoded control-flow graph, block by block. It runs to a fixpoint and alternates with traversal until no new code appears.

* **Values.** Per general register: a constant (from `ADR`, `ADRP`, `MOVZ/MOVN/MOVK`, `ADD/SUB` immediate, `MOV`), an unsigned bound, a table element, or a table-relative target. Join is equality; anything else is unknown. Unrecognised writers make their destination registers unknown. That list is derived per instruction form, including writeback bases and SIMD-to-general moves.
* **Bounds.** A `CMP Rn, #imm` is tracked while NZCV is unchanged and Rn is not redefined. On the edges of `B.HI`/`B.LS`/`B.HS`/`B.LO` it bounds Rn. A bound from a 32-bit compare covers a 64-bit index only when the register's upper half is known zero (last written as a W register).
* **Tables.** A load `[const, bounded, extend/shift]` of 1, 2, 4 or 8 bytes is a table element. Adding it, extended and shifted, to a constant gives a target set. Every entry is read from mapped data and must land on an instruction-aligned executable address, or the table is not resolved. Resolved targets become block leaders and edges of the branch, so the states of the case blocks (for example the table base in X26 flowing back to the loop head) feed further resolution. They are not treated as function entries.
* **Calls.** Roots and call targets start with no knowledge. A call edge clobbers X0–X18, X30 and the flags and preserves X19–X29 (AAPCS64), and an SVC clobbers X0–X7. These are assumptions about the guest's ABI, used only to propose targets.
* **Code pointers.** Constants in executable memory that are stored, passed in X0–X7 to a call, or used as a branch target become speculative roots.

Every proposed address is traversed like any other speculative root, so a wrong proposal is rejected if traversal leaves executable memory. A proposal that decodes cleanly but is never executed only adds blocks. Branches whose targets are not proven keep faulting with `unresolved-control-flow`.

## Consequences

The libnx probe gains 4 jump tables (140 entries) and 62 roots: about 5,600 reachable instructions without it, 12,811 with it. It runs to `svcExitProcess`. Analysis of the probe takes 0.05 s in a release build of the CLI and about 0.2 s in a debug build. Each outer iteration re-runs the dataflow over the whole program, so very large executables pay this once per round of new discoveries. Tables reached through memory-held bases, computed bounds (`AND`, `UBFX`), or a compare in another function stay unresolved; the runtime reports them precisely.
