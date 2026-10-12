# Wanderlust AArch64 ISA Offline Analysis and Reference Tools

This directory provides standalone, offline analysis and reference semantic verification tools for the ARMv8.0-A AArch64 instruction set used by Wanderlust's ahead-of-time recompilation pipeline.

---

## 1. Tools Overview

| Tool | Purpose | Requirements |
|---|---|---|
| [`analyze_gaps.py`](analyze_gaps.py) | Parses Wanderlust analysis outputs (`analysis-sdk.json`) or opcode surveys, prunes non-code data roots, and classifies unsupported instructions into architectural families. | Python 3, Capstone (optional, recommended) |
| [`reference_semantics.py`](reference_semantics.py) | Executes AArch64 instructions on Unicorn Engine to produce authoritative golden test vectors for decoder and runtime verification. | Python 3, Unicorn |

---

## 2. Usage

### A. Opcode Gap and Root Discovery Analysis

Analyze an analysis dataset from `wanderlust survey` or an execution analysis JSON dump:

```bash
# Analyze a JSON analysis output
.venv/bin/python tools/isa/analyze_gaps.py local-inputs/totk/analysis-sdk.json

# Export a structured JSON report
.venv/bin/python tools/isa/analyze_gaps.py local-inputs/totk/analysis-sdk.json --json report.json

# Run built-in classification self-test
.venv/bin/python tools/isa/analyze_gaps.py --test
```

### B. Golden Test Vector Generation

Generate golden reference vectors for CRC32 / CRC32C or SIMD instructions:

```bash
# Print test vectors to stdout
.venv/bin/python tools/isa/reference_semantics.py

# Export golden vectors to JSON
.venv/bin/python tools/isa/reference_semantics.py --json crc32_golden.json
```

---

## 3. Findings Summary from Tears of the Kingdom (`sdk`)

When analyzing the 1,832 unsupported instruction sites recorded during the 1,000,000-instruction exploration of the Switch retail `sdk` module:

- **1,556 sites (84.9%)** are non-code data words (`0x00000000`, `0x00008002`, `0x00000001`) reached due to speculative code-pointer harvesting entering data/alignment pools.
- **276 sites (15.1%)** represent genuine instruction set gaps across 4 primary architectural families:
  1. **System Counter Reads (38 sites):** `mrs Xt, cntpct_el0` (`0xd53be020 | Rt`).
  2. **CRC32 / CRC32C (13 sites):** `crc32b`, `crc32x`, `crc32cb`, `crc32cx` (Data-processing 2-source, opcodes 16..=23).
  3. **Cryptographic Extensions (38 sites):** `aese`, `aesd`, `aesimc`, `sha1c`, `sha256su0`, `pmull`.
  4. **Advanced SIMD (187 sites):** `smull`, `smull2`, `sqdmulh`, `sqrdmulh`, `sqrshrn`, `saddlp`, `smlal`.
