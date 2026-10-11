# Content packages and process metadata

`wanderlust intake PATH [--verify-content]` ([`wl-package`](../../crates/wl-package/src/lib.rs)) identifies the formats below with bounded reads and emits deterministic JSON. It is read-only, performs no decryption and emits no key material. Sources: Switchbrew [NCA rev. 14983](https://switchbrew.org/w/index.php?title=NCA&oldid=14983), [NPDM rev. 15075](https://switchbrew.org/w/index.php?title=NPDM&oldid=15075), [Ticket rev. 12553](https://switchbrew.org/w/index.php?title=Ticket&oldid=12553), [NSO rev. 14193](https://switchbrew.org/w/index.php?title=NSO&oldid=14193).

## PFS0 (NSP)

```text
0x00  "PFS0"
0x04  u32 entry_count            (≤ 4096 accepted)
0x08  u32 string_table_size      (≤ 1 MiB accepted)
0x0C  u32 reserved
0x10  entry_count × { u64 data_offset, u64 size, u32 name_offset, u32 reserved }
....  string table (NUL-terminated names)
....  data; absolute offset = 0x10 + 24·entry_count + string_table_size + data_offset
```

Every entry must lie inside the file and every name inside the string table. Entries are classified by name: `*.nca`, `*.cnmt.xml`, `*.tik`, `*.cert`. Bracketed tokens in the package file name are reported as `filename_claims` and are never treated as metadata.

## NCA

A content's **content ID** is the first 16 bytes of the SHA-256 of the whole NCA; package entry names carry it. `--verify-content` streams each NCA through SHA-256 in 8 MiB chunks and compares.

The first 0xC00 bytes (header and four section headers) are AES-XTS encrypted in distributed content. `intake` reads 0x400 bytes and checks offset 0x200 for a plaintext `NCA0`/`NCA2`/`NCA3` magic. Without it the header is reported as encrypted and nothing further is interpreted. With a plaintext (already decrypted) header it reports: distribution type (0x204), content type (0x205), key generation (max of 0x206 and 0x220), content size (0x208), program ID (0x210), content index (0x218), SDK add-on version (0x21C), whether the rights ID (0x230) is non-zero, and section ranges from the FS entry table (0x240, units of 0x200 bytes). The encrypted key area (0x300) is never read out.

## Content meta XML

Packaging tools write a plaintext CNMT XML beside the Meta NCA. `intake` reads `Type`, `Id`, `Version`, `RequiredSystemVersion`, `PatchId`, `KeyGenerationMin`, and each `Content` record (`Type`, `Id`, `Size`, `Hash`, `KeyGeneration`). Each record is matched to the package entry `<Id>.nca` or `<Id>.cnmt.nca`, and size and full SHA-256 are compared when hashes were computed.

An application's `PatchId` equal to the title ID in another package's ticket rights ID establishes that package as the application's patch (`application-patch` relationship).

## Ticket

```text
0x000  u32 signature_type        0x10000/0x10003 RSA-4096 (0x200 + 0x3C pad)
                                 0x10001/0x10004 RSA-2048 (0x100 + 0x3C pad)
                                 0x10002/0x10005 ECDSA    (0x3C + 0x40 pad)
h      = 4 + signature + padding (0x140 for RSA-2048)
h+0x000 issuer[0x40]
h+0x040 title-key block[0x100]   ← never read out or reported
h+0x141 u8  title_key_type       0 common, 1 personalized
h+0x145 u8  key_generation
h+0x158 u64 device_id            reported only as "bound" (non-zero)
h+0x160 u8  rights_id[0x10]      title ID (8 bytes, big-endian) ‖ zeros ‖ key generation
h+0x170 u32 account_id           reported only as "bound" (non-zero)
```

A test fills the title-key block with a canary pattern and asserts it is absent from the serialized report.

## NPDM

```text
0x00 "META"          0x0C u8 flags (bit 0: 64-bit; bits 1–3: address space)
0x0E u8 main thread priority     0x0F u8 main thread core
0x14 u32 system resource size    0x18 u32 version
0x1C u32 main thread stack size  0x20 name[0x10]
0x70 u32 ACI offset → "ACI0", program ID at ACI+0x10
```

Address-space values: 0 32-bit, 1 64-bit (36-bit), 2 32-bit without reserved region, 3 64-bit (39-bit), 4 64-bit (42-bit). The main thread's stack size and priority are process-creation parameters the runtime will need for a Horizon-conformant initial thread.

## ExeFS directories

A directory containing NSO files named `rtld`, `main`, `subsdk0`–`subsdk9` or `sdk` (optionally with `.nso`) is reported as an ExeFS with, per module, the SHA-256, build ID (module ID), segment and BSS sizes, MOD0 presence and relocation/symbol counts, plus `main.npdm` when present. The same directory is a valid `link`/`analyze`/`recompile` input. Directory scans are limited to depth 3 and 4096 entries and do not follow symbolic links.

## Game cards

An `HEAD` magic at 0x100 is reported as a game-card image (XCI); its HFS0 partitions are not parsed.
