# Graphics execution and memory boundaries

`wl-gpu` executes checked, borrowed graphics commands through wgpu. The headless
executor allocates vertex/index buffers and an RGBA8 target, renders indexed
triangles with a WGSL shader, waits for submission completion, and returns packed
RGBA bytes. It has executed on Windows D3D12 and Vulkan and WSL software GL.
The input is synthetic. This is not a Tears of the Kingdom frame.

## Source semantics and execution commands

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
Cross-mapping gathers, map removal, permissions and backing versions are absent.

`GuestMemory::read` leaves permissions, storage and fault policy to its implementation.
`SliceMemory` is a bounded test adapter; it does not implement Horizon memory or
permission handling. No production runtime adapter is connected. Address resolution
never treats a guest pointer as a host pointer. Aliases resolve to the same bytes;
resource-cache invalidation across aliases is not implemented.

`DirtyRanges` coalesces adjacent/overlapping intervals and preserves gaps. It can
represent canonical backing offsets, independent of GPU aliases. The current test
uses explicitly selected dirty buffer offsets; automatic CPU-write tracking and
guest-visible GPU writes are absent. The caller must acknowledge dirtiness only
after successful transfer, not when merely planning it.

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
height reduction and successive padded mip sizes. Arrays, depth/stencil and
multisampling are unsupported.

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
| `0x0000` | Bind class B197, engine instance zero |
| `0x0100` | Documented no-operation |
| `0x0f10` | Select modern Maxwell texture headers, value one |
| `0x155c`, `0x1560`, `0x1564` | Sampler pool upper/lower address and inclusive maximum index |
| `0x1574`, `0x1578`, `0x157c` | Image header pool upper/lower address and inclusive maximum index |
| `0x1424`, `0x1428` | NO_WFI sampler/header cache invalidation events, all or tagged line |

Method writes and faults allocate no heap storage. Unsupported classes, unbound
subchannels, invalid field bits and unimplemented methods fail explicitly; the
caller must stop a channel on fault. WFI invalidations, semaphores, render targets,
draw registers, MME macros and shader program state are not executed. NO_WFI cache
events advance descriptor epochs without asserting completion or guest memory writes.

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
accepts exactly generic-zero float4/generic-one float2 input, position xyzw and
generic-zero xy output. ALD B32/B64/B96/B128 accepts direct aligned attributes
within those input vectors, RZ addressing, no output/per-patch/indirect modifiers,
and the existing conservative barrier-zero/wait-one subset. AST accepts direct
position/generic-zero output groups from initialized, ready GPRs. All six output
components must have been written before EXIT. Other vertex instructions, including
vertex resources, built-ins and control flow, are unsupported.

The translated vertex entry exports actual decoded clip-space position and
screen-linear UV to the existing translated fragment stage. Typed stage wrappers
prevent reuse of fragment header acceptance for vertex binaries. Pipelines pair
vertex/fragment semantics, float4/float2 formats, stride and offsets; changing
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
Z scale one/offset zero, upper-left origin, zero window offsets, half-integer pixel
centers and zero-to-one depth clipping. Wgpu viewport/scissor are derived from
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

## Global additive blending and channel writes

B197 target-zero blend enable is 0x1360. Global blend selection remains explicit
(`SET_BLEND_STATE_PER_TARGET` at 0x12e4 must be zero). When enabled,
SET_BLEND_SEPARATE_FOR_ALPHA (0x133c) must be specified; color operation/source/
destination are 0x1340/0x1344/0x1348, and alpha operation/source/destination are
0x134c/0x1350/0x1358. Separate alpha disabled reuses the color equation.

The implemented operation is ADD (OGL 0x8006, D3D 1); factors are ZERO
(0x4000/1), ONE (0x4001/2), SRC_ALPHA (0x4302/5), ONE_MINUS_SRC_ALPHA
(0x4303/6). This supports conventional source-over, premultiplied-over and other
additive combinations of those four factors, on the existing RGBA8 UNORM target.
Other documented operations/factors, per-target equations, constants and dual
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
| Entry1 LEVEL | 9 | MAIN = 0 |
| Entry1 LENGTH | 30:10 | nonzero count of 32-bit words |
| Entry1 SYNC | 31 | PROCEED = 0 |

Address plus length must fit the 40-bit address space, allowing an exclusive end
exactly at 2^40. The wire field permits 2,097,151 words. The execution policy caps
one call at 2,048 entries and 1,048,576 aggregate words (4 MiB), independently of
hardware limits. Empty arrays are successful no-ops on healthy channels.
Zero-length entries have control/opcode semantics: even the documented NOP opcode
is currently rejected, rather than treating an arbitrary control operand as an
address or borrowing zero bytes. Conditional fetch, kernel privilege, subroutine
level, WAIT and reserved bit 1 are explicitly rejected. WAIT is not implemented
by calling a CPU sleep or interpreting it as a complete host fence.

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
WAIT/no-prefetch behavior and hardware synchronization remain unsupported.

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
full-reservation mappings of aligned backing subregions. It uses AddressMap,
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
other depth formats, depth bounds/bias and alternate depth transforms remain
unsupported. The implemented viewport has zero-to-one clip depth, scale Z one
and offset Z zero. All eight comparisons and depth-write enable are supported;
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
are required. Immediate/constant operand variants, saturation, negation/inversion,
additional predicate results, extended carry and PASS_B remain unsupported.

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
`0x10f8=0` and full surface clip `0x0ff4/0x0ff8`. Selected color/depth descriptors
and corresponding target selection must be complete. Color supports the same
pitch/block-linear RGBA8 layouts; depth uses block-linear ZF32. Normalized finite
clear values are supported, with the existing ZF32 zero/normal restrictions.
Stencil, partial component masks, additional targets/layers, partial clips and
optional scissor/viewport/rectangle/stencil-mask controls remain unsupported.

Clears resolve raw attachment geometry independently of draw state. They require
no guest shader, vertex/index buffer, viewport transform, blend coefficients or
depth comparison/write enable. Draw color write masks and blending do not affect
hardware clears; explicit clear control zero also excludes draw scissors/viewport
clipping. Surface clipping still applies, hence the full-clip validation.
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
