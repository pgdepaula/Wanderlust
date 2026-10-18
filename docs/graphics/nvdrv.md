# Process-local NVIDIA device adapter

`wl-nvdrv::Driver` adapts a bounded set of device requests to the existing
`wl-gpu::driver_backend::GpuDriver`. It owns file descriptors, nvmap allocation
handles and address-space/channel associations. It does not decode Maxwell
commands or implement Horizon kernel handles.

`RUNTIME_ADAPTER` contains the small module compiled against the exact
`wl_guest_runtime` identity used by a generated executable. Its `NativeHost`
implements the runtime's installed NVIDIA service interface. The integration
test compiles that module against the workspace runtime and enters through
literal HIPC/CMIF requests. The default AOT launcher does not automatically link
or install it; target-correct native dependency composition remains necessary.

## Wire subset

Scalar fields are decoded explicitly in little-endian order. The ioctl's
direction and encoded size determine the exact primary input/output lengths;
combined metadata is limited to 64 KiB. Unsupported requests return an explicit
NVIDIA error. The device session supports 32 live files, 64 allocations, one
nvmap node and up to 16 channel identities over its lifetime.

| Device | Request | Implemented restriction |
| --- | --- | --- |
| `/dev/nvmap` | Create `c0080101` | Nonzero size aligned to 4 KiB; unique handle |
| | Alloc `c0200104` | User-backed, flags 1, alignment 4096, kind 0, heap mask 0 |
| `/dev/nvhost-as-gpu` | InitializeEx `40284109` | Zero/default payload |
| | BindChannel `40044101` | Initialized space and unbound channel |
| | AllocSpace `c0184102` | Fixed address, 4 KiB pages, flags 1 |
| | MapBufferEx `c0284106` | Complete reservation, checked allocation subrange, flags 1/5, kind 0/default |
| | UnmapBuffer `c0084105` | Exact mapped reservation base |
| | FreeSpace `c0104103` | Exact unmapped reservation |
| `/dev/nvhost-gpu` | SetNvmapFd `40044801` | Live nvmap descriptor, one binding |
| | AllocGpfifo `40084805` | 1–2048 entries, flags 0 |
| | AllocObjCtx `c0104809` | B197, flags 0 |
| | KickoffPb `c018481b` | Ioctl2, hardware-format entries, flags 4 or 6 |

This subset follows the ISC-licensed libnx revision
[`feebd026ca0f5dcc2119f46ad8e0d16ad3dd4973`](https://github.com/switchbrew/libnx/tree/feebd026ca0f5dcc2119f46ad8e0d16ad3dd4973/nx/source/nvidia/ioctl),
including `nvmap.c`, `nvhost-as-gpu.c` and `nvchannel.c`. The entry parser and
command acquisition remain the existing GPFIFO implementation. No external
service-emulator implementation is vendored.

The fence-increment interpretation is corroborated by the MIT-licensed archived
Ryujinx revision
[`44862dce3ef307b12366376ada3fa0db693bfaf9`, NvHostChannelDeviceFile](https://git.axenov.dev/Museum/ryujinx/src/commit/44862dce3ef307b12366376ada3fa0db693bfaf9/src/Ryujinx.HLE/HOS/Services/Nv/NvDrvServices/NvHostChannel/NvHostChannelDeviceFile.cs).
Bit 1 requests two increments; bit 2 selects hardware entries. Only these
combinations are supported. Wait, increment-with-value, suppression of WFI,
AllocGpfifoEx2, control-device events, arbitrary syncpoint operations and the
general Host1x scheduler are unsupported.

## Ownership, ordering and failures

The runtime adapter pins each allocated backing through `Memory::pin_gpu_range`.
No guest slice is retained. GPU mappings use the existing checked canonical
backing controller. A bound channel prevents closing its nvmap descriptor even
before any GPU mapping exists. Mappings and reservations must be removed, and
channels closed, before releasing the backing/pin.

Execution is deliberately synchronous. Before each accepted request, all
registered allocations receive conservative full-range CPU-write notifications.
The existing driver then consumes GPFIFO entries, publishes pending output,
waits for actual host completion and retires the token. Ordinary uninstrumented
CPU writes are allowed only between these exclusive calls. This mode avoids
pretending that generated CPU stores already maintain a fine-grained GPU journal.

Only after successful completion/publication does bit 1 advance the channel's
owned threshold by two. A non-incrementing request returns its existing threshold;
it does not manufacture a new fence. Output flags are cleared. These bounded
process-local identities have no kernel event/wait implementation.

A consumed prefix can have real GPU side effects before a later method faults.
That prefix is completed and published, then the request fails fatally; the
runtime latches the backend fault and stops guest execution. No output metadata
is published and no automatic replay occurs. Publication/device failures also
halt the backend instead of allowing guest CPU writes over GPU-newer data.

## Executed validation

`tests/runtime_gpu.rs` uses literal service messages, real allocations/pins,
fixed mappings, an indexed asymmetric triangle and independently assembled
Maxwell shaders. Its oracle computes coverage and UVs separately in double
precision and checks both covered and uncovered pixels. It also verifies a CPU
texture mutation without new shader modules/pipelines, teardown, the bound-nvmap
lifetime guard, and a draw followed by an unsupported method without replay.

Both scenarios passed on WSL OpenGL/llvmpipe, native Windows D3D12 and native
Windows Vulkan. The two-draw mutation workload measured four host submissions,
two readbacks and two synchronization points. Conservative notifications covered
196,608 backing bytes. It read 668 command bytes through borrowed spans, uploaded
144 vertex bytes and 16 index bytes, and preserved the shader/pipeline creation
counts on the second draw. Metadata copies and GPU transfers remain real costs;
this is not an asynchronous or zero-copy service path.

These tests construct messages on the host using the actual runtime dispatcher.
They do not prove an AArch64 homebrew NVIDIA submission, original-game command
execution, VI presentation or a rendered Tears of the Kingdom frame.
