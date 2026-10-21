# Process-local NVIDIA device adapter

`wl-nvdrv::Driver` adapts a bounded set of device requests to the existing
`wl-gpu::driver_backend::GpuDriver`. It owns file descriptors, nvmap allocation
handles and address-space/channel associations. It does not decode Maxwell
commands or implement Horizon kernel handles.

`RUNTIME_ADAPTER` contains the small module compiled against the exact
`wl_guest_runtime` identity used by a generated executable. Its `NativeHost`
implements the runtime's installed NVIDIA service interface. The integration
test compiles that module against the workspace runtime and enters through
literal HIPC/CMIF requests. `recompile --horizon --native-gpu` now also emits it
into the final AOT executable and installs the backend before guest execution.
The CPU-only launcher remains available without this explicit option.

The runtime Initialize boundary validates the entire requested transfer-memory
range for read and write access. Adjacent accessible guest regions are accepted;
internal host allocation boundaries do not imply a guest mapping fault. A hole
or inaccessible region rejects initialization before the host callback. This is
scoped validation, not a kernel pin or a retained borrow of the transfer buffer.

Command 13 (`SetGraphicsFirmwareMemoryMarginEnabled`, historically called
`FinishInitialize`) accepts the bounded scalar values 0 and 1 after initialization
and returns no payload. The service records the request while keeping its effective
margin disabled, following the documented default of
`nv!nv_graphics_firmware_memory_margin`. Non-default firmware-margin policy and
firmware allocation are unsupported. This command neither submits GPU work nor
signals completion. Its wire contract follows
[Switchbrew NV services, revision 14790](https://switchbrew.org/w/index.php?title=NV_services&oldid=14790#SetGraphicsFirmwareMemoryMarginEnabled);
the disabled configuration follows
[System Settings, revision 14990](https://switchbrew.org/w/index.php?title=System_Settings&oldid=14990#nv).

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
| `/dev/nvhost-as-gpu` | InitializeEx `40284109` | Flags 0/1, selected 64/128 KiB large pages, default or checked explicit VA bounds |
| | GetVARegions `c0404108` | Explicitly configured lower/upper regions; Ioctl and Ioctl3, exact 48-byte region output |
| | BindChannel `40044101` | Initialized space and unbound channel |
| | AllocSpace `c0184102` | Fixed address, 4 KiB or configured large pages, flags 1; explicit region bounds |
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

Those two tests construct messages on the host using the actual dispatcher.
The additional `tests/horizon/nvidia.c` fixture instead executes real AArch64
libnx code. It initializes the service, opens the three supported nodes, pins
and maps allocation subranges, binds B197 and submits two indexed draws. The
guest checks inside/outside pixels and thresholds 2/4, unmaps its resources and
closes its handles. No host creates its ioctl messages or invokes its draw.

## Target-correct AOT composition

The optional GPU build invokes Cargo for the exact host/requested target and
selects dependencies from `compiler-artifact` messages. Target rlibs, host
proc-macro libraries and explicit native import/static archives are copied into
a content-keyed snapshot. The compiler identity, target, lockfile, adapter text
and every artifact digest determine its key. Native framework search paths
remain unsupported. No newest-file or directory-glob artifact selection is used.

An owned digest manifest is checked before final linking and before atomic
executable publication. CPU units still reference only `wl_guest_runtime`, so
ordinary GPU dependency updates do not invalidate their cache. A dedicated
Cargo target directory avoids taking the outer test runner's build-directory
lock recursively. The source workspace and native toolchain are trusted build
inputs; this does not sandbox compilation or make artifact snapshots immutable
against arbitrary host filesystem writes.

The homebrew integration executes both cold and warm generated binaries and
requires unchanged artifact identity and zero runtime/unit cache misses on the
warm build. It does not establish original-game command execution, VI
presentation or a rendered Tears of the Kingdom frame.


## Diagnostic window

Generated NVIDIA executables can display their explicitly published framebuffer
with `WL_GPU_DEBUG_WINDOW=1`. `WL_GPU_DEBUG_HOLD=1` keeps the last image visible
after guest termination until the window is closed. Repainting that image does
not execute guest instructions or produce additional GPU frames. The window is
labeled as a guest GPU diagnostic, not Tears of the Kingdom. This path is not VI
or BufferQueue presentation and uses the existing synchronous CPU readback.

`Driver::take_frame` transfers ownership of the latest successfully published
color frame. Taking it twice returns no second image. No guest memory borrow is
retained and presentation adds no GPU readback. Failure to open a requested
window is an error; presentation failure after accepted work is fatal rather
than a retryable ioctl failure.

On Windows, place the generated executable and the official x64 SDL2 runtime
`SDL2.dll` in a native Windows directory. The loader resolves an absolute DLL
path beside the executable, or the explicit `WL_SDL2_LIBRARY` path, and uses
restricted Windows library search flags. It does not search the working
directory or arbitrary PATH entries. Loading from a WSL network path may fail;
copy both files to NTFS in that case. SDL binaries are external dependencies
and are not included in this repository.

Native Windows D3D12 and Vulkan runs executed the source-built AArch64 NVIDIA fixture's
two indexed draws, verified both framebuffer results in the guest and presented
the framebuffer in the SDL window. A separately held D3D12 window was captured
and visually inspected. The result is synthetic homebrew
execution, not original-game graphics.

## Bounded diagnostic capture

`WL_GPU_CAPTURE_DIR` enables raw RGBA capture of successfully published color
outputs. Each invocation creates a unique directory. `WL_GPU_CAPTURE_LIMIT`
selects 1–64 images (default 8); the total raw pixel budget is 1 GiB. A capture
has a `.rgba` file and JSON metadata containing its dimensions and cumulative
draw count. Metadata is renamed last as the completion marker; incomplete files
are not captures. Host filesystem failures are fatal after accepted work and
must not cause command replay. Captures may contain private guest content and
should remain outside tracked source directories.

Capture borrows the existing owned readback image, writes it to disk and adds
no GPU submission, readback or wait. These outputs are explicit color-target
publications, **not VI presentation events**. Capturing or repainting one image
multiple times does not establish multiple game frames.

`WL_NVDRV_TRACE=1` logs at most 1024 device-open/ioctl events, including request
identities and results. Unknown device paths are redacted. Buffer contents,
shader bytes and allocation contents are not logged.
