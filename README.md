# Wanderlust

Wanderlust is an experimental toolchain for recompiling Nintendo Switch AArch64 programs into native host executables. It combines ahead-of-time compilation, a Horizon-compatible runtime and a Maxwell graphics backend implemented in Rust.

The long-term goal is to run the original Nintendo Switch version of **The Legend of Zelda: Tears of the Kingdom** on Windows x86-64 using the user's own game data.

**Wanderlust does not currently run Tears of the Kingdom as a playable game.** Native execution of guest instructions and verified synthetic graphics workloads are implemented; complete game compatibility and presentation remain unfinished. There is no end-to-end original-game rendering claim.

## Purpose and research objectives

Wanderlust investigates whether Nintendo Switch AArch64 programs can be translated ahead of time and executed with a native host runtime while preserving the guest CPU, operating-system and graphics behavior they depend on.

Its academic and engineering objectives are to study binary translation, executable loading, instruction semantics, operating-system compatibility, GPU command processing and cross-platform graphics execution. Reproducible homebrew programs, independently assembled shaders and reference-based tests provide evidence for each implemented behavior. Research findings, hardware references and technical limits are documented alongside the code.

The eventual compatibility target is the original Switch release of Tears of the Kingdom running on Windows x86-64. This repository contains the toolchain and compatibility implementation; it does not contain a reconstruction of the game's source code or an official PC port.

## Current development stage

**Experimental implementation and subsystem integration.** The project can compile and execute small AArch64 fixtures and source-built homebrew applications, and it can render independently constructed Maxwell workloads on actual host GPUs.

The NVIDIA service adapter is connected to generated executables through `--native-gpu`. A source-built AArch64 homebrew program initializes NVIDIA services, allocates and maps guest buffers, submits Maxwell commands and verifies two indexed GPU draws. Synthetic host-message tests also cover partial faults and retry safety. Runtime service coverage and Maxwell instruction/state coverage are still limited; this does not establish original-game graphics or VI presentation.

**A playable TOTK build, an original-game GPU command stream reaching this backend, and a verified original TOTK frame have not been demonstrated.** Passing a synthetic rendering test does not establish those milestones. Current code and tests are intended for research, development and conformance testing.

## What is implemented

- **Executable loading:** NSO, NRO and NPDM parsing, ExeFS module loading and relocation, and read-only inspection of Switch content packages.
- **Native recompilation:** AArch64 instruction decoding, control-flow discovery, Rust code generation, separate translation units and a persistent native build cache.
- **Runtime:** checked guest memory, process bootstrap, guest threads, synchronization primitives, HIPC/CMIF messages and a bounded set of Horizon services.
- **Graphics:** guest-formatted GPFIFO acquisition, Maxwell B06F/B197 state, a bounded vertex/fragment shader translator, indexed drawing, textures, depth, blending and pitch-linear/block-linear render targets through wgpu.
- **Driver integration:** a bounded NVIDIA device adapter connecting runtime service requests to GPU mappings, channels and synchronous execution. The general nvdrv, Host1x synchronization and display interfaces remain incomplete.

The graphics backend has executed headless validation workloads on Windows D3D12, Windows Vulkan and WSL OpenGL. Those results establish the tested subsets, not general Switch or game compatibility. See [graphics execution](docs/graphics/execution.md), [validation](docs/graphics/validation.md) and the [Horizon runtime](docs/platform/horizon-process.md) for precise behavior and limits.

## Build

Requirements:

- Stable Rust with the 2024 edition, Cargo, rustfmt and Clippy.
- A native linker. On Windows, use Visual Studio C++ Build Tools with the MSVC Rust toolchain, or a matching GNU/MinGW setup.
- Python 3.10 or newer for fixture generation and differential tests.
- A supported graphics adapter and driver for GPU tests; ordinary CPU tests do not execute the ignored graphics scenarios.

```sh
cargo build --workspace --locked
cargo run --locked -- --help
```

Detailed platform and optional devkitPro setup is documented in [build environment](docs/development/build-environment.md).

## Run a self-contained example

The repository can generate its own small AArch64 test program. No commercial game files are required.

```sh
python3 tools/python/fixtures.py
cargo run --locked -- recompile generated/fixtures/mixed-compressed.nso \
  --entry 0x10000 --out generated/mixed
./generated/mixed
```

The resulting native executable reports its guest register state as JSON. The fixture's expected result is `x0=26`, `nzcv=6` and `steps=21`.

On Windows, use `python` if appropriate, pass `--out generated/mixed.exe`, and run `./generated/mixed.exe`.

A source-built homebrew NRO fixture exercises the Horizon startup path:

```sh
cargo run --locked -- recompile crates/wl-nro/tests/fixtures/probe.nro \
  --horizon --allow-unsupported --out generated/horizon-probe
./generated/horizon-probe
```

Use an `.exe` output name on Windows. `--allow-unsupported` emits precise runtime faults for unsupported reachable instructions; it does not add support for those instructions.

To exercise guest-issued NVIDIA submissions using the original homebrew fixture:

```sh
cargo run --locked -- recompile tests/horizon/nvidia \
  --horizon --allow-unsupported --native-gpu --out generated/nvidia-probe
./generated/nvidia-probe --steps 3000000
```

The guest verifies two rendered framebuffer results and reports them through debug output. This is a graphics conformance program, not TOTK or a VI presentation implementation. `--native-gpu` requires this source workspace, Cargo and a supported GPU. It compiles target-specific graphics dependencies and retains a checked artifact snapshot in the native build cache; the first build has additional time and storage costs. `WL_GPU_TARGET_DIR` can select a dedicated GPU dependency build directory. For an optional diagnostic window, set `WL_GPU_DEBUG_WINDOW=1`; `WL_GPU_DEBUG_HOLD=1` keeps the last published image visible until you close it. Windows requires the official x64 `SDL2.dll` beside the generated executable in a native Windows directory. This shows the guest test output, not TOTK; see [the NVIDIA adapter documentation](docs/graphics/nvdrv.md#diagnostic-window).

For local executable inputs, see [module loading](docs/platform/module-loading.md), [executable formats](docs/platform/executable-formats.md) and [NRO support](docs/platform/nro.md). Extracted, decrypted inputs must be supplied locally. Wanderlust does not distribute game code, game assets, console keys or firmware.

## Test

```sh
cargo fmt --all --check
cargo clippy --workspace --all-targets --locked -- -D warnings
cargo test --workspace --locked
```

GPU scenarios are deliberately ignored by the ordinary test run. Execute them explicitly on a suitable machine:

```sh
cargo test -p wl-gpu --locked -- --ignored --nocapture
cargo test -p wl-nvdrv --test runtime_gpu --locked -- --ignored --nocapture
```

Select a backend with `WGPU_BACKEND=gl`, `vulkan` or `dx12` when appropriate for the host. A successful build is not evidence that a GPU scenario ran. Tests built with devkitPro require the optional toolchain described in the build guide.

[Validation methodology](docs/testing/validation-methodology.md) covers differential CPU execution and independently specified expectations. Graphics fixtures include assembly sources and provenance; they are not shaders extracted from a commercial game.

## Architecture

| Component | Responsibility |
| --- | --- |
| `wl-nso`, `wl-nro`, `wl-npdm`, `wl-package` | Input formats and metadata |
| `wl-loader` | Module layout and relocation |
| `wl-aarch64`, `wl-analysis` | Instruction semantics and control-flow analysis |
| `wl-codegen-rust` | Native source generation and cached compilation |
| `wl-runtime` | Guest memory, Horizon execution and hosted services |
| `wl-nvdrv` | Bounded NVIDIA device and ioctl adaptation |
| `wl-gpu` | Maxwell command/shader translation and wgpu execution |
| `tools/wl-cli` | Inspection, analysis and recompilation commands |

See the [execution model](docs/architecture/execution-model.md) and [design decisions](docs/decisions/ADR-0001-aot-source-backend.md). GPU addresses, guest addresses and host resources are distinct domains; compatibility work must preserve their ownership and synchronization contracts.

## Contribute

Contributions are welcome, especially reproducible bug reports, hardware conformance tests, instruction semantics and runtime compatibility improvements.

Start with [CONTRIBUTING.md](CONTRIBUTING.md). A useful change includes a minimal reproduction, a source for hardware behavior, tests with independent expectations and a clear account of what was actually executed. Keep unrelated changes separate and sign off commits with `git commit -s`.

Do not attach commercial executables, game assets, keys, firmware, memory dumps or private extraction logs to issues or pull requests. Use synthetic fixtures or redistributable homebrew programs. Keep local inputs in ignored `local-inputs/` and generated output in ignored `generated/`.

For security-sensitive reports, read [SECURITY.md](SECURITY.md).

## License

Project-authored code is available under the [MIT License](LICENSE). Third-party dependencies and redistributed test fixtures retain their own licenses and notices. In particular, see the [graphics fixture notices](crates/wl-gpu/tests/fixtures/NOTICE), [homebrew fixture notices](crates/wl-nro/tests/fixtures/NOTICE) and [runtime fixture notices](tests/horizon/NOTICE).

## Project boundaries and proprietary content

- Wanderlust does not distribute Nintendo game executables, assets, firmware, console keys, proprietary shaders or Nintendo SDK components.
- It does not provide game downloads, decryption keys or tools to bypass content encryption or access controls. Package inspection identifies supported structures without decrypting them.
- Commercial inputs used in local experiments are supplied by the user and remain outside the repository. Contributions, issues and pull requests must use synthetic data or redistributable homebrew instead.
- The project does not claim general Switch compatibility, playable TOTK support or original-game rendering on the strength of synthetic tests.
- The MIT license covers project-authored code; it does not grant rights to Nintendo software or other third-party material.

Wanderlust is an independent project and is not affiliated with Nintendo or NVIDIA. Product names identify compatibility targets and hardware documentation.
