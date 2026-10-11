# Graphics source assessment

Sources inspected on 2026-10-11. Commit links identify the source considered;
dependency checksums and actual selected versions are recorded in `Cargo.lock`.

## Adopted implementations

| Implementation | Pinned source | Decision |
|---|---|---|
| wgpu 26.0.1 / Naga 26 | [`0c978d0d46c7eaf68f6d1ecf2ea1bd03a96b7c47`](https://github.com/gfx-rs/wgpu/tree/0c978d0d46c7eaf68f6d1ecf2ea1bd03a96b7c47) | MIT/Apache-2.0 native backend and controlled WGSL validation. wgpu declares Rust 1.84, compatible with this workspace's Rust 1.85 floor. |
| tegra_swizzle 0.4.0 | [`6fb9dfef9e6630bd155632802b3ead5ce5c656c8`](https://github.com/ScanMountGoat/tegra_swizzle/tree/6fb9dfef9e6630bd155632802b3ead5ce5c656c8) | MIT safe Rust Tegra X1 tiling. Reuse the library behind bounded geometry/input checks instead of maintaining another converter. |
| stats_alloc 0.1.10 | [instrumented allocator source](https://docs.rs/stats_alloc/0.1.10/src/stats_alloc/lib.rs.html) | MIT development-only process-wide Rust allocation measurement; no custom unsafe allocator in this project. |

wgpu remains actively developed: inspected upstream trunk was
[`1f3a1fbb4747a13cd63eeeeb1784788e295c21b9`](https://github.com/gfx-rs/wgpu/commit/1f3a1fbb4747a13cd63eeeeb1784788e295c21b9).
The dependency deliberately uses the older compatible release. The inspected
tegra_swizzle main revision was
[`d7ed318ce233a4d557d4a2f207ed5dc04ed813c0`](https://github.com/ScanMountGoat/tegra_swizzle/commit/d7ed318ce233a4d557d4a2f207ed5dc04ed813c0),
with repository activity in July 2026. The release algorithm handles complete GOBs
and partial edge GOBs, while the surface API additionally handles mip/array layout.
The current wrapper intentionally exposes only a bounded single 2D mip.

The wgpu 26 [`queue implementation and API contract`](https://github.com/gfx-rs/wgpu/blob/0c978d0d46c7eaf68f6d1ecf2ea1bd03a96b7c47/wgpu/src/api/queue.rs)
establishes that writes copy input into staging before returning, and execute before
the next submission. This determines both upload borrow lifetimes and the command
ordering restriction. The [`device API`](https://github.com/gfx-rs/wgpu/blob/0c978d0d46c7eaf68f6d1ecf2ea1bd03a96b7c47/wgpu/src/api/device.rs)
provides polling/error scopes. Naga's inspected frontends accept WGSL, GLSL and
SPIR-V; there is no Maxwell instruction frontend.

## Maxwell and Switch references

The Ryubing repository endpoints attempted from this environment were unavailable
(GitHub 404; self-hosted endpoint unreachable). Their current source/maintenance
could not be independently assessed. An archived original Ryujinx source snapshot,
[`44862dce3ef307b12366376ada3fa0db693bfaf9`](https://git.axenov.dev/Museum/ryujinx/src/commit/44862dce3ef307b12366376ada3fa0db693bfaf9),
was retrieved and inspected directly, including its MIT `LICENSE.txt`. It is an
architectural reference, not evidence of a maintained dependency or a validated
Wanderlust integration.

The inspected `src/Ryujinx.Graphics.Gpu/Engine/GPFifo/GPFifoProcessor.cs` maintains
packet state across input buffers and distinguishes incrementing, nonincrementing,
increment-once and immediate methods. `GpuChannel.cs` keeps channel state and binds
a separate memory manager. These are source-side concepts absent from the current
execution command stream.

`src/Ryujinx.Graphics.Gpu/Memory/MemoryManager.cs` distinguishes GPU virtual mapping
from physical backing, returns contiguous borrowed spans, and gathers fragmented
regions into allocated storage. This supports preserving explicit address spaces
and refusing an impossible contiguous borrow rather than hiding an allocation.

`src/Ryujinx.Graphics.Shader/Translation/Translator.cs` decodes compute and graphics
shader inputs separately, parses graphics headers and invokes a dedicated Maxwell
decoder before intermediate processing. Reusing that compiler requires its C#
runtime/dependency surface and a verified semantic output/binding adapter. Feeding
its output into Naga would also require checking the accepted SPIR-V subset and
wgpu limits. No decoder was copied or linked; a new full shader compiler is also
absent. This preserves the decoder/lowering/backend separation.

[Switchbrew's GPU overview](https://switchbrew.org/wiki/GPU) identifies the hardware
and links engine/register references. NVN operates above channel/method submission;
synthetic host commands establish no NVN or game ABI compatibility.

## Texture compatibility limits

[`wwylele/astc-decode`](https://github.com/wwylele/astc-decode) provides an
Apache-2.0 Rust software LDR ASTC decoder. Its repository last push observed through
the GitHub API was June 2021. It is a candidate for a separately validated fallback,
not an adopted codec; HDR behavior and performance were not established. ASTC
availability is probed per adapter. Native D3D12 and Vulkan must not be assumed to
expose identical compression support. Decoding to RGBA adds CPU conversion and
upload bandwidth; BCn transcoding introduces a separate quality/cost decision.

Only permissively licensed dependencies were introduced. No GPL emulator dependency,
proprietary shader, asset, game dump or decryption material is included. The root
repository currently contains no license file; this assessment does not assign a
new license to the project's own code or replace dependency notice obligations.
