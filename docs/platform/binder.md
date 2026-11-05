# Hosted graphic buffer producer

The generated native host provides a bounded `android.gui.IGraphicBufferProducer`
service through the runtime's existing typed service extension. It handles the
single Binder producer ID 32 returned by the native-window service. Direct HIPC
sessions are intercepted for producer transactions; reference-count and native
handle requests retain their existing runtime handlers. Domain producer sessions
are outside this hosted adapter's supported scope.

## Wire format and ownership

Requests and replies use explicit little-endian Parcel headers and scalar fields.
The adapter validates the UTF-16 interface token, exact parcel extent, flattened
object lengths and absence of file descriptors. Input parcels are bounded to
16 KiB. Preallocated GBFR descriptions retain their owned original metadata;
RGBA8/BGRA8, dimensions up to 8192 and a 64 MiB storage bound are accepted.
Opaque native allocation metadata is retained, not interpreted as a host pointer.

Supported transactions are Connect (10), Query (9), SetPreallocatedBuffer (14),
SetBufferCount (2), DequeueBuffer (3), RequestBuffer (1), CancelBuffer (8),
QueueBuffer (7) and Disconnect (11). Replies have transaction-specific layouts:
a queried format is not a buffer slot, and a slot is not an image width.
Only preallocated buffers are supported; there is no implicit allocator.
There are at most 32 slots, with at least one slot reserved from simultaneous
producer dequeue ownership. Invalid, disconnected and temporarily unavailable
operations return distinct status values where the transaction defines one.

A successful dequeue returns a non-null flattened 36-byte NvMultiFence, including
for a fresh slot with zero fences. A zero fence count denotes absence of pending
work; it does not signal any GPU work as complete. Cancellation retains an actual
nonempty fence byte-for-byte for subsequent acquisition. Queue input retains
its acquire fences and native buffer metadata. Replies are checked and written
before slot state is committed, so a failed output write cannot consume a slot.

## Consumer and visibility limits

Slots transition from free to dequeued to queued. Queue acceptance does not
release the slot, publish GPU memory, increment a presented-frame count or prove
GPU completion. An explicit trusted consumer can inspect the oldest owned queue
record and acknowledge its sequence after actual consumption and required fence
handling. Release must be ordered. A producer IPC transaction can trigger the installed
trusted consumer, but cannot bypass its fence/backing validation.
The native availability event reflects free preallocated slot availability, not
a GPU completion fence.

Native-GPU executables install a consumer connected to the same NVIDIA driver.
After a successful producer transaction it examines the oldest queued buffer,
checks real completion thresholds and copies the canonical native surface into
an owned display image before release. A future fence leaves the slot queued;
a later transaction retries it. Frame counts advance only after acquisition.
Pitch-linear and Tegra block-linear RGBA8 native surfaces are supported with
checked nvmap ID/offset/size resolution and padding removal.

General BufferQueue allocation, timed vblank, arbitrary crop/scaling/rotation,
compressed surfaces and multi-plane formats remain unsupported. Queue acceptance
alone is not presentation. Host GPU completion and guest RAM publication remain
separate boundaries; unpublished GPU output cannot be read as a display image.

## Evidence and validation

The wire definitions were checked against libnx revision
[`feebd026ca0f5dcc2119f46ad8e0d16ad3dd4973`](https://github.com/switchbrew/libnx/tree/feebd026ca0f5dcc2119f46ad8e0d16ad3dd4973),
particularly `nx/source/display/buffer_producer.c`, `parcel.c` and
`nx/include/switch/display/buffer_producer.h`. Android's
[`IGraphicBufferProducer.cpp`](https://android.googlesource.com/platform/frameworks/native/+/29a3e90879fd96404c971e7187cd0e05927bbce0/libs/gui/IGraphicBufferProducer.cpp)
was used as a semantic reference for nullable flattened Fence objects. No source
implementation was imported.

`wl-runtime/tests/binder_hosted.rs` checks distinct replies, exact flattened buffer
metadata, slot bounds, output-write failure atomicity, retained fences, ordered
consumer release and malformed buffer descriptors. The runtime audio integration
suite also submits authentic HIPC transactions through SVC, checks the original
session endpoint is restored and verifies different reply extents.


## Virtual display pacing

`BqBufferInput.swapInterval` at byte 44 controls consumption by the configured
60 Hz virtual display. The bounded profile accepts intervals 0..4; this limit
is an implementation restriction, not a claimed NVIDIA maximum. Zero permits
immediate acquisition. Positive intervals retain queue ownership until their
integer refresh deadline. Deadlines use rational `tick * 1e9 / 60` rounded up,
without accumulating a rounded frame-period error. A late submission targets a
future refresh rather than replaying missed refreshes in a burst.

The hosted callback poll integrates this deadline with the existing scheduler's
timer interface. It does not sleep in a producer request, busy-wait on host time,
submit another GPU command or invent fence completion. At a due deadline an
unresolved GPU fence leaves the buffer queued and disarms that expired timer;
subsequent actual work/polls can retry without a zero-time idle loop. Consumption
still requires canonical visible memory and independently satisfied acquire
fences. Disconnect resets the presentation timeline.

This profile supports automatic timestamps and the zero/as-soon-as-possible
explicit timestamp only. Nonzero explicit timestamps and intervals outside the
bounded range reject queueing without losing the dequeued slot. It is not native
vblank, full VI composition or a hardware-equivalent Nintendo frame scheduler.
The wire location and forwarding follow pinned libnx `buffer_producer.h` and
`display/native_window.c` (`nwindowQueueBuffer`).

The real GPU consumer test uses interval three: no pixels are acquired before
50,000,000 virtual nanoseconds, and reaching that time cannot complete a future
GPU fence. A later GPU draw completes that fence. A second block-linear surface
stays queued at 99,999,999 ns and is consumed at 100,000,000 ns, with independently
checked pixels and padding. CPU tests verify invalid timing metadata preserves
slot ownership and output status.


This pacing profile passed seven Binder CPU tests and the hosted audio/service
regressions (87 CPU passes in the combined run; two optional build tests were
ignored). The NVIDIA runtime fixture passed nine actual GPU cases each on WSL
OpenGL, WSL Vulkan, native Windows D3D12 and native Windows Vulkan, including
both presentation deadlines and independently specified guest framebuffer pixels.
Four non-device NVIDIA adapter tests also passed. Formatting and strict
all-target Clippy passed for the runtime and NVIDIA packages. These synthetic
service tests do not establish a recognizable original-game scene.
