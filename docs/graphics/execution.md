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
little-endian RGBA bytes), pitch layout (`0x1000`), third dimension one, zero array
pitch/layer and a 256-byte-aligned 40-bit address. The address alignment is a
supported-subset restriction, not a universal hardware alignment claim. In pitch
layout **WIDTH is byte pitch**. It must be positive and divisible by four; storage
width is pitch/4. Logical visible bounds come from surface/viewport clipping and
scissor. Padding pixels remain initialized guest data and are preserved.
Block-linear, layered, depth, compressed and multisampled render targets are not
supported, independently of sampled-texture layout support.

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
color masks and global blending additionally accept the validated states below:
unconditional rendering, rasterization enabled, one color attachment, single sample,
RGBA baseline writes, fill mode, no depth/stencil/cull/alpha test/logic op/sample mask,
no polygon smoothing/offset/stipple, and the supported viewport controls. Missing
state or unsupported values fail. No undocumented power-on values are substituted.
No instancing, additional shader stages, perspective IPA, arbitrary
formats or guest semaphore operations are implemented by this slice.

## Framebuffer visibility and ownership

The optional `GuestMemoryWrite` boundary extends checked read access with whole-span
write validation and an atomic all-or-nothing write. `SliceMemoryMut` implements
that independent test contract. It is not a runtime memory implementation. The
adapter must keep backing identity, permissions and contents stable during the
exclusive call; no guest borrow outlives queue upload or spans GPU completion.

The existing bounded offscreen target gains COPY_DST. First use/rebinding/content
changes initialize it from guest bytes through `Queue::write_texture`, using byte
pitch without the buffer-copy 256-byte row restriction. The render pass uses Load,
so uncovered pixels are retained. Readback uses the existing 256-byte-aligned buffer
copy, submission completion wait and row packing. After completion one checked
whole-span guest write publishes the framebuffer and updates the shared generation
journal. Host resource handles remain owned by wgpu across submissions; no explicit
destroy or sleep is used. Allocation/upload validation errors poison the backend.

This is an explicitly synchronous validation visibility contract, not a hardware
fence/semaphore implementation. Every draw still waits and reads back the entire
storage target. Asynchronous GPU/CPU visibility and presentation require a separate
runtime service contract. The latest target cache keys canonical guest backing,
size, dimensions and journal version, not GPU VA alone. An alias can reuse contents;
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
Standalone harness execution is rejected while guest output is pending.

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
texels. A changed epoch or evicted patch history fails before writeback; the caller
must synchronize before such a transition. Failure of an atomic guest write leaves
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
