# Module loading and dynamic linking

This document specifies how Wanderlust turns a set of NSO modules into a guest process image, and which parts of Horizon's loader (`ldr`) and runtime linker (`rtld`) behavior it reproduces. Implementation: [`wl-loader`](../../crates/wl-loader/src/lib.rs) (layout and linking) and [`wl-nso::dynamic`](../../crates/wl-nso/src/dynamic.rs) (dynamic metadata).

## Sources

| Source | Revision | Used for | Reuse |
|---|---|---|---|
| Switchbrew [Rtld](https://switchbrew.org/w/index.php?title=Rtld&oldid=13760) | rev. 13760 | entry convention, rtld sequence, relocation types | facts only |
| Switchbrew [NSO](https://switchbrew.org/w/index.php?title=NSO&oldid=14193) | rev. 14193 | header, argument placement | facts only |
| Atmosphère `stratosphere/loader/source/ldr_process_creation.cpp`, `libvapours/.../svc_memory_map.hpp` | [0e9cd964](https://github.com/Atmosphere-NX/Atmosphere/tree/0e9cd964c094946df14926b6c81a631b603a30f0) | module order, size rounding, placement, address-space bases | GPL-2.0: algorithm restated, no code copied |
| [oss-rtld](https://github.com/Thog/oss-rtld/tree/c0c483634f719b0502060894464a0d2901982a66) (clean-room rtld reimplementation) | c0c48363 | relocation phases, lookup order, visibility rule, lazy binding, init order | ISC: semantics restated, no code copied |
| Arm *ELF for the Arm 64-bit Architecture* (AAELF64) | — | relocation formulas and ABS32 overflow range | specification |

oss-rtld is a reimplementation, not Nintendo's binary. Where this document states rtld behavior derived only from it, that behavior is marked *reimplementation-derived*.

## Process layout (`ldr`)

Modules are named by their ExeFS file names. `ldr` considers them in the fixed order

```text
rtld, main, [wkc0..wkc9], subsdk0, ..., subsdk9, sdk
```

and places every present module contiguously. For module *i* with NSO header fields `{text,ro,rw}_dst_offset`, `{text,ro,rw}_size` and `bss_size`:

```text
size_i  = AlignUp(max(text_end, ro_end, rw_end) + bss_size, 0x1000)
base_0  = code_region_start (+ ASLR slide)
base_i+1 = base_i + size_i            (+ argument region after the first module, if arguments exist)
```

`ldr` requires `text_dst_offset == 0` and page-aligned `ro`/`rw` offsets. Permissions are applied per segment at page granularity: text `R-X` (or execute-only when NSO flag bit 6 is set), rodata `R--`, data and BSS together `RW-` over `AlignUp(rw_size + bss_size, 0x1000)` bytes. Gaps are zero-filled.

For the 64-bit 39-bit address space, `code_region_start = AddressMap39Start = AddressNullGuard64Size = 0x8000000` and the region size is `512 GiB − 128 MiB = 0x7F_F800_0000`. ASLR adds a random multiple of `os::MemoryBlockUnitSize`. Wanderlust fixes the slide at zero (`DEFAULT_CODE_BASE = 0x8000000`) because ahead-of-time translation embeds absolute guest addresses (ADRP results, branch targets); correctness does not depend on the slide, and `--base` selects another page-aligned base.

`Process::build` implements this layout. It rejects unknown module names, duplicates, unaligned bases, segment overlap after page rounding, and images exceeding the region. Browser-core `wkc*` modules and the argument region are not implemented: an argument block exists only when the process is launched with arguments, which applications normally are not.

## Dynamic metadata (MOD0)

Text word 1 holds the module-relative offset of the MOD0 header. All MOD0 fields are signed 32-bit offsets *relative to the MOD0 header itself*:

| Offset | Field | Wanderlust field |
|---|---|---|
| 0x00 | `"MOD0"` | — |
| 0x04 | `.dynamic` | `dynamic_offset` |
| 0x08 / 0x0C | BSS start / end | `bss_start`, `bss_end` |
| 0x10 / 0x14 | `.eh_frame_hdr` start / end | `unwind_start`, `unwind_end` |
| 0x18 | rtld module-object storage (in BSS) | `module_object` |

NSO modules are linked at address 0, so every `d_ptr`, symbol value and relocation offset is a `ModuleOffset`. `Dynamic::parse` interprets these tags and validates every table against the mapped segments before use:

| Tags | Validation |
|---|---|
| `DT_STRTAB` + `DT_STRSZ` | both or neither; range mapped; names NUL-terminated within the table |
| `DT_SYMTAB`, `DT_SYMENT` | `SYMENT == 24`; count = `DT_HASH` `nchain`, else the NSO header dynsym range (must start at `DT_SYMTAB`) |
| `DT_HASH` | `nbucket > 0`; counts ≤ 2²⁴; every bucket/chain index `< nchain` |
| `DT_RELA`/`RELASZ`/`RELAENT`, `DT_REL`/`RELSZ`/`RELENT` | address and size paired; `RELAENT == 24`, `RELENT == 16`; size a multiple of the entry size |
| `DT_JMPREL`/`DT_PLTRELSZ`/`DT_PLTREL` | `PLTREL` ∈ {`DT_REL`, `DT_RELA`} and required with `JMPREL` |
| `DT_RELACOUNT`, `DT_RELCOUNT` | not larger than the table |
| `DT_RELR`/`RELRSZ`/`RELRENT` | parsed and bounded; applied by host linking (`*P += B` for each address and bitmap entry), recorded as `DT_RELR (relative)`, diagnostic note `relr-extension`. rtld does not process it; libnx's crt0 applies it itself in guest mode |
| `DT_INIT`, `DT_FINI`, `DT_INIT_ARRAY(SZ)`, `DT_FINI_ARRAY(SZ)` | arrays sized in multiples of 8 |
| `DT_NEEDED`, `DT_SONAME` | string offsets inside `DT_STRTAB` |

Every relocation's symbol index must be inside the symbol table. Unknown tags are retained in `uninterpreted_tags`. Malformed metadata rejects the module rather than truncating a table.

## Linking (`rtld`)

### Entry

The process starts at the beginning of the first module's text (rtld in a retail application) with `X0 = 0` and `X1 =` the main thread handle. rtld locates the other modules by scanning memory with `svcQueryMemory` for `R-X` static regions, requires MOD0 in each, clears BSS, and links.

### Phase 1 — relative relocations, per module in load order

Only the first `DT_RELACOUNT` RELA entries and the first `DT_RELCOUNT` REL entries are processed, and only those of type `R_AARCH64_RELATIVE` (1027):

```text
RELA:  *P = B + A
REL:   *P = B + *P          (the implicit addend is the stored word)
```

`B` is the module's own base. Relative relocations outside the count prefix are *not* applied by rtld; a module that has them but no count tag would fault on hardware.

### Phase 2 — symbolic relocations, per module in load order

Remaining `DT_REL`/`DT_RELA` entries of type `R_AARCH64_ABS64` (257), `R_AARCH64_ABS32` (258) and `R_AARCH64_GLOB_DAT` (1025), and `DT_JMPREL` entries of type `R_AARCH64_JUMP_SLOT` (1026), are resolved. Symbol resolution (`TryResolveSymbol`, *reimplementation-derived*):

1. A reference whose symbol has non-default visibility binds to a definition **in the same module**, found through that module's `DT_HASH` table. If none exists and the symbol is weak, the value is 0.
2. Otherwise the global scope is searched **in load order** (rtld, main, subsdk0…9, sdk); the first module whose hash table yields a *definition* (`st_shndx ∉ {SHN_UNDEF, SHN_COMMON}`) with non-local binding wins. `S = module_base + st_value`.
3. If no definition is found the target is left unchanged (unresolved).

Relocation formulas, with REL using the stored word as addend:

| Type | RELA | REL | Width |
|---|---|---|---|
| ABS64, GLOB_DAT | `S + A` | `*P + S` | 64 |
| ABS32 | `S + A`, requires `−2³¹ ≤ X < 2³²` | `S + sext32(*P)` | 32 |
| JUMP_SLOT | `S + A` | `S` | 64 |

A REL jump slot does not carry an addend: LLD (and the platform toolchains) store the lazy PLT stub address there. rtld's lazy binder returns `S` for REL, so treating the stored word as an addend would branch into the middle of nowhere; this exact defect was caught by the REL-format test below.

*Known discrepancy.* oss-rtld routes ABS32 through the same 64-bit store as ABS64. Wanderlust implements the AAELF64 definition (a 32-bit field with the overflow check). The behavior of Nintendo's rtld for ABS32 is unverified; LLD refuses to emit ABS32 dynamic relocations for shared objects, so it is not expected in practice.

### Lazy binding

rtld initializes `DT_JMPREL` slots lazily by default (`*P = B + *P`, the PLT stub) and stores its module object and resolver at `GOT[1]`, `GOT[2]` (`DT_PLTGOT`). The first call through a slot enters the resolver. Wanderlust's host linking binds jump slots **eagerly**. For code that only *calls* through the PLT the observable result is identical; a program that reads a GOT slot before its first call, or relies on `GOT[1..2]`, would observe a difference. That state is reported, not emulated.

### Initialization

After linking, rtld publishes its module lists into modules that define `nn::ro::detail::g_pAutoLoadList`, `g_pManualLoadList`, `g_pRoDebugFlag`, `g_pLookupGlobalAutoFunctionPointer` and `g_pLookupGlobalManualFunctionPointer`, then starts the SDK (`nn::init::Start`, or the older `__nnDetailInitLibc0` … `nnMain` sequence), which calls each module's `DT_INIT` in **reverse load order** (sdk first, rtld last) and finally `nnMain`. rtld does not interpret `DT_INIT_ARRAY` itself; the module's own `DT_INIT` code is expected to run its arrays. `LinkSummary::init_order` reports this order; the loader never executes guest code itself.

## Wanderlust link modes

| Mode | Image mapped for execution | Who relocates | Valid when |
|---|---|---|---|
| `host` (default without rtld) | relocated by `wl-loader` | host, eagerly, as specified above | no guest code will relocate again |
| `guest` (default with rtld) | exactly as `ldr` leaves it | guest code: rtld, or a self-relocating crt0 | rtld or a self-relocating entry is present |

Linking is idempotent for some relocation forms but not others, which is why a host-linked image must never be relocated again by guest rtld:

| Form | Re-application | Idempotent |
|---|---|---|
| RELA RELATIVE `B + A` | same value | yes |
| REL RELATIVE `B + *P` | adds `B` twice | **no** |
| RELA symbolic `S + A` | same value | yes |
| REL ABS64/GLOB_DAT `*P + S` | adds `S` twice | **no** |
| lazy JUMP_SLOT init `B + *P` over a bound slot | corrupts the slot | **no** |

`Process::build` therefore sets `execution_blocker` for a host-linked image whose first module is rtld, and for any host link that produced an error diagnostic. Code generation refuses blocked images. In guest mode the image is never modified; linking still runs on a private copy so that analysis can see relocated pointer values.

### Diagnostics

Each condition is a structured `Diagnostic { severity, module, kind, detail, address }`; nothing is silently replaced by zero.

| Kind | Severity | Effect |
|---|---|---|
| `relative-outside-count`, `non-relative-in-count` | warning | not applied (rtld would not apply it) |
| `unresolved-symbol` | error (strong) / warning (weak) | target unchanged; jump slot left at its PLT stub address |
| `unsupported-relocation` (TLS, IRELATIVE, unknown) | error | not applied |
| `relocation-target-unmapped`, `relocation-target-not-writable` | error | not applied |
| `relocation-overflow` (ABS32) | error | not applied |
| `relr-extension` | note | DT_RELR applied by host linking (not an rtld feature) |
| `relr-malformed` | error | a bitmap entry precedes any address entry; nothing further applied |
| `relr-unmapped` | error | the DT_RELR table is outside mapped memory |
| `rtld-state-not-published` | warning | host mode does not provide rtld's globals |
| `guest-linker-required` | note | guest mode without rtld |

### Code pointers for analysis

Every value written by a relocation that lands on a 4-byte-aligned address in an executable mapping, every defined `STT_FUNC` dynamic symbol, and every `DT_INIT`/`DT_FINI` becomes a `CodePointer`. These seed analysis as *speculative* roots: a pointer into text is not proof of an instruction boundary (it may address constant data), so analysis discards a speculative root, and everything reached only through it, if traversal leaves executable memory (see [execution model](../architecture/execution-model.md)). Relocated function pointers are the principal source of indirect-branch targets: vtables, function-pointer tables, GOT entries and PLT slots all appear here.

### Initialization-array candidates

`DT_INIT_ARRAY` and `DT_FINI_ARRAY` supply additional `CodePointer` candidates
with `initializer-array` and `finalizer-array` provenance. Their ELF64 elements
are read as little-endian pointers from the already relocated analysis copy;
the module base is never added a second time. Guest-mode storage remains
unrelocated. Candidate collection does not execute constructors, change their
order, remove duplicate guest calls or alter `init_order`'s DT_INIT-only contract.

The [ELF gABI dynamic-linking specification](https://gabi.xinuos.com/elf/08-dynamic.html)
defines the array addresses, byte sizes and execution order. Discovery retains
only aligned executable targets, as for other code-pointer sources. Empty,
null and non-executable targets add no candidate. An unmapped pointer entry,
address overflow or partial pointer is an explicit error. The combined arrays
have a conservative analysis budget of 1,048,576 entries per process, independent
of hardware limits and the instruction-discovery budget.

The `link` JSON exposes these provenance tags for filtering into an existing
`--roots-file`. This permits initializer-specific analysis without automatically
seeding every vtable or dynamic export. Synthetic tests cover explicit and
implicit relocation data, alternate bases, cross-module targets, duplicate
pointers, unmodified guest bytes and malformed metadata.

## Validation

* [Synthetic modules](../../crates/wl-loader/tests/support/mod.rs) with exact control of every dynamic table: RELA vs REL relative semantics independent of the stored word, base changes, the count prefix, load-order symbol precedence independent of input order, hidden-visibility binding, REL jump slots, ABS32 field width and overflow, strong/weak unresolved symbols, unsupported/unmapped/read-only targets, layout and permissions, rtld blocking, guest mode, and init order. Fourteen malformed-table cases are rejected by the parser ([tests](../../crates/wl-loader/tests/dynamic_tables.rs)).
* [LLD-linked modules](../../tools/python/modules.py): original A64 code in ELF *objects* is linked by LLD into two `-shared` objects (`main` imports a function through the PLT, a data object through the GOT and a function pointer through `.data`; `sdk` uses a RELATIVE-relocated pointer). LLD, an independent linker, produces the PLT, GOT, `.dynamic`, `DT_HASH`, `DT_RELACOUNT`, and RELA (`-z rela`) or REL (`-z rel`) tables; the tool repackages the result as NSO0 with MOD0. Expected values come from the program's semantics and from LLD's own symbol table, not from the loader. `main` must return `sdk_double(inc(triple(sdk_add_bias(6, 7)))) + ++sdk_counter = 1681`.

Observed: both formats link with zero diagnostics; every relocation-derived code pointer equals `module_base + st_value` from LLD's symbol table; the recompiled process returns `X0 = 1681`, `X9 = 1001` after 43 guest instructions on Linux x86-64 and as a Windows x86-64 PE (`x86_64-pc-windows-gnu`). Without host linking (`--link guest`, no rtld) the same program faults at its first PLT call, branching to the unrelocated stub offset.

Source-built libnx programs exercise self-relocation, threads and hosted service calls through the [Horizon integration tests](../../tools/wl-cli/tests/horizon.rs). The [NVIDIA fixture](../../tools/wl-cli/tests/native_gpu.rs) also executes guest-issued graphics requests. These are original homebrew fixtures, not original-game graphics evidence.
