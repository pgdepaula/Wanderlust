# Graphics execution and memory boundaries

`wl-gpu` executes guest-format GPFIFO/B06F/B197 commands through translated
Maxwell shaders and wgpu. GpuDriver owns channel/address-space lifetimes; deferred
rendering separates queue acceptance, host completion and explicit guest-memory
publication. Hardware-state draws use guest geometry, resource descriptors and
color/depth attachments. The older direct command executor remains a separate
headless validation helper with synchronous packed-RGBA readback. Both paths have
executed on Windows D3D12/Vulkan and WSL OpenGL using synthetic workloads; no
original-game GPU command or Tears of the Kingdom frame is established.

## Direct validation commands

`command::Command` separates resource creation, borrowed buffer uploads, indexed
draws and resource removal from wgpu encoding. These commands are an execution
boundary, not Maxwell method packets or NVN entry points. Vertex records contain
six little-endian `f32` values (x, y, r, g, b, a); indices are little-endian `u16`.
The pipeline uses triangle lists, no blending, no depth attachment and one sample.

A complete stream is validated against a cloned resource descriptor/initialization
table before backend mutation. Validation rejects duplicate or unknown IDs, zero
and oversized buffers, non-four-byte-aligned sizes/uploads, overflow, wrong usage,
uninitialized bindings, out-of-bounds uploads/index spans and draws after removal.
`DrawTexture` samples a separately synchronized resident texture; the executor
checks its ID and mip bounds before any backend mutation.
Index *values* are not checked against vertex count; this boundary relies on wgpu's
robust resource accesses and does not claim guest-equivalent out-of-range behavior.
Whole-table validation currently allocates and clones intervals each execution.

Queue writes execute before the next submitted command buffer. Therefore an upload
after a draw in one stream is rejected. The caller must split such work into ordered
submissions. Destroy drops the CPU resource handle; it does not call wgpu's explicit
`Buffer::destroy` while an encoded draw can still reference the allocation.

## Address translation and borrowing

`GuestAddress` and `GpuAddress` are distinct types. `AddressMap` stores GPU-to-guest
range translations, not a duplicate process memory image. Checked insertion rejects
GPU overlaps and overflow, while distinct GPU mappings can alias the same guest
range. A read must fit one GPU mapping and one contiguous runtime-provided borrow.
Cross-mapping gathers remain unsupported. Exact mapping removal is checked by
AddressMap; GpuDriver separately owns backing identities, mapping lifetimes and
mutation restrictions. AddressMap alone does not implement Horizon permissions.

`GuestMemory::read` leaves permissions, storage and fault policy to its implementation.
`SliceMemory` is a bounded test adapter; it does not implement Horizon memory or
permission handling. No production runtime adapter is connected. Address resolution
never treats a guest pointer as a host pointer. Aliases resolve to the same bytes;
TextureChanges and resource stamps invalidate caches using canonical resolved
guest ranges and memory epochs, independently of numerical GPU VA aliases.

`DirtyRanges` coalesces adjacent/overlapping intervals and preserves gaps. It can
represent canonical backing offsets, independent of GPU aliases. The current test
uses explicitly selected dirty buffer offsets. GpuDriver::write_backing records
checked canonical writes; explicit publication exposes GPU-written attachment
bytes with retryable atomic adapter writes. Automatic production-runtime CPU
write tracking is not installed. Dirtiness is acknowledged only after successful
transfer, not when merely planning it.

The CPU representation principles follow the inspected ToThePast byte-memory and
machine-boundary contracts: preserve raw bytes, interpret byte order at a named
boundary, keep addresses distinct from storage offsets, borrow contiguous data,
and measure allocations rather than infer their absence. The inspected source,
technical documentation and keyword memory contained no definition of the ZCA/LCA
acronyms; this implementation assigns them no new expansion or unverified invariant.

## Copies, lifetime and completion

The execution path has explicit copy boundaries:

1. Resolving and borrowing a contiguous guest range creates no payload copy.
2. `Queue::write_buffer` copies the borrow into wgpu-owned staging immediately.
   Native wgpu 26 allocates staging for each such call and releases it after GPU
   completion. This is an upload, not zero-copy GPU access.
3. Tegra block-linear conversion produces one owned linear byte vector. The
   checked wrapper bounds dimensions and source size before invoking the library.
4. Texture-to-buffer readback transfers padded rows. After completion, packing the
   mapped rows into the returned vector performs another CPU copy.

`execute` retains all resource handles during encoding, submits once, requests
mapping, polls the specific submission, waits for the mapping callback, copies the
mapped bytes, drops the mapped view and unmaps. There are no synchronization sleeps.
This intentionally synchronous readback path is suitable for correctness tests;
one wait per frame is unsuitable for a high-throughput presentation loop.

Buffers and sampled textures persist by ID until removed. The target, view and readback allocation persist
until the extent changes. One pipeline and optional timestamp-query allocation
persist for the executor lifetime. Sampled textures retain a view/bind group per mip.
Descriptor hashing, cross-resource texture sharing,
asynchronous eviction and staging reuse are not implemented. GPU validation and
allocation errors are captured in error scopes; a backend failure prevents reuse
of potentially inconsistent executor state. Ordinary preflight errors permit retry.

## Formats and capabilities

Rendering/readback targets implement `Rgba8Unorm`. Guest texture sampling implements
RGBA8, BC1 RGBA and a validated ASTC LDR 4×4 subset, in UNORM and sRGB interpretations, with nearest/bilinear clamp-to-edge and explicitly selected 2D mips.
`Format` describes block geometry
for RGBA8 linear/sRGB, BC1 RGBA, BC3 RGBA and ASTC 4×4; geometry is not codec or
backend sampling support. `BlockLinear2d` uses `tegra_swizzle` for one 2D mip with
GOB height 1, 2, 4, 8, 16 or 32, and a 64 MiB conversion budget. Compressed input
uses block coordinates. It preserves encoded block bytes and does not decompress
ASTC or BCn itself. A separate compressed block decoder follows deswizzling. `TextureDesc` assembles 2D mip chains using the library's per-mip GOB
height reduction and successive padded mip sizes. Sampled arrays, depth/stencil textures and multisampling remain unsupported.
The separate B197 attachment path supports the bounded ZF32 depth format
specified below; this does not imply sampled-depth or stencil support.

## Guest texture transfers and coherence

`sync_texture` resolves every mip through `AddressMap`, reads through `GuestMemory`,
and converts tiled mips before allocating or mutating host textures. Packed linear
input stays borrowed until `Queue::write_texture` copies it. Texture uploads use
tight rows; that API does **not** require the 256-byte row pitch used by encoded
buffer/texture copies. The texture sampling shader executes a fullscreen triangle;
no CPU code writes the expected image to the render target. One view per mip makes
the selected source level explicit, including narrow final mips. sRGB texture views
perform RGB-to-linear sampling; alpha remains linear.

`TextureChanges` is a bounded journal of canonical guest byte writes with an
explicit memory epoch. All aliases query the same journal. A cached mip records
its resolved guest interval and content stamp; changed mappings, epochs, layouts,
formats or contents cannot reuse the old stamp. The runtime adapter must notify
every backing write and change epoch when the same guest address acquires a new
backing/process identity. No runtime adapter is installed by this graphics code.
After 64 journal entries, eviction advances a conservative invalidation floor.

Dirty tiled bytes invalidate whole affected mips, including padding. They are not
misinterpreted as row-major rectangles. Overlapping writes coalesce into one upload
per affected mip at synchronization. Unchanged textures perform no payload reads,
conversion or uploads. Successful synchronization acknowledges wgpu-owned queue
staging acceptance; it does not assert GPU completion. Uploads do not insert waits.
Texture removal drops CPU handles, leaving submitted references owned by wgpu.

Adapter information, all supported features, enabled features, adapter limits and
requested device limits are exposed. Timestamp queries and native BC compression are optionally enabled;
ASTC features are reported but not required or enabled. Device limits use
conservative downlevel defaults with the adapter's resolution limits. The baseline
does not require descriptor indexing, push constants or pipeline-cache extensions.

`fifo::Decoder` separately decodes Maxwell B06F pushbuffer method packets into
subchannel, byte method offset and value. It handles incrementing, nonincrementing,
increment-once, immediate and exact-zero NOP packets, preserves packet state across
input chunks, rejects unsupported opcodes and method-address overflow, and detects
truncation at a logical stream end. It retains no source borrows or heap storage.
The caller supplies little-endian decoded words and stops the channel on a fault.
The packet decoder itself does not execute engine registers. The separate bounded
B197 texture state interpreter is described below. Legacy/tertiary packets and end-segment control are rejected.

Authentic game rendering additionally requires NV service/channel setup, GPFIFO
entry fetching and engine state, resource descriptors, synchronization semantics
and a general Maxwell shader decoder/lowering path. The constant-fragment
experiment below handles only two machine operations. Naga validates host WGSL;
it cannot decode Maxwell machine instructions. See [sources](sources.md)
and [executed validation](validation.md).


## Compressed sampling and transfer policy

`TextureUploadPolicy::Auto` selects native BC1 on negotiated-capable adapters when
base dimensions are multiples of four. Each encoded mip upload rounds its copy
extent to physical block dimensions, including 1×1 final mips. Other BC1 extents
and `Software` use BC1-alpha decoding into RGBA8. ASTC always uses software RGBA8:
unbounded LDR void extents and single-partition LDR endpoint modes are supported;
HDR, multipartition, bounded void extent and illegal encodings return errors.
Footprints other than 4×4 are not represented. Edge pixels crop to the declared
extent. Encoded sRGB values remain encoded until the host sRGB texture view samples
them; alpha is never transferred through the sRGB curve.

Host format participates in resident identity, so switching native/fallback policy
cannot reuse an incompatible resource. All changed mips are validated/converted
before any host mutation. A bad compressed block preserves prior resident data
and content stamps. Both encoded source and decoded image allocations are bounded;
a decoded surface may not exceed 64 MiB. Unchanged compressed resources do not
repeat decompression or uploads. Dirty compressed writes conservatively invalidate
whole mips, not an imagined linear byte rectangle. Partial block uploads and
reusable staging pools are not implemented.

`decompress_bytes` counts produced RGBA bytes; `deswizzle_bytes` counts linearized
encoded bytes; `guest_texture_bytes` counts borrowed guest source reads, including
padding. `temporary_allocations` counts owned image conversion buffers, excluding
metadata and driver storage. The texture benchmark additionally measures global
Rust allocator traffic around synchronization. CPU stage durations measure source
access, deswizzle, decompression, and host upload preparation separately. Readback
waits remain explicit. These counters do not imply zero-copy driver transfers.

## Maxwell texture state boundary

`maxwell::Maxwell3d` consumes `fifo::MethodWrite` with byte offsets, for one B197
engine instance shared by its bound subchannels. It implements only:

| Byte method | Implemented interpretation |
|---|---|
| `0x0000` | Bind a documented engine class, engine instance zero |
| `0x0100` | Documented no-operation |
| `0x0f10` | Select modern Maxwell texture headers, value one |
| `0x155c`, `0x1560`, `0x1564` | Sampler pool upper/lower address and inclusive maximum index |
| `0x1574`, `0x1578`, `0x157c` | Image header pool upper/lower address and inclusive maximum index |
| `0x1424`, `0x1428` | NO_WFI sampler/header cache invalidation events, all or tagged line |

Method writes and faults allocate no heap storage. Unsupported classes, unbound
subchannels, invalid field bits and unimplemented methods fail explicitly; the
caller must stop a channel on fault. WFI invalidations and semaphores remain
unsupported. Render targets, draw registers and shader program state are handled
by the separate paths below. General MME programs remain unsupported. NO_WFI cache
events advance descriptor epochs without asserting completion or guest memory writes.

The eight subchannels retain typed class bindings. `SET_OBJECT` recognizes
MAXWELL_B (`0xb197`), MAXWELL_COMPUTE_B (`0xb1c0`), MAXWELL_DMA_COPY_A
(`0xb0b5`), KEPLER_INLINE_TO_MEMORY_B (`0xa140`) and FERMI_TWOD_A (`0x902d`).
All define a no-operation at `0x0100`. Auxiliary bindings and their NOPs can
coexist with a B197 draw; their other methods fail with the actual class and
method, before touching 3D registers. This is not compute, DMA, inline-upload or
2D execution support. Shader/texture lookup requires an actual B197 binding,
including after rebinding a subchannel away from 3D. Unknown classes and engine
instances fail without replacing the prior binding.

### Bounded MME state macros

B197 `0x0114/0x0118` select and upload instruction RAM; `0x011c/0x0120`
select and bind entry RAM. Both data ports auto-increment their respective word
or entry pointers. The implementation budgets 2048 instruction words and 128
entries per channel; these are explicit supported limits, not universal hardware
RAM-size claims. Instruction storage is owned and allocated on first upload,
then reused. Invalid pointer/data writes preserve previously uploaded contents.

Entry launch methods `0x3800 + slot*8` execute a bounded subset:
32-bit wrapping register arithmetic, signed 18-bit ADD-immediate, MOV,
method-address assignment, SEND and MADDRSEND with bounded method increments. R0 remains zero,
R1 receives the launch argument, and other registers require prior initialization.
EXIT executes its following delay slot. A resumable invocation executes at most
1024 instructions in total across all yields. Emissions reuse the strict B197
method dispatcher, with rebinding, running-program mutation and recursive
launches excluded. The backend
executes each effect before resuming the invocation; state/register writes and
reads cannot run ahead of an earlier wait. Existing raster validation and
pipeline identity remain active.
An independently assembled macro changes a real textured draw from red-only to
RGBA writes by changing its guest argument, without recompiling the guest shaders.

`SET_MME_SHADOW_SCRATCH(i)` at `0x3400 + i*4`, for 256 words, retains exact
u32 state per channel. MME READ-immediate can consume explicitly written scratch
values using a register plus signed 18-bit method-word offset. Reads of unwritten
scratch or other registers fail; no power-on value or general register-read
behavior is inferred. A separate independently assembled scratch-read macro
changes real GPU color masks without changing the shader binaries. Scratch
storage is distinct from the unsupported shadow-RAM tracking/replay modes.

Branches, other register reads, address wraparound, macro-generated draws and
general shadow/replay behavior remain unsupported.
Multiple supported state/wait emissions execute in order. A later fault does not
roll back earlier accepted effects or replay prior GPU work. The pure
`MacroEngine::execute` compatibility helper still preflights exactly one emission,
using the same interpreter core; it is not the production dispatch path.
Ordinary GPU channel fault/publication semantics remain unchanged.

Pool addressing is 40-bit, descriptors are 32 bytes, limits are inclusive and
index arithmetic is checked before mapped memory reads. Pool bases must be 32-byte
aligned in this implementation; the public method header does not establish a
hardware alignment fault rule. Modern block-linear TICs preserve the separate
48-bit image address, 16-bit width/height-minus-one, GOB block height, allocated and
view mip range, UNORM channel types, identity swizzle and sRGB flag. Supported
formats are A8B8G8R8 (`0x08`), DXT1 (`0x24`) and ASTC 4×4 (`0x40`). Accepted textures
are normalized, single-layer 2D, without sparse modes, depth, multisampling or mip
subviews. Unsupported fields retain word/bit diagnostics instead of being masked
into a plausible descriptor.

The TSC subset accepts clamp-to-edge in all axes, matching nearest or linear
minification/magnification, and minimum/maximum LOD zero. MIP_NONE and MIP_POINT
are equivalent under that clamp and the fixed LOD-zero shader. Comparison,
anisotropy, reduction, LOD bias, border values and normalization overrides fail.
The synthetic explicit-mip draw API remains separate from these guest sampler
semantics. Future general shader execution cannot infer the original sampler
state from the reduced filter field alone.

`resolve_texture` reads mapped TIC/TSC memory every call, so pool changes and
descriptor backing updates cannot leave stale decoded records. It accepts explicit
image/sampler indices supplied by its caller. They are **not** obtained from
`SET_BINDLESS_TEXTURE`: that register selects a constant-buffer slot, not a texture.
The independent pool test still supplies descriptor indices; the separate
constant-fragment experiment does not execute shader program registers. The tested
bridge runs literal FIFO pool methods, descriptor resolution, guest texture reads,
conversion and real sampled draw/readback. The last draw is the existing synthetic
execution command with fixed WGSL, not a decoded Maxwell draw or shader.


## Bounded binary fragment translation

`shader::ConstantFragment` consumes an 80-byte NVIDIA shader program header plus
complete 32-byte Maxwell code bundles. The accepted PS header declares target-zero
RGBA exports from R0–R3, without resource, attribute or side-effect fields. It
supports unpredicated full-mask MOV32I to those four registers and exact unconditional
EXIT. Canonical stall-six scheduling words and NOP padding are checked separately
from instructions. All output registers must be initialized; unknown opcodes,
predicates, lane masks, header fields, scheduling variants, nonfinite immediates,
missing EXIT and noncanonical padding fail explicitly. Code is bounded to 4 KiB.
The valid decoding path allocates no heap storage.

Translation interprets immediate contents as floating-point register bits and emits
WGSL using `bitcast<f32>`, preserving those bits rather than converting integer
values numerically. `Renderer::set_constant_fragment` compiles that fragment against
the retained original vertex shader and pipeline layout. The latest translated
pipeline is cached; repeating it or restoring the default requires no new shader
or pipeline allocation. Replacement drops handles without explicitly destroying
objects still referenced by wgpu. `pipeline_allocations`, `shader_module_allocations`
and `shader_cache_hits` count these API operations, excluding driver internals.

This is a real binary-input translation experiment with independently assembled
synthetic code, not a general Maxwell shader compiler. It does not consume a
0x30-byte NVN wrapper, read shader registers or constant buffers itself, translate TEX,
accept vertex programs, fetch varyings, execute arithmetic/branches, or connect
source shader-program state to a guest draw. Texture sampling still uses its fixed
WGSL path. Both experiments share the actual wgpu execution and resource backend.


## Guest-backed straight-line fragment resources

`shader_program::FragmentProgram` preserves raw-register dataflow separately from
`ConstantFragment`. It accepts full-mask unpredicated MOV32I and register MOV,
scalar B32 LDC with RZ base and nonnegative aligned signed-16-bit byte offsets,
and unconditional EXIT under the existing strict resource-free PS header. At most
64 distinct constant word references are packed into one 256-byte uniform buffer;
WGSL depends on code and reference locations, never on mutable constant values.
The program is bounded to 4 KiB of code. It does not implement indirect LDC,
subword/vector access, predicate execution or branches.

The supported scoreboard subset uses only write barrier zero, no read barriers,
no reuse, stall six and wait masks zero/one. A load requires an unused barrier;
wait-one releases pending destinations before another producer or consumer. Pending
register overwrites and unready MOV/export reads fail. This is intentionally more
restrictive than valid Maxwell scheduling. Source byte offsets identify faults.

B197 selector methods 0x2380/84/88 establish size and 40-bit GPU address.
`BIND_GROUP_CONSTANT_BUFFER(j)` at 0x2410+j*32 snapshots the selector into group
j (0..4) and slot (0..31); bit zero controls validity and bits 8:4 select the slot.
Fragment group four differs from pixel pipeline index five. Selector sizes accepted
here are nonzero, four-byte aligned and at most 64 KiB: an implementation subset,
not the limit of the hardware's 17-bit size field. Constant access validates the
bound size and checked AddressMap/GuestMemory read. Inline constant upload methods
are unsupported. Updating selectors alone does not mutate existing bindings.

`set_resource_fragment` caches the latest translated pipeline; the separate
`sync_fragment_resources` packs referenced guest words into persistent owned GPU
storage. Canonical guest ranges and the existing write-journal epoch/version skip
unchanged reads even across GPU aliases. Backing replacement requires a new epoch;
CPU/runtime writes must be reported before synchronization. A failed preflight
makes the harness unready and prevents fallback to stale successful values.
Only the enclosing changed word interval is uploaded, without a GPU wait. Queue
acceptance is not completion; wgpu owns staging and submitted resource references.

`Command::DrawFragment` is explicitly a fullscreen-triangle fragment test harness.
It executes the translated program and synchronized resources but does not represent
a B197 hardware draw or vertex shader. Existing renderer and texture paths remain
available. No runtime memory owner or runtime ABI has been changed.

## Binary texture-resource semantics

The straight-line program additionally accepts one unpredicated TEX.B.LZ 2D,
RGBA destination mask, float results, normalized coordinates, no offsets, shadow,
array, sparse or derivative modifiers. Two consecutive coordinate registers feed
`textureSampleLevel(...,0)`. The handle register must have scalar LDC provenance
(possibly through MOV); arithmetic/dynamic handle selection is unsupported. Raw
register bits remain integers until coordinate/output float bitcasts.

A 32-bit combined handle resolves TIC from bits 19:0 and TSC from bits 31:20.
The runtime resolves both pools independently, validates limits and descriptors,
and binds the resulting texture/sampler to a stable host group-one interface.
No GPU address or changing handle value enters WGSL. TEX.B does not use
SET_BINDLESS_TEXTURE: that method selects a constant-buffer slot for the distinct
bound-TEX encoding, which remains unsupported.

IPA pass/default-sample for generic attribute zero x/y (byte offsets 0x80/0x84)
provides screen-linear coordinates. The accepted SPH variant declares exactly
those two screen-linear components (word six 0xf); other interpolation/header
variants fail. IPA and TEX use the same conservative barrier-zero dependency
subset as LDC. The harness vertex shader provides genuine interpolated UV values;
no original vertex binary is executed. Hardware geometry/draw state remains absent.

Descriptor cache identity includes canonical TIC/TSC backing ranges, journal
versions, mapping epoch and engine invalidation epochs. Changed descriptor handles,
sampler fields, pools or mappings cannot retain stale bindings. Existing texture
conversion, format fallback and dirty-mip caching are reused. Internal texture IDs
are allocated from unused entries; dropping/replacing a resident program releases
its cached texture entry, while wgpu retains submitted/bound resource references.
No explicit resource destruction occurs before completion. Every harness draw
requires a successful synchronization; an invalid handle disables the draw.

## Guest pixel-program selection

B197 `SET_PROGRAM_REGION_A/B` (0x1608/0x160c) establish a 40-bit code base.
Pixel pipeline index five accepts `SET_PIPELINE_SHADER(5)` (0x2140) type five
with explicit enable/disable, `SET_PIPELINE_PROGRAM(5)` (0x2144) byte offset,
`SET_PIPELINE_REGISTER_COUNT(5)` (0x214c) and `SET_PIPELINE_BINDING(5)` (0x2150)
constant-binding group zero through four. Complete state is required; no previous
shader or implicit group is substituted. Decoded register usage must fit the
selected count. Other enabled shader stages and unimplemented methods fault.

`sync_selected_fragment` resolves base+offset through AddressMap/GuestMemory,
reads the 80-byte SPH and scans complete scheduled 32-byte bundles to the first
exact unconditional EXIT, within a 4 KiB code budget. The final bundle's padding
is validated. There is no fabricated B197 program-length register. Short, unmapped,
overflowing, unsupported or unterminated programs fail with the harness unready.
Code addresses, fetch spans and pool addresses remain GPU VAs, never host pointers.

The latest code-read cache uses canonical guest start/span and write-journal
version/epoch. GPU aliases of that backing reuse it; a remap or code write forces
fetch/decode. Its translated-program/pipeline identity is separate from constant
values, descriptor handles and texture contents. Manual installation of another
translated program invalidates the selected-code cache. Resource-only writes
preserve code; register-count and group changes are checked even on a code hit.
Program and texture caches retain the latest resident program, not an unbounded
compiler cache. Revisited evicted programs may compile again.

Literal B06F packets now select the pixel binary and its constant binding group;
the binary's LDC and TEX choose resource data and descriptors. The caller no longer
passes a shader fixture or TIC/TSC indices to this path. The original fragment-only API retains its explicit fullscreen validation harness.
The paired vertex and hardware draw paths below resolve guest geometry and targets
separately. Nvhost service integration is not implemented.


## Translated vertex stage and linkage

The bounded straight-line decoder shares scheduling and MOV handling across
strict stage wrappers. Fragment SPH acceptance remains unchanged. Vertex VTG SPH
accepts generic-zero position/generic-one float2 input, position xyzw and
generic-zero xy output. ALD B32/B64/B96/B128 accepts direct aligned attributes
within those input vectors, RZ addressing, no output/per-patch/indirect modifiers,
and the existing conservative barrier-zero/wait-one subset. AST accepts direct
position/generic-zero output groups from initialized, ready GPRs. All six output
components must have been written before EXIT. Other vertex instructions, including
vertex resources, built-ins and control flow, are unsupported.

The translated vertex entry exports actual decoded clip-space position and
screen-linear UV to the existing translated fragment stage. Typed stage wrappers
prevent reuse of fragment header acceptance for vertex binaries. Pipelines pair
vertex/fragment semantics, position format, stride and offsets; changing
geometry bytes does not change program identity. The resident fragment retains its
shader module and binding layout for paired pipelines. Wgpu owns resource lifetimes
across submitted work; no raw guest borrow is retained.

`set_vertex_program`, `upload_vertices` and `Command::DrawVertices` are intermediate
validation boundaries, not B197 draw methods. The initial vertex test provides its
buffer explicitly and checks non-fullscreen rasterization. Guest vertex arrays,
viewport, target selection and hardware draw initiation require separate B197
resolution; this interface does not imply their support.


## Guest vertex arrays

Vertex pipeline index one uses B197 0x2040/0x2044/0x204c/0x2050 for vertex type
one, program offset, GPR budget and binding group. Stage-specific code caches use
checked canonical backing spans and the shared write journal, separately from the
pixel cache. Shader fetch and termination scanning are shared across both stages.

The initial stream-zero subset uses 0x1c00..0x1c0c for enabled stride, 40-bit
address and frequency one; 0x1880 explicitly selects noninstanced fetching.
0x1f00/0x1f04 establish an inclusive byte limit. Attribute A0/A1 at 0x1160/0x1164
accept active stream-zero FLOAT R32_G32_B32_A32 and R32_G32, respectively, with
checked four-byte-aligned offsets within the stride. All fields must be explicit.
Other formats/streams/instancing are unsupported. The supported fetch span includes
the final vertex's complete stride; buffers without that trailing padding are
currently rejected even if all individual attributes would fit.

`sync_selected_vertices` resolves code and array state without host binary or
geometry arguments. Borrowed vertex bytes are uploaded directly to a persistent
wgpu buffer; no permanent CPU shadow is kept. Unchanged canonical backing/version
skips reads and uploads. Position/UV writes and remapping synchronize data without
changing shader code. Paired-pipeline changes preserve a compatible vertex buffer.
Any failed resolution disables vertex drawing. This intermediate API takes a draw
range; the hardware submission boundary below obtains it from the B197 method.


## Hardware channel draw and guest color target

`Renderer::submit_pushbuffer(channel, words, map, memory, changes)` consumes the
existing B06F decoder and B197 engine in channel order. Its arguments contain no
host geometry, shader binary, descriptor index, draw count or target extent.
`Channel` owns those existing state objects and a fault latch, not another command
interpreter. Chunks may split packets; `Channel::finish` validates the actual end
and latches truncation. Any packet, method, resource or draw fault stops the channel.
Previously completed draws remain visible; failed later state cannot execute a
stale resident pipeline. Replacing a faulted channel is explicit.

B197 `DRAW_VERTEX_ARRAY_BEGIN_END_INSTANCE_FIRST` at byte method 0x1214 supplies
start index bits 15:0, count 27:16 and topology 31:28. Only nonempty triangle lists
(topology four, count divisible by three) and a single instance are accepted.
Literal FIFO words `0x20010485, 0x40030000` initiate three vertices starting at zero.
The method resolves the existing selected vertex/pixel programs, constant buffers,
TIC/TSC resources and stream-zero attributes before encoding `DrawVertices` through
the existing renderer. This uses actual decoded vertex exports and screen-linear
UV linkage, never the fragment harness's fullscreen vertex entry.

Target zero uses 0x0800..0x0820: address A/B, width, height, format, memory layout,
third dimension, array pitch and layer. The accepted subset is A8B8G8R8 (`0xd5`,
little-endian RGBA bytes), pitch (`0x1000`) or bounded block-linear layout,
third dimension one, layer zero and a 256-byte-aligned 40-bit address. Block-linear
array stride and footprint rules are specified under Block-linear color storage. The address alignment is a
supported-subset restriction, not a universal hardware alignment claim. In pitch
layout **WIDTH is byte pitch**. It must be positive and divisible by four; storage
width is pitch/4. Logical visible bounds come from surface/viewport clipping and
scissor. Padding pixels remain initialized guest data and are preserved.
Layered, compressed and multisampled color targets are unsupported.
The separate bounded ZF32 depth attachment is described below.
Block-linear RGBA8 target conversion is implemented separately from sampled textures.

Viewport zero uses scale/offset 0x0a00..0x0a14, identity coordinate swizzle at
0x0a18, clip rectangle/depth at 0x0c00..0x0c0c, surface clips 0x0ff4/0x0ff8 and
enabled scissor 0x0e00..0x0e08. The subset requires positive X scale, nonzero Y scale,
canonical Z scale/offset (one/zero for 0..W clipping, one-half/one-half for
-W..W clipping), upper-left origin, zero window offsets and half-integer pixel
centers. Wgpu viewport/scissor are derived from
those fields. Positive Maxwell Y scale negates translated clip Y to implement
`window_y = offset_y + scale_y * clip_y/clip_w`; negative scale needs no negation.
The flip flag participates in paired-pipeline identity; viewport dimensions remain
dynamic state. Fractional viewport rectangles are explicitly unsupported because
wgpu-hal 26.0.6 GLES truncates viewport coordinates/extents to integers. Empty or
out-of-bounds viewport/clip intersections are also explicitly unsupported.

`raster::FIXED_RASTER_STATE` enumerates required explicit baseline state writes;
color masks, global blending, ZF32 depth and face culling additionally accept the validated
states described below. The explicit disabled-depth baseline uses:
unconditional rendering, rasterization enabled, one color attachment, single sample,
RGBA baseline writes, fill mode, no depth/stencil/cull/alpha test/logic op/sample mask,
no polygon smoothing/offset/stipple, and the supported viewport controls. Missing
state or unsupported values fail. No undocumented power-on values are substituted.
No instancing, additional shader stages, arbitrary formats or guest semaphore
operations are implemented by this slice. Perspective IPA has the bounded
canonical subset described below.

## Framebuffer visibility and ownership

The optional `GuestMemoryWrite` boundary extends checked read access with whole-span
write validation and an atomic all-or-nothing write. `SliceMemoryMut` implements
that independent test contract. It is not a runtime memory implementation. The
adapter must keep backing identity, permissions and contents stable during the
exclusive call; no guest borrow outlives queue upload or spans GPU completion.

The existing bounded offscreen target gains COPY_DST. First use/rebinding/content
changes initialize it from guest bytes (deswizzled when tiled) through
`Queue::write_texture`, using packed byte pitch without the buffer-copy 256-byte
row restriction. The render pass uses Load,
so uncovered pixels are retained. Readback uses the existing 256-byte-aligned buffer
copy, submission completion wait and row packing. After completion one checked
whole-span guest write publishes the framebuffer and updates the shared generation
journal. Host resource handles remain owned by wgpu across submissions; no explicit
destroy or sleep is used. Allocation/upload validation errors poison the backend.

The synchronous compatibility wrapper waits and publishes after drawing; deferred
submission instead retains GPU-newer contents until explicit publication or a
required coherence barrier. Neither API implements a guest hardware semaphore.
Production GPU/CPU visibility and presentation require a runtime service contract.
The target cache keys canonical guest backing, size, dimensions, storage layout
and journal version, not GPU VA alone. An alias can reuse contents;
remapping, CPU writes, target changes or host-harness rendering invalidate them.
Same-size rebinding reuses the host texture but uploads the newly selected guest
contents. Caches remain bounded; revisiting evicted state may rebuild resources.

## Indexed triangle lists

The additional B197 path uses INDEX_BUFFER_A..F (0x17c8..0x17dc),
DRAW_INDEX_BUFFER (0x17e0), BEGIN (0x1618) and END (0x1614). Address and inclusive
limit are 40-bit; format E is one for U16 and two for U32. F is a first **element**,
not byte offset. BEGIN must explicitly select triangle list, first instance (4),
with END zero. GLOBAL_BASE_VERTEX_INDEX (0x1434) is signed; base instance (0x1438)
and primitive restart (0x1644) must explicitly be zero. Compact forced-width
methods and U8 indices remain unsupported.

Selected indices are checked little-endian, scanned for minimum/maximum, and
validated after signed base adjustment against the guest vertex limit before
encoding. The supported scan budget is at most 1,048,576 elements per draw; this
is an implementation limit. Indices are uploaded to persistent INDEX/COPY_DST
buffers; U16 uploads with odd element counts are padded to four bytes without
changing the draw count. The host draw uses the selected index span starting at
zero and the original signed base. Shader code and pipelines are independent of
mutable index contents. Cache identity includes canonical guest backing, selected
byte span, width and dirty generation; base changes revalidate vertex bounds.

## Deferred hardware draws and explicit visibility

`submit_pushbuffer_deferred` accepts the same channel, guest mappings, exclusive
memory adapter and write journal as the synchronous entry, and returns the number
of submitted draws. Draws use the resident color image with Load/Store. Each draw
is submitted **before** subsequent queue uploads, so mutable vertex/constant data
cannot retroactively change earlier draws. There is one queue submission per draw,
but no unconditional readback, buffer mapping or CPU completion wait per draw.
This is not yet one encoder/submission for an entire frame.

`synchronize_target(memory, changes)` orders one texture readback after pending
draws, waits for that submission, and atomically publishes packed RGBA bytes to
the canonical guest backing. It returns None without work if nothing is pending.
The existing synchronous entry invokes this boundary after each hardware draw,
preserving its externally visible framebuffer contract. Optional GPU timestamps
refer to the latest draw, not the sum of deferred work or end-to-end frame time.
Standalone harness execution and direct guest-resource synchronization are
rejected while output is pending. Only guarded channel source resolution may
synchronize resources before the explicit visibility boundary.

Residency is deliberately bounded to one color target. Rebinding to another
backing, format/extent replacement, or CPU writes to the pending target require a
conditional barrier before reuse. Aliased GPU VAs resolving to the same canonical
backing and dimensions retain the image. Changing mappings cannot redirect old
pending writeback: it retains the original checked guest backing. This assumes the
adapter's backing identity remains stable within its process epoch.

GPU write generation is counted separately (`gpu_target_writes`). The first GPU
write in a pending interval invalidates overlapping resource caches in the shared
write journal; further ordered writes do not consume a journal slot per draw.
A source-read guard rejects reads from pending output during channel resource
resolution. A hazard causes one completion/publication and a resource-resolution
retry, covering shader code, constants, descriptors, vertices, indices and sampled
texture aliases without silently reading stale RAM. Feedback currently uses a CPU
coherence barrier, not direct resident-target sampling.

CPU writes must be declared with exact canonical guest byte ranges. Before
readback, subsequent journal entries are intersected/coalesced and their current
CPU bytes saved. Those patches override the corresponding GPU output bytes at
atomic publication; unchanged pixels retain the GPU result. A patched target is
initialized from merged RAM before the next draw. Overlapping writes therefore
preserve CPU-after-GPU ordering without interpreting tiled dirty offsets as linear
texels. A changed epoch or missing protected attachment history fails before writeback;
the caller must synchronize before such a transition. Failure of an atomic guest write leaves
pending output and journal version retryable. Channel faults preserve earlier
submitted work for explicit synchronization. Dropping a renderer does not publish
pending output. These are GPU test-adapter contracts, not Horizon fences, syncpoints,
GPU-service wiring or cross-thread runtime memory transactions.

## Global blending and channel writes

B197 target-zero blend enable is 0x1360. Global blend selection remains explicit
(`SET_BLEND_STATE_PER_TARGET` at 0x12e4 must be zero). When enabled,
SET_BLEND_SEPARATE_FOR_ALPHA (0x133c) must be specified; color operation/source/
destination are 0x1340/0x1344/0x1348, and alpha operation/source/destination are
0x134c/0x1350/0x1358. Separate alpha disabled reuses the color equation.

The implemented operations are ADD, SUBTRACT, REVERSE_SUBTRACT, MIN and MAX
(equation interpretation and aliases are described below); factors are ZERO
(0x4000/1), ONE (0x4001/2), SRC_ALPHA (0x4302/5), ONE_MINUS_SRC_ALPHA
(0x4303/6). This supports conventional source-over, premultiplied-over and other
combinations of those four factors, on the existing RGBA8 UNORM target.
Other documented factors, per-target equations, constants and dual
sources are rejected explicitly. Enabling incomplete blend state cannot draw.

SET_CT_WRITE(0), 0x1a00, independently enables R/G/B/A at bits 0/4/8/12.
All sixteen channel combinations are accepted; other bits are rejected. Disabled
channels retain the resident target contents. Normalized blend state and channel
mask participate in pipeline identity. Equivalent OGL/D3D encodings share that
identity. Raster-only changes recreate a compatible pipeline while reusing the
existing translated vertex shader module, fragment module and vertex buffer;
mutable constant data remains outside code/pipeline identity.

## GPFIFO command acquisition

`Renderer::submit_gpfifo_deferred(channel, entry_bytes, map, memory, changes)`
accepts a stable borrowed array of eight-byte, little-endian B06F GPFIFO entries.
This is the GPU-owned execution boundary, not an nvdrv ioctl decoder or a hardware
ring PUT/GET implementation. The runtime must validate/copy its ioctl metadata
before calling; this API neither dereferences an ioctl userspace pointer nor
interprets ioctl flags/fences.

| Entry field | Bits | Accepted subset |
| --- | --- | --- |
| Entry0 FETCH | 0 | UNCONDITIONAL = 0 |
| Entry0 reserved | 1 | zero |
| Entry0 GET | 31:2 | aligned address low bits |
| Entry1 GET_HI | 7:0 | address bits 39:32 |
| Entry1 PRIV | 8 | USER = 0 |
| Entry1 LEVEL | 9 | MAIN = 0 or SUBROUTINE = 1, straight-line packets |
| Entry1 LENGTH | 30:10 | nonzero count of 32-bit words |
| Entry1 SYNC | 31 | PROCEED = 0 or WAIT = 1, deferred payload acquisition |

Address plus length must fit the 40-bit address space, allowing an exclusive end
exactly at 2^40. The wire field permits 2,097,151 words. The execution policy caps
one call at 2,048 entries and 1,048,576 aggregate words (4 MiB), independently of
hardware limits. Empty arrays are successful no-ops on healthy channels.
Zero-length entries have control/opcode semantics: even the documented NOP opcode
is currently rejected, rather than treating an arbitrary control operand as an
address or borrowing zero bytes. Conditional fetch, kernel privilege and reserved
bit one remain rejected. LEVEL is retained as descriptor metadata, distinct from
a pushbuffer CALL/RETURN; unsupported packet control flow still faults. USERD
GET/TOP_LEVEL_GET, resume registers and preemptive scheduling are not emulated.
WAIT disables early payload acquisition, including prefetch of following entries.
It is not a CPU sleep or a complete host fence.

Metadata length, flags, address ranges and aggregate work are preflighted before
any FIFO word or engine method is consumed. Entries are decoded a second time during execution to avoid
allocating an owned entry array; the decode counter includes both passes. A
preflight failure latches the channel with zero executed words/draws. Command
mapping/access and engine errors occur during ordered execution and may follow
successful draws. The returned `SubmissionFault` includes its phase and progress:
completed entries, consumed words (including a rejected method word), and draws
accepted by the queue. This is not an atomic transaction or a completion fence.
Whole-array retry on a faulted channel is rejected without replay; prior pending
output remains available through `synchronize_target`. Failed explicit writeback
can be retried without rerunning commands.

Each full entry range must resolve inside one AddressMap mapping. Adjacent
mappings are rejected even if they name adjacent guest bytes; a runtime gather
extension has not been approved. GuestMemory must supply the entire requested
contiguous span, with permissions and backing stable for the borrow. Short reads
fail explicitly. GPU addresses never become host pointers. The current mapper's
canonical GuestAddress is the backing identity; replacing an AddressMap between
calls is observed without a cached command-pointer assumption.

The bridge borrows remaining command bytes and decodes scalar words explicitly
with `u32::from_le_bytes`. It uses the same `Channel::push_word` FIFO/engine
operation as direct pushbuffer submission, stopping at each draw. The borrow ends
before mutable resource resolution, queue submission or a conditional coherence
barrier. Remaining bytes are borrowed afresh afterward; no command Vec or guest
borrow survives GPU submission. Byte spans can overlap across these borrows;
`gpfifo_borrowed_bytes` counts successful span lengths and `gpfifo_command_bytes`
counts actual consumed words. Neither counter claims a bulk CPU copy. A GPU-newer
target overlapping command bytes triggers the existing explicit completion and
writeback before acquisition, preventing execution of stale RAM instructions.
This is a conservative host coherency contract, not a model of PBDMA command
prefetch timing or a hardware guarantee for self-modifying PROCEED buffers.
The executor acquires every payload only when its entry executes, satisfying
WAIT's no-prefetch policy without an additional per-entry GPU wait. PROCEED also
uses this conservative acquisition policy. Full PBDMA scheduling remains outside
the supported subset.

FIFO packet and B197 object/register state survive entry boundaries and calls.
No implicit `Channel::finish` occurs: the caller invokes it only at logical stream
termination, where truncation latches a fault. Entry order and draw queue order are
preserved. Reads after a draw can observe its required visibility barrier; unrelated
CPU writes concurrent with this exclusive host call are not modeled. CPU changes
between calls must be reported through the existing exact backing dirty journal.
No unconditional framebuffer publication is introduced.

The bridge does not implement production GPU page tables, nvdrv channel handles,
Horizon service dispatch, syncpoints/semaphores, nvhost fences, arbitrary engine
classes, general NVN/shader compatibility or VI presentation. Metadata acceptance,
command consumption, host queue acceptance, GPU completion and guest visibility
remain distinct states.

## GPU-owned driver contexts

`driver_backend::GpuDriver` owns a fresh Renderer, a canonical dirty journal,
registered runtime-owned backing, address spaces and Maxwell channel contexts.
The constructor deliberately cannot import a renderer with existing guest caches:
caller-chosen journal epochs could otherwise collide with cached resource stamps.
These opaque, process-unique IDs are not Horizon fds, nvmap handles or syncpoints.
The runtime retains ownership of guest bytes and must keep backing stable until
explicit release. Overlapping registrations are rejected; GPU aliases must map
one existing BackingId. All guest byte access remains checked and scoped.

The typed mapping API supports fixed, nonzero, 4 KiB-aligned reservations and
full-reservation or independently owned subinterval mappings of aligned backing
subregions. It uses AddressMap,
including its single-contiguous-mapping borrow restriction. Explicit full unmap
invalidates the old MappingId; remapping produces a fresh ID and advances the
address-space epoch. Partial unmap, sparse memory, automatic VA allocation, other
page sizes, GMMU page tables and memory-kind conversion are unsupported. This is
a bounded adapter policy, not a claim about the full Switch allocator. Limits are
16 spaces/channels and 128 backings/reservations per registry/space respectively.

Mapping/backing mutation is rejected while a target is GPU-newer. Pending
publication therefore cannot be redirected to recycled RAM. Backing release also
requires all aliases unmapped; space destruction requires all reservations freed
and channels closed. Canonical-address reuse marks the whole newly registered
backing dirty, invalidating existing guest resource stamps. Changes between
submissions must use `write_backing` (or a future equivalent runtime notification
contract); unreported external writes and recycled canonical storage are invalid.
While output is pending, each color/depth attachment retains at most 32 overlapping
CPU write notifications before publication. Disjoint canonical writes do not use
that budget, even when they rotate the global cache journal.

`submit(channel, code, payload, extra, memory)` decodes the libnx submission wire
layout. Legacy SubmitGpfifo is IOWR('H', 0x08, 24+8*n), with up to 512 inline
entries. KickoffPb is ioctl2 IOWR('H', 0x1b, 24), code `0xc018481b`, with a
separate entry buffer and up to 2,048 entries. Header offsets are: ignored u64
pointer at 0, u32 entry count at 8, flags at 12, input fence ID/value at 16/20.
Only HW_FORMAT=4 with zero input fence is accepted. Fence wait/get/increment,
WFI suppression and other flags fail explicitly. No output syncpoint fence or
nvdrv service result is synthesized. Request bytes use explicit little-endian
reads, exact buffer sizes and the existing aggregate GPFIFO work budget.

Metadata and channel-identity failures reject before admission and do not fault
an otherwise healthy channel. Accepted requests return a SubmissionReport; its
optional execution fault must be inspected even when admission returned Ok.
Progress includes consumed words, completed entries and queue-accepted draws.
Execution faults latch only that channel, preserve earlier submitted work and
return a completion token for the accepted prefix. Whole-request replay is
rejected. FIFO continuation and B197 registers persist per channel across calls.
Channels can alternate when no target is pending; one pending target has one
channel owner. Cross-channel submission/publication is blocked until that owner
publishes. This is serialized context switching, not concurrent hardware channels.

A token has channel identity and sequence, with at most 64 outstanding records.
`query` polls without blocking and observes a wgpu completion callback;
`wait` waits on the actual SubmissionIndex for its queue prefix. Neither operation
publishes RAM or adds a readback. A fast device may already be complete on the first
query. NOP-only prefixes without host work are already complete. Callback flags
are not advanced by draw counts or software sequence numbers. `publish` separately
performs checked readback/CPU-patch merging/atomic guest writeback; failure retains
pending output for retry without replay. Completion is distinct from visibility.
Retirement requires both completion and publication. `close` drains and publishes
before invalidating channel tokens; it is not cancellation. A failed close remains
retryable. Callers must close explicitly before dropping the controller if guest
visibility is required; Drop cannot publish without a runtime memory adapter.

The adapter does not dispatch Horizon IPC, implement nvhost handle allocation,
AllocGpfifoEx2/AllocObjCtx, hardware syncpoint increments or guest kernel events.
Those operations cannot be replaced with these service-independent tokens.

### Disabled global color key

B197 `SET_GLOBAL_COLOR_KEY` at `0x1354` defines ENABLE bit 0. The accepted subset
supports the FALSE write (zero) and rejects TRUE/reserved bits. It is an enable
field, not a blend coefficient or arbitrary ignored register. A seven-word INC
packet from `0x1340` can therefore include an explicit disable between alpha
source/destination coefficients. Since enablement is unsupported, no additional
pipeline variant is required. Direct raster register application also rejects
unaligned offsets before range indexing, even though normal FIFO methods are
already aligned.

## Block-linear color storage

Target zero accepts RGBA8_UNORM (`A8B8G8R8`, format `0xd5`), one 2D layer,
third dimension one, and layer zero. `ColorTarget` separates logical dimensions
from `ColorLayout::Pitch { bytes_per_row }` and
`ColorLayout::BlockLinear { block_height_log2 }`. WIDTH (`0x0808`) is a byte
stride for pitch storage and a pixel width for block-linear storage. Pitch storage
retains the existing full-stride host width, with surface clipping limiting visible
coverage. HEIGHT (`0x080c`) counts rows. MEMORY (`0x0814`) accepts either pitch
bit 12 alone, or block-linear with log2 block height in bits 7:4, values 0..5
(1, 2, 4, 8, 16 or 32 GOBs). Block width/depth must be one GOB, with array
third-dimension control. Other bits and layouts fail explicitly. ARRAY_PITCH
(`0x081c`) is in DWORDs; this single-layer subset accepts zero or the exact tiled
footprint divided by four. Pitch storage requires zero array pitch. Arbitrary
layer strides, arrays, 3D, multisampling and other color formats are unsupported.
The existing 256-byte address alignment remains an implementation restriction.

For width W, height H and block height B GOBs, the tiled span is
`ceil(4*W/64) * ceil(H/(8*B)) * 512*B`. Mapping/read/write validation and pending
source hazards cover this complete span, including edge GOB padding. The address
plus footprint must fit the 40-bit GPU address domain. The bounded publication
transaction limits tiled storage to `floor(64 MiB / 5)` bytes, conservatively
bounding simultaneously owned conversion, logical output and CPU patch vectors.
This bound excludes resident wgpu resources and driver-internal allocations.
`tegra_swizzle` retains its separate 64 MiB per-conversion bound.

Initialization deswizzles logical texels and uploads packed RGBA rows into the
existing resident attachment. Deferred draws continue to use load/store and do
not convert, read back or wait between unchanged draws. Canonical guest span,
logical dimensions, layout, block height and dirty generation determine residency;
numerically different GPU addresses mapping the same canonical surface can reuse
it. A layout change invalidates interpretation even when dimensions and allocation
size match. Compatible host attachments and shader/pipeline resources remain
reusable across guest storage layouts. `GpuDriver` submission/completion/publication
signatures are unchanged; NVDRV does not perform surface conversion.

Publication validates the whole destination and journal history, then decodes the
current full padded allocation into a bijective working image using the dependency.
After actual GPU completion/readback, it overlays only logical texel rows, swizzles
the entire padded image, and reapplies CPU-after-GPU dirty intervals in exact guest
byte coordinates. Padding and unrelated bytes survive: no partial-edge swizzle
result replaces the allocation with zero padding. CPU intervals are never treated
as row-major RGBA offsets. The public returned Frame remains logical RGBA, including
CPU modifications; a patched target is conservatively reinitialized on the next
draw. Atomic guest write and journal advancement occur last. Failure retains
pending GPU output and dirty history for publication retry, without replaying draws.

Counters distinguish full guest target reads, host uploads, logical/padded decode
bytes (`target_deswizzle_bytes`), full encoded bytes (`target_swizzle_bytes`),
logical overlay copies (`target_pack_copy_bytes`), and raw CPU intervals acquired
(`target_patch_bytes`). Retried attempts count actual repeated work. Conversion
allocates vectors and publication copies the complete allocation; these are not
zero-copy operations. Unchanged draws do not incur those conversion allocations.

## B197 ZF32 depth execution

A bounded single-layer ZF32 attachment is selected entirely by B197 ZT state.
Block heights 1–32 GOBs use the same checked four-byte Tegra geometry as color
storage, with complete padded mapping coverage. Color/depth dimensions must
match; overlapping canonical attachment spans are rejected. Arrays, stencil,
formats outside the ZF32/Z16 subsets, depth bounds/bias and arbitrary depth
transforms remain unsupported. The implemented viewport uses canonical 0..W
or -W..W clipping and the corresponding Z scale/offset. All eight comparisons and depth-write enable are supported;
active depth requires explicit target selection, test enable, comparison and
complete descriptor state. Initial samples must be finite normalized f32 in
[0,1]; subnormals and negative zero are explicitly outside this transfer subset.

Depth remains GPU-resident across draws. A cold attachment requires a GPU
initialization pass; an unchanged draw neither uploads depth nor waits for it.
Explicit publication exports actual GPU depth, packs logical rows, swizzles into
the original padded storage and overlays exact raw CPU-after-GPU byte ranges.
Padding is preserved. Depth and color are atomically published **per attachment**,
in depth-first order. This is not a transaction spanning separate allocations.
A failure retains each unpublished attachment and retries without command replay;
a successful depth publication is not repeated if color publication subsequently
fails. Guest-visible completion is reported only after the complete publication
boundary succeeds. Source aliases and command fetches guard both pending padded
spans. GpuDriver's mapping/teardown restrictions apply while either output is
pending. Pipeline identity includes depth comparison/write state, while shader
module identity remains independent of depth data.

## Bounded floating arithmetic

The straight-line vertex/fragment decoder accepts unpredicated GPR/GPR FADD
and FMUL with default modifier bits and six-cycle stalls. Registers retain raw
u32 bits; operations bitcast their operands to f32, perform host arithmetic and
bitcast the result back. RZ reads as positive zero. Undefined or pending source
registers, pending destination writes, alternate rounding, FTZ/FMZ, scaling,
negation/absolute value, saturation and condition-code writes are rejected.
The existing pending-destination guard remains in force.

Multiplication provenance follows MOV and is cleared by actual overwrites,
including every lane of ALD/TEX groups. FADD consuming an FMUL-derived register
is explicitly unsupported: this excludes contraction-prone dataflow until an
unfused rounding contract is available. This restriction alone does not establish
Maxwell numerical equivalence. WGSL host f32 does not promise Maxwell-exact NaN,
denormal, signed-zero, overflow/underflow or rounding behavior. Current executable
numeric evidence covers exactly representable finite fixtures, including
independent addition/multiplication, chained addition and RZ. General precise
floating shader compatibility remains unproven.

## Perspective interpolation subset

A PS header with PositionW at word five bit 31 and generic-zero xy interpolation
modes Perspective=2 (word six `0x0a`) selects a perspective interface. Screen-linear
xy (`0x0f`) retains its existing behavior. Both stages declare matching WGSL
interpolation qualifiers; the selected fragment interface participates in vertex
module reuse as well as paired pipeline identity.

The decoder accepts IPA.PASS `a[0x7c]` into a GPR, MUFU.RCP of that ready PositionW
value, and perspective IPA `a[0x80/0x84]` with the ready reciprocal GPR multiplier.
These variable-latency instructions serialize write barrier zero with actual
wait-one controls. MOV preserves position/reciprocal provenance; overwrites,
including all ALD/TEX destination lanes, clear it. Arbitrary reciprocal sources,
unproven multipliers, mixed interpolation headers, flat/centroid/offset/indexed
forms, predicates and modifiers are unsupported.

For fragment position W `q=sum(lambda_i/W_i)`, the guest IPA plane is
`sum(lambda_i*a_i/W_i)` times its explicit GPR multiplier. WGSL perspective UV
already divides that plane by q, so lowering multiplies WGSL UV by position.w
and the decoded multiplier. It does not discard the guest multiplier. The narrow
reciprocal lowers to host f32 `1/q`; this is not a bit-exact Maxwell approximate
MUFU implementation. Exceptional values, subnormals and exact reciprocal rounding
remain unverified. The executable evidence uses finite positive unequal clip W
and texture samples separated from quantization boundaries. This subset establishes
perspective-dependent sampling, not general MUFU compatibility.

## Integer arithmetic and bit logic

The straight-line VS/PS decoder additionally accepts GPR/GPR IADD wrapping modulo
2^32 and LOP AND/OR/XOR, with unconditional execution, default modifiers, no carry
input/output or CC writes, and LOP predicate output discarded to PT. Operands
remain raw u32 bits in WGSL. Both sources must be initialized and ready; RZ is
zero and cannot be a destination. Six-cycle stalls and ALU write-barrier seven
are required. Immediate/constant operand variants, saturation, IADD operand modifiers,
additional predicate results and extended carry remain unsupported. The bounded
LOP complement/PASS_B forms below extend the original bit-logic subset.

Integer results clear constant-handle, PositionW and reciprocal provenance. Even
identity arithmetic does not authorize an altered TEX handle or perspective
multiplier. FMUL rounding-hazard taint is conservatively retained from either
integer source, including in-place destinations: an optimizer could erase integer
identities and expose an otherwise prohibited multiply-to-add contraction. This
policy preserves the existing floating subset's rejection contract; it is not a
new general floating-point accuracy guarantee. Integer bit operations themselves
use exact wrapping/bitwise semantics.

## B197 attachment clears

`CLEAR_SURFACE` at byte method `0x19d0` executes full target-zero/layer-zero RGBA
(`0x3c`), ZF32 depth (`0x01`) or both (`0x3d`). Color values come from
`0x0d80..0x0d8c`; depth from `0x0d90`. The supported subset requires explicit
unconditional rendering `0x0138=1`, single sample `0x15d0=0`, clear controls
`0x10f8` in `{0, 1, 0x100, 0x101}` and full surface clip `0x0ff4/0x0ff8`.
When scissor zero is requested, it must be explicitly enabled and match the
entire attachment exactly. Selected color/depth descriptors
and corresponding target selection must be complete. Color supports the same
pitch/block-linear RGBA8 layouts; depth uses block-linear ZF32 or Z16. Normalized finite
clear values are supported, with the existing ZF32 zero/normal restrictions.
Stencil, partial component masks, additional targets/layers, partial clips and
partial scissors, viewport/rectangle controls and stencil operations remain unsupported.
Respect-stencil-mask has no effect on the supported color/depth-only operations.

Clears resolve raw attachment geometry independently of draw state. They require
no guest shader, vertex/index buffer, viewport transform, blend coefficients or
depth comparison/write enable. Draw color write masks and blending do not affect
hardware clears; clear control zero excludes draw scissors/viewport clipping.
Only a full-attachment scissor is also admitted, never a partial region silently
expanded to a full clear. Surface clipping still applies, hence the full-clip validation.
Execution uses actual attachment `LoadOp::Clear`/`Store` operations, not RAM fills
or a synthetic draw. Full clears require no old-pixel upload. ZF32 publication
still uses the existing portable GPU depth export path.

Both FIFO and GPFIFO dispatch clears in order. A clear ends the scoped command
borrow just as a draw does, so an unread suffix overlapping GPU-newer storage is
published and reacquired. GPFIFO `draws_submitted` remains a draw count;
`Counters::clear_commands` separately counts accepted clears, while queue submit
tracking establishes real completion even for a submission containing no draw.
Completion waits do not implicitly publish RAM. A depth-only publication returns
no color Frame, but counts as a successful visibility barrier.

A full clear supersedes prior logical attachment contents and CPU writes before
it. Its new stamp excludes those earlier writes from the later CPU patch overlay;
CPU writes after clear still win by exact canonical byte range. Block-linear
padding remains sourced from current guest storage. Unchanged pending clears
coalesce journal invalidation; CPU writes advance the stamp and cause fresh
invalidation. Untouched pending attachments retain their own stamps and contents.
Incompatible selected-target rebinding drains earlier pending output first;
attachment overlaps and changed process/journal epochs fail before execution.
Atomic publication failure retains pending output for retry without re-clearing.


## B197 face culling and source observations

Byte methods `OGL_SET_CULL` (`0x1918`, strict 0/1), `OGL_SET_FRONT_FACE`
(`0x191c`, CW=`0x900`, CCW=`0x901`) and `OGL_SET_CULL_FACE`
(`0x1920`, FRONT=`0x404`, BACK=`0x405`) configure triangle-list culling.
Enabled culling requires both orientation and face to have been explicitly set.
Disabled culling normalizes those irrelevant fields out of pipeline identity.
FRONT_AND_BACK, the separate D3D cull mode, alternate window origins and other
raster modes remain unsupported. Invalid method values preserve existing register
state; incomplete enabled state cannot execute a previously selected pipeline.
Attachment clears remain independent of draw culling.

For the supported upper-left window origin, guest CW/CCW maps directly to host
CW/CCW. The translated vertex Y adjustment already implements signed guest
viewport scale; changing scale Y changes actual winding without an additional
front-face inversion. Culling participates in paired pipeline identity, while a
compatible vertex module is reused across front/back/orientation changes.

Vertex-program, vertex-buffer and index-buffer caches retain the global dirty
journal revision last observed. On a hit, checked canonical range resolution and
retained journal history must prove no overlapping writes since that revision.
The observation then advances without rereading or uploading unchanged bytes.
This avoids repeated invalidation when unrelated writes evict older journal
records. A new epoch, future cursor, missing history, changed canonical range,
changed size/format or overlapping write requires a fresh read. Index min/max
and signed base/vertex bounds are still validated for each draw. Other resource
caches retain their existing conservative range-stamp policy. Observations are
not content identities or GPU fences and do not replace pending-output stamps:
publication still requires exact retained CPU patch history and atomic writes.


## Unsigned integer shifts

Both stages accept GPR SHL and unsigned SHR with default modifiers and optional
`.W`. Without `.W`, the complete unsigned count >=32 produces zero. With `.W`,
counts reduce modulo 32. WGSL lowering masks every evaluated host shift count;
clamp forms separately select zero using the original count. A bare host shift
would incorrectly wrap the clamp forms. In-place destinations read the prior
operand bits, and RZ is zero. Initialization, readiness, six-cycle scheduling,
no variable-latency write barrier and existing integer provenance/rounding-hazard
rules remain enforced. Signed SHR, carry/CC, `.X`, `.BREV`, execution predicates,
immediate/constant operand forms and other modifiers remain unsupported.


## Unsigned integer minimum and maximum

Both stages accept GPR IMNMX with unsigned 32-bit comparison and static selection
PT (minimum) or !PT (maximum). The selection predicate is distinct from the outer
execution predicate, which remains unconditional PT. Exact masks exclude dynamic
selection P0..P6, extended modes, CC and alternate operand forms.
The existing binary-integer IR lowers to WGSL min/max on raw u32, retaining
initialization/readiness, six-stall scheduling and provenance/rounding-hazard rules.
This does not introduce general predicate registers or conditional execution.


## Predicate comparisons and conditional data selection

Both stages accept unsigned GPR ISETP LT/EQ/LE/GT/NE/GE with AND PT, primary
output P0 and complementary output discarded to PT. SEL accepts two ready GPRs
and initialized P0 or !P0, writing the left value when its condition is true and
the right otherwise. Both candidates require initialization even if one is not
selected at runtime. Outer execution remains unconditional PT. Other predicates,
extended comparisons, Boolean combinations, alternate operands, general
predicated instructions and branches remain unsupported.

Predicate initialization is tracked separately from GPR dataflow. ISETP bypasses
GPR destination processing: its low byte encodes predicate outputs and must not
initialize, clear readiness or overwrite provenance for R7. Predicates do not
increase guest GPR allocation requirements. The bounded schedule requires stall
13 for ISETP and six for GPR operations, with no variable-latency predicate barrier;
prior LDC/ALD waits remain necessary. These are conservative source-model-based
accepted combinations, not a claim about Maxwell's exact minimum latency or
compatibility with every valid scheduling sequence.

WGSL introduces a local P0 bool only in programs using these operations. Source
values remain runtime u32 bits, and conditional selection does not specialize on
mutable guest data. SEL clears special constant-handle/PositionW/reciprocal
provenance and conservatively retains FMUL rounding-hazard taint from either
candidate and the predicate's comparison inputs. Existing programs retain their
previous WGSL shape and scheduling acceptance.


## Paired constant-buffer loads

Fragment programs accept LDC.B64 with RZ addressing, default mode, unconditional
PT execution, an even destination through R252 and nonnegative eight-byte-aligned
immediate byte offsets. Destination lanes receive successive little-endian B32
words. The existing scalar IR preserves each lane's own constant reference and
handle provenance; deduplicated inputs need not have consecutive packed indices.
One write barrier zero covers both lanes, and the next consumer/load must wait
for the complete pair. Repeated writes erase both lanes' previous arithmetic and
interpolation metadata. The 64-word constant budget counts distinct words.

The program retains paired access metadata independently of its scalar lowering.
Resource preflight validates the full bound CB range, effective GPU address
alignment, 40-bit end and a single contiguous eight-byte mapping before any scalar
uploads. Short backing reads still fail before execution; failure clears readiness
rather than drawing a prior resource binding. Even destinations, positive/aligned
offsets, aligned effective addresses and contiguous mappings are conservative
Wanderlust subset limits, not universal hardware invalidity rules. Vertex LDC,
negative/register-relative addressing, other modes and LDC.B128 remain unsupported.
No additional driver-facing API or GPU uniform layout is introduced.


## Signed integer minimum and maximum

Both stages additionally accept signed GPR IMNMX with static PT minimum or !PT
maximum. Raw operands are bitcast to i32 solely for comparison; the selected result
is bitcast back to u32. No numeric conversion, subtraction-based comparison or
floating min/max is substituted. Initialization, RZ, in-place writes, six-stall
scheduling, resource provenance erasure and arithmetic hazard taint follow the
existing integer path. Signed comparison changes only bit 48 of the accepted
unsigned encodings. Dynamic selection predicates, CC, extended modes, alternate
operand forms and general predicated execution remain unsupported.


## IADD subtraction and fragment source observations

Both shader stages accept the two GPR IADD forms with exactly one source negated.
Negated source A computes B−A, and negated source B computes A−B, modulo 2^32.
They normalize to one raw-u32 subtraction operation; negated-A swaps the source
order before lowering. Source/destination aliasing reads old operand values. RZ,
readiness, six-stall scheduling, provenance clearing and both-source arithmetic
hazard propagation remain unchanged. Both negation bits together are the distinct
IADD.PO encoding and remain rejected, alongside saturation, CC, extended carry,
other operands and predicated execution.

Selected fragment code and packed CB words use the existing bounded journal
observation proof already used by geometry. Complete canonical mappings, CB bounds
and paired-access preflight remain mandatory. A proved unchanged access advances
its observation without reading guest bytes; changed/evicted history or an epoch
change forces acquisition. CB staging observations commit only after the entire
resource synchronization succeeds. These cursors do not replace pending-output
publication stamps or erase CPU patch history. GPU-newer aliases still require
the guarded source acquisition/publication path before reuse.


## Protected pending attachment histories

The shared dirty journal retains 64 global entries for conservative source-cache
proofs. Independently, its two fixed pending slots retain up to 32 exact canonical
CPU-write intersections for color and depth. Each slot records the complete guest
storage range, epoch, GPU-write baseline and latest overlapping write revision.
Block-linear padding belongs to the protected storage footprint. Unrelated writes
can rotate the global journal without losing CPU-after-GPU patches or changing a
pending target's content stamp. Source observation cursors remain separate and
still require reacquisition when their global proof history is missing.

`GpuDriver::write_backing` calls the additive `TextureChanges::can_mark_write_at`
preflight before validating/writing RAM. An overlapping write at a full patch
budget fails before either RAM or journal changes; a disjoint write remains valid.
Recording a successful write uses fixed interval storage and the preallocated
journal. The caller must exclusively own memory and journal through the atomic
write and notification. This is a scoped GPU-adapter contract, not an asynchronous
reservation or a production runtime memory transaction guarantee.

The first accepted GPU write begins a protected interval. Ordinary repeated draws
retain its baseline and CPU history. A supported full clear after CPU changes
rebases only the cleared attachment, superseding older logical texel patches;
publication still obtains padding from current RAM. Unchanged repeated clears
coalesce invalidation. Complete color/depth range and revision-capacity preflight
precedes submission, with overlapping attachment storage rejected.

Publication requires the exact protected range, epoch and baseline. Conversion
and raw CPU-byte merging precede one checked atomic write per attachment. Only a
successful write retires that slot and records the global publication revision;
this GPU-originated publication bypasses the retiring slot's CPU-patch budget.
Depth publishes first. If color publication then fails, depth is already retired
and color remains retryable, including later CPU patches and disjoint writes.
Retry may repeat readback/conversion, but never replays the draw. Mapping mutation,
channel switching and direct source guards retain their existing restrictions.
No guest syncpoint, Horizon memory hookup or automatic per-draw wait is added.


## Signed predicate comparisons

Both stages additionally accept signed GPR ISETP LT/EQ/LE/GT/NE/GE with the same
P0/PT outputs, AND PT accumulator and unconditional execution restriction.
Instruction bit 48 selects signed interpretation independently for each operation;
mixed signed and unsigned comparisons in one shader retain their own semantics.
Signed operands are bitcast to i32 for comparison, preserving two's-complement
extrema without float conversion or overflow-prone subtraction. SEL still selects
raw u32 candidates under P0 or !P0. Comparison variants carry their interpretation
in `IntegerComparison`; the original `UnsignedComparison` name remains a type
alias for existing callers of the unsigned variants.

Predicate initialization, separate GPR metadata, conservative 13-stall scheduling,
LDC/ALD waits, provenance clearing and FMUL-hazard taint remain unchanged. The
extension admits only the signedness bit in the previously verified instruction
mask. Extended .X comparisons, other Boolean operations/accumulators/predicate
outputs, immediate/constant operands and predicated execution remain unsupported.
This adds conditional data selection, not branches or general control flow.


## Arithmetic signed right shifts

Both stages additionally accept GPR SHR and SHR.W with signed value interpretation.
The shifted value is raw two's-complement i32; the shift count remains the complete
unsigned u32 register value. Default counts of 32 or greater produce sign fill
(all ones for a negative value, zero otherwise). Wrapped .W counts use the low
five bits. WGSL bounds every evaluated count: arithmetic shift by min(count,31)
for default or count&31 for .W, with the result bitcast back to raw u32.

These semantics differ from unsigned SHR's out-of-range zero result. Signedness
bit 48 and wrapping bit 39 are admitted only through the two exact verified GPR
encodings. Readiness, RZ, in-place operands, six-stall scheduling, provenance
erasure and conservative FMUL taint from either operand retain the shared integer
path. .X, .BREV, CC, immediate/constant operands and execution predicates remain
unsupported. No wide shift, funnel shift or integer conversion is implied.

## Variable-latency integer multiplication

Both stages accept low-result GPR IMUL with unconditional PT execution, full
32-bit operands and all four verified U32/S32 source interpretations. Low product
bits are identical modulo 2^32, so these forms normalize to raw-u32 multiplication.
The two source signedness flags do not imply signed high-result support. HI, CC,
immediate/constant operands, execution predicates and unknown fields remain
unsupported. No XMAD halfword or integer multiply-add behavior is introduced.

IMUL is a variable-latency producer in the supported SM50 model. It requires
ready initialized operands, unused write barrier zero and no other outstanding
variable-latency result. Its destination remains pending until a wait on barrier
zero. Six stalls alone do not make the result ready. Independent ordinary register
writes can occur while it is pending; consumers, destination overwrites and barrier
reuse must still pass the existing dependency checks. An in-place product reads
old source values before assignment. The common metadata path clears resource,
PositionW and reciprocal provenance and retains any FMUL hazard from either source.
A genuine overwrite using untainted sources clears an older float hazard.

The pending-destination exemption is limited to a successfully validated IMUL
producer. It cannot overwrite an existing pending register because the producer
preflight rejects any outstanding result before marking its own destination.
These are conservative accepted barrier/scheduling combinations, not calibrated
Maxwell timing or general scoreboard emulation. No CPU/GPU wait is added per draw.


## Global RGB and alpha blend equations

B197 byte methods 0x1340 (color equation) and 0x134c (alpha equation) accept ADD,
SUBTRACT, REVERSE_SUBTRACT, MIN and MAX. Compact values 1/2/3/4/5 and GL aliases
8006/800a/800b/8007/8008 normalize to the same typed state. SUBTRACT computes
source times source factor minus destination times destination factor; REVERSE
SUBTRACT reverses those terms. RGBA8 attachment storage clamps the result.

Ordinary MIN/MAX compare unweighted source and destination components. Only the
resolved pipeline state canonicalizes their ignored factors to ONE/ONE; raw guest
factor registers remain available when a later command selects another equation.
With separate-alpha disabled at 0x133c, alpha inherits both the color equation and
its coefficients. Enabled separation requires independent, explicit alpha state.
Supported factors include ZERO, ONE, source/destination color and alpha, and
their complements as detailed below. Unknown factors remain rejected even when
the selected equation ignores them.

The public GPU BlendState carries color_operation and alpha_operation alongside
its factor pairs. Pipeline identity includes both operations, normalized factors
and write masks. Equation changes reuse compatible shader modules; equivalent
aliases and ignored-factor changes reuse the current pipeline. GpuDriver request,
completion and publication APIs are unchanged. Weighted MIN/MAX, logic operations,
independent MRT blending and other factors remain unsupported. The inactive
color-key register at 0x1354 and direct method-alignment validation are retained.


## Native FLOAT3 vertex position fetching

B197 SET_VERTEX_ATTRIBUTE_A(0), byte method 0x1160, now accepts FLOAT R32_G32_B32
(component-width value 2) alongside R32_G32_B32_A32 (value 1). Stream zero,
active source, no R/B swap and checked four-byte-aligned offsets remain required.
Attribute one remains FLOAT2 UV. PositionFormat in the public VertexArray preserves
the distinction: position spans 12 or 16 bytes, while the selected stream stride
and inclusive limit bound the complete acquisition. Tight XYZ/UV vertices occupy
20 bytes; padded or reordered fields also work when both fit the checked stride.

The host vertex format is Float32x3 or Float32x4. The existing translated vec4
input and guest ALD B128 instruction remain unchanged: native vertex fetch fills
missing FLOAT3 W with 1. No CPU vertex repacking or synthetic shader constant
replaces the fetched attribute. Format participates in both resident pipeline
comparisons; compatible shader modules and raw vertex buffers can remain resident
when only the attribute interpretation changes. The intermediate public
set_vertex_program API retains its existing FLOAT4 behavior. GpuDriver submission,
completion and publication APIs are unchanged.


## Explicit homogeneous depth clipping

Draws require explicit B197 SET_VIEWPORT_Z_CLIP at byte method `0x0d7c`:
range one clips at zero through positive W; range zero clips at negative W
through positive W. Missing state is an error. The selector remains independent
of viewport clip control (`0x193c`). Two exact unclamped XYZ-frustum profiles are
supported: `0x1801` and the source-verified `0x181d` initialization profile.
Other profiles, including depth-clamp enable, remain unsupported; unnamed
control bits are not generalized. Clear operations retain their own state
requirements and do not infer draw clipping state.

Viewport Z scale/offset must be one/zero for 0..W, or one-half/one-half for -W..W.
For negative-W clipping the translated Maxwell vertex output is remapped to
host Z as `z_host = z_guest/2 + w_guest/2`, preserving W and the original
homogeneous clipping planes. The depth convention participates in vertex
shader/pipeline specialization identity, separately from decoded Maxwell code.
Changing conventions recompiles the host specialization without rereading or
redecoding unchanged guest programs. Host fixed-function clipping rejects
out-of-frustum geometry even with ALWAYS depth comparison. Partial primitives,
unequal positive W and screen-linear varyings retain the same screen-space
interpolation plane.

All sixteen viewport scale/offset, identity-swizzle and clip/depth-bound banks
are retained independently. Supported shaders cannot export a viewport index;
only bank zero is rasterized. Inactive bank writes never alias zero or change
its effective pipeline state. Canonical zero/one depth bounds are supported;
arbitrary depth transforms, nonidentity swizzles, depth bias/clamping and
fragment-depth export remain unsupported. Submission, completion and
publication interfaces are unchanged.


## Component and destination-alpha blend factors

The four B197 RGB/alpha source/destination coefficient methods accept SRC_COLOR,
INV_SRC_COLOR, DST_ALPHA, INV_DST_ALPHA, DST_COLOR and INV_DST_COLOR in addition
to ZERO, ONE and source-alpha factors. Compact3/4/7/8/9/10 and aliases
4300/4301/4304/4305/4306/4307 normalize to the same typed factors. Color factors
multiply corresponding RGB components, rather than replicating a selected color
channel. In the alpha equation their corresponding component is alpha.

Destination factors read the attachment value established by earlier ordered
draws, including its quantization. Disabling alpha writes preserves destination
alpha but does not stop that alpha from contributing to subsequent RGB blending.
Shared-alpha mode inherits the RGB factors and evaluates them on alpha; explicit
separate-alpha coefficients remain independently stored. MIN/MAX continue to
ignore validated coefficients without erasing their raw guest values.

The existing wgpu factors implement these operations, including native DX12
alpha-slot conversion for color factors. No extra device feature, shader
translation, framebuffer publication or driver-facing API is introduced. Pipeline
identity includes the typed factors. Constants, saturation, dual-source factors
and compact BOTH_SRC_ALPHA forms remain unsupported.


## LOP operand complements and canonical PASS_B

Both shader stages accept GPR/GPR AND, OR and XOR with independent bitwise
complements on their two 32-bit sources. The supported canonical PASS_B has
unmodified source A=RZ and either ordinary B or complemented B (NOT). An unused
noncanonical A is explicitly unsupported; its hardware readiness behavior is not
inferred. Register-zero, initialization, pending-source/destination, fixed-six
stall and write-barrier-seven requirements remain enforced.

The complete instruction match varies only destination7:0, sourceA15:8,
sourceB27:20, invertA39, invertB40 and operation42:41. Execution is unconditional,
predicate output is discarded to PT and condition mode is default. CC, extended
operations, predicate results, execution predicates and immediate/constant-source
families remain rejected. Bitwise complement lowers to WGSL u32 complement; it
is neither floating numeric negation nor Boolean inversion.

Existing plain AND/OR/XOR IR variants remain stable. Complemented and PASS_B
forms use typed LogicalOperator data within the common Integer operation, sharing
register validation and both-stage lowering. In-place sources are read before
destination assignment. Identities/complements clear resource-handle, PositionW
and reciprocal proofs; they cannot bypass the restricted resource/interpolation
contracts. FMUL hazards follow both sources and only an actual untainted overwrite
clears an older hazard. No new host waits or guest-memory interface is introduced.


The full-width LOP32I subset accepts AND/OR/XOR with independently selected
source and immediate complements. The second operand is all 32 raw bits in
instruction51:20, including positions used for predicate outputs in GPR LOP.
It is never sign-extended from20 bits or interpreted as a register number.
The additive LogicImmediate operation retains source FMUL hazards and clears
resource/reciprocal/PositionW proofs, even for mathematical identities. Both
stages lower unsigned bitwise operations; mutable constant data stays outside
shader identity, while changes to encoded immediate bits invalidate code.
PT execution, initialized ready GPR/RZ source, ordinary destination, WB7 and
the existing conservative six-stall schedule are required. CC52, X57, operator3,
other predicates and operand families remain explicitly unsupported. This
restriction does not assert that operator3 is an invalid hardware encoding.


## Full-width integer immediate addition

IADD32I executes ordinary low-32-bit addition with an optional independently
encoded source-A negation. The second operand is the complete raw 32-bit field
at instruction bits 51:20. Both stages lower unsigned arithmetic, including
`0u - A` for negation, before overwriting the destination. This implements
modulo-2^32 behavior; it does not numerically convert operands to floating point.

The additive AddImmediate IR operation counts only actual source/destination
registers, retains source FMUL rounding hazards and clears resource/PositionW/
reciprocal proofs even for addition by zero. Ready initialized GPR/RZ inputs,
ordinary destination, PT execution, WB7 and the existing conservative six-stall
schedule are required. CC output, X/carry input, saturation, PO and other
predicates remain unsupported. Changes to encoded immediate bits invalidate
shader code; mutable constant-buffer source values preserve compiled artifacts.


The signed-20-bit immediate IADD form reuses AddImmediate after explicit
sign extension. Payload bits 38:20 plus sign bit 56 represent -524288 through
524287; source-A negation is bit 49. These positions deliberately differ from
IADD32I, where bit 56 negates the source. The short form retains identical
raw-u32 wrapping arithmetic, readiness, proof erasure and rounding-hazard rules.
Its immediate-negation/PO, CC47, X43, SAT50 and other predicate/modifier forms
remain unsupported. An explicitly encoded immediate zero is accepted, although
the pinned assembler prefers an equivalent GPR-RZ instruction for textual zero.


## Native Z16 attachments

B197 `SET_ZT_FORMAT=0x13` selects single-layer block-linear Z16_UNORM. Existing
ZT address, geometry, block-height, selection, comparison and write methods apply;
unknown formats, stencil, multisampling and pitch-linear depth remain unsupported.
The complete two-byte-per-sample GOB footprint is mapped and guarded. ZF32 retains
its four-byte storage and normalized finite-value restrictions.

`DepthFormat` is explicit in `DepthTarget` and `DepthState`. Guest storage stamps,
native attachment residency and graphics pipeline identity include that format.
A pending format change publishes prior output before replacing native storage.
Canonical aliases retain compatible residency; GPU mapping mutation remains
blocked while output is pending. Existing GpuDriver submission/completion APIs
are unchanged.

Host testing and writes use `Depth16Unorm`, not a float shadow. Portable transfers
expand LEu16 codes into R32Uint staging, initialize normalized native depth, and
export the sampled attachment through another R32Uint image. Guest initialization
and publication use two bytes/sample; staging/upload/readback use four plus row
padding. Publication reuses the Tegra conversion, preserves padding and overlays
CPU-after-GPU patches in exact canonical tiled byte intervals. Atomic-write failure
retains retryable pending depth without draw replay. Attachments publish separately;
a whole color-plus-depth transaction is not promised.

Thirty-two warm unchanged draws need no additional depth conversion/upload,
attachment/pipeline/shader allocation, readback or blocking wait. They still
submit32 host queues. Stored code round-trip and endpoint equality are verified;
Maxwell equivalence for arbitrary depth-conversion ties is not established.


## Short-float shader operands

The bounded vertex/fragment translator additionally accepts unconditional default
FADD3858 and FMUL3868 with one initialized GPR/RZ source and one short f32 immediate.
`FloatImmediate` retains raw bits, operand identity and Add/Multiply semantics;
it uses shared stage lowering and register accounting. Guest constant-buffer data
remains runtime data. Code changes invalidate translation, while mutable constants
do not require shader recompilation.

Side-effect/modifier variants remain explicit errors. Fixed scheduling, pending
register checks and resource proof rules apply unchanged. Immediate FMUL results
cannot flow into either form of FADD until portable unfused-rounding control exists;
MOV, logical and integer identities do not remove that restriction. This is a
small additional operand family, not general floating-point or shader compatibility.


## Full32 floating operands

FADD32I and FMUL32I additionally use the existing `FloatImmediate` IR and shared
stage lowering. Every raw immediate bit is retained, including low mantissa bits
absent from F20. Strict unconditional/default forms only are accepted; register,
scheduling, provenance and multiply-to-add restrictions remain unchanged across
GPR, short and full-width encodings. No extra compiler or resource-binding path
is introduced. Exceptional floating-point equivalence and FFMA remain unproven.

A guest shader can compare the raw arithmetic result with an independently
provided constant using ISETP/SEL. This permits exact low-bit validation without
introducing a forbidden multiply-to-add chain or reducing the observation to
8-bit color quantization alone.

## Conditional register moves

The bounded translator accepts full-lane GPR MOV executed under P0 or !P0 in
both vertex and fragment stages. ISETP supplies the runtime predicate; a false
predicate preserves the previous destination bits. WGSL performs a conditional
assignment, without specializing on constant-buffer contents or vertex data.
Only this MOV form gains conditional execution; other predicates and general
predication/control flow remain unsupported.

The supported static subset requires initialized P0 and initialized, ready source
and prior destination registers, even when the runtime predicate would be false.
This conservative acceptance rule is stricter than hardware's suppressed operand
read on false execution. Fixed scheduling and ordinary barrier waits remain
mandatory. Multiplication hazards survive through either candidate value or the
predicate producer. Conditional moves erase texture-handle and perspective
proofs, including identity moves, until stronger path-sensitive proofs exist.

## Floating predicates

Vertex and fragment programs accept all sixteen GPR FSETP conditions: F,
LT/EQ/LE/GT/NE/GE, NUM/NAN, LTU/EQU/LEU/GTU/NEU/GEU and T. The result goes
into P0, with the complementary result discarded to PT and AND PT as the
boolean input. Execution is unconditional. FTZ, operand modifiers, other
predicate outputs/accumulators and immediate/constant operand forms remain
unsupported. The conservative predicate latency is thirteen cycles;
ordinary readiness and wait checks apply. Predicate writes do not modify GPR R7
despite the instruction's low-byte predicate-pair encoding.

The translation classifies IEEE binary32 input bits using integer operations,
normalizes both signed zeros, and compares monotonic unsigned keys. Any NaN
operand makes every ordered relation false, including NE. Subnormal bits retain
their numerical order without relying on host floating-point flush or NaN modes.
Unordered relations are true if either operand is NaN, otherwise they follow
their ordered relation. NUM accepts both numeric operands, including infinities
and subnormals; NAN detects either NaN, including signaling encodings of both
signs. F/T always return false/true. The static accepted subset still requires
ready initialized operands and retains multiplication provenance for F/T, even
though their runtime result does not depend on operand values.
This exact comparison path does not establish exceptional-value equivalence for
the separate floating arithmetic instructions. Multiplication dependencies in
either comparison operand propagate through the resulting predicate into SEL
and conditional MOV; existing unfused-rounding restrictions remain enforced.

## External CPU writes at a serialized runtime boundary

`GpuDriver::notify_cpu_write(backing, offset, length)` invalidates the exact
canonical range after a runtime-owned CPU write, without reading, copying or
writing those bytes again. Backing identity, bounds, integer overflow, journal
capacity and instrumentation overflow are checked before the journal changes.
Rejected notifications change neither journal nor notification counters.

The caller must exclusively own driver and backing throughout this sequence:
publish any GPU-newer output **before** externally changing RAM, perform the CPU
write, then notify its range before submitting more GPU commands. Checking only
after an external write is too late. The method rejects every pending attachment
and cannot undo an external write if notification fails. This is a deliberately
serialized adapter contract, not asynchronous CPU/GPU sharing or automatic AOT
dirty tracking. `write_backing` remains the checked atomic path for CPU patches
while GPU output is pending. Host completion alone does not establish guest
publication or grant external-write permission.

Notification counters describe accepted ranges and their summed byte lengths,
not transferred bytes. Existing memory adapters, mappings, shader/resource
caches, submission tokens and completion semantics remain unchanged.

## Immediate bitfield extraction

Vertex and fragment programs accept unconditional signed/unsigned BFE from one
ready GPR or RZ, with canonical immediate offset/width. Offset must be below 32,
width at most 32 and their sum at most 32. Signed extraction extends the field's
top bit. Zero width returns zero and full width preserves all source bits;
dedicated lowering avoids any host shift by 32. Source/destination aliasing is
supported, while pending destinations, unready sources, noncanonical scheduling,
BREV, CC, execution predicates, noncanonical high immediate bits, dynamic controls
and out-of-range fields fail explicitly. Some rejected forms are valid hardware
configurations beyond this bounded implementation.

Static readiness remains mandatory even for zero-width extraction. Multiplication
provenance is conservatively retained, including full-width identities and
zero-width outputs. Extraction clears direct-LDC texture-handle, PositionW and
reciprocal proofs; these uses require separately validated dataflow. Mutable guest
source words remain ordinary runtime data, independent of the shader cache key.

## Three-source bitwise truth tables

Both stages accept all 256 LOP3.LUT tables with three ready GPR/RZ operands,
unconditional execution, discarded PT predicate output and canonical six-cycle
scheduling. The WGSL lowering snapshots all three source words before its one
destination assignment, so the destination may alias any source. The result is
the selected truth-table bit independently at each of the 32 bit positions.
PT discard preserves existing P0 state; it does not initialize P0.

All operands must be statically initialized and ready even for constant or
identity tables. Multiplication hazards propagate from every operand. Ordinary
pending-destination rejection applies, while TEX/PositionW/reciprocal proofs are
cleared. Alternate predicate outputs, T/Z/NZ modes, X, CC, execution predicates
and immediate/cbuf source forms remain unsupported. No new runtime-facing API
or guest memory representation is introduced.

## Population count

Both shader stages accept unconditional GPR/RZ POPC with optional source bit
inversion. The result is a raw u32 count from zero through 32; destination may
alias the source. This SM50 instruction has variable latency. It requires an
unused write barrier zero and a ready source. Consumers, overwrites and exports
must explicitly wait for that barrier; six encoded stalls do not replace the
wait, including for RZ. Only one outstanding barrier is supported. Immediate
and cbuf forms, execution predicates, alternate barriers and extra modifiers
remain unsupported.

Register allocation includes the real source in bits27:20. Multiplication
hazards remain conservative, while texture-handle and perspective proofs are
cleared. POPC neither initializes nor overwrites P0. WGSL countOneBits operates
on the raw source, after optional bit inversion, in both entry points. No GPU
driver/runtime API or completion semantics change.

## Highest non-sign bit

Both stages accept unconditional non-SH FLO from a ready GPR/RZ, in unsigned
or signed mode. Unsigned finds the highest one bit; signed negative inputs
find the highest zero bit. No qualifying bit returns raw FFFFFFFF. WGSL
firstLeadingBit implements these two cases; its signed result is bitcast back
to u32 so the -1 sentinel retains its original bits. Destination/source aliases
are supported. No guest numeric float conversion is involved.

Like POPC, SM50 FLO requires unused write barrier zero and an explicit wait
before consuming, overwriting or exporting its result. This applies even to
RZ and sentinel-producing inputs. Source register metadata, conservative FMUL
hazards and invalidation of TEX/PositionW/reciprocal proofs are retained. FLO
leaves P0 unchanged. SH, inversion, CC, predicates, immediate/cbuf sources and
alternate scoreboard configurations remain unsupported.

## Integer-to-float conversion in translated shaders

Both supported shader stages accept scoreboarded GPR/RZ I2F RN, F32 destination
and U32/S32 source, with no operand modifiers. Register data remains raw u32
through the conversion; the result is the exact RN-even binary32 encoding.
The WGSL helper uses integer packing because host numeric conversion does not
promise this rounding choice. It is emitted once, globally before the entry
points, only for programs containing this operation. Existing shader source
without I2F is unchanged.

The single-barrier readiness restrictions also apply to zero and aliased inputs.
I2F does not initialize or change P0, retain texture-handle provenance, or establish
position-W/reciprocal proofs. Conservative unfused-rounding hazard tracking
remains intact. Mutable constants and vertex data do not specialize the helper
or shader cache key. Other type widths, rounding modes, byte selections,
execution predicates, modifiers and immediate/cbuf conversion forms are rejected.

## Floating comparisons producing GPR values

Vertex and fragment translation accept unmodified GPR FSET, AND PT, with all
sixteen comparison modes and mask/float result selection. False writes raw
zero; true writes FFFFFFFF or 3F800000 respectively. Sources must be initialized
and ready, incoming supported waits are respected, and a pending destination
cannot be overwritten. This fixed six-cycle operation does not allocate a
scoreboard barrier or initialize/change P0.

Both operands are observed before an aliased destination is written. Result
registers lose texture-handle and position-W/reciprocal proofs, while the
conservative FMUL rounding hazard propagates from either source. Float-result
selection is a binary instruction property, not a specialization on mutable
constants. Comparisons use raw-bit IEEE classification/order, including signed
zeros, subnormals, infinities and arbitrary NaN encodings. Unsupported modifiers,
logical predicates/combiners and source forms fail explicitly.

## Scaled integer addition

Vertex and fragment translation accept unmodified GPR ISCADD with all32
five-bit immediate shifts. Both ready source words are read before any aliased
destination is stored; shift/addition wrap modulo2^32. RZ is a valid source,
while a RZ destination remains outside the supported decoder subset. Fixed
stall6/no-write-barrier scheduling and supported incoming waits are validated.

The operation does not initialize or change P0. It clears texture-handle and
position-W/reciprocal proofs, including identity forms, and conservatively
propagates the existing FMUL rounding hazard from either input. Mutable guest
operands remain runtime data, independent of shader/pipeline identity. Other
source forms, carry/negation modifiers, CC, reserved shift bits and execution
predicates are rejected explicitly. Driver and submission APIs are unchanged.

## Low-lane integer widening

Vertex and fragment translation accept four unmodified GPR I2I forms:
U8 to U32, U16 to U32, S8 to S32 and S16 to S32. Only the low source byte or
halfword participates; upper source bits are discarded. Unsigned inputs are
zero-extended, signed inputs are sign-extended as raw 32-bit values. No host
floating-point conversion is involved. Source aliasing and RZ are supported;
RZ destinations remain outside the decoder subset.

These conversions require write barrier zero and a supported consumer wait,
even when the input is zero. The conservative single-barrier decoder requires
all earlier pending results to be ready before assigning that barrier. Type
fields are not register indices. I2I neither initializes nor changes P0 and
cannot preserve a texture handle or position-W/reciprocal proof. Existing
FMUL rounding-hazard metadata propagates from the source. Mutable guest data
does not specialize generated code. Other destination widths, cross-signed
conversion, byte selectors, saturation, source modifiers, CC, execution
predicates and immediate/constant-memory forms are rejected explicitly.

## Integer comparisons producing GPR values

Vertex and fragment translation accept unmodified GPR ISET, AND PT, for
LT/EQ/LE/GT/NE/GE on unsigned or signed 32-bit operands. Mask results are
FFFFFFFF or zero; BF results are the raw bits of1.0 or zero. Both sources are
read before any aliased destination is written. The scoped relation does not
initialize or modify P0. Unsigned comparisons operate on u32, signed comparisons
on bitcast i32, with no subtraction overflow or floating-point arithmetic.

ISET uses fixed stall6 and no write barrier, independently of ISETP's stall13.
Inputs and any earlier destination value must be ready; supported incoming
waits remain honored. The result loses texture-handle/position-W/reciprocal
provenance, while both sources contribute to conservative FMUL rounding-hazard
metadata. BF is bit44, not FSET's bit52. Valid hardware F/T modes, extended
carry, CC, non-PT boolean operands/combiners, execution predicates and
immediate/constant-memory source forms remain explicitly unsupported.

## Generic byte permutation

Vertex and fragment translation accept unmodified generic-mode PRMT with GPR
or canonical sixteen-bit immediate selectors. Four selector nibbles independently
choose bytes from the concatenated A/B words; nibble bit3 requests sign-byte
replication. Only the low sixteen selector bits participate. All register inputs,
including a GPR selector, must be initialized and ready even if their values
would not affect the selected bytes. Destination/source aliases snapshot all
inputs. RZ sources are supported; RZ destinations remain unsupported.

Fixed stall6/no-write-barrier scheduling and supported incoming waits are
validated. PRMT preserves P0, propagates rounding-hazard metadata from all
register inputs, and clears texture-handle and position-W/reciprocal provenance,
including identity permutations. The integer-only WGSL helper is emitted only
when needed. Constant-memory forms, specialized permutation modes, modifiers,
execution predicates and noncanonical immediate encodings remain unsupported.
The driver, renderer and runtime-facing APIs are unchanged.

## Three-source integer addition

Vertex and fragment translation accept ordinary full-width GPR IADD3. Each
source is a raw32-bit word and the result is their sum modulo2^32. All three
inputs must be initialized and ready before an aliased destination is written;
RZ sources are supported. Fixed stall6/no-write-barrier scheduling and supported
incoming waits are validated. P0 is preserved, all three sources contribute
to register requirements and conservative rounding-hazard metadata, and
texture-handle/position-W/reciprocal provenance is erased even for identity
sums. Halfword selection, intermediate shifts, source negation, carry/CC,
execution predicates and immediate/constant-memory forms remain unsupported.
Driver, channel, resource and runtime-facing APIs are unchanged.

## Low-halfword multiply-add

Vertex and fragment translation accept ordinary GPR XMAD with both low
halfwords unsigned or both signed, plus the untruncated third32-bit operand.
The result wraps modulo2^32. Upper halfwords of the multiplicands do not
participate. Signed products fit i32; lowering converts that product to raw
u32 before adding C. Destination aliases snapshot all three input values.
RZ inputs are supported; RZ destinations remain outside the subset.

All three inputs and any earlier destination value must be ready. Fixed
stall6/no-write-barrier scheduling and supported incoming waits are checked.
P0 is preserved, all three sources contribute to register/rounding-hazard
metadata, and texture-handle/position-W/reciprocal provenance is erased. Mixed
signedness, high-halfword selectors, shifted/merged products, alternate C modes,
carry/CC, execution predicates and immediate/constant-memory forms remain
explicitly unsupported. Renderer, driver and runtime-facing APIs are unchanged.

## Fragment MOV from constant memory

Fragment translation accepts full-quad-lane, unpredicated MOV from a bound
constant-buffer slot with a nonnegative aligned immediate byte offset through
7FFC. Slots0..31 reuse the existing B197 bindings, checked guest GPU mappings,
canonical dirty journal and fixed uniform image. Mutable words stay runtime
data. References deduplicate with scalar LDC and share its64-reference budget.

This direct MOV source uses word-offset encoding and fixed stall6/no write
barrier. Its result is immediately ready; unrelated pending registers remain
valid. An explicit destination-pending check protects the shared Constant IR
path, whose LDC producer has different barrier rules. Fresh constant data
replaces prior arithmetic/provenance metadata and can supply the verified TEX
handle source. P0 is preserved. Partial lane masks, execution predicates, the
ambiguous high offset bit and vertex-stage constant resources remain explicitly
unsupported. No driver or runtime-facing API changes.

## Fragment floating-point constant operands

Unmodified fragment FADD/FMUL can read one GPR/RZ source and one directly bound
constant-buffer word. Nonnegative aligned offsets through7FFC and slots0..31
reuse the existing checked resource resolver and64-reference uniform budget.
The typed FloatConstant operation stores a uniform reference, never mutable
constant contents or an immediate specialization. MOV, LDC and arithmetic
references deduplicate within that budget.

Fixed stall6/no write barrier, initialized/ready source and no pending destination write
are validated. Multiplication marks the existing unfused-rounding hazard;
addition rejects a product-derived GPR source. This applies across GPR, full
and short immediate, and constant-memory operand forms. Fresh raw constant
loads can replace old data and its hazard. P0 is preserved and arithmetic
clears texture-handle/position-W/reciprocal provenance. Vertex resources, high
offset ambiguity, modifiers, alternate rounding/denormal modes and fused
arithmetic remain unsupported. Lowering inherits the existing bounded F32
accuracy contract; it does not establish new all-input exact-IEEE guarantees.

## Predicated full-immediate moves

Vertex and fragment programs support full-lane MOV32I guarded by P0 or !P0.
The immediate is raw32-bit register data. A true guard replaces the destination;
a false guard retains its old value. The typed ConditionalImmediate operation
keeps this distinction without numeric conversion or specialization on mutable
predicate/resource values. Immediate instruction bits remain part of shader identity.

P0 and the prior destination must be initialized, and the destination must
have no pending write. This conservative false-path rule also applies when
an earlier comparison could be constant-folded. The supported schedule uses
fixed stall6 and no write barrier, retaining the existing explicit wait rules.
Old-destination and predicate-derived FMUL hazards are preserved. Conditional
writes clear texture-handle, position-W and reciprocal proofs conservatively.
The immediate never contributes a fake source register to allocation metadata.

Only P0/!P0 and the full four-lane mask are supported. Other execution predicates,
partial lane masks, reserved fields and general control flow remain unsupported.
Existing unconditional PT MOV32I behavior is unchanged. No driver or runtime
interface changes are required.

### GPU controller identity queries

The process-local NVIDIA adapter opens `/dev/nvhost-ctrl-gpu` as a separate,
closeable descriptor. It accepts `GET_CHARACTERISTICS` (`0xc0b04705`, 176-byte
in/out payload, using either Ioctl or Ioctl3) and a one-GPC `GET_TPC_MASKS` query (`0xc0184706`, 24-byte in/out
payload). Neither operation submits GPU work, pins guest memory, or publishes a
framebuffer. Descriptor ownership, ioctl direction/size checks, and the existing
32-file bound apply to this node as to the other nodes.

The characteristics structure describes the emulated GM20B/A1 chip, its 40-bit
GPU address domain, and its Maxwell interface class identifiers. It does not
expose properties of the host Intel/software adapter. Optional capability flags
are zero: sparse mappings, statistics, sync-fence file descriptors, and general
Host1x syncpoint operations are not advertised. Returning an interface class
identifier does not imply implementation of every instruction in that class.

The Switch ABI returns characteristics inline after two 64-bit header fields.
A requested structure size of at least 160 bytes is supported and the returned
size is 160. The nonzero address field is preserved as an opaque value, including
libnx's literal value of one; it is never dereferenced. Short requests and null
legacy address tags fail before modifying output. Ioctl3 does not require the
legacy tag: its second output is the destination, and a zero unused tag is valid. TPC-mask queries support exactly one
32-bit mask, with zero reserved bits and a nonzero opaque address. The returned
mask is three, identifying the two TPCs in GPC zero; unused inline bytes are zero.
Other shapes and controller ioctls, including ZCULL/ZBC and GPU-time queries,
remain explicitly unsupported.

The Ioctl3 characteristics variant additionally requires exactly 160 bytes in the
second output and no additional input. Both output extents and all input fields
are checked before either output changes. The identity is returned in both the
regular structure and the second output. The runtime validates all guest output
ranges, including overlap with each other and the TLS reply, before invoking the
backend. A failed transport/publication preflight does not poison the descriptor;
a valid query can be retried. These bounded query forms do not enable Ioctl3 on other
device nodes or unimplemented controller requests.

TPC masks also support Ioctl3 with a four- or eight-byte second output. Its
nonzero size tag is not an allocation request, and its reserved words are not a
legacy pointer. Both the regular mask field and the second output report mask
three for GPC zero; the absent second GPC has a zero mask. Zero size tags and
unsupported output extents fail before either metadata output is written.

### Explicit NVIDIA address-space initialization

`InitializeEx` (`0x40284109`, input-only, 40 bytes) decodes flags, signed
address-space descriptor, large-page size and reserved word at offsets 0, 4, 8
and 12. Three 64-bit range fields follow at 16, 24 and 32. The Switch wire order
follows libnx; it must not be replaced by the differently ordered desktop
`nvgpu_alloc_as_args` structure. The adapter accepts flags 0/1, descriptor zero,
reserved zero and 64/128 KiB large pages (zero selects 128 KiB). Nonzero sharing
descriptors and other flags remain unsupported.

All-zero range fields retain the existing bounded fixed-mapping policy. Explicit
ranges require aligned, strictly ordered start/split/end within the 40-bit GPU
address domain. Fixed 4 KiB reservations occupy the lower region; configured
large-page reservations occupy the upper region. Ends are checked exclusively.
The existing GpuDriver retains reservation, mapping and canonical backing
ownership; no hardware page-table implementation is introduced. Free requests
match the original reservation's page size as well as its address and length.
Validation precedes marking a context initialized, and neither initialization
nor reservation submits graphics work or publishes memory. Sparse mappings remain unsupported.

`GetVARegions` (`0xc0404108`) exposes those configured regions as two
24-byte descriptors: 64-bit base, 32-bit page size, reserved zero, and 64-bit
page count. It accepts a 64-byte primary request/output with the bounded buffer
size zero (sizing query) or 48. The returned size is always 48. Ioctl3 additionally requires an exact 48-byte second output; both
destinations are validated before mutation. The unused address tag is preserved
without dereferencing it. The default configuration with unspecified ranges
returns unsupported rather than inventing a host VA allocator or inconsistent
region limits. Neither query changes mappings, submits GPU work nor guarantees
completion.

`AllocSpace` with flags zero interprets the final 64-bit input as an alignment,
not a GPU address. For explicit configured regions it uses bounded first fit
over live reservations, with zero alignment selecting the requested page size.
Nonzero alignment must be a power of two at least that page size. Successful
requests acquire a real reservation through the existing GpuDriver and return
its address. Failure leaves metadata unchanged. Freed gaps can be reused; live
reservations cannot overlap. The search uses at most the registry's bounded
reservation count and does not allocate a command buffer or page-table shadow.
Sparse flags and automatic reservation without explicit region bounds remain
unsupported. This allocation policy is a host implementation choice within the
requested guest VA regions, not a claim of identical kernel allocation order.

### User-backed nvmap cache metadata and alignment

`NVMAP_ALLOC` accepts cache flags zero and one and a power-of-two requested
allocation alignment of at least 4096. The user-supplied CPU address must be
page aligned, nonzero and have a checked complete extent, but need not satisfy
that larger allocation alignment. It refers to already allocated runtime
storage, not a request to allocate or move guest CPU memory. Heap mask zero,
kind zero and reserved zero remain required.

The libnx consumer selects these two flag values from CPU cacheability. They
are not interpreted as read-only/read-write permissions. Both require the
existing checked runtime pin for one stable NORMAL RW region and register
canonical backing through the existing GPU driver. Host coherence remains the
shared write journal and explicit publication boundary; no hardware CPU cache
maintenance or arbitrary guest memory attribute support is claimed. Invalid
metadata fails before pinning; registration failure releases its temporary pin.

### Cache-policy transitions on registered runtime storage

The runtime permits `SetMemoryAttribute` Uncached transitions over complete GPU
registrations. The request still passes normal range, memory-state and attribute
validation. Changes that cut inside a registered span remain unsupported because
the current guest-memory borrowing interface requires one contiguous region per
pin. Permission changes, unmapping, alias creation and replacement of registered
storage retain their existing guards. The registration token, canonical guest
addresses and bytes survive cache-policy transitions; no host borrow escapes.
This is coherent-host cache metadata, not emulation of hardware cache contents.

When isolating a small interval near the beginning of a large region, the runtime
retains the larger Vec allocation and allocates only the shorter prefix. It
still moves the remaining bytes in place. Splitting near the end retains the
large prefix and allocates the short tail. This avoids a second large allocation
when a one-page cache request isolates part of a multi-gigabyte guest heap.

`NVMAP_PARAM` (`0xc00c0109`, 12-byte in/out) reads the process-local allocation
handle and parameter selector and writes a 32-bit result at offset eight. SIZE
(1) reports the created object's checked size. ALIGNMENT (2), HEAP (4) and KIND
(5) require attached user backing and report its retained allocation alignment,
system-memory heap `0x40000000`, and the supported kind zero. BASE (3) remains
unsupported; unknown selectors and stale handles reject without changing the
output. These queries acquire no guest-memory borrow, pin, upload, submission
or completion. They do not implement cross-process nvmap IDs or import/export.

### Automatic user-buffer GPU mappings

`MapBufferEx` accepts fixed/cacheable bits zero and two, kind zero, generic
uncompressed `0xfe` (`Generic_16BX2`), or default,
and whole or checked page-aligned subranges of an attached nvmap allocation.
Without the fixed bit, a zero final offset requests bounded automatic VA
placement in the configured small-page region. The page hint (4/64/128 KiB,
or zero selecting retained allocation alignment) constrains VA alignment.
Only the actual 4 KiB-granular backing extent is mapped: a 4 KiB allocation
with a 64 KiB hint does not grant access to another 60 KiB of guest RAM.
The hint is retained in the returned wire metadata; this software mapper does
not claim to select or emulate physical GMMU page sizes.

The existing GPU driver owns each reservation, mapping ID and mapping epoch.
Automatic VA is acquired only after size/handle validation and is released if
mapping fails. Unmapping an automatic mapping also releases its VA; fixed
reservations retain their distinct `FreeSpace` lifecycle. Nonzero automatic
offset/alignment unions remain unsupported. Canonical backing pins survive unmapping until allocation/session
teardown, and pending-output guards still prohibit mapping mutation.
Pitch and generic-uncompressed aliases retain identical canonical storage.
Mapping does not swizzle bytes: supported TIC/B197 surface descriptors select
the existing layout converters when resources are accessed. Compressed PTE
kinds requiring compression tags remain unsupported.
With an explicit 64 KiB page hint, `MapBufferEx` also applies the same bounded
uncompressed-kind fallback used by `Remap`, including C32_2CRA (`0xdb`) to generic
storage. NVIDIA disables compression below its 128 KiB compression granularity.
This does not decode compressed bytes or enable additional attachment formats.
Unspecified/128 KiB page hints and unknown kinds cannot use this fallback.

Switch `MapBufferEx` bit eight (`0x100`) accepts checked kind changes that retain
uncompressed storage. Its base must identify a live mapping;
the byte offset and length must select resident 4 KiB-aligned bytes within that
mapping, excluding VA padding. Handle/page fields must be zero in this subset.
Generic/pitch/default kinds are accepted; known color/depth-compression aliases also
require a mapping established with the 64 KiB fallback above (or 64 KiB Remap).
The mapping identity, epoch and canonical bytes remain unchanged because no
physical pages or compression representation are replaced. This operation does
not submit GPU work or publish pending output. Other kind-changing operations
remain unsupported. The wire interpretation follows the
[pinned MIT-licensed device implementation](https://github.com/alula/Ryujinx/blob/4930dced25c0d9ec33096b0f2601d262daa477bf/src/Ryujinx.HLE/HOS/Services/Nv/NvDrvServices/NvHostAsGpu/NvHostAsGpuDeviceFile.cs).

Fixed `MapBufferEx` mappings may occupy an unmapped subinterval of an existing
reservation, including multiple disjoint buffers within one large reservation.
Each complete mapping has its own identity, exact extent and canonical backing;
`UnmapBuffer` selects that mapping's start address. Removing it leaves adjacent
mappings valid, advances the mapping epoch and keeps the reservation allocated.
`FreeSpace` rejects while any child mapping remains. Overlap, crossing reservation
boundaries, backing overflow and stale IDs fail without changing the epoch or
existing mappings. There are at most 4096 mappings per address space. This does
not implement fragmented gathers or real hardware page tables. Replacement and
splitting use the distinct reservation-scoped Remap operation below.

Resident backing/reservation registries are bounded to 4096 objects; the
process-local nvmap registry permits 1024 live handles. These are implementation
budgets, not hardware limits. Individual Remap transactions remain limited to
128 entries, independently of resident capacity. Exhaustion preserves existing
identities, mappings and guest bytes. Opt-in `WL_NVDRV_TRACE=1` reports GPU
validation reasons before projecting them to the existing guest error code.

### Reserved virtual allocation remapping

`AllocSpace` accepts the sparse flag (bit 1), optionally with fixed-address bit
0, for 64 KiB pages. It reserves GPU VA without making absent pages readable.
The GPU characteristics sparse-allocation capability is set; partial
`MapBufferEx`, sync-fence FD and cycle-stat capabilities remain unset.

The variable-size `0xc0??4114` Remap ioctl consumes 20-byte little-endian
entries: 16-bit flags, 16-bit kind, nvmap handle, reserved physical-offset word,
GPU page offset and page count. Supported flags are zero/cacheable (4), kinds
include pitch (0), generic uncompressed (0xfe), and the bounded small-page
uncompressed fallbacks described below. Physical offset must be zero.
Page offsets and counts are in 64 KiB units. A zero handle removes resident
backing; a nonzero handle binds the beginning of a checked allocated nvmap
backing. Every range must fit one existing sparse reservation. Nonresident
resource reads fault explicitly; this is not sparse-texture zero-fill support.

At most 128 entries are applied in order to a bounded metadata transaction.
Replacement splits surviving mapping intervals while retaining their canonical
backing offsets. Failed validation preserves existing mapping IDs and epochs;
successful replacement issues fresh IDs for changed intervals. Aliases outside
the replaced interval remain valid. No command, texture or framebuffer bytes
are copied by this transaction. Mapping mutation is rejected while GPU output
is pending, so replacement cannot redirect an earlier framebuffer publication.
After unmapping every resident interval, `FreeSpace` releases the reservation.
Nonzero physical offsets, compressed payloads/tags and hardware sparse PTEs
remain unsupported.

For these 64 KiB mappings, known C32/C64/C128 compression kinds resolve to
generic uncompressed storage; Z16, Z24S8, ZF32 and ZF32_X24S8 aliases resolve to
their corresponding uncompressed kind. This follows NVIDIA's downgrade when
page size is below the 128 KiB compression granularity. The explicit subset is
validated before changing mappings; unknown kinds still reject. Registration
preserves canonical bytes and does not enable the corresponding raster format,
multisampling, compression metadata or stencil behavior. B197/TIC descriptors
continue to validate the actual resource interpretation at use time.

### Contiguous CPU backing across internal regions

Runtime GPU registration accepts contiguous NORMAL read/write, non-executable
storage with matching cache attributes (cached or Uncached). If the interval
crosses adjacent internal regions, registration first validates the entire
extent and prepares one owned byte buffer, bounded to 256 MiB. It then isolates
the exact interval and consolidates its bytes into one runtime region. This is
a one-time CPU copy, not a borrowed gather or zero-copy allocation. Later GPU
reads borrow that region directly under the existing scoped memory contract.

Bytes and attributes outside the interval and existing nonoverlapping pin
tokens are retained. Gaps, incompatible attributes/state, executable/readonly
pages, overlap with a pin and excessive consolidation sizes reject. An ordinary
single-region pin does not copy payload bytes and retains its existing size
limits. CPU mutations and GPU publication keep using canonical guest addresses;
no guest pointer becomes a host pointer, and no asynchronous guest borrow is
retained. This consolidation changes internal storage boundaries, not ownership
of the guest allocation or GPU VA mappings.

### Channel allocation and error events

`AllocGpfifoEx2` (`0xc020481a`) accepts the bounded Switch configuration with
flags one, zero unknown input fields and 1..32768 queue entries. Its output
fence fields at bytes 12 and 16 are overwritten, irrespective of their input
contents. Allocation associates capacity with the existing channel and returns
its initial process-local synchronization identity and threshold; it submits no
GPU work and advances no completion counter. The independent per-call execution
budget remains 2048 GPFIFO entries. Duplicate allocation is rejected unchanged.
The legacy eight-byte allocation retains its original bounded configuration.

`QueryEvent` service command four supports channel error event three. It returns
a copied readable kernel-event handle, initially unsignalled. Repeated queries
refer to one event object. Enabling `SetErrorNotifier` (`0xc018480c`) latches an
accepted execution/publication fault and signals the event through the existing
kernel waiter mechanism before the fatal service result propagates. Ordinary
preflight errors and successful submissions do not signal it. This diagnostic
is not a GPU-completion fence; hardware error-notification payloads, breakpoint
events and general Host1x events remain unsupported.

Channel priority accepts the three libnx policy values under the current
serialized execution model, which has no concurrent jobs to preempt. User data
is retained as opaque metadata without dereferencing it. `SetTimeout`
(`0x40044803`, one little-endian u32 in milliseconds) configures a per-channel
host queue-progress deadline. Zero retains the unbounded queue-wait policy;
positive values bound nonblocking polling for actual queue completion and buffer
mapping callbacks. Only observed queue completion resets the deadline. Setting
the policy does not submit work, publish memory or create a fence.

This deadline is a host-side supported subset, not Maxwell instruction
preemption or a hardware GPFIFO-GET watchdog. It cannot interrupt a backend call
that itself blocks. Completion of an earlier token still refers to that exact
queue prefix. Readback timeout cancels the pending map and leaves guest output
unpublished and retryable. Deferred draws do not acquire a blocking wait simply
because the channel has a finite deadline. `completion_timeouts` records actual
no-progress expiry separately from accepted submissions and completion queries.

`SetTimeslice` (`0xc004481d`) validates and retains a 1000..50000 microsecond
channel policy and preserves its in/out payload. Serialized execution has no
competing runlist; this does not provide hardware preemption. Priority updates
select the corresponding policy before a later explicit timeslice override.

Kickoff accepts `HW_FORMAT | IncrementWithValue` when the reserved guest
increment count is zero. It executes and publishes through the same checked
submission path and returns the existing completed threshold, without inventing
an increment. Nonzero guest-command increment reservations remain unsupported:
they require command-driven syncpoint execution. The separately supported
driver-generated fence increment still advances by two after real completion.

If a submission publishes a valid rendered prefix and then reports an accepted
command fault, the diagnostic capture path retains that published image before
propagating the original fatal error. It does not turn the failed request into
success, write a successful ioctl response, replay commands or invent a VI
presentation. A publication failure produces no new image to capture.

### SDL diagnostic status

`WL_GPU_DEBUG_WINDOW=1` opens the SDL2 diagnostic window at backend creation.
The runtime periodically supplies actually retired AArch64 instructions and host
elapsed time. The window title reports effective MIPS, actual GPU adapter/backend,
consumed Maxwell command words, draws, host queue submissions, translated shaders
and explicitly published color targets. Command words are not dynamic shader
instruction counts, and a published target is not a verified VI frame.
`WL_GPU_STATUS_FILE` optionally publishes bounded text status via atomic file
replacement. The `wl-runtime` example `status_window STATUS.txt` displays this
file in a separate native SDL2 window title, including Windows, while compilation or
execution proceeds elsewhere. Its repaints never increment guest graphics or
frame counters. Host clock frequency and a gameplay FPS value are not inferred
from CPU instruction throughput. The image area remains reserved for guest output; diagnostics do not draw a text panel.

`SetErrorNotifier` uses the Switch semantics of the legacy `mem` word: zero
turns notifications off and every nonzero value turns them on. The offset, size
and reserved words are ignored, including nonzero values; no pointer is
resolved from these fields. In particular, a legacy-shaped request with size
4096 and mem four enables the same actual error event as libnx's zero-size,
mem-one form. The Linux driver's shared notifier-buffer semantics do not apply.

GPU-enabled AOT builds also compare an embedded `native_gpu` host module with
this code generator's adapter source before creating caches or compiling guest
units. A saved entry wrapper can otherwise compile using default trait callbacks
while omitting newer event and diagnostic implementations. Stale wrappers fail
explicitly. Refreshing the host-only module does not require changing guest
instruction sources, image blobs or runtime-compatible translation libraries.

`SET_ANTI_ALIAS_ENABLE` (`0x1534`) accepts only the documented FALSE value zero.
It preserves the supported single-sample pipeline; TRUE, reserved bits and
multisample target modes remain rejected. A failed enable write leaves the
previous supported raster state intact. This does not implement MSAA or resolve
operations. Literal GPFIFO draws exercise explicit disable before rasterization.

B197 scissor arrays retain all 16 enable/horizontal/vertical slots at
`0x0e00 + index*16`, `+4` and `+8`. The reserved `+12` word remains unsupported;
alignment and enable values are checked before mutation. Supported vertex
programs cannot export a viewport index, so only scissor zero is consumed by
rasterization. Inactive scissor writes, including empty rectangles, cannot alter
zero's clipping or pipeline identity. This does not add multi-viewport execution.

The single-sample subset also accepts the inactive value zero for alpha coverage
control `0x153c` (both alpha-to-coverage and alpha-to-one disabled), and for
Nouveau's legacy `CSAA_ENABLE` at `0x15b4`. NVIDIA's public B197 header does not
name the latter; the interpretation is corroborated by Nouveau initialization
and NVK's Maxwell-era context discussion. Nonzero values remain unsupported.
No coverage-sampling, MSAA, or alpha-to-one implementation is inferred.

## Inline constant-buffer command writes

B197 `LOAD_CONSTANT_BUFFER_OFFSET` (`0x238c`) programs an aligned 16-bit byte
offset. The sixteen `LOAD_CONSTANT_BUFFER(i)` ports (`0x2390+i*4`) all append
one little-endian u32 to the current selector, advancing the same offset by four.
They do not write to `base+i*4`. Selector state is independent from the stage/slot
binding snapshots. The existing implementation budget remains 64 KiB per selector;
missing selectors, unsupported offsets, range overflow and 40-bit address overflow
fault before a write effect is exposed. There is no inferred offset wraparound.

A typed write effect snapshots the destination GPU VA and data. GPFIFO acquisition
releases its command borrow before execution, validates the mapped write range,
conditionally publishes overlapping GPU-newer attachments, preflights the shared
journal, commits one atomic four-byte write, then marks the canonical byte range.
It reacquires the unread command suffix afterward, so a write aliasing future
command words cannot execute stale borrowed data. No guest pointer or borrow is
retained. The direct synchronous pushbuffer helper retains its most recent frame
when later non-rendering effects return no frame.

`constant_buffer_write_bytes` counts committed canonical writes, separately from
host uniform uploads. Write failures leave the journal unmodified and latch the
channel; already accepted draws remain publishable without replay. The path does
not implement conditional constant loads or general inline/DMA engines. Changes
become shader resource data through the existing dirty synchronization, leaving
shader and compatible pipeline identities unchanged.

Color-reduction threshold registers (`0x10cc`, `0x10e0..0x10f0`) retain both
hardware fields with strict reserved-bit validation. FP11 uses two 6-bit fields;
the other formats use two 8-bit fields. Programming thresholds does not imply
enabling or disabling reduction. If any threshold was programmed, a draw or clear
requires an explicit `SET_REDUCE_COLOR_THRESHOLDS_ENABLE=FALSE` (`0x0d9c=0`).
TRUE remains unsupported; no lossy reduction or precision behavior is synthesized.
The retained disabled thresholds do not specialize the render pipeline.

`SET_ALPHA_FRACTION` (`0x074c`) retains its 8-bit value. Its use is similarly
limited to explicitly disabled alpha coverage/alpha-to-one (`0x153c=0`); no
coverage table or power-on default is inferred. Invalid writes preserve earlier
supported state. These register-only writes can precede the disabling controls,
but rendering before the required controls have been established fails explicitly.

Window clip parameters at `0x0d00 + index*8` and `+4` retain eight supported
horizontal/vertical pairs. The initialized B197 context disables window clipping,
matching the Nouveau B197 method-initialization table (`0x194c=0`); programming
rectangle bounds or mode alone does not enable it. `0x194c` accepts FALSE/TRUE
and `0x1950` retains inclusive, exclusive and clip-all modes. When enabled,
execution currently accepts only an explicit exclusive mode with all eight
rectangles explicitly empty. An empty half-open X or Y interval cannot reject a
sample. Other enabled modes fail before drawing or clearing. Invalid enable
values do not mutate the previous state. This initialized-context value is not a
claim about hardware power-on state, other register defaults, or complete window
clipping support.

Clear controls are latched independently from the `CLEAR_SURFACE` trigger.
All defined configuration bits can be programmed, including combinations that
are not executable yet. Unsupported clear execution fails before attachment work;
it does not block unrelated draws or silently perform a full-surface clear.
Reserved configuration bits still fail at the write. Full-scissor clears use the
existing real GPU attachment clear and preserve pitch/block-linear padding and
explicit publication semantics.

B197 cache eviction preferences are retained separately for ROP prefetch reads
(`0x0218`), VAF (`0x1000`), noninterlocked reads (`0x10fc`), interlocked reads
(`0x1290`), and noninterlocked/interlocked writes (`0x12d8/0x12dc`). POLICY values
FIRST/NORMAL/LAST occupy bits 5:4. Only the nonvolatile form is accepted for VAF;
reserved policy values, volatility and other bits remain unsupported. These are
eviction preferences, not cache flushes or guest-memory visibility barriers.
wgpu manages physical host cache policy; canonical dirty tracking and ordering
remain mandatory. Changing a preference does not invalidate shaders or pipelines.

The bounded shader formats accept `CHECK_SPH_VERSION=0x00030003` and
`CHECK_AAM_VERSION=0x00020002`, matching the pinned Maxwell layout. Other pairs
fail explicitly; version-setting operations and general firmware version handling
are not implemented. Header and instruction validation remains unchanged.
`SET_SAMPLER_BINDING=INDEPENDENTLY` (`0x1234=0`) configures the existing separate
TIC/TSC handle path. Shader exceptions may be explicitly disabled (`0x1528=0`).
Header-bound sampler mode and enabled guest shader exceptions remain unsupported.

`NVB06F_NOP` at channel byte offset `0x0008` accepts a full-width tag independently
of engine/subchannel binding. It is distinct from the zero packet word and from
engine `NO_OPERATION` (`0x0100`). It preserves engine/packet state and creates no
queue submission, memory barrier, interrupt or completion signal.

`SET_DA_PRIMITIVE_RESTART_VERTEX_ARRAY` (`0x0de8`) retains its boolean policy.
When it is TRUE, a nonindexed draw requires explicit global restart disable
(`0x1644=0`). Indexed draws already require that state. Global restart enable,
cut-index behavior and additional topologies remain unsupported. This distinction
admits policy initialization without pretending that active restart is implemented.

All sixteen vertex-array instance flags (`0x1880+index*4`) are retained separately.
The supported position/UV attributes use stream zero, whose flag must remain FALSE;
other stream flags cannot change its fetch behavior. No instanced drawing or new
stream bindings are added. Only canonical attribute defaults `0x1610=0x0e` are retained;
other Float3/component expansion modes remain unsupported, and
missing/default attributes still fail existing input/linkage validation.

Provoking-vertex FIRST/LAST (`0x1684`) and bounded API-visible call limits
(`0x0d64`, zero through 128 using the documented enum) are retained. Flat guest
interpolation and guest CALL/RET remain unsupported, so those controls do not
specialize admitted smooth, straight-line programs. NO_CHECK is not admitted.
Only canonical OGL point-center configuration (`0x165c=0`) is accepted; point
rasterization remains unsupported. The `0x1140` format blend gate is retained
for SNORM8/UNORM16/SNORM16; it does not gate the implemented RGBA8 format and does
not add those unsupported target formats. These inactive settings cannot seed
host geometry, manufacture default inputs or bypass shader validation.


### Auxiliary 2D configuration and vertex-ID control

The NV902D engine independently retains only SRCCOPY operation (`0x02ac=3`),
clip disabled (`0x0290=0`) and the ten-bit memory corral size (`0x0884`). These
writes do not transfer memory or launch a copy. Other operations, enabled clip,
reserved bits and launch methods remain explicit errors. Engine selection routes
these fields separately from B197; channel instances retain independent state.

B197 `SET_DA_OUTPUT` (`0x164c`) retains only its defined bit 12 controlling
whether VertexID includes array start. The bounded vertex shader subset admits
position/UV attribute loads, not VertexID reads, so this does not change their
fetching, provide missing inputs or extend built-in shader support.


### MME register arithmetic

The bounded MME interpreter supports register ADD, SUB, XOR, OR,
AND, ANDN and NAND. ADD/SUB wrap at 32 bits; ANDN computes left AND inverted right,
and NAND inverts the complete 32-bit intersection. R0 remains zero, nonzero source
registers must be initialized, and reserved instruction bits are rejected.
ADC/SBB are not accepted: no incoming carry state is synthesized. Existing
instruction/step budgets, exit delay slot and scratch-only reads remain enforced.
Emissions are restricted to validated state writes and waits; this does not
introduce branches, parameter waits, privileged registers or macro-generated draws.

An independently assembled chain exercises all six new operations before emitting
a B197 color mask. Actual indexed GPFIFO draws validate red-only versus full-channel
pixels with unchanged shader resources. CPU vectors separately distinguish each
ALU function, underflow, overflow, sign-bit handling and uninitialized sources.


### Restricted auxiliary raster controls

B197 ZCULL-statistics enable (`0x151c`) and aliased-line-width enable (`0x020c`)
retain their documented boolean values. This does not produce ZCULL reports or
add line topology: report/semaphore queries and line drawing remain rejected.
Conditional constant loading can only be explicitly disabled (`0x030c=0`),
matching the existing unconditional checked constant uploads.

Pixel-shader output-mask usage (`0x0300`) retains its two boolean fields. Both
admitted SPH decoders require the sample-mask output bit to be zero, so these
settings cannot introduce a mask output absent from the guest program. The
negative shader-header test preserves that restriction. Supporting sample-mask
exports later requires implementing this control in raster execution as well.


### Normalized-target blend controls

`SET_BLEND_OPT_CONTROL` (`0x0fdc`) retains its boolean permission for float-pixel
kill optimizations; setting it does not issue a kill or implement float targets.
`SET_BLEND_FLOAT_OPTION` (`0x19c0`) admits only ZERO_TIMES_ANYTHING_IS_ZERO=TRUE
alongside the existing RGBA8_UNORM color-target subset. FALSE and floating-point
attachments remain rejected. This is not a general IEEE floating-target mode.

A guest LDC fragment exercises finite values, positive/negative quiet NaNs and
positive/negative infinities. With both literal ZERO and runtime source-alpha-zero
weights, all destination RGBA8 bytes remain unchanged. A finite ONE/ZERO positive
control produces covered and uncovered pixels, proving that the draw executes.
The eleven draws share one translated vertex/fragment pair. Each iteration
explicitly publishes for verification; these test readbacks are not introduced
by either register setter or the deferred submission path.


### Bounded shader/raster initialization

The initialization state retains polygon-clip generated-edge selection (`0x0f8c`),
two-sided-stencil selection (`0x1594`), canonical SPA 5.3 (`0x0310=0x0503`), and
all 32 bits of the shader-local memory window base (`0x077c`). The window is not
a GPU allocation or a host pointer. Both SPH decoders still reject nonzero local
storage, and the ISA subset cannot access local memory. Line polygon modes,
enabled stencil, other SPA assertions and geometry stages remain errors.

TIR control (`0x1130`, defined bits 0,1,4) and balanced primitive workload
(`0x0374`, defined bits 0,4) are retained without adding multisampling, alpha
coverage or statistics queries. Execution remains one raster/color sample.
RT-layer selection (`0x15cc`) admits only V=0 with either defined selector;
all attachments have one layer and admitted shaders cannot export a layer.
This does not introduce layered geometry or manufacture a layer from host data.


### Automatic texture-handle selector and retained thresholds

`SET_BINDLESS_TEXTURE` (`0x2608`) retains the five-bit constant-buffer slot
selector. It does not bind a texture or allocate a constant buffer. The admitted
TEX.B form instead obtains its handle from a GPR with explicit LDC provenance;
changing the global selector to an unbound slot leaves that dataflow unchanged.
Automatic handle forms remain unsupported and cannot inherit a hidden slot zero.

`SET_COMPRESSION_THRESHOLD` (`0x1220`) retains only the documented sample-count
enums 0 through 12 (zero through 2048 samples). This adds neither NVIDIA memory
compression tags nor compressed render-target storage. `SET_PS_WARP_WATERMARKS`
(`0x1450`) retains independent 16-bit LOW/HIGH values; this implementation admits
only LOW <= HIGH. That ordering is a conservative supported-subset limit, not a
claim about every legal hardware combination. Host scheduling remains owned by
wgpu; these fields introduce no completion, fence, timing or allocation promises.


### B197 wait for idle

`WAIT_FOR_IDLE` (`0x0110=0`) produces a typed execution effect. The GPFIFO
consumer releases its guest command borrow before waiting, then reacquires any
unread suffix. Both direct and GPFIFO paths wait for the renderer's exact last
submitted queue prefix through the existing completion watchdog and actual wgpu
completion mechanisms. A queue with no submitted work requires no host wait.
Other argument values remain outside the supported subset.

WFI does not submit a new draw, perform framebuffer readback, publish guest RAM,
reset engine state or signal a fabricated guest syncpoint. Pending color/depth
output remains resident and explicitly publishable. A subsequent channel fault
cannot undo that earlier work or require replay. The synchronous compatibility
path retains its already returned frame across a trailing WFI. The same effect
is available through the bounded resumable MME interpreter.


### Resumable MME state emissions

An invocation owns only fixed-size PC/register/readiness state and references the
existing bounded uploaded RAM through the owning engine. No guest command borrow
survives a backend yield. The consumer executes the returned effect, resumes the
same invocation, and only then acquires the unread FIFO suffix. Guest register
rebinding, code upload or nested macro launch cannot replace an invocation while
its backend effect is outstanding. The step budget is not reset on resumption.

MADDRSEND emits bits 17:12 and advances the method by the encoded word increment.
Method-address wraparound remains unsupported. The independently assembled WFI
instruction selects byte method `0x0110` with zero data and zero increment.
A synthetic macro emits a color-mask write, a real WFI and another color-mask
write; its guest argument changes verified texture pixels. Fault injection after
WFI preserves an earlier real draw and permits publication without draw replay.
General firmware requests remain unsupported; the restricted privileged
initialization protocol and validated draw routing are described below.


### MME parameter ports and scratch emissions

Parameter assignment modes 0, 3, 5 and 6 retain explicit pending state. They
consume one complete u32 from the exact DATA method (`launch+4`) on the launching
subchannel. An absent parameter is never replaced with zero. Mode 3 sends its
previous ALU result before fetching the new destination; mode 6 preserves its
ALU result in the destination and sends the fetched value. Parameter-dependent
EXIT completes its following delay slot only after the parameter is consumed.
An outstanding backend effect cannot be acknowledged by supplying a DATA word.

Methods retain 12-bit word addresses and six-bit word increments, converted to
checked byte offsets. Incrementing outside `0x0000..0x3ffc` is rejected, without
assuming address wrap. MME output may update the already supported scratch range
`0x3400..0x37fc`; these are actual guest instruction writes, not firmware replies.
Waits and color masks remain supported; privileged outputs outside the exact
initialization profile below remain errors.

Parameters can continue across GPFIFO entries and GPU VA aliases in the same API
submission. Waiting for an argument at the end of an API submission is explicitly
unsupported and faults that channel; it does not return a completed/fake fence.
This restriction is distinct from ordinary B06F packet continuation across calls.
Failures retain earlier GPU work for publication and prevent submission replay.
The 1024-instruction budget persists across all parameter and backend resumptions.

### Bounded MME conditional branches

BRAZ and BRANZ test a ready u32 register against zero. Their signed 18-bit
displacement is relative to the current instruction in instruction-word units.
A taken ordinary branch executes one delay-slot instruction before jumping;
a taken ANNUL branch jumps immediately. An untaken ANNUL branch still executes
the following instruction. A branch's EXIT modifier applies only when the
branch is not taken, retaining the ordinary EXIT delay slot.

Branch targets are checked against uploaded-RAM capacity; an unuploaded target
does not become an implicit NOP. A branch in any delay slot is rejected. EXIT in
a branch delay slot is also explicitly unsupported. Delayed targets survive
both parameter waits and backend yields without retaining guest borrows.
Backward branches share the existing total 1024-instruction invocation budget;
neither a jump nor a backend resumption resets it. Budget failure faults the
channel while preserving already submitted GPU output for explicit publication.
Branch execution itself does not acknowledge privileged requests; only the
separate validated initialization protocol below can do so.


### Privileged initialization through MME

MME output `SET_FALCON04` (`0x2310`) admits only the seven exact baseline masked
writes listed in the source ledger. Firmware scratch word zero must contain
request flag zero; words one and two must be explicitly initialized value and
mask. The channel must have completed a macro-emitted WFI before each request.
The backend's private resume boundary supplies that completion; a guest DATA
word cannot acknowledge a wait. Any intervening non-WFI backend effect revokes
the authorization, including a draw, clear or checked guest-memory write.

The implementation updates fixed-size per-channel context-register storage,
preserves all previously known bits outside the mask, and then writes the
completion flag. Unknown reset bits carry a separate unknown mask instead of
invented reset values. Each operation consumes its WFI authorization. Direct
firmware method calls without this guarded macro protocol remain unsupported.
An engine NO_OPERATION emission is admitted for the guest polling sequence;
it does not submit work, publish memory, or invent a guest syncpoint.

This is a baseline initialization HLE, not a Falcon instruction interpreter or
general MMIO implementation. The exact source-verified baseline does not add
support for alternative privileged raster modes. Unsupported requests fault
before context mutation or acknowledgement, preserving earlier queued draws
for explicit framebuffer publication. No additional readback is needed merely
to acknowledge an admitted context write.


### MME bitfield dataflow

EXTRINSRT replaces the selected destination bits of source A with logically
extracted source-B bits. The two EXTRSHL forms extract source-B bits and shift
them into a zero-initialized result, using either source A as the extraction
position or as the destination shift. The zero register, old-input reads before
aliased assignment, parameter modes, SEND ordering and total instruction budget
remain shared with the existing interpreter. Width zero produces no inserted
bits; bits extending outside a 32-bit source or destination are zero/truncated.
Unused encodings and variable shifts outside 0..31 fail explicitly.


### Hierarchical Z/stencil controls

`0x1968` and `0x196c` accept only bits 0 and 4. `0x1590` retains a six-bit region
identifier. `0x0dd8` accepts comparison 0..7, eight-bit stencil reference and mask,
with reserved and no-invalidate/force-match bits zero. It may be emitted by the
bounded MME interpreter. Invalid fields preserve the previous control state.

These are hierarchy optimization controls. The backend continues performing
complete supported fragment depth tests/writes; a hierarchy hint cannot enable
functional depth/stencil tests or change their comparisons. Functional stencil
enabling remains unsupported. No ZCULL auxiliary storage, statistics reports,
flush/sync operation or reconstruction is implied. Unsupported ZCULL sync and
report methods still fault. The optimization controls do not specialize shaders
or render pipelines.


### Retained instrumentation initialization

The exact source-verified header/data pair `49000000/49000001` (hexadecimal)
is retained per B197 context. HEADER resets the previously retained DATA; DATA
requires the admitted HEADER and can be supplied once. Different payloads,
DATA without HEADER, or additional DATA words remain unsupported. The fields
persist across ordinary channel submissions without sharing another channel's
state. They do not issue host work, create guest-visible profiling results or
specialize a render pipeline. This is initialization-field support, not general
instrumentation protocol execution.


### MME emissions reuse the supported B197 execution path

A SEND is routed to the existing strict method validator on its launching
subchannel. No method receives a less strict value, geometry, format or memory
check because it originated in MME. SET_OBJECT/channel methods below `0x100`,
MME RAM/entry writes `0x114..0x120`, and macro ports `0x3800..0x3ffc` remain
explicitly unsupported as emissions. Scratch writes are ordinary validated state.
Unknown B197 methods still fail; this does not add unspecified hardware methods.

Constant-buffer selectors and group binding, indexed/nonindexed draws, checked
constant writes and supported clears produce the same state/effects as direct
packets. A fixed-size pending-effect value retains its actual type. The consumer
releases guest command borrows, executes the backend operation, and resumes the
same invocation only afterward. A DATA word cannot acknowledge backend work.
Only completion of a WFI grants the separate privileged-initialization permission;
a completed memory write or draw is never an idle receipt. Descriptor-cache
invalidations update the same engine epochs inline and do not strand a pending
macro expecting an unnecessary backend callback.

The original total instruction budget, strict register readiness, parameter
ports and failure latching still apply. An error after earlier GPU work does not
roll back or replay that work. Two unchanged macro-driven draws can retain the
resident target until one explicit publication, without a per-draw readback or
wait; each draw still has its own host queue submission.


### MME invocation and submission work bounds

The invocation limit is 1024 instructions, including EXIT and delay slots; the
instruction-RAM capacity remains a separate 2048-word limit. A valid long loop
can therefore execute more instructions than an early 64-step implementation
budget without accepting unbounded execution. Each direct or GPFIFO submission
also has one 1,048,576-instruction MME budget shared by every invocation, entry,
parameter wait and backend resumption. An instruction reserves work when it
starts; completing a reserved parameter operation does not charge it again.
Missing parameters and repeated readiness polling do not invent work/completion.

The submission budget resets once at the validated API boundary, not on entry
changes, launches or yields. Faulted channels cannot use a new request to reset
their fault. B06F packet and B197 state still persist across legitimate requests.
The direct pushbuffer API also shares the existing 1,048,576-word input bound.
These are software resource limits, not hardware fault rules or physical RAM
claims. Exceeding either work bound preserves already executed writes/draws and
prevents command replay; completion/publication remains explicit.


The B197 vertex-stream substitute address (`0x0f84/0x0f88`) is retained as
independent upper-eight/lower-32-bit register state. Both halves must have been
written before the complete configured address is available. This address is
unused by supported in-bounds vertex fetches: configuring it does not make an
unmapped vertex stream or an out-of-range indexed/nonindexed draw valid.
Actual substitute fetches remain unsupported. Updating the address does not
invalidate otherwise compatible shaders or force a guest-memory read.


B197 inline uploads support a single pitch-linear row of 0..65536 bytes in
aligned four-byte words. The exact supported `LAUNCH_DMA` value is `0x1011`.
Launch snapshots the descriptor, checks the complete nonempty destination through
AddressMap/GuestMemoryWrite, and publishes earlier aliased pending output before
accepting data. Each DATA word is committed little-endian with an atomic checked
four-byte write followed by journal notification. Backend failure latches the
channel and preserves the successfully written prefix; no transfer-wide rollback
or automatic command replay is claimed. Empty launches read/write no guest bytes.

Inline launch/data effects release command borrows, resume MME only after actual
backend processing, and reacquire unread command suffixes. Ordinary DATA packets
can continue across GPFIFO submissions; explicit channel finish rejects incomplete
transfers. Configuration/relaunch while data is pending is explicitly unsupported.
No guest span or permanent whole-transfer shadow is retained. Flush-only writes
are already ordered canonical CPU writes in this implementation; they do not
synthesize a GPU fence or guest syncpoint, and add no unrelated queue submission,
GPU wait or framebuffer publication. Actual alias hazards use existing barriers.


`SET_BLEND_STATE_PER_TARGET` selects between the retained global coefficient
bank and the per-target bank for the active color target zero. The seven words
at `0x1e00 + j*32` retain independent state for `j=0..7`; bank selection does not
enable blending or increase the number of render attachments. Enabling blending
requires complete selected RGB state and explicit separate-alpha selection;
separate alpha additionally requires all three selected alpha fields. Other
banks never alias target zero, and invalid/reserved writes preserve prior state.
Pipeline identity uses resolved blend semantics, so equivalent global/per-target
and OGL/D3D encodings reuse compatible pipelines and shader modules.


B197 `SET_EDGE_FLAG` retains the literal one-bit edge mark in auxiliary channel
state. Supported draws require polygon FILL for both faces, where edge marking
has no coverage effect. Changing the bit therefore preserves filled geometry
and compatible pipelines. Unsupported polygon POINT/LINE modes remain rejected.


### Bounded MME shadow state

Explicit B197 `0x124` programs TRACK, TRACK_WITH_FILTER, PASSTHROUGH or REPLAY.
There is no assumed reset snapshot. TRACK captures only successfully validated
writes in a bounded pure-raster subset while applying live state. PASSTHROUGH
applies live state without changing captures. REPLAY applies a known captured
argument through the same ordinary method validator, ignoring the incoming
argument. This includes MME-emitted methods; it does not introduce a second
method interpreter or repeat an already executed draw.

Filtered mode executes supported live state and tags affected captures as
unverified. The hardware filter's capture eligibility is not guessed: replay
of filtered/unwritten words faults before live mutation. Unsupported replay
methods, including attachment addresses and draw actions, also fault explicitly.
Control methods below `0x200` and macro launch/data ports retain their existing
routing. To draw after replaying raster state, explicitly select a live mode.
No completion, guest-memory publication or queue submission is synthesized by
shadow mode changes. Faults retain accepted output for ordinary publication.

Shadow words use one lazily allocated bounded array, with no command-span or
resource borrow retained. Capturing a field again in unfiltered mode repairs
only that field's known snapshot; it does not invent unrelated history. Warm
replay reuses unchanged decoded shaders and compatible pipelines.

All sixteen A-stream addresses and inclusive bounds can be programmed without
aliasing stream zero. Unsupported inactive stream formats/frequencies and
attributes selecting other streams remain errors. Merely setting an unused,
unmapped stream address or limit does not read guest memory or supply geometry.

Initialization accepts bounded inactive point/stencil/sample controls, the
source-derived disabled tile-cache baseline and separate B1C0 configuration.
These fields do not enable unsupported topology, stencil, MSAA or compute
execution. Exact public masked privileged initialization requests still require
an actual preceding WFI; unknown requests cannot acknowledge completion.

Switch host `0x2c` dirty-flush/system-invalidate operations conservatively publish
pending attachments through the existing retryable publication transaction.
They work independently of engine/subchannel binding. Empty barriers do not
submit, wait or read back. Publication failure latches ordinary channel faults;
previously accepted output remains retryable without command replay.

B197 shader and whole texture-data NO_WFI invalidations release command borrows
and run as ordered backend effects, including when emitted by MME. Shader code,
constant words or texture bytes are reacquired on next use. Existing compatible
GPU resources remain reusable; these invalidations do not imply completion.

Nvmap GetId and FromId preserve the identity of an existing process-local
allocation. Import requires allocated user storage and increments a checked
reference count on the same handle; it does not pin memory or register backing
again. Unknown and closed-session IDs are rejected before output mutation.
IDs are session-local; cross-process allocation sharing is unsupported.
`Free` (`0xc0180105`) decrements one reference. Nonfinal release returns the
remaining count with NOT_FREED_YET set; final release removes the allocation
only after its backing has no mappings/pending GPU writes and its runtime pin
has been released. Failed unpin leaves the registration and handle valid for
retry. Size, count and flags are returned explicitly; stale handles reject
without mutating output. Final release while mapped is explicitly rejected;
automatic deferred destruction of mapped allocations remains unsupported.

Fixed `MapBufferEx` accepts a 64 KiB-rounded VA window whose canonical backing
ends within the last page. The entire window participates in reservation and
overlap checks, while only the actual page-aligned backing prefix is mapped.
The remaining padding cannot be fetched or published and faults on access.
This does not pin adjacent CPU memory, fabricate padding bytes or expose a
fragmented guest borrow. Larger excess, unsupported alignment and out-of-range
offsets still reject. Automatic mappings retain their exact backing extent.

### B06F semaphore subset

Channel methods `0x10/0x14/0x18` retain the 40-bit semaphore address and payload,
independently of engine/subchannel binding. All selectors must be initialized;
upper reserved bits and a non-word-aligned lower address fail explicitly.
RELEASE operation 2 supports four- and sixteen-byte reports, with WFI enabled
or disabled. `0x1c = 0x01000002` is the four-byte/WFI-enabled form. It validates the
mapped destination, observes actual host GPU completion, publishes prior resident
color/depth output, and only then writes the little-endian payload and journals
the canonical guest write. Publication can advance the journal, so write capacity
is checked again before the payload mutation. This is an explicit visibility
boundary, not an unconditional readback after every draw.

`0x1c = 1` admits an already-satisfied equality ACQUIRE. Pending output aliasing
the source is synchronized before its checked read. An unsatisfied condition
fails with the explicit channel-suspension limitation; it does not busy-wait,
fabricate a completed fence or replay an earlier draw. GEQ/AND/reduction and acquire-switch remain unsupported. This subset is not general Host1x syncpoint/PBDMA scheduling.

The GPFIFO executor drops its guest command borrow before semaphore effects and
reacquires an unread suffix afterward. A semaphore may therefore update command
storage without stale snapshot execution. A release retains one latest published
image for the existing driver diagnostic-frame boundary, moving its pixel vector
without another readback/copy. It cannot be taken by another channel, and an
accepted channel switch may supersede the old diagnostic image without changing
already-published guest RAM. This remains diagnostic output, not VI presentation.

Sixteen-byte reports store little-endian payload at +0, zero reserved word at +4
and a 64-bit timestamp at +8, in one checked guest write. The bounded virtual GPU
clock uses real monotonic host elapsed time and 614,400,000 ticks/s (384/625 ticks
per nanosecond), corroborated by the pinned reference; it is not the CPU's
19.2 MHz counter or a zero placeholder. It is not claimed to reproduce physical
GPU timestamp-query precision. WFI-disabled release reports front-end progress
without a host completion wait or unconditional framebuffer publication. Actual
backing overlap may still require the existing conservative alias barrier.
Completion queries skip host polling only when no host fence exists or its real
completion callback has already signalled; unresolved fences still poll normally.

B197 tile-cache barrier `0x0f7c=0` and ordinary flush `0x0f80=0` use the
existing conservative attachment publication boundary. They release command
borrows before publication, preserve accepted draws on failure, and reacquire the
unread command suffix afterward. Unsupported operands, including the alternative
flush value one, fault explicitly. Deferred cache-maintenance publication retains
one owning-channel diagnostic image for the next driver publication/capture call;
that call does not perform a second readback. This image is not a VI presentation
event or a guest synchronization fence.

B1C0 sampler/header-pool and local-memory address/size selectors have their own
channel-local compute configuration. Their fields are checked before mutation;
partial writes persist across submissions and engine rebinding. Configuration
does not fetch guest descriptors, map scratch memory, alter B197 bindings or
execute compute shaders. Unsupported launch methods continue to latch faults
while preserving previously submitted graphics work for explicit publication.

B197 graphics scratch address, size and per-warp selectors are retained separately
from B1C0 compute scratch state. Individual fields can be written before the
other halves and survive engine rebinding. They do not alter pipeline identity
for the supported shaders without local-memory accesses. Reserved field bits
fail before mutation; no unmapped scratch address is silently translated to a
host pointer or used to authorize an unsupported shader instruction.

Engine reports use separate initialized B197/B1C0 A/B/C selector state; they
cannot inherit a B06F payload or another engine's address. Supported actions
require a four-byte-aligned destination, a fully mapped four/16-byte range and
checked atomic publication. Invalid selectors/actions fail without a report
write. Completion/publication failure retains earlier GPU output and latches the
channel, avoiding whole-request replay. These ordered memory reports are not
Host1x syncpoint or guest event implementations.

Single-output `SET_CT_SELECT` validation checks its active fields rather than
requiring one full-register literal. Inactive TARGET1..7 mappings do not affect
attachment selection or host pipeline identity. Reserved bits, target count
other than one and an active TARGET0 remap are rejected before state mutation;
this does not enable multiple color attachments.

`ColorTarget::format` distinguishes RGBA8 UNORM and R11G11B10 unsigned-float.
Native target format participates in residency and pipeline identity; changing
format at identical dimensions/backing replaces incompatible target/pipeline
resources while preserving compatible translated shader modules. Both formats
use four-byte pitch/block-linear guest storage. GPU copies and checked atomic
publication operate on native packed bytes, including exact CPU byte patches
and preserved GOB padding. Conversion to the existing RGBA8 `Frame` occurs only
after successful guest writeback; failed publication keeps native pending output
retryable. HDR clear values admit nonnegative finite values above one.

R11G11B10 rendering requires the adapter's `RG11B10UFLOAT_RENDERABLE` feature.
Unsupported adapters reject the actual draw before pipeline creation rather than
substituting UNORM storage. The tested OpenGL backend lacks this feature; native
D3D12/Vulkan and WSL Vulkan support the tested HDR path.

Color FORMAT zero explicitly disables an attachment. Secondary FORMAT selectors
can be initialized without providing their addresses, dimensions or backing;
those targets remain inactive under the supported one-output routing. State is
retained per slot and channel. Rendering through a disabled active slot fails
before resource resolution or queue submission. Unsupported formats and active
MRT routing still fail explicitly, with earlier accepted GPU output preserved.

MME READ admits the nine validated color-target fields for each of eight targets,
besides the existing scratch bank. A bounded inline bank tracks successful B197
writes without allocating or retaining guest memory. Addresses in READ are method
word offsets; state writes use byte offsets. Rejected writes, other engine
classes, reserved holes and fields never initialized cannot contribute values.
The observation order includes macro-emitted state changes before execution
resumes, preserving live-state reads without relaxing shadow replay restrictions.
