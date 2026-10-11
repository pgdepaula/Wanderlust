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
and a Maxwell shader decoder/lowering path. Naga validates the controlled WGSL
shader; it cannot decode Maxwell machine instructions. See [sources](sources.md)
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
Original shader handles and constant buffers are still unimplemented. The tested
bridge runs literal FIFO pool methods, descriptor resolution, guest texture reads,
conversion and real sampled draw/readback. The last draw is the existing synthetic
execution command with fixed WGSL, not a decoded Maxwell draw or shader.
