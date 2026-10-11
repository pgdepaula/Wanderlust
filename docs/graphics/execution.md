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

Buffers persist by ID until removed. The target, view and readback allocation persist
until the extent changes. One pipeline and optional timestamp-query allocation
persist for the executor lifetime. Descriptor hashing, texture/bind-group caches,
asynchronous eviction and staging reuse are not implemented. GPU validation and
allocation errors are captured in error scopes; a backend failure prevents reuse
of potentially inconsistent executor state. Ordinary preflight errors permit retry.

## Formats and capabilities

Rendering/readback implements `Rgba8Unorm` only. `Format` describes block geometry
for RGBA8 linear/sRGB, BC1 RGBA, BC3 RGBA and ASTC 4×4; geometry is not codec or
backend sampling support. `BlockLinear2d` uses `tegra_swizzle` for one 2D mip with
GOB height 1, 2, 4, 8, 16 or 32, and a 64 MiB conversion budget. Compressed input
uses block coordinates. It preserves encoded block bytes and does not decompress
ASTC or BCn. Mip chains, arrays, depth/stencil, multisampling and sRGB execution
are unsupported by the executor.

Adapter information, all supported features, enabled features, adapter limits and
requested device limits are exposed. Only timestamp queries are optionally enabled;
compression features are reported, not implicitly required. Device limits use
conservative downlevel defaults with the adapter's resolution limits. The baseline
does not require descriptor indexing, push constants or pipeline-cache extensions.

`fifo::Decoder` separately decodes Maxwell B06F pushbuffer method packets into
subchannel, byte method offset and value. It handles incrementing, nonincrementing,
increment-once, immediate and exact-zero NOP packets, preserves packet state across
input chunks, rejects unsupported opcodes and method-address overflow, and detects
truncation at a logical stream end. It retains no source borrows or heap storage.
The caller supplies little-endian decoded words and stops the channel on a fault.
This decoder does not execute engine registers or translate method writes into
the host command stream. Legacy/tertiary packets and end-segment control are rejected.

Authentic game rendering additionally requires NV service/channel setup, GPFIFO
entry fetching and engine state, resource descriptors, synchronization semantics
and a Maxwell shader decoder/lowering path. Naga validates the controlled WGSL
shader; it cannot decode Maxwell machine instructions. See [sources](sources.md)
and [executed validation](validation.md).
