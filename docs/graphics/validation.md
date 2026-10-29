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


## Arithmetic SHR and complete result observation

Five CPU tests verify the independent signed/default/wrapped encodings in both
stages, explicit unsigned count bounds, i32 bitcasts, RZ, in-place value/count
operands, readiness/barriers, unsupported modifiers/predicates/operand fields,
resource provenance erasure and FMUL-hazard taint from either input. The former
unsigned suite's signed-word rejection now asserts the distinct signed IR variant;
unsigned lowering remains covered unchanged.

Two actual GPU scenarios enter through GpuDriver→GPFIFO→B197. The full-word
observer checks nine values against twenty counts for each default and .W form:
360 observed results, including zero, signed extrema, −1, nonuniform bit patterns,
counts 0/31/32/33/64 and high unsigned counts through ffffffff. Expected bits come
from an independent logical-shift/sign-mask oracle, separately cross-checked with
mathematical signed floor division. Every result byte is verified through RGBA8
readback and analytic triangle coverage, avoiding NaN/color clamping loss.
For 80000001 shifted by 33, default gives ffffffff and .W gives c0000000.

Data-only changes preserve the selected program/pipeline. An accepted later .W
draw remains pending when newly written .BREV code faults; publication returns
`[0,0,0,192]` without draw replay or premature guest writes. The vertex shader's
real SHR sign-mask changes guest geometry according to independent min(X,0)
coverage. Its 32 warm repeated draws add 32 queue submissions and no uploads,
shader reads, modules, pipelines, intermediate readbacks or waits. Explicit final
visibility adds one readback/wait. This is deferred ordered execution, not host
submission batching or allocation-free drawing.

The word workload records 361 successful draws and one later fault, 2888 CB guest
bytes and 1504 CB upload bytes, two valid 464-byte programs plus one failed decode
attempt, one target allocation and 360 reuses. Its per-case oracle intentionally
uses 361 readbacks/waits; the separate warm scenario proves deferred reuse.
Representative warm32 debug CPU observations were 21.25ms with 1573 allocations/
130 reallocations on GL, 16.33ms with 1410/34 on D3D12 and 11.83ms with 1568/32 on
Vulkan. Process-wide Rust activity includes backend/channel bookkeeping; these
are not GPU timings, zero-copy claims or game-level performance measurements.

Validation passed 148 CPU tests in each feature configuration, strict package
Clippy and whole-workspace formatting. All 74 GPU scenarios passed on each
actual WSL OpenGL, native Windows D3D12 and native Windows Vulkan backend:
222 passes, zero explicit skips/failures. Ordinary default-feature tests ignore
those 74 GPU cases. A faithful ordinary source snapshot passed 352 CPU tests
(74 GPU cases ignored) and strict workspace Clippy, preserving unfinished
peer-owned files outside the snapshot. Original-game GPU execution remains
unproven; these independently assembled programs are synthetic conformance data.

## Ordered mutable constants across deferred channel requests

`mutable_draw_gpu.rs` submits 64 genuine driver/GPFIFO requests with guest-indexed
geometry, the independently assembled ALD/AST vertex program and LDC pixel program.
The guest vertex buffer specifies a clipped triangle covering the viewport; each
request selects its own RGB constants and one-pixel B197 scissor. Thirty-two
successive overlapping draw pairs cover an 8×4 tile grid inside 52×29 pitch RGBA8.
An independent exact-frame oracle requires each odd-colored draw to overwrite its
even-colored predecessor, with all other pixels and unused pitch columns retaining
background. Reversed pair order or retroactively replaced constant data changes
observable pixels. No host draw helper supplies the geometry or fragment color.

After the first cold draw, 63 mutable draws read 1008 CB bytes and upload 756 changed
bytes; unchanged alpha is not uploaded again. Assertions require 63 target reuses,
zero additional target/vertex/index/texture uploads, shader reads/decodes, module/
pipeline creation, intermediate readbacks or completion waits. The two CPU writes
per request rotate the global journal without changing protected pending target
identity. This workload depends on actual mutable per-draw resource isolation,
not identical-draw endurance alone.

All 64 framebuffer updates remain GPU-newer until one explicit publication. A 65th
request must report the specific completion-capacity preflight error, without
altering RAM or graphics counters. Publication checks every framebuffer byte,
uses one readback/wait and writes 6032 guest bytes. All 64 tokens must then report
host completion and guest visibility before retirement. Total queue submissions
remain 65: 64 draws and one readback; no submission batching is claimed.

The allocation interval includes test fixture vectors, channel/WGPU bookkeeping
and asynchronous backend activity. Representative debug observations for 63 warm
mutable submissions were 68.36 ms with 3972 allocations/252 reallocations on GL,
38.31 ms with 3740 allocations/67 reallocations on D3D12
and 27.32 ms with 3812/65 on Vulkan. Transfer and synchronization counts are asserted;
CPU timings are not GPU durations or game-performance predictions.

The normal gates passed 148 CPU tests in each feature configuration, strict package
Clippy and workspace formatting. The full matrix passed 75 GPU scenarios per WSL
OpenGL/native Windows D3D12/native Windows Vulkan backend, with no GPU failures or
explicit skips. The strengthened overlapping-pair/capacity-specific candidate was
additionally executed successfully on all three backends after review. Native
launch initially encountered a WSL interoperability startup timeout 110 before
GPU execution; a bounded startup retry after verifying native process launch
completed the matrix. This is separate from graphics-test execution.
A faithful ordinary source snapshot passed 352 CPU tests (75 GPU cases ignored)
and strict workspace Clippy; the final strengthened test also passed package
Clippy in that snapshot. No production code or peer-owned runtime files changed
for this regression, and no original-game GPU workload is established.

## Variable-latency low-word integer multiplication

Seven CPU tests cover the four GPR IMUL signedness encodings, both shader stages,
barrier-zero production and consumption, unavailable registers, RZ, in-place
destinations, unsupported HI/CC/predicate forms, provenance erasure and retention
of unfused floating-point hazards. The independently assembled fragment, byte
observer and vertex fixtures retain the original assembler instruction bytes.

Three actual driver/GPFIFO GPU scenarios validate integer multiplication. The
byte observer checks 78 boundary/random operand pairs under all four low-result
signedness encodings: 312 complete 32-bit products, with expectations calculated
using an independent 16-bit-limb oracle. A later accepted draw followed by an
unsupported HI program fault remains publishable without replay; the expected
product 5621ca08 appears as RGBA bytes `[8,202,33,86]`. A second scenario checks
four finite float-bit exports, distinguishing integer multiplication from numeric
floating-point multiplication. The vertex scenario changes asymmetric raster
coverage according to independently specified min(X,0) positions.

The word workload performs 313 successful draws and one fault, 314 GPFIFO requests,
five pixel-program decode attempts and 2320 shader bytes acquired. Four valid
signedness encodings normalize to the same low-result semantics and reuse the
compatible pipeline. It reads 2504 CB bytes and uploads 2432, with one target
allocation and 312 reuses. Its per-case oracle intentionally performs 313
readbacks/waits, transferring 2328720 staging bytes over 626 queue submissions.
These per-case checks are separate from deferred rendering measurements.

Thirty-two warm vertex draws add no resource uploads, shader bytes, modules,
pipelines, intermediate readbacks or waits. They still add 32 queue submissions;
one explicit publication provides final visibility. Representative debug CPU
observations were 30.208ms and 1574 allocations/129 reallocations on GL, 10.802ms
and 1417/34 on D3D12, and 11.172ms and 1568/32 on Vulkan. Process-wide allocator
traffic includes backend activity. These are neither GPU durations nor game
performance estimates, and the conversion/upload paths do not become zero-copy.

Validation passed 155 CPU tests in each feature configuration, strict package
Clippy and whole-workspace formatting. All 78 GPU scenarios actually passed on
each WSL OpenGL, native Windows D3D12 and native Windows Vulkan backend: 234
passes, zero failures or explicit skips. Ordinary default tests ignore those
78 GPU cases. The historical ordinary source snapshot reported 359 CPU tests and strict
workspace Clippy, but shared its Cargo artifact directory with the live checkout.
It is not independent proof of executing that exact source snapshot; the isolated
artifact validation below supersedes that evidence.
No original-game GPU execution is established by these synthetic fixtures.


## Independent global blend equation matrix

Six CPU tests verify 100 RGB/alpha compact-versus-GL alias combinations, independent
alpha selection, incomplete state, raw coefficient preservation across MIN/MAX,
unsupported equations and unaligned methods. Literal incrementing B06F packets
cross 0x1354 with explicit inactive color-key state; nonzero values latch a fault,
preserve successfully consumed state and never consume the unread coefficient.

Three GPU scenarios enter through GpuDriver, mapped GPFIFO commands and B197 indexed
draws. An independently defined f64 equation oracle checks all 25 RGB/alpha pairs
under both compact and GL aliases, plus two factor-sensitive subtraction cases.
Source RGBA consists of exact binary fractions [3/4,1/4,5/8,3/8]; destination RGBA8
is [51,153,204,230]. ONE/ONE ADD, SUBTRACT, reverse SUBTRACT, MIN and MAX respectively
expect [242,217,255,255], [140,0,0,0], [0,89,45,134], [51,64,159,96] and
[191,153,204,230], with at most one RGBA8 unit of conversion tolerance. MIN/MAX use
contradictory raw ZERO/ZERO coefficients to expose accidental weighting. Analytic
triangle coverage includes clipped and uncovered pixels and unused pitch columns.

A tiled test combines MIN RGB/MAX alpha, then restores stored source-alpha factors
for reverse subtraction with an R/B write mask. The independent overlap-blue
expectation is 40; reversing the draw order gives 68. Guest RAM stays unchanged
until publication. Unsupported weighted MIN at 0x134c faults before its following
draw, while both earlier draws remain publishable exactly once. Independent scalar
GOB addresses verify padding bytes remain unchanged.

The 52-case matrix intentionally resets and publishes the target for each oracle
check: 52 draws, 104 queue submissions, 52 readbacks/waits, 313664 target upload
bytes, one target allocation and 51 reuses. It creates 30 pipelines while retaining
four total shader modules (including baseline infrastructure). Equivalent aliases
reuse the current pipeline; actual equation/factor changes rebuild the compatible
pipeline without decoding either guest shader again.

A separate tiled warm scenario changes ignored factors and an inactive alpha
equation, then executes 32 unchanged deferred draws. Assertions require zero extra
uploads, layout conversions, shader reads/decodes, module/pipeline/target creation,
intermediate readbacks or waits. Draws still use 32 queue submissions; one final
publication provides guest visibility. Representative debug CPU observations were
17.172ms and 1575 allocations/130 reallocations on GL, 31.379ms and 1409/32 on
D3D12, and 10.338ms and 1571/33 on Vulkan. These process-wide observations include
backend work and do not establish GPU timing, allocation-free execution or game
frame performance.


Validation passed 161 CPU tests in each graphics feature configuration and strict
package Clippy. All 81 GPU scenarios passed on each actual WSL OpenGL, native
Windows D3D12 and native Windows Vulkan backend: 243 passes, zero failures or
explicit skips. The final strengthened three-test blend executable also passed
on both native backends. GPU-owned Rust formatting passes.

The live workspace attempt encountered concurrent peer-owned CLI/analysis edits:
E0560 for an instruction_limit field in Roots, followed by formatting differences
in unfinished analysis source. Those files were preserved. An ordinary faithful
snapshot of committed d4eb09c plus the exact GPU candidate, with identical
Cargo.lock, reported 369 CPU tests (81 GPU tests ignored), strict workspace Clippy
and whole-workspace formatting. That run shared Cargo artifacts with the live
checkout, so it cannot establish exact snapshot execution independently. The
isolated artifact validation below supersedes that evidence. It does not claim
that unfinished live peer changes passed. Original-game GPU execution remains unproven.


## Native FLOAT3 vertex fetch

Seven CPU tests validate B197 FLOAT3 position descriptors, 12-byte position
footprints, tight/reordered/padded attributes, limits, invalid descriptor atomicity,
aliases and remaps. An independently assembled ALD/MOV/AST vertex fixture exports
the fetched position W through UV.x. Missing FLOAT3 W must become 1, even when
adjacent guest bytes contain 0.25 or 0.5. The actual translated vertex program,
GPFIFO channel and B197 indexed draw determine both coverage and texture samples.

Two GPU scenarios verify tight 20-byte, reordered 32-byte and padded 24-byte
strides against analytic coverage and an independently specified asymmetric
texture. FLOAT3 samples column 36 of the 37-column texture; FLOAT4 with explicit
W=0.5 samples column 18 and changes coverage. Switching formats rebuilds the
pipeline without decoding shaders or uploading unchanged vertex bytes. Aliases
reuse resources; CPU writes and backing remaps each upload the required 72 bytes.
A legal FLOAT4 declaration whose offset exceeds the stride faults before drawing
with stale state, retaining earlier accepted output for explicit publication.

The three packing cases upload their actual 60, 96 and 72 guest bytes: 228 bytes
total, with no CPU expansion of FLOAT3 to FLOAT4. Thirty-two warm draws add no
resource reads/uploads, conversions, shader modules, pipelines, readbacks or waits.
They still add 32 queue submissions and require one final visibility boundary.
Representative debug CPU observations were 49.568ms and 1568 allocations/128
reallocations on GL, 9.264ms and 1417/32 on D3D12, and 21.288ms and 1569/32 on
Vulkan. Process-wide allocation traffic includes backend work; these observations
are not GPU durations, game performance estimates or zero-copy claims.

The exact six implementation/test/fixture files were compared byte-for-byte
against an ordinary source snapshot based on d27d44b, retaining identical
Cargo.lock. Its dedicated Cargo artifact directory was separate from both the
live checkout and other source snapshots. Whole-workspace formatting and strict
workspace Clippy passed; workspace tests passed 380 CPU tests with 83 GPU cases
ignored. Graphics tests without default features passed 168 CPU tests. All 83
GPU scenarios actually passed on isolated WSL OpenGL, native Windows D3D12 and
native Windows Vulkan: 249 passes, zero failures or explicit skips. Windows
executables included the final strengthened invalid-layout case. These results
do not validate concurrent unfinished runtime edits.

The isolated artifact directory is essential: Cargo executable names may collide
across ordinary source snapshots sharing one target directory. Historical shared
artifact snapshot counts above are retained as reported observations, not exact
source-isolation proof. The GPU backend executions and their explicit pixel
assertions are recorded separately. No original-game GPU submission or frame is
established by these synthetic fixtures.


## Explicit homogeneous Z interval

Six CPU tests distinguish 0x0d7c from viewport clip control, require every bit of
the 32-entry presence mask, reject alternate/reserved/unaligned selectors without
mutation, preserve literal packet fault prefixes and verify clear independence.
The GPU scenario enters through GpuDriver and mapped GPFIFO indexed draws with
an actual translated vertex program and red or TEX/IPA pixel programs. Missing
0x0d7c faults before any draw or queue submission. A fresh channel then tests
uniform normalized depth -0.25,0,0.25,0.5,0.75,1,1.25 and four mixed-depth
near/far crossings with unit or unequal clip W. Two more crossings use textured
screen-linear inputs with an independently specified asymmetric image.

The color/depth storage stays non-square 52x29, while guest viewport, surface
clip, viewport clip and scissor explicitly select 32x16. Binary-exact authored
positions project to window vertices (2,15),(31,13),(8,1). Near/far intersections
are edge midpoints on the raster subpixel grid. Independent window-space
barycentrics determine coverage, affine UV and interpolated Z/W. Uniform accepted
depths compare bit-exactly; nonuniform depths use the existing 2e-6 bound. Pixels
near geometry/clip/sample-quantization boundaries are excluded from ambiguous
comparisons. Uncovered color, rejected depth and tiled padding remain unchanged.

The initial decimal-geometry oracle failed D3D12 at (18,5): observed0.68851227
versus unsnapped0.68846403846. Independently snapping projected vertices to the
documented 1/256-pixel grid gives0.68851224456. The final fixture corrects that
oracle premise without increasing tolerance or changing production translation.
The earlier failure is not counted as a passing run.

Fourteen accepted draws remain deferred until each explicit validation boundary.
The scenario intentionally resets/publishes attachments for individual checks:
28 readbacks/waits, 56 queue submissions including portable depth initialization,
1008 vertex upload bytes, one color and one depth allocation, 13 color-target
reuses, 114688 depth guest-write bytes and 84448 color guest-write bytes. Changing
only depth/vertex data reuses shader modules/pipelines within each program mode.
Switching red/TEX/red deliberately translates three pixel program selections.
An unsupported interval after an accepted draw faults without queue submission,
implicit publication or replay; the earlier color/depth remain publishable.
These oracle-driven synchronization counts are not a per-draw runtime policy.

Final validation passed 174 graphics CPU tests in both feature configurations,
strict workspace Clippy and whole-workspace formatting. A faithful ordinary
snapshot of 229ca1d plus the exact four clipping implementation/test files, with
unchanged Cargo.lock and a dedicated artifact directory, passed 393 workspace
CPU tests (84 GPU scenarios ignored). All 84 GPU scenarios actually passed on
each WSL OpenGL, native Windows D3D12 and native Windows Vulkan: 252 passes,
zero failures or explicit skips in the final matrix. Both native depth
executables separately passed their seven scenarios after the oracle correction.
A WSL interop startup110 occurred before the full native run launched GPU work;
a functioning broker then ran the full matrix. Neither startup failure nor the
earlier depth-oracle failure is disguised as a passing or skipped GPU scenario.
Concurrent unfinished runtime changes were excluded and preserved.


## Component blend coefficient execution

Six CPU tests cover all ten supported factors in every coefficient slot under
both literal encodings, shared/separate-alpha state, ignored MIN/MAX factors and
raw-state restoration. Negative tests retain explicit rejection of constants,
saturation, dual-source, malformed methods and nonzero color-key writes. Literal
incrementing packets check successful state prefixes and unread suffix refusal.

Two additional GPU scenarios execute through GpuDriver, mapped GPFIFO and B197
indexed draws. The 48-case matrix tests each new factor in all four source/
destination RGB/alpha slots under both aliases. A separate f64 component equation
oracle uses independently specified source fractions [3/4,1/4,5/8,3/8] and
destination bytes [51,153,204,230], with at most one RGBA8 conversion unit of
tolerance. Color factors are evaluated per channel, including alpha; neither
production factor decoding nor shader output is used to derive expectations.
Equivalent aliases retain the pipeline, and real factor changes retain shader
modules and decoded programs.

A tiled overlap first computes S*S+D*(1-S), then S*destinationAlpha+D*D with
an R/B write mask. Independent rational expectations are [230,131,234,180] in
the overlap, versus [189,131,195,180] in reverse order. Alpha remains masked
during the second draw but its newer value still weights RGB. Guest RAM stays
byte-exactly unchanged after both draws and a later unsupported constant-factor
fault; zero intermediate readbacks/waits and one final boundary are asserted.
Independent GOB addressing verifies untouched padding; no accepted draw replays.

The matrix plus its warm sequence executes 81 draws and 130 queue submissions:
49 explicit validation readbacks/waits, 364560 readback bytes, 295568 guest target
upload/write bytes, 72 vertex upload bytes and 16 constant upload bytes. It uses
one target allocation with 80 reuses, 28 pipelines and four shader modules.
Thirty-two warm S*S overwrite draws add no resource reads/uploads, shaders,
pipelines, readbacks or waits; they still add 32 queue submissions and one final
publication. Representative debug CPU observations were 26.254ms with1606
allocations/129 reallocations on GL, 27.470ms with1441/32 on D3D12, and10.651ms
with1607/34 on Vulkan. Process-wide allocator counts include backend activity;
these are not GPU durations, zero-copy claims or game performance estimates.

A faithful ordinary snapshot of 515c444 plus the exact five implementation/test
files, identical Cargo.lock and dedicated artifacts passed whole-workspace
formatting, strict workspace Clippy, 180 graphics CPU tests in each feature
configuration and 399 workspace CPU tests (86 GPU cases ignored). Its source
files were updated by content after prior validation finished, retaining artifact
separation from the live checkout and preserving unchanged source timestamps.
Concurrent unfinished runtime changes were excluded and preserved.

All 86 GPU scenarios actually passed on each WSL OpenGL, native Windows D3D12
and native Windows Vulkan backend: 258 passes, zero failures or explicit skips.
The focused GL run separately passed five blend GPU scenarios and eleven CPU
tests. Source-grounded independent review covered production mappings and the
math/visibility oracle. No original-game GPU submission or frame is established.


## Register logical complements and canonical PASS_B

Six CPU tests validate fourteen literal GPR forms in both shader stages,
register readiness, in-place writes, RZ, strict scheduling/modifier rejection,
resource/proof erasure and retained FMUL hazards. Existing negative suites now
accept only the independently verified complement bits; CC, predicate results,
X, unsupported operand families and noncanonical PASS_B remain rejected.
Three original assembly fixtures were independently reassembled byte-for-byte.

Two GPU scenarios enter through GpuDriver, guest GPFIFO and indexed B197 draws.
Fourteen forms times 28 input pairs exercise every output bit through a finite
RGBA8 byte observer. The independent oracle evaluates a per-bit Boolean truth
table; it does not evaluate production IR or use GPU output as expectations.
A later unsupported CC form faults with an earlier draw still pending, without
submission, publication or replay; publication yields [135,169,203,237], the
bytes of ~0x12345678. A vertex XOR of ~0x7fffffff mirrors guest positions; an
independent analytic coverage oracle distinguishes the result from a NOP.

The full-word workload executes 393 draws, 786 queue submissions, 393 intentional
validation readbacks/waits, 2923920 readback bytes and 2370576 guest framebuffer
write bytes. It observes fifteen decode attempts (fourteen valid plus one fault),
6960 shader bytes, 30 pipelines, 17 modules, 3144 constant guest bytes and 2864
constant upload bytes, 72 vertex upload bytes and eight index upload bytes.
One target allocation is reused 392 times. Per-case publication is oracle policy,
not an unconditional runtime draw policy.

Thirty-two warm vertex draws add no resource reads/uploads, shader decodes,
modules, pipelines, readbacks or waits; they add 32 queue submissions before
one explicit final publication. Debug CPU observations were 58.220ms with1568
allocations/128 reallocations on isolated GL, 26.403ms with1408/32 on D3D12 and
22.047ms with1568/32 on Vulkan. Counts are process-wide and include backend work;
they are not GPU durations, zero-copy measurements or game benchmarks.

A faithful ordinary snapshot of 267d6d2 plus the exact eleven source/test/fixture
files, unchanged Cargo.lock and a dedicated artifact directory passed workspace
formatting, strict workspace Clippy, 186 graphics CPU tests in each feature
configuration and 405 workspace CPU tests (88 GPU scenarios ignored). All 88
GPU scenarios actually passed on each WSL OpenGL, native Windows D3D12 and
native Windows Vulkan: 264 passes, zero failures or explicit skips in the final
matrix. Native execution used Intel UHD; GL used llvmpipe. Earlier Windows
interop startup110 failures occurred before GPU execution and are not passes.
An initial CPU scheduling test incorrectly expected a ready-register operation
to fault before the first pending UV read; the corrected test asserts the real
AST failure and separately tests a LOP reading that pending register. Production
validation was not weakened. Unfinished peer runtime edits were excluded and
preserved. No original-game GPU command or frame is established.


## Full-width immediate logical execution

Seven CPU tests independently check original LOP32I words, twelve operator/
complement forms in both stages, every immediate bit including48:50, RZ and
high-register accounting, source/destination readiness, conservative scheduling,
strict predicate/modifier rejection, proof erasure and retained FMUL hazards.
Two GPU scenarios enter through GpuDriver, GPFIFO and indexed B197 execution.
Twelve forms times twelve edge immediates times four mutable CB source values
produce576 independently checked full-word results. A per-bit Boolean truth
table specifies expected bytes; finite byte-to-UNORM observation avoids NaN
float-export assumptions. Full-width masks distinguish narrow/sign-extended
decoding, while CB-only changes preserve shader/pipeline identity.

An additional accepted A XOR ~ffffffff draw remains pending before a new CC52
shader faults. The fault adds no draw/submission/publication; explicit publication
yields [120,86,52,18] for12345678 without replay. A separate translated vertex
program XORs clip X with80000000; analytic mirrored coverage distinguishes the
actual raster effect from a NOP. Thirty-two warm repeated draws add no resource
reads/uploads, shader decodes/modules/pipelines, readbacks or waits. They still
require32 queue submissions and one final explicit visibility boundary.

The full-word workload has577draws,1154queues,577intentional oracle readbacks/
waits,4292880readback bytes and3480464guest framebuffer write bytes. It reads
67744shader bytes over146decode attempts (144matrix programs, one explicit
good program and one rejected CC variant), creates148modules/292pipelines,
reads4616constant bytes and uploads2312, plus72vertex and eight index bytes.
A single target allocation has576reuses. Per-case publication is test policy,
not a per-draw runtime requirement. Warm32debug CPU observations were23.516ms
with1569allocations/128reallocations on GL,27.639ms with1408/32 on D3D12 and
32.561ms with1568/32 on Vulkan. These process-wide measurements include backend
activity and are not GPU durations, allocation-free or game-performance claims.

The initial full CPU run exposed process-wide allocator noise in the unchanged
FIFO decoder test: one unexpected allocation. Its separately committed test
instrumentation now gates counters by calling thread, proves local37B inclusion
and concurrent127B exclusion, and retains the actual5000-write zero-allocation
assertions.256isolated executable runs passed before full-suite validation.
No FIFO production behavior or allocation acceptance threshold changed.

A faithful ordinary snapshot of a9449b7 plus exactly nine LOP32I candidate
source/test/fixture files and unchanged Cargo.lock passed workspace formatting,
strict workspace Clippy,193graphics CPU tests in each feature configuration
and412workspace CPU tests (90GPU scenarios ignored). Dedicated artifacts were
kept separate from the live checkout. All90GPU scenarios actually passed on
each WSL OpenGL, native Windows D3D12 and native Windows Vulkan:270passes,
zero failures or explicit skips in the final matrix. Native runs used39test
executables per backend; compilation alone is not counted. Unfinished runtime
changes remained excluded and preserved. Earlier diagnostic-test and wrong-cwd
prototype compile failures were corrected before this final validation and are
not counted as passing GPU executions. No original-game graphics is established.


## Full-width IADD32I execution

Seven CPU tests independently verify the original assembled words, plain and
source-negated forms in both stages, every immediate bit, RZ/in-place/high GPRs,
conservative scheduling, pending-source/destination checks, strict side-effect
rejection, proof erasure and retained FMUL hazards. Two GPU scenarios execute
through GpuDriver, guest GPFIFO and indexed B197 drawing. Two forms times twelve
immediates times 28 edge/deterministic source values produce 672 full-word
observations. An independent signed-i64 calculation with mathematical modulo
2^32 determines all four expected bytes, including carry wrap and source
negation. The oracle does not evaluate production WGSL or reuse GPU output.

Each encoded immediate change invalidates code; 28 mutable CB source values
under that code preserve translation, shader modules and pipelines. A final
accepted A+ffffffff draw remains pending before a newly selected CC52 shader
faults. No extra draw, queue submission or guest publication occurs at the
fault. Explicit publication yields [119,86,52,18] for A=12345678 without replay.
A vertex program adds raw80000000 to clip X modulo2^32, toggling its sign bit;
independent asymmetric mirrored coverage verifies actual vertex execution.

The arithmetic workload has 673 draws, 1346 queue submissions, 673 intentional
oracle readbacks/waits, 5007120 readback bytes and 4059536 guest framebuffer
write bytes. It reads 12064 shader bytes over 26 decode attempts, creates 28
modules and 52 pipelines, reads 5384 constant bytes and uploads 2696, plus 72
vertex and eight index bytes. One target allocation is reused 672 times. These
per-case publications validate individual arithmetic results; they are not an
unconditional runtime requirement.

Thirty-two warm vertex draws add no resource reads/uploads, shader decodes,
modules/pipelines, readbacks or waits. They still add 32 queue submissions
before one final explicit publication. Debug CPU observations were 15.979ms
with 1568 allocations/128 reallocations on GL, 25.132ms with 1420/41 on D3D12
and 13.988ms with 1568/32 on Vulkan. Process-wide allocation counts include
backend activity; these are not GPU durations or game-performance claims.

A faithful ordinary snapshot of 0ad9ea6 plus exactly nine IADD32I candidate
source/test/fixture files, unchanged Cargo.lock and dedicated artifacts passed
workspace formatting, strict workspace Clippy, 200 graphics CPU tests in each
feature configuration and 419 workspace CPU tests (92 GPU scenarios ignored).
All 92 GPU scenarios actually passed on each WSL OpenGL, native Windows D3D12
and native Windows Vulkan: 276 passes, zero failures or explicit skips. Forty
native test executables ran per backend. Unfinished runtime work was excluded
and preserved. Source/production/math reviews and independent fixture assembly
covered the exact candidate; external coordination review remains separate.
No original-game GPU submission or frame is established.


## Signed-20-bit immediate IADD execution

Three additional CPU tests independently pin the assembled words, cross sign56
with source-negation49 in both stages, exercise every low payload bit and its
complement, and verify strict register/scheduling/side-effect/proof rules. The
reference sign extension uses wide signed arithmetic, not production bitwise OR.
Explicit immediate-zero mutations are distinguished from assembler output.

Two additional GPU scenarios enter through GpuDriver, mapped GPFIFO and indexed
B197 draws. Two source-negation forms times nine signed immediates times 28 edge/
deterministic source values yield 504 full-word observations. Independent signed
i64 arithmetic modulo2^32 supplies expected bytes, including -524288 and524287.
Per-code CB mutations retain translations/modules/pipelines. One later accepted
A-1 draw remains pending when CC47 faults: no extra draw, submission or premature
publication occurs, and explicit publication yields [119,86,52,18] without replay.

The vertex fixture adds -524288 to raw clip X. Independently specified inputs
[-0.75,-0.5],[0.5,-0.25],[-0.25,0.75] become
[-0.71875,-0.5],[0.484375,-0.25],[-0.2421875,0.75]. Four safe pixel centers
independently distinguish that geometry from a NOP/missing sign extension.
Thirty-two repeated draws add no shader/vertex/index/CB/texture/descriptor/target
source reads, uploads, modules, pipelines, readbacks or waits; they still add32
queues before one explicit final publication. New assertions cover all resource
classes, not only shader reads.

The arithmetic workload has505draws,1010queues,505intentional oracle readbacks/
waits,3757200readback bytes and3046160guest framebuffer write bytes. It reads
9280shader bytes over20decode attempts, creates22modules/40pipelines, reads
4040constant bytes and uploads2024, plus72vertex/eight index bytes. One target
allocation is reused504times. These synchronizations validate individual
arithmetic results; they are not unconditional per-draw runtime behavior.

A faithful ordinary snapshot of5395cf9 plus exactly nine signed20 candidate
source/test/fixture files and unchanged Cargo.lock passed workspace formatting,
strict workspace Clippy,203graphics CPU tests in each feature configuration and
422workspace CPU tests (94GPU scenarios ignored). Dedicated artifacts remained
separate from the live checkout. All94GPU scenarios actually passed on each WSL
OpenGL, native Windows D3D12 and native Windows Vulkan backend:282passes,zero
failures or explicit skips. Forty native test executables ran per backend. The
final matrix includes the strengthened resource-read assertions. Independent
production/math review and complete fixture reassembly covered exact candidate
hashes. Unfinished runtime work remained excluded and preserved. No original
game GPU command or frame is established.


## Ordered 1024-draw frame workload

`guest_frame_1024_ordered_draws_one_publication` submits one mapped GPFIFO entry
containing 1024 indexed B197 draws. Guest constant-buffer bindings alternate two
quarter-valued colors; RGB SUBTRACT and separate alpha ADD make order observable.
An independent integer recurrence with nearest-even RGBA8 quantization predicts
[0,191,255,255]. It distinguishes stale late-bound constants, reversed order,
framebuffer reinitialization and default blending. Uncovered background and
pre-publication guest bytes are checked independently. This is a targeted
ordering oracle, not a claim that every intermediate image is unique.

The measured delta is 1024 draws and 1024 host queue submissions, 49152 consumed
command bytes, 1025 command borrows and 25198592 borrowed-span bytes. The larger
span count describes suffix reacquisition, not copied or repeatedly decoded
payload bytes. Constants account for 16368 guest-read bytes, 12276 upload bytes
and 1023 upload calls; the first binding is already resident. There are 1024
target reuses and no new vertex/index/texture/descriptor/target/shader reads,
resource uploads other than constants, buffers, textures, bind groups, shader
modules, pipelines or target allocations. No intermediate readback or blocking
wait occurs. One final visibility boundary reads 7440 host bytes and publishes
6032 guest bytes, without replay; its completion token is retired explicitly.

Debug CPU submission observations were 968.805ms on OpenGL, 436.004ms on D3D12
and 368.741ms on Vulkan. Process-wide Rust allocation/reallocation counts were
58362/4096, 54283/1031 and 55321/1028 respectively. These include backend activity;
they establish neither allocation-free rendering, bounded long-term memory
usage, GPU durations nor game frame rates. This remains one queue submit per draw.

A faithful ordinary snapshot of 6bd69cb plus exactly the candidate GPU test file,
unchanged Cargo.lock and dedicated artifacts passed formatting, strict workspace
Clippy, 203 graphics CPU tests in each feature configuration and 422 workspace
CPU tests (95 GPU scenarios ignored). All 95 GPU scenarios actually passed on
WSL OpenGL, native Windows D3D12 and native Windows Vulkan: 285 passes, zero
failures or explicit skips. Forty native executables ran per backend. The final
candidate SHA-256 is ddbc72d583bac75babdbe9bcef681eb0dfef7f30c9824166a6f5023fb43cd8e9.
The native matrix resumed missing executions from preserved same-build logs;
successful runs were not replaced by compilation claims. Separate unfinished
Z16 and runtime edits were excluded and preserved. No original-game GPU command
or Tears of the Kingdom frame is established.

## Native Z16 attachment execution

Two additional CPU tests validate B197 Z16=`0x13`, format transitions and invalid
state, then compare exact two-byte GOB addressing with the independent scalar
oracle across seven asymmetric geometries and all six block heights. Production
uses tegra_swizzle; the oracle does not. Truncated storage and geometry/address
limits fail explicitly. ZF32 and pitch-linear RGBA8 regressions remain covered.

Four additional actual GPU scenarios enter through GpuDriver/GPFIFO/B06F/B197:

- Sixteen comparison/write combinations verify asymmetric textured coverage,
  exact logical LEu16 samples, uncovered pixels and every padding byte. Incoming
  Z=.375 and stored codes8192/24575/40959/57343 avoid ambiguous comparison ties.
- A canonical alias retains depth residency. Thirty-two warm draws add32 queues
  with zero new uploads, attachment allocations, shaders/pipelines, readbacks or
  waits. Separate CPU low/high-byte and padding patches survive failed atomic
  publication and retry. Failure leaves all guest RAM intact; retry checks both
  patched bytes and untouched GPU-produced inside/original outside samples.
- A32x2048 guest surface stores every one of65536 Z16 codes exactly once. A fully
  clipped guest triangle still drives native attachment initialization/export;
  all codes and the unchanged color image survive exact publication. Endpoint
  Z=0/1 then verifies real EQUAL/NOT_EQUAL color visibility independently.
- Pending ZF32-to-Z16-to-ZF32 transitions publish the old attachment before native
  replacement. Immediate RAM assertions prove raw CPU full-patch precedence.
  Guest shaders remain cached; a later unsupported format faults without replay,
  leaving accepted output publishable.

Z16 is native Depth16Unorm during testing/writes. For52x29/BH2, guest footprint is
4096bytes, logical depth3016bytes and portable R32 upload6032bytes. Readback remains
four bytes/sample with host row padding. In the exhaustive65536-code case, initial
portable upload is262144bytes versus131072guest bytes; publication preserves the
exact131072-byte storage. Conversion/patch/readback counters measure these distinct
boundaries. Failed publication may repeat export/readback, but never the draw.
No allocation-free conversion or whole-frame atomic transaction is claimed.
Maxwell halfway depth-conversion equivalence remains unproven.

A faithful ordinary snapshot of7a2cc2a plus the seven Z16 production/test files
and unchanged Cargo.lock passed workspace formatting, strict workspace Clippy,
205graphics CPU tests/configuration and424workspace CPU tests (99GPU ignored).
All99GPU scenarios passed on each WSL OpenGL, native Windows D3D12 and Vulkan:
297unique backend/scenario passes, zero failures or explicit skips. After review
strengthened assertions, the final11-scenario depth executable passed again on
all three, with final package Clippy/formatting; these repeated passes are not
added to the297count. Final depth test SHA-256:
0e12e9ff48341027585505f5de304c5de5224178d024561c2079519c33a9871b.
Native execution used Intel UHD; WSL used llvmpipe. Independent source/test review
resolved two oracle gaps. Unfinished runtime and subsequent shader work remained
excluded and preserved. No original-game GPU submission or frame is established.

## Short-immediate FADD/FMUL execution

Five additional CPU tests pin independently assembled words and raw IEEE payload
reconstruction, including sign/exponent/mantissa boundaries. They cover both
stages, RZ sources, pending destinations, strict modifiers/scheduling and mixed
GPR/immediate contraction hazards. Immediate multiplication taint survives MOV,
LOP.OR-RZ and IADD32I-zero identities. Valid-schedule TEX negatives require handle
provenance rejection after immediate FADD+0/FMUL1. Exceptional bit decoding is
separate from unproven portable exceptional arithmetic equivalence.

Two additional GPU scenarios enter through GpuDriver/GPFIFO and indexed B197
geometry. Independent dyadic arithmetic predicts [64,96,128,255], then mutable
constants produce [128,32,64,255] without module/pipeline recreation. In-place
code changes produce [64,96,64,255]. A later unsupported SAT instruction faults
without stale execution, replay or extra submission; accepted output remains
publishable. The fragment workload has4draws,8queues,3decode attempts (including
the rejected program),528shader bytes read,5modules/6pipelines,48constant bytes
read/uploaded,4intentional oracle readbacks/waits,29760readback bytes and24128guest
publication bytes. One target allocation is reused3times.

The vertex fixture genuinely scales X by.5 and offsets Y by.5; an independent
analytic coverage oracle verifies the rasterized result. Thirty-two unchanged
draws add no resource reads/uploads, shader/pipeline allocations, readbacks or
waits, but still submit32queues. Debug CPU observations were31.345ms on OpenGL,
16.325ms on D3D12 and17.798ms on Vulkan. No allocation-free or GPU-duration claim
is made.

A faithful ordinary snapshot of850e2e2 plus seven exact shader/test/fixture files
and unchanged Cargo.lock passed workspace formatting, strict workspace Clippy,
210graphics CPU tests/configuration and429workspace CPU tests (101GPU ignored).
All101GPU scenarios actually passed on each WSL OpenGL, native Windows D3D12 and
Vulkan:303passes,zero final failures or explicit skips,40native executables/backend.
Linux and Windows used separate exclusive artifact directories for this source.
An earlier shared Windows artifact set linked the old decoder and failed the two
new tests before host shader compilation; it is excluded. The fresh exclusive
executables contain the new lowering and pass. Interop startup failures before
process acquisition are also excluded, never counted as GPU passes. The final
source and fixture hashes match the frozen validation manifest and independent
review/reassembly. Subsequent clear-test and unfinished runtime edits were
excluded and preserved. No original-game GPU command or frame is established.

## Z16 clear, full-storage CPU patches and depth reuse

`z16_clear_full_cpu_patch_retry_and_indexed_depth_test` exercises the existing
B197 CLEAR_SURFACE path through GpuDriver/GPFIFO with native Z16. A52x29/BH2
surface occupies4096guest bytes, including padding. Clear depth.375 publishes
exact LEu16 code24576 at every logical sample while retaining all padding and
unrelated mapped tail bytes. The clear requires zero depth initialization reads,
uploads or layout conversions; command acquisition and native resource creation
still occur. Host completion alone leaves guest RAM unpublished. The initial
explicit depth-only publication performs one readback and returns no color frame.

A second pending clear receives an entire4096-byte canonical CPU patch: checker
depths8192/57343 and padding6d. Injected atomic write failure and successful retry
preserve that exact allocation without repeating either clear. A following indexed
LESS/write draw at Z=.375 must initialize native depth from the CPU checker rather
than reuse the stale clear. It uploads6032R32 bytes for3016logical guest depth bytes.
An independent barycentric/GOB oracle checks all logical color/depth samples,
rejected and uncovered pixels, every padding byte and unrelated storage. Limited
edge ambiguity permits only coherent depth/color pairs and only when all weights
are within the triangle's edge tolerance; clearly outside edge-line extensions
receive strict background/original-depth checks.

The final test SHA-256 is
a2b746a07081adba5ba321c2e4c37aba1913863423f96e633a3f9c4dcdd36f72.
Independent review identified and resolved the edge-extension oracle gap.
A faithful ordinary9dec2eb snapshot plus exactly this test file passed formatting,
strict workspace Clippy,210graphics CPU tests/configuration and429workspace CPU
tests (102GPU scenarios ignored). All7focused clear scenarios passed on each WSL
OpenGL, native Windows D3D12 and Vulkan:21actual passes,zero failures or skips.
Together with the preceding unchanged101-scenario production matrix, the new case
has three-backend coverage; this is not reported as a fresh full102-scenario run.
Exclusive source/artifact provenance was retained. No production API changed,
whole-frame atomicity or allocation-free transfer is claimed. Runtime edits were
excluded and preserved; no original-game GPU submission is established.

## Full-width floating immediates

FADD32I and FMUL32I preserve all 32 immediate payload bits, including the low
mantissa bits absent from short-immediate instructions. Independently assembled
fragment and vertex fixtures have SHA-256
`88472bdac976fb58f5144a9284ebb74e027af272425ee0f6a418e050fb343676` and
`58f41ea86fd831e1c36a52ef501436384276d41899e09adea7d674f5445f90f2`.
CPU tests exercise every payload bit, strict modifiers, predicates, readiness,
register bounds and multiplication-to-addition contraction hazards across all
three supported operand forms. The IADD32I opcode-bit-57 test now verifies the
distinct valid FMUL32I instruction instead of incorrectly expecting rejection.

The fragment fixture uses guest constant operands and compares raw arithmetic
results against independently supplied expected bits. For immediate `3f800001`,
adding -1 yields `34000000`, multiplying 1 yields `3f800001`, and multiplying 2
yields `40000001`. Correct answers produce yellow `[255,255,0,255]`; deliberately
truncated answers produce black. Mutable constants preserve shader/pipeline
identity. Changing only the addition immediate to `3f800002` produces green
`[0,255,0,255]`. An unsupported subsequent CC modifier faults without replay,
stale execution or an extra host submission; earlier output remains publishable.
The vertex fixture changes actual indexed geometry using `3f000001`; independent
barycentric coverage checks demonstrate the transform. Its one-ULP difference is
below pixel resolution, so low-bit fidelity is established by the raw fragment
oracle and decoder tests rather than by that coverage result.

The fragment workload records 5 draws, 10 queue submissions, 3 decode attempts
(including rejected code), 720 shader bytes, 5 shader modules, 6 pipelines,
80 constant guest bytes and 72 constant upload bytes. Five intentional oracle
publications perform 5 readbacks/waits, 37,200 readback bytes and 30,160 guest
write bytes. One target allocation is reused four times. Thirty-two unchanged
vertex draws add no resource reads/uploads, shader modules, pipelines, readbacks
or waits, but still submit 32 queues. Debug CPU observations were 41.597 ms on
OpenGL, 26.326 ms on D3D12 and 25.252 ms on Vulkan. This is not GPU timing or a
game performance estimate; allocator traffic was not separately measured here.

A faithful ordinary snapshot of `31e4c4d` plus the exact eight changed Rust and
fixture files, with unchanged Cargo.lock, passed workspace formatting, strict
workspace Clippy, 213 graphics CPU tests per feature configuration and 432
workspace CPU tests (104 GPU scenarios ignored). All 104 GPU scenarios actually
passed on each WSL OpenGL, native Windows D3D12 and native Windows Vulkan:
312 passes, zero failures or explicit skips, 40 native executables per backend.
Exclusive artifact directories and final source hashes matched the validation
manifest. Unfinished runtime files were excluded and preserved. Exceptional
floating-point equivalence and portable fused multiply-add remain unproven; no
general predication, general shader coverage or original-game graphics is claimed.

## P0/!P0 conditional MOV execution

Eight CPU tests cover independently assembled MOV predicates in both stages,
false-path destination readiness, RZ sources, self moves, strict full-lane masks,
P1-P6/!PT rejection, preserved unconditional PT, real barrier waits, register
accounting, multiplication hazards from both values and predicate producers,
and texture-handle/position-W proof erasure. Original envyas reassembly matches
both committed instruction streams exactly. The supported static readiness rule
is deliberately stricter than hardware's false-predicate operand suppression.

Two actual GpuDriver/GPFIFO indexed scenes exercise the translation. Unsigned
runtime predicates select `[255,128,191,255]` or `[64,128,0,255]`; equality and
high-bit pairs distinguish signed or floating comparisons. False MOVs retain
their original quarter/three-quarter values. Constant changes preserve shader
and pipeline identity. Inverting the first MOV in guest code yields
`[64,128,191,255]`; a later unsupported P1 faults without stale execution,
additional draw/submission or replay, while accepted output remains publishable.
The vertex fixture conditionally replaces only the first X with .5. Changing
its guest X to -.6 changes the geometry under an independent barycentric oracle,
uploads exactly72vertex bytes and preserves shader/pipeline identity.

The fragment workload records7draws,14queues,3decode attempts (one rejected),
624shader bytes,5modules/6pipelines,56constant read/upload bytes,7intentional
oracle readbacks/waits,52080readback bytes and42224guest write bytes. One target
is reused6times. Thirty-two unchanged vertex draws each report one accepted
draw, add32queues and no resource reads/uploads, decodes, modules, pipelines,
readbacks or waits. Debug CPU observations were28.699ms onOpenGL,50.539ms on
D3D12 and13.842ms onVulkan; these are not GPU timings or game benchmarks.
Allocator traffic was not separately measured for this feature.

A faithful ordinary6817c28 snapshot plus exactly seven candidate source/fixture
files and unchanged Cargo.lock passed formatting, strict workspace Clippy,
221graphics CPU tests per configuration and440workspace CPU tests (106GPU
scenarios ignored). All106GPU scenarios actually passed on each WSL OpenGL,
native Windows D3D12 and native Windows Vulkan:318passes,zero failures/skips,
40native executables per backend. Final source hashes matched the frozen
manifest. An initial checkout-scoped focused invocation is excluded; affected
GPU artifacts were removed before rebuilding the final frozen source. The
runtime deliveries through a209972 are preserved in shared main but excluded from
this snapshot's validation scope. Commit45e6254 contains the exact tested GPU
source/fixture hashes. No original-game GPU submission is established.

## Ordered floating predicates: IEEE edge cases

Seven CPU tests validate the six ordered GPR FSETP encodings, both shader
stages, all sixteen mode classifications, strict predicate/boolean/modifier
fields, thirteen-cycle scheduling, real waits, RZ sources, predicate/GPR7
metadata separation, multiplication hazards and preserved unrelated resource
proofs. Both fixture code sections independently reassemble exactly with the
original pinned envyas. The integer-key lowering is independent of the numeric
binary64 oracle used for expected pixels.

The fragment GpuDriver/GPFIFO fixture executes all six relations for 67 input
pairs: signed zeros, positive/negative subnormal boundaries, infinities, quiet
and signaling NaN payloads of both signs, and deterministic general bit patterns.
Exact dyadic weights encode all six answers into red. Less/equal/greater pairs
produce `[84,64,191,255]`, `[151,64,191,255]`, `[223,64,191,255]`; every pair
containing NaN produces `[0,64,191,255]`. Mutable guest constants preserve shader
and pipeline identity. Changing only the first LT to GT yields
`[80,64,191,255]` for -1 versus 1. A subsequent unsupported FTZ faults without
stale execution or replay, leaving accepted output publishable. The vertex
fixture compares guest X against -.5; a guest X mutation changes geometry
without recompilation and uploads exactly 72 vertex bytes.

The fragment workload records 68 draws, 136 queue submissions, 3 decode attempts
(one rejected), 1,296 shader bytes, 5 modules, 6 pipelines, 544 constant guest
bytes and 492 constant upload bytes. Sixty-eight intentional oracle publications
perform 68 readbacks/waits, 505,920 readback bytes and 410,176 guest write bytes.
One target allocation is reused 67 times. Thirty-two unchanged vertex draws add
32 queues with no extra resource reads/uploads, decodes, modules, pipelines,
readbacks or waits. The GL debug CPU observation was 41.702 ms; it is not GPU
timing or a game benchmark. Allocator traffic was not separately measured here.

A faithful ordinary `e271fb5` snapshot plus seven exact candidate Rust/fixture
files and unchanged Cargo.lock passed GPU formatting, strict GPU and workspace
Clippy, 228 graphics CPU tests per configuration and all 108 actual WSL OpenGL
GPU scenarios, with zero GPU failures/skips. Final hashes matched the frozen
manifest. The full workspace initially lacked the deliberately untracked ExeFS
fixtures. Rebuilding all three from committed C with installed devkitA64/libnx
and running with `--no-fail-fast` produced 452 passes, 3 failures and 108 ignored
GPU scenarios. The remaining Horizon/CLI expectations use the former heap ratio
or expect now-implemented time:u to be absent. Workspace formatting also differs
in runtime/CLI files. Those files and test expectations were not changed for this
GPU delivery; the workspace is not reported green.

All 40 native Windows GPU executables compiled from the same frozen source.
Native execution remained unavailable: both existing WSL interop brokers and
automatic broker selection failed before launching Windows programs with
`UtilAcceptVsock:273: accept4 failed 110`, including a bounded `cmd.exe /c ver`
startup check. Consequently this feature has no new D3D12/Vulkan execution
result yet; compilation and historical backend results are not counted as native
passes. The earlier conditional-MOV matrix remains independently verified.
No original-game GPU command or frame is established by these synthetic tests.

## External CPU write notification

`driver_gpu` exercises `GpuDriver::notify_cpu_write` through real GPFIFO indexed
draws. An externally modified Tegra texture is invalidated by canonical byte
range without recopying RAM through `write_backing`. Covered pixels change to
`[77,137,199,255]`, uncovered background remains intact, and shader/pipeline
identities are reused. The 6,144-byte guest texture footprint causes one bounded
2,812-byte host texture update. Three accepted notifications cover 6,812 bytes;
three actual draws submit six queues and perform three explicit oracle
publications. A rejected counter-overflow notification overlapping cached shader
code leaves the next actual draw free of extra shader reads or resource uploads.

Foreign/stale backing identities, zero lengths, end bounds, address overflow,
pending output and notification-counter overflow are rejected. A synthetic
registered 2^63-byte range owns no RAM and makes byte-counter exhaustion directly
testable. In a separate serial allocator measurement, 1,000 four-byte
notifications used zero Rust allocations/reallocations and no GPU submissions,
uploads or readbacks; the final GL debug CPU observation was 557.872 microseconds.
This is bookkeeping measurement, not a zero-copy GPU or game-performance claim.

A faithful ordinary `ebeb288` snapshot plus the two candidate Rust files and
unchanged Cargo.lock passed GPU formatting, strict package Clippy, 228 graphics
CPU tests per feature configuration and all 110 actual WSL OpenGL GPU scenarios.
The final strengthened footprint, draw-count and notification-total assertions
also passed all five `driver_gpu` scenarios. Full workspace execution produced
454 passes, three Horizon/CLI expectation failures and 110 ignored GPU tests;
workspace Clippy reports `manual_range_patterns` in runtime `services.rs`.
These peer-owned failures were preserved and reported, not hidden by changes to
the test expectations. The snapshot excludes the later display-layer commit.

All 40 native Windows GPU executables compiled, but the existing WSL interop
failure still prevents Windows process startup. No new D3D12/Vulkan execution
passes are claimed. The notifier is a serialized runtime integration contract;
it neither installs automatic AOT dirty tracking nor connects Horizon NVDRV to
the renderer, and does not equate host completion with guest publication.

## All sixteen floating predicate conditions

The 848-byte `float-predicate-all` fixture independently reassembles exactly
with original pinned envyas. All sixteen conditions execute through indexed
GpuDriver/GPFIFO draws for 67 IEEE operand pairs. A separate numeric binary64
oracle predicts the four RGBA truth-mask patterns: numeric less
`[159,159,159,159]`, equal `[191,191,191,191]`, greater `[0,239,0,239]`, and
either NaN `[0,0,239,239]`. Signed zeros, both signs of subnormals, infinities,
quiet/signaling NaNs and seeded arbitrary payloads are covered. Guest constants
change without shader/pipeline replacement. Changing only F to T in code gives
`[175,159,159,159]` for -1 versus 1 and triggers exactly one decode.

Thirty-two unchanged deferred draws add 32 queue submissions and zero extra
shader/resource reads, uploads, modules, pipelines, readbacks or waits. The final
GL debug CPU observation is 17.857 ms; allocator traffic is not separately
measured. Before that warm sequence the test performs 68 intentional oracle
publications: 505,920 readback bytes and 410,176 guest write bytes, with 1,696
fragment program bytes read across two decodes, 544 constant guest bytes and
492 constant upload bytes. Final publication remains explicit. Completion
records are retained through publication, rather than retired at queue acceptance.

The separate vertex test changes NAN's comparison constant without exporting
NaN: true moves all vertex X coordinates to finite .5, producing a zero-area
primitive and untouched background. Numeric subnormal/negative-zero constants
retain the original asymmetric red triangle. Five real draws, five vertex code
decodes and one fragment decode are observed; vertex uploads stay at 72 bytes.
All sixteen vertex conditions also pass decoding and Naga validation. Existing
predicate scheduling, operand readiness, R7 metadata, provenance and strict
modifier rejection regressions remain covered.

A faithful ordinary `688bfcc` snapshot plus five exact candidate source/fixture
files, unchanged Cargo.lock and source-built homebrew fixtures passed GPU
formatting, strict package/workspace Clippy, 230 graphics CPU tests per feature
configuration and all 112 actual WSL OpenGL GPU scenarios, with no GPU failures
or skips. Workspace execution produced 457 passes, three existing Horizon/CLI
expectation failures and 112 ignored GPU tests. These expectations still use the
old heap ratio or treat the now-present time:u service as absent. Concurrent
runtime diagnostics were excluded; the live worktree is not reported validated.

All 40 Windows GPU executables compiled from the same frozen source. A bounded
Windows startup attempt still failed at WSL interop `UtilAcceptVsock:273:
accept4 failed 110`, before launching a Windows GPU test. No new D3D12/Vulkan
passes are claimed. This remains independently assembled synthetic Maxwell
execution; no original-game GPU submission or frame is established.

## Signed and unsigned immediate bitfields

Original pinned envyas independently reassembles both complete fixtures exactly:
336-byte PS and 144-byte VS. Six CPU tests cover every contained offset/width
combination, full-width and zero-width cases, RZ, aliasing, register allocation,
pending source/destination, scheduling, unsupported forms and preserved FMUL
hazards. Full-width extraction intentionally clears TEX, PositionW and reciprocal
proofs; negative tests establish that the rejection follows a valid wait, rather
than an unrelated unready operand. A separate per-bit oracle checks the emitted
contained-field arithmetic across 44,800 scalar cases.

The GpuDriver/GPFIFO PS test checks 41 raw source words against independent
per-bit signed/unsigned reference values, producing `[64,128,191,255]`. Flipping
one reference bit changes only red to zero without recompilation, ruling out an
unconditional pass color. Changing the first BFE to zero width changes guest
semantics and recompiles exactly once. Thirty-two warm deferred draws add 32
queues and no extra shader/resource reads, uploads, modules, pipelines, readbacks
or waits. A subsequent unsupported BREV faults without another draw or queue;
the previously accepted output remains publishable.

Before the warm sequence, 43 deliberate oracle publications transfer 319,920
readback bytes and 259,376 guest framebuffer bytes. Two fragment decodes read
672 bytes, while constants account for 860 guest bytes and 788 upload bytes.
One target allocation is reused 42 times. The warm GL debug CPU observation is
39.403 ms; allocator traffic is not separately measured and this is not GPU
timing or a game benchmark. Final guest visibility remains explicit.

The vertex fixture clears guest X's sign bit and produces an asymmetric triangle
with 25 independently calculated interior validation samples. Changing only the
first guest X yields 47 interior samples, one bounded 72-byte vertex update and
no shader/pipeline replacement. Both draws use actual indexed guest geometry.

A faithful ordinary `d7104db` snapshot plus seven exact candidate Rust/fixture
files and unchanged Cargo.lock passed GPU formatting, strict package/workspace
Clippy, 236 graphics CPU tests per configuration and all 114 actual WSL OpenGL
GPU scenarios, with no GPU failures or skips. Full workspace execution produced
463 passes, the same three Horizon/CLI expectation failures and 114 ignored GPU
tests; the workspace is not reported green. All 40 Windows GPU executables
compiled; the unchanged WSL interop failure prevents native process startup,
so no new D3D12/Vulkan execution passes are claimed. Concurrent runtime changes
were excluded, and synthetic shader results do not establish original-game
GPU execution.

## Three-source ternary logic

Original pinned envyas independently reassembles the 240-byte fragment and
176-byte vertex fixtures exactly. All 256 LUT encodings and the three fixture
LOP3 words also match the separately pinned MaxAs assembler. Seven CPU tests
cover all truth tables, destination aliasing with each source, RZ, high third
register bits, register allocation, readiness and strict modifier rejection.
Constant and identity tables retain all source readiness and FMUL hazards, and
clear texture, PositionW and reciprocal proofs. The discarded PT output does
not initialize or overwrite P0. An independent per-bit oracle additionally
checks 19,712 scalar truth-table cases.

The indexed GpuDriver/GPFIFO fragment workload executes every LUT. The patterns
F0F0F0F0, CCCCCCCC and AAAAAAAA enumerate every three-bit input combination,
so the independently expected result is the LUT byte repeated four times.
Mutable guest constants, 36 boundary/seeded input triples, source aliases and
a deliberately wrong reference verify actual instruction dataflow. Correct
results produce [64,128,191,255]; the incorrect reference changes red to zero
without shader recompilation. A second LOP3 preserves the comparison predicate
used by SEL. Unsupported X faults without another draw or queue submission,
while earlier accepted output remains explicitly publishable.

Before the warm sequence, 300 draws account for 600 queue submissions, 264
fragment decodes reading 63,360 bytes, 267 shader modules and 530 pipelines.
The 300 intentional oracle publications transfer 2,232,000 readback bytes and
1,809,600 guest framebuffer bytes. Constant reads total 4,800 bytes and uploads
4,780 bytes. One target allocation is reused 299 times. Thirty-two unchanged
deferred draws add 32 queue submissions and no shader/resource reads, uploads,
modules, pipelines, readbacks or waits. Final publication remains explicit.
The final GL debug CPU observations were 3.467 seconds for all 256 tables and
22.364 ms for 32 warm draws. An earlier focused run observed 17.138 seconds
and 29.383 ms respectively; driver/disk cache state was uncontrolled. These
are neither strictly cold compilation timings nor GPU/game performance. Rust
allocator traffic was not separately instrumented.

The vertex fixture performs guest-bitwise absolute X through LUT C0 with RZ,
producing independently calculated asymmetric coverage. Changing the first
guest X changes coverage with one bounded 72-byte vertex update and no shader
or pipeline replacement. Both draws use actual guest indexed geometry.

A faithful ordinary 6776b02 snapshot plus seven exact candidate Rust/fixture
files and unchanged Cargo.lock passed GPU formatting, strict package/workspace
Clippy, 243 graphics CPU tests per configuration and all 116 actual WSL OpenGL
GPU scenarios, with zero GPU failures or skips. Workspace execution produced
470 passes, the same three Horizon/CLI expectation failures and 116 ignored GPU
tests. Concurrent runtime diagnostics were excluded; the live workspace is not
reported green. All 40 native Windows GPU executables compiled, but the WSL
interop startup failure still prevents actual D3D12/Vulkan execution. These
results establish synthetic Maxwell execution, not original-game GPU commands
or a rendered game frame.

## Scoreboarded population count

Both 208-byte fixtures exactly reassemble using original pinned envyas; ordinary
POPC words also match independently pinned MaxAs. Seven CPU tests cover GPR/RZ,
inversion, high register metadata, destination/source aliases, explicit waits,
barrier reuse, pending destination overwrite, unready exports, unsupported modes,
predicate initialization and conservative FMUL/resource proofs.

The GpuDriver/GPFIFO fragment workload tests 129 raw words for each plain/inverted
form: every count from zero through 32, every single-bit/complement pattern and
32 seeded words. A separate scalar sum predicts the exact expected count.
ISETP/SEL yields [64,128,191,255] when the guest result matches; deliberately
wrong reference data changes red to zero without recompilation. Both RZ and
inverted RZ additionally execute an asserted draw and produce counts zero/32.
The original focused VS test exposed an invalid test method before any draw;
its corrected hardware program selection now verifies actual vertex execution.

Before the warm sequence, 262 draws account for 524 queue submissions, five
fragment decodes reading 1,040 bytes, 2,096 constant guest bytes and 2,032
constant upload bytes. The 262 deliberate oracle publications transfer
1,949,280 readback bytes and 1,580,384 guest framebuffer bytes. One target is
allocated and reused 261 times. Thirty-two unchanged deferred draws add 32
queues, with no shader/resource reads, uploads, modules, pipelines, readbacks
or waits. Removing only the consumer wait faults without another draw/queue;
previously accepted output remains explicitly publishable. Final full-suite GL
debug CPU observation for the warm draws was 27.822 ms; a separately strengthened
focused run observed 41.701 ms. Allocator traffic and GPU time were not measured,
and cache/driver state was uncontrolled.

The vertex program's independent raw-bit counts select only the first vertex:
-.8 has 19 set bits and moves to finite X=.5; changing guest X to -.6 gives
18 and leaves X=-.6. Analytic asymmetric triangle coverage verifies both actual
indexed draws. Only a bounded 72-byte vertex update is needed; shader/pipeline
identity remains unchanged.

A faithful ordinary 4452b15 snapshot plus seven candidate Rust/fixture files
and unchanged Cargo.lock passed GPU formatting, strict package/workspace
Clippy, 250 CPU tests per configuration and all 118 actual WSL OpenGL GPU
scenarios, zero failures/skips. Workspace execution produced 477 passes, the
same three Horizon/CLI expectation failures and 118 ignored GPU tests. After
adding explicit draw-count assertions to both RZ cases, both focused GPU
scenarios passed again and package Clippy was rerun. Concurrent runtime/NSO
changes were excluded; the live workspace is not reported green. Native Windows
GPU test executables compiled. A native cmd.exe version query succeeded, but
subsequent PowerShell-suite and direct D3D12 starts failed at WSL interop
UtilAcceptVsock/accept4 before GPU execution. No new D3D12/Vulkan passes are
claimed. No original-game submission or frame is established by these synthetic
programs.

## Non-SH signed and unsigned highest-bit search

All three 208-byte fixtures independently reassemble exactly with original
pinned envyas; unsigned forms also match pinned MaxAs. Seven CPU tests verify
strict signed-bit encoding, RZ, aliases/high registers, explicit waits, barrier
reuse across variable-latency instructions, pending overwrite/export rejection,
FMUL hazards, unchanged P0 and invalidation of resource/perspective proofs. SH,
inversion, CC, predicates and other source forms remain explicitly unsupported.

The GpuDriver/GPFIFO fragment workload executes 132 source words in each signed
and unsigned mode, including every bit position, power/complement boundaries,
zero, all ones and 32 seeded words. An independent per-bit scalar scan predicts
the highest qualifying bit or raw FFFFFFFF. ISETP/SEL compares raw results and
exports [64,128,191,255]; deliberately wrong reference data produces red zero
without recompilation. Both RZ modes assert an actual draw and compare against
FFFFFFFF. Sentinel validation does not depend on rendering a NaN color.

Before the warm sequence, 268 draws account for 536 queue submissions, five
fragment decodes reading 1,040 bytes, 2,144 constant guest bytes and 1,792
constant upload bytes. Eight shader modules and twelve pipelines are created
over the deliberately changed programs. The 268 oracle publications transfer
1,993,920 readback bytes and 1,616,576 guest framebuffer bytes; one target is
allocated and reused 267 times. Thirty-two unchanged deferred draws add 32
queues and zero extra resource reads/uploads, modules, pipelines, readbacks or
waits. Removing only the consumer wait faults without another draw or queue;
prior accepted output remains publishable. Final visibility is explicit.

The vertex program compares signed FLO against 29, selecting only initial
second-vertex X=+.6 and moving it to finite +.5. Changing that guest X to-.6
changes its highest non-sign bit to 30 and leaves X=-.6. Independent analytic
coverage changes from 259 to 25 interior samples, with uncovered/background
pixels also checked. Only 72 extra vertex upload bytes are needed and shader
and pipeline identity remains unchanged.

An initial default-suite run reported two process-wide allocations in the old
Maxwell allocation test. The same unchanged workload independently passed 20
no-harness trials, each 6000 method writes and zero allocations/reallocations.
Installed libtest source confirms concurrent harness-thread allocation paths
even with RUST_TEST_THREADS=1; the exact two allocation stacks were not captured.
The established calibrated per-thread allocator was extracted and reused by
the three allocation tests without changing workloads or zero thresholds. Its
zero-sized guard cannot migrate threads. Focused tests passed both feature
configurations before that measurement-scope correction was delivered.

The final faithful ordinary 3bd688b snapshot plus nine candidate Rust/fixture
files and unchanged Cargo.lock passed GPU formatting, strict package/workspace
Clippy, 257 CPU tests per configuration and all 120 actual WSL OpenGL GPU
scenarios, zero failures/skips. Workspace execution produced 484 passes, the
same three Horizon/CLI expectation failures and 120 ignored GPU tests; concurrent
runtime/NSO work was excluded, and the live workspace is not reported green.
The final GL debug CPU observation for 32 warm
draws was 209.882 ms, versus 30.855 ms in an earlier focused run. CPU load, memory
pressure and driver/cache state were uncontrolled; no GPU timing, allocator
traffic or game performance is inferred from these observations.

Forty native Windows GPU executables compiled from the same FLO production,
GPU test and fixture bytes. The allocation-scope fix affects separate CPU
test binaries. A nested cmd/PowerShell launch was additionally attempted, but
WSL interop again failed at UtilAcceptVsock/accept4 before GPU startup; no new
D3D12/Vulkan execution passes are claimed. These are synthetic independently
assembled programs, not original-game GPU commands or frames.

## I2F RN32: integer conversion through hardware-selected shaders

Original-envyas unsigned/signed fragment programs and a signed vertex program
execute I2F through `GpuDriver`, guest GPFIFO entries, B06F/B197 and real wgpu
rasterization. The fragment checks 208 raw words per signed mode, including
power boundaries, even/odd half-ULP ties, significand carry, negative ties,
INT_MIN/INT_MAX, UINT_MAX and a deterministic xorshift corpus. An independent
binary32-neighbor search using exact f64 distances supplies expected raw bits;
it does not reproduce the production integer-packing algorithm. Each result
is compared by guest ISETP/SEL before outputting [64,128,191,255]. Deliberately
incorrect reference data produces [0,128,191,255]. Both RZ modes explicitly
produce positive zero despite a nonzero guest source buffer.

The vertex program converts raw position-X bits only for an integer predicate;
converted large values are never clip positions. Only the first initial vertex
moves to finite X=.5. Mutating that guest X from-.8 to-.6 disables the move.
Independent analytic coverage checks distinguish the two triangles; the vertex
resource uploads 72 additional bytes, with no new vertex decode or pipeline.
Missing the fragment consumer's wait faults before another draw/queue submission,
while all earlier deferred output remains publishable without replay.

Executed checks for a faithful ordinary source snapshot of `17cec29` plus the
ten candidate Rust/WGSL/assembly/binary files, preserving Cargo.lock and the
previous source-built homebrew fixtures:

- `cargo fmt -p wl-gpu --check`: passed.
- Strict package and workspace Clippy: passed.
- GPU tests without default features: 265 passed, zero failures.
- GPU tests with default features: 266 passed, zero failures, 122 GPU tests ignored.
- Explicit WSL OpenGL run: 122 passed, zero failures or skips, llvmpipe LLVM23.1.1,
  Mesa26.2.4. The new fragment and vertex scenarios are included.
- `cargo test --workspace --locked --no-fail-fast`: 493 passed, three known
  CLI/Horizon failures, 122 ignored GPU tests. These are the same preexisting
  heap-ratio and timeout expectations; the snapshot does not include unfinished
  runtime/AOT changes.
- Whole-workspace formatting reports existing runtime/CLI formatting differences;
  those files are not reformatted by this graphics change.

Forty Windows GPU test executables compiled from the identical candidate bytes.
Native execution was attempted rather than inferred: the PowerShell file launch
was blocked by the host's script-signing policy; direct executable launch and
launch through cmd, including an exact temporary copy on the Windows filesystem,
failed in the WSL interoperability broker with `accept4` error110 before GPU
adapter acquisition. No execution policy was changed, no shared WSL restart
was attempted, and no D3D12/Vulkan pass is claimed for this candidate.

Before the warm loop, the controlled fragment workload records 420 draws,
840 queue submissions, five fragment decodes/1040 shader bytes, 3360 constant
bytes read/2980 uploaded, eight shader modules, twelve pipelines, one target
allocation and 419 reuses. Its 420 intentional oracle publications transfer
3,124,800 readback bytes and 2,533,440 guest framebuffer bytes. The 32 warm draws
add 32 queue submissions and no resource reads/uploads, shader modules,
pipelines, target allocations, readbacks or waits. This is ordered deferred
execution, not host draw batching or zero-copy rendering. Command acquisition
still borrows 1280 additional bytes, including 1024 command bytes; its entry
decode counter increases by64. Warm CPU wall time
was 17.092ms in the full run versus 44.321ms in an earlier focused run with four
fewer corpus words; CPU load, compiler/cache state and debug instrumentation
were uncontrolled. Neither value is GPU time or game-frame performance. Rust
allocator traffic is not instrumented in this new draw workload.

Separately, the identical integer-only WGSL helper passed a private real OpenGL
kernel probe for 20,792 inputs/41,584 unsigned-plus-signed results against an
exact rational adjacent-float oracle. It used 249,504 upload bytes, 166,336
readback bytes, one dispatch/submission and one completion wait. That isolated
probe is mathematical/backend corroboration; it does not imply guest compute
shader support. The delivered acceptance path is the Maxwell graphics path
above. Only independently assembled synthetic programs are used; no original
TOTK GPU stream or frame has been demonstrated.

## GPR FSET: exact comparison results and predicate isolation

Guest-selected FSET programs execute through `GpuDriver`, GPFIFO, B06F/B197
and wgpu. All32 comparison/result combinations are exercised with67 independently
specified IEEE pairs each: 2144 raw-result checks covering ordered/unordered
relations, classification, F/T, signed zeros, subnormals, infinities and multiple
signaling/quiet NaN payloads. The oracle uses numeric binary64 relations, which
embed binary32 exactly, independently of production's integer-key comparison.
The guest shader compares each mask/float encoding using ISETP before exporting
[64,128,191,255]. Wrong reference bits produce [0,128,191,255]. A second FSET.F
between ISETP and SEL must preserve P0, directly exposing accidental predicate
clobber. Right-destination aliasing and both poisoned-source RZ modes are also
verified through real draws.

The vertex fixture moves only X=-.8 to finite+.5 using FSET.BF.LT against-.5.
Changing guest X to-.4 changes analytic triangle coverage, uploads72 additional
vertex bytes and preserves the compiled vertex shader/pipeline. Changing the
binary to mask output plus its expected MOV32I value restores equivalent
coverage: exactly one new vertex decode/pipeline and no fragment decode.
Both result modes are observed as raw GPR bits, never exported NaN masks.
Unsupported CC faults without another draw/queue submission; earlier deferred
output is explicitly published without replay.

The faithful ordinary source snapshot is `16d0234` plus nine candidate
Rust/assembly/binary files, unchanged Cargo.lock and preserved source-built
homebrew fixtures. It includes the newer committed runtime/presentation changes,
unlike the preceding I2F snapshot. Executed graphics gates:

- GPU formatting and strict all-target package Clippy: passed.
- Without default features: 272 CPU tests passed, zero failures.
- Default features: 273 CPU tests passed, zero failures;124 GPU tests ignored.
- Explicit WSL OpenGL:124 real GPU scenarios passed, zero failures/skips,
  llvmpipe LLVM23.1.1/Mesa26.2.4, including both new FSET scenarios.
- Whole-workspace Clippy fails five `manual_range_contains` diagnostics in the
  newly committed runtime `lib.rs`; whole-workspace formatting reports changes
  outside the GPU package. Graphics code is not modified to hide these failures.

Forty native Windows GPU executables compiled from identical candidate bytes.
D3D12 and Vulkan launches were attempted independently through cmd; both fail
in the WSL broker with `accept4` error110 before adapter acquisition. Neither
backend is counted as a pass, skip or shader-validation result. Host policies
and shared WSL processes are unchanged.

Before warm repetition, the controlled fragment workload records2150 draws,
4300 queue submissions,37 fragment decodes/7696 shader bytes,25800 constant
bytes read/17340 uploaded,40 shader modules,76 pipelines and one target
allocation/2149 reuses. Its2150 intentional oracle publications read15,996,000
bytes, including row padding and timestamp records, and publish12,968,800
packed guest bytes. The32 warm draws add32 queues,1280 borrowed command-source
bytes including1024 command bytes, and64 entry decodes; no additional resource
reads/uploads, target allocations, modules, pipelines, readbacks or waits occur.
CPU wall time was43.607ms in the full run versus25.351ms focused; host load and
cache/compiler state were uncontrolled. These are debug CPU observations,
not GPU times, game performance, full batching or zero-copy claims. Rust
allocator traffic is not instrumented in this workload. The independently
assembled programs are synthetic; no original TOTK GPU stream/frame is implied.

The completed FSET workspace snapshot run reports507 passed, three existing
CLI/Horizon failures and124 ignored GPU tests. This is the `16d0234` plus
nine-file snapshot described above, not a live-worktree workspace pass.

## ISCADD: five-bit shifts, wrapping addition and retry-safe publication

The indexed guest-shader workload enters through `GpuDriver`, GPFIFO and
B06F/B197. Forty-eight raw operand pairs, including32 deterministic seeded
pairs, exercise all32 shifts:1536 arithmetic results. The independent oracle
multiplies in u64 by2^shift, adds the second operand, and truncates to32 bits.
The guest shader checks raw results with ISETP before exporting
[64,128,191,255]; a wrong reference produces[0,128,191,255]. Additional real
draws exercise right-destination aliasing and poisoned-source RZ combinations.
The independent auxiliary ISCADD must preserve P0.

The vertex fixture changes asymmetric analytic coverage through the decoded
arithmetic. Guest X mutation uploads72 additional vertex bytes without shader
retranslation. Subsequent instruction/expected-immediate mutation causes one
new vertex decode/pipeline without another vertex upload or fragment decode.
Coverage is checked analytically with a47x29 viewport on a52x29 target,
including covered and uncovered pixels; this fixture does not clip a primitive.

For failure injection, guest RAM first holds red0 while accepted pending GPU
work produces red64. Thirty-two warm draws leave RAM unchanged. Unsupported
CC faults without appending a draw or queue submission. An injected atomic
write failure leaves the red0 RAM image and visibility tokens unchanged;
retry publishes red64 without replaying any draw. Readback may repeat after
failed publication; this test does not claim readback caching.

Executed live-worktree graphics gates use the identical candidate source:

- GPU formatting and strict all-target package Clippy: passed.
- Without default features:279 CPU tests passed, zero failures.
- Default features:280 CPU tests passed, zero failures;126 GPU tests ignored.
- Explicit WSL OpenGL:126 actual GPU scenarios passed, zero failures/skips,
  llvmpipe LLVM23.1.1/Mesa26.2.4, including both new ISCADD scenarios.

These Linux builds disable debug information to reduce repeated link costs while
retaining unoptimized code, overflow checks and debug assertions. The faithful
workspace/native source snapshot is `2b7d93c` plus seven exact candidate code,
assembly and binary files, unchanged Cargo.lock and preserved source-built
homebrew fixtures. Whole-workspace formatting reports peer-owned differences; strict workspace
Clippy reports six runtime diagnostics (five range comparisons and identical
branches), including the newer committed `d0e0394` change. Graphics-only
results do not imply whole-workspace success.

Before warm repetition the fragment workload records1558 draws,3115 queue
submissions,45 fragment decodes/9360 shader bytes,18696 constant bytes read and
17616 uploaded,48 shader modules,92 pipelines and one target allocation with
1557 reuses. The1557 intentional oracle publications read11,584,080 bytes and
publish9,391,824 packed guest bytes. The32 warm draws add32 queue submissions,
1280 command-source borrowed bytes including1024 command bytes, and64 entry
decodes, with no additional resource uploads/reads, shader modules, pipelines,
target allocations, readbacks or waits. Full-suite warm CPU wall time was
23.218ms versus81.713ms focused under uncontrolled concurrent host load.
These are CPU/debug observations, not GPU time, game performance, full batching
or zero-copy evidence. Rust allocator traffic is not instrumented for this
new draw workload. Only independently assembled synthetic shaders execute;
no original TOTK GPU command stream or frame is demonstrated.

The updated Windows `alu_gpu` executable compiled with debug information from
identical frozen candidate bytes. Actual D3D12 and Vulkan launches were attempted independently;
both fail in the WSL interoperability broker with `accept4` error110 before
adapter acquisition. Zero native executions passed in this milestone. The
other unchanged native test targets are not rebuilt or counted as executed.
No Windows policy or shared WSL process was changed to conceal this limitation.

The completed ISCADD workspace snapshot reports514 passing tests, three
existing CLI/Horizon failures and126 ignored GPU tests. The failures are the
homebrew heap-ratio expectation in two tests and absent-service exit status
in one; graphics results remain green. This is the frozen `2b7d93c` plus
seven-file source snapshot, not a live-worktree workspace pass.

## I2I: exact low-lane widening and shader-controlled vertex coverage

The four guest-selected integer-widening variants execute through `GpuDriver`,
GPFIFO, B06F/B197 and actual wgpu indexed rasterization. Fifty-four raw source
words per type (22 boundary/poison cases plus32 deterministic seeded words)
provide216 result checks. An independent oracle interprets little-endian low
bytes as u8/i8 or u16/i16, then numerically widens them to32 bits. The guest
shader compares those expected raw bits with ISETP before exporting
[64,128,191,255]; an incorrect reference exports[0,128,191,255]. Four additional
poisoned-source RZ draws and an auxiliary I2I verify zero-register semantics
and P0 isolation. Changing constant data does not recreate compiled shaders.

The vertex program changes the second X from+.6 to finite+.5. Changing only
guest X to+.57 (raw3F11EB85) gives signedFFFFEB85 and disables the substitution.
Changing I2I to U16 plus its expected literal to0000EB85 restores second-only
+.5 with one new vertex decode and no additional vertex upload or fragment
decode. Both binary changes are necessary to restore that predicate. Independent
analytic coverage on the47x29 viewport/52x29 target has ten unambiguous pixels
distinguishing+.57 from+.5 after edge exclusion. This is geometry execution,
not host-injected positioning or an exported integer masquerading as a float.

The pending-output test first publishes red0 RAM, then accepts correct red64
GPU work. Thirty-two unchanged draws leave RAM red0. Removing the first I2I
consumer wait faults without another draw or queue submission. Explicit
publication still produces red64 and updates the prior tokens' visibility,
without replaying accepted commands. Existing failed-atomic-publication,
layout, alias/remap and channel regressions remain separate acceptance cases.

The focused live run passed seven CPU tests and two actual OpenGL scenarios.
Before warm repetition the fragment workload records222 draws,443 queue
submissions, nine fragment decodes/1872 shader bytes,1776 constant bytes read
and1724 uploaded, twelve shader modules, twenty pipelines and one target
allocation/221 reuses. Its221 intentional oracle publications read1,644,240
bytes and publish1,333,072 packed guest bytes. The32 warm draws add32 queues,
1280 command-source borrowed bytes including1024 command bytes, and64 entry
decodes, with no additional resource reads/uploads, shaders, pipelines, target
allocations, readbacks or waits. Warm CPU wall time was24.965ms focused versus53.508ms in the full suite
under uncontrolled host load. This is not GPU time, game performance, full batching,
zero-copy rendering or an allocation-free claim. Rust allocation traffic is
not instrumented in this new draw workload.

Executed live-worktree I2I regression gates:

- GPU formatting and strict all-target package Clippy: passed.
- Without default features:286 CPU tests passed, zero failures.
- Default features:287 CPU tests passed, zero failures;128 GPU tests ignored.
- Explicit WSL OpenGL:128 actual GPU scenarios passed, zero failures/skips,
  llvmpipe LLVM23.1.1/Mesa26.2.4, including both new I2I scenarios.

Linux builds retain unoptimized code, debug assertions and overflow checks,
with debug information disabled. The faithful workspace/native snapshot is
`22d4c81` plus thirteen exact candidate code/assembly/binary files, unchanged
Cargo.lock and preserved source-built homebrew fixtures. Workspace validation
is separate from the passing live GPU gates. Whole-workspace formatting
reports peer-owned differences and strict Clippy reports six runtime errors;
those files are unchanged by this graphics delivery.

The updated Windows `alu_gpu` executable compiled with debug information from
identical frozen candidate bytes. D3D12 and Vulkan were each actually launched;
both fail in the WSL broker with `accept4` error110 before adapter acquisition.
Zero native GPU executions passed in this milestone; compilation is not pixel
validation. Unchanged native targets are not counted as rerun. No execution
policy or shared WSL process was changed. The tested shader programs remain
independently assembled synthetic inputs; no original TOTK GPU submission or
frame has been demonstrated.

The completed I2I workspace snapshot reports521 passing tests, three existing
CLI/Horizon failures and128 ignored GPU tests. This is the frozen `22d4c81`
plus thirteen-file snapshot described above, not a live-worktree workspace pass.

## GPR ISET: integer relations and exact mask/BF results

The real `GpuDriver`/GPFIFO/B06F/B197 indexed draw path executes all24 supported
relation/type/result combinations for48 raw pairs each:1152 main result checks.
An independent oracle widens u32 or sign-interpreted i32 to i64 and compares
using three-way Ordering. Its integer arithmetic has no overflow or float
precision dependency. The shader compares expected raw mask/BF bits through
ISETP before exporting[64,128,191,255]; wrong expected bits export
[0,128,191,255]. Six right-destination-alias cases and24 poisoned-RZ cases
exercise independent result storage and zero operands. The auxiliary ISET
must preserve an already established P0.

The vertex fixture selects only the second initial vertex using signed raw
X>0. BF-to-mask plus expected-immediate changes preserve identical analytic
coverage, cause one new vertex decode/pipeline and no additional vertex upload
or fragment decode. Guest second X=-.6 then disables the finite+.5 substitution,
adds72 vertex upload bytes and does not recompile the shader. The independent
coverage oracle distinguishes235 unambiguous pixels between those geometries;
it does not rely on near-edge tolerance to pass incorrect positions.

RAM first holds red0 while accepted pending GPU work produces red64. Thirty-two
unchanged draws preserve that separation. Changing only ISET's stall6 to13,
leaving adjacent ISETP's13 unchanged, faults without an extra draw/submission.
Explicit publication still produces red64 and advances prior visibility tokens
without replay. Existing atomic-write failure, alias, layout and channel tests
remain distinct regressions rather than newly claimed ISET functionality.

Focused live validation passed seven CPU tests and two actual OpenGL scenarios.
Before warm repetition the fragment workload records1184 draws,2367 queue
submissions,51 fragment decodes/10608 shader bytes,14208 constant bytes read and
10856 uploaded,54 shader modules,104 pipelines and one target allocation with
1183 reuses. Its1183 intentional oracle publications read8,801,520 bytes and
publish7,135,856 packed guest bytes. The32 warm draws add32 queue submissions,
1280 command-source borrowed bytes including1024 command bytes, and64 entry
decodes; resource reads/uploads, shaders, pipelines, target allocations,
readbacks and waits remain unchanged. Warm CPU wall time was37.753ms focused versus66.975ms in the full suite
under uncontrolled host load. These are CPU/debug observations, not GPU time,
game performance, full draw batching, zero-copy or allocation-free claims.
Rust allocation traffic is not instrumented in this new draw workload.

Executed live-worktree ISET regression gates:

- GPU formatting and strict all-target package Clippy: passed.
- Without default features:293 CPU tests passed, zero failures.
- Default features:294 CPU tests passed, zero failures;130 GPU tests ignored.
- Explicit WSL OpenGL:130 real GPU scenarios passed, zero failures/skips,
  llvmpipe LLVM23.1.1/Mesa26.2.4.

Linux builds retain unoptimized code, debug assertions and overflow checks,
with debug information disabled. The faithful workspace/native source snapshot
is committed `c530ca13` plus thirteen exact candidate code/assembly/binary files,
unchanged Cargo.lock and preserved source-built homebrew fixtures. It excludes
concurrent uncommitted runtime changes. Whole-workspace formatting reports
peer-owned differences; strict Clippy reports nine runtime diagnostics. These
are separate from the passing live graphics package gates.

The updated Windows `alu_gpu` executable actually ran30 shader GPU scenarios
on Intel UHD Vulkan (driver101.7076): zero failures/ignored, with one CPU test
filtered by the explicit `--ignored` selection. Both new ISET scenarios and the
preceding FSET/ISCADD/I2I/FLO/POPC/LOP3 scenarios execute in that current binary.
This validates the current candidate, rather than retroactively describing
older unexecuted revisions as native passes. The first D3D12 launch failed in
the WSL interoperability broker before adapter initialization.

All40 native GPU test executables subsequently compiled from the same frozen
source with debug information. Attempts to run that complete matrix failed in
the WSL broker with `accept4` error110 before its first adapter on each backend.
A small native process runner, intended to execute the suite inside one Windows
session, also compiled but was blocked at launch on the alternate existing
interop socket. Thus the native result remains30 actual Vulkan passes and
zero D3D12 passes; full native coverage is unavailable in this run. No host
execution policy or shared WSL process was changed. Only synthetic independently
assembled shaders execute; original TOTK GPU submission/presentation is unproven.

The completed ISET workspace snapshot reports528 passing tests, three failing
CLI/Horizon tests and130 ignored GPU tests. The failures are the existing
libnx/rebuilt-fixture heap-ratio expectations and absent-service exit status.
This is separate from the passing live GPU package and actual native Vulkan run.

## Generic PRMT selectors and raw-byte results

Seven focused CPU tests cover independent fixtures, all64 lane/nibble choices,
high register fields, register aliases, RZ, all three readiness dependencies,
scheduling rejection, P0 preservation and provenance erasure. Two real GPU
scenarios enter through GpuDriver/GPFIFO/B06F/B197. An independent byte-array
oracle uses signed-byte interpretation for replication and eight explicit
reference vectors. The fragment workload checks476 dynamic cases, twelve
destination aliases, five RZ combinations and twenty immediate cases. High
selector bits are poisoned independently. Mutable guest constants change raw
results without recompiling shaders. A wrong reference produces[0,128,191,255];
correct results produce[64,128,191,255].

The vertex program permutes initial first X=-.8 into a raw comparison, then
conditionally exports finite+.5. Changing guest X to-.6 disables that move.
Changing only selector/expected shader immediates restores first-only coverage
without another vertex upload. The analytic coverage oracle distinguishes the
geometry independently. Unsupported PRMT mode1 faults after prior accepted
work without another draw; explicit publication preserves that pending work
and advances its visibility tokens without replay.

Before warm repetition the fragment workload records515 draws,1029 queue
submissions,19 fragment decodes/4240 shader bytes,8160 constant bytes read and
4568 uploaded,22 shader modules,40 pipelines, one target allocation and514
reuses. Its514 intentional oracle readbacks transfer3,824,160 bytes and publish
3,100,448 guest bytes. Thirty-two warm draws add32 queue submissions,1280
borrowed command-source bytes including1024 command bytes and64 entry decodes,
with no resource reads/uploads, shader modules, pipelines, target allocations,
readbacks or waits. CPU wall time was39.548ms focused and30.961ms in the full
suite under uncontrolled host load. These are unoptimized CPU measurements,
not GPU/game timings, full batching, zero-copy or allocation-free claims; Rust
allocator traffic is not instrumented for this workload.

Executed live graphics gates: formatting and strict package Clippy passed;
300 CPU tests without default features and301 with default features passed.
The default suite ignored132 GPU tests; explicit WSL OpenGL execution ran all
132, zero failures/skips. Linux debug information was disabled while retaining
unoptimized code, debug assertions and overflow checks.

The faithful workspace snapshot is committed `b753dac` plus ten exact candidate
code/assembly/binary files, unchanged Cargo.lock and preserved source-built
homebrew fixtures. It excludes concurrent uncommitted runtime changes. It
reports535 passes, the same three CLI/Horizon failures and132 GPU ignores.
Whole-workspace formatting and nine committed-runtime Clippy diagnostics remain
outside the passing GPU package.

The updated Windows alu_gpu executable compiled with debug information. D3D12
and Vulkan launch attempts both failed in the WSL interoperability broker with
`accept4` error110, before adapter acquisition. A separately compiled small
native process runner was also blocked on the other existing interop socket.
There are zero current-PRMT native execution passes; the preceding ISET
revision's30 actual Vulkan passes are separate evidence. No execution policy,
shared WSL process or driver configuration was changed. These are synthetic
Maxwell programs, not original-game GPU submissions or TOTK frames.

## Ordinary three-source IADD3

Seven focused CPU tests cover independent stage fixtures, all destination
aliases and RZ subsets, high registers, strict modifier rejection, all three
readiness dependencies, pending destinations, P0 and provenance/rounding
hazards. Two actual OpenGL scenarios execute through GpuDriver/GPFIFO/B06F/B197.
The independent oracle widens each operand to u64, adds at most2FFFFFFFD, then
retains the low word. Sixty-four raw tuples include32 explicit overflow/sign/
carry boundaries and32 deterministic seeded cases; six additional B/C
destination aliases and seven poisoned RZ subsets distinguish source handling.
Expected bits are guest constant-buffer data, not shader specialization.

The vertex fixture uses all three inputs, including the previous B value at
an aliased destination. Initial first X=-.8 uniquely selects finite+.5. Guest
first X=-.6 disables it; changing shader source C from1 to2 and expected bits
to3F19999B restores first-only+.5, without another vertex upload. An analytic
coverage oracle checks each state. The wrong fragment reference leaves RAM
red0; correct pending output is red64. After32 warm draws, unsupported CC47
faults without an extra draw or queue submission. Explicit publication retains
accepted red64 output and prior visibility tokens without replay.

Before warm repetition the fragment workload records79 draws,157 queue
submissions,11 fragment decodes/2640 shader bytes,1264 constant bytes read and
1116 uploaded,14 shader modules,24 pipelines, one target allocation and78
reuses. Its78 intentional readbacks transfer580,320 bytes and publish470,496
guest bytes. Thirty-two warm draws add32 queue submissions,1280 borrowed
command-source bytes including1024 command bytes, and64 entry decodes;
resource reads/uploads, shaders, pipelines, target allocations, readbacks and
waits remain unchanged. Focused warm CPU wall time was42.185ms under uncontrolled
host load. This is not GPU/game timing, full batching, zero-copy or an
allocation-free claim; this workload does not instrument Rust allocator traffic.

Live IADD3 regression gates passed: GPU formatting, strict all-target package
Clippy,307 CPU tests without default features and308 with default features.
The normal default suite ignored134 GPU tests; explicit OpenGL execution ran
all134, zero failures/skips, on llvmpipe LLVM23.1.1/Mesa26.2.4. Unoptimized
Linux builds retain debug assertions and overflow checks with debug information
disabled.

The frozen workspace/native source is committed `2e71cb3` plus seven exact
candidate code/assembly/binary files, unchanged Cargo.lock and preserved
source-built homebrew fixtures. It excludes concurrent dirty runtime files.
Snapshot checks reproduce whole-workspace formatting differences, nine
committed-runtime Clippy diagnostics and the three existing CLI/Horizon
heap-ratio/absent-service failures. No passing whole-workspace claim is made.

The updated Windows alu_gpu executable compiled with debug information.
Both D3D12 and Vulkan launches failed in the WSL interoperability broker with
`accept4` error110 before adapter acquisition; zero current-IADD3 native GPU
passes are claimed. These are synthetic assembled programs, not authentic
original-game GPU submissions, presentation or TOTK frames.

The completed IADD3 workspace snapshot reports542 passing tests, the same
three CLI/Horizon failures and134 ignored GPU tests. This is separate from
the passing live graphics package and explicit134-scenario OpenGL execution.

## Ordinary low-halfword XMAD

Seven focused CPU tests and two actual OpenGL scenarios passed. Independent
fixtures, all source/destination aliases and RZ subsets, high-register fields,
strict mode/modifier rejection, every source dependency, pending destinations,
P0, register requirements and provenance/rounding hazards are covered. The
GPU tests enter through GpuDriver/GPFIFO/B06F/B197. A separate byte-array
u16/i16 oracle computes widened u64/i64 products plus full C, then retains
the low word. Each signedness executes64 tuples, including32 literal
boundary/overflow/upper-half poison cases and32 seeded cases, six B/C aliases
and seven nonempty RZ subsets. Mutable constants do not recompile the shader.

The vertex fixture multiplies genuine guest position/UV low halfwords.
Independent literal products identify only first X=-.8; it conditionally
exports finite+.5. Guest first X=-.6 disables that selection. Changing only
the XMAD opcode from signed to unsigned plus expected raw bits restores
first-only+.5 with one new vertex decode, no new fragment decode or vertex
upload. An analytic coverage oracle checks all three geometries.

For each signedness, wrong reference bits publish[0,128,191,255]; correct
GPU-pending work produces[64,128,191,255] while RAM retains the earlier red0.
After32 warm draws, changing only one signedness bit to an unsupported mixed
form faults without another draw/queue submission. Publication still exposes
accepted red64 output and advances prior visibility tokens without replay.

Each mode's pre-warm workload records79 draws,157 queue submissions,11
fragment decodes/2640 shader bytes,1264 constant bytes read and1204 uploaded,
14 shader modules,24 pipelines, one target allocation and78 reuses. Its78
intentional oracle readbacks transfer580,320 bytes and publish470,496 guest
bytes. Each32-draw warm sequence adds32 queue submissions,1280 borrowed
command-source bytes including1024 command bytes, and64 entry decodes, with
no additional resource reads/uploads, shader modules, pipelines, target
allocations, readbacks or waits. Focused warm CPU wall times were38.892ms
unsigned and48.866ms signed under uncontrolled host load. These are not
GPU/game timings, full batching, zero-copy or allocation-free claims; this
workload does not instrument Rust allocator traffic.

Completed live XMAD regression gates passed: GPU formatting, strict all-target
package Clippy,314 CPU tests without default features and315 with default
features. The normal suite ignored136 GPU tests; explicit OpenGL execution
ran all136, zero failures/skips, on llvmpipe LLVM23.1.1/Mesa26.2.4. Linux
builds retain unoptimized code, overflow checks and debug assertions with
debug information disabled.

The faithful workspace/native source snapshot is committed `a87e5b5` plus
nine exact candidate code/assembly/binary files, unchanged Cargo.lock and
preserved source-built homebrew fixtures. It reports549 passes, the three
existing CLI/Horizon heap-ratio/absent-service failures and136 GPU ignores.
Whole-workspace formatting differences and nine committed-runtime Clippy
diagnostics are separate from the passing GPU package gates. Later runtime
changes in `a7344fc` are outside this snapshot; they do not change graphics
sources, Cargo configuration or lockfile.

The updated Windows alu_gpu executable compiled with debug information. Both
D3D12 and Vulkan process launches failed in the WSL interoperability broker
with `accept4` error110 before adapter acquisition. Zero current-XMAD native
execution passes are claimed. No host execution policy, shared WSL process
or driver configuration was changed. The shaders/commands are synthetic;
original-game GPU submissions and real TOTK frames remain unproven.

## Direct constant-buffer MOV and texture handles

Seven focused CPU tests and two actual OpenGL scenarios passed. Tests cover
all32 slots with four offset boundaries, full destination fields, lane/predicate
and high-offset rejection, pending-destination protection, unrelated pending
work,64-reference capacity/deduplication with LDC, P0, fresh constant metadata
and explicit vertex rejection. The color fixture loads c3[10..1c] through an
actual B197 slot-three binding. Guest constants produce[64,128,191,255] and
[191,64,128,128] without shader recompilation. Changing only the red MOV's
binary offset changes its source to the green word. Unbinding slot three
faults without another draw/submission; previously accepted output remains
publishable without replay.

The texture fixture uses MOV c0[0] as the TEX handle origin. Literal pools at
GPU30100/31180 contain two TICs and one TSC. Guest handle1 selects TIC1 with
TSC0, handle0 selects TIC0, and handle1 selects TIC1 again. An accidental
shared texture/sampler index would exceed the sampler pool limit. TIC1 maps
GPU D0000 to canonical guest12000; two nonuniform transformed images are
packed by the independent scalar GOB test oracle. A separate f64 barycentric
nearest-sampling oracle validates more than100 interior samples and600
background pixels, excluding only edge/texel-boundary ambiguity. Guest texture
changes update pixels without shader recompilation. Unsupported TIC2 faults
while newer image2 output is pending; publication exposes the correct image
and prior visibility tokens without replay.

Each scenario performs32 unchanged deferred draws with32 additional host
queue submissions,1280 borrowed command-source bytes including1024 command
bytes and64 entry decodes. No new resource reads/uploads, shaders, pipelines,
target allocations, readbacks or waits occur. Before warm repetition the color
case records4 draws/7 queue submissions, one144-byte fragment decode,64
constant bytes read and60 uploaded, three readbacks/22320 bytes and18096 guest
write bytes. The texture case likewise records4/7, one144-byte fragment decode,
12 constant bytes read/uploaded, six descriptor reads, four texture uploads
and24576 guest texture bytes and the same three readbacks. Focused warm CPU times
were45.994ms color and56.588ms texture under uncontrolled load, not GPU/game
timings, full batching, zero-copy or allocation-free claims. Rust allocator
traffic is not instrumented for these new workloads.

Live MOV-constant gates passed: formatting, strict all-target GPU package
Clippy,321 CPU tests without default features and322 with default features.
The normal default suite ignored138 GPU tests; explicit WSL OpenGL ran all138,
zero failures/skips. Linux builds retain unoptimized code, debug assertions
and overflow checks with debug information disabled.

The current Windows alu_gpu executable compiled with debug information. Two
initial invocations each completed38 shader GPU scenarios on Intel UHD Vulkan
(driver101.7076), zero failures/ignores, one CPU test filtered. One invocation
was labelled D3D12 by its shell wrapper, but its actual adapter was Vulkan;
it is not D3D12 evidence and duplicate runs do not broaden unique coverage.

A native Rust process runner sets WGPU_BACKEND directly in child environments,
confirming actual D3D12 and Vulkan adapters. The serialized D3D12 process
completed23 individual scenarios, including both new MOV-constant tests;
the Vulkan process completed17, also including both new tests. Each was
subsequently terminated by the wrapper's90-second per-executable limit while
still progressing through other scenarios. Neither has a passing terminal
suite summary; these are partial execution results, not full-suite passes.

All40 native GPU executables were then rebuilt from the same frozen candidate.
Full-matrix attempts using a300-second per-executable runner failed in the
WSL broker with accept4 error110 before their first adapter, through both
existing interop connections. Thus current native validation includes real
pixel execution of the new scenarios on both host APIs, but no completed
full native matrix. No production test expectations, host execution policy,
shared WSL process or driver configuration were changed.

The faithful workspace/native source snapshot is committed `12239fe` plus
seven exact candidate code/assembly/binary files, unchanged Cargo.lock and
preserved source-built homebrew fixtures. Concurrent uncommitted runtime/SDL
changes are excluded. Snapshot formatting reports differences outside the GPU
package, and strict workspace Clippy reports13 committed-runtime diagnostics;
no passing whole-workspace result is claimed. Only synthetic independently
specified commands and shader fixtures execute here; original TOTK GPU
submissions and game frames remain unproven.

The completed preceding MOV-constant workspace snapshot reports556 passing
tests, the same three CLI/Horizon failures and138 ignored GPU tests. Its
source is the frozen12239fe-plus-seven-code-file image already described;
this is separate from the live package and native per-case execution results.

## Floating-point ALU constant-source resources

Eight focused live CPU tests and one real OpenGL scenario passed. The CPU
matrix checks both operations across every slot and offset boundaries,
aliases, RZ/high registers, invalid fields, source/destination scheduling,
shared-reference capacity/deduplication, P0, resource proofs and rounding
hazards. Explicit short-immediate FADD/FMUL cases extend the cross-form
hazard checks beyond GPR/full-immediate/CB forms. The complete144-byte shader
fixture independently reassembles with original envyas.

The GPU test enters through GpuDriver/GPFIFO/B06F/B197. Slot-three runtime
constants produce five independently specified dyadic results, including
negative finite operands. Examples are[64,128,191,255] and[128,191,64,255].
Changing only constant contents leaves shader/pipeline counters unchanged.
A literal FADD-to-FMUL opcode mutation with identical runtime operands changes
red128 to16, causes one fragment retranslation and no vertex upload. Unsupported
FTZ44 subsequently faults with no extra draw/submission; accepted red16 remains
publishable while guest RAM still contains red128. No draw is replayed.

Before warm repetition this workload records6 draws/11 queue submissions,
one144-byte fragment decode,96 constant bytes read/uploaded, four shader
modules/pipelines, one target allocation/five reuses, five intentional readbacks
transferring37200 bytes and30160 packed guest write bytes. Thirty-two warm
draws add32 queue submissions,1280 borrowed command-source bytes including1024
command bytes and64 entry decodes; resource reads/uploads, modules/pipelines,
target allocations, readbacks and waits remain unchanged. Focused warm CPU
wall time was72.307ms under uncontrolled load, not GPU/game time, full batching,
zero-copy or an allocation-free claim. Rust allocator traffic is not
instrumented for this new draw workload.

The FloatConstant Windows alu_gpu executable compiled successfully. Both
focused native launch attempts failed in the WSL interop broker with accept4
error110 before adapter acquisition, including the alternate existing socket.
There are zero Windows execution passes for this candidate. The earlier
MOV-constant native results above apply to their preceding revision only.
No shared WSL processes, driver settings or host execution policy were changed.

A faithful ordinary source snapshot uses committed72accf7 plus the five exact
FloatConstant production/test/assembly/binary files, the unchanged Cargo.lock
and preserved source-built homebrew fixtures. Concurrent uncommitted runtime
and SDL files are excluded. Snapshot GPU formatting and strict all-target
Clippy pass, with329 CPU passes without default features and330 with default
features; the latter normally ignores139 GPU tests. Separate live evidence
is the eight focused CPU tests, one focused OpenGL scenario and strict GPU
Clippy above; snapshot results are not live-worktree workspace results.

Explicit snapshot OpenGL execution passed all139 GPU scenarios, zero failures
or skips, including the new constant-source program and existing coherence,
publication-retry, depth, layout, channel and shader regressions. Whole-workspace
formatting still reports differences outside the GPU package, and strict
workspace Clippy reports13 committed-runtime diagnostics. No passing
whole-workspace result is implied by these package gates.

## Predicated MOV32I in both graphics stages

Seven live CPU tests pass, covering both independently assembled stage fixtures,
all32 immediate bit positions and raw NaN/sign payloads, high destinations,
initialized predicate/false-path requirements, unsupported fields, schedules,
waits, old-value/predicate rounding hazards and resource-proof erasure.

Two focused GPU scenarios pass on each of OpenGL llvmpipe, native Windows
D3D12 and native Windows Vulkan: six actual GPU passes, no failures/skips.
Native child environments select WGPU_BACKEND directly; logs identify Intel
UHD D3D12 driver32.0.101.7076 and Vulkan driver101.7076, with terminal two-pass
summaries. This is execution and pixel verification, not cross-compilation.

The fragment test uses runtime unsigned comparisons of guest constant words.
Six cases distinguish P0 and !P0 without recompilation. Changing the immediate
bits1→.5 and then only predicate inversion produces distinct red255→128→64
results. A subsequent unsupported P1 faults without an extra draw/submission;
pending red64 publishes while guest RAM previously held red128, without replay.
The vertex test changes the first clip X from-.8 to+.5 through decoded MOV32I.
Changing guest X to-.6 disables that guard with72 vertex-upload bytes and no
shader recompile. Updating the guest compare constant and code immediate then
moves only the first vertex to+.25 with one vertex decode and no vertex upload.
An independent f64 edge/barycentric coverage oracle checks inside/outside pixels.

Before warm repetition the fragment workload records8 draws/15 queue submissions,
three fragment decodes/624 shader bytes,64 constant bytes read/uploaded, six
shader modules/eight pipelines, one target allocation/seven reuses and seven
intentional readbacks transferring52080 bytes with42224 packed guest-write bytes.
Thirty-two warm draws add32 queues,1280 borrowed command-source bytes including
1024 command bytes and64 entry decodes; resource reads/uploads, modules/pipelines,
target allocations, readbacks and waits remain unchanged. Focused OpenGL CPU
wall time was16.640ms and native D3D12 was26.264ms under uncontrolled load;
these are not GPU/game timings, full batching or allocation-free/zero-copy claims.
Rust allocator traffic is not instrumented for these new draw workloads.

Live package gates pass: formatting, strict all-target Clippy,336 CPU tests
without default features and337 with default features. The normal default
suite ignores141 GPU tests; explicit OpenGL execution passes all141 with zero
failures/skips. Linux remains unoptimized with debug assertions and overflow
checks enabled, debug information disabled. A broader native alu_gpu launch
subsequently failed in the WSL broker with accept4 error110 before adapter
acquisition; it adds no full-suite Windows pass to the focused results above.

The completed preceding FloatConstant workspace snapshot is committed72accf7
plus its five exact candidate code/assembly/binary files and unchanged lock,
excluding concurrent uncommitted runtime/SDL work. It reports564 passing tests,
the same three CLI/Horizon failures and139 ignored GPU tests; workspace
formatting and13 committed-runtime Clippy diagnostics remain separate from
passing GPU package gates. That snapshot is not the later f955345 live tree
or a complete validation of its runtime changes.

### GPU controller identity and descriptor isolation

`wl-nvdrv` has five CPU tests for the inline characteristics/TPC wire formats,
opaque address tags, short/null requests, output failure atomicity, reserved
fields, and padding. The existing four capture tests also pass. Normal package
execution reports nine passing tests and five ignored device-dependent tests.

Explicit device execution ran `control_gpu` (one scenario) and `runtime_gpu`
(four scenarios) on WSL OpenGL, native Windows D3D12, and native Windows Vulkan:
fifteen passes, no device-test skips. The controller scenario checks 32 repeated
queries, actual FD close/reopen, cross-device and stale-FD rejection, ioctl
kind/direction errors, and zero additional GPU submissions, draws, uploads or
readbacks. A memory adapter that panics on every read/write/pin operation proves
that the address tag is never accessed. The two runtime scenarios retain real
HIPC/CMIF-to-indexed-rendering, CPU mutation, pixel and partial-fault publication
coverage. Strict package Clippy passes.

A separate local execution of the original base executable with returned-callback
discovery reached 113,150,504 guest instructions and 99 IPC requests. It advanced
past the previously unresolved callback, then observed an unsupported GPU
controller open and later an unmapped read. This does not establish that the
controller open caused the later fault, or demonstrate an original GPU draw,
framebuffer capture, or VI presentation. The independently assembled rendering
tests above are synthetic compatibility tests, not game frames.

The additional runtime scenario sends literal HIPC command-12 descriptors for
Ioctl3 GET_CHARACTERISTICS. Both output buffers receive the independently checked
GM20B identity, while overlapping outputs fail without modifying either and a
correct retry succeeds. CPU tests cover zero, short and oversized second outputs,
null legacy address tags, and failure atomicity of both metadata outputs. A subsequent
original executable run reached 113,154,258 instructions and 102 IPC requests,
opened the controller successfully, and identified Ioctl3 GET_CHARACTERISTICS as
the next unsupported request before this extension. It still ended at the same
unmapped read and did not produce original GPU output.

The Ioctl3 transport test also supplies a zero unused address tag, matching the
original SDK call. Its bounded second output remains mandatory and both guest
destinations still undergo the same preflight. An original run with the earlier
shared legacy-tag restriction reached the controller query but rejected it as
BadParameter. This distinction is now tested separately from the legacy nonzero
tag requirement, including malformed-size failure without partial output.

The Ioctl3 TPC-mask scenario verifies four- and eight-byte second outputs through
literal HIPC descriptors, with zero legacy address words. CPU cases exercise
nonzero size tags up to UINT32_MAX without allocation, ignored reserved data,
unsupported output extents, and no partial writes on rejection. Direct adapter
tests additionally prove that enabling this transport does not enable it on an
nvmap descriptor or cause guest-memory dereferences. An original run reached
113,167,568 instructions and 103 IPC requests, successfully completed the
characteristics query with its zero unused tag, and then identified Ioctl3 TPC
masks as the next unsupported request. No original GPU draw or frame was
observed in that run.

### Explicit Switch address-space configuration

The address-space decoder has three CPU tests covering literal field order,
64 KiB configuration, default 128 KiB configuration, region edges, unsupported
page sizes/sharing/flags, malformed geometry and overflowing reservations.
A device-backed scenario verifies failed initialization followed by successful
retry, duplicate initialization, a real 64 KiB reservation/free pair and an
out-of-region request leaving output unchanged. Existing literal HIPC indexed
textured-draw and partial-fault/publication tests now initialize with flag one
and an explicit 64 KiB large-page selection.

Validation: 12 CPU tests and six explicitly executed device scenarios per
backend passed on WSL OpenGL, native Windows D3D12 and native Windows Vulkan
(18 device passes, zero skips in the explicit runs). Package strict Clippy and
workspace formatting passed. Device initialization/reservation scenarios do not
claim rasterization; the existing runtime draw scenarios perform actual draws.
No full workspace test run is included in this result.

### Configured VA-region query

The region-query tests assert literal bases, page sizes, reserved zeroes and
independently specified page counts for the configured lower and upper regions.
A too-short Ioctl3 second output leaves both destinations unchanged; legacy
Ioctl and valid Ioctl3 return identical descriptors. Literal HIPC/CMIF coverage
checks both output buffers against the configured bounds with no GPU submission.

Validation: 13 CPU tests and seven explicit device scenarios passed per backend
on WSL OpenGL, native Windows D3D12 and native Windows Vulkan (21 device passes,
zero explicit-run skips). Strict package Clippy and workspace formatting passed.
The unchanged indexed textured-draw and partial-publication/fault regressions
remain included. No full workspace run is claimed.

An original-executable run accepted the explicit InitializeEx request and then
reached legacy GetVARegions, which the preceding backend did not support. It
executed 113,222,743 original instructions and 110 IPC requests; it produced no
original GPU frame. This identifies a service boundary rather than demonstrating
general NVIDIA compatibility or presentation.

### Zero-size VA queries and automatic reservations

Original-executable inspection showed a fully zero-initialized GetVARegions
header. The query accepts this sizing form and returns the available 48-byte
size with both configured descriptors. Direct and literal HIPC/CMIF tests check
legacy Ioctl and Ioctl3 using that header. The bounded automatic-reservation
policy has a CPU oracle for alignment, unsorted occupied ranges, holes, region
selection, exhaustion, unsupported defaults and malformed sizes. Device tests
reserve distinct intervals, free one, reject bad alignment without changing
output, reuse the freed hole and release both real reservations.

Fourteen CPU tests and seven device scenarios passed on WSL OpenGL, native
Windows D3D12 and native Windows Vulkan (21 executed device passes). One Vulkan
process launch initially failed in WSL interop with `UtilAcceptVsock` error 110
before the Rust test runner started. After confirming no orphan test process,
executing the same built binary succeeded: five runtime scenarios, zero
failures. The startup failure is not counted as an executed GPU scenario.
Strict package Clippy, workspace formatting and diff whitespace checks passed.
No full workspace test result is claimed.

### User-backed nvmap allocation metadata

Two CPU decoder tests verify both cache flags, allocation alignment independent
of CPU page alignment, reserved fields, unsupported heaps/kinds/flags, null and
unaligned CPU addresses, size and address overflow. The literal runtime service
scenario pins and unpins a 4 KiB backing deliberately not aligned to the
requested 128 KiB allocation boundary. Indexed textured drawing and partial
fault/publication regressions use cache flag zero and 128 KiB alignment.

Sixteen CPU tests and seven device scenarios passed on WSL OpenGL, Windows
D3D12 and Windows Vulkan (21 executed device passes). A WSL interop startup
failed before the native runtime test runner started. The current Cargo-selected
PE binary was then hash-verified and executed through a Windows-owned launcher:
five runtime scenarios passed on each Windows backend. A stale diagnostic
binary encountered while preparing that launcher is excluded from these totals.
Strict package Clippy and workspace formatting passed.

Additional `wl-gpu` CPU regression runs passed 337 tests with default features
(141 ignored GPU scenarios, not counted as passes), and 336 without default
features. No full workspace run is claimed.

The preceding original-executable run accepted InitializeEx, both legacy/Ioctl3
VA-region queries, AllocSpace and nvmap Create before the allocation metadata
restriction. It executed 113,271,840 original instructions and 117 IPC requests,
with no original GPU frame.

### Cache-policy transitions without releasing GPU registrations

The original executable successfully attached its 4 KiB nvmap backing and then
requested Uncached through SVC 3. The previous blanket pin guard rejected that
request with kernel result `0xd401` (module 1, description 106), causing the
guest's own break path. No guest registers or instructions were patched.

Runtime tests now verify complete-pin Uncached set/clear, exact stored bytes,
unchanged RW permissions, neighboring attributes, retained registration tokens,
and continued reprotection rejection. Partial-pin attribute changes still fail
before splitting or writing. Region-split tests use asymmetric byte sequences
and assert that the larger side retains the original pointer and capacity.
They do not claim zero copying: the prefix split shifts the retained bytes.

All 147 runtime CPU tests passed. The NVIDIA package passed 16 CPU tests and
seven device scenarios on each of WSL OpenGL, native Windows D3D12 and native
Windows Vulkan (21 explicit device passes, no skips). Its indexed textured-draw
and fault/publication scenarios set Uncached while the backing remains pinned;
the successful path clears it before registration teardown and repinning.
Strict Clippy for both packages and workspace formatting passed.

A runtime change invalidates dependent Rust archive identities. A standalone
compiler preflight confirmed that reusing dependent archives from the old
runtime fails with E0460. Rebuilding against the new runtime is required; a
completed synthetic validation is not proof that the new original executable
has already run.

### Process-local nvmap parameter queries

The CPU oracle checks literal SIZE, ALIGNMENT, HEAP and KIND results, missing
backing, unsupported BASE, unknown selectors and oversized SIZE. Direct device
tests query SIZE before backing registration and verify rejected metadata
queries preserve output without accessing guest RAM. A literal runtime service
scenario attaches real backing and checks all four results.

Seventeen CPU tests and seven device scenarios passed on WSL OpenGL, native
Windows D3D12 and native Windows Vulkan (21 device executions, zero skips in
explicit runs). Strict package Clippy and workspace formatting passed. This
adds a source-verified query subset; it does not claim that an original game
has already exercised this new request.


### Bounded MME branch execution

The independently assembled branch corpus verifies forward ANNUL and backward
ordinary branches, exact register conditions, conditional EXIT, signed targets
and delay-slot execution. CPU cases retain delayed targets across parameter
waits and SEND yields; reject unknown fields, unready registers, missing uploads
and control flow in delay slots; and enforce one total 64-instruction budget.

The GPFIFO device fixture chooses red-only versus full color writes from guest
macro arguments, executes a backward loop, and checks asymmetric indexed texture
pixels without creating another shader module. Fault scenarios issue a real WFI
after an earlier draw, then hit an infinite loop or an invalid delay-slot branch.
They check that guest RAM remains unpublished, retry cannot replay the draw, and
explicit publication still produces the independent expected image.

Strict package Clippy and workspace formatting passed. The default GPU/NVIDIA
CPU suites passed 417 tests; 168 device tests remained separately ignored there.
The no-default GPU suite passed 392 tests. Explicit device runs passed 22 graphics
scenarios on WSL OpenGL, 22 on native Windows D3D12, and 22 on native Windows
Vulkan, with no skips or failures. Each Windows run additionally passed the
completion-watchdog unit test. Windows tests were executed as native processes
with verified executable hashes and recorded exit status, not only compiled.
These are synthetic conformance workloads; they do not establish original-game
firmware handling, submitted draws, display output or frames.


### Privileged baseline initialization and acknowledgement ordering

CPU tests cover all seven source-verified startup writes, partial known-bit
updates, repeated writes, independent context initialization and preservation of
unknown reset bits. Missing WFI, absent scratch fields, nonzero request flags,
unknown addresses, different masks/values, zero masks, combined unsupported
writes and nonzero conservative-raster values fail before context mutation or
acknowledgement.

The independent GPFIFO fixture executes WFI, supplies real DATA arguments,
performs the admitted context write, reads and tests its completion flag through
MME instructions, and then selects a color mask for a genuine indexed textured
draw. A second invocation changes verified pixels without another shader module.
Counters retain two explicit readbacks for two framebuffer publications and one
additional WFI wait after prior GPU work; context updates themselves submit no
GPU commands and perform no readback. Adversarial requests after a valid pending
draw preserve that output, latch channel failure and prevent replay.

Strict Clippy and formatting passed. Default GPU/NVIDIA CPU suites passed 419
tests, with 170 device tests separately ignored there; the no-default GPU suite
passed 394 tests. Explicit graphics execution passed 24 scenarios each on WSL
OpenGL, native Windows D3D12 and native Windows Vulkan, with no failures or skips.
Each native backend also passed the completion-watchdog unit test. These results
validate a bounded initialization HLE, not execution of Falcon firmware code,
general privileged register semantics or original-game frames.


### MME bitfield replacement and extraction

An independent bit-at-a-time oracle verifies 1,536 deterministic combinations
across replacement and both extraction/shift forms. Cases include zero/31-bit
widths, high positions, truncated source/destination spans and old-input reads
when the result aliases a source. Negative cases reject unready operands,
nonzero unused fields and unsupported variable shifts of 32, 33 and UINT_MAX.
The independent assembler corpus executes all three operations in a GPFIFO-driven
indexed textured draw. Changing its guest argument changes the color mask and
verified pixels while preserving the existing shader module.

Strict package Clippy and workspace formatting passed. Default GPU/NVIDIA CPU
suites passed 422 tests (171 device tests separately ignored); no-default GPU
passed 397 tests. Explicit graphics runs passed 25 scenarios on WSL OpenGL,
25 on native Windows D3D12 and 25 on native Windows Vulkan, without skips or
failures. The native completion-watchdog unit test also passed on both backends.
These are actual device executions of synthetic fixtures; no original-game
frame or general MME compatibility follows from these results.


### ZCULL configuration with complete fragment tests

The literal incrementing B06F packet `[2002065a, 11, 0]` independently selects
`SET_ZCULL` and `SET_ZCULL_BOUNDS`. CPU tests exercise all four depth/stencil and
min/max flag combinations, valid six-bit region IDs, eight stencil comparisons,
asymmetric stencil reference/mask bytes, channel isolation, invalid bits,
unaligned methods and preservation of prior state on failure. ZCULL sync,
statistics reports and functional stencil activation remain explicit failures.

Real indexed textured GPFIFO draws include these hierarchy controls; a separate
assembled macro emits the criterion and preserves independently checked pixels.
The depth comparison suite changes hierarchy flags and enabled/disabled region
selection while exercising every supported fragment comparison with depth
writes on/off. Its independent color/depth oracle must remain unchanged.
The full existing depth suite also covers Z16/ZF32 transfers, aliases, CPU patches,
publication retry, clipping and overlapping blended geometry.

Strict Clippy and formatting passed. Default GPU/NVIDIA CPU suites passed 423
tests (172 device tests separately ignored); no-default GPU passed 398 tests.
Explicit graphics execution passed 37 scenarios on each of WSL OpenGL, native
Windows D3D12 and native Windows Vulkan, with no skips/failures. The native
completion-watchdog unit also passed on each backend. Retained hierarchy controls
do not imply auxiliary ZCULL memory, query reports or stencil rasterization.
No original-game frames are established by these synthetic tests.


### Ordered instrumentation initialization metadata

Literal FIFO packets verify HEADER/DATA byte offsets, exact payloads, retained
per-context values, reinitialization, and rejection of missing headers, duplicate
DATA, unknown payloads and unaligned methods without changing earlier metadata.
The existing indexed textured GPFIFO device fixtures include the exact pair and
still verify asymmetric color masks, coherent publication and shader reuse.
No profiling result or instrumentation completion is synthesized.

Strict Clippy and formatting passed. GPU/NVIDIA CPU suites passed 424 default
tests (172 device tests separately ignored); GPU no-default passed 399 tests.
Explicit graphics runs passed 37 scenarios on each of WSL OpenGL, native Windows
D3D12 and native Windows Vulkan, with no skips/failures. Each native backend also
passed its completion-watchdog unit test. These results validate only the stated
initialization subset, not arbitrary instrumentation packet interpretation.


### MME dispatch through existing B197 validation

CPU tests reject emitted SET_OBJECT/channel methods, mutation of running MME
RAM/entry state, and nested launches. Invalid draw arguments now fail in the
same existing draw validator as direct packets. Descriptor invalidation macros
finish inline with exact epoch increments, rather than leaving an unacknowledged
backend effect.

Independent GPFIFO fixtures prove that MME alone binds the CB used by LDC/TEX
(the prior binding is explicitly removed) and that MME alone initiates an indexed
draw (all caller draw methods are removed). Two unchanged macro-driven draws
retain one resident target until one publication/readback. MME constant writes
change actual mapped guest bytes and independently expected solid LDC colors
from `[64,128,191,255]` to `[255,0,0,255]` without another shader decode or pipeline.
An MME clear produces the expected complete RGBA target while draw/shader-decode
counts remain zero. A completed constant write after WFI cannot authorize a
privileged request: it faults, preserves the earlier pending draw, and cannot
replay on retry. Reentrant-RAM fault injection retains the earlier completed WFI
and draw for explicit publication.

Strict GPU Clippy, workspace Clippy and formatting passed. Default GPU/NVIDIA
CPU suites passed 426 tests (177 device tests separately ignored); GPU no-default
passed 401 tests. Explicit graphics execution passed 42 scenarios on each of
WSL OpenGL, native Windows D3D12 and native Windows Vulkan, without failures or
skips. Native watchdog unit tests passed on both backends. These are synthetic
conformance fixtures, not original-game frame evidence. Queue submission remains
per draw; only the explicit final publication reads back the repeated-draw image.


### Finite MME loops with a shared submission work allowance

The independent constant-fill program completes 128 iterations and 517
instructions, exceeding the earlier 64-step budget. CPU tests verify exact
emissions and instruction debits, sharing across invocations, zero-budget
failure, nonemitting loops, yields, and reservation across parameter pauses.
Infinite invocations remain bounded at 1024 instructions. GPU tests initialize
a previously invalid CB with 512 actual zero bytes, preserve adjacent canaries,
and verify the resulting asymmetric indexed texture pixels through LDC/TEX.

Literal nonincrementing upload packets and 1024 exact-budget macro launches
consume one complete submission allowance. One extra launch through a second
aliased GPFIFO entry fails with `SubmissionWorkBudgetExceeded`, preserves the
prior pending GPU draw and cannot replay on retry. A separate request using the
complete valid allowance leaves engine/shader state intact; the next submission
can upload a draw macro and render, proving that reset occurs at the API boundary.
No per-draw readback or completion wait is added.

Strict package/workspace Clippy and formatting passed. Default GPU/NVIDIA CPU
suites passed 429 tests (180 device tests separately ignored); GPU no-default
passed 404 tests. Live-worktree workspace tests passed 789 tests with 181
separately ignored tests, without failures. Explicit graphics execution passed
45 scenarios each on WSL OpenGL, native Windows D3D12 and native Windows Vulkan;
watchdog unit tests also passed natively. These synthetic results do not establish
original-game frames or general unlimited MME execution.


### Vertex-stream substitute register retention

A literal incrementing B06F packet writes `0x0f84/0x0f88` at the maximum
40-bit address. Tests verify independent partial writes, invalid upper bits,
unaligned methods, zero address, channel isolation, and unchanged state after
rejection. A configured substitute does not create missing vertex state or
relax inclusive vertex-span/overflow checks.

The MME indexed-raster fixtures configure an intentionally unmapped substitute
address. Their independently expected masked/textured pixels and shader reuse
prove that supported in-bounds geometry does not dereference it. Actual runout
fetches remain unsupported.

Formatting and strict package Clippy passed. Default GPU/NVIDIA CPU suites passed
430 tests (180 device tests separately ignored); GPU no-default passed 405 tests.
Explicit graphics runs passed 45 scenarios on each of WSL OpenGL, native Windows
D3D12 and native Windows Vulkan, without skips or failures. Completion-watchdog
unit tests also passed on both native backends.


### B197 inline pitch uploads

CPU tests cover exact launch flags, address width/overflow, descriptor bounds,
zero/max length, missing state, active-transfer mutation rejection, surplus data,
channel isolation, packet continuation and explicit finish with missing words.
An independent envyas MME configures and writes actual mapped constant bytes;
its LDC fragment changes covered pixels from `[64,128,191,255]` to `[255,0,0,255]`
without shader retranslation or pipeline recreation. Each draw is still genuine
GPFIFO/B06F/B197 execution, with explicit publication for the pixel assertion.

Adversarial device scenarios verify unread command-suffix replacement through a
GPU VA alias, publication of prior pending color output before an aliased inline
write, and injected atomic-write failure preserving the journal and prior draw
without replay. Another scenario proves that a zero-length unmapped upload makes
no guest access; a nonempty transfer crossing its mapping fails at launch before
DATA, and two requests continue one nonincrementing DATA packet in correct byte
order. The metadata/data-only scenario adds no queue submission, wait or readback.
Completed bytes are counted in `inline_upload_write_bytes`, separately from CB
method-port writes. No zero-copy GPU transfer claim is made.

Formatting and strict package Clippy passed. Default GPU/NVIDIA CPU suites passed
432 tests (185 device scenarios separately ignored); GPU no-default passed 407.
Explicit execution passed 50 graphics/device scenarios on each of WSL OpenGL,
native Windows D3D12 and native Windows Vulkan, including bounds-only cases that
initialize a device without rasterizing. Completion-watchdog unit tests also
passed on both native backends. Live-worktree workspace Clippy passed, and the
workspace suite passed 792 tests with 186 separately ignored device tests.


### Per-target blend bank selection

CPU tests verify independent storage of all eight seven-word banks, selection
of target zero, separate/shared alpha requirements, global-bank preservation,
reserved words, invalid factors/operations, unaligned writes and failed-write
atomicity. Existing omitted-global-field tests retain their diagnostic contract.

Three explicit blend scenes cover the original global path, per-target zero
with conflicting bank-one state, and global-to-per-target switching. Their
independent scalar oracle checks both overlapping asymmetric triangles, separate
RGB/alpha arithmetic, uncovered pixels and channel write masks. Equivalent bank
selection and OGL/D3D encodings create no extra pipeline or shader module. An
incomplete newly selected channel fails before drawing and cannot reuse the
previous resident blend state or modify published framebuffer bytes.

The explicit graphics/device batch passed 53 scenarios on each of WSL OpenGL,
native Windows D3D12 and native Windows Vulkan; completion-watchdog unit tests
also passed on both native backends. Bounds-only device scenarios are included
in this count. Default GPU/NVIDIA CPU suites passed 434 tests, with 187 device
tests separately ignored. No-default GPU CPU suites passed 409 tests. Formatting
and strict package Clippy passed. Multiple color attachments remain
unsupported; retaining inactive banks does not claim MRT rasterization.


### Filled-triangle edge marking

A literal FIFO packet verifies B197 edge-flag retention, reserved-bit rejection,
unaligned-method rejection, channel isolation and continued refusal of polygon
POINT/LINE modes. Maxwell integration tests passed 22 cases in both feature
configurations; strict package Clippy and formatting passed. Three asymmetric
blending scenes toggle edge marking from true to false between genuine draws,
then verify the same independent filled-coverage, alpha and write-mask oracle.
All three scenes passed on WSL OpenGL, native Windows D3D12 and native Windows
Vulkan; native completion-watchdog unit tests also passed. The toggle adds no
shader/pipeline recreation or forced intermediate readback. This validates only
FILL-mode invariance, not wireframe or per-vertex boundary-edge rendering.


### Canonical negative-W clipping and independent viewport banks

CPU tests cover explicit -W..W selection, matching one-half Z scale/bias,
continued rejection of missing selectors and unsupported clip profiles, and
independent storage of all sixteen viewport banks. Unaligned, reserved-word,
nonfinite and incompatible swizzle/depth-bound writes cannot alter viewport zero.
The source-verified unclamped `0x181d` profile is accepted alongside `0x1801`;
no general depth-clamp or undocumented guardband implementation is claimed.

The GpuDriver/GPFIFO clipping scene alternates 0..W and -W..W on one channel,
with thirteen plane/endpoint/partial-clipping configurations, unequal positive W,
textured NoPerspective varyings and indexed geometry. Independent screen-space
barycentric and scalar-depth oracles validate covered/uncovered color, exact
clipping and guest block-linear ZF32 storage/padding. Conventions are switched
without changing guest program bytes: Maxwell decode counts remain unchanged,
while each required host vertex specialization creates exactly one new shader
module and pipeline. Unchanged repetitions reuse both. Failed selector writes
preserve the preceding accepted image for explicit publication without replay.

Default GPU/NVIDIA CPU suites passed 437 tests (188 device tests separately
ignored); GPU no-default passed 412 tests. Strict package/workspace Clippy and
formatting passed. Explicit graphics/device batches passed 54 scenarios on each
of WSL OpenGL, native Windows D3D12 and native Windows Vulkan, plus watchdog unit
tests natively. Device-only boundary tests are included in these totals; they
must not be counted as rasterized original-game frames. Live-worktree workspace
validation passed 797 tests, with 189 separately ignored device tests.


### Guarded MME shadow state and inactive vertex ranges

CPU tests distinguish unknown reset history, unfiltered capture, passthrough,
known replay and unverified filtered captures. Failed writes preserve snapshots;
filtered/unwritten replay and unsupported replay actions fail before live state
mutation. Literal FIFO packets verify byte/word method addressing, channel-local
capture and fault latching. All fifteen inactive vertex address/limit pairs have
independent 40-bit field validation; they never provide missing stream-zero
layout or authorize a larger fetch.

An indexed textured GPFIFO scene captures a red-only mask, changes live state in
passthrough, then replays the captured mask through an independently assembled
MME. Incoming replay data is deliberately invalid as a normal mask. Scalar
pixel checks prove that the known captured argument controls actual rendering.
All inactive vertex ranges are deliberately unmapped and remain unread. Warm
replay retains decoded programs and shader modules; an unchanged effective mask
also reuses its pipeline. Explicit publication/reset between assertions is test
policy, not a new per-draw synchronization requirement.

Three fault injections retain a prior actual draw pending: filtered capture,
missing capture and unsupported draw replay. Each preserves old output, adds no
queue submission/readback, publishes the independently expected earlier image,
and refuses whole-channel retry without duplicating accepted work.

Default GPU/NVIDIA CPU suites passed 440 tests (190 device tests separately
ignored); GPU no-default passed 415 tests. Strict package/workspace Clippy and
formatting passed. Live-worktree workspace tests passed 800 cases, with 191
separately ignored device tests. Explicit graphics/device batches passed 56
scenarios on each of WSL OpenGL, native Windows D3D12 and native Windows Vulkan;
native watchdog unit tests also passed. Hardware filtering, general resource
replay and original-game frame rendering are not established by these fixtures.

### Initialization and ordered cache actions

The initialization-control batch passed 444 CPU tests across `wl-gpu` and
`wl-nvdrv` with default features, plus 419 `wl-gpu` tests with default features
disabled. Strict package Clippy and workspace formatting passed. A previous
engine-binding regression expected every compute program-address write to fail;
it now verifies isolated retained compute state, unchanged B197 program state,
and continued rejection of compute execution.

The source-derived GPFIFO initialization scene executes an indexed textured
triangle, 22 literal public-driver privileged requests, cache invalidations,
and another draw. Independent triangle/UV coverage verifies the result. All
22 privileged requests require actual WFI; none reads back the framebuffer.
Shader/constant/texture cache invalidation increases the corresponding guest
acquisition counters, reuses compatible shader modules and pipelines, and
performs no implicit readback. A subsequent host dirty-flush publishes exactly
once; the following system invalidation performs no second readback or draw.
Unit tests separately verify final masked privileged context bits, unknown upper
bits, invalid requests without acknowledgement, field bounds and atomic state
preservation.

The GL regression selection passed 57 device/graphics scenarios: 41 GPFIFO,
12 depth, three blend and one zero-coefficient blend scenario. These are
synthetic validation workloads, not original-game frames. Full workspace tests
were not repeated for this batch; the prior 800-test workspace result remains
historical.

The identical selection also executed natively on Windows D3D12 and Vulkan:
57 scenarios per backend plus the device-completion watchdog unit on each.
All ten native processes exited zero after binary SHA-256 verification.
The first WSL launcher failed before process startup (`UtilAcceptVsock`);
selecting the working `/run/WSL/1_interop` socket allowed the actual native runs.
The launch failure is not included in the GPU pass count. No independent peer
review is claimed for this batch.

B06F semaphore coverage includes literal selector/control boundaries, missing
state, malformed alignment/reserved bits and unsupported operations. GPU tests
submit an indexed textured draw followed by release, verify guest framebuffer
pixels before observing the payload, and confirm the following equality acquire.
Another release overwrites an unread GPFIFO packet header; execution must consume
the new header. Unsatisfied acquire preserves the earlier submitted draw and
permits separate publication without replay. A driver test retrieves the
semaphore-published image once, rejects another channel taking it and measures no
additional readback at diagnostic retrieval.

Allocation benchmarks distinguish caller-thread bookkeeping from process-wide
Rust allocation events, which may include asynchronous graphics-driver threads.
The scoped test allocator delegates to the existing process-wide instrumented
System allocator; both scopes are reported. A positive test explicitly allocates
and reallocates to verify that caller-thread accounting is active. Zero caller
allocations must not be presented as zero total process allocations or zero-copy
GPU transfers.

Sixteen-byte release validation checks exact payload/reserved words, untouched
adjacent bytes, monotonic nonzero timestamps bounded by independently measured
host elapsed time, and the exact rational timer conversion including the u64
boundary. An indexed textured draw followed by WFI-disabled timestamp reports
retains pending GPU output, performs no readback at those reports and only
publishes pixels at a later explicit visibility boundary. The NOP retirement
benchmark also verifies that CPU-only completion records avoid backend polling.

Tile-cache CPU tests exercise literal B06F packet dispatch, missing engine binding,
operand-zero barrier/flush effects, malformed offsets and unsupported operands.
Rejected channel methods include subchannel, byte method and raw operand in their
diagnostics, preserving the channel fault latch.
The `tile_cache_operations_publish_real_pixels_and_preserve_driver_capture`
GPU case submits an indexed textured triangle through `GpuDriver` and GPFIFO,
checks independently calculated covered/uncovered pixels in guest RAM, and checks
one coherence barrier/readback. Subsequent publication returns the same completed
image exactly once with no draw replay or second readback. Both tile methods were
executed on WSL OpenGL and native Windows D3D12/Vulkan.

Compute selector tests check exact NVIDIA field maxima, first-invalid values,
misaligned direct offsets, partial selectors, channel defaults and separation
from graphics pool writes. `compute_resource_selectors_preserve_graphics_and_reject_dispatch`
submits literal subchannel-one B1C0 selectors, a real indexed/textured B197 draw,
and an unsupported compute launch through the driver/GPFIFO boundary. It checks
one accepted draw, a launch-specific fault, deferred guest visibility, and exact
reference pixels after publication without replay. This verifies graphics
isolation and failure semantics; it does not claim compute shader execution.

Graphics scratch tests distinguish the 40-bit address, 38-bit size and 26-bit
per-warp fields and check atomic rejection at their first-invalid boundaries.
`graphics_scratch_configuration_preserves_real_pixels_and_warm_pipeline`
submits a guest indexed/textured draw with maximum selectors, changes all scratch
selectors to zero and submits another draw. Both images match the independent
pixel oracle; the second draw requires no additional pipeline/module allocation
or shader-code read. No scratch allocation or scratch-access shader is implied.

Engine-report CPU tests cover partial selectors, upper-field limits, alignment,
independent engine state and rejection of counter/acquire/reduction/event forms.
`graphics_engine_reports_follow_real_pixels_and_write_exact_records` submits
literal B197 A/B/C/D packets after a real indexed/textured driver draw. It checks
four-byte release and 16-byte payload/timestamp records, untouched surrounding
bytes, nonzero time, independent framebuffer pixels and exactly one GPU readback
with no draw replay during the subsequent capture call.

`target_zero_selection_ignores_only_documented_inactive_target_fields` tests the
independently encoded identity mapping, all-inactive-seven mapping and 1024
bounded deterministic inactive mappings. It also tests all unsupported counts,
active remaps, reserved bits and an unaligned offset without changing valid state.
`inactive_color_target_selectors_preserve_pixels_and_pipeline` exercises literal
GPFIFO/B197 commands and two indexed textured draws with different inactive
mappings. Both outputs match the independent pixel oracle, with no extra shader
read, shader module or pipeline allocation on the second draw.

`hdr.asm`/`hdr.bin` are independently assembled GM107 fragments exporting
(2, 0.5, 0.25, 1), with the same verified SPH/scheduling recipe as `quarters`.
The R11G11B10 guest result is exactly `0x681c0400`; background (0, 2, 0) is
`0x00200000`, demonstrating values above UNORM range in both upload and rendering.
The native HDR driver test exercises pitch and block-linear storage, independent
GOB padding checks, 32 warm deferred draws, a packed CPU-after-GPU patch, a padding
patch and injected atomic-publication failure/retry without draw replay. A
same-backing HDR-to-UNORM transition checks target/pipeline replacement and shader
module reuse. The diagnostic image is checked with independent triangle coverage;
its values are clamped for display while guest HDR bits remain unchanged.

The HDR scenario passed native Windows D3D12/Vulkan and WSL Vulkan. Its initial
OpenGL attempt failed because that backend does not expose native renderability;
that case is unavailable there, not a successful execution or a silent fallback.
A test-only premature completion-token retirement was corrected before the
successful native runs; completed draw/publication behavior was not weakened.

The HDR change passed 435 default-feature CPU tests (194 GPU cases ignored by
that command), 430 no-default-feature tests, strict package Clippy, formatting
and a live all-target workspace check. Explicit execution passed all 194 GPU
scenarios on WSL Vulkan and all 13 driver scenarios on native Windows D3D12 and
Vulkan. OpenGL passed the other 12 driver scenarios; the unsupported HDR case was
explicitly excluded after its capability failure, not counted as passed.

Disabled-attachment tests initialize every secondary FORMAT, check stored state
and unchanged primary rasterization, reject unsupported formats and unaligned
writes, and verify engine/channel isolation. Disabling then re-enabling target
zero does not restore stale renderability while disabled. The corresponding
GPFIFO driver scenario renders one indexed textured image after all secondary
targets are disabled, then rejects a color draw after disabling target zero,
without changing guest bytes, submitting another draw, or replaying work on retry.

MME color-state tests use independently assembled register-derived READ operands,
all 72 admitted field locations, overwritten values, uninitialized fields,
reserved holes and boundary addresses. Integration checks verify that rejected
B197 writes do not corrupt the live read bank. The actual GPU driver scenario
reads the guest-written target dimension, emits a red-only color mask, renders
an indexed triangle with independently predicted pixels, then rejects an
uninitialized read while preserving the prior pending draw for publication.


Empty exclusive window rectangles are validated independently from enable/reset
state. The CPU regression covers all eight initialized rectangles, missing
bounds, an empty X or Y interval, one nonempty exclusion, and incompatible modes.
The GpuDriver/GPFIFO clear regression verifies pitch and block-linear RGBA8 guest
bytes and padding against the scalar oracle; a subsequent unsupported active clip
fault leaves the earlier pending clear publishable without replay. Nine clear
scenarios executed on each of WSL Vulkan, WSL OpenGL, native Windows D3D12 and
native Windows Vulkan (36 actual passes). The package CPU suites passed 439 tests
with default features (197 GPU scenarios ignored by that ordinary run) and 434
without default features. Package strict all-target Clippy and formatting passed.
These synthetic clear results do not establish an original-game draw or frame.


The initialized B197 window-clip disable state is separately tested with eight
zeroed rectangles and no enable/mode writes before a guest clear. The supported
active empty-exclusive case now explicitly enables clipping; nonempty active
rectangles still fault and retain earlier accepted output. Ten clear scenarios
executed on each of WSL Vulkan, WSL OpenGL, native Windows D3D12 and native Windows
Vulkan (40 passes, zero skips). The CPU suites passed 439 tests with default
features (198 GPU scenarios ignored in that ordinary run) and 434 without default
features. Strict package all-target Clippy and whole-workspace formatting passed.
This models a source-backed initialized-context value, not unspecified power-on
state or general active window clipping.


Initialized single-sample mode is covered by the channel clear scenario with the
literal `0x15d0` write removed. Other missing controls still fail and diagnostics
include the precise required method plus bounded clear state. The CPU suites
passed 439 default-feature tests (198 GPU scenarios ignored in that ordinary run)
and 434 without default features. Ten clear GPU scenarios passed on each of WSL
Vulkan/OpenGL and native Windows D3D12/Vulkan (40 actual passes, zero skips).
Package all-target strict Clippy and workspace formatting passed. Initialization
still leaves unrelated absent fixed controls unresolved.


The GpuDriver initialized-channel regression removes explicit global TRUE,
multisample-mode and base-layer writes from the caller's B06F stream. Pitch and
tiled clears produce the independent expected guest bytes. A later unsupported
global FALSE faults without replaying or dropping the pending clear. The raw
engine regression still rejects a missing global gate. The package passed 439
CPU tests with default features (198 GPU scenarios ignored by that ordinary run)
and 434 without default features. All 198 GPU scenarios executed on WSL Vulkan;
10 clear scenarios additionally passed on WSL OpenGL. Native Windows D3D12 and
Vulkan each executed 10 clear and 15 driver scenarios, all passing with zero
skips. Package strict all-target Clippy and workspace formatting passed. The
global TRUE starting policy is explicit software-driver initialization, not
independent evidence of a Switch firmware reset value.


Pure-raster shadow replay is tested through literal B06F writes for the supported
fixed-state set and five disabled auxiliary controls. CPU tests cover known
capture, invalid-write preservation, replay substitution, same-value filtered
capture, changed filtered rejection and independent-channel missing history.
The GpuDriver GPU test replays deliberately invalid incoming words from earlier
valid captures, restores a red-only color mask, and executes a real indexed
Maxwell draw. The independent coverage/color oracle expects covered pixels
`[255,17,23,255]` and background `[11,17,23,255]`. A later changed filtered-mask
fault leaves exactly one draw and one final readback, with no command replay.
The CPU suites passed 440 default-feature tests (199 GPU scenarios ignored in
that ordinary run) and 435 without default features. All 199 GPU scenarios
executed on WSL Vulkan with zero failures/skips; the new scenario also executed
on WSL OpenGL. Native Windows D3D12 and Vulkan each executed all 16 driver GPU
scenarios, with zero failures/skips. Package strict all-target Clippy and
workspace formatting passed. No original-game frame is inferred from these
synthetic pixel comparisons.


Filtered capture of the supported pure-raster whitelist is validated against the
pinned reference semantics. The indexed driver pixel test now starts in
TRACK_WITH_FILTER, captures fixed raster controls and a changed red-only mask,
and replays them despite invalid incoming replacements. Changed filtered mask
history also survives passthrough writes. Missing viewport history still faults
after that accepted draw and publication preserves its pixels without replay.
The earlier filtered-mask rejection case is superseded by this supported capture
behavior; negative GPFIFO cases retain uncaptured-state, excluded attachment
address and excluded action failures. No capture defaults or general physical
filter membership are inferred. CPU tests passed 441 with default features (199
GPU scenarios ignored by that ordinary run) and 436 without default features.
All 199 GPU scenarios executed on WSL Vulkan. WSL OpenGL additionally executed
the driver replay scenario and two GPFIFO shadow scenarios. Native Windows
D3D12 and Vulkan each executed 16 driver and 44 GPFIFO GPU scenarios. These
explicit GPU runs had zero failures/skips. Strict package all-target Clippy and
workspace formatting passed.


The disabled iterated-blending regression validates both enable bits, byte-wide
count bounds, count-before-disable ordering, invalid-write preservation and
filtered shadow capture/replay. The indexed textured GPU scene changes the
inactive pass count from 255 to one without changing pixels, shader modules,
pipelines or texture uploads. An active-iteration request faults without
replaying or discarding the previous pending draw. CPU suites passed 442 tests
with default features (200 GPU scenarios ignored in that ordinary run) and 437
without default features. All 200 GPU scenarios executed on WSL Vulkan, and the
new scenario also passed on WSL OpenGL. Native Windows D3D12 and Vulkan each
executed 17 driver scenarios successfully. The initial Windows launch attempt
failed in WSL interoperability before test startup (`UtilAcceptVsock`, 110);
a subsequent launch through the live interoperability endpoint executed the
actual binaries, with zero GPU failures/skips. Strict package all-target Clippy
and workspace formatting passed. Active iterated blending remains unsupported.


Inactive target-bank coverage exercises all seven blend enables and all sixteen
RGBA mask combinations per inactive target, including alignment, reserved bits,
last-bank bounds and shadow replay. The independently assembled two-entry MME
fixture programs each complete family through GpuDriver/GPFIFO, uses filtered
capture and replay with invalid incoming replacements, and renders an indexed
asymmetric texture scene. Changing only inactive target state preserves pixels,
shader modules, pipelines and texture uploads. An invalid macro-generated mask
fails at RT1 without replaying or discarding the earlier pending draw. The error
diagnostic preserves its previous prefix while adding the actual B197 byte
method in hexadecimal. CPU suites passed 443 default-feature tests (201 GPU
scenarios ignored in that ordinary run) and 438 without default features.
All 201 GPU scenarios executed successfully on WSL Vulkan. The new MME scenario
also passed on WSL OpenGL. Native Windows D3D12 and Vulkan each executed 18
driver, five blend-equation and twelve depth scenarios (35 per backend), all
passing. WSL interoperability launch failures were isolated from test results;
final native runs used hash-verified copies in a Windows-local directory and a
single native batch with per-binary logs and a fresh completion receipt. Two
intermediate Vulkan suite failures were diagnostic-prefix assertion mismatches;
the compatible final prefix passed the complete suite. Strict package all-target
Clippy and workspace formatting passed. Additional color attachments are still
unsupported; inactive-bank retention does not implement MRT rendering.


Unused fixed-blend constants are tested as four independent raw component
words, including signed zero, infinity and a NaN payload, with exact shadow
capture/replay. Constant-consuming factors remain rejected. A real indexed
textured draw uses ordinary ONE/ZERO blending; 32 subsequent hardware draws
change only the unused words and retain identical expected pixels with no new
pipeline, shader module, texture upload, readback or synchronization point before
final publication. This is 32 queue submissions, not one combined GPU batch or
an allocation-free claim. CPU suites passed 444 default-feature tests (202 GPU
scenarios ignored in that ordinary run) and 439 without default features. All
202 GPU scenarios executed on WSL Vulkan. The new scenario also passed on WSL
OpenGL. Native Windows D3D12 and Vulkan each executed 19 driver GPU scenarios,
with all passes and no skips. A failed WSL launch was not counted: final results
came from the actual Windows-local executables with matching SHA-256 hashes,
fresh native batch receipt and per-backend Rust test summaries. Strict package
all-target Clippy and workspace formatting passed. No original-game frame is
inferred from these synthetic texture scenes.

The initial-command capture suite validates independently encoded little-endian
records, a method packet split across two entries, all truncated prefixes,
invalid headers/counts/lengths, 40-bit command-range overflow, aggregate work
limits, extra trailing bytes, short guest reads, nonreplacement and cleanup of
partial records. The GPU regression submits a real B197 RGBA8 clear followed by
a command entry aliasing the cleared target. The snapshot contains the original
NOP; actual execution correctly republishes/reacquires the changed bytes and
faults on the new header. Both successful capture and injected filesystem failure
must preserve the clear and identical guest pixels. This is a synthetic
command-acquisition regression, not an original-game frame.
Neutral depth-bias tests preserve resolved raster state and reject nonzero,
nonfinite and unaligned inputs without mutating the valid state.
The capture/neutral-bias validation ran 450 default-feature CPU tests and 445
without default features. The full Vulkan regression executed 203 ignored GPU
scenarios, followed by the additional neutral-bias GPU scenario; none failed or
skipped. Both new scenarios also executed on WSL OpenGL and native Windows D3D12
and Vulkan. Native Windows additionally ran all 20 driver GPU scenarios per
backend. Strict package all-target Clippy and workspace formatting passed.

Fragment-barrier validation checks two explicit completion waits after an indexed
textured draw, with no additional queue submission, readback or guest framebuffer
publication. Guest RAM remains at its initial background while the resident image
is newer. The all-lines WFI invalidate forces a texture refresh on the next draw
without recompiling the shader, and final publication matches the independent
triangle/UV oracle. Disabled pixel controls and correctly typed inactive optional
shader stages are exercised in the same hardware fixture. CPU tests reject
system-memory/per-tag barrier operands, active pixel controls, enabled optional
stages, wrong types and unaligned offsets without corrupting retained state.
The fragment-barrier/control validation ran 452 default-feature CPU tests, 447
without default features, and all 205 explicit Vulkan GPU scenarios. The new
scenario also passed on WSL OpenGL; all 21 driver GPU scenarios passed on native
Windows D3D12 and Vulkan. No executed case failed or skipped. Package all-target
strict Clippy and workspace formatting passed.

RGBA8 sRGB tests render an independently assembled .25/.5/.75/1 fragment through
GPFIFO into `0xd6`, expecting encoded RGB 137/188/225 (one-step conversion
tolerance) and alpha 255. The independent triangle oracle checks coverage and
unchanged background exactly. A warm draw causes no target/pipeline/module
allocation or readback; rebinding `0xd5` produces 64/128/191 with the same cached
shader decode. Pitch and block-linear sRGB clear tests verify encoded storage,
linear alpha, untouched GOB padding, exact CPU-after-GPU byte patches, failed
atomic publication followed by successful retry without clear replay, and a
subsequent GPU clear superseding the earlier CPU patch.
The sRGB delivery passed 453 default-feature CPU tests, 448 without default
features and all 207 explicit Vulkan GPU scenarios. Both new GPU scenarios also
passed on WSL OpenGL. Native Windows executed all 22 driver and 11 clear scenarios
on each of D3D12 and Vulkan, with no failures or skips. Strict all-target package
Clippy and workspace formatting passed. An obsolete negative test was changed
from now-supported `0xd6` to still-unsupported SNORM `0xd7`; the sRGB conversion
oracle retains the independent transfer values and its documented one-step RGB
tolerance rather than adopting backend readback values as expectations.

The FLOAT16x4 fixture uses exact independent binary16 positions and a separately
assembled vertex shader exporting constant UV through guest instructions. An
analytic triangle/nearest-texel oracle verifies coverage and sampled color, then
checks accumulated coverage after a guest position mutation. Cold vertex upload
is 24 bytes; 32 unchanged draws add no vertex upload, pipeline/module creation,
readback or completion wait. The mutation uploads another 24 bytes without
redecoding the shader. Rebinding a vertex program which reads inactive UV faults
without drawing from the previous pipeline. CPU tests additionally check all
thirty inactive generic descriptors and fifteen unreferenced stream formats,
reserved bits, strict active-profile rejection, eight-byte bounds and Naga
validation of the omitted-input interface.
The half-position delivery passed 455 default-feature CPU tests, 450 without
default features and all 208 explicit Vulkan GPU scenarios. The new fixture also
passed on WSL OpenGL and all 23 driver scenarios ran on each native Windows
backend. The 32-draw warm measurement used 32 queue submissions and zero added
vertex uploads, readbacks or waits. Caller-thread allocations/reallocations in
this debug workload were 1574/33 on WSL Vulkan, 1568/128 on WSL OpenGL, 1427/39 on
Windows D3D12 and 1589/39 on Windows Vulkan. These are observed host API costs,
not allocation-free rendering, combined GPU batching or game performance data.

The initialized-context fixture deliberately omits source-verified baseline
register writes while retaining guest programs, resources, geometry, viewport,
scissor and draw commands. Indexed textured pixels still match the independent
oracle. Changing subtile fractions does not recreate pipelines/modules or force
readback. A later partial sample mask faults without losing either earlier draw;
publication after that fault preserves the valid prefix. CPU tests challenge
all four masks, absent masks, color-target enable/reserved bits, subtile width
bounds, shadow replay and unaligned offsets. An opt-in shader diagnostic run
produced byte-identical copies of the independently assembled vertex/texture
fixtures; injecting a non-directory destination left the same GPU test passing.
The initialized raster/subtile validation passed 457 default-feature CPU tests,
452 without default features and all 209 explicit Vulkan GPU scenarios. The new
scenario passed on WSL OpenGL; all 24 driver GPU scenarios passed on each native
Windows D3D12 and Vulkan backend, without failures or skips. Strict package
all-target Clippy, workspace formatting and `cargo check -p wl-nvdrv --locked`
passed. These package results do not establish original-game shader or frame
compatibility.
