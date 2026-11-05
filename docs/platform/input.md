# Initial HID controller snapshot

`hid` / `IAppletResource.GetSharedMemoryHandle` creates a read-only 256 KiB
shared-memory object containing one initial sample of the configured controller
state. The sample is populated before a guest can map the object. Opening another
handle reuses that object without resetting its sequence or inventing input.
The default state has no buttons pressed and neutral sticks.

The existing bounded input profile exposes Player 1 and Handheld controller views
with FullKey, Handheld and JoyDual ring data. Each common ring has 17 atomic
storage records of 48 bytes, following the 32-byte common LIFO header. Buffer
capacity is distinct from the count of actual written samples. Tail advances on
each write; count saturates at 17 independently of sampling-number discontinuity
or rollover. Scalars are serialized explicitly in little-endian order.

These layouts follow ISC-licensed libnx revision
[`feebd026ca0f5dcc2119f46ad8e0d16ad3dd4973`](https://github.com/switchbrew/libnx/blob/feebd026ca0f5dcc2119f46ad8e0d16ad3dd4973/nx/include/switch/services/hid.h),
`HidCommonLifoHeader`, `HidNpadCommonStateAtomicStorage`, `HidNpadCommonLifo` and
`HidNpadInternalState`.

This is an initial snapshot, not complete HID emulation. Generic shared-memory
mapping currently copies storage; later source updates are not automatically
propagated to already mapped guest aliases. Native GPU window input is not yet
connected to a complete ongoing HID stream. Activation/style-policy filtering,
motion sensors and additional controller types remain outside this profile.

`tests/input_display.rs` verifies exact ring positions/counts, repeated wrap and
untouched bytes on invalid controller/short-storage requests. `tests/ipc.rs`
requests the actual HID service/resource, maps the returned handle through SVC,
checks literal header/sample offsets and verifies guest writes are rejected.


The focused input/display and IPC suites passed 32 tests. Strict all-target
runtime Clippy and formatting passed. The same literal IPC/mapping assertion
also passed against the frozen runtime used by the local AOT executable through
a checked service adapter. That compatibility run does not establish live input
streaming or original-game visual output.
