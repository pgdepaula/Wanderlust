# Graphics source assessment

Initial sources inspected on 2026-10-11; later sections record additional
implemented hardware subsets. Commit links identify the source considered;
dependency checksums and actual selected versions are recorded in `Cargo.lock`.

## Adopted implementations

| Implementation | Pinned source | Decision |
|---|---|---|
| wgpu 26.0.1 / Naga 26 | [`0c978d0d46c7eaf68f6d1ecf2ea1bd03a96b7c47`](https://github.com/gfx-rs/wgpu/tree/0c978d0d46c7eaf68f6d1ecf2ea1bd03a96b7c47) | MIT/Apache-2.0 native backend and controlled WGSL validation. wgpu declares Rust 1.84, compatible with this workspace's Rust 1.85 floor. |
| tegra_swizzle 0.4.0 | [`6fb9dfef9e6630bd155632802b3ead5ce5c656c8`](https://github.com/ScanMountGoat/tegra_swizzle/tree/6fb9dfef9e6630bd155632802b3ead5ce5c656c8) | MIT safe Rust Tegra X1 tiling. Reuse the library behind bounded geometry/input checks instead of maintaining another converter. |
| stats_alloc 0.1.10 | [instrumented allocator source](https://docs.rs/stats_alloc/0.1.10/src/stats_alloc/lib.rs.html) | MIT development-only Rust allocation measurement. Calibrated test-thread scopes forward allocation to the system allocator and exclude unrelated harness threads. |

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
channel submission and fences. The original packet decoder handles method
words; later sections describe the implemented bounded GPFIFO/driver boundary.
Horizon service dispatch and guest fence compatibility are separate contracts.
Method-span overflow is rejected as a
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
proprietary shader, asset, game dump or decryption material is included.
Project-authored code uses the root MIT license. Dependency and fixture licenses
and notice obligations remain separate.

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

### Subchannel engine bindings

NVIDIA open-gpu-doc MIT revision `9fdf5c4062007929d9f4e6cbad9c9771fe61b880`
defines class identities and method-zero binding in
[`clb1c0.h`](https://github.com/NVIDIA/open-gpu-doc/blob/9fdf5c4062007929d9f4e6cbad9c9771fe61b880/classes/compute/clb1c0.h),
[`clb0b5.h`](https://github.com/NVIDIA/open-gpu-doc/blob/9fdf5c4062007929d9f4e6cbad9c9771fe61b880/classes/dma-copy/clb0b5.h),
[`cla140.h`](https://github.com/NVIDIA/open-gpu-doc/blob/9fdf5c4062007929d9f4e6cbad9c9771fe61b880/classes/inline-to-memory/cla140.h)
and [`cl902d.h`](https://github.com/NVIDIA/open-gpu-doc/blob/9fdf5c4062007929d9f4e6cbad9c9771fe61b880/classes/twod/cl902d.h).
Their NOP/NO_OPERATION registers are all `0x0100`, with a full-width parameter.
The B06F class defines eight subchannels and the class/engine fields of
SET_OBJECT. The supported engine-instance-zero routing keeps these identities
separate; it imports no auxiliary engine implementation. Tests bind all four
auxiliary classes around a real B197 textured draw and reject attempts to write
3D shader state through those bindings.

## GPFIFO channel boundary references

MME instruction fields and EXIT delay slots are independently documented by the
MIT [envytools macro ISA at f102b82381f3f11cee113d16374c87091db039d9](https://github.com/envytools/envytools/blob/f102b82381f3f11cee113d16374c87091db039d9/envydis/macro.c).
The pinned Maxwell [macro interpreter](https://github.com/alula/Ryujinx/blob/4930dced25c0d9ec33096b0f2601d262daa477bf/src/Ryujinx.Graphics.Gpu/Engine/MME/MacroInterpreter.cs)
corroborates assignment modes, R0, the initial argument, signed immediates and
the delayed exit. Its [RAM ports](https://github.com/alula/Ryujinx/blob/4930dced25c0d9ec33096b0f2601d262daa477bf/src/Ryujinx.Graphics.Gpu/Engine/GPFifo/GPFifoClass.cs#L197-L209)
increment **both** upload pointers; the older envytools diagnostic observer does
not advance the entry pointer, and is not used as the reference for that behavior.
NVIDIA's pinned B197 class defines the four RAM ports and `SET_CT_WRITE(0)` at
byte offset `0x1a00`. No foreign interpreter implementation is copied.

`tests/fixtures/mme/state-mask.asm` was actually assembled and disassembled using
the pinned unmodified macro ISA (`envyas -m macro -i -o state-mask.bin
state-mask.asm`, `envydis -m macro -i state-mask.bin`). The 12-byte binary SHA-256
is `d2f455bd19af05668dbc96b6d4c909bc28675cb1e601a9c4646f9c12595397c4`.
The explicit expression `send (add $r1 0)` avoids the upstream assembler's
shorthand-parser crash; instruction tables are unchanged. Signed-immediate and
forbidden draw-emission negative fixtures were also assembled independently.

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
  NVIDIA class names; class definitions determine the parsed fields.
- Ryujinx reference fork `alula/Ryujinx`, MIT, revision
  `4930dced25c0d9ec33096b0f2601d262daa477bf`,
  [GPFifoProcessor.cs](https://github.com/alula/Ryujinx/blob/4930dced25c0d9ec33096b0f2601d262daa477bf/src/Ryujinx.Graphics.Gpu/Engine/GPFifo/GPFifoProcessor.cs)
  retains method-count state across Process calls, independently corroborating
  packet continuation. [GPFifoDevice.cs](https://github.com/alula/Ryujinx/blob/4930dced25c0d9ec33096b0f2601d262daa477bf/src/Ryujinx.Graphics.Gpu/Engine/GPFifo/GPFifoDevice.cs)
  corroborates address/word-length extraction and maps WAIT to deferred payload
  acquisition, suppressing early prefetch of following entries. Wanderlust uses
  deferred acquisition for all payloads and preserves its alias-publication
  barrier before every borrow. LEVEL is retained for the bounded straight-line
  path; accepting its descriptor value does not provide CALL/RETURN, USERD GET
  registers or hardware resume scheduling. No processor code was copied or
  vendored. Synthetic tests cover a GPU-written next command entry in MAIN/
  PROCEED, SUBROUTINE/WAIT and WAIT followed by MAIN/PROCEED configurations.

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
  ioctl surface is not implemented: reservations and disjoint fixed mappings
  retain separate lifetimes, with a bounded automatic-placement adapter above
  them. [map.c](https://github.com/switchbrew/libnx/blob/feebd026ca0f5dcc2119f46ad8e0d16ad3dd4973/nx/source/nvidia/map.c)
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


## Homogeneous Z clipping and raster precision

Pinned NVIDIA [clb197.h](https://github.com/NVIDIA/open-gpu-doc/blob/9fdf5c4062007929d9f4e6cbad9c9771fe61b880/classes/3d/clb197.h)
defines SET_VIEWPORT_Z_CLIP at byte0x0d7c, range bit0: negative-W-to-positive-W=0
and zero-to-positive-W=1. SET_VIEWPORT_CLIP_CONTROL at0x193c independently
defines pixel depth range, clamp modes, geometric clipping and guardbands.
Value1801 selects pixel zero-to-one and XYZ geometric clipping; its Z guardband
uses the XY setting. These fields must not be conflated with the 0x0d7c range.
Pinned Mesa [nvc0_state.c](https://github.com/chaotic-cx/mesa-mirror/blob/e7959c6009d85d278585b2896495b2b3046d461b/src/gallium/drivers/nouveau/nvc0/nvc0_state.c)
emits the clip-halfz-dependent selector separately. Nonidentity viewport depth
transforms require further guardband interpretation and are not accepted here.

[OpenGL4.6 section13.7.1](https://registry.khronos.org/OpenGL/specs/gl/glspec46.core.pdf)
and [Vulkan clipping outputs](https://docs.vulkan.org/spec/latest/chapters/vertexpostproc.html#vertexpostproc-clipping-shader-outputs)
define NoPerspective interpolation at generated clip vertices. A homogeneous
edge parameter t corresponds to screen parameter t*W1/((1-t)*W0+t*W1). Thus
the original affine screen UV plane is preserved under clipping with unequal W.
This is a source-and-mathematics inference, not an original-Tegra experiment.

Microsoft's [Direct3D functional specification, section3.4.1](https://github.com/microsoft/DirectX-Specs/blob/a610933eac2103cd502bacfb5193ae7f02c6fa54/d3d/archive/D3D11_3_FunctionalSpec.htm)
requires post-clipping viewport XY to snap to eight fractional bits before
interpolator setup; section15.16.2 includes depth. D3D12 retains these baseline
rendering rules. The depth-clipping fixture deliberately uses binary-exact
positions and a 32x16 viewport with all clipping intersections on the subpixel
grid, so an independent analytic plane remains a valid cross-backend oracle.
This avoids assuming that unsnapped decimal geometry must have identical depth
on every host rasterizer. No backend-specific production workaround is applied.


## Component blend coefficient encodings

Pinned NVIDIA [clb197.h](https://github.com/NVIDIA/open-gpu-doc/blob/9fdf5c4062007929d9f4e6cbad9c9771fe61b880/classes/3d/clb197.h)
SET_BLEND_COLOR_SOURCE_COEFF, COLOR_DEST_COEFF, ALPHA_SOURCE_COEFF and
ALPHA_DEST_COEFF define the same six additional compact/GL pairs:
SRC_COLOR3/4300, INV_SRC_COLOR4/4301, DST_ALPHA7/4304, INV_DST_ALPHA8/4305,
DST_COLOR9/4306 and INV_DST_COLOR10/4307. Byte methods are1344,1348,1350,1358;
the1354 color-key field remains separate. Pinned Mesa
[nvc0_blend_fac](https://github.com/chaotic-cx/mesa-mirror/blob/e7959c6009d85d278585b2896495b2b3046d461b/src/gallium/drivers/nouveau/nvc0/nvc0_state.c)
corroborates the source/destination mappings and independent alpha programming.

Cargo.lock pins wgpu-hal26.0.6. Its
[DX12 coefficient lowering](https://github.com/gfx-rs/wgpu/blob/6f8edda9b180efada6a24b6d40c567c57f59e9ea/wgpu-hal/src/dx12/conv.rs)
maps Src/Dst and their complements to corresponding alpha coefficients when
lowering the alpha blend component. This respects
[D3D12's alpha-slot restrictions](https://learn.microsoft.com/en-us/windows/win32/api/d3d12/ns-d3d12-d3d12_render_target_blend_desc)
without changing guest semantics or patching the dependency. These factors need
no additional feature beyond the existing blendable Rgba8Unorm attachment.
Constant, saturation and dual-source forms are not inferred from this subset.


## Maxwell LOP source complements

Pinned MIT [envytools GM107 tables](https://github.com/envytools/envytools/blob/f102b82381f3f11cee113d16374c87091db039d9/envydis/gm107.c)
define AND/OR/XOR/PASS_B in bits42:41, source complements39/40, extended43,
predicate condition45:44, CC47 and predicate output50:48. The accepted
full-word variable mask is000007800ff0ffff against base5c47000000070000.
Unmasked fields retain their independently assembled values; this is not a
general opcode-prefix match. Pinned MIT Mesa
[SM50 OpLop2 encoding](https://github.com/chaotic-cx/mesa-mirror/blob/e7959c6009d85d278585b2896495b2b3046d461b/src/nouveau/compiler/nak/sm50.rs)
and [logic folding](https://github.com/chaotic-cx/mesa-mirror/blob/e7959c6009d85d278585b2896495b2b3046d461b/src/nouveau/compiler/nak/ir.rs)
corroborate field positions and raw-bit semantics.

Pinned UAM [NOT emission](https://github.com/devkitPro/uam/blob/5a5afc2bae8b55409ab36ba45be63fcb73f68993/mesa-imported/codegen/nv50_ir_emit_gm107.cpp)
uses canonical PASS_B(RZ,complementedB). Its
[latency model](https://github.com/devkitPro/uam/blob/5a5afc2bae8b55409ab36ba45be63fcb73f68993/mesa-imported/codegen/nv50_ir_target_gm107.cpp)
assigns six cycles to these bit operations, retained as the conservative existing
scheduling subset. This does not establish complete Maxwell timing/scoreboards.
No implementation source is copied.

Three original synthetic assembly files were independently assembled and
disassembled twice with unmodified pinned envytools. Validated existing 80-byte
SPH variants are retained; instruction bytes reproduce exactly:

| Fixture | Bytes | SHA-256 |
| --- | ---: | --- |
| lop-invert.bin |144|a73cce06e2a4b926ae48e6e84f8a7f0cbd0997c30c207a737df586d9d1cf438d|
| lop-bytes.bin |464|e15579ecf3646f5044be1182ede93fea63676a0df56abb548e9a5e6fdd919496|
| vertex-lop.bin |176|8bc6a5e6087b4924b734f00ca1473e9cfdf5603e16f8ce3ef9845f38d4d836b9|

The byte observer uses the existing validated finite byte-to-UNORM sequence, not
raw floating export of arbitrary NaN bit patterns. The vertex fixture XORs clip X
with the complement of7fffffff, toggling its sign bit. No original-game or
original-Maxwell-hardware execution is claimed for these fixtures.


## Full-width Maxwell LOP32I

Pinned [envytools GM107](https://github.com/envytools/envytools/blob/f102b82381f3f11cee113d16374c87091db039d9/envydis/gm107.c)
and MIT [Mesa SM50 OpLop2](https://github.com/chaotic-cx/mesa-mirror/blob/e7959c6009d85d278585b2896495b2b3046d461b/src/nouveau/compiler/nak/sm50.rs)
identify the full32-bit immediate at51:20, GPR source15:8, destination7:0,
operator54:53 and complement55/56. PT occupies19:16, CC52 and X57 are separate.
The complete-word variable mask01effffffff0ffff against0400000000070000
keeps every other bit fixed. The common six-stall readiness subset is retained.

Operator3 is intentionally unsupported: envytools names a PASS_B form and UAM
uses long-immediate NOT emission, but the pinned NAK encoder explicitly refuses
PASS_B for LOP32I. No equivalence is inferred from that disagreement. The three
verified operators are sufficient for full-width masks and sign-bit changes.
No implementation source is copied.

Three original fixtures retain existing strict80-byte stage headers and were
independently assembled/disassembled twice using pinned unmodified envytools:

| Fixture | Bytes | SHA-256 |
| --- | ---: | --- |
| lop32i.bin |144|fd955a68bd0ca5ef491e09467a6a5438a62011fc80244f93df92a4890bc2de1e|
| lop32i-bytes.bin |464|1f3487c32aecbf7242f66ec02ed0a91192bf6631974dcdb4602d5d4dae3e3913|
| vertex-lop32i.bin |144|fd8bf836fac36a357b7027638c0360f3c10741041542201b79e4e80c8a07d92b|

The finite byte observer exposes all32 result bits through RGBA8. The vertex
program XORs clip X with immediate80000000, independently distinguishing a
full-width immediate from a narrow or sign-extension interpretation.


## Maxwell IADD32I raw integer semantics

Pinned MIT [envytools GM107](https://github.com/envytools/envytools/blob/f102b82381f3f11cee113d16374c87091db039d9/envydis/gm107.c),
[Mesa SM50 OpIAdd2](https://github.com/chaotic-cx/mesa-mirror/blob/e7959c6009d85d278585b2896495b2b3046d461b/src/nouveau/compiler/nak/sm50.rs)
and [NAK integer folding](https://github.com/chaotic-cx/mesa-mirror/blob/e7959c6009d85d278585b2896495b2b3046d461b/src/nouveau/compiler/nak/ir.rs)
corroborate raw immediate bits 51:20 and source-A negation at bit 56. Destination
and source registers occupy 7:0 and 15:8, with PT execution at 19:16. The strict
variable mask `010ffffffff0ffff` against `1c00000000070000` excludes CC52, X53,
SAT54 and bit55, including the distinct PO encoding. Ordinary negation is
two's-complement source negation before low-32-bit addition. Carry/X forms are
not inferred from the ordinary encoding.

Pinned [UAM long-immediate emission](https://github.com/devkitPro/uam/blob/5a5afc2bae8b55409ab36ba45be63fcb73f68993/mesa-imported/codegen/nv50_ir_emit_gm107.cpp)
independently corroborates field placement. The six-stall scheduling restriction
remains a conservative translator subset, not measured original-Maxwell timing.
No implementation source is copied.

Unmodified pinned envyas uses signed textual notation for this full-width field:
`-1`, `-0x80000000` and `-0x76543211` encode raw `ffffffff`, `80000000` and
`89abcdef`. This notation does not imply signed-20-bit binary decoding. Original
fixtures were independently assembled/disassembled twice and complete bytes
compared, retaining existing strict 80-byte stage headers:

| Fixture | Bytes | SHA-256 |
| --- | ---: | --- |
| iadd32i.bin | 144 | eef4b0894812e376f3e9a9314ac13ec38b54313e4b7d4e9609cad9a617df2d9c |
| iadd32i-bytes.bin | 464 | 04ebbd399e3d8a3f3abd5a52b3cd112e60dca7ebd7b997429940c9209702bfa7 |
| vertex-iadd32i.bin | 144 | 07b65db0e3c8a6f6cfe4341caae5747c712c2b904fb0da4bf20b4fa4d1e8e78e |

The vertex program adds raw `80000000` modulo 2^32 to clip X bits. This toggles
the floating-point sign bit and produces an independently testable mirrored
triangle without host-injected positions. The finite byte observer exposes
arbitrary raw arithmetic results through RGBA8, avoiding NaN export assumptions.


### Portable fused-rounding limit

The [WGSL September 2026 specification](https://www.w3.org/TR/2026/CRD-WGSL-20260921/#fma-builtin)
permits `fma` to perform a rounded multiply followed by a rounded add. Installed
[Naga 26 HLSL lowering](https://github.com/gfx-rs/wgpu/blob/03f1b535ca2356d4d490540e76480b0116cf7d7f/naga/src/back/hlsl/writer.rs)
emits `mad`; [Microsoft's contract](https://learn.microsoft.com/en-us/windows/win32/direct3dhlsl/mad)
also permits fused or separate arithmetic even with `precise`. Thus an intrinsic
spelling or successful adapter test cannot establish general Maxwell FFMA.RN
single-rounding semantics. FFMA remains unsupported and FMUL-to-FADD taint
validation remains necessary.

An independent exact-rational counterexample uses raw float inputs `3f800001`,
`3f7ffffe`, `bf800000`: `(1+2^-23)*(1-2^-23)-1` rounds to `a8800000` with one
rounding, versus `00000000` after separately rounded multiply/add. General FFMA
requires a stronger host-operation guarantee or explicitly implemented integer
floating-point semantics. This is a translation-contract limitation, not a
claim that the current hardware adapters necessarily produce the unfused result.


## Signed-20-bit IADD immediate fields

Pinned [envytools GM107 S20_20](https://github.com/envytools/envytools/blob/f102b82381f3f11cee113d16374c87091db039d9/envydis/gm107.c),
[Mesa SM50 OpIAdd2](https://github.com/chaotic-cx/mesa-mirror/blob/e7959c6009d85d278585b2896495b2b3046d461b/src/nouveau/compiler/nak/sm50.rs)
and [UAM emitIADD](https://github.com/devkitPro/uam/blob/5a5afc2bae8b55409ab36ba45be63fcb73f68993/mesa-imported/codegen/nv50_ir_emit_gm107.cpp)
corroborate low immediate bits 38:20, sign bit 56 and source negation at bit 49.
The complete-word variable mask `0102007ffff0ffff` against `3810000000070000`
excludes immediate-negation/PO48, CC47, X43, SAT50 and other side effects.
Sign extension produces raw 32-bit data before the existing AddImmediate
operation executes; no new lowering or guest-memory representation is required.

All three original fixtures were independently assembled/disassembled twice
with unmodified pinned envytools, retaining strict existing 80-byte headers:

| Fixture | Bytes | SHA-256 |
| --- | ---: | --- |
| iadd20.bin | 144 | d624f2786795fb0f901e07ff732aa8a44d17462b3d6044053c8652ed97b4179f |
| iadd20-bytes.bin | 464 | 493f130e7a04f3f449ad28ffdb67ee084d1826a75b9a3c3a2aeac47e62448145 |
| vertex-iadd20.bin | 144 | bb6c68c43421e73193f4bd2237d41d18915639790002c8adc9b86b2cb1f89b27 |

Textual zero assembles as the equivalent GPR-RZ form in envytools. Immediate-zero
boundary cases therefore explicitly mutate the verified binary field; they are
not falsely attributed to assembler output. The vertex fixture uses -524288,
whose zero low payload and set sign bit independently catch a missing sign
extension. No original-game content or implementation source is copied.


## Native Z16 depth storage

The pinned NVIDIA [B197 class](https://github.com/NVIDIA/open-gpu-doc/blob/9fdf5c4062007929d9f4e6cbad9c9771fe61b880/classes/3d/clb197.h)
defines `SET_ZT_FORMAT` at byte offset `0x0fe8`, field4:0, Z16=`0x13` and
ZF32=`0x0a`. Pinned Mesa [nv50_formats.c](https://github.com/chaotic-cx/mesa-mirror/blob/e7959c6009d85d278585b2896495b2b3046d461b/src/gallium/drivers/nouveau/nv50/nv50_formats.c)
corroborates Z16_UNORM; framebuffer setup retains pixel dimensions separately
from the block-linear allocation footprint. Z16 therefore uses two little-endian
bytes per sample and GOB row width `2*width`, with the same validated B197 block
height fields and single-layer restrictions. It is not an RGBA image of half width.

Locked wgpu26 maps `Depth16Unorm` to native D16 storage on the three tested
backends. As for ZF32, direct depth-buffer copies are not the portable GLES
contract. R32Uint transfer shaders expand guest codes to f32(code)/65535 on
initialization and recover normalized sampled codes on export. Native depth
comparison and writes occur in the D16 attachment. Export's integer packing is
checked; it does not define Maxwell rounding at halfway values. Exact endpoints
and codes away from comparison ties provide portable independent raster tests.
All65536 stored codes also have an executed native initialization/export test.


## Short-immediate floating arithmetic

Pinned MIT [GM107 tables](https://github.com/envytools/envytools/blob/f102b82381f3f11cee113d16374c87091db039d9/envydis/gm107.c)
define FADD3858 and FMUL3868 with `F20_20`: nineteen payload bits38:20 plus
sign56 reconstruct f32 bits31:12. Low twelve mantissa bits are zero. Pinned
[UAM GM107 emitter](https://github.com/devkitPro/uam/blob/5a5afc2bae8b55409ab36ba45be63fcb73f68993/mesa-imported/codegen/nv50_ir_emit_gm107.cpp)
corroborates this in `emitIMMD`/`longIMMD` and the operation emitters. These
are raw IEEE representations, not signed20 integer values or numeric conversions.

The accepted variable mask0100007ffff0ffff permits destination7:0, source15:8,
payload38:20 and sign56. Round modes, saturation, FTZ/FMZ, scaling, absolute/
negate modifiers, condition-code outputs and predicates remain excluded. The
existing stall-six/write-barrier-seven fixed-latency contract remains strict.
Original unmodified envyas/envydis assembled both stage fixtures. Float immediate
assembly uses raw IEEE hex; decimal tokens are not relied upon. Explicit positive
zero mutations are distinguished from the assembler's GPR-RZ canonical choice.

The shared lowering bitcasts the immediate operand. Immediate multiplication
sets the existing contraction-hazard metadata; GPR/immediate additions reject
multiply-derived values even through identities. Arithmetic erases resource and
perspective proofs. FFMA remains unsupported. Exceptional immediate bit decoding
does not establish exact Maxwell NaN, subnormal, negative-zero or halfway behavior;
the same bounded portable f32 limitations as existing GPR arithmetic apply.


## Full-width floating immediates

The same pinned GM107 tables define FADD32I=`0x08` and FMUL32I=`0x1e` with
`F32_20`, a complete raw32-bit IEEE operand in instruction bits51:20. UAM's
long-immediate emitters corroborate both bases and select this form when low
twelve mantissa bits are nonzero. Strict default bases are0800000000070000 and
1e00000000070000; variable mask000ffffffff0ffff permits destination7:0,
source15:8 and immediate51:20. Sign is immediate bit31 at instruction51, unlike
the short encoding's separate sign56.

Bits39/40 and every other position within51:20 are data in these forms, not
short-form rounding controls. FADD condition-code, source/immediate negate/abs
and FTZ modifiers remain excluded. FMUL condition-code, FTZ/FMZ and SAT remain
excluded. Bit57 of FMUL32I is opcode space: flipping it yields the separately
supported IADD32I=`0x1c`, not a floating modifier. Tests preserve that valid
integer interpretation rather than requiring a false rejection. Neither pinned
long emitter exposes a rounding selector here; unmodified envyas rejects `rm`.
No inference from neighboring short encodings is used.

## Predicated GPR MOV

Pinned [GM107 tables](https://github.com/envytools/envytools/blob/f102b82381f3f11cee113d16374c87091db039d9/envydis/gm107.c)
and [UAM emitPred/emitMOV](https://github.com/devkitPro/uam/blob/5a5afc2bae8b55409ab36ba45be63fcb73f68993/mesa-imported/codegen/nv50_ir_emit_gm107.cpp)
agree on destination7:0, GPR source27:20, predicate selector18:16 and inversion19.
The full lane mask is bits42:39=f. Selector7 is PT; inverted PT is never execution.
The added strict P0/!P0 subset uses base5c98078000000000 and variable
mask000000000ff800ff. Existing unconditional PT decoding remains separate;
P1 through P6 and inverted PT remain unsupported.

Original unmodified envyas/envydis assembles P0 MOV R0,R4 as5c98078000400000
and !P0 as5c98078000480000. NVIDIA's
[CUDA8 programming guide](https://docs.nvidia.com/cuda/archive/8.0/pdf/CUDA_C_Programming_Guide.pdf)
describes suppression of operand reads and destination writes on false
predication. This is conditional execution, not an unconditional SEL. Requiring
both possible GPR values ready is a conservative translation limit, not a claim
that hardware faults on a suppressed read. No scheduling bits are ignored.

## Ordered FSETP

The pinned GM107 table and UAM emitter above corroborate GPR FSETP base
`5bb0038000070007`, variable mask `000f00000ff0ff00`, mode bits 51:48,
left GPR bits 15:8 and right GPR bits 27:20. Modes 1 through 6 are ordered
LT/EQ/LE/GT/NE/GE. FTZ is bit 47 and remains excluded, along with source
absolute/negate, execution predicates and noncanonical boolean/output fields.
Original envyas independently assembles all sixteen comparison encodings;
the original subset accepted only the six ordered forms. The extension accepts
all sixteen modes with the same operand/output mask and scheduling restrictions.
Mode 0 is F, 7 NUM, 8 NAN, 9 through 14 LTU/EQU/LEU/GTU/NEU/GEU, and 15 T.
[Pinned NAK SM50](https://github.com/chaotic-cx/mesa-mirror/blob/e7959c6009d85d278585b2896495b2b3046d461b/src/nouveau/compiler/nak/sm50.rs#L775)
corroborates all fourteen nonconstant conditions;
[pinned UAM](https://github.com/devkitPro/uam/blob/5a5afc2bae8b55409ab36ba45be63fcb73f68993/mesa-imported/codegen/nv50_ir_emit_gm107.cpp#L394)
also defines F/T and the six unordered conditions. UAM's NUM/NAN cases are
commented out and are not used as the sole evidence for those encodings.

NVIDIA's [CUDA 8 PTX comparison definitions](https://docs.nvidia.com/cuda/archive/8.0/parallel-thread-execution/index.html#comparison-and-selection-instructions-setp)
define NaN rejection for all ordered relations, signed-zero equality, and
subnormal preservation without FTZ. The [PTX 12.8.1 definitions](https://docs.nvidia.com/cuda/archive/12.8.1/parallel-thread-execution/index.html#comparison-and-selection-instructions-setp)
corroborate the ordered/unordered distinction. This semantic evidence is separate
from the Maxwell binary encoding proof. Integer classification and ordering
avoid depending on host f32 exceptional-value behavior.
Unordered conditions accept either NaN; NUM requires two non-NaN operands and
NAN requires at least one NaN. These classifications include signaling NaNs,
both signs and arbitrary nonzero payloads, and NUM includes infinities.

Pinned UAM's `SchedDataCalculatorGM107::recordWr` records predicate readiness at
cycle + 13. [Pinned NAK SM50](https://github.com/chaotic-cx/mesa-mirror/blob/e7959c6009d85d278585b2896495b2b3046d461b/src/nouveau/compiler/nak/sm50.rs)
also assigns thirteen-cycle predicate latency; its IR classifies FSetP as fixed
latency. This supports a conservative accepted schedule, not a measured hardware
minimum or a general scoreboard implementation. Reference implementations were
examined for semantics; the bit-order lowering is independently derived.

## Contained immediate BFE fields

The same pinned [GM107 table](https://github.com/envytools/envytools/blob/f102b82381f3f11cee113d16374c87091db039d9/envydis/gm107.c),
[NAK SM50 encoder](https://github.com/chaotic-cx/mesa-mirror/blob/e7959c6009d85d278585b2896495b2b3046d461b/src/nouveau/compiler/nak/sm50.rs#L1160)
and [UAM emitter](https://github.com/devkitPro/uam/blob/5a5afc2bae8b55409ab36ba45be63fcb73f68993/mesa-imported/codegen/nv50_ir_emit_gm107.cpp#L2294)
corroborate immediate BFE base `3800000000070000`, destination bits 7:0,
source bits 15:8, low-byte offset bits 27:20, next-byte width bits 35:28 and
signed-result bit 48. The accepted mask is `0001000ffff0ffff`. The original
signed20 immediate also has high payload bits 38:36 and sign bit **56**, not
38; this subset excludes those irrelevant control bits rather than guessing
their meaning. BREV bit 40, CC bit 47 and execution predicates remain excluded.

[CUDA 8 PTX BFE](https://docs.nvidia.com/cuda/archive/8.0/parallel-thread-execution/index.html#integer-arithmetic-instructions-bfe)
defines extraction and signed extension. Pinned UAM constant folding independently
corroborates contained fields, but its native shifts do not safely establish
all out-of-range SASS cases. Consequently supported offsets are below 32,
widths at most 32 and offset plus width at most 32; dynamic GPR/cbuf controls and
overrunning fields remain unsupported. Width zero returns zero; full width
preserves the raw source. Ordinary six-cycle GPR scheduling is retained.

Unmodified pinned envyas independently assembles the complete PS/VS fixtures.
Textual immediate zero canonicalizes to a GPR-RZ control instruction, so the
assembled zero-width regression uses nonzero offset 31. Explicitly encoded
immediate-zero decoder cases are not represented as assembler-produced fixtures.
The translation and scalar per-bit oracle are independently derived; reference
implementation source was examined for semantics rather than copied.

## SM50 three-input logic tables

[CUDA 8 PTX LOP3](https://docs.nvidia.com/cuda/archive/8.0/parallel-thread-execution/index.html#logic-and-shift-instructions-lop3)
requires SM50 and defines an eight-bit truth table with input patterns F0, CC,
AA. Per result bit its index is `(A_bit << 2) | (B_bit << 1) | C_bit`.
The original GM107 opcode table and the independent MIT
[MaxAs grammar at 54eda7af086a46c9dae1688b691968235d560164](https://github.com/NervanaSystems/maxas/blob/54eda7af086a46c9dae1688b691968235d560164/lib/MaxAs/MaxAsGrammar.pm#L329)
agree on all 256 all-GPR encodings, verified by both assemblers. MaxAs's x32T
classification also corroborates conservative six-cycle latency. Pinned NAK's
SM50 encoder does not implement this opcode; its SM70 encoder is not used as
Maxwell encoding evidence.

The strict form is base `5be7000000070000`, mask `00007f8ffff0ffff`:
destination bits 7:0, A bits 15:8, B bits 27:20, LUT bits 35:28 and C bits
46:39. C's high bits are register data, not operand modifiers. Predicate output
bits 50:48 are fixed to discarded PT; modes 37:36, X bit 38, CC bit 47 and
execution predication remain excluded. Every LUT value is valid in this form.

The CUDA 8 example for `((a & b) | c) ^ a` contains a LUT typo; the truth-table
equation and the corrected later PTX example give 1A. Fixtures and oracles use
the equation rather than copying the typo. Reference implementations establish
encoding and semantics; the minterm lowering and scalar per-bit oracle are
independently written. No MaxAs implementation is imported into the renderer.

## SM50 population count and scoreboard readiness

The pinned GM107 table defines GPR POPC with opcode 5c08, destination bits7:0,
source bits27:20 and optional inversion bit40. The strict unconditional base is
`5c08000000070000`, variable mask `000001000ff000ff`. Original envyas and pinned
MaxAs independently agree on the fixture's ordinary GPR words and RZ/alias
forms. MaxAs does not describe the inverted spelling; inversion is separately
corroborated by GM107 and the
[NAK SM50 OpPopC encoder](https://github.com/chaotic-cx/mesa-mirror/blob/e7959c6009d85d278585b2896495b2b3046d461b/src/nouveau/compiler/nak/sm50.rs#L1616).

[NAK folding and latency classification](https://github.com/chaotic-cx/mesa-mirror/blob/e7959c6009d85d278585b2896495b2b3046d461b/src/nouveau/compiler/nak/ir.rs)
corroborate bit inversion before population count and variable latency. MaxAs's
s2r scoreboard class independently agrees; its guessed barrier-latency number
is not treated as a measured hardware minimum. The accepted scheduling subset
uses write barrier zero and explicit wait1 before consuming the result. This
extends existing bounded scoreboard validation, rather than treating POPC as
ordinary fixed-latency arithmetic. Immediate/cbuf forms remain excluded.

The scalar test oracle sums individual bits independently of the host intrinsic.
Only encoding/semantic evidence is used from reference implementations; no
assembler/compiler implementation is imported. FLO and its disputed SH zero
behavior are not implied by this instruction's support.

## Non-SH SM50 highest-bit search

Pinned GM107 and
[NAK SM50 OpFlo](https://github.com/chaotic-cx/mesa-mirror/blob/e7959c6009d85d278585b2896495b2b3046d461b/src/nouveau/compiler/nak/sm50.rs#L1197)
agree on GPR opcode 5c30, destination bits 7:0, source bits 27:20 and signed bit 48. The
strict unconditional base is `5c30000000070000`, variable mask
`000100000ff000ff`. Original envyas independently assembles unsigned/signed
fragment and signed vertex fixtures. Pinned MaxAs independently matches the
unsigned words; its grammar is not claimed to assemble the signed form.

[CUDA 8 PTX bfind](https://docs.nvidia.com/cuda/archive/8.0/parallel-thread-execution/index.html#integer-arithmetic-instructions-bfind)
and pinned UAM constant folding corroborate non-SH semantics: unsigned highest
one-bit index, signed negative highest zero-bit index, and FFFFFFFF when no
qualifying bit exists. UAM's util_last_bit[_signed]-1 handles the sentinel
explicitly; no reliance is placed on an unchecked unsigned subtraction in
NAK's folding expression.
[WGSL signed firstLeadingBit](https://www.w3.org/TR/2026/CRD-WGSL-20260921/#firstLeadingBit-signed-builtin)
and [unsigned firstLeadingBit](https://www.w3.org/TR/2026/CRD-WGSL-20260921/#firstLeadingBit-unsigned-builtin)
provides matching signed/unsigned host semantics, with signed -1 retained by
bitcast. The independent scalar oracle scans each bit instead of using that
host intrinsic. Reference implementations provide semantic evidence only.

NAK and MaxAs independently classify this SM50 instruction as variable latency.
The existing single-barrier WB0/wait1 contract is mandatory. SH bit 41 is
excluded because its zero-result behavior is not resolved by the examined
references; inversion bit 40, CC bit 47, execution predicates and other source forms
are also outside the accepted subset. No general FLO/CLZ support is implied.

## SM50 I2F RN32 and host conversion precision

The pinned [GM107 opcode table](https://github.com/envytools/envytools/blob/f102b82381f3f11cee113d16374c87091db039d9/envydis/gm107.c),
[NAK SM50 OpI2F encoder](https://github.com/chaotic-cx/mesa-mirror/blob/e7959c6009d85d278585b2896495b2b3046d461b/src/nouveau/compiler/nak/sm50.rs#L1842-L1880)
and original MaxAs grammar at `54eda7af086a46c9dae1688b691968235d560164`
agree on GPR opcode 5cb8, destination bits 7:0, source bits 27:20 and signed-source
bit 13. Bits 9:8 and 11:10 encode destination and source widths; both equal 2
for F32 <- U32/S32. These fields must not be interpreted as a source register.
The strict unconditional RN base is `5cb8000000070a00`, variable mask
`000000000ff020ff`. Original envyas independently assembles all three fixtures;
MaxAs independently matches their conversion words. NAK and pinned UAM latency
classification require scoreboard readiness; the bounded WB0/wait1 contract
is retained, including RZ and destination/source aliases. Other widths, rounding
modes, byte suboperations, NEG, ABS, CC, execution predicates and non-GPR source
forms remain excluded.

[CUDA 8 PTX rounding modifiers](https://docs.nvidia.com/cuda/archive/8.0/parallel-thread-execution/index.html#data-movement-and-conversion-instructions-cvt)
define RN as nearest with even ties. However,
[WGSL numeric conversion](https://www.w3.org/TR/2026/CRD-WGSL-20260921/#numeric-scalar-conversion-to-floating-point)
permits either adjacent floating-point value for inexact integer conversion.
A bare `f32(integer)` therefore does not establish Maxwell RN semantics.
The shared lowering packs binary32 using only bounded unsigned integer
operations: signed magnitude, leading bit, discarded-bit comparison, even tie
selection and significand carry. Zero produces positive zero; INT_MIN is handled
without signed overflow. All 32-bit integer inputs convert to finite normal
values or zero, avoiding subnormal and NaN-payload ambiguities.

The pixel-test oracle independently searches adjacent IEEE binary32 encodings
using exact f64 distances, rather than reproducing the packing algorithm. A
separate exact rational oracle also verifies boundary and seeded cases. Only
semantic/encoding evidence is used from the referenced compilers; no compiler
implementation or restrictive-license assembler source is imported.

## SM50 GPR FSET comparison results

Pinned GM107, [NAK SM50 OpFSet](https://github.com/chaotic-cx/mesa-mirror/blob/e7959c6009d85d278585b2896495b2b3046d461b/src/nouveau/compiler/nak/sm50.rs#L828-L864)
and [UAM Maxwell FSET emission](https://github.com/devkitPro/uam/blob/5a5afc2bae8b55409ab36ba45be63fcb73f68993/mesa-imported/codegen/nv50_ir_emit_gm107.cpp#L1539-L1585)
agree on unmodified GPR FSET: base `5800038000070000`, variable mask
`001f00000ff0ffff`; destination bits7:0, left operand15:8, right27:20,
comparison51:48 and float-result bit52. The logical operation is fixed to
AND PT, without inversion. The destination is a GPR; this instruction does
not write predicate P0. The fixed six-cycle GPR schedule differs from the
conservative thirteen-cycle SETP schedule.

[CUDA8 set semantics](https://docs.nvidia.com/cuda/archive/8.0/parallel-thread-execution/index.html#comparison-and-selection-instructions-set)
corroborate the ordered/unordered and classification semantics, preserved
subnormals, false=00000000 and true=FFFFFFFF for mask output or 3F800000 for
float output. Hardware F/T encodings are established separately by GM107/UAM
and independent assembly, rather than attributed to the PTX set syntax.
Raw integer classification/order implements signed-zero equality and NaN
handling independently of host FTZ/NaN comparison behavior. The existing FSETP
condition emitter is shared through an explicitly named destination; FSET uses
a scoped local boolean and leaves P0 unchanged.

Original pinned envyas independently assembled all32 comparison/result forms
and the three fixtures. MaxAs independently matches fourteen unmodified
nonconstant mask-result forms; its pinned grammar lacks F/T/BF spelling.
Its shared modifier table disagrees with the corroborated FSET left-ABS and
right-NEG fields, and is not used as modifier evidence. Source modifiers,
FTZ, CC, execution predicates, non-PT logical inputs, OR/XOR combiners and
immediate/cbuf forms remain unsupported. Only semantic/encoding evidence is
used from these MIT sources; their compiler/assembler implementation is not
imported into Wanderlust.

## SM50 GPR ISCADD scaled integer addition

Pinned GM107 envytools `f102b82381f3f11cee113d16374c87091db039d9` and
[UAM Maxwell ISCADD emission](https://github.com/devkitPro/uam/blob/5a5afc2bae8b55409ab36ba45be63fcb73f68993/mesa-imported/codegen/nv50_ir_emit_gm107.cpp#L1871-L1898)
establish GPR base `5c18000000070000`, variable mask `00000f800ff0ffff`,
destination bits7:0, shifted source15:8, addend27:20 and shift43:39.
UAM emits a five-bit immediate at39 for OP_SHLADD. The accepted operation is
unsigned bit-vector `(A << shift) + B`, truncated modulo2^32. Bit43 is the
high shift bit, not IADD's extended-carry flag. NEG49/48 and CC47 remain outside
the supported subset, as do reserved44..46 and execution predicates.

Original pinned envyas reassembles both fixtures exactly and all32 shift forms.
Pinned MaxAs `54eda7af086a46c9dae1688b691968235d560164` independently matches
those32 words; its permissive `i39w8` immediate grammar does not establish
shift32..255 semantics. The pinned NAK SM50 encoder has no ISCADD/LEA encoding
and is not used as opcode evidence. The fixed six-cycle GPR scheduling subset
uses no write barrier and requires ready inputs/destination, while honoring
supported incoming waits. Only semantic and encoding evidence is used from
these MIT sources; no compiler implementation is imported.

## SM50 low-byte and low-halfword I2I widening

The pinned MIT GM107 table, [NAK SM50 I2I encoder](https://github.com/chaotic-cx/mesa-mirror/blob/e7959c6009d85d278585b2896495b2b3046d461b/src/nouveau/compiler/nak/sm50.rs#L1883-L1926)
and [UAM Maxwell I2I emission](https://github.com/devkitPro/uam/blob/5a5afc2bae8b55409ab36ba45be63fcb73f68993/mesa-imported/codegen/nv50_ir_emit_gm107.cpp#L893-L930)
agree on register opcode5ce0, destination bits7:0 and source27:20. The
validated decoder uses register mask `000000000ff000ff` and four exact fixed
bases: `5ce0000000070200` (U32 from U8), `5ce0000000070600` (U32 from U16),
`5ce0000000073200` (S32 from S8), `5ce0000000073600` (S32 from S16).
Bits9:8 encode destination size,11:10 source size,12 destination signedness
and13 source signedness. The four-base check accepts only the documented
like-signed widening subset; it does not generalize other conversion semantics.

The source lane selector41:42 is fixed zero. NEG45, CC47, ABS49 and SAT50 are
unsupported. NAK/UAM classify conversion instructions as variable latency;
Wanderlust's bounded schedule assigns WB0 and requires a matching wait before
consumption, including RZ and aliases. Original pinned envyas independently
assembles all five fixtures; pinned MaxAs independently agrees on all four
type encodings and RZ operands. The reference oracle reads the low bytes using
signed/unsigned byte interpretation, separately from WGSL's bounded mask or
shift-based sign extension. Only encoding/semantic evidence is used; no
compiler implementation is imported.

## SM50 GPR ISET integer comparison results

Pinned GM107 `f102b82381f3f11cee113d16374c87091db039d9` table2029 and
[UAM ISET emission](https://github.com/devkitPro/uam/blob/5a5afc2bae8b55409ab36ba45be63fcb73f68993/mesa-imported/codegen/nv50_ir_emit_gm107.cpp#L2038-L2081)
establish base `5b50038000070000`, variable mask `000f10000ff0ffff`, destination
bits7:0, left15:8, right27:20, signedness48, relation51:49 and BF44. The
accepted relation values1..6 are LT/EQ/LE/GT/NE/GE; F0 and T7 are valid hardware
encodings but outside this implementation. X43 and CC47 are rejected, as are
changes to the fixed AND PT combiner. UAM OP_SET uses fixed six-cycle scheduling;
this is a GPR instruction, unlike predicate-producing ISETP.

[CUDA8 set semantics](https://docs.nvidia.com/cuda/archive/8.0/parallel-thread-execution/index.html#comparison-and-selection-instructions-set)
corroborate signed/unsigned integer comparisons and true=FFFFFFFF or1.0f for
mask/BF output, false=zero. Original pinned envyas assembles all32 relation,
signedness and result combinations; the decoder intentionally accepts24.
Pinned MaxAs `54eda7af086a46c9dae1688b691968235d560164` independently matches
all twelve ordinary mask forms, but its grammar has no BF spelling. BF44 is
corroborated by GM107/UAM/envyas. The pinned NAK SM50 encoder has ISetP only,
so it is not evidence for this GPR opcode. No compiler implementation is
imported from these MIT sources. The existing ISETP condition emitter is
shared through an explicit destination name, preserving its prior WGSL bytes.

## SM50 generic PRMT byte selection

Pinned MIT [GM107 tables](https://github.com/envytools/envytools/blob/f102b82381f3f11cee113d16374c87091db039d9/envydis/gm107.c)
and [NAK SM50 emission](https://github.com/chaotic-cx/mesa-mirror/blob/e7959c6009d85d278585b2896495b2b3046d461b/src/nouveau/compiler/nak/sm50.rs#L1955-L1995)
establish GPR base `5bc0000000070000`, mask `00007f800ff0ffff`, and canonical
immediate base `36c0000000070000`, mask `00007f8ffff0ffff`. Destination is7:0,
A15:8, selector27:20 or35:20, and B46:39. SASS operand order is A,selector,B.
Bits43..46 belong to source B, not instruction modifiers. Mode48..50 is fixed
zero; unsupported fields remain fixed by the exact base/mask check. Immediate
bits36..38 and56 are excluded as noncanonical forms, without claiming that all
are invalid hardware encodings.

[NAK selector semantics](https://github.com/chaotic-cx/mesa-mirror/blob/e7959c6009d85d278585b2896495b2b3046d461b/src/nouveau/compiler/nak/ir.rs#L4997-L5155)
and [CUDA8 generic prmt](https://docs.nvidia.com/cuda/archive/8.0/parallel-thread-execution/index.html#data-movement-and-conversion-instructions-prmt)
corroborate low-sixteen-bit control, eight source bytes and sign replication.
Original pinned envyas independently assembles the committed fixtures. Pinned
MaxAs `54eda7af086a46c9dae1688b691968235d560164` agrees on the encodings and
fixed-six scheduling. Literal zero in envyas selects GPR RZ; independently
assembled MaxAs word `36c0028000070404` establishes canonical immediate zero.
UAM GM107 has no PRMT emitter and is not opcode evidence. No compiler
implementation is imported; the bounded integer helper follows these semantics.

## SM50 ordinary GPR IADD3

Pinned MIT [GM107 table](https://github.com/envytools/envytools/blob/f102b82381f3f11cee113d16374c87091db039d9/envydis/gm107.c#L1994)
and [MaxAs grammar](https://github.com/NervanaSystems/maxas/blob/54eda7af086a46c9dae1688b691968235d560164/lib/MaxAs/MaxAsGrammar.pm#L314)
establish base `5cc0000000070000`, variable mask `00007f800ff0ffff`, destination
7:0, A15:8, B27:20 and C46:39. C43..46 are register bits. Halfword selectors
31..36, RS/LS37..38, CC47, X48 and negate49..51 are fixed zero in the accepted
subset. MaxAs and GM107 differ in halfword spellings; no such form is accepted.
Original unmodified envyas and independent MaxAs agree on both fixtures'
IADD3 words and RZ/high-register words. MaxAs classifies the ordinary form as
fixed-six.

The [independent MIT Maxwell emitter](https://github.com/alula/Ryujinx/blob/4930dced25c0d9ec33096b0f2601d262daa477bf/src/Ryujinx.Graphics.Shader/Instructions/InstEmitIntegerArithmetic.cs#L343-L404)
corroborates full-width unmodified (A+B)+C. Its unresolved CC/X/corner-case
paths are outside this decoder. NAK's generic IAdd3 fold corroborates ordinary
wrapping arithmetic, but its SM50 builder uses IAdd2 and provides no native
IADD3 encoding evidence. UAM GM107 likewise has no IADD3 emitter. No compiler
implementation is copied; lowering uses direct u32 addition, and the test
oracle independently sums widened u64 values before retaining the low word.

## SM50 ordinary low-halfword XMAD

Pinned MIT [GM107 XMAD entry](https://github.com/envytools/envytools/blob/f102b82381f3f11cee113d16374c87091db039d9/envydis/gm107.c#L2031),
its type/C-mode tables1457..1472 and independently matching [MaxAs grammar](https://github.com/NervanaSystems/maxas/blob/54eda7af086a46c9dae1688b691968235d560164/lib/MaxAs/MaxAsGrammar.pm#L340-L343)
establish exact accepted bases `5b00000000070000` and `5b03000000070000`,
variable register mask `00007f800ff0ffff`, destination7:0, A15:8, B27:20 and
C46:39. Signedness is A48/B49; only UU and SS are accepted. A.H1=53, B.H1=35,
PSL36, MRG37, X38, CC47 and C-mode50..52 remain fixed zero. C43..46 are data.
Original envyas independently assembles the three committed fixtures; MaxAs
independently agrees on their XMAD words, mixed-sign words and RZ/high registers.
Mixed signedness is valid hardware but not implemented by this bounded subset.

[UAM unsigned emission](https://github.com/devkitPro/uam/blob/5a5afc2bae8b55409ab36ba45be63fcb73f68993/mesa-imported/codegen/nv50_ir_emit_gm107.cpp#L1901-L1960)
corroborates the GPR fields and unsigned form. Its signed flag emission is
coupled to H1 and does not establish ordinary signed-low16 semantics. MaxAs
x32T and [UAM latency](https://github.com/devkitPro/uam/blob/5a5afc2bae8b55409ab36ba45be63fcb73f68993/mesa-imported/codegen/nv50_ir_target_gm107.cpp#L330-L351)
corroborate fixed-six scheduling, unlike scoreboarded IMUL.

The [independent MIT Maxwell implementation](https://github.com/alula/Ryujinx/blob/4930dced25c0d9ec33096b0f2601d262daa477bf/src/Ryujinx.Graphics.Shader/Instructions/InstEmitIntegerArithmetic.cs#L565-L655)
corroborates low16 sign/zero extension, multiplication and full C addition for
ordinary Cfull with carry disabled. The signed product lies within
[-1073709056,1073741824]; unsigned product is at most65535 squared. NAK has no
XMAD encoding; its distinct IMAD opcode is not evidence. No compiler
implementation is copied. The test oracle extracts little-endian bytes as
u16/i16 and computes widened u64/i64 products independently of WGSL shifts.

## SM50 direct constant-buffer MOV

Pinned MIT [GM107 MOV table](https://github.com/envytools/envytools/blob/f102b82381f3f11cee113d16374c87091db039d9/envydis/gm107.c#L2073),
[NAK MOV encoding](https://github.com/chaotic-cx/mesa-mirror/blob/e7959c6009d85d278585b2896495b2b3046d461b/src/nouveau/compiler/nak/sm50.rs#L1926-L1952)
and [UAM emission](https://github.com/devkitPro/uam/blob/5a5afc2bae8b55409ab36ba45be63fcb73f68993/mesa-imported/codegen/nv50_ir_emit_gm107.cpp#L700-L754)
establish opcode4C98, destination7:0, word-offset33:20, slot38:34 and lane
mask42:39. NAK's constant source helper divides byte offsets by four. Envytools
interprets the14-bit word offset as signed; NAK emits it unsigned. The accepted
common subset excludes bit33, retaining byte offsets0..7FFC. This is a
conservative implementation restriction, not a declaration that other valid
hardware offsets are malformed. Exact base `4c98078000070000` and variable
mask `0000007dfff000ff` retain all lanes, PT and zero unused fields.

Original unmodified envyas and independent pinned MaxAs agree on128 combinations
of all slots and four boundary offsets. MaxAs MOV x32T establishes fixed-six
scheduling without a write barrier. This is separate from scalar LDC's
variable-latency barrier subset. No compiler code is imported. The existing
ConstantRef/constant-memory resolver and WGSL uniform lowering are reused;
vertex resource support is not inferred from shared decoder structures.

## SM50 FADD/FMUL with a direct constant source

Pinned MIT [GM107 entries2078/2080](https://github.com/envytools/envytools/blob/f102b82381f3f11cee113d16374c87091db039d9/envydis/gm107.c#L2078-L2080),
[NAK FADD](https://github.com/chaotic-cx/mesa-mirror/blob/e7959c6009d85d278585b2896495b2b3046d461b/src/nouveau/compiler/nak/sm50.rs#L481-L533),
[NAK FMUL](https://github.com/chaotic-cx/mesa-mirror/blob/e7959c6009d85d278585b2896495b2b3046d461b/src/nouveau/compiler/nak/sm50.rs#L639-L705)
and [UAM emission](https://github.com/devkitPro/uam/blob/5a5afc2bae8b55409ab36ba45be63fcb73f68993/mesa-imported/codegen/nv50_ir_emit_gm107.cpp#L1254-L1345)
corroborate direct constant-memory opcodes4C58/4C68, destination7:0 and GPR
source15:8. The CB layout reuses word-offset33:20 and slot38:34. The same
conservative nonnegative-offset subset as MOV excludes bit33: exact bases
`4c58000000070000`/`4c68000000070000`, variable mask `0000007dfff0ffff`.

Round-mode39..40 is fixed nearest, while FTZ44, CC47 and SAT50 remain zero.
FADD source modifiers45/46/48/49 are excluded; FMUL's scale41..43, DNZ45 and
shared NEG48 are excluded. PT and all other fixed fields remain checked.
Original envyas assembles the complete fixture, and independent pinned MaxAs
agrees on every instruction word and boundary/RZ examples. Fixed-six ALU
scheduling uses no write barrier. Only semantic/encoding evidence is used;
no compiler implementation is imported. The new source form lowers through
the existing WGSL f32 arithmetic contract and adds no stronger precision claim.

## SM50 predicated MOV32I

Pinned MIT [GM107 predicate table](https://github.com/envytools/envytools/blob/f102b82381f3f11cee113d16374c87091db039d9/envydis/gm107.c#L408-L412)
and [MOV32I entry](https://github.com/envytools/envytools/blob/f102b82381f3f11cee113d16374c87091db039d9/envydis/gm107.c#L2170)
identify predicate16..18/invert19, destination7:0, full immediate51:20 and
lane mask15:12. Pinned [MaxAs movement grammar](https://github.com/NervanaSystems/maxas/blob/54eda7af086a46c9dae1688b691968235d560164/lib/MaxAs/MaxAsGrammar.pm#L358)
and [predicate encoding](https://github.com/NervanaSystems/maxas/blob/54eda7af086a46c9dae1688b691968235d560164/lib/MaxAs/MaxAsGrammar.pm#L1193-L1201)
independently encode the same fields. The bounded decoder fixes predicateP0,
all four lane bits and reserved fields: base `010000000000f000`, variable
mask `000ffffffff800ff`. PT remains the separate unconditional form.

Pinned [NAK generic predicate encoding](https://github.com/chaotic-cx/mesa-mirror/blob/e7959c6009d85d278585b2896495b2b3046d461b/src/nouveau/compiler/nak/sm50.rs#L225)
and [UAM predication](https://github.com/devkitPro/uam/blob/5a5afc2bae8b55409ab36ba45be63fcb73f68993/mesa-imported/codegen/nv50_ir_emit_gm107.cpp#L238)
corroborate those predicate fields. The existing MOV fixed-six timing subset
is reused. NVIDIA's [CUDA8 predicated-execution description](https://docs.nvidia.com/cuda/archive/8.0/parallel-thread-execution/index.html#predicated-execution)
corroborates the false-guard no-write behavior; it is not used as a SASS encoding
source. Original envyas assembles both complete fixtures, and original MaxAs
independently matches positive/inverted and high-register raw-bit examples.
No compiler implementation is imported.

### Switch GPU controller identity ABI

- libnx ISC revision `feebd026ca0f5dcc2119f46ad8e0d16ad3dd4973`,
  [nvhost-ctrl-gpu.c](https://github.com/switchbrew/libnx/blob/feebd026ca0f5dcc2119f46ad8e0d16ad3dd4973/nx/source/nvidia/ioctl/nvhost-ctrl-gpu.c),
  `nvioctlNvhostCtrlGpu_GetCharacteristics` and `GetTpcMasks`: ioctl direction,
  payload sizes, inline output, and the literal nonzero address tag. No source
  implementation was copied.
- The same revision's
  [ioctl.h](https://github.com/switchbrew/libnx/blob/feebd026ca0f5dcc2119f46ad8e0d16ad3dd4973/nx/include/switch/nvidia/ioctl.h),
  `nvioctl_gpu_characteristics`: explicit field order and GM20B identity values.
  These identify the emulated chip rather than host capabilities. Wanderlust
  deliberately clears optional feature flags instead of copying the physical
  device's capability mask. The supported one-GPC mask is four bytes; no guest
  pointer or fragmented buffer acquisition is involved.

- [Switchbrew NV services revision 14790](https://switchbrew.org/w/index.php?title=NV_services&oldid=14790#NVGPU_GPU_IOCTL_GET_CHARACTERISTICS)
  documents characteristics through both Ioctl and Ioctl3 on Switch, rather than
  only the later Switch 2 variant. The original base executable independently
  requested `0xc0b04705` through service command 12. The pinned libnx revision's
  [nv.c](https://github.com/switchbrew/libnx/blob/feebd026ca0f5dcc2119f46ad8e0d16ad3dd4973/nx/source/services/nv.c),
  `nvIoctl3`, supplies one input and two auto-select output buffers. Runtime tests
  independently encode those HIPC descriptors and check both destinations.

The two transports differ in address-tag validation. The original base workload
uses a zero unused tag with an explicit Ioctl3 output. The historical
[Yuzu controller implementation, revision ebcee03b0c4e02a58eeee6152950e20ab2769283](https://ni.4a.si/anonymous/yuzu/diff/src/core?id=ebcee03b0c4e02a58eeee6152950e20ab2769283)
also routes Ioctl3 characteristics into its separate output without requiring a
nonzero incoming address tag. This is a semantic cross-check only: no GPL source
or arbitrary returned pointer constants were copied. Wanderlust preserves the
unused input tag and never dereferences it; the legacy libnx contract retains its
nonzero-tag restriction.

The same Switchbrew revision's `NVGPU_GPU_IOCTL_GET_TPC_MASKS` entry specifies
both transports, a nonzero ignored size tag, three reserved words, and two
32-bit mask slots. The pinned libnx legacy wrapper's pointer-shaped layout is
not imposed on the separate Ioctl3 output. The one-GPC GM20B model returns
`[3, 0]`, with either the first slot or both slots exposed by the bounded output.

### Switch address-space initialization field order

The ISC-licensed [libnx InitializeEx implementation at
feebd026ca0f5dcc2119f46ad8e0d16ad3dd4973](https://github.com/switchbrew/libnx/blob/feebd026ca0f5dcc2119f46ad8e0d16ad3dd4973/nx/source/nvidia/ioctl/nvhost-as-gpu.c#L120)
places flags at byte zero and large-page size at byte eight. This order is also
observable at the original executable's service boundary. Switchbrew's
[ALLOC_AS_EX description](https://switchbrew.org/wiki/NV_services#NVGPU_AS_IOCTL_ALLOC_AS_EX)
names the three trailing range fields but reverses the first and third words
relative to that consumer; its field order is not used for decoding.
The NVIDIA-authored [nvgpu header at
a35286a4b10fbcfb689aa77be42c201d2091dca2](https://github.com/LineageOS/android_kernel_nvidia_shield/blob/a35286a4b10fbcfb689aa77be42c201d2091dca2/include/uapi/linux/nvgpu.h#L239)
describes fixed-offset userspace-managed address spaces. It is semantic evidence,
not the Switch wire layout. No external emulator code is copied.

The same [libnx source's GetVARegions](https://github.com/switchbrew/libnx/blob/feebd026ca0f5dcc2119f46ad8e0d16ad3dd4973/nx/source/nvidia/ioctl/nvhost-as-gpu.c#L100)
specifies the 16-byte primary header plus two region descriptors and no pointer
dereference. [Switchbrew's GET_VA_REGIONS](https://switchbrew.org/wiki/NV_services#NVGPU_AS_IOCTL_GET_VA_REGIONS)
provides the 24-byte descriptor fields and Ioctl3 transport. Reported page counts
are derived from the adapter's actual validated start/split/end bounds; they are
not arbitrary emulator constants.

[libnx AllocSpace at the same revision](https://github.com/switchbrew/libnx/blob/feebd026ca0f5dcc2119f46ad8e0d16ad3dd4973/nx/source/nvidia/ioctl/nvhost-as-gpu.c#L17)
defines the final 64-bit field as an alignment on non-fixed input and a GPU
address on output. Flags select fixed versus automatic reservation; sparse
allocation is outside this adapter's supported subset. First-fit ordering is
not specified by this ABI and is an explicit bounded implementation policy.
The Switchbrew region-query size field is overwritten with the available
48-byte descriptor size. A zero-initialized header observed at the original
executable's service boundary is a sizing query, not a null destination pointer.

### nvmap user storage versus allocation alignment

[libnx map.c at feebd026ca0f5dcc2119f46ad8e0d16ad3dd4973](https://github.com/switchbrew/libnx/blob/feebd026ca0f5dcc2119f46ad8e0d16ad3dd4973/nx/source/nvidia/map.c#L30)
independently checks CPU page alignment, requested allocation alignment and
CPU cacheability. Its call selects flags zero/one from cacheability, contrary
to the read-only/read-write comment in the lower-level ioctl wrapper. The
adapter follows the actual consumer and retains runtime permission checks.
An original-executable boundary observation corroborated flags zero and
allocation alignment 128 KiB with only page-aligned CPU storage. Only scalar
request metadata was inspected; no original shader or executable fixture was
added.

### Horizon cache policy on device-associated storage

[Atmosphère KPageTableBase::SetMemoryAttribute at
6b0c077b4c608a3a2ee712b8dc35fee87ceb62e8](https://github.com/Atmosphere-NX/Atmosphere/blob/6b0c077b4c608a3a2ee712b8dc35fee87ceb62e8/libraries/libmesosphere/source/kern_k_page_table_base.cpp#L1531)
checks the requested attribute capability while excluding DeviceShared from its
rejection mask. The source is used as semantic evidence; GPL implementation code
is not copied. Wanderlust's stricter registration lifetime guard permits only
Uncached changes that preserve complete contiguous pins.

[NVMAP_PARAM in Switchbrew](https://switchbrew.org/wiki/NV_services#NVMAP_IOC_PARAM)
specifies SIZE, ALIGNMENT, unsupported BASE, system-memory HEAP and KIND selectors.
The pinned [libnx ioctl wrapper](https://github.com/switchbrew/libnx/blob/feebd026ca0f5dcc2119f46ad8e0d16ad3dd4973/nx/source/nvidia/ioctl/nvmap.c#L79)
corroborates the three-u32 wire payload and result position. Results come from
the actual created object and retained user-backing metadata. No pointer-shaped
value is fabricated for BASE.

### Automatic MapBufferEx ownership

[libnx nvAddressSpaceMap at feebd026ca0f5dcc2119f46ad8e0d16ad3dd4973](https://github.com/switchbrew/libnx/blob/feebd026ca0f5dcc2119f46ad8e0d16ad3dd4973/nx/source/nvidia/address_space.c#L59)
passes the address space's configured page field, zero offset and zero mapping
size for an entire user buffer. The corresponding
[ioctl wrapper](https://github.com/switchbrew/libnx/blob/feebd026ca0f5dcc2119f46ad8e0d16ad3dd4973/nx/source/nvidia/ioctl/nvhost-as-gpu.c#L58)
defines fixed bit zero and cacheable bit two. An original-executable scalar
boundary observation confirmed this shape with a 64 KiB field and a 4 KiB
allocation. The bounded software mapping uses that field for VA alignment,
preserves the exact backing extent and does not claim physical page selection.
No original command or executable bytes are committed as test fixtures.

[libnx NvKind at the same revision](https://github.com/switchbrew/libnx/blob/feebd026ca0f5dcc2119f46ad8e0d16ad3dd4973/nx/include/switch/nvidia/types.h#L247)
identifies `0xfe` as `Generic_16BX2`. NVIDIA's
[kind classification at a35286a4b10fbcfb689aa77be42c201d2091dca2](https://github.com/LineageOS/android_kernel_nvidia_shield/blob/a35286a4b10fbcfb689aa77be42c201d2091dca2/drivers/gpu/nvgpu/gk20a/kind_gk20a.c#L130)
excludes this generic kind from compressible kinds. The adapter accepts this
uncompressed mapping kind with unchanged canonical bytes; the existing surface
descriptors, rather than the ioctl, select tiling conversions. NVIDIA kernel
sources are semantic references only; no GPL implementation is imported.

[NVIDIA address-space UAPI at a35286a4b10fbcfb689aa77be42c201d2091dca2](https://github.com/LineageOS/android_kernel_nvidia_shield/blob/a35286a4b10fbcfb689aa77be42c201d2091dca2/include/uapi/linux/nvgpu.h#L781)
defines reservation ranges for later fixed mappings, rejects freeing a range
while buffers remain mapped, and unmaps individual buffers by their mapped
start. The same source distinguishes backing offset, mapping size and GPU VA.
Only these lifetime/range semantics are adopted: desktop ioctl numbers differ
from Switch and are not substituted for the pinned libnx wire identities.

### Switch GPU channel initialization and error-event transport

[libnx channel ioctl definitions at feebd026ca0f5dcc2119f46ad8e0d16ad3dd4973](https://github.com/switchbrew/libnx/blob/feebd026ca0f5dcc2119f46ad8e0d16ad3dd4973/nx/source/nvidia/ioctl/nvchannel.c#L148)
establish the 32-byte Ex2 payload, output-only fence fields, priority, timeout,
user-data and notifier wire identities. The [GPU-channel consumer](https://github.com/switchbrew/libnx/blob/feebd026ca0f5dcc2119f46ad8e0d16ad3dd4973/nx/source/nvidia/gpu_channel.c#L11)
uses flags one and zero unknown fields, followed by object-context allocation,
error-event query, notifier enablement and priority. No meaning is borrowed from
the different desktop/Linux allocation flags. Unknown Switch modes stay rejected.
[Service QueryEvent](https://github.com/switchbrew/libnx/blob/feebd026ca0f5dcc2119f46ad8e0d16ad3dd4973/nx/source/services/nv.c#L304)
uses command four, two input u32s, an NVIDIA error result and one HIPC copy handle;
[event identifiers](https://github.com/switchbrew/libnx/blob/feebd026ca0f5dcc2119f46ad8e0d16ad3dd4973/nx/include/switch/services/nv.h#L27)
distinguish channel error event three from debug and control-device events.

[Switchbrew SetErrorNotifier](https://switchbrew.org/wiki/NV_services#NVGPU_IOCTL_CHANNEL_SET_ERROR_NOTIFIER)
(also preserved in the [pinned source mirror, revision 12787d8d212a164c7d6f2dacb4e35ff6bff087e1](https://github.com/backupbrew/switchbrew/blob/12787d8d212a164c7d6f2dacb4e35ff6bff087e1/NV%20services.md#nvgpu_ioctl_channel_set_error_notifier))
explicitly distinguishes Switch's nonzero `mem` enable switch and ignored
legacy offset/size/reserved fields from Linux's shared notifier buffer. The
pinned libnx wrapper calls the same field `enable` with u32 type. A read-only
original-executable boundary observation confirmed size 4096 and mem four;
only those scalar ABI values inform the independent regression fixture.

[NVIDIA channel timeout handling at a35286a4b10fbcfb689aa77be42c201d2091dca2](https://github.com/LineageOS/android_kernel_nvidia_shield/blob/a35286a4b10fbcfb689aa77be42c201d2091dca2/drivers/gpu/nvgpu/gk20a/channel_gk20a.c#L2794)
stores the requested u32 milliseconds per channel. Its
[progress watchdog](https://github.com/LineageOS/android_kernel_nvidia_shield/blob/a35286a4b10fbcfb689aa77be42c201d2091dca2/drivers/gpu/nvgpu/gk20a/channel_gk20a.c#L1399)
measures lack of GPFIFO-GET progress rather than total workload duration.
Wanderlust uses actual host queue-completion callbacks as its explicitly limited
progress signal. It does not claim equivalent prefetch, context scheduling or
preemption. The Switch wire identity comes from the pinned libnx `SetTimeout`
wrapper, not the Linux ioctl numbering. No kernel implementation is copied.

[Pinned libnx kickoff](https://github.com/switchbrew/libnx/blob/feebd026ca0f5dcc2119f46ad8e0d16ad3dd4973/nx/source/nvidia/gpu_channel.c#L83-L119)
passes the guest-command increment reservation in the fence value and uses flag
bit eight separately from the driver's bit-one increment. The independent MIT
[submission implementation](https://github.com/alula/Ryujinx/blob/4930dced25c0d9ec33096b0f2601d262daa477bf/src/Ryujinx.HLE/HOS/Services/Nv/NvDrvServices/NvHostChannel/NvHostChannelDeviceFile.cs#L424-L469)
corroborates addition of that value and the separate driver increment of two.
Only the zero guest-reservation case is accepted here; no metadata-only
syncpoint advancement is substituted for guest commands.

The same pinned [timeslice implementation](https://github.com/alula/Ryujinx/blob/4930dced25c0d9ec33096b0f2601d262daa477bf/src/Ryujinx.HLE/HOS/Services/Nv/NvDrvServices/NvHostChannel/NvHostChannelDeviceFile.cs#L355-L413)
checks the 1000..50000 microsecond range and priority policies. Switchbrew's
[pinned device ABI table](https://github.com/backupbrew/switchbrew/blob/12787d8d212a164c7d6f2dacb4e35ff6bff087e1/NV%20services.md#nvgpu_ioctl_channel_set_timeslice)
defines the u32 payload and `0xc004481d` identity. These sources inform the
serialized policy subset; no foreign implementation code is copied.

B197 scratch registers use the same pinned NVIDIA header's
`SET_MME_SHADOW_SCRATCH(i)` (`0x3400+i*4`, full u32 values). The independent
[pinned 3D state definition](https://github.com/alula/Ryujinx/blob/4930dced25c0d9ec33096b0f2601d262daa477bf/src/Ryujinx.Graphics.Gpu/Engine/Threed/ThreedClassState.cs#L996)
corroborates 256 entries. The pinned envytools macro ISA documents READ operand
method-word addressing; its signed immediate and register source fields were
verified by assembling `maddr 0x680; read $r2 0xd1a; exit send (add $r2 0); nop;`
into words `01a00021 03468215 000010c0 00000011`. Tests retain these independent
words; no commercial program is included. Only explicitly initialized scratch
reads are supported, independently of shadow track/filter/replay behavior.

The pinned NVIDIA B197 header defines `SET_ANTI_ALIAS_ENABLE` at byte `0x1534`,
bit zero, with FALSE zero and TRUE one. The supported implementation admits only
FALSE with its existing single-sample attachment/pipeline restrictions; this
register is distinct from `SET_ANTI_ALIAS` target sample mode at `0x15d0`.

The pinned NVIDIA B197 `SET_SCISSOR_ENABLE/HORIZONTAL/VERTICAL(j)` fields establish
the `0x0e00+j*16` array stride; the independent pinned Maxwell 3D state definition
uses sixteen scissor states. Supported vertex output validation excludes viewport
index exports, which justifies selecting only slot zero while retaining other
slots without aliasing them.

[MIT Nouveau initialization](https://github.com/chaotic-cx/mesa-mirror/blob/e7959c6009d85d278585b2896495b2b3046d461b/src/gallium/drivers/nouveau/nvc0/nvc0_screen.c#L989)
sets `CSAA_ENABLE` to zero alongside disabled multisampling and the MS1 mode.
Its [register table](https://github.com/chaotic-cx/mesa-mirror/blob/e7959c6009d85d278585b2896495b2b3046d461b/src/gallium/drivers/nouveau/nvc0/nvc0_3d.xml.h#L1280)
identifies byte `0x15b4`. The same pinned
[NVK context discussion](https://github.com/chaotic-cx/mesa-mirror/blob/e7959c6009d85d278585b2896495b2b3046d461b/src/nouveau/vulkan/nvk_cmd_draw.c#L282)
notes the proprietary driver's zero write and unofficial Gallium name. This is
independent interpretation evidence, not a public NVIDIA definition of CSAA's
complete behavior. Only zero is admitted. NVIDIA's pinned B197 header independently
defines alpha-to-coverage at bit zero and alpha-to-one at bit four of `0x153c`;
only their jointly disabled value is supported.

The pinned NVIDIA B197 class defines the selector SIZE/address fields at
`0x2380/84/88`, the 16-bit `LOAD_CONSTANT_BUFFER_OFFSET` at `0x238c`, and the
full-width data ports from `0x2390`. The independent MIT
[ConstantBufferUpdater at 4930dced25c0d9ec33096b0f2601d262daa477bf](https://github.com/alula/Ryujinx/blob/4930dced25c0d9ec33096b0f2601d262daa477bf/src/Ryujinx.Graphics.Gpu/Engine/Threed/ConstantBufferUpdater.cs#L134-L153)
corroborates `selector address + byte offset`, four-byte advancement and the same
state offset across ports. Its 3D state defines sixteen data words. Wanderlust
uses its existing checked GPU VA and canonical-memory journal rather than copying
that implementation or retaining command payloads. The independent LDC shader
fixture consumes uploaded words in the real GPFIFO pixel tests.

NVIDIA's pinned B197 header defines color-reduction threshold field pairs at
bits 7:0/23:16, or 5:0/21:16 for FP11, and the separate enable at `0x0d9c`.
`SET_ALPHA_FRACTION` is 8 bits at `0x074c`. The pinned
[NVK initialization](https://github.com/chaotic-cx/mesa-mirror/blob/e7959c6009d85d278585b2896495b2b3046d461b/src/nouveau/vulkan/nvk_cmd_draw.c#L307-L332)
independently programs these thresholds while explicitly disabling reduction,
and programs alpha fraction on pre-Volta hardware. These definitions justify
retaining bounded fields while admitting rendering only under explicit inactive
controls; they do not establish implementation of active reduction or coverage.

NVIDIA's pinned B197 header independently defines window clip pairs from `0x0d00`,
the separate enable at `0x194c`, and inclusive/exclusive/clip-all values at `0x1950`.
Pinned [NVK discard-rectangle state](https://github.com/chaotic-cx/mesa-mirror/blob/e7959c6009d85d278585b2896495b2b3046d461b/src/nouveau/vulkan/nvk_cmd_draw.c#L2760-L2826)
corroborates rectangle programming separately from enable/mode. Only explicitly
disabled window clipping is admitted by this subset.

The same NVIDIA clear definitions distinguish state programming from triggering,
and make scissor-zero and respect-stencil-mask separate bits. The supported clear
masks contain only color/depth, so respect-stencil-mask has no selected stencil
component to modify. Explicitly full-attachment scissor bounds make the existing
full GPU attachment clear equivalent; other scissor bounds remain rejected before
host execution. Defined but unimplemented rectangle/viewport controls are retained
as state and rejected when used by a clear, rather than rejected by every write.

The pinned NVIDIA B197 header names six separate L2 policy registers at
`0x0218/0x1000/0x10fc/0x1290/0x12d8/0x12dc`, defines POLICY bits 5:4 with
FIRST/NORMAL/LAST values 0/1/2, and separately defines VAF system-memory volatility
at bit zero. Keeping that bit FALSE preserves the supported memory contract.
These policy writes are distinct from the separately defined flush/barrier
methods; no forced readback is inferred from a replacement preference.

The same pinned header defines current/oldest 16-bit version fields and
independent/header sampler binding enums. Pinned
[NVK Maxwell initialization](https://github.com/chaotic-cx/mesa-mirror/blob/e7959c6009d85d278585b2896495b2b3046d461b/src/nouveau/vulkan/nvk_cmd_draw.c#L330-L339)
checks SPH 3/3 and AAM 2/2. Those are the only accepted pairs in this bounded
translator. NVIDIA defines shader exception enable at bit zero of `0x1528`;
only FALSE is supported. Neither assertion relaxes binary header validation.

Pinned NVIDIA `clb06f.h` defines NOP at byte `0x0008` with a 32-bit handle, unlike
engine NO_OPERATION at `0x0100`. B197 independently defines the array-restart
policy at `0x0de8` and global restart at `0x1644`. The pinned MIT
[primitive restart state update](https://github.com/alula/Ryujinx/blob/4930dced25c0d9ec33096b0f2601d262daa477bf/src/Ryujinx.Graphics.Gpu/Engine/Threed/StateUpdater.cs#L1058-L1064)
corroborates the conjunction of global enable with indexed drawing or the array
policy. The implementation admits only the proven disabled result.

The pinned B197 header defines sixteen vertex-array instance flags at
`0x1880+index*4`, six attribute-default bits at `0x1610`, FIRST/LAST selection at
`0x1684`, OGL/D3D point centers at `0x165c`, and API call-limit encodings 0..8
for limits 0,1,2,4,8,16,32,64,128. NO_CHECK=15 is explicitly outside this subset.
The sole bit of `SET_BLEND_PER_FORMAT_ENABLE` at `0x1140` gates
SNORM8/UNORM16/SNORM16, rather than the supported RGBA8 target. Existing strict
input, topology and instruction validation keeps these otherwise inactive controls
from implying point, flat-shading, instancing, default-input or CALL support.

[NVK's canonical attribute defaults](https://github.com/chaotic-cx/mesa-mirror/blob/e7959c6009d85d278585b2896495b2b3046d461b/src/nouveau/vulkan/nvk_cmd_draw.c#L361-L368)
encode `0x0e`. Only that profile is admitted here; other bit combinations require
separate verification of component expansion and default-interface behavior.
Retaining the profile does not supply an otherwise missing guest input.


### NV902D configuration fields

Pinned NVIDIA [cl902d.h](https://github.com/NVIDIA/open-gpu-doc/blob/9fdf5c4062007929d9f4e6cbad9c9771fe61b880/classes/twod/cl902d.h)
defines clip-enable at `0x0290` bit zero, SRCCOPY=3 in operation bits 2:0 at
`0x02ac`, and memory corral size bits 9:0 at `0x0884`. These are state setters;
`PIXELS_FROM_MEMORY_SRC_Y0_INT` (`0x08dc`) is not accepted as configuration.
The pinned B197 header defines `SET_DA_OUTPUT` (`0x164c`) with the sole field
`VERTEX_ID_USES_ARRAY_START` at bit 12. NVK's pinned initialization writes TRUE;
accepting this setting does not admit unsupported VertexID shader loads.


### MME register ALU corpus

Pinned MIT [envytools macro opcode table](https://github.com/envytools/envytools/blob/f102b82381f3f11cee113d16374c87091db039d9/envydis/macro.c)
defines arithmetic function bits 21:17 for ADD=0, SUB=2, XOR=8, OR=9,
AND=10, ANDN=11 and NAND=12. Incoming-carry ADC/SBB remain outside the subset.
The `register-alu.asm` fixture was assembled with that revision's unmodified
`envyas -m macro -i`, then checked using `envydis`. Its exact little-endian words
are retained in `register-alu.words`; the generated 40-byte binary SHA-256 is
`68f69429167577cea88bf7d85e67e7b251a90385505677ba47119f77ba42d946`.
CPU tests also use separately assembled two-source instructions and fixed scalar
expectations, rather than deriving expected answers from the interpreter.


The pinned NVIDIA B197 header defines `SET_ZCULL_STATS` (`0x151c`) and
`SET_ALIASED_LINE_WIDTH_ENABLE` (`0x020c`) as single-bit controls.
`SET_RENDER_ENABLE_CONTROL` (`0x030c`) bit zero controls conditional constant
loads; only FALSE is implemented. `SET_PS_OUTPUT_SAMPLE_MASK_USAGE` (`0x0300`)
defines enable bit zero and antialias qualification bit one. The pinned NVK
initialization sets the latter pair to TRUE/ENABLE. This does not imply a guest
sample-mask export: the strict SPH interfaces require its final output word to
remain zero. ZCULL statistic reporting is not inferred from its enable setter.


### Blend zero-factor behavior

Pinned B197 defines `SET_BLEND_OPT_CONTROL` (`0x0fdc`) bit zero as
ALLOW_FLOAT_PIXEL_KILLS and `SET_BLEND_FLOAT_OPTION` (`0x19c0`) bit zero as
ZERO_TIMES_ANYTHING_IS_ZERO. The pinned NVK initialization writes TRUE to both.
The latter is admitted only for the implemented RGBA8_UNORM attachment subset,
with explicit nonfinite LDC GPU tests and rejection of float attachment formats.
[Microsoft D3D11.3 section 17.5](https://microsoft.github.io/DirectX-Specs/d3d/archive/D3D11_3_FunctionalSpec.htm#17.5%20Blending%20Precision)
requires NaN propagation including zero weights for floating attachments, and
clamping to the attachment's range for fixed-point attachments. Consequently,
accepting the normalized-target subset must not be generalized into a host
no-op for a future floating-point target implementation.


The pinned B197 definitions distinguish generated-edge DRAW_LINE=0 from
DO_NOT_DRAW_LINE=1 at `0x0f8c`; the retained boolean uses that distinction.
`SET_TWO_SIDED_STENCIL_TEST` (`0x1594`) is separate from stencil enable `0x1380`.
`SET_SPA_VERSION` (`0x0310`) has minor bits 7:0 and major bits 15:8; only the
5.3 assertion is admitted. `SET_SHADER_LOCAL_MEMORY_WINDOW` (`0x077c`) is a
full-width base field, distinct from local allocation address registers `0x0790+`.
TIR control bits 0,1,4 select raster samples for ZPASS/alpha coverage and coverage
reduction. Balanced primitive workload bits 0,4 are scheduling configuration.
`SET_RT_LAYER` (`0x15cc`) separates layer V bits 15:0 from selector bit 16.
The admitted SPH stage interfaces have no layer export and targets have one layer;
nonzero V, geometry stages, line rasterization and enabled stencil remain rejected.


Pinned B197 `SET_BINDLESS_TEXTURE_CONSTANT_BUFFER_SLOT_SELECT` (`0x2608`) occupies
bits 4:0. This selector is distinct from the GM107 TEX.B register-handle operand
already established by the independently assembled texture fixture. Changing
it must not replace the explicit LDC constant reference of an admitted TEX.B.
`SET_COMPRESSION_THRESHOLD_SAMPLES` (`0x1220`) defines enum 0 as zero, then enums
1..12 as powers of two from one to 2048. `SET_PS_WARP_WATERMARKS` (`0x1450`)
defines LOW bits 15:0 and HIGH bits 31:16. The pinned NVK source documents uncertainty
about their detailed scheduling role; only exact field retention is implemented,
not an inferred NVIDIA scheduler or compressed allocation format.


Pinned B197 defines `WAIT_FOR_IDLE` at byte `0x0110`, distinct from cache/header
invalidation at `0x1334`. The bounded implementation admits canonical argument
zero. Host completion uses the pinned wgpu queue-prefix completion callback and
`Device::poll(PollType::wait_for(index))`, or the existing finite watchdog's
nonblocking poll/ready loop when configured. These operations establish host
queue completion, not guest publication or NVIDIA syncpoint/fence semantics.
The independent test keeps guest RAM unchanged across WFI and only observes
pixels after an explicit publication; a fault after WFI does not replay the draw.


### Resumable MADDRSEND/state corpus

The pinned envytools macro source defines MADDRSEND as setting the method address,
sending result bits 17:12, and retaining the result in its destination register.
Those bits also describe the method increment. The admitted implementation uses
them for both emitted data and checked word-address advancement; it does not
assume method-address wraparound.
The independent `streaming-state.asm` fixture, assembled with unmodified pinned
`envyas -m macro -i`, yields exact words in `streaming-state.words` and binary
SHA-256 `a2509d11e1585fcc6e66c9ff055c9c84023e2d529200d294b7b42bf6172a7365`.
Its `0x00110071` instruction emits B197 WFI, not a synthetic syncpoint update.
CPU tests distinguish individual yields, exit-delay execution and a total 64-step
budget; GPU tests distinguish resumed color masks and fault-safe prior output.


### MME parameter assignment corpus

The pinned envytools `macro.c` describes assignment modes 0 (fetch), 3 (send then
fetch), 5 (set method address then fetch), and 6 (store/set address then fetch and
send). Its primary ordering for mode 3 is preserved explicitly instead of
assuming that fetching a missing argument may produce zero. Only the exact DATA
port of the launching macro/subchannel is admitted. Pending argument storage owns
scalar values and register/PC state, never a borrowed guest command array.

`parameter-state.asm` was assembled and disassembled with the unmodified pinned
envytools tools; exact words are retained in `parameter-state.words`. Its generated
28-byte binary SHA-256 is
`c12404f20e255a881031411c680184c6aa74e993077998c164e7cc8a968670f1`.
The fixture covers all four assignments, incremented scratch writes, WFI and a
parameter-dependent EXIT. CPU tests check bit-exact arguments and destination
behavior; GPU tests split arguments across aliased GPFIFO entries and reject
missing, wrong-port and unmapped continuations without replaying prior draws.

### MME conditional-branch corpus

The MIT-licensed [envytools macro ISA](https://github.com/envytools/envytools/blob/f102b82381f3f11cee113d16374c87091db039d9/envydis/macro.c)
defines the signed PC-relative 18-bit branch field, zero/nonzero tests, taken
ANNUL behavior, conditional EXIT and the prohibition on branches in delay slots.
These semantics are implemented in the same resumable interpreter used for ALU
and parameter operations; no second command interpreter is introduced.

`branch-state.asm` is independently assembled by that revision's unmodified
`envyas -m macro -i`, then disassembled with `envydis -m macro -i -n`. Its 52-byte
little-endian binary has SHA-256
`3f5b7384b2526688e4f08bee72e5449f2265a351e3ffb9e9abc5002fecb68847`.
The retained words distinguish a forward taken ANNUL branch, a skipped SEND,
a two-iteration backward delayed branch and a shared EXIT. Additional literal
assembler cases test conditional EXIT, parameter fetching and SEND in delay
slots. Unknown reserved bits, invalid targets, unready condition registers,
unuploaded code and instruction-budget exhaustion remain explicit errors.


### Bounded privileged-context initialization

The zlib-licensed [deko3d startup sequence](https://github.com/devkitPro/deko3d/blob/373bfd43d8b2d805fa2eb7d406dbb823032df958/source/maxwell/gpu_3d_base.cpp)
issues these exact address/value/mask triples during ordinary 3D initialization:
`418800/1/1`, `419a08/0/10`, `419f78/0/8`,
`404468/07ffffff/3fffffff`, `419a04/1/1`, `419a04/2/2`,
and `418800/0/01800000` (all hexadecimal).
Its [PGRAPH macro](https://github.com/devkitPro/deko3d/blob/373bfd43d8b2d805fa2eb7d406dbb823032df958/source/maxwell/pgraph.mme)
specifies a masked context-register write: WFI, request flag zero, value and mask
in firmware scratch words 0..2, firmware call 4, then polling word zero for one.
The independently implemented [Mesa NVK protocol](https://github.com/chaotic-cx/mesa-mirror/blob/e7959c6009d85d278585b2896495b2b3046d461b/src/nouveau/vulkan/nvk_cmd_draw.c)
corroborates this ordering and identifies `418800` bits 23..24 with conservative
raster state; only their zero baseline is admitted. It separately identifies
`419f78` bit 3 with fragment-helper memory-load control. The other physical
bit meanings are not generalized from these startup recipes.

The bounded HLE retains masked context bits and their known-bit masks per
channel; unknown reset bits remain unknown. It acknowledges only the listed
initialization writes after a completed WFI and a successful context update.
There is no unconditional success response, physical MMIO or firmware-code
execution. Unknown addresses, different values/masks and missing request fields
remain errors. These baseline recipes do not implement conservative rendering,
general exceptions, arbitrary privileged state changes or general Falcon calls.
No implementation source was copied from either reference.

The independent `privileged-state.asm` uses the pinned envytools assembler,
including an acknowledgement polling branch and a guest-selected color mask.
Its 68-byte little-endian binary SHA-256 is
`ad04cc0f18a5b6fcbf036fae1d8b067002ba11ed1ec80ea80777ca6e823a307d`.


### MME bitfield operations

The pinned envytools macro ISA defines EXTRINSRT (replace), EXTRSHL with a
register extraction position and immediate shift, and EXTRSHL with an immediate
extraction position and register shift (opcodes 2, 3, 4). Position and width fields
are unsigned five-bit values. The independent MIT-licensed
[Maxwell macro reference](https://github.com/alula/Ryujinx/blob/4930dced25c0d9ec33096b0f2601d262daa477bf/src/Ryujinx.Graphics.Gpu/Engine/MME/MacroInterpreter.cs)
corroborates logical extraction, zero-width masks and 32-bit destination
truncation. Unused fields must be zero. Variable shift operands above 31 remain
explicitly unsupported; host-language shift-count masking is not presumed to be
a hardware guarantee.

`bitfield-state.asm` independently assembles all three operations with the pinned
unmodified envytools tools. The 28-byte binary SHA-256 is
`2c7e4f1d950e744aabed4b0313cf4f5413e6be9d7bba1314958ff57d007e410e`.
A separate scalar bit-by-bit oracle validates 1,536 deterministic operand/field
combinations, including zero widths, high bits and truncated source/destination
edges. Alias and uninitialized-register tests do not use the production mask
expression as their expected-value oracle.


### Hierarchical-culling configuration without metadata emulation

Pinned NVIDIA `clb197.h` defines `SET_ZCULL` at `0x1968` with independent depth
bit 0 and stencil bit 4; `SET_ZCULL_BOUNDS` at `0x196c` with unbounded-minimum bit 0
and unbounded-maximum bit 4; and `SET_ACTIVE_ZCULL_REGION` at `0x1590` with six-bit
ID. `SET_ZCULL_CRITERION` at `0x0dd8` defines SFUNC values 0..7, SREF in 23:16,
SMASK in 31:24, and separate no-invalidate/force-match flags. Only zero flags are
admitted by this implementation.

The pinned Mesa `nvk_cmd_draw.c` initializes these controls (lines 508..515)
separately from optional ZCULL auxiliary-plane allocation and region activation
(lines 1496..1593). `nvc0_screen.c` independently uses region `0x3f` to disable
hierarchical culling. These controls do not replace depth comparison, depth
write-enable or functional stencil state. Wanderlust retains them while doing
complete per-fragment host tests, without creating a hierarchy or synthesizing
ZCULL statistics/memory. The pinned deko3d `state.mme` independently emits the
stencil criterion from a cached scratch value; the admitted macro output runs
through the same register validation as a direct method.


### Instrumentation initialization fields

NVIDIA `clb197.h` at pinned revision `9fdf5c4` defines full-u32 fields for
`SET_INSTRUMENTATION_METHOD_HEADER` (`0x0150`) and DATA (`0x0154`). The pinned
deko3d `gpu_3d_base.cpp` writes `0x49000000` then `0x49000001` during engine
initialization. Wanderlust admits and retains only this exact ordered pair.
The numeric payload is opaque; no meaning of other packets is inferred from
its prefix. No instrumented-method execution, profiler, trace producer, timing
result or instrumentation completion event is implemented.


### Validated MME method routing corpus

The pinned envytools `macro.c` defines SEND as dispatching its result at the
current method address, then advancing by the encoded increment. It does not
restrict hardware SEND to color masks. Wanderlust routes supported emissions
through its existing B197 method validator and backend, retaining conservative
restrictions against engine rebinding, RAM/entry modification and recursion.
No second draw interpreter, memory mapper or resource resolver is introduced.

Independent pinned-envyas programs exercise the supported path:

| Assembly fixture | Binary SHA-256 |
| --- | --- |
| `cb-selectors.asm` | `aceb47b74dd2851bd214f40a41c2a4944bf13314275e53dce5f57c96a1015494` |
| `indexed-draw.asm` | `f3a0376feab8ee6fb9c68d4691341bb65d8e27d861b87ba3c1e884a7b7b6c7de` |
| `constant-write.asm` | `ab9060f35795745f004c1c6c04bae93f2e5e8533931b9f9dffcb1ede401c1482` |

The binding fixture removes the prior fragment CB binding before each macro,
so ignoring its output cannot still produce successful LDC/TEX pixels. The draw
fixture removes every caller-side draw method. The write fixture supplies raw
u32 color data through guest DATA arguments and the real checked CB upload ports;
translated LDC reads those mapped bytes. Separate literal assembled programs
validate color clear dispatch and rejected attempts to rewrite running MME RAM.
