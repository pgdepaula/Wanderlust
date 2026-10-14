# AArch64 SIMD and floating point

This document specifies how Wanderlust represents and executes the ARMv8.0 Advanced SIMD and floating-point instruction set. Decoding: [`wl-aarch64::simd`](../../crates/wl-aarch64/src/simd.rs). Semantics: [`wl-runtime::fp`](../../crates/wl-runtime/src/fp.rs) and [`wl-runtime::simd`](../../crates/wl-runtime/src/simd.rs). Lowering: [`wl-codegen-rust`](../../crates/wl-codegen-rust/src/lib.rs). Integer instructions are covered in [AArch64 support](aarch64.md).

## Register state

`Cpu::v: [u128; 32]` holds V0–V31. The B, H, S, D and Q names are the low 8, 16, 32, 64 and 128 bits of the same register; there is no separate scalar register file. For an arrangement of `e`-bit elements, lane `i` occupies bits `[i·e, (i+1)·e)`. With little-endian memory, the element at the lowest address is lane 0, so a 16-byte `LDR Q` followed by lane extraction observes elements in memory order.

Every instruction states its own write width:

| Write | Bits written | Bits above |
|---|---|---|
| Scalar FP and scalar AdvSIMD results (`FADD S`, `ADD D`, `FMOV S`, reductions) | element size | zeroed |
| 64-bit vector arrangements (`8B`, `4H`, `2S`) | 64 | zeroed |
| 128-bit arrangements | 128 | — |
| `INS`, `FMOV V.D[1], X`, narrowing "2" forms (`XTN2`, `SHRN2`, `FCVTN2`, …) | one element or the upper half | preserved |
| SIMD&FP loads (`LDR B..Q`, `LDP`) | access size | zeroed |
| Single-lane structure loads (`LD1 {V.S}[i]`) | the lane | preserved within the arrangement width; upper half zeroed for 64-bit forms |

Generated code applies these rules with an explicit mask per instruction (`vwrite`), never with one global policy.

FPCR and FPSR are `Cpu::fpcr` and `Cpu::fpsr`. Writes through `MSR` and the runtime command line are masked to the bits the modelled CPU implements: FPCR `0x07F7_0000` (AHP, DN, FZ, RMode, Stride, Len; the trap-enable bits read as zero because the CPU has no FP exception trapping) and FPSR `0xF800_009F` (AArch32 NZCV, QC and the cumulative flags IOC, DZC, OFC, UFC, IXC, IDC). Both masks were observed on the reference (Unicorn, Cortex-A72 model) and are the values the differential suite compares.

## Floating-point semantics

Values are carried as raw bit patterns (`u64`, low bits used for H and S). Every operation follows the Arm ARM pseudocode:

1. **`FPUnpack`** classifies an operand as zero, infinity, quiet NaN, signalling NaN, or finite `(sign, mantissa, exponent)` with `value = mantissa · 2^exponent` (a `u128` mantissa). With `FPCR.FZ = 1`, single and double denormal inputs become signed zeros and set IDC. Half-precision operands are never flushed in ARMv8.0.
2. **NaN processing** (`FPProcessNaNs`, `FPProcessNaNs3`): the first signalling NaN in operand order wins over any quiet NaN; it is quieted and IOC is set. Otherwise the first quiet NaN is returned. With `FPCR.DN = 1` the result is the default NaN (`0x7FC00000`, `0x7FF8000000000000`). FSUB processes NaNs before negating the subtrahend; FMSUB, FNMADD and FNMSUB negate operands first (`FPNeg` flips a NaN's sign).
3. **Exact combination.** Products (`m1·m2` ≤ 106 bits), sums (operands normalized to bit 124; a shift larger than the 70-bit headroom ORs the discarded bits into the least-significant "sticky" bit), quotients (dividend normalized to bit 125, divisor to bit 63, remainder → sticky) and square roots (integer square root of a mantissa normalized to bit 124/125 with an even exponent, remainder → sticky) are computed exactly up to one sticky bit. At least 55 significant bits remain whenever a sticky bit is set, so the single rounding below is exact.
4. **`FPRound`** rounds once under `FPCR.RMode` (nearest-even, +∞, −∞, zero), producing subnormals and handling the carry into the next binade. Flags follow the pseudocode: IXC when the discarded part is non-zero; UFC when the result is inexact and the *unrounded* exponent is below the minimum (tininess before rounding); OFC and IXC on overflow, which returns infinity or the largest finite value depending on the mode and sign; with FZ, a result that is tiny before rounding becomes a signed zero with UFC only.

Special results follow the pseudocode case by case: ∞−∞, 0·∞, 0/0, ∞/∞ and `sqrt` of a negative number are invalid (default NaN, IOC); `x/0` for finite non-zero `x` gives a signed infinity with DZC; exact zero sums are +0 except −0 in round-to-minus-infinity, and `(−0)+(−0) = −0`; FMAX/FMIN order `−0 < +0`; FMAXNM/FMINNM replace a single quiet NaN by the infinity that loses the comparison, so the number is returned; FMULX returns ±2.0 for ∞·0; FRECPS and FRSQRTS compute `2 − a·b` and `(3 − a·b)/2` with one rounding and return 2.0 or 1.5 for ∞·0.

Conversions:

| Operation | Rule |
|---|---|
| FCVT (H/S/D) | NaN: IOC if signalling, sign and high fraction bits kept, quieted, or default NaN with DN; finite: one `FPRound` in the target format. `FPCR.AHP = 1` (alternative half precision) faults with `unsupported FPCR.AHP`. |
| FCVT[NPMZA][SU], fixed-point FCVTZ[SU] | exact round to integer in the selected mode, then saturation; NaN gives 0 and IOC; out of range saturates with IOC (no IXC); in range and inexact sets IXC |
| SCVTF/UCVTF | exact integer (scaled by `2^-fbits`) rounded once under FPCR |
| FRINT[NPMZAI], FRINTX | exact integral rounding; FRINTX raises IXC when the value changes; values with non-negative exponent are returned unchanged |
| FMOV (immediate) | `VFPExpandImm`, evaluated at translation time |

### Host fast path and its validity domain

Host `f32`/`f64` addition, subtraction, multiplication, division and square root are correctly rounded IEEE-754 operations in round-to-nearest-even. Rust guarantees this, and x86-64 implements it with SSE2. They differ from the Arm results only in NaN propagation (the x86 default NaN is negative and payload selection differs), in the other rounding modes, in FZ/DN behaviour and in FPSR. The runtime uses the host operation only when **all** of the following hold:

* FPCR has RMode = round to nearest, FZ = 0 and DN = 0;
* FPSR.IXC is already set (IXC is cumulative, so the exactness of this result cannot change architectural state);
* no operand is a NaN or infinity;
* the result is finite and normal with a biased exponent above 1, so neither tininess nor overflow is possible.

Under these conditions the host result is the Arm result bit for bit and no other flag can be raised; otherwise the exact path runs. FMA (`FMADD` family, vector `FMLA`/`FMLS`) always uses the exact path: host fused multiply-add correctness depends on the C library on targets without a hardware FMA instruction. Integer-to-float conversion uses the host conversion under the same FPCR/IXC condition.

## Advanced SIMD

Vector operations are lanewise helpers selected by constants (`simd::ADD`, `simd::FMLA`, …) that the translator emits next to the arrangement (element size, lane count). Integer lanes use wrapping arithmetic at the element width; saturating forms (`SQADD`, `UQSUB`, `SQXTN`, `SQABS`, …) set FPSR.QC. FP lanes call the scalar `fp` operations in ascending lane order, so FPSR accumulates exactly as in the pseudocode. Reductions (`FMAXV`, scalar `FADDP`) use the pseudocode's pairwise tree, not a left fold.

| Group | Instructions |
|---|---|
| Loads/stores | LDR/STR/LDUR/STUR B/H/S/D/Q (unsigned, unscaled, pre/post-index, register offset), literal S/D/Q, LDP/STP/LDNP/STNP S/D/Q, LD1–LD4/ST1–ST4 multiple structures, LD1R–LD4R, single-lane LD1–LD4/ST1–ST4, post-index by immediate or register |
| Scalar FP | FMOV (register, immediate, general, `V.D[1]`), FADD, FSUB, FMUL, FNMUL, FDIV, FMAX, FMIN, FMAXNM, FMINNM, FABS, FNEG, FSQRT, FMADD, FMSUB, FNMADD, FNMSUB, FCMP, FCMPE, FCCMP, FCCMPE, FCSEL, FCVT (H/S/D), FRINT[NPMZAXI], SCVTF/UCVTF and FCVT[NPMZA][SU] to and from W/X, fixed-point forms |
| Data movement | MOVI, MVNI, ORR/BIC (vector immediate), FMOV (vector immediate), DUP (element, general), INS (element, general), UMOV, SMOV, EXT, TBL, TBX, ZIP1/2, UZP1/2, TRN1/2 |
| Integer | ADD, SUB, MUL, MLA, MLS (vector and by element), SQDMULH and SQRDMULH (vector, by element and scalar by element, 16/32-bit lanes), [SU]MULL[2], [SU]MLAL[2] and [SU]MLSL[2] by element, [SU]ADDLP, [SU]ADALP, SQSHRN[2], SQRSHRN[2], UQSHRN[2], UQRSHRN[2], SQSHRUN[2], SQRSHRUN[2] (vector and scalar), ADDP, [SU]MAX[P], [SU]MIN[P], [SU]ABD, [SU]ABA, [SU]HADD, [SU]RHADD, [SU]HSUB, [SU]QADD, [SU]QSUB, CMEQ/CMGT/CMGE/CMHI/CMHS/CMTST (and against zero), [SU]SHL, NEG, ABS, SQABS, SQNEG, CNT, CLZ, CLS, RBIT, REV16/32/64, NOT, AND, BIC, ORR, ORN, EOR, BSL, BIT, BIF |
| Shifts and width changes | SHL, [SU]SHR, [SU]SRA, [SU]RSHR, [SU]RSRA, SLI, SRI, [SU]SHLL[2], XTN[2], SQXTN, UQXTN, SQXTUN, SHRN, RSHRN; three-different [SU]ADDL/W, [SU]SUBL/W, [SU]ABAL, [SU]ABDL, [SU]MLAL, [SU]MLSL, [SU]MULL, [R]ADDHN, [R]SUBHN (all with "2" forms) |
| Vector FP | FADD, FSUB, FMUL, FDIV, FMLA, FMLS (vector and by element), FMULX, FMAX, FMIN, FMAXNM, FMINNM and pairwise forms, FADDP, FABD, FCMEQ/FCMGE/FCMGT (and against zero), FACGE, FACGT, FRECPS, FRSQRTS, FNEG, FABS, FSQRT, FRINT*, FCVT[NPMZA][SU], SCVTF, UCVTF (with fixed-point forms), FCVTL[2], FCVTN[2], FMAXV, FMINV, FMAXNMV, FMINNMV |
| Scalar AdvSIMD | 64-bit ADD/SUB/compare/shift, saturating arithmetic and narrowing, scalar pairwise ADDP/FADDP/FMAX(NM)P/FMIN(NM)P, DUP to scalar, FP compares, FABD, FMULX, FRECPS, FRSQRTS, conversions, by-element FMUL/FMLA/FMLS/FMULX |

Cryptographic forms AESE, AESD, AESIMC, SHA1C, SHA256SU0 and PMULL (64 x 64 -> 128) follow the Arm pseudocode (`AESShiftRows`, `AESSubBytes`, `AESInvMixColumns`, `SHA1C`, `SHA256SU0`, `PolynomialMult`); the AES S-box is computed at compile time from the GF(2^8) inverse and affine transform.

Not implemented: half-precision arithmetic (ARMv8.2), FRECPE/FRSQRTE/URECPE/URSQRTE estimates, FRECPX, SQDMULL/SQDMLAL/SQDMLSL, saturating left shifts by immediate (SQSHL, UQSHL, SQSHLU), PMUL and 8-bit PMULL, PMULL2, FCVTXN, and the other crypto instructions (AESMC, SHA1H/M/P/SU0/SU1, SHA256H/H2/SU1). They are rejected at decode time and, with `--allow-unsupported`, fault precisely when reached.

## Validation

* **Differential execution.** 20 programs ([`simd_programs.py`](../../tools/python/simd_programs.py), 388 distinct instructions) run under the [harness](../../tests/differential/run.py), which compares X0–X30, V0–V31, SP, NZCV, FPCR, FPSR, PC, instruction count and 64 KiB of memory bit for bit with Unicorn 2.1.4. Initial V registers hold FP corner cases in every arrangement (signed zeros, smallest/largest subnormals, smallest normal, largest finite, infinities, quiet and signalling NaNs with payloads, values one ULP above ties, integer-boundary values, half-precision specials) and random patterns. The 16 samples per program cycle FPCR through round-to-nearest, +∞, −∞, zero, FZ, DN and combinations, and FPSR through cleared, IXC-only (exercising the fast path) and all-set. Comparison is exact: no tolerance is applied anywhere.
* **Encodings.** Test programs are written in assembly and encoded with Keystone; an encoding is accepted only if Capstone disassembles it and Keystone re-assembles Capstone's text to the same word ([`encode_simd.py`](../../tools/python/encode_simd.py)). The resulting table is committed, so test runs need neither tool.
* **Compiled code.** [`tests/programs/kernel.rs`](../../tests/programs/kernel.rs) — quaternion normalisation, a 4×4 transform with fused multiply-add over 128 points, min/max/energy reductions, float↔integer conversions and a histogram — is compiled for AArch64 by LLVM (rustc, `opt-level=3`, `target-cpu=cortex-a57`), linked by LLD as a MOD0 module and executed through `link`/`recompile`. The same source compiled for the host is the oracle. The guest uses vector FMLA/FMUL/FADD, FMINNM/FMAXNM, FADDP, LD4/ST4/LD1R, UMULL, SHRN/USHLL, FCVTMS/FCVTZS/UCVTF, FSQRT and FRINTN; it returns hashes identical to the host's for every tested seed (18,522 guest instructions per run) on Linux and as a Windows x86-64 executable.
* **Unit tests** ([`floating_point.rs`](../../crates/wl-runtime/tests/floating_point.rs)) pin values derived from IEEE 754 and the pseudocode: rounding of 0.1 + 0.2, directed rounding of 1/3, invalid operations, NaN priority and quieting, FZ input and output flushing, tininess before rounding, overflow per rounding mode, saturating conversions, FMAXNM, FCMP/FCMPE flags, half-precision overflow, immediate expansion, fast-path equivalence and little-endian lane order.

The oracle models a Cortex-A72; its floating-point and Advanced SIMD architecture is ARMv8.0, as on the Cortex-A57.
