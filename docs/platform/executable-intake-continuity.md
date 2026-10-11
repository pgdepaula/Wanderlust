# Wanderlust Executable Intake & Binary Validation Continuity Record

**Subsystem Owner:** Agy (`wanderlust/ai:agy`)
**Scope:** `crates/wl-npdm/**`, `crates/wl-nro/**`, `docs/platform/npdm.md`, `docs/platform/nro.md`
**Shared Worktree:** `main` branch under Git mutex `.git/wanderlust-integration.lock`
**MCP Persistent Namespace:** `wanderlust/shared`

---

## 1. Mission and Pipeline Architecture

Wanderlust recompiles Nintendo Switch AArch64 binaries ahead-of-time to native Windows x86-64 to run *The Legend of Zelda: Tears of the Kingdom* (Title ID `0x0100f2c0115b6000`).

The executable intake subsystem sits at the entry of the execution pipeline:
1. **Intake & Verification**: Bounded structural validation of NSO0, NRO0, and NPDM formats (`wl-npdm`, `wl-nro`).
2. **Process Layout & Relocation Contract**: Construction of process image (`wl-loader`), preserving guest self-relocation boundaries (`LinkMode::Guest` vs `LinkMode::Host`).
3. **Control-Flow Analysis**: Identifying candidate code pointers and entry points for AOT translation (`wl-analysis`).
4. **Native Execution**: Horizon OS process creation, threading, SVC dispatch, and HIPC/CMIF service manager (`wl-runtime`).
5. **GPU Translation**: Maxwell B06F/B197 FIFO decoding, binary shader translation, and host rendering (`wl-gpu`).

---

## 2. Current Authoritative Public Interfaces

### 2.1 Process Metadata & ExeFS Audit (`wl-npdm`)
- `wl_npdm::Npdm::parse(bytes: &[u8]) -> Result<Npdm, NpdmError>`: Zero-allocation parser for `main.npdm` (META, ACID, ACI0).
- `wl_npdm::Npdm::to_process_metadata(&self) -> Result<ProcessMetadata, NpdmError>`: Extracts kernel capability permissions (core mask, priority mask, 39-bit address space, handle table size, syscall bitmap).
- `wl_npdm::audit_exefs(dir: &Path) -> Result<ExeFsReadinessReport, AuditError>`: Read-only readiness gate evaluating decrypted retail ExeFS packages.
- `wl_npdm::ExeFsReadinessReport`: Structured diagnostics containing module audits, link mode, symbol resolution state, init order, candidate code pointers, and missing runtime services.

### 2.2 Homebrew Executable Intake (`wl-nro`)
- `wl_nro::Nro::parse(bytes: &[u8]) -> Result<Nro, NroError>`: Zero-allocation parser for NRO0 format, MOD0 headers, and optional ASET asset trailers.
- `wl_nro::Nro::plan(&self) -> Result<HomebrewImagePlan, NroError>`: Produces execution layout containing text, rodata, data, and BSS extents with page alignment.
- `wl_nro::plan::RelocationRequirement::GuestSelfRelocating`: Enforces that libnx crt0 performs self-relocation.

---

## 3. Important Architectural Invariants

1. **Strict Separation of Address Domains**: Container file offsets, module-relative offsets, guest virtual addresses, and host virtual addresses are distinct typed spaces. Never collapse them through unchecked casts.
2. **Static Readiness vs Runtime Execution**: `audit_exefs` guarantees `runtime_execution_occurred == false`. Static structural analysis must never be reported as an executed game.
3. **Relocation Ownership**:
   - Retail ExeFS with `rtld`: Uses `LinkMode::Guest`. `rtld` relocates `main`, `subsdk*`, and `sdk` at guest startup.
   - Homebrew NRO: Uses `HomebrewImagePlan` with `RelocationRequirement::GuestSelfRelocating`. Loader must NOT apply relocations before guest startup.
4. **Fail-Closed Capability Model**: Unknown or unsupported capability descriptors in NPDM must never silently grant privileges.

---

## 4. Verified Capabilities & Empirically Audited Milestones

- **Tears of the Kingdom Retail ExeFS Audit (Verified 2026-10-11)**:
  - Both `local-inputs/totk/base-v0-exefs` and `local-inputs/totk/update-v720896-exefs` pass `audit_exefs` with `aot_analysis_ready = true`.
  - Title ID: `0x0100f2c0115b6000`, 39-bit address space (`AddressSpace64Bit39`), handle table size 512, stack 1 MiB, system resource 24 MiB, core 0, prio 44.
  - Multi-module process linking verified via `wanderlust link`: `rtld`, `main`, `subsdk0`, and `sdk` linked with 198,816 candidate code pointers (base) and 193,327 candidate code pointers (update).
  - Unresolved symbols in retail TOTK are strictly weak references (e.g. `glslcCompile`, `nnosGetDefaultUserExceptionStackForApplication`); zero fatal strong unresolved symbols.
- **Homebrew ABI & Probe Execution (Verified 2026-10-11)**:
  - `probe.nro` executed successfully under Claude's scheduler (commit `2f5eaf4`), printing 3 probe lines and returning status 0.
  - `display_probe.nro` executed 6,838 guest instructions natively under `wanderlust recompile --horizon`.

---

## 5. Current Technical Blockers & Next Actions

1. **`sm:` Service Registration for Display**:
   - `display_probe.nro` requests `vi:u` via `sm:`.
   - In `wl-runtime`, unregistered services are deferred, causing the calling thread to wait indefinitely.
   - Next step for display: Claude / Codex coordination on registering `vi:u` mock or relay service session to prevent deadlock.
2. **Missing Runtime Services Inventory**:
   - TOTK Base declares access to 47 services; Update v720896 declares access to 48 services (adds `ngc:u`).
   - Earliest boot services required by Horizon `appletOE` / `set` / `nvdrv` must be modeled incrementally.

---

## 6. Session Reconstruction Protocol

To resume an Agy session efficiently:
1. Run `git rev-parse HEAD` and `git status`.
2. Inspect MCP inbox: `memory.py agy memory_signal_inbox '{"limit": 10}'`.
3. Check active leases: `reserve.py list agy`.
4. Review this continuity document (`docs/platform/executable-intake-continuity.md`).
5. Run targeted tests: `cargo test -p wl-npdm -p wl-nro --locked`.
