# NRO Probe Test Fixture

## Overview

`probe.nro` is a standalone Nintendo Switch Homebrew NRO0 executable built from `probe.c` using devkitA64 and libnx. It serves as an authoritative, reproducible guest binary fixture for validating:

1. Binary ingestion and validation in `wl-nro` (`Nro::parse`, `Nro::plan`).
2. Conversion to runtime process memory layout (`wl_loader::Process::from_nro`).
3. Execution bootstrap under Wanderlust's Horizon runtime with Homebrew ABI (`X0 = loader_config`, `X1 = 0xFFFFFFFFFFFFFFFF`, returnable `LR`).

## Provenance and Toolchain

- **Source:** [`probe.c`](probe.c) (derived from [`tests/horizon/probe.c`](../../../tests/horizon/probe.c))
- **Compiler:** `aarch64-none-elf-gcc` (devkitA64) 16.1.0
- **C Library:** newlib 4.6.0.20260123
- **Switch Support Library:** libnx 4.12.0 (commit feebd0263b2fed8fdc6fdf14b5ef46efd85e5d72)
- **NRO Conversion Tool:** `elf2nro` (devkitPro tools)
- **Compilation Flags:** `-march=armv8-a+crc+crypto -mtune=cortex-a57 -mtp=soft -fPIE -O2 -g -Wall -ffunction-sections -D__SWITCH__ -specs=switch.specs -lnx`

## Binary Hashes and Layout

- **File size:** 94,208 bytes (0x17000)
- **SHA-256:** `d205f4e9279880373f3df5beaa76fe7d07a68fb2a51ef1864706c6a6f6b5618f`
- **Module ID:** `eb2144c2922ab9416d93dd94f75818b8c6aba1f6000000000000000000000000`
- **Entry point offset:** 0x0000 (starts with `b 0x80`)
- **MOD0 offset:** 0x00F0 (240 bytes)
- **Segments:**
  - **Text:** file offset `0x00000000`, file size `0x00010000` (64 KiB), memory size `0x00010000`, permission `R-X`
  - **RoData:** file offset `0x00010000`, file size `0x00003000` (12 KiB), memory size `0x00003000`, permission `R--`
  - **Data:** file offset `0x00013000`, file size `0x00004000` (16 KiB), memory size `0x00004000`, permission `RW-`
  - **BSS:** declared size `0x00001000` (4 KiB, 1 page), memory offset `0x00017000`, permission `RW-` (zero-filled)
  - **Total mapped memory:** `0x00018000` (96 KiB)

## Independently Derived Expected Output

When recompiled and executed under Wanderlust's Horizon environment (`--horizon --allow-unsupported`), the probe emits three lines to `svcOutputDebugString`:

```text
probe tid=0x72 pid=0x51 handle=0x83ff tls=0x5cbc getinfo_bad_subtype=0xe801 out=0
text state=3 perm=5 size=0x10000; stack state=11 perm=3 size=0x100000 in_stack_region=1
heap_ok=1 sum=952e8000 ratio=0.001953 used_below_total=1
```

### Derivation of Fields

1. **Line 1 (`probe`):**
   - `tid=0x72`: Horizon deterministic main thread ID in `wl-runtime::horizon`.
   - `pid=0x51`: Horizon deterministic process ID in `wl-runtime::horizon`.
   - `handle=0x83ff`: Default 1024-entry handle table (`index 1023 | (1 << 15) = 0x83ff`) provided to the application via `ConfigEntry` key 1 (`EntryType_MainThreadHandle`) and returned by `threadGetCurHandle()`. (Note: Retail NSO probe with 1023-entry NPDM table yields `0x83fe`; default table yields `0x83ff`).
   - `tls=0x5cbc`: Initial TLS value `0x5a5a` + `fib(15)` (610 = `0x262`) = `0x5cbc`.
   - `getinfo_bad_subtype=0xe801`: Horizon kernel result code for invalid subtype on `TotalMemorySize` (`1 | (116 << 9) = 0xe801`, `INVALID_COMBINATION`).
   - `out=0`: Output register reset to 0 on kernel failure.

2. **Line 2 (`memory mappings`):**
   - `text state=3`: `MemoryState::Code` (3).
   - `perm=5`: `MemoryPermission::Rx` (Read = 1, Execute = 4 -> 5).
   - `size=0x10000`: Mapped code region size (64 KiB).
   - `stack state=11`: `MemoryState::Stack` (0x0B = 11).
   - `perm=3`: `MemoryPermission::Rw` (Read = 1, Write = 2 -> 3).
   - `size=0x100000`: Default main thread stack size (1 MiB).
   - `in_stack_region=1`: Stack address located within the Horizon Stack region.

3. **Line 3 (`heap and memory ratio`):**
   - `heap_ok=1`: 16 KiB heap allocation via `malloc` succeeded at or above the base heap address returned by `svcSetHeapSize`.
   - `sum=952e8000`: Algorithmic checksum of 4096 PRNG words (`values[i] = i * 2654435761u; sum ^= values[i] + (sum << 5)`).
   - `ratio=0.001953`: Stack size (1 MiB) / default total memory limit (512 MiB) = `1 / 512 = 0.001953125`, formatted `%.6f`.
   - `used_below_total=1`: Used memory (code + stack + heap + TLS) < total memory limit (512 MiB).
