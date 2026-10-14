# AArch64 support

The Switch CPU (Cortex-A57) implements ARMv8.0-A. The decoder ([`wl-aarch64`](../../crates/wl-aarch64/src/lib.rs)) accepts the ARMv8.0 A64 *integer* instruction set used by application code. ARMv8.1+ encodings (LSE atomics such as `CAS`/`LDADD`, `LDAPR`, LORegions `LDLAR`/`STLLR`) are rejected: they cannot occur in code built for this CPU, and accepting them would hide a misidentified input. Register number 31 is resolved to `Sp` or `Zero` at decode time according to each encoding's definition.

## Implemented families

| Family | Instructions | Decode-time constraints |
|---|---|---|
| Move wide | MOVZ, MOVN, MOVK (W/X) | reserved `opc`; W halfwords 2/3 |
| Add/sub immediate | ADD, ADDS, SUB, SUBS (optional LSL #12) | — |
| Add/sub shifted register | ADD/ADDS/SUB/SUBS with LSL/LSR/ASR | ROR (shift 3) and shifts ≥ width rejected |
| Add/sub extended register | UXTB…SXTX operands, LSL #0–4; SP forms | `opt ≠ 0`, shift > 4 rejected |
| Add/sub with carry | ADC, ADCS, SBC, SBCS (NGC/NGCS) | — |
| Logical shifted register | AND, BIC, ORR, ORN, EOR, EON, ANDS, BICS | shifts ≥ width rejected |
| Logical immediate | AND, ORR, EOR, ANDS with bitmask immediates; AND/ORR/EOR may write SP | `N = 1` for W, reserved all-ones element |
| Bitfield | SBFM, BFM, UBFM (ASR/LSL/LSR/SXT*/UXT*/SBFX/UBFX/BFI/BFXIL) | `N ≠ sf`, W fields ≥ 32 |
| Extract | EXTR (ROR immediate) | `op21`/`o0`, W `imms ≥ 32` |
| Conditional select | CSEL, CSINC, CSINV, CSNEG (CSET, CINC, CNEG, …) | `S = 1`, `op2 ≥ 2` |
| Conditional compare | CCMP, CCMN, register and immediate | `S = 0`, `o2`/`o3` set |
| 2-source | UDIV, SDIV, LSLV, LSRV, ASRV, RORV, CRC32B/H/W/X, CRC32CB/CH/CW/CX | other opcodes |
| 1-source | RBIT, REV16, REV32, REV, CLZ, CLS | W REV32 slot, PAC opcodes |
| 3-source | MADD, MSUB, SMADDL, SMSUBL, UMADDL, UMSUBL, SMULH, UMULH | `op54 ≠ 0`, W long/high forms |
| PC-relative | ADR, ADRP | checked target formation |
| Branches | B, BL, B.cond, CBZ/CBNZ, TBZ/TBNZ, BR, BLR, RET | — |
| Load/store register | all sizes; STR/LDR (zero-extending), LDRSB/LDRSH/LDRSW to W or X; unsigned offset, unscaled, pre/post-index, register offset (UXTW/LSL/SXTW/SXTX, optional scaling) | unprivileged LDTR/STTR; register option `x0x`; writeback with `Rn == Rt` |
| Load literal | LDR W/X, LDRSW, PRFM | — |
| Load/store pair | STP/LDP W/X, LDPSW, STNP/LDNP; offset, pre/post-index | `opc = 11`; LDPSW non-temporal/store; overlapping writeback; LDP `Rt == Rt2` |
| Exclusive / ordered | LDXR/STXR, LDAXR/STLXR (B/H/W/X), LDXP/STXP, LDAXP/STLXP (W/X), LDAR/STLR (B/H/W/X) | v8.1 forms; non-ones unused fields; status register overlapping data/base; LDXP `Rt == Rt2` |
| Prefetch | PRFM, PRFUM (all address forms) | executes as no-op |
| Hints | NOP, YIELD, WFE, WFI, SEV, SEVL and all other hint-space encodings (PAC/BTI hints) | executed as NOP, as ARMv8.0 hardware does |
| Barriers | DMB, DSB, ISB, CLREX | SB and other `op2` values |
| System registers | MRS/MSR TPIDR_EL0, NZCV, FPCR, FPSR; MRS TPIDRRO_EL0, CTR_EL0, DCZID_EL0 | any other register; MSR to read-only registers |
| Cache maintenance | DC ZVA (zeroes the 64-byte block); DC CVAC, DC CVAU, DC CIVAC, IC IVAU (no effect) | other SYS operations |
| Exceptions | SVC (dispatched to the runtime), BRK, HLT (reported as guest faults) | — |

SIMD&FP registers, floating point and Advanced SIMD are specified in [AArch64 SIMD and floating point](aarch64-simd-fp.md). System counter reads `MRS Xt, CNTPCT_EL0`, `CNTVCT_EL0` and `CNTFRQ_EL0` (writes are undefined at EL0 and rejected) are implemented in [`crypto.rs`](../../crates/wl-runtime/src/crypto.rs): the frequency is 19.2 MHz, and both counters return the deterministic virtual clock of a Horizon process (the value svcGetSystemTick reports at the same point) or, in harness mode, the retired-instruction count as nanoseconds. CRC32/CRC32C are the bit-reflected polynomials 0x04C11DB7 and 0x1EDC6F41 over the low 1, 2, 4 or 8 bytes, without inversion. The decoders for these forms and for the recognised crypto instructions are in [`extensions.rs`](../../crates/wl-aarch64/src/extensions.rs). Not implemented: pointer-authentication data operations, and system registers other than those listed. Unsupported reachable instructions are either reported at translation time or, with `--allow-unsupported`, compiled into a fault that reports the exact opcode and PC.

## Semantics

W writes zero the upper 32 bits; zero-register writes are discarded. Arithmetic uses unsigned wrapping operations masked to width.

* **Flags.** `ADDS/SUBS/CMP/CMN/CCMP/CCMN` use

  ```text
  N = (r & s) != 0;  Z = r == 0
  C(add) = widened(a) + widened(b) > width_mask      C(sub) = a >= b
  V(add) = (~(a ^ b) & (a ^ r) & s) != 0              V(sub) = ((a ^ b) & (a ^ r) & s) != 0
  ```

  ADC/SBC and their flag-setting forms use the Arm ARM `AddWithCarry(x, y, carry_in)` definition directly (SBC computes `x + NOT(y) + C`): `C` is set when the unsigned sum differs from its width-truncated result, `V` when the signed sum differs. Logical flag-setting forms set N/Z and clear C/V. NZCV is stored as bits 3..0; `MRS/MSR NZCV` move it to and from bits 31..28.
* **Bitmask immediates and bitfields** use `DecodeBitMasks(N, imms, immr)`, evaluated at decode time into `(wmask, tmask)` ([`decode_bit_masks`](../../crates/wl-aarch64/src/lib.rs)). UBFM computes `ROR(src, R) & wmask & tmask`; SBFM replaces bits outside `tmask` with `src<S>`; BFM merges into the destination under `wmask` then `tmask`.
* **Extended registers** follow `ExtendReg`: the low 8/16/32/64 bits, zero- or sign-extended, then shifted by 0–4.
* **Division** by zero yields 0; `INT_MIN / −1` yields `INT_MIN`. Variable shifts use the amount modulo the width.
* **CLS** is `CountLeadingZeroBits(x<N−1:1> EOR x<N−2:0>)`.
* **Loads** sign-extend to the encoded destination width when signed; a W destination then zeroes bits 63..32. Effective addresses wrap at 64 bits; the memory subsystem checks full access ranges. Accesses with SP as base require SP to be 16-byte aligned. Ordinary accesses may be unaligned.
* **Exclusives.** There is one guest thread. A local monitor records `(address, size)` at each load-exclusive; a store-exclusive succeeds — stores and writes 0 to the status register — only if the monitor holds exactly that address and size, and fails with 1 otherwise. Either outcome clears the monitor, as does CLREX. Exclusive and ordered accesses fault unless naturally aligned to the full access size (16 bytes for a 64-bit pair). Acquire/release ordering and barriers have no observable effect with a single thread and are compiled to nothing. Multi-threaded guest execution requires a real memory-ordering design and is not implemented.
* **System registers.** TPIDR_EL0 is stored as written. FPCR and FPSR writes are masked to the implemented bits and drive floating-point execution ([SIMD and FP](aarch64-simd-fp.md)). TPIDRRO_EL0 is read-only at EL0 and is set by the process environment (on Horizon it addresses the thread-local region). CTR_EL0 reads `0x8444C004` and DCZID_EL0 reads `4` (64-byte DC ZVA block), the Cortex-A57 values; the differential reference reports the same values.

Reserved, unallocated and constrained-unpredictable encodings produce a PC/opcode diagnostic; no guest exception delivery is implemented.

## Encoding classes

`wl_aarch64::encoding_class(word)` assigns every 32-bit word to an encoding group of the Arm ARM top-level decode tables (`dp-imm/bitfield`, `ldst/register-offset`, `fp/scalar`, `simd/vector`, …). `wanderlust survey` reports unsupported words per class. The classification describes encodings only: data words and unallocated encodings inside a group are classified too.

### Proxy measurement

To prioritize work without a readable game executable, `survey` was run over the executable sections of the precompiled `aarch64-unknown-none` Rust standard library (`libcore`, `liballoc`, `compiler_builtins`; rustc 1.98.1; 1,479 sections, 71,398 words), extracted by [`elf_text.py`](../../tools/python/elf_text.py). This is LLVM AArch64 output — the same backend family as the clang-based platform toolchain — but it is **not** game code, and object files are unlinked. Linear-sweep acceptance:

| Decoder state | Accepted words | Unsupported non-SIMD/FP | Unsupported SIMD/FP |
|---|---|---|---|
| Original subset | 71.3% | 15.4% | 13.3% |
| ARMv8.0 integer set | 86.5% | 0.14% (LSE atomics, CAS) | 13.3% |
| ARMv8.0 integer, FP and Advanced SIMD | 99.86% | 0.14% (LSE atomics, CAS) | 0% |

Before SIMD/FP support, scalar FP was 8.5% of the corpus (FMOV, FMUL, FADD, FSUB, FCMP, FDIV, FCSEL, FCVT, …, confirmed with Capstone), vector SIMD 2.4% and SIMD&FP register loads/stores 2.3% (LLVM moves 16/32-byte aggregates through Q registers even in integer code). The remaining 100 words are ARMv8.1 LSE atomics from `compiler_builtins`' outlined-atomic helpers, which code for an ARMv8.0 CPU cannot execute. [`classify_unsupported.py`](../../tools/python/classify_unsupported.py) ranks unsupported words of any survey by Capstone mnemonic and operand shape.

## References

[Arm's ISA overview](https://developer.arm.com/-/media/Files/pdf/graphics-and-multimedia/ARMv8_InstructionSetOverview.pdf) and the Arm Architecture Reference Manual (DDI 0487) pseudocode (`AddWithCarry`, `DecodeBitMasks`, `ExtendReg`, `CountLeadingSignBits`) define the semantics. Independent evidence comes from [QEMU's A64 translator](https://github.com/qemu/qemu/blob/615ece3c406b262996e569f29ea6eed0d81c4d8d/target/arm/tcg/translate-a64.c), [Ghidra's AARCH64 definitions](https://github.com/NationalSecurityAgency/ghidra/blob/918d44ebc2962a53d23eeee52b8f44c7816e7815/Ghidra/Processors/AARCH64/data/languages/AARCH64instructions.sinc), Capstone disassembly of every test encoding, and executed [Unicorn differential cases](../../tests/differential/run.py). No code from these projects is copied into the decoder or runtime.
