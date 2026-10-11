# Build environment

## Native Windows

Install stable Rust for `x86_64-pc-windows-msvc` and Visual Studio C++ Build Tools with a Windows SDK. Cargo builds the virtual workspace; `tools/wl-cli` is its default member.

```powershell
cargo build --workspace --release
cargo run -- --help
python tools/python/fixtures.py
cargo run -- inspect generated/fixtures/mixed-compressed.nso --json
cargo run -- recompile generated/fixtures/mixed-compressed.nso --entry 0x10000 --out generated/mixed.exe
./generated/mixed.exe
cargo test --workspace
```

These are the intended native MSVC commands; MSVC execution was not validated in the initial Linux/WSL environment. Generated code uses safe Rust/std and does not require clang, a standalone LLVM installation, a guest compiler, or a graphics device. A consistent Rust GNU/MinGW environment is an alternative. Do not mix incompatible CRT import libraries.

## Linux and verified Windows cross-compilation

The verified host was Arch Linux in WSL, x86-64, rustc/cargo 1.98.1, Python 3.14.7 and GCC 16.2.1. Rust targets `x86_64-pc-windows-gnu` and `x86_64-pc-windows-msvc` were installed; external Windows linker libraries initially were absent. Local read/write build artifacts remain under ignored `target/` and `generated/`.

A local linker setup used rust-lld, Debian MinGW-w64 12.0.0 import libraries and GCC 16.2.0 exception/runtime libraries. Packages were extracted under `target/toolchains/` without administrator privileges or global installation. Exact source packages: [Debian mingw-w64-x86-64-dev 12.0.0-5](https://deb.debian.org/debian/pool/main/m/mingw-w64/mingw-w64-x86-64-dev_12.0.0-5_all.deb), [Arch mingw-w64-gcc 16.2.0-2](https://fastly.mirror.pkgbuild.com/extra/os/x86_64/mingw-w64-gcc-16.2.0-2-x86_64.pkg.tar.zst). These are local validation prerequisites, not vendored project dependencies or a portable installer.

The same environment variables apply to `recompile --target x86_64-pc-windows-gnu`, which builds the runtime and units for the target and links the executable with `WL_LINKER`. For that extracted layout, the executed build command was:

```sh
CARGO_TARGET_X86_64_PC_WINDOWS_GNU_LINKER=rust-lld \
RUSTFLAGS='-C link-self-contained=yes -L native=target/toolchains/debian/usr/x86_64-w64-mingw32/lib -L native=target/toolchains/usr/lib/gcc/x86_64-w64-mingw32/16.2.0' \
cargo build --workspace --release --target x86_64-pc-windows-gnu
```

The executed native-guest compilation and launch commands were:

```sh
WL_LINKER=rust-lld \
WL_NATIVE_LIB_DIR=target/toolchains/debian/usr/x86_64-w64-mingw32/lib \
WL_GCC_LIB_DIR=target/toolchains/usr/lib/gcc/x86_64-w64-mingw32/16.2.0 \
target/debug/wanderlust recompile generated/fixtures/mixed-compressed.nso \
  --entry 0x10000 --target x86_64-pc-windows-gnu --out generated/mixed.exe
./generated/mixed.exe
```

`WL_LINKER` specifies one executable path, never a shell command. For explicit Windows GNU cross-compilation it also enables rustc's bundled startup objects (`link-self-contained=yes`). The two optional library-directory variables each add one native search path. Native Windows users normally need none of these overrides. Relative paths are resolved from the working directory running the compiler.

The mixed executable returned X0=26 and 21 instructions on Windows. The Windows CLI `--help` also ran. Testing the initially available Arch MinGW-w64 14 UCRT library set produced a startup failure even for a trivial Rust console program; the Debian import-library set resolved it. This is a measured local toolchain incompatibility, not evidence about guest correctness. WSL process interoperability separately prevented the full Windows differential run from completing.

## Test-input toolchain

Relocatable module fixtures need Python 3 and an AArch64-capable LLD: `ld.lld` from PATH, else the `rust-lld` that every rustup toolchain ships under `$(rustc --print sysroot)/lib/rustlib/<host>/bin`, else `WL_LLD`. No AArch64 compiler or assembler is required; code is hand-encoded into ELF objects.

```sh
python3 tools/python/modules.py            # generated/fixtures/modules/{rela,rel}/{main,sdk}
target/debug/wanderlust link generated/fixtures/modules/rela
target/debug/wanderlust recompile generated/fixtures/modules/rela --out generated/modules
./generated/modules                        # X0 = 1681
```

The `aarch64-unknown-none` target also builds the LLVM-compiled test program (`tests/programs/guest.rs`):

```sh
python3 tools/python/modules.py --rust tests/programs/guest.rs --out-dir generated/kernel
target/debug/wanderlust recompile generated/kernel --out generated/kernel-bin/kernel
./generated/kernel-bin/kernel --reg 0=0x123456789abcdef --steps 100000000
rustc --edition 2024 -C opt-level=3 tests/programs/host.rs -o generated/kernel-bin/host
./generated/kernel-bin/host 0x123456789abcdef      # same hash as the guest's X0
```

### libnx applications

[`homebrew.py`](../../tools/python/homebrew.py) builds a libnx C program into an ExeFS directory (`main` NSO and `main.npdm`) with the flags of libnx's `switch_rules` (`-march=armv8-a+crc+crypto -mtune=cortex-a57 -mtp=soft -fPIE`, `switch.specs`, `elf2nso`, `npdmtool`). It locates devkitPro through `DEVKITPRO`, else `target/toolchains/devkitpro/root/opt/devkitpro`.

The verified local toolchain was extracted there without administrator rights from the devkitPro pacman repositories (`https://pkg.devkitpro.org/packages/linux/x86_64/` for host tools, `https://pkg.devkitpro.org/packages/` for target libraries). Each archive was checked against the SHA-256 in the repository databases `dkp-linux.db` and `dkp-libs.db`:

| Package | SHA-256 |
|---|---|
| devkita64-binutils-2.46.0-1-x86_64 | `0d61ad4946eb1080c6e645c41f0cfaac0a2eba5fec88426fef8f03363e933b5a` |
| devkita64-gcc-16.1.0-1-x86_64 | `8192b899f0a5fcfe10d11fa63a631b45310c0c8f0b98d88c7c283c16e9d6b0fa` |
| devkita64-newlib-4.6.0.20260123-4-any | `fc97ba1009a94b68f8714c3cc548ef697ca82687b72fe0c05a7ca3df81beb53e` |
| devkita64-rules-1.1.1-1-any | `2649cf7827243447070073acc2aa2fee152a0592fc30e827ceed803c565a1ede` |
| general-tools-1.4.4-1-x86_64 | `b6b59bd3d22d4f83a7f0668f749df7cf97b1f05083031e629d43ca4f4377c7b5` |
| libnx-4.12.0-1-any | `c6950bdf8e4b872ece492225368c03e633776329548a7494f91aa1e8f239cd8a` |
| switch-tools-1.13.1-1-x86_64 | `de0a9473764d3c5205a7cc9b640e9e913c371b5684d526133384eb7e545c7dab` |

```sh
python3 tools/python/homebrew.py tests/horizon/probe.c --out-dir generated/probe
target/debug/wanderlust recompile generated/probe --horizon --allow-unsupported --out generated/probe-bin/probe
./generated/probe-bin/probe
```

The built probe is committed as a test fixture ([`tests/horizon/probe`](../../tests/horizon/probe)), so tests need devkitPro only to rebuild it.

SIMD/FP differential programs are assembled once with Keystone (`keystone-engine` 0.9.2, installed only to regenerate `tools/python/simd_encoding.json` with `tools/python/encode_simd.py`); the harness itself needs only Unicorn and Capstone.

## Native build cache

`recompile` compiles the runtime and each translation unit as rlibs in a cache directory (`--cache-dir`, `WL_CACHE_DIR`, default `.wanderlust-cache` beside the executable) with `--jobs` parallel rustc processes (`WL_JOBS`, default `min(CPUs, 4)`). Peak memory per process is roughly proportional to the unit size (`--unit-shift`, default 12, i.e. 4 KiB of guest code per unit; measured ≈ 170 MiB per process). A `.deps` directory beside each executable holds links to the rlibs it was linked from. Cache entries are immutable and keyed by content, so a cache directory can be shared between builds and deleted at any time.

The AArch64 proxy corpus used for instruction-coverage and backend-scaling measurements comes from the precompiled standard library of the `aarch64-unknown-none` target (`rustup target add aarch64-unknown-none`, a user-scope component):

```sh
S=$(rustc --print sysroot)/lib/rustlib/aarch64-unknown-none/lib
python3 tools/python/elf_text.py $S/libcore-*.rlib $S/liballoc-*.rlib $S/libcompiler_builtins-*.rlib \
  --out generated/proxy/rust-std-aarch64.a64 --roots generated/proxy/roots.txt
target/release/wanderlust survey generated/proxy/rust-std-aarch64.a64 --raw
target/release/wanderlust recompile generated/proxy/rust-std-aarch64.a64 --raw --entry 0x10000 \
  --roots-file generated/proxy/roots.txt --allow-unsupported --out generated/proxy/rust-std
```

The measurement host for these numbers was WSL2 with 12 logical CPUs and 3.7 GiB of RAM. A single-function emission of the 42k-instruction proxy exceeded that memory and was killed by the kernel ([ADR-0003](../decisions/ADR-0003-partitioned-block-functions.md)); a single crate needed 2.2 GiB; separately compiled 4 KiB units need ≈ 170 MiB per process ([ADR-0004](../decisions/ADR-0004-separate-translation-units.md)). `python3 tools/python/benchmarks.py measure -- target/release/wanderlust recompile …` records wall time, total child CPU time and peak child RSS; `benchmarks.py loop FILE` writes the dispatch benchmark used in [ADR-0004](../decisions/ADR-0004-separate-translation-units.md).

## Input and artifact layout

`local-inputs/` and `.dumps/` are ignored and reserved for user-supplied inputs; tools treat them as read-only. `generated/` holds derived source, executables, fixture manifests, scratch snapshots and differential reports. `target/` holds Cargo/compiler outputs and local tools. No binary/asset/key extraction or encryption bypass tooling is included.

The fixture generator's `--out-dir` controls its destination. Runtime `--memory PATH` initializes at most 64 KiB of scratch; `--dump PATH` writes all scratch on either successful execution or guest failure. `--reg N=VALUE`, `--sp`, `--nzcv` and `--steps` specify reproducible initial state. Values use decimal or `0x` hexadecimal. The runtime's `--help` describes this protocol.

CI describes Linux and Windows checks, but remote execution is separate from local evidence. See [validation](../testing/validation-methodology.md).
