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
The layout wrapper exposes a bounded single 2D mip; `TextureDesc` now composes it
using the inspected surface implementation's successive mip sizes and block-height
reduction. Layer alignment and depth packing remain unsupported.

The wgpu 26 [`queue implementation and API contract`](https://github.com/gfx-rs/wgpu/blob/0c978d0d46c7eaf68f6d1ecf2ea1bd03a96b7c47/wgpu/src/api/queue.rs)
establishes that writes copy input into staging before returning, and execute before
the next submission. This determines both upload borrow lifetimes and the command
ordering restriction. The [`device API`](https://github.com/gfx-rs/wgpu/blob/0c978d0d46c7eaf68f6d1ecf2ea1bd03a96b7c47/wgpu/src/api/device.rs)
provides polling/error scopes. Naga's inspected frontends accept WGSL, GLSL and
SPIR-V; there is no Maxwell instruction frontend.

## Maxwell and Switch references

NVIDIA's MIT-licensed [B06F class definitions](https://github.com/NVIDIA/open-gpu-doc/blob/9fdf5c4062007929d9f4e6cbad9c9771fe61b880/classes/host/clb06f.h)
at revision `9fdf5c4062007929d9f4e6cbad9c9771fe61b880` define method address,
subchannel, payload count, immediate data and secondary opcodes. They provide the
field specification for the independent `fifo` decoder; no header code was copied.
The [libnx channel interface](https://github.com/switchbrew/libnx/blob/feebd026ca0f5dcc2119f46ad8e0d16ad3dd4973/nx/include/switch/nvidia/gpu_channel.h)
at revision `feebd026ca0f5dcc2119f46ad8e0d16ad3dd4973` separates GPFIFO entries,
channel submission and fences. The current decoder handles pushbuffer methods,
not those channel/service operations. Method-span overflow is rejected as a
Wanderlust validation policy; hardware fault behavior was not established.

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
now adopted with the explicit subset and validation described below. ASTC
availability is probed per adapter. Native D3D12 and Vulkan must not be assumed to
expose identical compression support. Decoding to RGBA adds CPU conversion and
upload bandwidth; BCn transcoding introduces a separate quality/cost decision.

Only permissively licensed dependencies were introduced. No GPL emulator dependency,
proprietary shader, asset, game dump or decryption material is included. The root
repository currently contains no license file; this assessment does not assign a
new license to the project's own code or replace dependency notice obligations.

## Compressed texture implementation

[`astc-decode` 0.3.1](https://github.com/wwylele/astc-decode/tree/4c1a13de8c63693f52ab0593b062f8bb6c83c7af)
(Apache-2.0) now supplies the block decoder. Its RGBA callback and boolean validity
result fit bounded block-at-a-time conversion. The wrapper additionally rejects
HDR endpoint modes, multipartition blocks, and bounded void extents: the upstream
routine can otherwise emit an error color for unsupported endpoint modes while
returning success. Invalid blocks fail the upload; magenta is not accepted as a
successful fallback. Assertions in malformed dependency input are caught at the
conversion boundary under the default unwind profile, publishing no partial image.
This is deliberately a subset of ASTC LDR 4×4, not general ASTC support.

[`texture2ddecoder` 0.1.2](https://github.com/K0lb3/texture2ddecoder-rs/tree/2b1f1f2f45595058f9523578c8924c2cb14d2127)
provides BC1 RGBA decoding via `decode_bc1a_block`. Its MIT/Apache-2.0 distribution
and MIT BC implementation permit reuse with retained notices. Packed `u32` pixels
are BGRA in little-endian bytes; the wrapper explicitly writes RGBA. The non-alpha
BC1 routine has different selector-three behavior and is unsuitable for BC1 RGBA.
The published crate fails to compile with all features disabled; its `alloc`
feature is enabled. No Crunch entrypoint or upstream ASTC decoder is invoked.
The latter lacks the validity distinction needed at this boundary.

The independent nonuniform ASTC reference block and expected bitmap pixels come
from [Google astc-codec](https://github.com/google/astc-codec/tree/9757befb64db6662aad45de09ca87cd6f599ac02/src/decoder/testdata),
Apache-2.0; revision, original-file hashes and license are retained in the test
fixtures. Void extent and LDR endpoint restrictions follow the
[Khronos Data Format Specification 1.3.1](https://registry.khronos.org/DataFormat/specs/1.3/dataformat.1.3.html).
No ASTC encoder was substituted for a decoder.

Native BC upload follows the wgpu 26 utility implementation's physical mip extent
rounding. Native ASTC is intentionally not enabled yet: one validated software path
establishes portable behavior on D3D12 without claiming that native hardware
acceptance validates LDR input. BC1 native selection depends on the negotiated BC
feature and a block-aligned base extent; other extents preserve their exact sampling
domain through RGBA fallback. BC3 remains geometry-only.

## Maxwell texture fields and shader compiler assessment

NVIDIA [clb197.h](https://github.com/NVIDIA/open-gpu-doc/blob/9fdf5c4062007929d9f4e6cbad9c9771fe61b880/classes/3d/clb197.h)
and [clb197tex.h](https://github.com/NVIDIA/open-gpu-doc/blob/9fdf5c4062007929d9f4e6cbad9c9771fe61b880/classes/3d/clb197tex.h),
MIT, define the implemented B197 method offsets, modern BL texture header and
sampler fields. Literal test words are specification-derived; no descriptor
encoder generates the validation reference. The archived Ryujinx `TextureDescriptor`,
`SamplerDescriptor`, `FormatTable` and texture pool sources at the revision above
were inspected as a second interpretation reference. Legacy TEXHEAD and TEXHEADV2
layouts are distinct and are not decoded as the modern BL format.

[`shader-compiler-rs` f9f0fbc](https://github.com/DCNick3/shader-compiler-rs/tree/f9f0fbc998279a663d0bc98937a0bde880efc8db)
was assessed at source level. The library takes a 0x50-byte shader program header
plus Maxwell instruction data; its CLI additionally strips a 0x30-byte NVN header.
The C++ bridge emits SPIR-V 1.4; GLSL output involves an additional SPIRV-Cross step.
Graphics stage mapping excludes VertexA and compute in the wrapper. The inspected
environment returns zero for constant-buffer values, assumes 2D textures and throws
for unimplemented pixel-format queries. A raw header cast and missing structured
exception boundary require a bounded input/error adapter. Hardcoded capabilities
and combined image/sampler bindings require adapter negotiation and Naga/wgpu
binding adaptation, not direct unchecked shader import.

Although the README declares MPL-2.0, vendored files such as
[`program_header.h`](https://github.com/DCNick3/shader-compiler-rs/blob/f9f0fbc998279a663d0bc98937a0bde880efc8db/vendor/shader_compiler/program_header.h)
retain GPL-2.0-or-later SPDX identifiers. This discrepancy prevents treating the
entire distribution as an established permissive dependency. No source was imported
or linked. A viable mature-compiler integration requires resolving the actual file
licenses, then implementing bounded SPH input, constant-buffer/descriptor callbacks,
capability control and separate texture/sampler bindings before Naga validation.
Naga remains a host IR/compiler component, never a Maxwell machine-code decoder.

## Independently assembled constant-fragment experiment

The tiny fragment translator follows the
[envytools gm107 ISA table](https://github.com/envytools/envytools/blob/f102b82381f3f11cee113d16374c87091db039d9/envydis/gm107.c)
at revision `f102b82381f3f11cee113d16374c87091db039d9` (MIT), independently
cross-checked against devkitPro UAM
[`5a5afc2b`](https://github.com/devkitPro/uam/tree/5a5afc2bae8b55409ab36ba45be63fcb73f68993).
UAM's Mesa instruction emitter is MIT; its header and compiler interface are zlib.
Those sources establish MOV32I operand bits, unconditional EXIT, PS output register
mapping, and six-cycle MOV scheduling. The
[NVIDIA SPH specification](https://download.nvidia.com/open-gpu-doc/Shader-Program-Header/1/Shader-Program-Header.html)
defines the header fields. Its field enumeration and UAM agree on PS type two;
the introductory prose swaps the two type names and is insufficient alone.

Actual envytools assembler/disassembler binaries built from that pinned revision
produced two independent synthetic fixtures. Their source assembly, header
construction, exact hashes and reproduction commands are retained in the fixtures.
No assembler source or emulator compiler is linked. This small constant-export
slice was selected after assessing the mature compiler's license/API obstacles;
it does not replace a full decoder. The next shader integration needs constant
buffer loads, TEX descriptor handles, stage interfaces and explicit capability
lowering rather than extending the constant-only experiment into unsupported claims.


The guest-backed LDC path cross-checks B197 selector/binding fields against the
pinned NVIDIA header and instruction width/addressing against MIT Mesa/UAM
`nv50_ir_emit_gm107.cpp` (`emitLDC`, `emitLDSTs`, `emitMOV`). B32 is size encoding
four in bits 50:48 (`0xef94...`); RZ-based immediate LDC uses signed byte offsets.
The independently assembled `ldc.asm`/`ldc.bin` fixture is produced with the same
pinned envytools tools as the constant fragments and disassembled before decoding.
Write/read/wait field placement is independently specified by envytools' gm107
scheduler table and the Mesa scheduler. No external compiler runs at application
runtime and no GPL implementation was copied.

For TEX.B/IPA, the same MIT Mesa emitter independently establishes indirect TEX
opcode `0xdeb80000`, LZ mode one in bits 38:37, 2D dimension one in 30:29,
RGBA mask 15 in 34:31, coordinate base in 15:8 and handle GPR in 27:20.
`emitIPA`, `TargetGM107::isBarrierRequired` and UAM `SetPsImap` establish
screen-linear generic input fields and the required variable-latency write barriers.
Mesa's MIT [`nvc0_tex.c`](https://github.com/chaotic-cx/mesa-mirror/blob/e7959c6009d85d278585b2896495b2b3046d461b/src/gallium/drivers/nouveau/nvc0/nvc0_tex.c)
at `e7959c6009d85d278585b2896495b2b3046d461b` uses combined TIC-low20/TSC-high12
handles; these are not invented descriptor IDs. Its API-level 64-bit residency
marker is separate from the 32-bit TEX operand. The pinned NVIDIA pool/TIC/TSC
definitions remain the source of descriptor semantics.

The TEX fixture uses envytools with one development-only grammar repair: in
`envydis/gm107.c`, the TEX.B dummy token `N("0x0")` is named `N("zero")` so envyas
can parse it as a keyword rather than a numeric expression. No encoding fields
change. The original unmodified envydis disassembles the resulting bytes as
`tex b lz ... 0x0 t2d 0xf`; Mesa independently confirms those fields. Neither
assembler nor patched source is linked or needed at runtime.

B197 pixel selection uses the pinned NVIDIA `clb197.h` program-region and
`SET_PIPELINE_*` definitions. Pixel pipeline index five is distinct from constant
binding group four; the explicit pipeline-binding field selects the group. Program
offsets are added as checked GPU byte addresses. The SPH/code reader imposes its
own termination/size budget because those method definitions provide no binary
length. Register allocation is checked against the actual supported dataflow.


Vertex ALD/AST encodings are cross-checked against the pinned envytools gm107
attribute tables and MIT UAM/Mesa `emitALD`/`emitAST`: width-minus-one at bits
48:47, direct attribute byte offset at 29:20, RZ address/vertex selectors,
GPR group at 7:0. NVIDIA's SPH VTG table and zlib UAM `SetVtgImap`/`SetVtgOmap`
establish generic masks and position sysvalues 28..31. The packed VTG output map
starts at byte 50, not byte 52. Actual pinned envyas produced and unmodified
envydis decoded both vertex fixtures; no production assembler dependency exists.


The pinned NVIDIA B197 header establishes stream A stride/enable and inclusive
limit addresses, attribute A component-size encodings one/four and FLOAT encoding
seven. Generic attribute slots 0/1 correspond to shader byte addresses 0x80/0x90,
as independently specified by UAM's `nv_attributes.h`. FIFO fixture headers divide
byte method addresses by four; method arguments remain unmodified little-endian
words. Vertex input and program caches share the previously validated GPU mapping
and write-journal contract rather than introducing another guest memory owner.


The hardware draw/target/raster subset uses the same pinned MIT NVIDIA B197 header,
including compact draw 0x1214 and color-target 0x0800 fields. MIT Mesa
[`nvc0_state_validate.c`](https://github.com/chaotic-cx/mesa-mirror/blob/e7959c6009d85d278585b2896495b2b3046d461b/src/gallium/drivers/nouveau/nvc0/nvc0_state_validate.c)
at the already pinned revision independently distinguishes linear target byte pitch
from tiled pixel width in framebuffer setup. Its viewport scale/translation setup
cross-checks the affine guest-to-window relationship. The shader fixtures continue
to come from pinned envyas/envydis, with no commercial shader input.

The locked [`wgpu-hal 26.0.6 GLES command source`](https://docs.rs/crate/wgpu-hal/26.0.6/source/src/gles/command.rs)
casts viewport rectangles to integers in `set_viewport`. WSL execution exposed that
loss for a fractional extent that passed D3D12/Vulkan. The accepted hardware subset
therefore requires integral viewport rectangles, instead of silently accepting
backend-dependent geometry. This is a host-backend restriction, not Maxwell behavior.
No new dependency or shader compiler code is imported for this hardware slice.

### Indexed command corroboration

NVIDIA `clb197.h` at `9fdf5c4062007929d9f4e6cbad9c9771fe61b880`
(MIT) defines index methods 0x17c8..0x17e0 and BEGIN/END. Mesa
[`nvc0_vbo.c`](https://github.com/chaotic-cx/mesa-mirror/blob/e7959c6009d85d278585b2896495b2b3046d461b/src/gallium/drivers/nouveau/nvc0/nvc0_vbo.c)
(MIT), `nvc0_draw_elements` and pre-Turing index descriptor programming, independently
corroborates inclusive byte limits, U16/U32 format encoding, signed base vertex,
first-element/count and BEGIN/END ordering. No Mesa implementation was vendored.

### Global blend and channel fields

The pinned NVIDIA
[`clb197.h`](https://github.com/NVIDIA/open-gpu-doc/blob/9fdf5c4062007929d9f4e6cbad9c9771fe61b880/classes/3d/clb197.h)
(MIT) specifies SET_BLEND_SEPARATE_FOR_ALPHA, the global color/alpha ADD equations
and OGL/D3D coefficient aliases, SET_BLEND(0), SET_BLEND_STATE_PER_TARGET and
SET_CT_WRITE(0). The implementation lowers only the four documented coefficients
listed in execution.md to wgpu 26 blend factors; it imports no external compiler
or emulator implementation. The output oracle evaluates the equations separately
from shader translation and quantizes between draws on the UNORM attachment.

## GPFIFO channel boundary references

- NVIDIA open-gpu-doc, MIT, revision
  `9fdf5c4062007929d9f4e6cbad9c9771fe61b880`,
  [classes/host/clb06f.h](https://github.com/NVIDIA/open-gpu-doc/blob/9fdf5c4062007929d9f4e6cbad9c9771fe61b880/classes/host/clb06f.h),
  GPFIFO entry definitions 154–176: eight bytes, GET/FETCH/PRIV/LEVEL/LENGTH/SYNC
  and control opcodes. This is the source of the accepted wire fields; it does not
  justify pretending unsupported scheduling or control semantics are implemented.
- libnx, ISC, revision `feebd026ca0f5dcc2119f46ad8e0d16ad3dd4973`,
  [gpu_channel.c](https://github.com/switchbrew/libnx/blob/feebd026ca0f5dcc2119f46ad8e0d16ad3dd4973/nx/source/nvidia/gpu_channel.c)
  and [ioctl.h](https://github.com/switchbrew/libnx/blob/feebd026ca0f5dcc2119f46ad8e0d16ad3dd4973/nx/include/switch/nvidia/ioctl.h).
  AppendEntry sets descriptor=start and high word |= flags|(num_cmds<<10),
  independently confirming the word count and two-word entry packing. Kickoff's
  ioctl flags/fence bookkeeping are separate from entry flags and are not adopted
  as GPU completion semantics. Its 0x800 queue size informs the bounded call policy,
  not a claim that every hardware channel has that limit.
- [Switchbrew GPU, revision 12795](https://switchbrew.org/w/index.php?title=GPU&oldid=12795)
  and [NV services](https://switchbrew.org/wiki/NV_services), inspected 2026-10-12:
  mapped command buffers and inline GPFIFO entries in channel submission. The
  NV-services page's informal allow_flush/is_push_buf bit names differ from the
  NVIDIA class names; those bits are rejected, not assigned speculative semantics.
- Ryujinx reference fork `alula/Ryujinx`, MIT, revision
  `4930dced25c0d9ec33096b0f2601d262daa477bf`,
  [GPFifoProcessor.cs](https://github.com/alula/Ryujinx/blob/4930dced25c0d9ec33096b0f2601d262daa477bf/src/Ryujinx.Graphics.Gpu/Engine/GPFifo/GPFifoProcessor.cs)
  retains method-count state across Process calls, independently corroborating
  packet continuation. [GPFifoDevice.cs](https://github.com/alula/Ryujinx/blob/4930dced25c0d9ec33096b0f2601d262daa477bf/src/Ryujinx.Graphics.Gpu/Engine/GPFifo/GPFifoDevice.cs)
  corroborates address/word-length extraction and gives WAIT distinct prefetch
  handling. It is a source reference, not a dependency or proof that WAIT is
  implemented here. No processor code was copied or vendored.

## Driver-context ABI and host completion

libnx ISC revision `feebd026ca0f5dcc2119f46ad8e0d16ad3dd4973`:

- [ioctl/nvchannel.c](https://github.com/switchbrew/libnx/blob/feebd026ca0f5dcc2119f46ad8e0d16ad3dd4973/nx/source/nvidia/ioctl/nvchannel.c),
  SubmitGpfifo and KickoffPb: IOWR group 'H', commands 08/1b, 24-byte header,
  inline versus ioctl2 entry storage, ignored pointer and input/output fence pair.
  [gpu_channel.c](https://github.com/switchbrew/libnx/blob/feebd026ca0f5dcc2119f46ad8e0d16ad3dd4973/nx/source/nvidia/gpu_channel.c)
  supplies flag bit 2 for hardware entries and separately bit 8 for fence increments.
  Only bit 2 is adopted; the input fence must be zero and no fence is manufactured.
- [ioctl/nvhost-as-gpu.c](https://github.com/switchbrew/libnx/blob/feebd026ca0f5dcc2119f46ad8e0d16ad3dd4973/nx/source/nvidia/ioctl/nvhost-as-gpu.c)
  and [address_space.c](https://github.com/switchbrew/libnx/blob/feebd026ca0f5dcc2119f46ad8e0d16ad3dd4973/nx/source/nvidia/address_space.c)
  establish distinct BindChannel, AllocSpace, MapBufferEx, UnmapBuffer and FreeSpace
  operations, fixed offset/page-size arguments and mapping subregions. Their full
  ioctl surface is not implemented: the typed adapter adopts only fixed complete
  reservations/mappings. [map.c](https://github.com/switchbrew/libnx/blob/feebd026ca0f5dcc2119f46ad8e0d16ad3dd4973/nx/source/nvidia/map.c)
  checks 4 KiB CPU backing alignment and size. The adapter's 4 KiB-only policy is
  deliberately narrower than available GPU page sizes; it is not nvmap emulation.
- [ioctl/nvhost-ctrl.c](https://github.com/switchbrew/libnx/blob/feebd026ca0f5dcc2119f46ad8e0d16ad3dd4973/nx/source/nvidia/ioctl/nvhost-ctrl.c)
  distinguishes syncpoint reads/increments/threshold waits and event operations.
  Host queue tokens do not implement these hardware/guest event semantics.

[wgpu 26.0.1 Queue completion documentation](https://docs.rs/wgpu/26.0.1/wgpu/struct.Queue.html#method.on_submitted_work_done)
and [Device::poll](https://docs.rs/wgpu/26.0.1/wgpu/struct.Device.html#method.poll)
(MIT/Apache-2.0) specify callbacks for previously submitted work and explicit
native polling/waiting. Completion tracking uses these APIs and the actual
SubmissionIndex. No sleeps, guessed timing or synthetic syncpoint increments
are used. The pinned NVIDIA B197 header above defines GLOBAL_COLOR_KEY at 1354,
ENABLE 0:0, FALSE=0 and TRUE=1; only the documented disable state is implemented.
External service-emulator implementations are not imported or vendored.

## Block-linear color target storage

The NVIDIA MIT header at revision
`9fdf5c4062007929d9f4e6cbad9c9771fe61b880`,
[`clb197.h`](https://github.com/NVIDIA/open-gpu-doc/blob/9fdf5c4062007929d9f4e6cbad9c9771fe61b880/classes/3d/clb197.h),
defines color target WIDTH 27:0, HEIGHT 16:0, MEMORY BLOCK_WIDTH 3:0,
BLOCK_HEIGHT 7:4, BLOCK_DEPTH 11:8, LAYOUT 12:12 and THIRD_DIMENSION_CONTROL
16:16. ONE_GOB has encoding zero; BLOCKLINEAR is zero and PITCH is one.
These definitions establish field positions, not alone the mode-dependent WIDTH
units or a universal target address-alignment rule.

Mesa Nouveau, MIT, revision `e7959c6009d85d278585b2896495b2b3046d461b`,
[`nvc0_state_validate.c`, nvc0_validate_fb](https://github.com/chaotic-cx/mesa-mirror/blob/e7959c6009d85d278585b2896495b2b3046d461b/src/gallium/drivers/nouveau/nvc0/nvc0_state_validate.c),
programs tiled width/height in pixels, tile mode and array stride in DWORDs;
pitch targets instead use byte pitch as WIDTH and MEMORY bit 12. This independently
corroborates the descriptor interpretation. No framebuffer implementation was copied.

The locked MIT dependency
[`tegra_swizzle` 0.4.0, swizzle module](https://docs.rs/tegra_swizzle/0.4.0/tegra_swizzle/swizzle/index.html)
provides both single-mip deswizzling and swizzling. Its source cites Tegra X1 TRM
v1.3 pages 1217–1218 for GOB addressing. Both public conversions allocate output;
partial logical swizzling zero-initializes unused storage. The surrounding renderer
therefore converts the full padded extent, overlays logical rows, and reconverts
that complete extent to retain every padding byte. No dependency fork or new
swizzle implementation is introduced.

The independent scalar test oracle in `tests/common/gob.rs` derives the byte
address from GOB/block equations and within-GOB bit interleaving, without calling
the dependency. Literal offsets and exact expected allocation bytes supplement
roundtrips, including partial edge GOBs and all six supported block heights.

## ZF32 depth attachments

The same pinned NVIDIA `clb197.h` defines ZT address/format/block size/array
pitch at `0xfe0..0xff0`, size at `0x1228..0x1230`, layer at `0x179c`,
selection at `0x1538`, depth test/write at `0x12cc/0x12e8`, and comparison
at `0x130c`. ZF32 is format `0x0a`. All eight OGL (`0x200..0x207`) and
D3D (`1..8`) comparison encodings map to the same eight comparisons.
Pinned Mesa `nvc0_state_validate.c::nvc0_validate_fb` programs the ZT dimensions
in pixels, array pitch in DWORDs, and the 2D single-layer size control.
The four-byte ZF32 storage uses the existing Tegra GOB conversion; it is not
interpreted as RGBA color during depth execution.

The locked `wgpu-core 26.0.1/src/conv.rs::is_valid_copy_dst_texture_format`
forbids CPU copies into Depth32Float. Its transfer validation requires
`DEPTH_TEXTURE_AND_BUFFER_COPIES` for direct readback; GLES lacks that flag.
Portable transfers therefore use R32Uint bit staging/export around real
Depth32Float render attachments. `wgpu-core/src/device/resource.rs` permits
an unfilterable float binding to a depth view; this avoids Naga 26's GLSL
rejection of `textureLoad` on `texture_depth_2d`. Initialization returns
`frag_depth`; export samples the actual depth attachment as `texture_2d<f32>`
and writes its bitcast into an integer color attachment. No CPU shadow depth
calculation substitutes for host depth testing.

## GPR floating arithmetic

Pinned MIT envytools `gm107.c` and UAM/Mesa `emitFADD`/`emitFMUL` independently
specify the GPR opcode bases `0x5c58` and `0x5c68`, destination at bit 0,
left source at bit 8 and right source at bit 20. The strict full-word masks
exclude predicates and all unimplemented modifiers. UAM's GM107 target latency
function assigns six cycles to non-F64 ADD/MUL, consistent with the accepted
stall-six fixtures. Newly retained assembly/binaries were produced by the same
pinned envyas and disassembled with unmodified envydis. The portable host f32
lowering is a bounded implementation, not evidence of exact Maxwell exceptional
arithmetic; WGSL offers no per-operation unfused-rounding control here.

## Perspective IPA and reciprocal

The pinned NVIDIA Shader-Program-Header specification at revision `9fdf5c4`
(linked above) defines PS PositionW at word five bit 31 and two-bit generic input
interpolation values Perspective=2 and ScreenLinear=3. Pinned MIT UAM/Mesa
`nv50_ir_emit_gm107.cpp::emitIPA` emits interpolation mode at bit 54, sample mode
at bit 52, generic byte attribute at bit 28 and OP_PINTERP multiplier GPR at bit
20. `emitMUFU` emits opcode 0x50800000, reciprocal function four at bit 20 and
source GPR at bit eight. Strict full-word masks exclude other fields.

Pinned Mesa `nv50_ir_from_nir.cpp` loads SV_POSITION.w, computes OP_RCP and passes
that register to OP_PINTERP. This corroborates the canonical sequence independently
of the envytools GM107 table. The WGSL position builtin defines fragment W as
interpolated reciprocal clip W; both stages use perspective interpolation with
center sampling. The explicit multiplier remains represented in the lowering.
No source implementation was copied. Host division does not establish exact
Maxwell MUFU approximation/exceptional behavior.

The [Direct3D functional specification, 7.18.7 Point Sample Addressing](https://microsoft.github.io/DirectX-Specs/d3d/archive/D3D11_3_FunctionalSpec.htm#PointSampleAddressing)
scales normalized coordinates to texel space and snaps to at least 16.8 fixed
point before choosing a texel; raster positions also undergo fixed-point snapping.
The perspective pixel oracle excludes one 1/256 texel fractional step around
nearest-sampling boundaries. Its three literal distinguishing samples are well
outside this uncertainty region. No backend-specific shader change is introduced.

## Integer IADD and LOP

The pinned MIT envytools `gm107.c` table at lines 2009/2015 and pinned UAM
`nv50_ir_emit_gm107.cpp::emitLOP/emitIADD` independently establish the accepted
GPR forms. After removing destination bits 7:0, left source 15:8 and right source
27:20, exact remaining words are IADD `0x5c10000000070000`, LOP.AND
`0x5c47000000070000`, OR `0x5c47020000070000`, XOR `0x5c47040000070000`.
LOP predicate output PT=7 is bits 50:48, separate from execution predicate P7.
Full-word masks reject carry/CC, inversion, alternate predicate modes and modifiers.
Pinned UAM `nv50_ir_peephole.cpp::ConstantFolding::expr` performs both S32/U32
addition on u32 bit patterns. WGSL concrete integer arithmetic wraps modulo the
bit width, so raw-u32 lowering preserves the supported addition semantics.
Independent unmodified envyas/envydis fixtures corroborate the encodings and
canonical six-stall scheduling; no source implementation was copied.

## Hardware attachment clearing

Pinned NVIDIA `clb197.h` at revision `9fdf5c4` defines COLOR_CLEAR_VALUE components
at `0x0d80..0x0d8c`, Z_CLEAR_VALUE at `0x0d90`, CLEAR_SURFACE_CONTROL at `0x10f8`
and CLEAR_SURFACE at `0x19d0`. Trigger bits are Z=0, stencil=1, RGBA=2..5,
MRT=9:6 and layer=25:10. Clear control bits select stencil mask (0), rectangle
(4), scissor0 (8) and viewport clip0 (12). Surface clip is independently defined
at `0x0ff4/0x0ff8` with origin in low16 and extent in high16.

Pinned MIT Mesa
[`nvc0_surface.c`](https://github.com/chaotic-cx/mesa-mirror/blob/e7959c6009d85d278585b2896495b2b3046d461b/src/gallium/drivers/nouveau/nvc0/nvc0_surface.c#L695)
explicitly excludes blend validation because draw color masks do not affect clear.
Its color/depth paths program surface clipping and target geometry independently
of shaders, viewport transforms and depth-test/write state. The same revision's
[`nvc0_screen.c`](https://github.com/chaotic-cx/mesa-mirror/blob/e7959c6009d85d278585b2896495b2b3046d461b/src/gallium/drivers/nouveau/nvc0/nvc0_screen.c#L1136)
sets clear controls to zero so draw scissors, viewport and stencil masks do not
constrain clears. These independently corroborate the selected bounded subset;
no implementation code was copied. Normalized-value and full-clip requirements
are Wanderlust limits, not asserted universal hardware restrictions.


## Face orientation and culling

Pinned NVIDIA [`clb197.h`](https://github.com/NVIDIA/open-gpu-doc/blob/9fdf5c4062007929d9f4e6cbad9c9771fe61b880/classes/3d/clb197.h#L3346)
defines the OGL cull enable, front orientation and cull-face byte methods at
`0x1918/0x191c/0x1920`, including CW/CCW and FRONT/BACK/BOTH enum values.
BOTH is a documented hardware mode but remains outside the implemented subset.

Pinned MIT Mesa [`nvc0_state.c`](https://github.com/chaotic-cx/mesa-mirror/blob/e7959c6009d85d278585b2896495b2b3046d461b/src/gallium/drivers/nouveau/nvc0/nvc0_state.c#L260)
programs these orientation/cull fields directly.
[`st_atom_viewport.c`](https://github.com/chaotic-cx/mesa-mirror/blob/e7959c6009d85d278585b2896495b2b3046d461b/src/mesa/state_tracker/st_atom_viewport.c#L61)
reverses viewport Y for top-origin surfaces, while
[`st_atom_rasterizer.c`](https://github.com/chaotic-cx/mesa-mirror/blob/e7959c6009d85d278585b2896495b2b3046d461b/src/mesa/state_tracker/st_atom_rasterizer.c#L77)
preserves front orientation for that origin. The same revision initializes
SCREEN_Y_CONTROL to zero. These establish why the existing vertex Y adjustment
must not be followed by another front-face inversion. Independent framebuffer
signed-area tests use guest viewport equations, not the host culling conversion.
No source implementation was copied.


## Unsigned SHL and SHR

The pinned MIT envytools GM107 table (`f102b82381f3f11cee113d16374c87091db039d9`)
and MIT UAM/Mesa emitter (`5a5afc2bae8b55409ab36ba45be63fcb73f68993`,
`emitSHL/emitSHR`) independently establish GPR operands at bits 7:0, 15:8, 27:20,
SHL base `0x5c48000000070000`, unsigned SHR base `0x5c28000000070000`, and wrap
bit 39. SHR signedness is bit 48; `.X`, CC and reverse-bit modifiers are excluded
by exact masks. The target's fixed non-F64 shift latency is six cycles.

Pinned MIT Ryujinx
[`InstEmitShift.cs`](https://github.com/alula/Ryujinx/blob/4930dced25c0d9ec33096b0f2601d262daa477bf/src/Ryujinx.Graphics.Shader/Instructions/InstEmitShift.cs#L164)
independently corroborates unsigned full-count clamp-to-zero and optional low-five-
bit wrapping. This is source-level semantic corroboration, not an experiment on
original Maxwell hardware. Mesa C++ constant-folding native shifts are not used
as an oracle for out-of-range counts. Unmodified pinned envyas/envydis assembled
and disassembled complete VS/PS fixtures. No source implementation was copied.

Pinned MIT Mesa NAK
[`OpShl::fold` and `OpShr::fold`](https://github.com/chaotic-cx/mesa-mirror/blob/e7959c6009d85d278585b2896495b2b3046d461b/src/nouveau/compiler/nak/ir.rs)
also independently define checked unsigned shifts: wrapping masks the count to
five bits, and nonwrapping out-of-range shifts produce zero. This corroborates
the high-count semantics beyond the archived Switch translator.


## Unsigned IMNMX

Pinned envytools GM107 and UAM/Mesa `emitIMNMX` establish unsigned GPR bases
`0x5c20038000070000` (PT minimum) and `0x5c20078000070000` (!PT maximum), with
ordinary GPR operand fields. Selection predicate is bits 41:39 and inversion bit
42; signedness is bit 48, CC bit 47, extended mode bits 44:43. Outer execution
predicate remains the separate P7 field at 18:16 with inversion 19 clear.

Pinned MIT Mesa NAK
[`OpIMnMx` SM50 encoding](https://github.com/chaotic-cx/mesa-mirror/blob/e7959c6009d85d278585b2896495b2b3046d461b/src/nouveau/compiler/nak/sm50.rs)
and [`OpIMnMx::fold`](https://github.com/chaotic-cx/mesa-mirror/blob/e7959c6009d85d278585b2896495b2b3046d461b/src/nouveau/compiler/nak/ir.rs)
independently corroborate the fields and unsigned true=min / false=max semantics.
The pinned UAM target schedules integer min/max at six cycles. Original unmodified
envyas/envydis fixtures independently corroborate encodings; no implementation
source was copied. These are original-format synthetic programs, not retail shaders.


## ISETP and SEL predicate dataflow

Pinned envytools GM107 and UAM/Mesa `emitISETP/emitSEL`, plus pinned MIT Mesa NAK
[`OpISetP::encode` and `OpSel::encode`](https://github.com/chaotic-cx/mesa-mirror/blob/e7959c6009d85d278585b2896495b2b3046d461b/src/nouveau/compiler/nak/sm50.rs),
establish unsigned ISETP opcode `0x5b60`, compare bits 51:49, signedness 48,
Boolean combine 46:45, accumulator predicate 41:39 with inversion 42, primary
predicate destination 5:3 and complementary destination 2:0. The accepted P0/PT,
AND PT form has fixed base `0x5b60038000070007`. SEL's GPR form is `0x5ca0`,
condition predicate 41:39 and inversion 42, with ordinary GPR operand fields.
Exact masks reject all unimplemented fields.

NAK [`OpISetP::fold` and `PredSetOp::eval`](https://github.com/chaotic-cx/mesa-mirror/blob/e7959c6009d85d278585b2896495b2b3046d461b/src/nouveau/compiler/nak/ir.rs)
independently corroborate unsigned comparisons and Boolean combination semantics.
Pinned MIT Ryujinx [`EmitSel`](https://github.com/alula/Ryujinx/blob/4930dced25c0d9ec33096b0f2601d262daa477bf/src/Ryujinx.Graphics.Shader/Instructions/InstEmitMove.cs)
corroborates true selecting the first GPR candidate. WGSL select reverses candidate
argument order to preserve this contract.

The old pinned UAM latency table assigns generic SET/SELP six cycles. Maintained
NAK `instr_latency` and `paw_latency` instead distinguish GPR six from predicate
13, with explicit calibration caveats. Therefore the accepted producer controls
use conservative stall 13; adjacent ISETP→SEL at stall six is rejected. Neither
model establishes the exact original-hardware minimum latency. Independently
assembled/disassembled conservative fixtures verify encoding; host execution
validates translation, not original Maxwell timing. No source code was copied.


## LDC.B64 paired words

Pinned envytools GM107 `tabef90_0`/`tabef90sz` fields and the existing pinned UAM
`emitLDC` establish size5 for B64 (size4 for B32), destination7:0, address GPR15:8,
signed byte offset35:20, CB slot40:36 and mode45:44. The accepted RZ/default-mode
base is `0xef9500000007ff00` before destination, offset and slot are inserted.
Pinned MIT Mesa NAK
[`OpLdc::encode`](https://github.com/chaotic-cx/mesa-mirror/blob/e7959c6009d85d278585b2896495b2b3046d461b/src/nouveau/compiler/nak/sm50.rs)
corroborates the operand fields and size encoding. Pinned MIT Ryujinx
[`Ldc`](https://github.com/alula/Ryujinx/blob/4930dced25c0d9ec33096b0f2601d262daa477bf/src/Ryujinx.Graphics.Shader/Instructions/InstEmitMemory.cs)
corroborates successive destination words at base offset and base+4. No reference
implementation source was copied. Original unmodified envyas/envydis independently
assemble the paired color fixture; TEX uses the already documented grammar alias.

A generic emitter's size encoder is insufficient evidence for LDC.B128. The
pinned envytools LDC table has no B128 form and independent assembly rejects it;
that form remains unsupported. Hardware behavior outside the accepted alignment,
positive-offset, even-register and contiguous-mapping subset is not claimed.


## Signed GPR IMNMX

The same pinned envytools GM107 table, UAM
[`emitIMNMX`](https://github.com/devkitPro/uam/blob/5a5afc2bae8b55409ab36ba45be63fcb73f68993/mesa-imported/codegen/nv50_ir_emit_gm107.cpp)
and maintained NAK `OpIMnMx::encode` establish signedness bit 48. The signed bases
are `0x5c21038000070000` (PT/minimum) and `0x5c21078000070000` (!PT/maximum).
In envytools spelling, absence of the `u32` qualifier means signed comparison.
NAK `OpIMnMx::fold` compares signed i32 values and returns the selected raw u32
payload. Original envyas/envydis corroborate the fixtures; no source implementation
was copied. This preserves the previously verified static predicate, operand,
modifier and six-stall scheduling subset.


## Single-negation IADD and the distinct PO encoding

Pinned envytools GM107 and UAM `emitIADD` establish source A negation bit 49 and
source B negation bit 48. Supported bases are `0x5c12000000070000` (B−A) and
`0x5c11000000070000` (A−B). The maintained MIT NAK
[`OpIAdd2::encode` and legalizer](https://github.com/chaotic-cx/mesa-mirror/blob/e7959c6009d85d278585b2896495b2b3046d461b/src/nouveau/compiler/nak/sm50.rs)
explicitly distinguish both bits together as IADD.PO; that encoding must not be
interpreted as ordinary double negation. The IR fold for ordinary IAdd2 does not
establish PO behavior. Its hardware semantics remain unsupported here. Original
unmodified envyas/envydis assemble both supported forms and the negative PO fixture.
No reference implementation code was copied.

The byte-observation fixture extracts each result byte, constructs the exact
finite float 0.5+byte/512, subtracts 0.5 and multiplies by rounded f32(512/255), bits
`0x40008081`. This permits independent integer oracles to verify raw arithmetic
through an RGBA8 target without NaN exports or a new guest conversion instruction.
The bounded byte domain is verified exhaustively; it is not a general float/int
conversion instruction implementation. The vertex fixture subtracts exponent bit
`0x00800000`; its half-coordinate oracle is limited to the chosen finite nonzero
normal values with biased exponent >=2, not general floating division.


## Signed ISETP and bit-preserving selection

The existing pinned MIT envytools GM107 ISETP tables, UAM/Mesa `emitISETP` and
[MIT NAK `OpISetP::encode`](https://github.com/chaotic-cx/mesa-mirror/blob/e7959c6009d85d278585b2896495b2b3046d461b/src/nouveau/compiler/nak/sm50.rs)
independently establish signedness at bit 48 (I32=1, U32=0), comparison at 51:49,
and the same register/predicate fields. NAK's
[`OpISetP` fold](https://github.com/chaotic-cx/mesa-mirror/blob/e7959c6009d85d278585b2896495b2b3046d461b/src/nouveau/compiler/nak/ir.rs)
interprets raw operands as signed i32 before the six comparisons. Its encoder
explicitly rejects pre-Volta .X because the accumulator interaction is unresolved;
Wanderlust retains rejection. The conservative accepted scheduling subset is
unchanged, not an assertion of calibrated Maxwell hardware latency. No source
implementation was copied.

Unmodified pinned envyas/envydis produced the retained synthetic fixtures, using
existing exact LDC PS or VTG SPH bytes followed by scheduled assembly. Reproduce
with `envyas -m gm107 -i -o NAME.code.bin < NAME.asm`, prepend the existing
`ldc.bin` or `vertex.bin` first 80 bytes, and disassemble code with
`envydis -m gm107 -i NAME.code.bin`. None was run on original Maxwell hardware;
translated execution is independently checked on host graphics backends.

| Fixture | SPH+code bytes | SHA-256 |
|---|---:|---|
| signed-predicate | 208 | `e139c9fed0db779acec531a7516c47697d50ae7424f2b78bdad318dd717c2fd4` |
| signed-select-bytes | 496 | `fd98f550efd9a9ec919d72aaf6ba519482eac74938445e4deb11a9ab4565bf10` |
| vertex-signed-predicate | 176 | `c59ca583a96115015680b8a0bdb12231077d04cdfd6c6d64280b433167d1b4a3` |

Independent single-instruction assembly for P0,PT,R4,R5,AND PT produced LT/EQ/LE/
GT/NE/GE words `5b63038000570407`, `5b65038000570407`, `5b67038000570407`,
`5b69038000570407`, `5b6b038000570407`, `5b6d038000570407`. Their unsigned
counterparts differ only in bit 48. The byte-observation fixture uses the already
validated finite float byte construction to observe every bit of runtime SEL
candidates through RGBA8. The vertex fixture retains negative X by signed X<0
and otherwise selects RZ. Mutating only signedness to unsigned makes X<0 always
false, collapsing the independently specified triangle to zero-area geometry.


## Signed SHR count and sign-fill semantics

Pinned MIT envytools GM107 and UAM/Mesa `emitSHR` establish signedness at bit 48,
.W at bit 39, .X at 44, .BREV at 40 and CC at 47. The GPR signed fixed bases are
`5c29000000070000` and `5c29008000070000`, excluding destination and GPR fields.
[MIT NAK `OpShr::fold`](https://github.com/chaotic-cx/mesa-mirror/blob/e7959c6009d85d278585b2896495b2b3046d461b/src/nouveau/compiler/nak/ir.rs)
independently clamps default unsigned counts to 32 and returns the signed value's
sign mask when checked shifting is out of range. Wrapped counts reduce to five
bits. The pinned MIT
[Ryujinx `EmitShr`](https://github.com/alula/Ryujinx/blob/4930dced25c0d9ec33096b0f2601d262daa477bf/src/Ryujinx.Graphics.Shader/Instructions/InstEmitShift.cs)
also implements signed sign-fill fallback for counts >=32. Source interpretation
was independently corroborated; no implementation code was copied.

Retained original unmodified envyas/envydis assembly/binaries use the existing
exact LDC PS and VTG headers and the reproduction commands above:

| Fixture | SPH+code bytes | SHA-256 |
|---|---:|---|
| signed-shr-bytes | 464 | `c63aa3e432cd7cfd635a3a2bd7a6b757bfe5d63451466c04dad5bd5adfd94737` |
| signed-shr-wrap-bytes | 464 | `46ad68ea7d2a6fa87fe0b8def8380921496491d8c97d90bfd00efdd8e791e0f0` |
| vertex-signed-shr | 176 | `42b413fe89315b35a63125fa9d7179293c907b1c5d1561c37d040e914b851a37` |

The fragment observer converts each result byte through the previously validated
finite float construction. The vertex fixture computes the sign mask of raw X
with SHR by 31, ANDs X with that mask and exports position/UV. For independently
specified finite positions this preserves negative X and zeros nonnegative X.
No execution on original Maxwell hardware or general shader coverage is claimed.

## Low-result IMUL and variable-latency dependencies

The pinned MIT envytools GM107 tables and
[MIT NAK `OpIMul::encode`](https://github.com/chaotic-cx/mesa-mirror/blob/e7959c6009d85d278585b2896495b2b3046d461b/src/nouveau/compiler/nak/sm50.rs)
establish GPR opcode 5c38, destination 7:0, sources 15:8 and 27:20, HI bit 39,
source signedness at 40/41 and CC at 47. Independent assembly gives low-product
fixed bases `5c38000000070000`, `5c38010000070000`, `5c38020000070000` and
`5c38030000070000`, excluding GPR fields. Low signed/unsigned products agree
modulo 2^32; the supported lowering uses raw u32 multiplication.

[UAM `TargetGM107::isBarrierRequired`](https://github.com/devkitPro/uam/blob/5a5afc2bae8b55409ab36ba45be63fcb73f68993/mesa-imported/codegen/nv50_ir_target_gm107.cpp)
explicitly requires a barrier for integer MUL/MAD. Pinned
[NAK's fixed-latency classification](https://github.com/chaotic-cx/mesa-mirror/blob/e7959c6009d85d278585b2896495b2b3046d461b/src/nouveau/compiler/nak/ir.rs)
likewise classifies IMUL as variable before SM70, and its builder selects low IMUL
for that backend family. The accepted six-stall spelling therefore accompanies
WB0 and a consumer wait; it is never treated as proof of result readiness.
No source implementation was copied, and no Maxwell hardware latency measurement
is claimed.

Original unmodified pinned envyas/envydis independently assemble/disassemble the
retained synthetic fixtures. Use the reproduction commands above and the existing
LDC PS or VTG 80-byte header:

| Fixture | SPH+code bytes | SHA-256 |
|---|---:|---|
| imul | 144 | `711888a75025204d4c74246e00b67a93e9fe9a9eaaf68cd310a49d99cbb32a07` |
| imul-bytes | 464 | `4e289b8c9d5d49c929f995a72cb1718c854d8a22e7dbaabf52706a11332dec8a` |
| vertex-imul | 208 | `bf71689a0823593ff7ebdbd0a406d981af137b701671012c8b7e9fd07ce2a1ea` |

The direct fragment program exports product bits as a float: ffffffff×c0800000
produces 3f800000 (red 1.0), distinguishing integer arithmetic from numeric float
multiplication. The byte observer exports every raw result bit through the already
validated finite conversion sequence. The vertex program constructs a 0/1 sign
mask and multiplies raw X bits by it, waiting before AST exports. Independently
specified finite input geometry retains negative X and zeros nonnegative X.
These binaries contain no commercial shader data and have not executed on original
Maxwell hardware.


## Global blend equations beyond ADD

Pinned NVIDIA [clb197.h](https://github.com/NVIDIA/open-gpu-doc/blob/9fdf5c4062007929d9f4e6cbad9c9771fe61b880/classes/3d/clb197.h)
(MIT), symbols NVB197_SET_BLEND_COLOR_OP and NVB197_SET_BLEND_ALPHA_OP, specifies
byte methods 0x1340/0x134c and the compact/GL aliases for ADD, SUBTRACT, reverse
SUBTRACT, MIN and MAX. SET_BLEND_SEPARATE_FOR_ALPHA at 0x133c selects independent
alpha state. The 0x1354 color-key method remains a distinct field in incrementing
packets, rather than another coefficient.

Pinned Mesa [st_update_blend](https://github.com/chaotic-cx/mesa-mirror/blob/e7959c6009d85d278585b2896495b2b3046d461b/src/mesa/state_tracker/st_atom_blend.c)
normalizes MIN/MAX coefficients to ONE/ONE separately for RGB and alpha.
[Softpipe equations](https://github.com/chaotic-cx/mesa-mirror/blob/e7959c6009d85d278585b2896495b2b3046d461b/src/gallium/drivers/softpipe/sp_quad_blend.c)
and [Nouveau emission](https://github.com/chaotic-cx/mesa-mirror/blob/e7959c6009d85d278585b2896495b2b3046d461b/src/gallium/drivers/nouveau/nvc0/nvc0_state.c)
corroborate subtraction order and equation programming. These are semantic
references; implementation code is not copied.

NVIDIA's [NV_blend_minmax_factor specification](https://registry.khronos.org/OpenGL/extensions/NV/NV_blend_minmax_factor.txt),
revision 3 (2017-07-26), distinguishes ordinary factor-independent MIN/MAX from
weighted FACTOR_MIN_AMD/FACTOR_MAX_AMD (901c/901d), which remain rejected. Ordinary
raw B197 factors are interpreted using this corroborated API model; contradictory
factors have not been tested on original Maxwell hardware.

[wgpu 26 blend operations](https://docs.rs/wgpu/26.0.1/wgpu/enum.BlendOperation.html)
support all five equations for the existing Rgba8Unorm attachment. Pinned
[wgpu-core pipeline validation](https://github.com/gfx-rs/wgpu/blob/0c978d0d46c7eaf68f6d1ecf2ea1bd03a96b7c47/wgpu-core/src/device/resource.rs)
checks blendability and relevant features; that version does not itself require
MIN/MAX factors to be ONE. Canonicalization follows the guest equation semantics,
not a claimed wgpu 26 validation error. No additional host feature is enabled.


## Native three-component vertex position

Pinned NVIDIA [SET_VERTEX_ATTRIBUTE_A](https://github.com/NVIDIA/open-gpu-doc/blob/9fdf5c4062007929d9f4e6cbad9c9771fe61b880/classes/3d/clb197.h)
defines stream bits4:0, active/inactive bit6, offset20:7, component widths26:21,
numerical type29:27 and swap31. FLOAT is type7; R32_G32_B32 is width value2.
Thus attribute-zero offset0 is 38400000, FLOAT2 UV offset12 is 38800600, and a
20-byte stream is 1014 at SET_VERTEX_STREAM_A_FORMAT(0). These are byte method
registers; only FIFO packet method addresses are divided by four.

Pinned Mesa [nv50_formats.c](https://github.com/chaotic-cx/mesa-mirror/blob/e7959c6009d85d278585b2896495b2b3046d461b/src/gallium/drivers/nouveau/nv50/nv50_formats.c)
maps R32G32B32_FLOAT directly to native FLOAT/32_32_32.
[nvc0_vertex_state_create](https://github.com/chaotic-cx/mesa-mirror/blob/e7959c6009d85d278585b2896495b2b3046d461b/src/gallium/drivers/nouveau/nvc0/nvc0_vbo.c)
uses the 12-byte element footprint rather than expanding storage. The default-W
interpretation follows this driver mapping and [OpenGL 4.6 section10.2](https://registry.khronos.org/OpenGL/specs/gl/glspec46.core.pdf)
missing-component rules. This is source-grounded evidence, not an original-Tegra
hardware experiment.

Pinned [wgpu-core26 vertex validation](https://github.com/gfx-rs/wgpu/blob/0c978d0d46c7eaf68f6d1ecf2ea1bd03a96b7c47/wgpu-core/src/validation.rs)
accepts matching scalar kinds across vertex vector widths because the driver fills
missing components. [Vulkan vertex input extraction](https://docs.vulkan.org/spec/latest/chapters/fxvertex.html#fxvertex-input-extraction)
also specifies default components. Wanderlust therefore retains its vec4 WGSL
input and selects native Float32x3; no translator special case is necessary.

The original synthetic vertex-float3-w.asm was assembled and disassembled with
unmodified envytools f102b82381f3f11cee113d16374c87091db039d9. Its existing
80-byte vertex SPH and two instruction bundles total144 bytes, SHA-256
cb440c7538b54c661d0f63ac9e2aad48393c3c27e87fa1467c67faa1bb263efb.
ALD B128 loads position, ALD B64 loads UV, MOV R4,R3 exports fetched W as UV.x,
then AST exports position/UV and EXIT terminates. Barrier/wait forms are unchanged.
The instruction bytes were independently reassembled exactly. No commercial data
or restrictive-license implementation source is included.
