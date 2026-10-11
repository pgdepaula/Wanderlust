# Tears of the Kingdom: supplied content layout

This document records what has been established about the locally supplied *The Legend of Zelda: Tears of the Kingdom* packages, how it was established, and where the evidence stops. It contains metadata and hashes only. No game content, key material or ticket title-key data is reproduced.

Reproduce with `wanderlust intake <directory> --verify-content` ([`wl-package`](../../crates/wl-package/src/lib.rs)); the report is deterministic JSON. Verification streams roughly 17.7 GB through SHA-256 (about 45 s on the reference machine).

## Containers

Both inputs are PFS0 archives (NSP). Entry offsets below are absolute file offsets.

**Base package** — 17,150,647,554 bytes, 5 entries:

| Entry | Kind | Size (bytes) |
|---|---|---|
| `dbf92394c5056c8e81cf1b69597613f0.cnmt.xml` | plaintext content-meta XML | 1,448 |
| `dd38de587cb690a36bf4f8c5afff81dc.nca` | Program NCA | 17,148,870,656 |
| `41064e01c82940536d6535842132bb2a.nca` | Control NCA | 1,540,096 |
| `1ba345f664f365b196b6dca92583c593.nca` | LegalInformation NCA | 231,424 |
| `dbf92394c5056c8e81cf1b69597613f0.cnmt.nca` | Meta NCA | 3,584 |

**Update package** — 526,968,672 bytes, 6 entries:

| Entry | Kind | Size (bytes) |
|---|---|---|
| `a83558333ead6da3e4af5c8d04013bb7.nca` | NCA (largest; role not verifiable, see below) | 523,858,944 |
| `cb4abe63a85ad7de97b7d826783ad4bd.nca` | NCA | 2,925,056 |
| `01a7179a72050fa7e0ccbcfec626322d.nca` | NCA | 175,104 |
| `83d0cc32cab0fc4f6ff5615c005479ab.cnmt.nca` | Meta NCA | 6,656 |
| `0100f2c0115b68000000000000000010.tik` | ticket | 704 |
| `0100f2c0115b68000000000000000010.cert` | certificate chain | 1,792 |

## Verified identity

**Base content meta (CNMT XML).** `Type = Application`, `Id = 0x0100f2c0115b6000`, `Version = 0`, `KeyGenerationMin = 15`, `RequiredSystemVersion = 1006632960` (`0x3C000000`; major field 15 under the Switchbrew system-version bit layout), `PatchId = 0x0100f2c0115b6800`. It lists four contents:

| Type | Content ID | Size | Full SHA-256 |
|---|---|---|---|
| Program | `dd38de58…81dc` | 17,148,870,656 | `dd38de587cb690a36bf4f8c5afff81dc5bcca69237451a1db201d48052e22890` |
| Control | `41064e01…bb2a` | 1,540,096 | `41064e01c82940536d6535842132bb2a3940adcaa5179c3f6eaa981b374cb616` |
| LegalInformation | `1ba345f6…c593` | 231,424 | `1ba345f664f365b196b6dca92583c59337692b4e2189454fc6fdc071a155f686` |
| Meta | `dbf92394…f0` | 3,584 | `dbf92394c5056c8e81cf1b69597613f02746f1038253a84a6437ab71844fc6d8` |

**Integrity.** For all eight NCAs the first 16 bytes of the streamed SHA-256 equal the content ID in the entry name. For the four base contents, size and full SHA-256 also equal the CNMT records. Two independent implementations (a standalone Python streaming hasher and `wl-package`) produced identical digests. The update NCAs' SHA-256 values:

| Content ID | SHA-256 |
|---|---|
| `a83558333ead6da3e4af5c8d04013bb7` | `a83558333ead6da3e4af5c8d04013bb725c737bb5ed12d7f0e03f460613578a6` |
| `cb4abe63a85ad7de97b7d826783ad4bd` | `cb4abe63a85ad7de97b7d826783ad4bd74153171424bfbdcc63ee8cfbb3082e0` |
| `01a7179a72050fa7e0ccbcfec626322d` | `01a7179a72050fa7e0ccbcfec626322d20da5f1c7b45ad3f397c7f474be2e7f5` |
| `83d0cc32cab0fc4f6ff5615c005479ab` | `83d0cc32cab0fc4f6ff5615c005479ab3d769ac5a99ee1b5f8da926fffbec553` |

**Base/update relationship.** The update's ticket has rights ID `0100f2c0115b68000000000000000010`, i.e. title `0100f2c0115b6800`, equal to the base CNMT's `PatchId`. The update package is therefore the patch title of this application. Ticket metadata: signature type `0x10004` (RSA-2048/SHA-256), issuer `Root-CA00000003-XS00000020`, title-key type *common*, key generation 16, not bound to a device or account. The base package carries no ticket.

## Not established

* **Update version.** The update's own CNMT is inside its encrypted Meta NCA. The file name's `[v720896]` is an unverified claim, as is any display version. Which update NCA is the Program content is likewise unknown.
* **Executable modules.** Every NCA header (all eight) is encrypted: offset 0x200 holds no plaintext `NCA0`/`NCA2`/`NCA3` magic. NCA headers are AES-XTS encrypted with the system NCA header key; section contents are encrypted with keys from the NCA key area or, for titles with a rights ID such as the update, with the ticket's title key — all of which require system key material that is not part of these inputs. ExeFS (`main.npdm`, `rtld`, `main`, `subsdk*`, `sdk`) and RomFS are therefore not readable from these inputs.

Wanderlust contains no decryption, does not read key files, and does not use the title-key block of the ticket. The executable-loading pipeline accepts an **already extracted, decrypted ExeFS directory** (`wanderlust intake DIR`, `link DIR`, `analyze DIR`, `recompile DIR`). For an updated game the effective program is the update's ExeFS: patch NCAs carry a complete ExeFS, while their RomFS is a delta over the base RomFS. Once such a directory is available, `intake` reports module build IDs, segment sizes, relocation and symbol counts and `main.npdm` process parameters, and `link`/`analyze` characterize the real modules.

## Consequence for execution

No Tears of the Kingdom instruction has been decoded, analyzed, translated or executed. The deepest authentic boundary reached for the game is package identification and integrity verification. Requirements that any executable for this platform has — MOD0 linking across `rtld`/`main`/`subsdk*`/`sdk`, the ARMv8.0 integer, floating-point and Advanced SIMD instruction sets — are implemented and validated against original and compiler-generated test programs ([module loading](../platform/module-loading.md), [AArch64](../platform/aarch64.md), [SIMD and FP](../platform/aarch64-simd-fp.md)). Execution starting at rtld additionally requires Horizon services (at least `svcQueryMemory`), a Horizon-conformant initial thread (NPDM stack, TLS through TPIDRRO_EL0) and the game's own instruction mix, none of which can be characterized without its modules.
