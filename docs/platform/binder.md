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
