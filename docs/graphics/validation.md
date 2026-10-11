# Graphics validation and transfer measurements

Executed on 2026-10-11 with Rust 1.98.1, wgpu 26.0.1 and Naga 26.0.0.
Inputs are synthetic geometry and independently generated Tegra layout patterns.
No game assets, shaders or executable data are required.

## Reproducible checks

```sh
cargo fmt -p wl-gpu --check
cargo clippy -p wl-gpu --all-targets --locked -- -D warnings
cargo test -p wl-gpu --no-default-features --locked
WGPU_BACKEND=gl cargo test -p wl-gpu --locked --test headless -- --ignored --nocapture
WGPU_BACKEND=gl cargo run -p wl-gpu --locked --release --example headless
```

Formatting and Clippy passed. Twelve CPU tests passed, including Naga shader
validation, checked address aliases, command preflight, exhaustive small dirty
intervals, compressed-block geometry and an independent scalar GOB address oracle.
The oracle exercises partial edge GOBs and GOB heights 1 through 32.

Four FIFO tests additionally verify literal B06F packets, all word-aligned chunk
splits through borrowed guest memory, full-width payloads, maximum counts,
unsupported opcodes, method-span overflow and truncation/continuation. A separate
allocation-instrumented test decodes 5,000 method writes plus malformed headers and
truncation checks with zero measured Rust heap allocations or reallocations.
These five FIFO tests also passed as native Windows GNU-target executables.

The GPU test is explicitly ignored in ordinary `cargo test`: running the command
above requests actual execution and fails if no adapter is available. It passed
on all three adapters/backends below. Each run validated every RGBA8 pixel of
66×19 and 34×13 images, exercised repeated/multiple draws, a four-byte vertex
update and restoration, target resizing, removal after encoding, and rejection
of subsequent use of removed buffers. Six successful submissions transferred
224 upload bytes and 52,064 readback bytes, with four staging allocations, two
target allocations and six completion waits. Packed output copied 26,848 bytes.

## Executed environments

| Environment | Actual backend / adapter | Result |
|---|---|---|
| Linux under WSL | GL, llvmpipe LLVM 23.1.1 / Mesa 26.2.4 | CPU software rendering; GPU execution test passed |
| Native Windows, build 26300.9457 | D3D12, Intel UHD Graphics, driver 101.7076 | GPU execution test and benchmark passed |
| Native Windows, build 26300.9457 | Vulkan, Intel UHD Graphics, driver 101.7076 | GPU execution test and benchmark passed |

Windows binaries were built for `x86_64-pc-windows-gnu` with Rust's `rust-lld`
and existing local MinGW libraries, then **executed as Windows processes** through
WSL interoperability. Both the example and the headless integration-test binary
were executed. Cross-compilation alone was not counted as execution.
`WGPU_BACKEND` must be explicitly propagated through `WSLENV` when launching a
Windows executable from WSL; the logged adapter backend confirmed each selection.
Native Windows users can select `dx12` or `vulkan` directly with `WGPU_BACKEND`.

All three tested adapters expose timestamp queries and BC compression. The Windows
Vulkan and GL software adapters advertise ASTC; the D3D12 adapter does not.
Only timestamps are enabled. The program prints full supported features and limits.
Linux Vulkan execution was unavailable because `libvulkan.so.1` was absent; GL
fallback was tested instead. No Linux Vulkan result is claimed.

## Controlled resource experiment

The example renders two differently colored indexed rectangles into 66×19 RGBA8.
After one warmup, each mode executes 32 frames and checks every pixel. Recreate
allocates and removes two buffers each frame; reuse-full uploads both existing
buffers; reuse-dirty uploads one explicitly selected four-byte range; unchanged
submits the same resources without uploads. The dirty update writes the same
value to keep output identical; the integration test separately verifies that a
changed value changes pixels. Target/readback/pipeline allocations are reused.

| Mode | Buffer allocations | Upload bytes | Staging allocations | Rust heap allocations, WSL release | Rust heap bytes, WSL release |
|---|---:|---:|---:|---:|---:|
| Recreate, full upload | 64 | 6,912 | 64 | 2,850 | 647,056 |
| Reuse, full upload | 0 | 6,912 | 64 | 2,400 | 611,040 |
| Reuse, dirty upload | 0 | 128 | 32 | 2,304 | 603,872 |
| Reuse, unchanged | 0 | 0 | 0 | 2,016 | 552,384 |

Resource/transfer counts matched across backends. Dirty uploads reduced controlled
upload bytes by 98.15% against full uploads. Reuse eliminated 64 buffer creations.
This does not eliminate driver staging copies, whole-table preflight allocation,
or output allocation. `stats_alloc` measures Rust process allocations, including
wgpu, but excludes allocations made directly by native drivers and libraries.
The table reports allocation calls and bytes allocated; reallocations are separately
printed by the example and are not included in those columns.

| Mode | WSL GL release: wall / CPU encoding, µs | Windows D3D12 debug: wall / CPU encoding, µs | Windows Vulkan debug: wall / CPU encoding, µs |
|---|---:|---:|---:|
| Recreate, full upload | 36,922 / 35,703 | 42,592 / 19,035 | 48,861 / 26,019 |
| Reuse, full upload | 28,350 / 27,327 | 33,121 / 13,148 | 31,237 / 13,248 |
| Reuse, dirty upload | 39,830 / 38,544 | 30,467 / 11,435 | 30,106 / 13,083 |
| Reuse, unchanged | 29,268 / 28,366 | 27,852 / 9,600 | 53,125 / 31,101 |

These are one fixed-order run per environment, with instrumented allocation and
synchronous readback. Different build profiles and software/hardware adapters are
not directly comparable. The noisy Vulkan unchanged result and GL dirty result
preclude a general timing speedup claim. Mean timestamp intervals were approximately
0.54–0.79 ms on software GL, 33–37 µs on D3D12 and 21–26 µs on Vulkan. They cover
the render passes, not uploads, readback, CPU work or presentation.

Every mode reads back 311,808 bytes, copies 160,512 packed pixel bytes, allocates
32 output vectors and waits 32 times. Each frame has 19 rows with 512-byte pitch,
plus 16 timestamp bytes. GPU readback and synchronous waiting dominate this small
correctness workload; it is not a game rendering benchmark or a production frame
scheduler.

Workspace baseline checks also ran. Whole-workspace formatting was blocked by
existing edits outside the graphics crate; workspace Clippy and tests failed in
`wl-runtime/tests/semantics.rs` because calls still used an earlier `service`
signature. These failures were present before graphics changes and were not
reported as passing graphics integration checks.

The deliverable was separately validated in a temporary source snapshot containing
the committed repository baseline plus only `wl-gpu`, these graphics documents
and its dependency lockfile. No unrelated in-progress source was included.
`cargo fmt --all --check`, workspace Clippy with `-D warnings` and
`cargo test --workspace --locked` all passed in that snapshot, with 108 passing
tests and no failures. The headless test
remained explicitly ignored there; actual GPU execution results are recorded above.
