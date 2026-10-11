# NRO Test Fixtures Inventory

## Overview

This directory provides authoritative, reproducible Nintendo Switch Homebrew NRO0 executables compiled directly with the installed devkitA64 toolchain and libnx. These fixtures define clear binary compatibility gates for Wanderlust's Horizon OS and graphics recompilation pipeline.

---

## 1. Fixture Inventory & Hashes

| Fixture | Source | Size | SHA-256 | Purpose |
|---|---|---|---|---|
| `probe.nro` | [`probe.c`](probe.c) | 94,208 | `d205f4e9279880373f3df5beaa76fe7d07a68fb2a51ef1864706c6a6f6b5618f` | Memory layout, stack, TLS, handles, heap (`malloc`), and debug output |
| `ipc_probe.nro` | [`ipc_probe.c`](ipc_probe.c) | 94,208 | `77ce3acb0385f5886b08763d3bd66f22d0192a663b7105bb5005bac7a3c39c7e` | Service Manager (`sm:`) IPC connection, service lookup (`time:u`), handle validation, and clean teardown |
| `display_probe.nro` | [`display_probe.c`](display_probe.c) | 98,304 | `fbbaefd334c8d8e57636ac1ee90db17bcce38a908355e025371906e3e6a8c686` | VI display service initialization (`vi:u`), default display opening, and resolution query |

---

## 2. Toolchain Provenance & Compilation Recipe

All fixtures are compiled reproducibly using:
- **Compiler:** `aarch64-none-elf-gcc` (devkitA64) 16.1.0
- **C Library:** newlib 4.6.0.20260123
- **Switch Runtime:** libnx 4.12.0 (commit `feebd0263b2fed8fdc6fdf14b5ef46efd85e5d72`)
- **Container Tool:** `elf2nro` (devkitPro tools)

### Build Recipe
```bash
aarch64-none-elf-gcc -march=armv8-a+crc+crypto -mtune=cortex-a57 -mtp=soft -fPIE -O2 -g -Wall \
  -ffunction-sections -D__SWITCH__ -I $DEVKITPRO/libnx/include <name>.c \
  -specs=$DEVKITPRO/libnx/switch.specs -L $DEVKITPRO/libnx/lib -lnx -o <name>.elf

elf2nro <name>.elf <name>.nro
rm <name>.elf
```

### Segment Geometries
- **`probe.nro` & `ipc_probe.nro` (94,208 bytes):**
  - **Text:** offset `0x00000000`, size `0x00010000` (64 KiB), permissions `R-X`
  - **RoData:** offset `0x00010000`, size `0x00003000` (12 KiB), permissions `R--`
  - **Data:** offset `0x00013000`, size `0x00004000` (16 KiB), permissions `RW-`
  - **BSS:** declared size `0x00001000` (4 KiB, 1 page), memory offset `0x00017000`, permissions `RW-` (zero-filled)
  - **Total mapped memory:** `0x00018000` (96 KiB)
- **`display_probe.nro` (98,304 bytes):**
  - **Text:** offset `0x00000000`, size `0x00011000` (68 KiB), permissions `R-X`
  - **RoData:** offset `0x00011000`, size `0x00003000` (12 KiB), permissions `R--`
  - **Data:** offset `0x00014000`, size `0x00004000` (16 KiB), permissions `RW-`
  - **BSS:** declared size `0x00001000` (4 KiB, 1 page), memory offset `0x00018000`, permissions `RW-` (zero-filled)
  - **Total mapped memory:** `0x00019000` (100 KiB)
- **MOD0 dynamic header:** offset `0x00F0` (240 bytes)
- **Relocation Requirement:** `RelocationRequirement::GuestSelfRelocating` (`__nx_dynamic` crt0)

---

## 3. Test Expectations & Blocker Analysis

### A. `probe.nro`
Exercises standard Horizon runtime bootstrap without external service dependencies.
- **Expected debug output lines:**
  ```text
  probe tid=0x72 pid=0x51 handle=0x83ff tls=0x5cbc getinfo_bad_subtype=0xe801 out=0
  text state=3 perm=5 size=0x10000; stack state=11 perm=3 size=0x100000 in_stack_region=1
  heap_ok=1 sum=952e8000 ratio=0.001953 used_below_total=1
  ```
- **Exit convention:** Returns through `LR` (`X30`) to runtime return sentinel.

---

### B. `ipc_probe.nro`
Exercises Service Manager connection and user service discovery.
- **Expected debug output with implemented `sm:` IPC:**
  ```text
  ipc_probe sm_init rc=0x0
  ipc_probe lookup srv=time:u rc=0x0 handle=0x...
  ipc_probe close srv=time:u rc=0x0
  ipc_probe sm_exit ok
  ```
- **Current Runtime Status:**
  - **Earliest failure point:** `smInitialize()` calls `svcConnectToNamedPort(&g_smSession, "sm:")`.
  - **SVC Number:** 0x1F (`svcConnectToNamedPort`).
  - **Fault category under Wanderlust:** `unsupported-service` (SVC 0x1F).
  - **Resolution required:** Implementation of named port `"sm:"` connection in `wl-runtime::horizon`.

---

### C. `display_probe.nro`
Exercises display service acquisition and initialization sequence leading toward graphics presentation.
- **Expected debug output with implemented `vi:` service:**
  ```text
  display_probe sm_init rc=0x0
  display_probe vi_init rc=0x0
  display_probe open_display rc=0x0 id=0x1
  display_probe get_res rc=0x0 width=1920 height=1080
  display_probe close_display rc=0x0
  display_probe vi_exit ok
  display_probe sm_exit ok
  ```
- **Current Runtime Status & Blocker Analysis:**
  - `display_probe.c` initializes Service Manager via `smInitialize()` before calling `viInitialize()`.
  - Connects to named port `"sm:"` via `svcConnectToNamedPort` (SVC 0x1F) and registers client.
  - Sends CMIF Command 1 (`GetService`, name `"vi:u"`) to `sm:`.
  - **Current status under Wanderlust:** `sm:` is functional (commit `3347fca`), but `"vi:u"` has not yet been registered. As designed by Mesosphere / Horizon service manager semantics, unregistered lookups defer until registered. When no threads remain runnable to register `"vi:u"`, the deterministic scheduler flags a clean `deadlock` fault naming service `"vi:u"`.
  - **Next requirement:** Implementation of `vi:u` service dispatch (`IApplicationDisplayService`: `OpenDisplay`, `GetDisplayResolution`, `CloseDisplay`) in Horizon runtime.
