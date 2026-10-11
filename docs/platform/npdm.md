# Nintendo Process Descriptor Meta (NPDM) Ingestion

This document specifies how Wanderlust ingests and validates Nintendo Process Descriptor Meta (`main.npdm`) files from ExeFS containers to configure Horizon process execution environments, kernel capabilities, service access rights, and memory limits.

Implementation: [`wl-npdm`](../../crates/wl-npdm/src/lib.rs), [`capability`](../../crates/wl-npdm/src/capability.rs), [`access_control`](../../crates/wl-npdm/src/access_control.rs). Tests: [`validation`](../../crates/wl-npdm/tests/validation.rs). Synthetic fixtures only; no proprietary or game-derived binaries are committed.

## Sources

| Source | Reference / Revision | Used for | Reuse |
|---|---|---|---|
| Switchbrew [NPDM](https://switchbrew.org/wiki/NPDM) | rev. 15075 | Section layout, descriptor bit patterns, filesystem permissions | facts only |
| Atmosphère `stratosphere/loader/source/ldr_meta.cpp`, `ldr_types.hpp` | [master](https://github.com/Atmosphere-NX/Atmosphere) | Header structures, subregion validation invariants, ACID verification | GPL-2.0: structural definitions and invariants restated |
| Atmosphère `stratosphere/loader/source/ldr_capabilities.cpp` | [master](https://github.com/Atmosphere-NX/Atmosphere) | Authoritative Kernel Capability (KAC) bitfield decoders and constraints | GPL-2.0: algorithmic decoders restated |
| libnx `switch_crt0.s`, `kernel/svc.h` | [feebd026](https://github.com/switchbrew/libnx/tree/feebd026ca0f5dcc2119f46ad8e0d16ad3dd4973) | Process startup assumptions, initial stack constraints, SVC numbers | facts only |

## Container Structure

An NPDM file resides at `/main.npdm` within the application ExeFS partition. It consists of three primary contiguous or bounded sections:

1. **META Header (`0x00..0x80`)**: Process metadata, memory architecture, scheduling defaults, and section offsets.
2. **ACID (Access Control Information Descriptor)**: Factory authorization envelope including RSA-2048 signature, public modulus, and maximum permissible capability bounds.
3. **ACI0 (Access Control Information 0)**: Process-specific requested capabilities, target Program ID, and service bindings.

```text
+-------------------------------------------------------+ 0x00
| META Header (0x80 bytes, Magic: 'META')               |
+-------------------------------------------------------+
| ACID Header (0x240 bytes base, Magic: 'ACID')         | <- acid_offset
|   - RSA-2048 Signature (0x100 bytes)                  |
|   - RSA-2048 Public Modulus (0x100 bytes)             |
|   - Header fields, Program ID Range, Subregion Offsets|
|   - ACID FAC (Filesystem Access Control)              |
|   - ACID SAC (Service Access Control)                 |
|   - ACID KAC (Kernel Access Control Descriptors)      |
+-------------------------------------------------------+
| ACI0 Header (0x40 bytes base, Magic: 'ACI0')          | <- aci_offset
|   - Program ID (Title ID)                             |
|   - ACI0 FAH (Filesystem Access Header)               |
|   - ACI0 SAC (Service Access Control)                 |
|   - ACI0 KAC (Kernel Access Control Descriptors)      |
+-------------------------------------------------------+
```

## Binary Layouts

### 1. META Header (0x80 bytes)

| Offset | Field | Type | Specification / Invariant |
|---|---|---|---|
| `0x00` | `magic` | `[u8; 4]` | Must equal `"META"` (`0x4154454D`). |
| `0x04` | `signature_key_generation` | `u32` | Key index used for ACID verification. |
| `0x08` | `reserved_08` | `[u8; 4]` | Reserved; checked for consistency. |
| `0x0C` | `flags0` | `u8` | Bit 0: 64-bit instruction mode.<br>Bits 1..=3: Address space type (`0`: 32-bit, `1`: 64-bit 36-bit, `2`: 32-bit no reserve, `3`: 64-bit 39-bit, `4`: 64-bit 42-bit).<br>Bit 4: Optimize memory allocation.<br>Bit 5: Disable device address space merge.<br>Bit 6: Address sanitizer.<br>Bit 7: Prevent code reads. |
| `0x0D` | `flags1` | `u8` | Bit 0: Enable shadow stack. |
| `0x0E` | `main_thread_priority` | `u8` | Valid priority range `0..=63` (`0` is highest priority in Horizon). |
| `0x0F` | `default_cpu_id` | `u8` | Ideal core ID `0..=3`. |
| `0x10` | `reserved_10` | `[u8; 4]` | Reserved. |
| `0x14` | `system_resource_size` | `u32` | Reserved memory for kernel page table allocations (`0x1000` page-aligned, `<= 0x1FE00000`). |
| `0x18` | `version` | `u32` | Program package version integer. |
| `0x1C` | `main_thread_stack_size` | `u32` | Initial main thread stack size (`0x1000` page-aligned; `0` specifies initial SP = 0 per Switchbrew, preserved directly without synthetic defaults). |
| `0x20` | `program_name` | `[u8; 16]` | ASCII/UTF-8 null-padded application title name (borrowed as zero-allocation `&'a str`). |
| `0x30` | `product_code` | `[u8; 16]` | ASCII/UTF-8 null-padded product code (borrowed as zero-allocation `&'a str`). |
| `0x40` | `reserved_40` | `[u8; 48]` | Reserved. |
| `0x70` | `aci_offset` | `u32` | File offset to ACI0 section (`>= 0x80`). |
| `0x74` | `aci_size` | `u32` | Total byte size of ACI0 section (`>= 0x40`). |
| `0x78` | `acid_offset` | `u32` | File offset to ACID section (`>= 0x80`). |
| `0x7C` | `acid_size` | `u32` | Total byte size of ACID section (`>= 0x240`). |

### 2. ACID Section (0x240 bytes base)

| Offset | Field | Type | Description |
|---|---|---|---|
| `0x000` | `signature` | `[u8; 0x100]` | RSA-2048 PSS SHA-256 digital signature. |
| `0x100` | `modulus` | `[u8; 0x100]` | RSA-2048 public modulus. |
| `0x200` | `magic` | `[u8; 4]` | Must equal `"ACID"` (`0x44494341`). |
| `0x204` | `size` | `u32` | Size of signed body (must satisfy `size >= 0x140` and `0x100 + size <= acid_size`). |
| `0x208` | `version` | `u8` | ACID format version. |
| `0x209` | `unknown_209` | `u8` | Target SDK version flag. |
| `0x20A` | `reserved_20A` | `[u8; 2]` | Reserved. |
| `0x20C` | `flags` | `u32` | Bit 0: Production / Retail release.<br>Bit 1: Unqualified approval.<br>Bits 2..=5: Pool partition (`0`: App, `1`: Applet, `2`: Sys, `3`: SysNonSecure).<br>Bit 7: LoadBrowserCoreDll. |
| `0x210` | `program_id_min` | `u64` | Lower bound of authorized program ID range (enforces `min <= max`). |
| `0x218` | `program_id_max` | `u64` | Upper bound of authorized program ID range. Non-zero range requires `min <= aci0.program_id <= max`. |
| `0x220` | `fac_offset` | `u32` | Offset relative to ACID start for Filesystem Access Control. |
| `0x224` | `fac_size` | `u32` | Byte size of FAC. |
| `0x228` | `sac_offset` | `u32` | Offset relative to ACID start for Service Access Control. |
| `0x22C` | `sac_size` | `u32` | Byte size of SAC. |
| `0x230` | `kac_offset` | `u32` | Offset relative to ACID start for Kernel Access Control (`4`-byte aligned). |
| `0x234` | `kac_size` | `u32` | Byte size of KAC (`4`-byte aligned). |

### 3. ACI0 Section (0x40 bytes base)

| Offset | Field | Type | Description |
|---|---|---|---|
| `0x00` | `magic` | `[u8; 4]` | Must equal `"ACI0"` (`0x30494341`). |
| `0x04` | `reserved_04` | `[u8; 12]` | Reserved. |
| `0x10` | `program_id` | `u64` | Process Program ID (Title ID). Must fall within ACID `[min, max]`. |
| `0x18` | `reserved_18` | `[u8; 8]` | Reserved. |
| `0x20` | `fah_offset` | `u32` | Offset relative to ACI0 start for Filesystem Access Header. |
| `0x24` | `fah_size` | `u32` | Byte size of FAH. |
| `0x28` | `sac_offset` | `u32` | Offset relative to ACI0 start for Service Access Control. |
| `0x2C` | `sac_size` | `u32` | Byte size of SAC. |
| `0x30` | `kac_offset` | `u32` | Offset relative to ACI0 start for Kernel Access Control (`4`-byte aligned). |
| `0x34` | `kac_size` | `u32` | Byte size of KAC (`4`-byte aligned). |

## Kernel Access Control (KAC) Descriptors

Descriptors are encoded as 32-bit little-endian words. The descriptor kind is identified by the count of trailing one bits (`(!desc).trailing_zeros()`):

| Trailing 1s | Descriptor Kind | Decoded Fields |
|---|---|---|
| **3** | `KernelFlags` (CorePriority) | Bits 4..=9: Lowest thread priority / max prio number (0..63)<br>Bits 10..=15: Highest thread priority / min prio number (0..63)<br>Bits 16..=23: Lowest CPU ID (0..3)<br>Bits 24..=31: Highest CPU ID (0..3) |
| **4** | `SyscallMask` | Bits 5..=28: 24-bit mask for SVC slot.<br>Bits 29..=31: Slot index (0..7, covering SVCs `slot*24 .. slot*24+23`). |
| **6** | `MapRange` | **Paired descriptors**:<br>Word 1: bits 7..=30 address (`<< 12`), bit 31 read-only.<br>Word 2: bits 7..=30 size (`<< 12`), bit 31 IO flag (`0` = IO, `1` = Normal). |
| **7** | `IoMemoryMap` | Bits 8..=31: Mapped physical IO memory page address (`<< 12`). (Switchbrew #IoMemoryMap). |
| **10** | `MapRegion` | Bits 11..=16: Region 0 type, bit 17: RO.<br>Bits 18..=23: Region 1 type, bit 24: RO.<br>Bits 25..=30: Region 2 type, bit 31: RO. |
| **11** | `InterruptPair` | Bits 12..=21: IRQ 0 (`0x3FF` = none).<br>Bits 22..=31: IRQ 1 (`0x3FF` = none). |
| **13** | `ApplicationType` | Bits 14..=16: 0 = System, 1 = Application, 2 = Applet. |
| **14** | `KernelVersion` | Bits 15..=18: Minor version.<br>Bits 19..=31: Major version. |
| **15** | `HandleTable` | Bits 16..=25: Maximum handle table capacity (`<= 1024`). |
| **16** | `DebugFlags` | Bit 17: Allow debug.<br>Bit 18: Force debug prod.<br>Bit 19: Force debug. |
| **32** | `Empty` | `0xFFFFFFFF` padding; ignored during parsing. |
| *other* | `Unknown` | Preserved in diagnostic form (`Capability::Unknown { id, raw }`) for forward compatibility with newer firmwares without panics. |

## Service Access Control (SAC)

Service Access Control tables represent packed sequences of IPC port permissions:
- Byte 0 (`ctrl`):
  - Bit 7: `0` = client access permission, `1` = server port registration.
  - Bits 0..=2: name length minus one (`len = (ctrl & 0x07) + 1`, 1 to 8 ASCII bytes).
- Bytes 1..=len: Port name characters.
- Wildcards: If the name ends with `'*'`, prefix matching applies (e.g. `"sm:*"`). Empty SAC slices return empty iterators safely.

## Filesystem Access Control (FAC / FAH)

Both ACID (FAC) and ACI0 (FAH) specify structured permission envelopes:
- **ACID FAC (version 1, minimum header size 0x2C)**:
  - `0x00`: `version` (`u8`, must be 1).
  - `0x01`: `fac_type` / content owner count (`u8`).
  - `0x02`: `save_data_owner_count` (`u8`).
  - `0x04..0x0C`: `fs_permissions` (`u64` little-endian).
  - `0x0C..0x14`: Content owner ID table extent (bounded within FAC slice).
  - `0x1C..0x24`: Save data owner ID table extent (bounded within FAC slice).
- **ACI0 FAH (version 1, minimum header size 0x1C)**:
  - `0x00`: `version` (`u8`, must be 1).
  - `0x04..0x0C`: `fs_permissions` (`u64` little-endian).
  - `0x0C..0x10`: `content_owner_info_offset` (`u32`, must be `>= 0x1C` if `size > 0`).
  - `0x10..0x14`: `content_owner_info_size` (`u32`, multiple of 8 bytes).
  - `0x14..0x18`: `save_data_owner_info_offset` (`u32`, must be `>= 0x1C` if `size > 0`).
  - `0x18..0x1C`: `save_data_owner_info_size` (`u32`, multiple of 8 bytes).
  - Owner extents must be within `fah_bytes` bounds and non-overlapping.

```text
effective_permissions = acid.fac_permissions & aci0.fah_permissions
```
A process may only access a filesystem resource if authorized by both its package envelope and the factory certificate.

## Validation Invariants

1. **Bounds and Overflows**: All offsets and sizes are evaluated with checked 32-bit addition (`checked_add`). Any overflow or out-of-bounds pointer immediately fails parsing.
2. **ACID Signed Envelope Bounds**: All ACID subregions (FAC, SAC, KAC) are strictly bounded within the signed body `[0x00 .. 0x100 + acid_payload_size]` (`signed_end`). Subregions extending beyond the signed body into unsigned trailing padding are rejected, preventing unsigned capability injection.
3. **Section Isolation**: ACID and ACI0 outer sections may not overlap. Internal subregions (FAC/FAH, SAC, KAC) may not overlap each other within their parent envelope.
4. **Empty Subregions**: Declared subregions with `size == 0` must specify `offset == 0` or an offset within their parent container bounds. Out-of-parent empty offsets are strictly rejected. Empty accessors safely return empty slices (`&[]`).
5. **Alignment**:
   - `main_thread_stack_size`: Must be page-aligned (`0x1000` = 4096 bytes).
   - `system_resource_size`: Must be page-aligned (`0x1000`) and `<= 0x1FE00000` (510 MiB).
   - KAC descriptors: Both offsets and slice lengths must be 4-byte aligned (`offset % 4 == 0` and `size % 4 == 0`).
6. **Range Invariants**:
   - `main_thread_priority <= 63`.
   - `default_cpu_id <= 3`.
   - `highest_thread_priority <= lowest_thread_priority` (Horizon convention: priority 0 is higher than priority 63).
   - Program ID authorization: `program_id_min <= program_id_max`. If non-zero, `program_id_min <= aci0.program_id <= program_id_max`.
   - `MapRange` descriptors must occur in complete, valid pairs.
7. **Strict Capability Conversion**: `Npdm::parse` and `preflight()` preserve unrecognized descriptors (`Capability::Unknown { id, raw }`) for forward diagnostics, but `to_process_metadata()` strictly rejects them with `Err(NpdmError::UnsupportedCapability { id, raw })` to guarantee that no unverified capability enters the Horizon execution sandbox.
8. **Zero Allocation**: Parsers borrow the backing buffer `&'a [u8]` and return borrowed strings `&'a str` for `program_name` and `product_code`. Iterators operate lazily without heap allocation based on untrusted sizes. Empirically verified via `tests/allocations.rs` with tracking `#[global_allocator]`.

## Process Configuration Bridge

The parser extracts a high-level summary `ProcessMetadata`:
```rust
pub struct ProcessMetadata {
    pub program_id: u64,
    pub main_thread_priority: u8,
    pub ideal_core: u8,
    pub main_thread_stack_size: u64,
    pub system_resource_size: u64,
    pub handle_table_size: u16,
    pub core_mask: u64,
    pub priority_mask: u64,
    pub is_application: bool,
    pub address_space: ProcessAddressSpace,
    pub syscall_bitmap: SyscallBitmap,
    pub effective_fs_permissions: u64,
}
```
This summary maps directly to the Horizon execution environment (`ProcessConfig` in `wl-runtime`), maintaining zero runtime coupling while providing deterministic process instantiation.

## Executable Preflight Diagnostic Tooling

`wl-npdm` provides `NpdmPreflight` via `npdm.preflight()`:
```rust
pub struct NpdmPreflight<'a> {
    pub structural_validation_passed: bool,
    pub program_id: u64,
    pub program_name: &'a str,
    pub product_code: &'a str,
    pub main_thread_priority: u8,
    pub default_cpu_id: u8,
    pub main_thread_stack_size: u32,
    pub system_resource_size: u32,
    pub address_space: ProcessAddressSpace,
    pub is_64_bit_instruction: bool,
    pub known_capability_count: usize,
    pub unknown_capabilities: Vec<UnknownCapabilityInfo>,
    pub allowed_services: Vec<&'a str>,
    pub effective_fs_permissions: Option<u64>,
    pub signature_verification: &'static str,
    pub horizon_runtime_compatibility: &'static str,
}
```
A standalone CLI tool is provided at `crates/wl-npdm/examples/preflight.rs`:
```bash
cargo run --example preflight <path/to/main.npdm>
```
The preflight tool provides human-readable formatted output or raw structured diagnostics for inspectable ExeFS verification prior to Horizon runtime process instantiation.
