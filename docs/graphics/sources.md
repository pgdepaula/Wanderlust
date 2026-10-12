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
