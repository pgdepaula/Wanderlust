# AArch64 ISA Coverage, Gap Taxonomy & Root Discovery in Tears of the Kingdom

## 1. Executive Summary & Scope Boundary

During authentic execution of **The Legend of Zelda: Tears of the Kingdom** (v1.0.0 base and v1.2.0 update), Wanderlust executed **48,424,921 guest instructions** through `rtld` dynamic self-relocation and into early Nintendo SDK (`sdk`) initialization before pausing for instruction-set gap investigation. This milestone represents genuine execution of the game's original executable code in the startup path; it is not a complete game boot or rendered frame.

An ahead-of-time (AOT) reachability analysis bounded to 1,000,000 instructions on the authentic Switch `sdk` executable (`local-inputs/totk/analysis-sdk.json`) recorded **1,832 unsupported instruction sites**.

This document establishes the authoritative architectural taxonomy of these sites, categorizing them into:
1. **Probable Non-Code Data Root Artifacts (84.9%, 1,556 sites):** Addresses containing non-code data words reached due to unconstrained candidate code-pointer harvesting entering literal data pools and alignment padding.
2. **Recognized Instruction Set Gaps (15.1%, 276 sites across 76 unique opcodes):** Valid instruction encodings spanning four distinct architectural families:
   - System Counter Reads (`CNTPCT_EL0`, `CNTVCT_EL0`, `CNTFRQ_EL0`)
   - CRC32 and CRC32C (Standard and Castagnoli polynomials)
   - Cryptographic Extensions (exact recognized forms of `AESE`, `AESD`, `AESIMC`, `SHA1C`, `SHA256SU0`, `PMULL`)
   - Advanced SIMD (`SMULL`, `SMULL2`, `SQDMULH`, `SQRDMULH`, `SQRSHRN`, etc.)

> [!IMPORTANT]
> **Engineering Scope Boundary & AOT Execution Status:**
> This workstream delivers validated instruction decoding, taxonomy classification, and independent reference semantic verification. It does **not** establish end-to-end AOT execution for these operations. Lowering in `wl-codegen-rust`, control-flow dataflow in `wl-analysis`, and runtime execution in `wl-runtime` remain strictly owned by Claude for integration starting October 14.

---

## 2. Non-Code Data Root Analysis & Candidate Pruning Guidelines

### 2.1 Phenomenon & Data Distribution

Wanderlust's speculative function pointer discovery scans `.rodata` and `.data` for 64-bit values aligned to 4 bytes that point within the `.text` virtual address range. In modern optimized Switch binaries:
- Jump tables, dispatch vectors, and virtual method tables (`vtable`) intermingle with literal constants, alignment padding, and metadata pointers.
- Of the 1,832 unsupported sites in `analysis-sdk.json`:
  - **1,309 sites (71.5%)** consist of literal `0x00000000` (`udf #0` or zero-fill padding).
  - **246 sites (13.4%)** consist of literal `0x00008002` (a 16-bit constant/offset `0x8002` in little-endian data).
  - **1 site (0.1%)** consists of literal `0x00000001` (data flag `1`).
  - Total non-code data words: **1,556 sites (84.9%)**.

### 2.2 Distinguishing Data Origins from Control-Flow Reachability

- **Probable Data Origins vs. Proven Unreachability:** An address containing `0x00000000`, `0x00008002`, or another data-like pattern indicates a probable data origin rather than control-flow-proven unreachability.
- **Recognized Encodings vs. Reached Execution:** Conversely, recognizing a valid instruction opcode in the binary does not prove that execution reaches that location during normal gameplay.
- **Candidate Guidelines (Not Rigid Enforcements):**
  - Heuristics such as checking for standard function prologue patterns (`stp x29, x30, [sp, -N]!`, `sub sp, sp, #N`) are **candidate guidelines** for future evaluation by Claude, not mandatory filters.
  - Mandating standard prologues would erroneously reject legitimate frameless leaf functions, tail-call targets, or hand-written assembly routines.
  - Zero/padding exclusions (`0x00000000`) and cross-referencing against verified metadata sections (`.init_array`, `.fini_array`) provide a safer initial pruning filter.

---

## 3. Genuine AArch64 Instruction Gap Taxonomy

Filtering probable data words leaves **276 genuine instruction sites** across **76 unique opcodes**:

| Architectural Family | Site Count | Relative % of Real Gaps | Key Encodings Recognized |
|---|---|---|---|
| **System Counter Reads** | 38 | 13.8% | `mrs Xt, cntpct_el0` (exact opcodes for x0, x8, x19..x22) |
| **CRC32 / CRC32C** | 13 | 4.7% | `crc32b`, `crc32x`, `crc32cb`, `crc32cx` |
| **Cryptographic Extensions** | 38 | 13.8% | Exact forms of `aese`, `aesd`, `aesimc`, `sha1c`, `sha256su0`, `pmull` |
| **Integer Advanced SIMD** | 187 | 67.8% | `smull`, `smull2`, `sqdmulh`, `sqrdmulh`, `sqrshrn`, `saddlp`, `smlal`, `frecpe` |
| **Total Genuine Gaps** | **276** | **100.0%** | |

---

## 4. Architectural Encodings & Decoder Scope

### 4.1 System Counter Registers (`CNTPCT_EL0`)

Nintendo Switch applications and the Nintendo SDK rely on the ARM Generic Timer virtual/physical counters for timing, delta calculation, and profiling.

#### Encodings
- `mrs Xt, cntpct_el0`: `0xd53be020 | Rt`
  - Bit layout: `1101 0101 0011 1011 1110 0000 001t tttt`
  - System register encoding: `op0=3, op1=3, CRn=14, CRm=0, op2=1` (`0x5F01`)
- `mrs Xt, cntvct_el0`: `0xd53be040 | Rt` (`0x5F02`)
- `mrs Xt, cntfrq_el0`: `0xd53be000 | Rt` (`0x5F00`)

#### Observed Usage in TOTK `sdk` (38 sites)
- Target registers: `x0` (10x), `x8` (11x), `x19` (2x), `x20` (7x), `x21` (6x), `x22` (2x).

#### Recompilation & Host Semantics (for Claude's integration)
- **Clock Frequency:** Switch hardware timer ticks at **19.2 MHz** (19,200,000 Hz, period ≈ 52.083 ns).
- **Host Execution:** On Windows x86-64, map to host performance counters (`QueryPerformanceCounter` or virtual deterministic ticks).
- **Permissions:** EL0 read-only; attempts to execute `msr cntpct_el0, Xt` are strictly rejected by the decoder.

---

### 4.2 CRC32 and CRC32C (Data-Processing 2-Source)

CRC32 instructions belong to the ARMv8.0-A CRC32 extension, used for asset hashing, checksum validation, and string hashing.

#### Architectural Encoding
- Class: Data-processing (2 source), `word & 0x7fe00000 == 0x1ac00000`
- Format: `sf:1 | 0:1 | S:1 | 11010110 | Rm:5 | 010 | C:1 | sz:2 | Rn:5 | Rd:5`
- Bit fields:
  - `C = 0`: Standard ISO 3309 / Ethernet CRC-32 (polynomial `0x04C11DB7`)
  - `C = 1`: Castagnoli CRC-32C (polynomial `0x1EDC6F41`)
  - `sz`:
    - `0b00` (0): Byte (8-bit), requires `sf = 0` (`crc32b` / `crc32cb`)
    - `0b01` (1): Halfword (16-bit), requires `sf = 0` (`crc32h` / `crc32ch`)
    - `0b10` (2): Word (32-bit), requires `sf = 0` (`crc32w` / `crc32cw`)
    - `0b11` (3): Doubleword (64-bit), requires `sf = 1` (`crc32x` / `crc32cx`)
  - Reserved rule: `(sz == 3 && sf == 0) || (sz < 3 && sf == 1)` is reserved and rejected.

#### Observed Usage in TOTK `sdk` (13 sites)
- `crc32b`: `w8, w8, w11` (3x), `w8, w8, w10` (1x), `w8, w8, w13` (1x)
- `crc32x`: `w8, w8, x4` (1x), `w8, w8, x3` (1x), `w8, w8, x13` (1x), `w8, w8, x9` (1x)
- `crc32cb`: `w8, w8, w11` (2x), `w8, w8, w13` (1x)
- `crc32cx`: `w8, w8, x9` (1x)

#### Host Execution Mapping (for Claude's integration)
- **CRC-32C (`C = true`):** Maps to hardware x86-64 SSE4.2 intrinsics (`_mm_crc32_u8/u16/u32/u64`).
- **Standard CRC-32 (`C = false`):** Software bit-reflected table lookup using standard IEEE polynomial `0xEDB88320`.

---

### 4.3 Cryptographic Extensions (Recognized Subsets)

The extension decoder recognized the exact forms present in the TOTK analysis:
- `AESE Vd.16B, Vn.16B` (`0x4E284800`, mask `0xFFFFFC00`): AddRoundKey + SubBytes + ShiftRows.
- `AESD Vd.16B, Vn.16B` (`0x4E285800`, mask `0xFFFFFC00`): AddRoundKey + InvSubBytes + InvShiftRows.
- `AESIMC Vd.16B, Vn.16B` (`0x4E287800`, mask `0xFFFFFC00`): InvMixColumns.
- `SHA1C Qd, Sn, Vm.4S` (`0x5E000000`, mask `0xFFE0FC00`): SHA-1 round computation.
- `SHA256SU0 Vd.4S, Vn.4S` (`0x5E282800`, mask `0xFFFFFC00`): SHA-256 schedule update stage 0.
- `PMULL Vd.1Q, Vn.1D, Vm.1D` (`0x0EE0E000`, mask `0xFFE0FC00`): 64x64 carry-less multiplication.

*(Note: The decoder does not claim full coverage of all ARMv8 cryptographic variants outside these observed encodings).*

---

### 4.4 Advanced SIMD (NEON) Integer Operations

Representing 67.8% of real gaps (187 sites):
1. **Widening Multiply (`SMULL`, `SMULL2` - 58 sites):** `.4H -> .4S` or `.2S -> .2D`. High-frequency form: vector-by-scalar.
2. **Saturating Doubling Multiply High (`SQDMULH`, `SQRDMULH` - 91 sites):** Multiplies, doubles with signed saturation, extracts high half. Host mapping: SSSE3 `_mm_mulhrs_epi16`.
3. **Saturating Round Shift Right Narrow (`SQRSHRN` - 33 sites):** Shifts right with rounding, narrows with signed saturation.

---

## 5. Delivered Software & Verification Substrate

The following deliverables are committed, tested, and verified on `main`:

1. **`crates/wl-aarch64::extensions`:**
   - Strict semantic decoders: `decode_system_counter`, `decode_crc32`, `decode_crypto`, and unified `decode_extension`.
   - Rejects EL0 writes to system counters.
   - Rejects reserved CRC32 forms.
   - Comprehensive unit tests in `crates/wl-aarch64/tests/extensions.rs` (21 passing tests in `wl-aarch64`).
2. **`tools/isa/analyze_gaps.py`:**
   - Standalone CLI analysis tool with data-filtering and JSON report export.
   - Built-in self-test passed against authentic TOTK test vectors.
3. **`tools/isa/reference_semantics.py`:**
   - Unicorn Engine-based reference vector generator covering all 8 CRC32/CRC32C width/polynomial combinations.
   - Tested across edge cases (all-zeros, all-ones seeds, maximum inputs).
   - Built-in self-assertion against independent known golden constants.
4. **`tools/isa/README.md`:**
   - Clear usage instructions and command references.

---

## 6. Integration Contract for Claude (October 14 Return)

To integrate these decoded extensions into the compilation and execution pipeline:

1. **Decoder Integration in `crates/wl-aarch64`:**
   - Promote `CounterRegister` variants into `SystemRegister`, and `Crc32Op` into `Op::Crc32`.
2. **Analysis Integration in `crates/wl-analysis`:**
   - Add `Op::Crc32` to pure register dataflow (no branch, no memory effect).
   - Evaluate candidate root filtering heuristics in `find_indirect_targets` with appropriate care for frameless functions.
3. **Code Generation in `crates/wl-codegen-rust`:**
   - Lower `SystemRegister::CntpctEl0` to guest timer query (`m.get_system_tick()`).
   - Lower `Op::Crc32` to host x86-64 SSE4.2 intrinsic or software table lookup.
4. **Runtime & Scheduler in `crates/wl-runtime`:**
   - Ensure `get_system_tick()` scales virtual deterministic time appropriately.
5. **Next CPU Milestone:**
   - Re-running TOTK startup under AOT with System Counter and CRC32 lowered to observe the next stopping condition.
