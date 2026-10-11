# ADR-0002: Keep WebAssembly as a possible host adaptation, without an empty crate

Status: accepted repository boundary; WebAssembly execution is not implemented or verified.

## Scope distinction

WASM could mean (1) a browser-hosted AOT guest program, (2) a WASI-hosted command-line executable, or (3) a compiler backend emitting WASM. These are different integration requirements. The current objective is Windows x86-64 native AOT. An AOT-generated WASM program subsequently compiled by a browser is not evidence of a Windows-native port, and converting guest code to a continuously interpreted bytecode would not satisfy the production execution model.

## Technical assessment

| Concern | Existing useful property | Unresolved host requirement |
|---|---|---|
| Integer semantics | Explicit guest-width wrapping arithmetic; u64 registers | WASM lowering and reference comparisons have not been run |
| Guest pointers | u64 guest addresses translated to owned regions | wasm32 host indices cannot represent arbitrary guest addresses directly; region sizes/allocation budgets must fit the host |
| Address-space capacity | Sparse mappings avoid reserving guest virtual-address span | Memory64 availability and browser resource limits are separate from guest 64-bit arithmetic |
| Executable generation | Generated Rust is target-selectable | Current generated `main`, filesystem protocol and process exit require a native/WASI environment |
| OS behavior | Small, explicit service boundary | Browser I/O, timers, scheduling and synchronization require host imports and an explicit async/cancellation model |
| Graphics | No GPU implementation exists yet | WebGPU API/device limits and host presentation differ from native wgpu/DX12/Vulkan |
| Threads/atomics | No unsupported concurrency assumptions are embedded in tests | Shared memory, browser isolation requirements and guest ordering need independent design/validation |
| Reproducibility | Independent CPU and memory snapshots | Browser/WASI harness, lifecycle and fault reporting are absent |

The [Rust target documentation](https://doc.rust-lang.org/rustc/platform-support/wasm32-unknown-unknown.html) states that important OS-facing standard-library facilities are unavailable on the bare WASM target. Simply passing `--target wasm32-unknown-unknown` is therefore insufficient for this runtime. The [WebAssembly type specification](https://webassembly.github.io/spec/core/syntax/types.html) defines i64 arithmetic independently of host address indexing. Memory64 must not be assumed to make a browser capable of allocating a Switch process image.

## Decision

Do not create `wl-wasm` now. Reuse the CPU/memory semantics in `wl-runtime` if a concrete WASM experiment is authorized. Host-specific adaptation can initially live in a module such as `wl-runtime::host::wasm`, alongside a native adapter, when those implementations exist. That path is a boundary designation, not a placeholder directory or an implemented interface.

Create a separate crate only when browser/WASI bindings introduce independent dependencies, artifact packaging, public ABI or release/test requirements. A distinct WASM code-generation backend would be a separate concern and require its own evidence. There is currently no reason to duplicate decoder/CFG/semantic crates for WASM. No browser support, memory64 support or WASM GPU compatibility is claimed.
