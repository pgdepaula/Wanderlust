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

## B197 hardware draw and guest framebuffer

`hardware_gpu` supplies one literal B06F stream, GPU mappings and checked mutable
guest memory. It does not supply a host draw, geometry slice, shader selection,
TIC/TSC index or output size. The stream configures both independently assembled
stages, LDC-backed texture resources, stream-zero FLOAT attributes, color target,
viewport/scissor and all required fixed raster state, then executes byte method
0x1214. The first packet is deliberately split across chunks.

The off-center triangle uses three different positions/UVs. The color target at
GPU VA 0x90000 maps to guest address 0x8000, with 208-byte pitch and 29 rows: 52
storage pixels per row, clipped to 47 visible pixels. Independent double-precision
edge/barycentric calculations check coverage and nearest texels. Pixels near edges
(0.005 barycentric distance) or nearest texel boundaries (0.02 texel) are excluded;
all asserted sampled channels retain the one-byte tolerance. Over 100 covered and
600 uncovered pixels are required for each variant. Uncovered pixels, including
the five padding pixels per row, retain `[11,17,23,255]` exactly. Frame readback and
the selected guest framebuffer bytes are equal across the entire storage span.

Thirteen actual method-triggered draws verify unchanged reuse, moved guest vertices,
texture-only mutation to `[9,200,41,255]`, pixel-program selection yielding
`[64,128,191,255]`, destination rebinding, both viewport Y signs, scissor narrowing,
vertex-program selection, in-place vertex binary mutation, target remapping, target
VA aliasing and a narrower integral viewport. The old destination remains unchanged
after rebinding/remapping. Invalid format, unmapped target, short vertex range and
short writable target fail without a further GPU submission. Faulted channels reject
subsequent draws. CPU tests cover pitch/field interpretations, unsupported state,
viewport/clip bounds, truncation fault latching and atomic checked guest writes.

The final graphics package passes 35 CPU tests with and without default features,
strict package Clippy and live workspace formatting. Normal package testing marks
11 hardware executables ignored; explicit execution runs all eleven successfully
on each of WSL OpenGL llvmpipe, native Windows Intel UHD D3D12 and Vulkan: **33 GPU
executions passed, zero GPU skips**. Some earlier native starts failed in WSL
`UtilAcceptVsock` before entering the test process; the completed final regression
ran all 22 native cases on their first attempt. A fractional viewport experiment
passed native APIs but failed GLES, which truncates it; the supported subset now
rejects fractional rectangles rather than broadening pixel tolerances.

Validation commands are `cargo test -p wl-gpu --no-default-features --locked`,
`cargo test -p wl-gpu --locked`, `cargo clippy -p wl-gpu --all-targets --locked -- -D warnings`
and `WGPU_BACKEND=gl cargo test -p wl-gpu --locked -- --ignored --nocapture`.
Native test binaries were cross-built with the exact lockfile and actually executed
with `WGPU_BACKEND=dx12` and `vulkan`; compilation alone is not counted. An ordinary
source snapshot of the committed workspace plus these graphics changes, using the
exact committed Cargo.lock, passes workspace formatting, strict workspace Clippy
and **239 workspace tests**, with the 11 GPU cases separately executed above.
Unfinished analysis/CLI/IDE changes remain outside that snapshot; this result does
not assert whole-workspace tests against those live edits. The first workspace run
was interrupted during CLI integration tests; the final complete rerun exited zero.

Measured counters for the controlled hardware workload are:

| Operation | Cold first draw | Identical next draw, added work |
| --- | ---: | ---: |
| Vertex shader code read | 144 bytes | 0 |
| Pixel shader code read | 144 bytes | 0 |
| Vertex guest read/upload | 72 bytes | 0 |
| Target guest read/upload | 6,032 bytes | 0 |
| Total host-to-GPU upload | 8,916 bytes | 0 |
| Shader modules/pipelines, including two renderer defaults | 4 | 0 |
| Target allocations | 1 | 0 |
| GPU-to-host padded readback | 7,440 bytes | 7,440 bytes |
| Packed framebuffer write to guest | 6,032 bytes | 6,032 bytes |

Across all thirteen variants, the workload records 105 method writes before fault
probes, 144 vertex-upload bytes, 464 vertex-code bytes/three decodes, 288 pixel-code
bytes/two decodes, two descriptor reads, one target allocation/twelve reuses,
72,120 total upload bytes and 78,416 bytes written to guest framebuffers. CPU resets
of the background deliberately trigger target uploads before mutation comparisons.

One complete unchanged D3D12 draw/readback/writeback observation took 3.34 ms and
performed 57 process-wide Rust allocations/one reallocation, including wgpu and
owned frame output. WSL observations varied by milliseconds. These debug, tiny
synchronous samples establish no speedup or game-frame timing. The separate
32-iteration resource synchronization test still records zero Rust allocations,
reads or uploads; it excludes draw encoding/readback and must not be confused with
a complete draw. This path transfers real data and synchronizes on every draw;
it is not zero-copy or asynchronous game rendering.

The result establishes a bounded hardware-driven synthetic guest draw. It does
not establish nvdrv/VI integration, a visible Windows presentation surface, original
game-submitted graphics commands or a Tears of the Kingdom frame.

## Indexed B197 validation

The indexed quad fixture uses four shared vertices plus an intentionally invalid
unused dummy vertex. A U16 prefix is skipped by first-index one and base vertex
+1; U32 rebinding uses base -1 and independently changed backing at the same GPU
VA. Independent rectangle/diagonal coverage assertions verify RGBA
[64,128,191,255] inside and [11,17,23,255] outside, including pitch padding.
Changing index contents removes the second triangle. An out-of-bounds resulting
vertex faults before submission and leaves framebuffer bytes unchanged.

Executed package checks: 37 CPU tests passed under both feature configurations;
12 GPU tests are ignored by ordinary Cargo test and explicitly executed on WSL
OpenGL (12 passed). The new indexed test and previous hardware test also passed
on native Windows D3D12 and Vulkan (four executions); the final indexed negative
case was rerun on both native backends (two passed). Formatting and strict package
Clippy passed. The indexed four-draw workload read/uploaded 48 index bytes in
three updates; its unchanged draw caused no index upload or pipeline creation.
These measurements describe this synthetic workload, not game performance.

## Deferred draw and coherence scenarios

`batched_gpu` executes three overlapping guest triangles, selecting independent
Maxwell quarters/red/TEX binaries through B197. The final texture draw uses the
real guest VS varying. An independent barycentric/pixel oracle verifies visible
layers, draw order and untouched pitch padding. Before explicit synchronization,
guest RAM retains its old bytes. Three draws use one target initialization, three
queue submissions, zero readbacks/waits; final visibility adds one submission,
one readback/wait and 6,032 guest write bytes (52x29 RGBA8). Padded readback is
7,424 bytes plus 16 optional timestamp bytes.

A 32-draw warm sequence asserts no additional shader or descriptor reads,
uploads, pipeline creations, intermediate readbacks or waits. Final visibility
uses one readback/wait and 33 submissions in total. Process-wide Rust allocations
and CPU submission/completion times are logged, with tests serialized for the
measurement; driver activity still contributes. No zero-allocation, zero-copy or
game frame-time claim follows from these counters.

Additional actual GPU tests cover CPU patch merging, target aliases/rebinds,
CPU writes before another draw, source texture aliasing pending GPU output,
readback after channel faults, failed atomic writeback retries, epoch changes and
lost dirty-journal history. The texture alias deliberately reinterprets linear RT
bytes through a block-linear TIC; that sampled-texture test alone does not establish
block-linear render-target support; the color-publication tests below cover it. All tests use redistributable fixtures.

The deferred/coherence implementation was explicitly executed on WSL OpenGL and
native Intel UHD D3D12/Vulkan: 16 GPU tests per backend passed (48 executions,
zero GPU skips). Two D3D12 texture-test process launches failed in WSL transport
before test startup; the third launch executed and passed. Those transport errors
are not counted as successful GPU runs. The 32-draw warm D3D12 observation recorded
7.82 ms CPU submission time and 1,346 process-wide Rust allocations plus 34
reallocations, with zero added uploads/pipelines/descriptor reads and one final
7,440-byte readback/wait. This is a debug microbenchmark, not a frame-rate estimate.

A faithful ordinary source snapshot of the intended GPU delivery, retaining the
exact lockfile and excluding unrelated unfinished runtime edits and the pending
independent conformance file, passed workspace formatting, strict workspace Clippy
and 242 CPU/workspace tests. Sixteen GPU tests remain explicitly ignored in that
ordinary workspace command and are accounted for by the execution runs above.

## Global alpha blend and write-mask validation

`blend_gpu` selects the existing independently assembled LDC fragment and vertex
binaries through B197. Two overlapping triangles read different guest constant
RGBA values with identical shader code. The independent per-draw UNORM oracle
expects [37,136,75,192] in the first-only region, [56,13,33,160] in the second-only
region, and [76,102,72,208] in overlap, from background [11,17,23,128]. A one-byte
channel tolerance covers blend/attachment quantization; coverage-edge ties are
excluded separately. All four coverage classes contain independently checked pixels.

Switching from OGL to equivalent D3D ADD/factor encodings creates no new pipeline
or shader module. Disabling blend and selecting red/blue writes creates exactly
one compatible pipeline, no new shader module, and preserves green/alpha. Mutable
constant data uploads 32 bytes over the three-draw scenario. CPU tests verify
all sixteen masks, incomplete state and unsupported factors/operations.

Final execution: 17 GPU tests passed on each of WSL OpenGL, native Windows Intel
UHD D3D12 and Vulkan (51 successful executions, zero GPU skips). Windows startup
transport failures were retried only before test startup; actual GPU failures were
not skipped. Package formatting/strict Clippy passed; 48 CPU tests passed under
both feature configurations. A faithful stable source snapshot including the
committed independent index suite passed workspace formatting, strict Clippy and
252 tests, with 17 GPU tests explicitly ignored by ordinary workspace Cargo test.
Uncommitted runtime/compiler changes were excluded from that snapshot; this
does not establish live-worktree runtime integration.

The indexed GPU fixture additionally verifies that a second GPU VA alias of the
same U32 index backing causes zero extra index reads/uploads. A three-element
U16 draw reads six bytes and uploads eight, validating four-byte upload padding
without fetching the padded element. The expanded six-draw fixture passed again
on WSL OpenGL and native Windows D3D12/Vulkan, with formatting and strict package
Clippy. It measured 60 guest index bytes including the rejected final scan,
56 uploaded index bytes, two index cache hits and one CPU padding allocation.
The previously reported four-draw/48-byte measurement above describes the smaller
fixture before these additional alias/alignment cases.

Direct guest-resource helpers additionally reject calls while a target is pending.
Their caches cannot be populated from stale RAM outside the channel's guarded
source resolution. The GPU writeback-retry test verifies rejection of direct
fragment, constant, vertex and texture synchronization with unchanged counters,
then completes the pending image and independently validates its pixels.

## GPFIFO acquisition and execution validation

`gpfifo_gpu` enters exclusively through guest-format GPFIFO bytes and mapped
command memory. It uses existing independently assembled Maxwell VS/TEX/quarters
binaries and literal B197 state. The primary fixture fetches an indexed triangle
with an asymmetric texture into a 52x29 pitched RGBA8 target. A scalar barycentric
oracle specifies coverage and UV samples independently of host rasterization;
texture colors and untouched background are checked at unambiguous pixel centers.
The first SET_OBJECT packet is split across both entries and submission calls.

The five GPU scenarios exercise:

- Ordered indexed draws, two program changes/draws in one entry, unchanged warm
  reuse, GPU VA command aliases, texture mutation without recompilation, and
  replacement command backing at the same GPU address selecting another binary.
- A valid draw followed by an unsupported packet or unmapped entry. Fault progress
  reports the submitted prefix; retry does not replay it. Atomic writeback rejection
  preserves pending output, and retry publishes the independently checked image.
- GPU-written target bytes subsequently fetched as another entry's command word.
  Publication must precede the read; generated color bytes reject as unsupported
  opcode 7 rather than executing the stale guest NOP.
- Metadata preflight versus fetch/engine failures, adjacent separate mappings,
  short guest reads, unsupported class and a zero-allocation NOP acquisition path.
- A draw overwriting the unread suffix of its own current entry. The old suffix
  contains NOPs; the generated [64,128,191,255] pixel is a different instruction
  word. Resumed acquisition performs one barrier/readback and rejects opcode 7,
  with one submitted draw and zero completed entries. A prematurely owned snapshot
  of the suffix would incorrectly succeed, so this tests actual reacquisition.

CPU cases cover literal field extremes, 40-bit end bounds, every rejected flag,
zero-length control entries, wire maximum versus supported aggregate budget,
entry-array boundaries, full-range mapping and packet continuation/fault latching.
A separate one-test allocation executable avoids interference from parallel Rust
unit tests: 1,000 metadata preflights and entry iterations allocate zero Rust heap
objects. The GPU-enabled NOP loop separately executes 1,000 submissions/8,000
command bytes with zero measured Rust allocations, excluding device setup/draws.
This does not imply driver allocation freedom during real drawing.

The 32-draw warm fixture consumes 1,024 command bytes through 64 successful guest
reads and 64 entry decodes (preflight plus execution), borrowing 1,280 span bytes
including overlap when resuming after draw. No owned command-buffer array is
created. Shader/descriptor reads, uploads and pipelines remain unchanged. Final
visibility requires one readback/wait; 32 draws plus readback use 33 submissions.
The earlier four-scenario native D3D12 observation measured 30.25 ms CPU submission,
1,346 process-wide Rust allocations/34 reallocations for that draw loop. These
noisy debug samples are not a throughput or game-frame performance claim.

The first CPU allocation probe incorrectly shared its executable with parallel
tests and observed unrelated allocations; isolating the probe fixes its scope.
The initial negative class assertion expected the wrong diagnostic capitalization;
its correction preserves UnsupportedClass rejection. Strict Clippy also identified
an API newer than the declared Rust 1.85 MSRV; byte-array divisibility now uses a
compatible operation. These unsuccessful runs are not counted as validation passes.

Final package validation passed formatting and strict Clippy. CPU graphics tests
passed under default and no-default features: 53 passed, zero failed. The final
five GPFIFO GPU scenarios passed on WSL OpenGL and native Intel UHD Windows D3D12
and Vulkan. Together with all seventeen existing GPU regressions executed on each
backend, the matrix covers 22 successful scenarios per backend, 66 total, with
zero GPU skips. Windows transport startup errors occurred before test execution;
retries were limited to those startup failures, never used to conceal GPU failures.

A faithful ordinary source snapshot retaining the exact Cargo.lock, including the
final GPU sources and excluding unfinished runtime/compiler edits, passed workspace
formatting, strict workspace Clippy and 257 tests. Its 22 GPU tests are ignored by
ordinary Cargo test and accounted for by the explicit execution matrix above.
This does not establish integration of the unfinished runtime changes or execution
of an original game-submitted command stream.

## Driver-owned channels, mappings and completion

`driver.rs` has four CPU integration tests for literal ioctl direction/size/
command fields, both submission buffer layouts, truncated headers, wrong entry
counts, unsupported flags/fences, exact capacity limits, fixed page alignment,
reservation overlap, alias identity, wrong-space IDs, full unmap/remap, stale IDs,
address overflow and cross-mapping borrow rejection. A private raster unit test
proves direct unaligned offsets cannot mutate aligned registers. A literal INC
packet covers 1340..1358 including GLOBAL_COLOR_KEY=0; enabled/reserved values fault.

Three ignored `driver_gpu` scenarios are explicitly executed with real adapters:

- A page-mapped driver/channel submission executes indexed TEX shaders. Independent
  barycentric coverage and nearest-texture expectations check asymmetric geometry
  and background pixels. Waiting completes host work without readback or RAM
  publication. Pending target ownership blocks another channel and mapping teardown.
  Thirty-two CPU patches survive publication; write 33 fails before changing RAM.
  Thirty-two warmed draws preserve uploads, shader reads and pipelines and perform
  zero intermediate readbacks/waits. Index remapping selects degenerate geometry,
  yielding background rather than stale indices. Closed tokens cannot be reused.
- A valid draw followed by malformed command returns its submitted prefix and
  faults only that channel. Real completion remains available. Injected atomic
  guest-write failure leaves RAM unchanged and close retry publishes the correct
  frame without incrementing draw count. A fresh channel can subsequently draw.
- One thousand NOP submit/retire calls measure zero Rust allocations/reallocations,
  4,000 command bytes borrowed and zero queue submissions (device setup excluded).
  The 65th outstanding token is rejected before consuming commands; retirement
  restores admission. Alternating channels preserve an incomplete packet in one
  channel without leaking its B197 binding into the other.

Final live package checks: `cargo fmt --all --check`, strict package Clippy,
`cargo test -p wl-gpu --no-default-features --locked` and default-feature tests
passed: 58 CPU tests, zero failures. Default Cargo execution ignores 25 GPU tests;
these were separately executed via `WGPU_BACKEND=gl cargo test -p wl-gpu --locked
-- --ignored --nocapture --test-threads=1`, and the same Windows test executables
ran with DX12 and Vulkan. Each backend passed 25 scenarios, zero failures/skips:
75 actual GPU executions. WSL used llvmpipe; native Windows used Intel UHD Graphics
(driver 32.0.101.7076 / Vulkan 101.7076).

A debug native D3D12 observation for 32 warm driver draws measured 14.53 ms CPU
submission, 1,420 process-wide Rust allocations and 36 reallocations. Vulkan
measured 10.51 ms, 1,580 allocations and 34 reallocations; GL measured 55.16 ms,
1,569 allocations and 128 reallocations. These include wgpu and callback overhead,
exclude final publication, and are noisy single observations, not GPU durations
or game benchmarks. Warm draws added zero upload bytes, readbacks or blocking
waits. One final publication submits one readback and waits once. The NOP loop
measured 5.59 ms on DX12 and 5.45 ms on Vulkan with zero measured Rust allocations;
it is not directly comparable to earlier NOP loops of different command lengths.

During development one warm-upload assertion incorrectly included the necessary
refresh after a CPU framebuffer patch. The final measurement begins after that
refresh. An MSRV-incompatible integer helper and a needless reference were also
corrected; these failed intermediate checks are not counted as passes.

A faithful ordinary source snapshot of committed `1aacfd4` plus the exact GPU
deliverable and unchanged Cargo.lock passed workspace formatting, strict workspace
Clippy and 262 CPU tests; 25 ignored GPU tests are accounted for by the explicit
backend matrix. This excludes unfinished shared compiler/CLI edits and is not a
claim that those live-worktree modifications passed workspace integration.

## Block-linear color publication

Three CPU tests compare every byte of bidirectional conversion with an independent
scalar GOB oracle for 1x1, 7x9, 16x17, 33x13, 64x8, 65x65 and 127x31 RGBA8
surfaces, each at all six block heights. Literal within-GOB landmarks supplement
roundtrips. Tests retain every padding byte, reject truncated buffers and validate
mode-dependent WIDTH, array pitch, unsupported memory fields, zero dimensions,
conversion limits and the exact 40-bit surface-end boundary. Existing pitch raster
assertions still check identical dimensions/stride/size, with complete-state
validation used for fields whose interpretation now depends on layout.

Four ignored `block_target_gpu` scenarios enter through `GpuDriver::submit` using
real GPFIFO entries and literal B06F/B197 indexed draw methods. They execute the
existing independently assembled ALD/AST vertex and LDC/IPA/TEX fragment fixtures:

- A non-square 52x29 target begins with asymmetric guest colors and nonzero padding.
  Independent barycentric coverage and nearest sampling check clipped textured
  geometry and uncovered pixels. Full storage bytes match the scalar oracle.
  A subsequent LDC color draw exercises alpha blending and R/B-only writes.
  A CPU texel write before another draw refreshes the tiled attachment and survives
  uncovered, with shader/pipeline reuse.
  Thirty-two warmed draws add no upload, conversion, module/pipeline creation,
  readback or blocking wait; one explicit final publication exposes the result.
- A valid draw followed by a malformed packet preserves the accepted GPU prefix.
  An unaligned raw CPU patch crosses a GOB microtile boundary; a disjoint patch
  modifies padding beyond the logical linear byte count. Atomic publication failure
  preserves RAM and pending output. Retry retains both patches, the rendered image
  and untouched padding without increasing the GPU draw count.
- A GPU VA alias reuses the same resident canonical target without upload/readback.
  Command acquisition from padding beyond logical image size forces full-footprint
  visibility. A block-height switch with identical dimensions and allocation size,
  without a CPU target write, forces publication and correct reinterpretation.
  Pitch/tiled rebinding reuses the host attachment and pipelines. Close publishes
  remaining tiled output.
- Remapping the same target GPU VA selects different canonical storage, retaining
  old backing and using new asymmetric contents. Pending unmap is blocked. A later
  target with only its first page mapped rejects the missing final GOBs before a
  draw or guest-memory mutation.

Expected pixel colors are independently specified by source image and barycentric
math. Boundary tie pixels are excluded from color assertions, while byte layout
and padding assertions still cover the complete allocation. Blending permits at
most one UNORM code step per checked color channel; other comparisons are exact.

Final live package validation passed `cargo fmt --all --check`, strict
`cargo clippy -p wl-gpu --all-targets --locked -- -D warnings`, and locked tests
with default and no-default features: 65 CPU tests passed in each configuration.
Ordinary default tests ignore 31 GPU scenarios; they were explicitly executed
with `WGPU_BACKEND=gl cargo test -p wl-gpu --locked -- --ignored --nocapture
--test-threads=1` and native Windows binaries through `windows_gpu.py`. All 31
passed per backend: WSL llvmpipe OpenGL, native Intel UHD D3D12 and native Intel
UHD Vulkan, 93 unique scenario/backend executions with zero GPU failures/skips.
The four tiled scenarios also passed after the final CPU-before-next-draw assertion
was added. WSL interop startup failures occurred before test execution and only
those launches were retried. An intermediate package check exposed the old raster
test's removed `pitch` field; it now asserts the equivalent `linear_pitch()` and
retains invalid pitch/stride validation. That failed check is not counted as a pass.

At 52x29/BH2, logical pixels occupy 6,032 bytes and guest tiled storage 8,192.
The controlled 36-draw workload (including 32 warm draws, blend/mask and a CPU
refresh) measured one target allocation, 35 reuses, 12,064 target upload bytes,
49,152 guest target read bytes, 44,832 decode bytes, 32,768 encode bytes,
24,128 overlay-copy bytes, four readbacks/waits, and 32,768 guest publication
bytes. The warm interval alone added 32 queue submissions and zero uploads,
conversions, target allocations, shader modules, pipelines, readbacks or waits.
Publication uses two padded conversion vectors plus the packed GPU readback;
CPU patch/redecode vectors are additional when required. This is not allocation-free.
The failed-publication fixture acquired seven exact CPU patch bytes twice, without
issuing another draw.

Single debug observations of 32 warmed tiled submissions measured 46.73 ms,
1,409 Rust allocations/34 reallocations on D3D12; 57.83 ms, 1,570/33 on Vulkan;
and 148.82 ms, 1,569/128 on GL during concurrent workspace compilation. Timing
includes wgpu/channel/callback work and excludes final publication. Process-wide
allocator traffic remains; these noisy observations are not GPU durations or a
game benchmark. The existing pitch workload uses the same 52x29 dimensions and
also adds zero warm uploads/readbacks/waits. Tiling adds conversion and an 8,192-byte
publication span instead of 6,032 bytes at explicit visibility, not a per-draw wait.
No comparative speedup is established.

A faithful ordinary snapshot of committed `270b5e4` plus the GPU deliverable,
retaining the exact Cargo.lock and excluding unfinished compiler/CLI/IDE changes,
passed workspace formatting, strict workspace Clippy and 269 CPU tests. Its 31
ignored GPU scenarios are accounted for by the explicit backend matrix. This is
not validation of unfinished peer modifications or original-game GPU execution.

## B197 ZF32 validation

`depth_gpu` adds six hardware-channel scenarios. Actual GpuDriver/KickoffPb
requests acquire mapped GPFIFO buffers, select guest shaders and indexed vertices,
and execute B197 depth state. Independent pixel-center barycentrics, scalar depth
comparisons and GOB byte addresses check the output, rather than using the
production conversion as an oracle. Coverage includes all eight comparisons with
writes on/off, asymmetric initialized depth, textured output, ordered overlapping
Z=.5/.625 geometry, additive blending/channel masking, CPU patches in texels and
padding, aliases, backing remaps, source feedback, malformed descriptors/storage,
channel faults, failed depth/color writes and retry without draw replay.
A separate exact-bit corpus checks zero/one, arbitrary normalized mantissas and
the smallest normal f32 through actual GPU initialization/export. Subnormals,
negative zero, NaNs and out-of-range initialization are unsupported.

The six scenarios ran successfully on WSL OpenGL/llvmpipe, native Windows D3D12
and Vulkan/Intel UHD. Native execution used actual Windows test executables.
The unchanged-depth 32-draw interval adds exactly 32 queue submissions and no
uploads, readbacks, waits, texture/target allocations, shader modules or pipelines.
One depth initialization uploads 6,032 logical bytes from 8,192 padded guest bytes
for the 52x29 BH=2 fixture. Explicit depth-plus-color publication currently uses
two readbacks/waits. It is a per-attachment visibility boundary, not per-draw
synchronization. Process allocator activity remains nonzero; the debug workload
is not a game benchmark. No original-game GPU submission is demonstrated.

Depth delivery gates: 67 CPU tests passed with default features and 67 without;
formatting and strict package Clippy passed. The regression matrix plus the six
new scenarios comprises 37 GPU scenarios per backend, all explicitly executed
on GL, D3D12 and Vulkan (111 distinct backend/scenario passes). Startup-only WSL
vsock failures were retried; GPU failures were not treated as skips. A faithful
ordinary source snapshot of the committed base plus the depth changes and exact
Cargo.lock passed strict workspace Clippy and 271 CPU tests; 37 GPU tests were
ignored by that ordinary suite and executed separately as above. The snapshot
excluded unrelated unfinished analysis/CLI/IDE changes; it is not a live shared
workspace integration result.

## Maxwell FADD/FMUL validation

`alu_gpu` enters through GpuDriver/GPFIFO with guest-selected shader binaries.
The pixel program loads three mutable guest constants and performs FADD/FMUL,
producing independently specified covered-pixel RGBA values [191,128,191,255]
and [64,16,64,64]. Changing only constants preserves shader modules, pipelines
and decode counts. Replacing the independent FMUL with the independently established
FADD opcode produces [191,255,191,255] and one additional binary decode. Outside
pixels retain the guest background. A separate independently assembled vertex
program applies x*=.5 and y+=.5 to guest positions; scalar barycentrics verify
its changed raster coverage. Neither test supplies host replacement geometry.
CPU tests verify masks, default register fields, stage identity, RZ operands,
uninitialized/pending registers, unsupported modifiers, existing WAW rejection
and direct/MOV-mediated contraction-prone dataflow rejection.

Arithmetic delivery validation passed 72 CPU tests in each feature configuration,
strict package Clippy and formatting. All 39 GPU scenarios passed explicitly on
WSL OpenGL, native Windows D3D12 and Vulkan: 117 distinct backend/scenario passes.
A faithful source snapshot of depth commit 1e1fbd5 plus arithmetic changes, with
exact Cargo.lock and excluding unfinished peer files, passed strict workspace
Clippy and 276 CPU tests; its 39 ordinary ignored GPU tests are covered separately.
One intermediate mutation fixture created deliberately unsupported FMUL-to-FADD
dataflow and failed on GL/D3D12; replacing an independent operation corrected the
fixture without weakening the decoder. One native startup failed before entering
tests and was retried. These failures are not counted as passing executions.

The four-draw arithmetic fixture measured 48 constant guest/upload bytes, two
pixel decodes/352 binary bytes, 72 vertex upload bytes, eight index upload bytes,
one target allocation/three reuses, eight queue submissions and four explicit
publication readbacks/waits. Constant-only updates created no new shader modules
or pipelines. Coverage uses exact binary fractions and UNORM quantization; no
claim of general Maxwell floating-point equivalence or game performance follows.

## Perspective IPA validation

The independent 176-byte fragment fixture selects PositionW and perspective xy
in its SPH, reads the LDC handle and executes IPA.PASS W, MUFU.RCP, perspective
IPA xy and TEX.B.LZ. Its guest channel draws indexed asymmetric geometry with
clip W=(1,2,4) into the existing guest target. Scalar f64 pixel-center barycentrics
and rational homogeneous interpolation specify the texture samples independently.
At (19,16), (18,11), (24,12), perspective UV selects 4x4 texels (1,0), (0,1), (1,1),
while affine interpolation selects (1,1), (0,2), (1,2). Exact RGBA expectations are
[67,11,40,255], [7,61,36,255], [67,61,45,255]. Uncovered pixels retain guest RAM
background. A known covered pixel remains old RAM until explicit publication.

The same scenario switches between guest affine/perspective pixel programs,
checking qualifier-dependent VS module creation, and then changes only guest
clip W/XY to (4,1,2), checking changed samples with no shader/pipeline creation.
The 32-draw warm interval asserts zero new uploads, shader reads, descriptor
reads, shader modules, pipelines, readbacks or waits, with 32 queue submissions.
CPU tests independently verify strict masks, header modes, register count,
scoreboard waits and MOV-preserved provenance followed by actual overwrite.

An initial test retired deferred tokens before publication; the driver correctly
rejected that request. Tokens are now retired after the single explicit warm
publication. An intermediate D3D12 sample at (13,12) differed from ideal f64
nearest addressing: scaled V=0.998845 lies within a 16.8 fractional sampling step
of the texel boundary. The oracle now excludes this documented precision region
on every backend, retaining exact assertions elsewhere and all literal samples.
No expected color or production shader was changed to accommodate that sample.

Final perspective gates passed 76 CPU tests with default features and 76 without,
strict package Clippy and formatting. All 40 GPU scenarios ran explicitly on WSL
OpenGL/llvmpipe, native Windows D3D12/Intel UHD and native Windows Vulkan/Intel UHD:
120 distinct scenario/backend passes. Startup-only interop launches were retried.
A faithful source snapshot of arithmetic commit 639f068 plus perspective production
and test changes, retaining Cargo.lock and excluding unfinished peer files, passed
strict workspace Clippy and 280 CPU tests; its 40 GPU scenarios were ignored by
ordinary execution and validated separately as above.

The controlled 36-draw perspective workload measured one target allocation/35
reuses, 144 vertex upload bytes, eight index upload bytes, 41 queue submissions
and five explicit publication readbacks/waits (37,200 staging readback bytes,
30,160 guest framebuffer bytes). Shader-interface switches rebuild compatible
stage modules/pipelines; vertex-data-only mutation does not. A single debug warm32
observation measured 22.10 ms and 1,640 allocations/131 reallocations on GL,
20.28 ms and 1,477/36 on D3D12, 12.40 ms and 1,647/36 on Vulkan. Allocator counters
are process-wide and timings are CPU observations under concurrent builds, not
GPU duration, throughput claims or a game benchmark. No zero-copy/zero-allocation
claim or original-game graphics execution is established.

## Integer shader validation

The independently assembled PS fixture loads five raw words through B197-bound
LDC and executes wrapping IADD plus AND/OR/XOR. Guest values exercise actual carry
overflow, sign-bit boundaries and nontrivial masks. Literal expected RGBA outputs
are [128,64,191,255], [64,64,96,128], [64,0,96,128], [0,0,128,128]. Constants-only
updates preserve modules, pipelines and decode count. Replacing the guest OR with
AND changes blue from 191 to zero and increments the binary decode count.
An independent VS fixture applies bit operations to real guest XY attributes;
chosen normal finite values produce X*=2 and Y=-Y, including partially clipped
geometry. Scalar barycentrics verify changed coverage, untouched background and
warm pipeline/vertex-upload reuse. No host geometry or replacement shader is used.

Six CPU tests check independently assembled fields and negatives, RZ source/dest,
uninitialized/pending operands and destination, stage identity, unsupported LOP
predicate outputs, special-provenance clearing and conservative FMUL taint through
both integer sources. Both new GPU scenarios passed on GL, native D3D12 and Vulkan.
The complete package passed 82 CPU tests in each feature configuration, formatting
and strict Clippy. All 42 GPU scenarios passed explicitly per backend, 126 unique
backend/scenario executions. Startup-only interop failures were retried. A faithful
source snapshot of fadb9bc plus integer changes, exact Cargo.lock and no unfinished
peer files passed strict workspace Clippy and 286 CPU tests; its 42 GPU tests were
ignored in ordinary execution and covered by the explicit backend runs.

The five-draw PS workload measured 100 constant guest-read bytes, 96 constant upload
bytes, two pixel decodes/416 binary bytes, 72 vertex upload bytes, eight index upload
bytes, one color target allocation/four reuses, ten queue submissions and five
explicit readbacks/waits (37,200 staging bytes; 30,160 guest framebuffer bytes).
Constant edits do not specialize shader code. These are small correctness fixtures,
not an instruction-coverage percentage or game-performance benchmark.

## Hardware clear validation

Four CPU tests validate minimal color/depth descriptors independently of draw
state, strict clear controls/trigger values, selected clear-value limits and a
literal incrementing B06F packet split across chunks. Six GPU scenarios enter
through GpuDriver/KickoffPb/GPFIFO and execute actual attachment clears:

- Pitch 7x9 and block-linear 7x9/BH1 and 52x29/BH2 produce exact RGBA
  [64,128,191,255] without shader/vertex selection or old-pixel uploads.
  Draw write mask zero and incomplete enabled blending do not suppress clears.
  Independent scalar GOB addresses check every logical byte and padded byte.
- CPU writes before clear are superseded; exact raw CPU patches after clear win,
  including padding. Atomic write failure leaves RAM unchanged and retry does
  not execute another clear. The initial warm test hit the driver's bounded
  pending-CPU-write budget by rewriting command RAM on every iteration; warming
  now reuses one explicitly uploaded command buffer, preserving that guard.
- Depth-only, color-only and combined clears preserve the untouched pending
  attachment. Partial depth-success/color-failure publication retires only the
  successful attachment; retry does not republish it. A depth-only visibility
  call returns no color Frame and increments the corrected visibility counter;
  a no-op visibility call does not. Waiting completes host work without RAM
  publication or readback.
- Clear-depth-before-draw rejects farther Z=.5 geometry with LESS against .25.
  ALWAYS writes .5 before a color-only clear; depth remains .5 at a known covered
  point. A later depth-only clear to .75 preserves independently checked red
  triangle coverage over blue. No target/depth initialization upload occurs.
- Later invalid clear commands latch the channel while preserving earlier output
  for close/publication. Invalid controls/values/mappings and missing final padded
  GOBs produce no clear. A 52x65/BH2 allocation needs 20,480 bytes and cannot use a
  16,384-byte mapping merely because its logical pixels fit.
- Rebinding a pending target drains the old backing before clearing the new one;
  aliases reuse compatible residency. Pending color/depth overlap rejects without
  discarding prior output. A clear overwriting its unread GPFIFO suffix publishes
  and reacquires zeros; the old invalid suffix is never executed.

The complete package passed formatting, strict Clippy and 94 CPU tests with each
feature configuration. All 48 GPU scenarios passed explicitly on WSL OpenGL,
native Windows D3D12 and native Windows Vulkan: 144 distinct backend/scenario
passes, zero GPU skips. Startup-only WSL launches were retried. An ordinary
faithful snapshot of dc367b7 plus the clear implementation, exact Cargo.lock and
excluding unfinished compiler/CLI and conformance revisions passed strict workspace
Clippy and 298 CPU tests; 48 ordinary GPU ignores are covered by the explicit runs.
This does not validate unfinished runtime changes or original-game GPU commands.

At 52x29/BH2, 33 clears use one target allocation/32 reuses, zero target uploads,
zero guest shader reads/decodes and no new shader/pipeline during the warm 32-clear
interval. That interval adds 32 queue submissions and zero conversions, readbacks or
waits. Injected publication failure plus retry adds two readbacks/waits: 14,880
staging bytes, 16,384 storage-read bytes and 8,192 successful guest-write bytes.
The raw five-byte CPU patch/padding overlay is acquired twice, without re-clearing.
One debug warm32 observation recorded 6.22 ms/1,269 allocations/38 reallocations on
GL, 23.02 ms/1,094/2 on D3D12 and 13.87 ms/1,259/2 on Vulkan. Process-wide allocator
traffic and CPU timings remain; these are not zero-copy, GPU-duration or game
performance claims. Each clear still submits separately to the host queue.


## Guest face culling and bounded source observations

The literal GpuDriver/KickoffPb/GPFIFO fixture exercises 24 combinations of
positive/negative viewport Y, both guest index windings, CW/CCW front orientation,
and disabled/front/back culling. An independent signed-area/barycentric oracle
checks strict interior/exterior pixels of asymmetric geometry on a 52x29 target.
Edges within the stated barycentric tolerance are excluded; no pixel expectation
comes from host cull state or prior output. Degenerate triangles produce only the
cleared background. Incomplete enabled state faults without a new draw/pipeline,
and earlier pending output remains publishable without replay.

After this matrix, 32 unchanged deferred draws produce zero new vertex/index or
other uploads, pipelines, modules, readbacks and blocking waits. They still issue
32 queue submissions. Process-wide Rust allocation instrumentation, excluding
native-driver allocations, measured the following single debug observations:

| Backend | CPU submission interval | Rust allocations / reallocations |
|---|---:|---:|
| WSL OpenGL / llvmpipe | 21.65 ms | 1576 / 131 |
| Native Windows D3D12 / Intel UHD | 23.41 ms | 1413 / 34 |
| Native Windows Vulkan / Intel UHD | 45.44 ms | 1573 / 34 |

These are noisy small-workload observations, not frame-rate or speedup claims.
The token vector is reserved before measurement; host/runtime bookkeeping remains
allocated and neither command submission nor GPU transfers are claimed zero-copy.

A separate 128-draw sequence rolls over the 64-entry journal using disjoint
command writes and explicit publication. Vertex/index upload bytes and vertex
shader decodes remain unchanged. A real index mutation changes visible coverage
and uploads eight padded bytes. An overlapping index write followed by 65 other
writes forces a fresh index/vertex read and uploads eight/72 bytes respectively;
the newly selected winding is verified. A vertex write then degenerates geometry
and uploads exactly 72 bytes. The allocation-free journal model test independently
checks 1024 advancing observations, interval boundaries, overlapping writes,
epoch mismatch, future cursors and lost history. Exact pending-output patch
retrieval still rejects lost history.

All three new GPU scenarios actually executed on WSL OpenGL and native Windows
D3D12/Vulkan. The full package has 98 CPU tests in each feature configuration;
ordinary default-feature testing leaves 51 GPU scenarios explicitly ignored.
Strict package Clippy and workspace formatting checks passed.

The complete explicit matrix passed 51/51 scenarios on each backend (153 GPU
executions, zero skips). A faithful ordinary source snapshot based on
`4a54cac4924ca456c98237d09e23196aaf6ed2b9` plus the owned graphics edits and exact
Cargo.lock passed strict workspace Clippy and 302 CPU tests; its 51 GPU scenarios
were ignored there and executed separately above. The snapshot excludes unfinished
analysis/CLI/IDE changes and is not a live-runtime integration result. Two native
process launches failed with WSL UtilAcceptVsock before tests began; bounded
startup-only retries executed and passed the actual binaries. No GPU assertion
failure was retried as a startup error.


## Unsigned Maxwell shifts

Independently assembled VS/PS fixtures add strict GPR SHL/SHR.U32 with clamp and
`.W` count modes. Five CPU tests check exact opcode/operand decoding, WGSL/Naga
validation, stage identity, rejected signed/reverse/extended/CC/predicate forms,
register readiness and RZ, plus special-resource provenance and FMUL rounding-
hazard retention. Source-level masks remain strict; this is not general shader
predication or every Maxwell shift variant.

The first actual GpuDriver/GPFIFO GPU scenario loads mutable value/count words
through B197 constant bindings and produces four exports: left clamp, left wrap,
right clamp and right wrap. Twelve independently specified RGBA expectations
include counts 0, 1, 31, 32, 33, 63, 64, `0x80000000`, `0xffffffff`.
For raw value `0x3f800001`, count 32 yields `[0,255,0,255]` and count 33 yields
`[0,255,0,0]`. Raw value `0x7e000000` shifted right by one is float .5, producing
`[0,0,128,128]`; count 33 gives `[0,0,0,128]`. These distinguish unsigned high-count
clamping from bare host wrapping. Inputs deliberately avoid NaN/infinity output
expectations. Quantized colors do not prove every result bit for all operands.

All twelve draws retain one PS decode (176 shader bytes), one VS decode (144
bytes), one target allocation and eleven reuses. Guest constant reads total 96
bytes and changed constant uploads total 56 bytes. Vertex/index uploads are
72/eight bytes initially, with no repeat uploads. There are twelve explicit
publications/readbacks/waits (89,280 staging readback bytes) and 24 queue submissions;
this correctness workload intentionally requests visibility after each result.

The second scenario executes guest VS shifts that clear clip-X's sign bit and
preserve Y using wrapped count 32. A separate barycentric oracle uses abs(X) for
the fixture's finite normal positions and checks asymmetric inside/outside pixels.
An unchanged repeat creates no module/pipeline or upload and does not decode VS
again. No host-selected substitute geometry or fragment code is used.

All 53 GPU scenarios passed on WSL OpenGL, native Windows D3D12 and native Windows
Vulkan (159 actual passes, zero GPU skips). The package passed 103 CPU tests in
both feature configurations; strict package Clippy and workspace formatting passed.
Native runs encountered WSL UtilAcceptVsock startup failures before tests; retained
logs and bounded startup-only retries, including verification of the active
interop socket, distinguish those failures from the successful GPU executions.

A faithful ordinary source snapshot based on `52e0447b488c3675b69053a5f78ea3fd4d40da0e`
plus the owned shader changes and exact lockfile passed strict workspace Clippy
and 307 CPU tests; its 53 GPU scenarios were ignored there and executed explicitly
above. Unfinished peer analysis/CLI/IDE changes remain excluded and preserved.
This does not establish live Horizon GPU-service integration.


## Unsigned Maxwell IMNMX

Five CPU tests validate exact static-predicate min/max fields, unsigned WGSL/Naga
lowering in both stages, readiness/RZ, rejected signed/extended/CC/dynamic-predicate
forms, and retained special-resource provenance and FMUL rounding-hazard guards.
Independent unmodified envyas reproduction matched both fixture hashes exactly.

Two actual GpuDriver GPU scenarios verify unsigned ordering in seven literal
RGBA cases, including reversed/equal operands and sign boundaries. Raw operands
`0xbf800000` and `0x3f000000` produce `[128,0,128,0]`; a mistaken signed comparison
would reverse that result. `0x80000000` and `0x3f800000` produce `[255,0,255,0]`.
An in-place guest MIN-to-MAX opcode replacement changes `[64,191,64,191]` into
`[191,191,64,191]` and adds exactly one PS decode. Mutable constant changes alone
retain shader modules/pipelines. The separate VS min/max program changes negative
clip X to zero while preserving Y, checked with independent asymmetric coverage.
An unchanged repeat uploads no resource or shader and recreates no pipeline.

The eight-draw PS workload uses one vertex/index upload (72/eight bytes), one
resident target allocation and seven reuses, 64 constant-read/upload bytes, two
PS decodes (352 bytes) including intentional code replacement, one VS decode
(144 bytes), eight explicit publications/readbacks/waits (59,520 staging bytes)
and 16 queue submissions. The code replacement creates one new PS module and
rebuilds its fragment harness and paired graphics pipelines; constant data changes
do not. These are bounded correctness measurements, not game performance evidence.

All 55 GPU scenarios actually passed on each of WSL OpenGL, native Windows D3D12
and native Windows Vulkan (165 passes, zero skips). Windows binaries were built
with the existing helper's build configuration, then executed as native child
processes of one PowerShell validation runner, retaining individual logs/backend
selection and 90-second per-process deadlines. This avoids repeated WSL process
handoffs without changing rendering or treating compilation as execution.

108 CPU tests passed in each package feature configuration; strict package Clippy
and formatting passed. A faithful ordinary snapshot based on
`1cd79a22aa51d4924c2d89f089cf3f2af3fba04f` plus owned IMNMX edits and exact Cargo.lock
passed strict workspace Clippy and 312 CPU tests, with 55 GPU scenarios ignored
there and explicitly executed above. Peer analysis/CLI/IDE changes remain preserved
and excluded; this is not a live Horizon integration claim.


## Predicate comparisons and SEL

Eight CPU tests validate the conservative scheduling subset, all six unsigned
ISETP comparisons, initialized P0 selection/inversion, rejected signed/extended/
Boolean/other-predicate forms, and independent GPR metadata. ISETP cannot initialize
or clear pending R7; an LDC-derived R7 texture handle retains its provenance across
the predicate write. SEL cannot manufacture constant-handle/PositionW/reciprocal
provenance. Candidate and predicate-only FMUL-taint regressions preserve the
existing conservative unfused-rounding guard. Both independently reassembled
fixtures matched their documented hashes; generated VS/PS WGSL passed Naga.

The PS GpuDriver scenario performs 27 genuine indexed draws. Nine literal color
cases cover unsigned sign boundaries, equality, false/true and inverted P0, P0
rewriting and independently changing the selected colors. The nine mutable-data
cases retain shader modules, pipelines and PS decode count. Eighteen additional
cases change the guest compare opcode through all six unsigned modes, checking
less/equal/greater operands against a separate scalar oracle. Condition true must
select the first candidate. For .25/.75 colors, `(0,1)` under LT yields
`[64,191,191,255]`; `(1,0)` yields `[191,64,191,255]`; equality yields
`[191,64,64,255]`. Compare inputs may contain all u32 bits without being
misinterpreted as floating-point values.

The controlled PS workload reads 432 constant bytes and uploads 192 changed
constant bytes. It decodes seven PS binaries (1456 bytes), including the notified
same-code write and five distinct mode changes. Only the five actual mode changes
allocate additional PS modules/paired pipelines. One VS decode (144 bytes), one
72-byte vertex upload, one eight-byte padded index upload, one target allocation
and 26 target reuses are observed. Its 27 requested publications perform 27
readbacks/waits (200,880 staging bytes), with 54 queue submissions. This intentionally
synchronous validation does not characterize frame-like performance.

The VS scenario selects clip X or RZ using an unsigned predicate, then mutates
only guest vertex X values. A separate finite-position coverage oracle confirms
the changed triangle. The mutation uploads 72 vertex bytes and retains VS decode,
shader modules and pipelines; an unchanged repeat uploads nothing. Genuine B197
clear state removes the earlier image before checking moved geometry.

All 57 GPU scenarios passed on WSL OpenGL and actual native Windows D3D12/Vulkan
(171 passes, zero skips). Native runs use the existing build configuration and
single-parent Windows process runner with individual logs and timeouts. Both
package feature configurations passed 116 CPU tests, including the final added
predicate-only taint case. Strict package Clippy and formatting passed.

A faithful ordinary source snapshot based on `29b1f559503a62e632794dc44ab90a7e13f63da0`
plus the owned predicate changes and exact Cargo.lock passed strict workspace
Clippy and 320 CPU tests; its 57 GPU scenarios were ignored there and explicitly
executed above. It excludes unfinished peer analysis/CLI/IDE changes and does not
establish live Horizon GPU-service execution.


## LDC.B64 word pairs and upper-lane texture handles

`ldc64.rs` adds seven CPU cases for independent pair encodings and Naga output,
full-pair scheduling/readiness, pending overwrites, stage/mode/alignment limits,
nonconsecutive deduplicated inputs, the 63/64-word budget, upper-lane handle
provenance and clearing previous interpolation/arithmetic metadata. The previous
scalar-load mutation test now distinguishes verified B64 from unsupported size6.

`ldc64_gpu.rs` adds three explicitly executed scenarios. Real GpuDriver→GPFIFO→
B197 indexed draws export runtime CB colors `[64,128,191,255]`, `[255,0,128,64]`
and `[0,255,64,191]`, using an analytic triangle-coverage oracle and literal colors.
A second program samples using R7 from B64 R6/R7, with an invalid lower-lane
handle. Two actual TIC entries select asymmetric 37×19 images; changed handle and
texture bytes change independently predicted barycentric/nearest-sampled pixels
without shader/pipeline recreation. A binding safety case rejects fragmented
mappings, missing final bytes and short backing, disables stale execution and
recovers using a valid binding. Hardware tests additionally reject short CB range
and an effective address aligned only to four bytes without another accepted draw.

Package CPU tests passed 123/123 with default features and 123/123 without them.
Strict package Clippy and whole-workspace rustfmt check passed. All 60 explicit
GPU scenarios passed separately on WSL OpenGL/llvmpipe, native Windows D3D12 and
native Windows Vulkan: 180 actual passes, no skips. Ordinary package execution
correctly reports 60 GPU scenarios ignored. A faithful ordinary source snapshot
of the committed base plus exact owned changes passed 327 CPU tests (60 GPU tests
ignored) and strict workspace Clippy; peer-owned unfinished analysis/CLI/IDE
changes were excluded. This is snapshot validation, not live runtime integration.

After three color publications, 32 identical deferred hardware submissions add
32 draws/queue submissions, 1024 consumed command bytes and 1280 borrowed command
bytes (suffix reacquisition is counted), with no new CB/geometry uploads, shader
reads/decodes, descriptors, modules, pipelines, target allocations, readbacks or
blocking visibility waits. One final explicit publication follows. This remains
one queue submission per draw. A representative debug observation measured
17.86 ms / 1570 allocations/128 reallocations on GL,10.63 ms / 1415 / 34 on D3D12 and
19.84 ms / 1571 / 33 on Vulkan. Process-wide allocator traffic includes driver/wgpu
bookkeeping and is not zero; timings are noisy synthetic CPU observations, not
GPU execution durations or game-performance predictions. The three cold/changed
color draws read/upload 48 CB bytes, decode the 112-byte PS once and upload 72 vertex
bytes plus 8 padded index bytes. The four textured draws retain one PS decode and
four shader modules/pipelines while changing resource data; they perform four
explicit readbacks (29,760 staging bytes) and four waits as requested by the test.

These are independently assembled synthetic Maxwell programs. Original-game GPU
submission, full LDC forms and universal Maxwell scheduling remain unproven.


## Signed IMNMX comparisons and vertex geometry

Five CPU cases in `signed_minmax.rs` validate the independently assembled signed
GPR bases, stage interfaces, raw signed bitcast WGSL/Naga, RZ, initialization,
scoreboards, modifier rejection, provenance erasure and retained arithmetic hazard
taint. The older unsigned suite now accepts the verified signed form rather than
classifying signedness as an unsupported bit mutation.

Two real GPU scenarios enter through GpuDriver→GPFIFO→B197. Nine independently
specified input pairs include signed extrema, equality, operand reversal and
finite IEEE payloads. The fragment exports selected sign bits as finite 0.5/1.0,
producing literal byte colors such as `[255,128,255,128]` and `[128,128,128,128]`.
This observes signed ordering across the sign boundary, not every selected result
bit. Changing only CB data retains shader/module/pipeline identity. Changing the
first signed minimum to unsigned in guest code produces `[128,128,255,128]` and
exactly one additional decode. The vertex program compares against RZ, modifying
finite geometry to X=min(X,0), Y=max(Y,0). An independent pixel-center/signed-area
oracle verifies coverage and uncovered background with more than 70 covered and 600
uncovered pixels; a repeated draw retains vertex data, code and pipeline resources.

Package validation passed 128 CPU tests with default features and 128 without them;
strict package Clippy and whole-workspace formatting passed. All 62 GPU scenarios
passed explicitly on WSL OpenGL, native Windows D3D12 and native Windows Vulkan:
186 actual passes, no ignored scenarios in those explicit executions. Normal
package tests report 62 GPU scenarios ignored. A faithful ordinary committed-source
snapshot plus exact owned changes passed 332 CPU tests (62 GPU scenarios ignored)
and strict workspace Clippy. Peer-owned unfinished changes were excluded.

The ten fragment draws (nine data cases plus code mutation) read 80 CB bytes and
upload 76, upload 72 vertex and 8 padded index bytes, allocate one target and reuse it
nine times. Two PS decodes read 608 shader bytes. Data-only changes retain the
initial four shader modules/pipelines; the code change adds one module and two
pipelines (including the existing validation harness). Ten requested publications
perform ten readbacks/waits, 74,400 staging bytes and 20 queue submissions. The
vertex case decodes its 176-byte program once; the unchanged repeat adds no shader
or geometry upload and no pipeline/module creation. These are bounded synthetic
workloads, not original-game shader coverage or game performance measurements.


## IADD subtraction, byte observation and fragment-cache endurance

`subtract.rs` adds five CPU cases for independent source-negation encodings,
operand normalization, WGSL/Naga validation, RZ/readiness, source/destination
aliasing, strict modifier/PO rejection, provenance clearing and arithmetic hazard
taint. The existing integer suites now distinguish supported single-negation
forms from the independently assembled, unsupported both-bit PO encoding.

Three real GPU scenarios enter through GpuDriver→GPFIFO→B197. The word oracle
checks 39 input pairs for each subtraction form (78 accepted draws), including
wraparound, signed-bit boundaries and a deterministic 32-pair generator. Expected
RGBA is the separately calculated low 32-bit result in little-endian byte order;
`0x89abcdef−0x01020304` must produce `[235,202,169,136]`. A separate sweep checks
all 256 possible output-byte values on all four RGBA channels. The observation
fixture uses bounded exact float construction rather than exporting arbitrary
integer bits as NaNs. The vertex shader uses both encodings and an independent
half-coordinate coverage oracle for the explicitly normal finite geometry.

A reproduced regression showed an unchanged fragment being decoded again when
unrelated writes rotated the global journal, despite unchanged shader modules and
pipelines. PS/CB observations now retain no-write proofs across frequent accesses.
The 256 data-changing cases retain one PS decode through journal rotation. A code
write followed by 65 unrelated writes still forces acquisition and rejects PO;
missing history cannot validate an old shader. Another case submits a valid draw,
then faults on PO before publication. RAM stays unchanged until explicit publish,
which returns the earlier accepted pixels `[67,51,34,17]` without draw replay.

A warm endurance request carries 128 identical draws in 4096 consumed command bytes.
It adds one GPFIFO request, two entry decodes (preflight/execution), 129 borrow calls,
265,216 borrowed-span bytes (suffix reacquisition), 128 host queue submissions and
128 target reuses. It adds no shader/CB reads, uploads, modules, pipelines,
readbacks or explicit completion waits. The final publication adds exactly one
readback and one wait. Identical output plus counters proves reuse/endurance;
differentiated ordering remains covered by the existing multi-draw scenarios.
This does not claim host submission batching or copied command payloads.

Representative debug CPU observations for this 128-draw request were 56.64ms with
6056 allocations/520 reallocations on GL, 32.64ms with 5379/128 on D3D12 and 69.93ms
with 6019/128 on Vulkan. Process-wide allocator traffic includes wgpu and channel
bookkeeping; these are not GPU durations, zero-copy claims or game benchmarks.
The 256 byte cases read 2048 CB bytes and upload 1020, with one 464-byte PS decode.
They intentionally request one publication/readback per observed byte result.
The normal 78-draw word case reads 624 CB bytes/uploads 600; its final PO fault adds
an unsuccessful decode attempt, giving three attempts/1392 shader bytes including
two valid programs. One target allocation and 77 reuses precede that fault.

Validation passed 133 CPU tests with default features and 133 without them, strict
package Clippy and whole-workspace formatting. All 65 GPU scenarios passed on each
WSL OpenGL, native Windows D3D12 and native Windows Vulkan backend: 195 actual
passes, no skips in explicit execution. Normal package tests report 65 GPU cases
ignored. A faithful ordinary source snapshot passed 337 CPU tests (65 GPU tests
ignored) and strict workspace Clippy, excluding peer-owned unfinished files.

That endurance request uses immutable commands and one completion record,
respecting the 64-record limit and the then-global 32-write guard. Protected
attachment histories below subsequently replace the global write-notification
limit with per-attachment overlap limits.
Original-game GPU submissions and general Maxwell shader compatibility remain
unproven.


## Pending attachment history under unrelated write pressure

Five CPU model tests cover exact clipped patch unions, stable pending content
stamps through 4096 disjoint notifications, unprotected cache-history eviction,
32-overlap preflight failure, stale/future baselines, ranges/epochs/revision
overflow, clear rebasing and independent attachment retirement.

Four actual GPU scenarios in `pending_history_gpu.rs` enter through
GpuDriver→guest GPFIFO→B197 and render an indexed asymmetric triangle into a
52×29 block-linear RGBA8 target with 8192 bytes of storage. Independent scalar
GOB addresses verify padding and CPU patches; analytic pixel-center barycentrics
verify more than 1400 unambiguous covered/uncovered pixels. Edge-adjacent pixels
are excluded from coverage expectations, not used to derive expected output.
A ZF32 ALWAYS/write scene independently checks depth bytes and padding.

The scenarios verify: pending output surviving 4096 disjoint writes and 32 more
unchanged draws; 32 actual overlapping notifications followed by rejection of the
33rd before RAM changes; failed atomic color publication and successful retry
through another 4096 disjoint writes; depth-first partial publication without
repeating depth on color retry; later CPU color patches; canonical target aliases
and close-time publication; and a full color clear superseding old visible patches
while preserving padding and pending depth patches. Existing pitch-linear retry
coverage now expects successful protected publication after disjoint global
history eviction, retaining its independent multilayer pixel oracle. Ordinary
unprotected history-loss rejection remains covered by CPU model tests.

An isolated instrumented region measures only 4096 synchronous disjoint RAM and
journal writes, after host completion and before further GPU activity/logging.
It observed zero Rust allocations/reallocations on GL, D3D12 and Vulkan, taking
approximately 3.66ms, 4.70ms and 1.59ms respectively in representative debug runs.
These are CPU adapter observations, excluding native driver allocation, not GPU
execution timings or a claim that drawing/conversion is allocation-free.
Global source proof eviction legitimately causes reacquisition on the first later
draw, but does not cause target upload/publication. After that prime draw, 32 warm
draws add exactly 32 target reuses/queue submissions and zero uploads, shader reads,
pipelines, readbacks or waits. One final publication adds one readback/wait and
7440 staging bytes including optional timestamps. No host submission batching or
zero-copy framebuffer transfer is claimed.


Validation passed 138 CPU tests with default features and 138 without them,
strict package Clippy and workspace formatting. All 69 GPU scenarios passed on
WSL OpenGL, native Windows D3D12 and native Windows Vulkan: 207 actual passes,
zero failures/skips in explicit runs. Ordinary default-feature tests report those
69 GPU cases ignored. A faithful ordinary committed-source snapshot plus the exact
owned patch passed 342 CPU tests (69 GPU cases ignored) and strict workspace
Clippy; unfinished peer-owned analysis/CLI/IDE changes were excluded. No original-game GPU submission or TOTK rendering is established.

## Signed ISETP and complete selected-word observation

Five additional CPU cases verify independent six-mode signed encodings in VS/PS,
per-operation signedness in mixed programs, unchanged unsigned WGSL, paired CB
inputs, raw-i32 bitcasts, readiness/RZ/predicate scheduling, special provenance
erasure and rejection of unsupported extended/Boolean/predicate/operand forms.
Alternate immediate/constant negatives pass the ordinary scheduling gate before
asserting opcode rejection. Reserved F/T comparison modes remain unsupported.

Three actual GPU scenarios enter through GpuDriver→GPFIFO→B197. The full-word
observer tests 43 input pairs for each of LT/EQ/LE/GT/NE/GE: signed extrema,
negative/positive ordering, equality and 32 deterministically generated pairs.
An independent i64 sign-extension oracle selects between two runtime CB words;
the candidates are complements, making every bit of a wrong selection observable.
The verified byte-observation shader exports all 32 selected bits as four RGBA8
bytes, rather than interpreting arbitrary integer words as floating colors. All
258 cases check analytic covered/uncovered pixels and retain shader/pipeline
identity under data-only changes. One additional accepted draw followed by an
unsupported .X code mutation remains unpublished until explicit visibility;
publication returns `[152,186,220,254]` without replaying either submission.

The mixed shader tests signed −1<0 alongside independently unsigned −1>=0:
changing only the second instruction's signedness changes `[64,191,191,255]`
to `[64,191,64,255]`. Thirty-two unchanged draws add zero uploads, shader/CB reads,
modules, pipelines, intermediate readbacks or waits, while retaining 32 ordered
host queue submissions. One final publication adds one readback/wait. The vertex
fixture applies signed X<0 then SEL to guest positions, producing asymmetric
coverage. After a genuine B197 background clear, changing only vertex instruction
signedness to unsigned collapses all X positions to zero and produces no covered
pixels. Expected geometry is independently specified; neither generated WGSL nor
previous readback supplies the oracle.

Representative debug warm32 CPU submission observations were 17.14ms with
1581 allocations/133 reallocations on GL, 9.87ms with 1414/36 on D3D12 and 20.09ms
with 1571/33 on Vulkan. These process-wide Rust counts include backend/channel
bookkeeping and do not establish GPU durations or game performance. The word
scenario executed 259 successful draws and one later fault, with 259 intentional
readbacks/waits, 4144 CB guest bytes, 4000 CB upload bytes, one target allocation
and 258 reuses. Seven PS decode attempts include six valid comparison programs
and the rejected .X program; changing CB values did not recreate their pipelines.

Validation passed 143 CPU tests under each feature configuration, strict package
Clippy and whole-workspace formatting. All 72 GPU scenarios passed on each actual
WSL OpenGL, native Windows D3D12 and native Windows Vulkan backend: 216 passes,
zero explicit skips/failures. Normal default-feature tests ignore those 72 GPU
cases. A faithful ordinary source snapshot passed 347 CPU tests and strict
workspace Clippy, excluding unfinished peer-owned work. The strengthened alternate
opcode negative tests were additionally rerun in both feature configurations and
in the snapshot after source review. These synthetic guest shaders and command
streams do not establish original-game GPU execution or general ISA coverage.
