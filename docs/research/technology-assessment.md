# Technology assessment

Review date: 2026-10-11. Repository commit IDs below were resolved through GitHub and source/README/license files inspected. A source review is not a local build or a compatibility test. Exact package versions in `Cargo.lock` and `tests/differential/requirements.txt` govern executable dependencies. No external implementation source was copied.

## Recompilation, analysis and platform candidates

### N64Recomp/N64Recomp

Repository snapshot: [ffb39cdad1da5de07eaaa48bd1db4a89a7986771](https://github.com/N64Recomp/N64Recomp/tree/ffb39cdad1da5de07eaaa48bd1db4a89a7986771).

- License: MIT; retain notices.
- Verified/source findings: MIPS-to-C translation, ELF symbols, relocations, indirect-call lookup and jump-table analysis; reviewed src/recompilation.cpp and src/elf.cpp.
- Missing capabilities and integration: Not A64, not a Switch runtime. Frontend replacement is substantial.
- Relevance: Reference literal lowering, guest-address preservation and runtime split; no copied code.
- Decision: **Reference**.

### Zelda64Recomp/Zelda64Recomp

Repository snapshot: [b65c482ed672258eb684ab94a4e8c8aa662615ee](https://github.com/Zelda64Recomp/Zelda64Recomp/tree/b65c482ed672258eb684ab94a4e8c8aa662615ee).

- License: GPL-3.0; integrated code redistribution requires its terms; individual components differ.
- Verified/source findings: Repository/build integration demonstrates the N64 recompilation/runtime/graphics composition for an existing game port.
- Missing capabilities and integration: N64-specific platform behavior; no evidence of A64 or Horizon support. Integration is unsuitable for direct reuse.
- Relevance: Reference executable milestone discipline; not a Switch runtime dependency.
- Decision: **Reference**.

### bgyss/SwitchRecomp

Repository snapshot: [0f99e1c5c1e78fdc8e01e58a0f8dc8622e82976b](https://github.com/bgyss/SwitchRecomp/tree/0f99e1c5c1e78fdc8e01e58a0f8dc8622e82976b).

- License: No repository license detected in reviewed tree; no code reuse selected.
- Verified/source findings: Exploratory Rust ISA/pipeline scaffolding; inspected recomp-isa/src/lib.rs and pipeline module declarations. README explicitly labels draft specifications.
- Missing capabilities and integration: No verified commercial-game native execution from this review. No standalone codegen crate in the reviewed tree.
- Relevance: Pipeline concepts only; does not replace an executable translator.
- Decision: **Reference; reject as execution backend**.

### revng/revng

Repository snapshot: [8bb7bac912ae6f7e2a8f51973aa329f1b5323001](https://github.com/revng/revng/tree/8bb7bac912ae6f7e2a8f51973aa329f1b5323001).

- License: GPLv2 as a whole due to dependencies; individual project files MIT, per README.
- Verified/source findings: LLVM/QEMU-based binary analysis and decompiler framework, documented in current repository.
- Missing capabilities and integration: Not a ready Switch AOT runtime. Toolchain and binary-model integration are substantial; no local build performed.
- Relevance: Potential complex CFG/lifting experiment after bounded frontend stops being sufficient.
- Decision: **Experiment, deferred**.

### llvm/llvm-project

Repository snapshot: [a5de0ad578b9510392dae607521366a2f1247b8d](https://github.com/llvm/llvm-project/tree/a5de0ad578b9510392dae607521366a2f1247b8d).

- License: Apache-2.0 with LLVM exceptions; legacy/third-party notices also apply.
- Verified/source findings: AArch64 disassembler source and general native compiler backends; reviewed llvm/LICENSE.TXT and AArch64Disassembler.cpp.
- Missing capabilities and integration: Decoding/codegen alone supplies no guest semantics, relocator or Horizon behavior. Direct embedding requires version/toolchain integration.
- Relevance: Used indirectly by normal rustc codegen; direct LLVM backend remains unselected.
- Decision: **Adopt through rustc; reference direct APIs**.

### bytecodealliance/wasmtime

Repository snapshot: [e4657a0ceb5cf020fb68a553f6f839c5e6f99d69](https://github.com/bytecodealliance/wasmtime/tree/e4657a0ceb5cf020fb68a553f6f839c5e6f99d69).

- License: Apache-2.0 with LLVM exception, per LICENSE.
- Verified/source findings: Cranelift supports x86-64 and Windows x64 calling convention; object backend emits/link-manages object code.
- Missing capabilities and integration: Not an A64 binary lifter or Switch runtime; semantic frontend and object linking still required. Local benchmark not performed.
- Relevance: Credible alternative AOT backend; avoid confusing object emission with JIT-only operation.
- Decision: **Experiment, deferred**.

### capstone-engine/capstone

Repository snapshot: [81497779add4f56196b16f0e0e2afe23c582c06f](https://github.com/capstone-engine/capstone/tree/81497779add4f56196b16f0e0e2afe23c582c06f).

- License: BSD-3-Clause; retain notices; LLVM-derived portions have separate notices.
- Verified/source findings: A64 disassembly and structured operands; Python 5.0.7 installed and used by differential diagnostics.
- Missing capabilities and integration: No execution semantics or complete CFG recovery. C native library required; development branch differs from pinned test package.
- Relevance: Independent diagnostics now; possible broader analysis decoder later.
- Decision: **Adopt as test tool**.

### capstone-rust/capstone-rs

Repository snapshot: [04f196369ee64f4ad1821a9453670e9a9ce9b25a](https://github.com/capstone-rust/capstone-rs/tree/04f196369ee64f4ad1821a9453670e9a9ce9b25a).

- License: Rust wrapper MIT; bundled Capstone has its own BSD/LLVM notices.
- Verified/source findings: Rust bindings expose ARM64 instruction/operand metadata; reviewed capstone-rs/src/arch/arm64.rs and license.
- Missing capabilities and integration: Still disassembly only. Native build/binding overhead does not remove need for semantic decoding.
- Relevance: Candidate for richer instruction survey; not a runtime dependency today.
- Decision: **Reference**.

### NationalSecurityAgency/ghidra

Repository snapshot: [918d44ebc2962a53d23eeee52b8f44c7816e7815](https://github.com/NationalSecurityAgency/ghidra/tree/918d44ebc2962a53d23eeee52b8f44c7816e7815).

- License: Apache-2.0 with bundled third-party components requiring separate review.
- Verified/source findings: AARCH64 SLEIGH instruction definitions provide analysis/p-code semantics; reviewed AARCH64instructions.sinc.
- Missing capabilities and integration: Not a directly embeddable Rust/Windows executable game runtime. Headless extraction and semantic validation need integration.
- Relevance: Reference for wider instruction coverage, CFG and manual module analysis.
- Decision: **Reference**.

### qemu/qemu

Repository snapshot: [615ece3c406b262996e569f29ea6eed0d81c4d8d](https://github.com/qemu/qemu/tree/615ece3c406b262996e569f29ea6eed0d81c4d8d).

- License: GPLv2 project with per-file exceptions/licenses; inspect relevant source notices.
- Verified/source findings: A64 translation/execution semantics in target/arm/tcg/translate-a64.c.
- Missing capabilities and integration: Dynamic execution engine, not selected production AOT architecture. System emulation is unnecessary for the current experiment.
- Relevance: Semantic reference; Unicorn supplies the focused independent executor.
- Decision: **Reference**.

### unicorn-engine/unicorn

Repository snapshot: [7c5db94191defc1e04a4f66f4eb1220903cba837](https://github.com/unicorn-engine/unicorn/tree/7c5db94191defc1e04a4f66f4eb1220903cba837).

- License: GPLv2; Python package/runtime stays external to distributed native execution.
- Verified/source findings: AArch64 reference execution, register/memory access and hooks; pinned 2.1.4 executed 400 Linux differential comparisons.
- Missing capabilities and integration: No Horizon HLE; agreement is not a proof of ISA correctness. No production interpreter/JIT fallback selected.
- Relevance: Independent deterministic CPU/memory reference, optional development dependency.
- Decision: **Adopt for differential tests**.

### lioncash/dynarmic

Repository snapshot: [a41c380246d3d9f9874f0f792d234dc0cc17c180](https://github.com/lioncash/dynarmic/tree/a41c380246d3d9f9874f0f792d234dc0cc17c180).

- License: 0BSD main project; bundled dependencies require separate checks. Reviewed snapshot archived.
- Verified/source findings: Actual ARM/A64 dynamic recompiler, C++20 API, custom memory callbacks, x86-64/A64 hosts; inspected A64 translator.
- Missing capabilities and integration: JIT execution is incompatible with the principal production model. AOT adaptation is substantial, maintenance cannot be assumed.
- Relevance: Instruction/IR/memory reference; not an AOT dependency.
- Decision: **Reference; reject as principal backend**.

### lifting-bits/remill

Repository snapshot: [56918a8c2554088e93389e97d292f4035286506c](https://github.com/lifting-bits/remill/tree/56918a8c2554088e93389e97d292f4035286506c).

- License: Apache-2.0; LLVM and other transitive licenses remain relevant.
- Verified/source findings: Actual AArch64-to-LLVM-bitcode lifting; reviewed AArch64/Arch.cpp and Semantics/BINARY.cpp.
- Missing capabilities and integration: Does not reconstruct a complete Switch process. C++/Clang/LLVM 15+ integration, build/optimization and runtime ABI remain substantial.
- Relevance: Most directly relevant additional semantic-lifting candidate; native integration not prototyped locally.
- Decision: **Experiment, deferred**.

### switchbrew/libnx

Repository snapshot: [feebd026ca0f5dcc2119f46ad8e0d16ad3dd4973](https://github.com/switchbrew/libnx/tree/feebd026ca0f5dcc2119f46ad8e0d16ad3dd4973).

- License: ISC; preserve copyright/permission text when adapting source.
- Verified/source findings: A64 userland, SVC bindings and entry assembly; entry saves arguments, self-relocates and initializes runtime.
- Missing capabilities and integration: Guest-side library, not host HLE. Startup includes instructions currently unsupported here.
- Relevance: Authoritative source reference for homebrew conventions and targeted fixtures; no compiled libnx module tested.
- Decision: **Reference**.

### switchbrew/switch-examples

Repository snapshot: [669786898205b7beb25ff1731e72982e6d0397d3](https://github.com/switchbrew/switch-examples/tree/669786898205b7beb25ff1731e72982e6d0397d3).

- License: No top-level license detected; inspect each sample before adapting/distributing.
- Verified/source findings: Public homebrew sample sources and build examples.
- Missing capabilities and integration: Requires guest devkit toolchain; no sample binary downloaded or executed.
- Relevance: Potential noncommercial integration inputs; build/provenance and per-file license checks remain necessary.
- Decision: **Reference**.

### alula/Ryujinx

Repository snapshot: [4930dced25c0d9ec33096b0f2601d262daa477bf](https://github.com/alula/Ryujinx/tree/4930dced25c0d9ec33096b0f2601d262daa477bf).

- License: MIT project license; third-party libraries have independent obligations. This is a public mirror/fork, not an assertion of official upstream continuity.
- Verified/source findings: Inspected NSO loader, HLE SVC dispatch, Maxwell shader decoder/translation and block-linear texture layout source.
- Missing capabilities and integration: C# implementation is not a Rust drop-in library. Porting needs subsystem-level behavior tests and notice preservation; no local emulator build.
- Relevance: Strong source reference for HLE, command/resource and shader semantics. No code copied.
- Decision: **Reference; evaluate narrow adaptation later**.

### skyline-emu/skyline

Repository snapshot: [21d31f74b9e711b2b71ee57e016b6b2e8966e896](https://github.com/skyline-emu/skyline/tree/21d31f74b9e711b2b71ee57e016b6b2e8966e896).

- License: Historical MPL-2.0 claim in README; shader component had separate history/license exemption. No reusable source/license tree available at reviewed commit.
- Verified/source findings: Current owner-controlled tree contains README only. It records discontinued ARMv8 Android execution and historical shader-compiler provenance.
- Missing capabilities and integration: Cannot inspect historical implementation from this tree. No verified reusable shader translator available here.
- Relevance: Historical architecture context only; do not claim present source availability or maturity.
- Decision: **Reject as direct dependency**.

### gfx-rs/wgpu

Repository snapshot: [1f3a1fbb4747a13cd63eeeeb1784788e295c21b9](https://github.com/gfx-rs/wgpu/tree/1f3a1fbb4747a13cd63eeeeb1784788e295c21b9).

- License: MIT OR Apache-2.0; retain chosen-license notices when distributing.
- Verified/source findings: Native DX12/Vulkan backends; Naga frontends accept WGSL/SPIR-V/GLSL and emit host shader languages. Reviewed naga README/Cargo.toml and feature definitions.
- Missing capabilities and integration: No Maxwell ISA frontend, command processor, texture swizzler or guest coherence model. ASTC is capability-dependent; guest geometry/tessellation and precision requirements need concrete investigation.
- Relevance: Preferred future host rendering library; GPU dependency deliberately absent from CPU execution experiment.
- Decision: **Adopt when GPU workloads are available; reference Naga**.

### KhronosGroup/SPIRV-Tools

Repository snapshot: [e56740d931256cc464d4d1a76149f69469c33f70](https://github.com/KhronosGroup/SPIRV-Tools/tree/e56740d931256cc464d4d1a76149f69469c33f70).

- License: Apache-2.0; retain license/notices.
- Verified/source findings: SPIR-V validator, optimizer and assembly/disassembly tooling per project README.
- Missing capabilities and integration: No Maxwell decoder or resource/state translator. Valid SPIR-V alone does not establish faithful rendering.
- Relevance: Shader validation tool after a real guest shader frontend exists.
- Decision: **Reference; deferred adoption**.

### Thog/oss-rtld

Repository snapshot: [c0c483634f719b0502060894464a0d2901982a66](https://github.com/Thog/oss-rtld/tree/c0c483634f719b0502060894464a0d2901982a66) (archived; last commit 2020-06-24). Reviewed 2026-10-11.

- License: ISC.
- Verified/source findings: clean-room reimplementation of Horizon's runtime linker. `librtld/source/ld.cpp` (self-relocation, module discovery through `svcQueryMemory`, list construction, publication of `nn::ro::detail` globals), `ModuleObject.cpp` (dynamic-tag interpretation, relative prefix, `TryResolveSymbol` visibility rule, lazy and eager jump slots), `relocation.cpp` (load-order global lookup, lazy binder, reverse-order `DT_INIT`).
- Missing capabilities and integration: a guest-side reimplementation, not Nintendo's binary; its ABS32 handling writes 64 bits, contrary to AAELF64.
- Relevance: the only reviewed source that states rtld's lookup order, visibility rule and initialization order precisely. Wanderlust's host linker restates these semantics ([module loading](../platform/module-loading.md)); behavior derived only from it is marked as such.
- Decision: **Reference** (no code copied).

### Atmosphere-NX/Atmosphere (loader)

Repository snapshot: [0e9cd964c094946df14926b6c81a631b603a30f0](https://github.com/Atmosphere-NX/Atmosphere/tree/0e9cd964c094946df14926b6c81a631b603a30f0). Reviewed 2026-10-11.

- License: GPL-2.0.
- Verified/source findings: `stratosphere/loader/source/ldr_process_creation.cpp` `DecideAddressSpaceLayout` (module order, `max(segment ends) + bss` rounded to pages, contiguous placement, argument region after the first module, ASLR slide), segment permission application; `libvapours/include/vapours/svc/svc_memory_map.hpp` (`AddressMap39Start = AddressNullGuard64Size = 128 MiB`, 512 GiB region).
- Missing capabilities and integration: system-module source; GPL terms preclude copying into this repository without a license decision.
- Relevance: authoritative open implementation of the process layout `wl-loader` reproduces.
- Decision: **Reference** (algorithm restated, no code copied).

### LLVM LLD (as a test oracle)

Used as installed (`ld.lld` 23.1.1, or rustup's `rust-lld` 22.1.8); not a project dependency.

- License: Apache-2.0 WITH LLVM-exception.
- Verified findings: links AArch64 ELF objects into shared objects with PLT/GOT, `DT_HASH` (`--hash-style=sysv`), `DT_RELACOUNT`/`DT_RELCOUNT` with combined relocation sections, and RELA or REL dynamic relocations (`-z rela`/`-z rel`); places NSO-compatible segments under a linker script.
- Relevance: an independent producer of real dynamic tables for loader tests, so that expected relocation results do not come from Wanderlust's own algorithm.
- Decision: **Adopt for test-input generation**.

### keystone-engine/keystone (as a test tool)

Python package `keystone-engine` 0.9.2. Reviewed 2026-10-11.

- License: GPL-2.0 (the engine; bindings differ). Used only by a developer tool that regenerates a committed data file; not linked, imported by tests, or redistributed.
- Verified findings: assembles ARMv8.0 A64 including Advanced SIMD and FP syntax. Its encodings are accepted only when Capstone's disassembly re-assembles to the same word; all 400 test encodings satisfied that check.
- Relevance: writing SIMD/FP differential programs as assembly instead of hand-built bit fields.
- Decision: **Adopt for test-encoding generation only**.

## Current Rust dependencies

| Dependency | Locked version | License | Use and limit |
|---|---|---|---|
| lz4_flex | 0.13.1 | MIT | Safe raw-block decompression into an explicitly bounded output; no frame decoder needed |
| sha2 | 0.10.9 | MIT OR Apache-2.0 | Whole-file identity and required decoded-segment hashes |
| serde / serde_json | 1.0.229 / 1.0.152 | MIT OR Apache-2.0 | Structured inspection/analysis/CLI output; absent from generated runtime |

Version/license evidence: local `cargo info`, downloaded package manifests, and [lz4_flex 0.13.1](https://docs.rs/lz4_flex/0.13.1/lz4_flex/), [sha2 0.10.9](https://docs.rs/sha2/0.10.9/sha2/), [serde](https://serde.rs/), [serde_json](https://github.com/serde-rs/json). Lockfile transitive dependencies must also be included in any redistribution license inventory. No root project redistribution license has been selected.

## Switch executable and runtime references

[Switchbrew NSO0](https://switchbrew.org/w/index.php?title=NSO0&oldid=14440), [NRO0](https://switchbrew.org/w/index.php?title=NRO0&oldid=14196), [Rtld](https://switchbrew.org/w/index.php?title=Rtld&oldid=13760), [NCA](https://switchbrew.org/w/index.php?title=NCA&oldid=14983), [NPDM](https://switchbrew.org/w/index.php?title=NPDM&oldid=15075), [Ticket](https://switchbrew.org/w/index.php?title=Ticket&oldid=12553), [SVC](https://switchbrew.org/wiki/SVC), and [NV services](https://switchbrew.org/wiki/NV_services) describe public reverse-engineering findings. They are not official Nintendo specifications. NSO segment/hash/compression findings drive the implemented parser; newer ZBIC flags are rejected. Rtld, NCA, NPDM and Ticket findings drive the [loader](../platform/module-loading.md) and [package intake](../platform/content-packages.md). SVC ExitProcess drives the only implemented kernel boundary. NV service documentation is reference material only; no GPU experiment or commercial game observation was performed.

The reviewed candidates provide useful pieces rather than a verified general ARM64 Switch static-port solution. Failed GitHub probes for `yaxpeax/yaxpeax-arm` and `LiftProject/lift` returned 404; those names are not evidence that the broader projects/alternatives do not exist. Remill is the additional verified A64 semantic lifter. Broader decoding/analysis should be reevaluated when actual input coverage requires it.

## Graphics compatibility gate

The required bridge remains: guest nvdrv/command submission and GPU address space → Maxwell command/resource interpretation → Maxwell shader semantics → host shader/resource/pipeline operations. Naga starts after guest shader decoding/lowering. Concrete compatibility questions include block-linear texture addressing, ASTC fallback, render-target aliasing, CPU/GPU dirty tracking, shader precision, unsupported shader stages, barriers and descriptor limits. Neither a wgpu window nor a synthetic triangle would answer them. No Vulkan/DX12 escape hatch is selected without a reproducible guest workload showing a wgpu limitation.

Consequential decisions: [AOT source backend](../decisions/ADR-0001-aot-source-backend.md), [WASM boundary](../decisions/ADR-0002-wasm-boundary.md), [partitioned block functions](../decisions/ADR-0003-partitioned-block-functions.md), [separate translation units](../decisions/ADR-0004-separate-translation-units.md).
