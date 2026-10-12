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

## Guest texture sampling

The texture continuation passed 15 CPU tests, including a new independent GOB
fixture, explicit six-mip offsets and shader validation. `texture_gpu` and the
previous `headless` test both passed actual WSL GL, Windows D3D12 and Windows
Vulkan execution. Commands:

```sh
cargo test -p wl-gpu --no-default-features --locked
WGPU_BACKEND=gl cargo test -p wl-gpu --locked --test texture_gpu --test headless -- --ignored --nocapture
```

The fixture is 37×35 RGBA8, two-GOB base block height, six mips ending at 1×1.
It crosses GOB columns and block-height rows and carries asymmetric RGB and alpha.
Every mip was sampled and compared byte-for-byte. Additional checks exercise
bilinear filtering, sRGB conversion (one-byte rounding tolerance), aliased dirty
writes, overlapping writes, a write spanning two mips, replacement GPU mappings,
truncated final mip, missing resources, destruction and recreation. No expected
image is copied into the render attachment by the CPU.

Four unchanged synchronizations performed zero payload reads/conversions/uploads.
A four-byte tiled update conservatively uploads the affected 5,180-byte mip per
aliased resident texture, not four bytes. A write spanning a mip boundary refreshes
both mips. The full test completed 11 draws, 30 texture uploads and 44,976 readback
bytes. Counters also include attempted conversions before a deliberately truncated
input fails, so they are not a minimal-transfer benchmark.

The exact delivery snapshot passed workspace formatting, Clippy and 111 tests;
both adapter-dependent tests were explicitly ignored in that CPU-only snapshot.
One Windows Vulkan launch failed before entering the test with WSL
`UtilAcceptVsock: accept4 failed 110`; repeating the same executable passed.

## Compressed guest texture sampling

Executed `cargo test -p wl-gpu --no-default-features --locked`: 17 CPU tests passed,
including independent ASTC/BC1 pixels, channel order, odd edge blocks, invalid
lengths/modes/HDR and unsupported formats. `cargo clippy -p wl-gpu --all-targets
--locked -- -D warnings` and crate formatting passed. The committed-baseline source
snapshot with the exact candidate lockfile passed workspace formatting, Clippy and
113 Rust tests; three GPU tests are explicitly ignored in that CPU run.

Explicit GPU runs of `compressed_gpu`, `texture_gpu` and `headless` passed on WSL
OpenGL llvmpipe, native Windows D3D12 and native Windows Vulkan. The Windows adapter
was Intel UHD Graphics. Compressed tests exercise 36×36 six-mip surfaces down to
1×1, 37×35 edge cropping, multiple independent blocks, BC1 native and software
paths, ASTC software fallback, sRGB, exact alpha, overlapping dirty writes, cache
reuse, invalid compressed data preserving resident contents and destruction/recreation.
Native ASTC was not exercised; native BC1 was exercised on all three adapters.
The corrected comparison policy permits a one-step BC interpolation difference
before applying sRGB, plus one output quantization step. Alpha remains exact.
The first GL run's fixed one-byte sRGB tolerance failed on a legitimate two-byte
linearized difference; the interval policy now expresses that specific bound.

Commands for reproducibility:

```sh
WGPU_BACKEND=gl cargo test -p wl-gpu --locked --test compressed_gpu --test texture_gpu --test headless -- --ignored --nocapture
WGPU_BACKEND=gl cargo run -p wl-gpu --locked --example textures
```

The same GNU Windows test binaries and `textures.exe` executed with `WGPU_BACKEND`
propagated to the Windows process for `dx12` and `vulkan`; cross-compilation alone
is not counted as execution. The texture benchmark validates baseline, dirty pixel
and decoded ASTC output through actual GPU readback.

The 37×35 six-mip synthetic benchmark transferred:

| Synchronization workload | Guest bytes read | Deswizzle bytes | RGBA decode bytes | Upload bytes | Image temporaries |
|---|---:|---:|---:|---:|---:|
| Initial RGBA8 | 15,360 | 6,776 | 0 | 6,776 | 6 |
| Unchanged, 32 frames | 0 | 0 | 0 | 0 | 0 |
| Four-byte dirty write | 9,216 | 5,180 | 0 | 5,180 | 1 |
| Two overlapping writes | 9,216 | 5,180 | 0 | 5,180 | 1 |
| Mip-one write | 4,096 | 1,224 | 0 | 1,224 | 1 |
| Destroy/recreate | 15,360 | 6,776 | 0 | 6,776 | 6 |
| ASTC fallback | 6,144 | 1,984 | 6,776 | 6,776 | 12 |

Unchanged synchronization observed zero Rust allocations on GL, D3D12 and Vulkan;
initial creation includes metadata/driver-facing allocations and is not allocation
free. The combined benchmark submitted and waited 41 times for deterministic
readback, transferring 368,016 padded GPU readback bytes and packing 212,380 bytes
on the CPU. Its dirty uploads are deliberately whole-mip conservative, not four-byte
GPU transfers. The benchmark prints source, deswizzle, decode, upload preparation,
Rust allocation traffic, draw/readback times and optional GPU timestamp samples.
Cold debug runs are diagnostic measurements, not frame-time or game speed claims.

One cold debug run on Intel UHD recorded 32 unchanged synchronization calls at
147 µs total on D3D12 and 230 µs on Vulkan; the corresponding 32 draw/readbacks took
36.24 ms and 52.10 ms. Mean render-pass timestamp samples were 38.86 µs and 25.69 µs,
respectively, excluding upload/readback. ASTC's six-mip CPU decode stage took
2.96 ms on D3D12 and 4.48 ms on Vulkan in that run, while its upload volume remained
6,776 bytes on both. Backend order, cold caches, debug compilation and system noise
prevent a comparative speed claim. Transfer-volume savings do not imply matching
frame-time savings.

## Maxwell pool and descriptor execution

`maxwell` tests validate literal NVIDIA TIC/TSC words, 48-bit image addresses versus
40-bit pool addresses, mip/view restrictions, format/channel/swizzle fields,
unsupported methods/classes, incomplete state, mapping/truncation, inclusive limits,
shared engine state across subchannels and NO_WFI invalidation without invented
completion. Every chunk size of the literal FIFO pool stream is tested.
`maxwell_allocations` processes 6,000 FIFO-to-engine writes and 1,000 descriptor
resolutions with zero measured Rust allocations/reallocations; malformed method
faults also remain compact. Geometry validation and layout construction share one
checked traversal, so descriptor validation does not allocate a mip vector.

`maxwell_gpu` executes literal pool packets, resolves literal RGBA8/BC1/ASTC TICs
and TSCs through guest memory, samples 37×19 images, repoints descriptor pools to
aliased texture backing, and verifies every framebuffer pixel. The descriptor
indices are explicit test inputs; original shader handles and draw methods remain
outside this test. Six actual draws transfer 16,872 upload bytes, 29,280 padded
readback bytes, and pack 16,872 bytes. Three unchanged synchronizations upload
nothing. Cache invalidation methods introduce no CPU waits; six readbacks do.

Executed 21 CPU-only graphics tests with `--no-default-features --locked`. All four
GPU executables (`headless`, `texture_gpu`, `compressed_gpu`, `maxwell_gpu`) passed
explicit execution on WSL GL and native Windows D3D12/Vulkan. Crate Clippy and
formatting passed. A source snapshot of graphics baseline `40531b1` plus this
deliverable, using its exact committed dependency lock, passed workspace formatting,
Clippy and 117 Rust tests; four GPU tests are ignored in that CPU run. A separate
newer workspace snapshot encountered NPDM Clippy warnings (`question_mark` and
`needless_range_loop`), independently of the graphics validation. No runtime or
NPDM source was changed by the graphics implementation.

## Binary fragment translation execution

Two 144-byte SPH-plus-machine-code fixtures were independently assembled and
then disassembled using pinned envytools. The translator decodes four MOV32I
instructions and EXIT, emits WGSL and passes Naga parsing/validation. CPU tests
cover every truncated prefix, excessive code size, incorrect header/export fields,
predicates, partial masks, unknown instructions/registers, nonfinite immediates,
missing EXIT and writes after EXIT. The allocation test additionally decodes
1,000 valid fragments with zero measured Rust allocations.

`shader_gpu` compiles and executes both translated fragments on WSL GL and native
Windows D3D12 and Vulkan. Every pixel at 66×19 and 34×13 matches exact RGBA8
`[255,0,0,255]` and `[64,128,191,255]`, respectively. Vertex colors deliberately
remain red/green; fragment output overrides both rectangles. Restoring the previous
pipeline restores the original independent red/green test pattern. Two fragment
programs create exactly two additional shader modules and pipelines; six repeated
selections hit the latest-program cache, with no additional vertex/index upload.
Seven submissions/readbacks transfer 216 upload bytes, 55,408 padded readback bytes
and pack 28,616 bytes. No Maxwell hardware or original game shader execution is
claimed.

Final execution ran all five GPU test executables explicitly on each of WSL GL,
Windows D3D12 and Windows Vulkan: 15 GPU-test executions passed, none skipped.
`cargo test -p wl-gpu --no-default-features --locked` passed 23 CPU tests. The
isolated `40531b1` baseline plus the complete graphics sources passed workspace
formatting, strict Clippy and 119 Rust tests, with five GPU tests explicitly ignored
in that separate CPU run. The known newer NPDM warnings are not counted as graphics
successes. Reassembling the documented `.asm` files independently reproduced both
144-byte fixture binaries exactly, including their recorded SHA-256 hashes.

## Guest-backed LDC fragment validation

The redistributable `ldc.bin` is 144 bytes, independently assembled/disassembled
with pinned envytools. Four B32 c0 reads at byte offsets 0x10..0x1c and a register
MOV produce exact RGBA pixels [255,0,0,255], [0,0,255,255] and [64,128,191,255]
when only guest constant data changes. Output is drawn by translated WGSL, not
copied into the target. Tests exercise B06F selector/binding packets, selector
snapshot semantics, aliases, rebinding, short/unmapped/out-of-range reads,
invalid instructions/predicates/widths/offsets, missing waits and uninitialized data.

Executed checks: both `cargo check -p wl-gpu` feature configurations with `--locked`;
`cargo test -p wl-gpu --no-default-features --locked` (26 CPU tests);
`cargo test -p wl-gpu --locked` (26 passed, six GPU tests explicitly ignored);
strict package all-target Clippy; all six GPU executables explicitly run on WSL
OpenGL llvmpipe and native Windows Intel UHD D3D12 and Vulkan (18 passes, zero
GPU skips). A Windows Vulkan launch failed in WSL `UtilAcceptVsock` before test
startup; retrying the same resource executable passed. This is distinct from a
shader/GPU test failure. Linux Vulkan remains unavailable (`libvulkan.so.1`).

The four-draw LDC test reads 68 guest bytes and uploads 60 constant bytes across
its changes/rebinds; one isolated word change uploads four bytes. It creates one
additional shader module, pipeline, uniform buffer and bind group. Thirty-two
unchanged synchronizations read/upload no bytes and allocate/reallocate no Rust
heap storage. A debug native D3D12 run measured 191.7 microseconds for those 32
CPU synchronizations; this excludes rendering and is not game-frame performance.
Readback totals are 17,984 padded GPU bytes and 10,204 packed CPU bytes. Queue
writes use owned wgpu staging; these results do not establish zero-copy transfers.

## Binary TEX.B / IPA validation

`tex.bin` independently encodes LDC c0[0], two screen-linear IPA pass inputs and
TEX.B.LZ 2D RGBA with explicit scoreboard waits. Its unchanged binary samples
TIC1/TSC0 first, then TIC1/TSC1 after a guest handle update. Invalid TIC/TSC limits
and the intentionally invalid TIC0 fail rather than selecting a default texture.
The 37x19 asymmetric image and 74x38 bilinear output validate orientation, channels,
clamped edge footprints, sampler selection, GPU aliases, descriptor invalidation
and guest texel changes. RGBA8 UNORM/sRGB, ASTC LDR fallback and BC1 fallback run
through the translated instruction. Independent scalar GOB placement and a double
precision bilinear/sRGB oracle define expected pixels with maximum one output-byte
tolerance. Alpha remains linear. A test-oracle mismatch initially compared encoded
sRGB bytes after aliasing; correcting it to decoded linear values restored validation.

The new GPU executable passed explicitly on WSL OpenGL, Windows Intel UHD D3D12
and Windows Vulkan. Initial Windows startup again encountered WSL vsock timeout;
the rebuilt final executable passed both native APIs. CPU instruction/provenance
negative tests and strict package Clippy pass. Thirty-two unchanged synchronizations
per format allocate/reallocate zero Rust heap storage and perform no constant reads,
descriptor reads or uploads. Across four formats and 13 readback draws the WSL run
recorded 84 constant guest bytes, 32 constant upload bytes, 34 descriptor reads,
13 texture uploads and 128 texture-cache hits. Exactly one additional module and
pipeline serve all these data/descriptor changes. This is a fragment harness,
not original vertex-program or guest hardware-draw execution.

The complete LDC delivery's committed-baseline workspace snapshot additionally
passed formatting, strict workspace Clippy and 224 tests (six GPU tests explicitly
ignored there and separately executed as documented above). This does not assert
that unrelated concurrent live-worktree edits passed those checks.

## Guest-selected fragment program validation

Literal B06F packets configure the B197 program region, pixel pipeline, register
count, binding group and constant-buffer state. `program_gpu` obtains the binary
from guest memory without a host shader or texture-index argument. It samples the
37x19 asymmetric Tegra image, observes a texel change, selects the LDC binary at
another program offset (exact blue), replaces that code in place (exact
[64,128,191,255]), follows a GPU alias without rereading code, and remaps the same
GPU address to a red binary (exact [255,0,0,255]). The decoded TEX sample uses the
existing one-byte maximum texture comparison tolerance. Invalid SPH, insufficient
register allocation, disabled pixel stage, unmapped code and a truncated mapping
all disable the draw rather than retaining the last valid output program.

Final package checks passed: `cargo test -p wl-gpu --no-default-features --locked`
(28 tests), `cargo test -p wl-gpu --locked` (28 passed, eight GPU tests explicitly
ignored), `cargo fmt -p wl-gpu --check` and strict all-target package Clippy.
All eight GPU executables were then run with `--ignored --nocapture` on each of
WSL OpenGL llvmpipe and native Windows Intel UHD D3D12/Vulkan: 24 executions
passed, zero GPU skips. A Windows launch initially failed in WSL `UtilAcceptVsock`
before running any test; repeating through the working local interop endpoint
completed the entire native suite. Linux Vulkan availability is not inferred from
the successful Windows Vulkan execution.

A normal source snapshot of committed baseline `88e9d34` plus the complete GPU
deliverable and the exact committed Cargo.lock passed workspace formatting, strict
all-target Clippy and 226 tests. Eight GPU tests were explicitly ignored in that
CPU run and separately executed above. Concurrent uncommitted CPU/runtime/tool
files were excluded; this is not a claim that the live workspace passed wholesale.

The selected-program test executes 29 method writes and five draw/readback
commands. Thirty-two unchanged synchronizations perform zero Rust allocations,
reallocations, shader reads, descriptor reads or uploads. One debug WSL run took
389.623 microseconds for those synchronizations; a native D3D12 run took 1.9087 ms.
These noisy CPU measurements exclude draw/readback and imply no game-level speedup.
Including valid program changes and rejected input, counters record 832 shader
bytes read, five decode attempts, 36 shader-cache hits, two descriptor reads,
20 constant guest bytes and eight constant upload bytes. Four valid distinct
programs create four modules/pipelines in addition to the renderer's two baseline
pipelines. Texture content changes preserve shader compilation. Readbacks transfer
19,792 padded bytes and pack 10,928 bytes; uploads remain explicit driver transfers.

These tests execute synthetic independently assembled fragment binaries through
real host GPUs. Geometry is still the fullscreen validation harness. No Maxwell
vertex program, B197 hardware draw trigger, guest render-target writeback or
original game-submitted graphics workload is established by these results.


## Vertex binary rasterization

Two independently assembled VTG programs load float4 position/float2 UV and export
position/generic-zero xy. The second exchanges X/Y through three MOVs. Both run
through the translated vertex stage on a 47x29 target; independently evaluated
edge functions verify covered red pixels and untouched opaque-black pixels for
two different asymmetric triangles. Only pixels within 0.005 barycentric distance
of an edge are excluded from coverage assertions; over 200 interior and 500
exterior pixels are required in each case. This test supplies vertices through the
explicit validation API, not B197 vertex state.

The vertex GPU executable passed on WSL OpenGL llvmpipe and native Windows Intel
UHD D3D12/Vulkan. Two draws upload 144 vertex bytes, read back 14,880 padded bytes
and pack 10,904 bytes. Each distinct vertex program creates one additional shader
module/pipeline, sharing the resident fragment module/layout; repeated program
selection creates neither. CPU checks pass 30 tests including stage separation,
truncation, attribute/width/modifier/predicate errors, invalid exports/register
groups and missing dependency waits. Strict package Clippy passes. This establishes
real decoded vertex influence on rasterization, without claiming a hardware draw.


## Guest vertex-array/interstage validation

Literal B06F packets select vertex/pixel binaries, stream-zero address/limit/stride,
FLOAT float4/float2 attributes and guest texture resources. A 47x29 asymmetric
textured triangle is checked against independent double-precision edge/barycentric
calculations and row-major source texels; a one-byte channel tolerance applies.
Samples within 0.005 of triangle edges or 0.02 texel of a nearest-filter boundary
are excluded to avoid asserting unspecified precision. Over 150 interior pixels
and exterior opaque-black pixels are required. Position writes move the triangle,
UV writes change sampling, GPU aliases preserve the upload, remaps replace vertex
backing and a guest program offset selects the X/Y-swapping vertex binary.

Thirty-two unchanged joint vertex/fragment synchronizations allocate/reallocate
zero Rust storage and add no shader reads, vertex reads or uploads. Geometry-only
changes preserve pipeline creation counts. A too-short inclusive vertex limit
fails and disables drawing. CPU tests independently validate literal stream fields,
formats, limits, bad instancing/frequency, stage classification and overflow.
This remains a guest-state geometry test with an explicit draw count and host
validation target; it is not yet a hardware draw trigger.

The guest-geometry GPU test passed on WSL GL, native Windows D3D12 and Vulkan.
D3D12 initially exposed a nearest-texel boundary within the original 0.001-texel
exclusion; the final 0.02-texel exclusion accounts for finite subpixel rasterization
precision without enlarging the one-byte channel tolerance. Both vertex tests pass
on both native APIs. CPU checks pass 31 tests and strict package Clippy.
