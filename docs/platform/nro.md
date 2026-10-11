# Nintendo Relocatable Object (NRO0) Executable Ingestion

This document specifies how Wanderlust ingests, validates, and prepares Nintendo Relocatable Object (`*.nro`) executable files for guest memory mapping, AOT static analysis, and native execution.

Implementation: [`wl-nro`](../../crates/wl-nro/src/lib.rs), [`header`](../../crates/wl-nro/src/header.rs), [`asset`](../../crates/wl-nro/src/asset.rs), [`abi`](../../crates/wl-nro/src/abi.rs), [`image`](../../crates/wl-nro/src/image.rs).
Tests: [`validation`](../../crates/wl-nro/tests/validation.rs), [`real_fixtures`](../../crates/wl-nro/tests/real_fixtures.rs), [`conformance`](../../crates/wl-nro/tests/conformance.rs), [`adversarial`](../../crates/wl-nro/tests/adversarial.rs), [`allocations`](../../crates/wl-nro/tests/allocations.rs).

## 1. Sources & Authoritative Specifications

| Source | Reference / Revision | Used For | Reuse |
|---|---|---|---|
| Switchbrew [NRO0](https://switchbrew.org/wiki/NRO0) | rev. 14196 | Container layout, header offsets, segment definitions | Facts only |
| Switchbrew [Homebrew ABI](https://switchbrew.org/wiki/Homebrew_ABI) | rev. latest | Entrypoint conventions, registers X0/X1/LR, LoaderConfig key-value tuples | Facts only |
| libnx `switch/nro.h` | [master](https://github.com/switchbrew/libnx/blob/master/nx/include/switch/nro.h) | Authoritative `NroStart`, `NroHeader`, `NroSegment`, `NroAssetHeader` structures | Facts only |
| switch-tools `elf2nro` | [master](https://github.com/switchbrew/switch-tools) | Segment packaging, MOD0 pointer placement, ASET asset generation | Independent toolchain verification |

---

## 2. Container Structure & Layout

An NRO file is a flat, uncompressed binary executable used for non-ExeFS binaries and Nintendo Switch homebrew applications. Unlike retail NSO files, an NRO can be directly mapped into memory with page-aligned segment offsets.

The binary consists of:
1. **NroStart Header (`0x00..0x10`)**: An executable jump instruction and pointer to the dynamic `MOD0` header.
2. **NroHeader (`0x10..0x80`)**: Magic `"NRO0"`, version, total size, segment descriptors, BSS size, and GNU build ID.
3. **Executable Segments**:
   - `.text`: Executable code starting at `text_offset` (typically `0x00` embedding the header or `0x80`).
   - `.rodata`: Read-only data starting at `ro_offset` (page-aligned).
   - `.data`: Writable initialized data starting at `data_offset` (page-aligned).
4. **BSS Region**: Uninitialized memory placed immediately after `.data` in guest address space.
5. **Optional Asset Section (`ASET`)**: An asset trailer appended at file offset `header.size` containing icon, NACP metadata, and RomFS.

```text
+-------------------------------------------------------+ 0x00
| NroStart (0x10 bytes)                                 |
|   - 0x00: Branch instruction (e.g. b +0x80)           |
|   - 0x04: MOD0 header file offset                     |
|   - 0x08: Padding (8 bytes)                           |
+-------------------------------------------------------+ 0x10
| NroHeader (0x70 bytes, Magic: 'NRO0')                 |
|   - 0x10: Magic ('NRO0')                              |
|   - 0x14: Version (0)                                 |
|   - 0x18: Total executable size                       |
|   - 0x1C: Flags (bit 0: AlignedHeader)                |
|   - 0x20: Text Segment { offset, size }               |
|   - 0x28: RoData Segment { offset, size }             |
|   - 0x30: Data Segment { offset, size }               |
|   - 0x38: BSS size                                    |
|   - 0x40: Module ID / Build ID (32 bytes)             |
|   - 0x60: DSO handle, embedded info, DynStr, DynSym   |
+-------------------------------------------------------+ 0x80
| .text Segment (R-X)                                   |
+-------------------------------------------------------+ ro_offset
| .rodata Segment (R--)                                 |
+-------------------------------------------------------+ data_offset
| .data Segment (RW-)                                   |
+-------------------------------------------------------+ header.size
| Optional ASET Trailer (Magic: 'ASET')                 |
|   - 0x00: Magic ('ASET'), Version                     |
|   - 0x08: Icon Section { offset, size }               |
|   - 0x18: NACP Section { offset, size }               |
|   - 0x28: RomFS Section { offset, size }              |
|   - Section payloads (Icon, Control NACP, RomFS)      |
+-------------------------------------------------------+ EOF
```

---

## 3. Binary Specifications

### 3.1 NroStart Structure (0x10 bytes)

| Offset | Field | Type | Description |
|---|---|---|---|
| `0x00` | `branch_instruction` | `u32` | AArch64 instruction jumping past the 0x80-byte header (e.g. `0x14000020` = `b +0x80`). |
| `0x04` | `mod0_offset` | `u32` | File offset to the `MOD0` dynamic metadata structure. |
| `0x08` | `padding` | `[u8; 8]` | Padding bytes. |

### 3.2 NroHeader Structure (0x70 bytes, base header total 0x80 bytes)

| Offset | Field | Type | Description |
|---|---|---|---|
| `0x10` | `magic` | `[u8; 4]` | Must equal `"NRO0"` (`0x304F524E` little-endian). |
| `0x14` | `version` | `u32` | Format version (must be `0` or `1`). |
| `0x18` | `size` | `u32` | Declared executable byte size (must be `<= file_length` and `>= 0x80`). |
| `0x1C` | `flags` | `u32` | Bit 0: `AlignedHeader`. |
| `0x20` | `text_offset` | `u32` | File offset of `.text` segment. |
| `0x24` | `text_size` | `u32` | Byte size of `.text` segment (4-byte aligned). |
| `0x28` | `ro_offset` | `u32` | File offset of `.rodata` segment (page-aligned). |
| `0x2C` | `ro_size` | `u32` | Byte size of `.rodata` segment (4-byte aligned). |
| `0x30` | `data_offset` | `u32` | File offset of `.data` segment (page-aligned). |
| `0x34` | `data_size` | `u32` | Byte size of `.data` segment (4-byte aligned). |
| `0x38` | `bss_size` | `u32` | BSS memory allocation size in bytes (4-byte aligned). |
| `0x3C` | `reserved_3c` | `u32` | Reserved. |
| `0x40` | `module_id` | `[u8; 32]` | GNU build ID from ELF note. |
| `0x60` | `dso_handle` | `u32` | Offset of DSO handle. |
| `0x64` | `reserved_64` | `u32` | Reserved. |
| `0x68` | `embedded_offset` | `u32` | RoData embedded section offset. |
| `0x6C` | `embedded_size` | `u32` | RoData embedded section size. |
| `0x70` | `dyn_str_offset` | `u32` | Dynamic string table offset. |
| `0x74` | `dyn_str_size` | `u32` | Dynamic string table size. |
| `0x78` | `dyn_sym_offset` | `u32` | Dynamic symbol table offset. |
| `0x7C` | `dyn_sym_size` | `u32` | Dynamic symbol table size. |

### 3.3 Optional ASET Asset Trailer (0x38 bytes header)

When `file_length > header.size`, an ASET trailer may follow immediately at file offset `header.size`:

| Offset | Field | Type | Description |
|---|---|---|---|
| `0x00` | `magic` | `[u8; 4]` | Must equal `"ASET"` (`0x54455341` little-endian). |
| `0x04` | `version` | `u32` | Trailer format version (normally 0). |
| `0x08` | `icon_offset` | `u64` | Byte offset of application icon relative to ASET header. |
| `0x10` | `icon_size` | `u64` | Byte size of application icon. |
| `0x18` | `nacp_offset` | `u64` | Byte offset of Control NACP structure relative to ASET header. |
| `0x20` | `nacp_size` | `u64` | Byte size of NACP structure (standard: 16,384 bytes). |
| `0x28` | `romfs_offset` | `u64` | Byte offset of RomFS image relative to ASET header. |
| `0x30` | `romfs_size` | `u64` | Byte size of RomFS image. |

---

## 4. Architectural Distinctions: NSO0 vs NRO0

Wanderlust strictly maintains separation between retail ExeFS NSO0 modules and homebrew NRO0 applications:

| Characteristic | Retail NSO0 | Homebrew NRO0 |
|---|---|---|
| **Container** | ExeFS partition inside NCA | Standalone file (`*.nro`) |
| **Compression** | LZ4 segment compression supported | Always uncompressed |
| **Process Authorization** | Factory ACID + package ACI0 in `main.npdm` | Self-contained, optional ASET trailer |
| **Initial Register X0** | NULL or process context pointer | Pointer to Homebrew Loader Config table |
| **Initial Register X1** | Main thread handle | Sentinel `0xFFFFFFFFFFFFFFFF` (`u64::MAX`) |
| **Return Convention (LR)** | Exit strictly via `svcExitProcess` | Can return via `RET` using loader-provided `LR` |
| **Dynamic Linking** | Linked by system `rtld` via `svcQueryMemory` | Self-relocating crt0 or pre-relocated |

---

## 5. Homebrew ABI Entry Convention

The `EntryConvention` enum models the exact register setup required at execution start:

```rust
pub enum EntryConvention {
    RetailNso {
        x0_context: u64,
        x1_main_thread_handle: u64,
    },
    HomebrewNro {
        x0_loader_config: GuestAddr,
        x1_sentinel: u64, // 0xFFFFFFFFFFFFFFFF
        lr_returnable: bool,
    },
}
```

### Loader Configuration Table

The Homebrew Loader Config table passed at `X0` consists of 24-byte key-value tuples terminated by `EndOfList` (Key 0):

```text
+-------------------+-------------------+---------------------------------------+
| Key (u32 LE)      | Flags (u32 LE)    | Value (2 x u64 LE = 16 bytes)         |
+-------------------+-------------------+---------------------------------------+
```

Common standard keys decoded by `wl_nro`:
- `0: EndOfList`
- `1: MainThreadHandle` (Value[0] = main thread handle)
- `2: NextLoadPath`
- `3: OverrideHeap`
- `4: OverrideService`
- `5: Argv`
- `6: SyscallAvailableHint` (Value[0] = bitmask of SVCs 0..63)
- `7: AppletType`
- `10: ProcessHandle`
- `11: LastLoadResult`
- `16: HosVersion`
- `17: SyscallAvailableHint2` (Value[0] = bitmask of SVCs 64..127)

---

## 6. Validation Invariants & Safety Guarantees

1. **Bounds and Checked Arithmetic**: All segment offsets and lengths are computed using `checked_add`. Any overflow or boundary violation immediately returns `NroError::SectionOutOfBounds` or `NroError::ArithmeticOverflow`.
2. **Mutual Section Disjointness**: Text, RoData, and Data segments must not overlap each other within the declared container or in guest memory.
3. **Alignment Verification**:
   - `text.size`, `ro.file_offset`, `ro.size`, `data.file_offset`, `data.size`, `bss_size` must be 4-byte aligned.
   - Segments in guest memory are mapped to `PAGE_SIZE` (4096-byte) boundaries.
4. **MOD0 Dynamic Verification**: If `start.mod0_offset` is non-zero, it must lie within container bounds and point to the literal magic bytes `b"MOD0"`.
5. **ASET Extent Invariants**:
   - Section offsets must be $\ge 0x38$ (header size).
   - Sections must not overflow `u64` or exceed the trailer slice.
   - Non-empty asset sections (`icon`, `nacp`, `romfs`) must not overlap each other.
6. **Zero Allocation**: `Nro::parse` and all segment slice accessors borrow the backing buffer `&'a [u8]` with **0 heap allocations**, empirically proven via `tests/allocations.rs` with tracking `#[global_allocator]`.

---

## 7. Guest Process Image Mapping

The parser extracts a high-level `NroImage`:

```rust
pub struct NroImage<'a> {
    pub text: NroSegment<'a>,
    pub rodata: NroSegment<'a>,
    pub data: NroSegment<'a>,
    pub bss_offset: ModuleOffset,
    pub bss_size: u64,
    pub total_memory_size: u64,
    pub module_id: [u8; 32],
    pub module_id_hex: String,
    pub entrypoint_offset: ModuleOffset,
    pub entry_convention: EntryConvention,
}
```

Calling `image.create_guest_mappings(base: GuestAddr)` generates mapped guest regions:
- Text: mapped at `base + 0` as `R-X`.
- RoData: mapped at `base + ro_offset` as `R--`.
- Data + BSS: mapped at `base + data_offset` as `RW-`, with BSS appended and zero-initialized.

These mappings project directly into `wl-loader::Mapping` and `wl-analysis::Image` without modifying existing loader or runtime crates.

---

## 8. Standalone Preflight Tooling

A CLI inspection utility is provided in [`crates/wl-nro/examples/inspect.rs`](../../crates/wl-nro/examples/inspect.rs):

```bash
cargo run --example inspect <path/to/application.nro> [--json]
```

Output includes:
- NRO container format and version.
- Segment boundaries and sizes.
- BSS allocation requirements and total mapped memory.
- GNU module build ID.
- MOD0 dynamic header presence and offset.
- ASET asset inventory (Icon, NACP, RomFS byte sizes).
- Applicable entrypoint ABI convention.
