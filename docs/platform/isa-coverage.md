# AArch64 ISA Coverage, Gap Taxonomy & Root Discovery in Tears of the Kingdom

## 1. Executive Summary

During the breakthrough authentic execution of **The Legend of Zelda: Tears of the Kingdom** (v1.0.0 base and v1.2.0 update), Wanderlust successfully executed **48,424,921 guest instructions** through `rtld` dynamic linking and into early `sdk` initialization before pausing for instruction-set gap investigation.

An extensive ahead-of-time (AOT) reachability analysis bounded to 1,000,000 instructions on the authentic Switch `sdk` executable (`local-inputs/totk/analysis-sdk.json`) recorded **1,832 unsupported instruction sites**.

This document establishes the authoritative architectural taxonomy of these sites, categorizing them into:
1. **Non-Code Data Root Artifacts (84.9%, 1,556 sites):** False code roots introduced by unconstrained candidate pointer discovery entering literal data pools and alignment padding.
2. **Genuine Executable Instruction Set Gaps (15.1%, 276 sites):** Real instructions spanning four distinct architectural families:
   - System Counter Reads (`CNTPCT_EL0`)
   - CRC32 and CRC32C (Standard and Castagnoli polynomials)
   - Cryptographic Extensions (`AES`, `SHA-1`, `SHA-256`, `PMULL`)
   - Advanced SIMD (`SMULL`, `SQDMULH`, `SQRDMULH`, `SQRSHRN`)

---

## 2. Non-Code Data Root Phenomenon & Pruning Strategy

### 2.1 Root Cause Analysis

Wanderlust's speculative function pointer discovery scans `.rodata` and `.data` for 64-bit values aligned to 4 bytes that fall within the `.text` virtual address range. In modern optimized Switch binaries:
- Jump tables, dispatch vectors, and virtual method tables (`vtable`) frequently intermingle with literal constants, alignment padding, and metadata pointers.
- Of the 1,832 unsupported sites in `analysis-sdk.json`:
  - **1,309 sites (71.5%)** consist of literal `0x00000000` (`udf #0` or zero-fill padding).
  - **246 sites (13.4%)** consist of literal `0x00008002` (a 16-bit constant/offset `0x8002` in little-endian data).
  - **1 site (0.1%)** consists of literal `0x00000001` (data flag `1`).
  - Total non-code data words: **1,556 sites (84.9%)**.

When an invalid data word is speculatively treated as a function root, the recursive block disassembler attempts to parse non-code bytes until encountering an invalid opcode, artificially inflating the apparent ISA gap by over 5.5x.

### 2.2 Recommended Root Filtering Gates for AOT Analysis

To eliminate false positives without omitting legitimate indirect targets, the AOT analysis pipeline (`wl-analysis`) should apply the following pre-filtering gates before admitting candidate roots:

1. **Zero and Padding Exclusion:** Reject candidate targets whose initial 32-bit word is `0x00000000`, `0x00008002`, or falls within known padding blocks.
2. **Function Prologue Heuristics:**
   - Valid compiler-generated AArch64 function entries overwhelmingly begin with:
     - Stack frame allocation: `stp x29, x30, [sp, -imm]!` (`0xa9b...`)
     - Stack pointer adjustment: `sub sp, sp, #imm` (`0xd10...`)
     - Callee-saved register preservation: `stp x..., x..., [sp, ...]` (`0xa90...`)
     - Return address signing / hint: `paciasp` (`0xd503233f`) or `nop` (`0xd503201f`)
   - Candidate roots beginning with invalid opcodes, raw immediates, or illegal branches must be discarded.
3. **Cross-Referenced Section Provenance:** Validate that indirect pointers originate from genuine `.init_array`, `.fini_array`, or identified C++ virtual tables, rather than raw asset buffers.

---

## 3. Genuine AArch64 Instruction Gap Taxonomy

Filtering non-code data leaves **276 genuine instruction sites** across **76 unique opcodes**.

| Architectural Family | Site Count | Relative % of Real Gaps | Key Opcodes |
|---|---|---|---|
| **System Counter Reads** | 38 | 13.8% | `mrs Xt, cntpct_el0` |
| **CRC32 / CRC32C** | 13 | 4.7% | `crc32b`, `crc32x`, `crc32cb`, `crc32cx` |
| **Cryptographic Extensions** | 38 | 13.8% | `aese`, `aesd`, `aesimc`, `sha1c`, `sha256su0`, `pmull` |
| **Integer Advanced SIMD** | 187 | 67.8% | `smull`, `smull2`, `sqdmulh`, `sqrdmulh`, `sqrshrn`, `saddlp`, `smlal` |
| **Total Genuine Gaps** | **276** | **100.0%** | |

---

## 4. Deep Architectural Specification

### 4.1 System Counter Registers (`CNTPCT_EL0`)

Nintendo Switch applications and the Nintendo SDK rely extensively on the ARM Generic Timer virtual/physical counters for timing, delta calculation, and profiling.

#### Encodings
- `mrs Xt, cntpct_el0`: `0xd53be020 | Rt`
  - Bit layout: `1101 0101 0011 1011 1110 0000 001t tttt`
  - System register encoding: `op0=3, op1=3, CRn=14, CRm=0, op2=1` (`0x5F01`)
- `mrs Xt, cntvct_el0`: `0xd53be040 | Rt` (`0x5F02`)
- `mrs Xt, cntfrq_el0`: `0xd53be000 | Rt` (`0x5F00`)

#### Observed TOTK Usage in `sdk`
- Total: 38 sites
- Target registers:
  - `x0`: 10 sites (`0xd53be020`)
  - `x8`: 11 sites (`0xd53be028`)
  - `x19`: 2 sites (`0xd53be033`)
  - `x20`: 7 sites (`0xd53be034`)
  - `x21`: 6 sites (`0xd53be035`)
  - `x22`: 2 sites (`0xd53be036`)

#### Recompilation & Host Semantics
- **Clock Frequency:** The Nintendo Switch hardware timer ticks at **19.2 MHz** (19,200,000 Hz, period ≈ 52.083 ns).
- **Host Execution:** On Windows x86-64, map to host performance counters:
  ```rust
  // Scaled QueryPerformanceCounter or virtual deterministic clock ticks:
  let ticks = (host_qpc * 19_200_000) / qpc_frequency;
  c.x[t] = ticks;
  ```
- **Permission:** EL0 read-only; attempts to execute `msr cntpct_el0, Xt` must raise `unsupported(word, pc)`.

---

### 4.2 CRC32 and CRC32C (Data-Processing 2-Source)

CRC32 instructions are part of the ARMv8.0-A CRC32 extension, used in game engines for asset hashing, checksum validation, and string hashing.

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
  - Reserved rule: `(sz == 3 && sf == 0) || (sz < 3 && sf == 1)` is reserved and must fault.

#### Observed TOTK Usage in `sdk`
- `crc32b w8, w8, w11`: 3 sites (`0x1acb4108`)
- `crc32b w8, w8, w10`: 1 site (`0x1aca4108`)
- `crc32b w8, w8, w13`: 1 site (`0x1acd4108`)
- `crc32x w8, w8, x4`: 1 site (`0x9ac44d08`)
- `crc32x w8, w8, x3`: 1 site (`0x9ac34d08`)
- `crc32x w8, w8, x13`: 1 site (`0x9acd4d08`)
- `crc32x w8, w8, x9`: 1 site (`0x9ac94d08`)
- `crc32cb w8, w8, w11`: 2 sites (`0x1acb5108`)
- `crc32cb w8, w8, w13`: 1 site (`0x1acd5108`)
- `crc32cx w8, w8, x9`: 1 site (`0x9ac95d08`)

#### Host Execution Mapping
- **CRC-32C (`C = true`):** Maps directly to hardware x86-64 SSE4.2 instructions:
  - 8-bit: `_mm_crc32_u8(acc, val)`
  - 16-bit: `_mm_crc32_u16(acc, val)`
  - 32-bit: `_mm_crc32_u32(acc, val)`
  - 64-bit: `_mm_crc32_u64(acc, val)`
- **Standard CRC-32 (`C = false`):** Bit-reflected bitwise table lookup using standard IEEE polynomial `0xEDB88320`.

---

### 4.3 Cryptographic Extensions

Tears of the Kingdom uses hardware-accelerated cryptographic primitives for save-game integrity, asset authentication, and network communication.

1. **AES Instructions:**
   - `AESE Vd.16B, Vn.16B` (`0x4E284800`): AddRoundKey + SubBytes + ShiftRows.
   - `AESD Vd.16B, Vn.16B` (`0x4E285800`): AddRoundKey + InvSubBytes + InvShiftRows.
   - `AESIMC Vd.16B, Vn.16B` (`0x4E287800`): InvMixColumns.
   - Host mapping: x86 AES-NI instructions (`_mm_aesenc_si128`, `_mm_aesdec_si128`, `_mm_aesimc_si128`).
2. **SHA Instructions:**
   - `SHA1C Qd, Sn, Vm.4S` (`0x5E000000`): SHA-1 hash computation (round 0..19 function).
   - `SHA256SU0 Vd.4S, Vn.4S` (`0x5E282800`): SHA-256 schedule update stage 0.
   - Host mapping: x86 SHA-NI instructions or high-performance software rollouts.
3. **Carry-less Multiplication:**
   - `PMULL Vd.1Q, Vn.1D, Vm.1D` (`0x0EE0E000`): 64x64 carry-less multiplication producing 128-bit result.
   - Host mapping: x86 PCLMULQDQ instruction (`_mm_clmulepi64_si128`).

---

### 4.4 Advanced SIMD (NEON) Integer Operations

Representing 67.8% of real gaps (187 sites), these instructions perform DSP audio processing, physics, and vector transforms:

1. **Widening Multiply (`SMULL`, `SMULL2` - 58 sites):**
   - Multiplies lower or upper half of vector elements, widening from 16-bit to 32-bit (`.4H -> .4S`) or 32-bit to 64-bit (`.2S -> .2D`).
   - High-frequency variant: `SMULL Vd.2D, Vn.2S, Vm.S[index]` (vector-by-scalar).
2. **Saturating Doubling Multiply High (`SQDMULH`, `SQRDMULH` - 91 sites):**
   - Multiplies corresponding elements, doubles result with signed saturation, and extracts high half (with optional rounding in `SQRDMULH`).
   - Host mapping: x86 SSSE3 `_mm_mulhrs_epi16` for 16-bit rounded variant.
3. **Saturating Round Shift Right Narrow (`SQRSHRN` - 33 sites):**
   - Shifts right with rounding, narrows element width with signed saturation (`.2D -> .2S` or `.4S -> .4H`).

---

## 5. Delivered Software & Verification Substrate

The following components are implemented, tested, and verified on `main`:

1. **`crates/wl-aarch64::extensions`:**
   - Strictly validates and decodes `SystemCounterOp`, `Crc32Op`, and `CryptoOp`.
   - Bounded unit tests in `crates/wl-aarch64/tests/extensions.rs` verifying all combinations and negative reserved-encoding rejection.
   - 21 passing tests in `wl-aarch64`.
2. **`tools/isa/analyze_gaps.py`:**
   - Automated offline classification and frequency analyzer.
   - Filters data/padding artifacts.
   - Built-in self-test verified against authentic TOTK opcodes.
3. **`tools/isa/reference_semantics.py`:**
   - Unicorn-based reference test vector generator producing authoritative input-output pairs for all 8 CRC32/CRC32C width and polynomial configurations.

---

## 6. Integration Contract for Claude (October 14 Return)

To integrate these decoded extensions into the compilation and execution pipeline:

1. **`crates/wl-aarch64::Op` & `SystemRegister`:**
   - Claude may promote `CounterRegister` variants (`CntpctEl0`, `CntvctEl0`, `CntfrqEl0`) into `SystemRegister`, and `Crc32Op` into `Op::Crc32`.
2. **`crates/wl-codegen-rust`:**
   - In `generate_block`:
     - Map `SystemRegister::CntpctEl0` to guest timer query:
       ```rust
       SystemRegister::CntpctEl0 => "m.get_system_tick()",
       ```
     - Map `Op::Crc32` to host intrinsic / software CRC helper:
       ```rust
       Op::Crc32 { dst, acc, val, sz, c } => {
           let helper = if c { "host_crc32c" } else { "host_crc32" };
           writeln!(out, "let res = {helper}({}, {}, {sz});", read(acc), read(val))?;
           out.push_str(&write(dst, "res", Width::W32));
       }
       ```
3. **`crates/wl-analysis`:**
   - Include `Op::Crc32` in pure register operations (no branch, no memory effect).
   - Apply the recommended root filtering heuristics in `find_indirect_targets` and candidate root selection to prune the 1,556 non-code data words.
