# Controller support and applet storage

The hosted launcher supports foreground ControllerSupport (applet ID `0x0c`,
private mode 0) for one already-connected FullKey controller at Npad Player1.
It validates that controller against the actual HID shared-memory sample before
returning a selection. It does not synthesize a button press, pair a reader,
acknowledge an error dialog or emulate another library applet.

## IPC and ownership

`ILibraryAppletCreator` command 0 creates the supported controller accessor;
command 10 creates bounded owned storage. `IStorage` command 0 opens an accessor;
its commands 0, 10 and 11 report size, write and read checked byte ranges.
Negative offsets, overflow, unmapped buffers and allocation overruns fail before
storage mutation. Accessors retain their storage after the original object closes.
At most 128 objects and 8192 nonrecycled native object lifetimes are supported;
each storage is limited to 4096 bytes.

The controller accessor supports its state-changed event (0), Start (10),
GetResult (30), PushInData (100) and PopOutData (101). It also records the supported
suspension setting (50); preset GPU time slice zero (60) requires no GPU work in
this profile. Completion is signaled only after all arguments and the configured
controller have been validated. Output is consumed once into independently owned
storage. Closing inputs after PushInData cannot invalidate the applet's snapshot.

The kernel resolves CMIF input object IDs in the requesting domain, bounds the
list to eight and rejects absent identities. A scoped, typed callback context
carries these objects without changing the `Request`, `AudioHost` or kernel
structure layouts. Other hosted interfaces reject nonempty input-object lists.
The context is restored on success and error. AM object-ID allocation stays
separate from audio and filesystem IDs during scoped dispatch.

## Supported controller profile

Three input storages are required: 32-byte common arguments (version 1), a
20-byte private argument and the controller-support argument. Library-applet
versions 3/4/5 use the 540-byte four-player layout; versions 7/8 use the 1072-byte
eight-player layout. The supported request permits one player, FullKey style and
retaining existing connections. Requests requiring pairing, multiple players,
connection reset, firmware updates, strap guidance or key remapping fail explicitly.

The selected HID entry must expose FullKey style, a 17-entry ring with a valid
nonempty count/tail and the connected attribute on its current sample. A successful
result is 12 bytes: one player, Player1 ID zero and result zero. No UI is needed
when this already-connected profile satisfies the request. Live keyboard/gamepad
polling and general controller assignment remain separate limitations.

`ISelfController` command 9 returns a copied real readable event for the free
library-applet launch slot. This bounded profile completes synchronously when
Start validates; it has no concurrently active interactive applet. The accessor's
completion event is initially unsignaled and becomes ready only after validation.
Requests for an event after completion obtain the completed state as well.

`ISelfController` commands 62/63 set/get a process-owned idle-time detection
extension (`u32`: None=0, Extended=1, ExtendedUnsafe=2). Unknown values and
unexpected request descriptors fail before changing policy. Session and domain
dispatch share the same state. This hosted profile has no automatic idle
suspension; recording the policy does not implement a power-management timer.

## Evidence and validation

Wire definitions are pinned to libnx
[`feebd026`, applet.c](https://github.com/switchbrew/libnx/blob/feebd026ca0f5dcc2119f46ad8e0d16ad3dd4973/nx/source/services/applet.c),
[`hid_la.c`](https://github.com/switchbrew/libnx/blob/feebd026ca0f5dcc2119f46ad8e0d16ad3dd4973/nx/source/applets/hid_la.c),
`hid_la.h` and `libapplet.h` (ISC). The controller-support declaration describes
avoiding the UI when it is unnecessary. Implementation is independently written.

Literal IPC tests cover domain creation, returned object IDs, storage writes,
input objects, closing input references, state-change events, completion and exact
result bytes. Negative tests cover wrong object types, missing HID state, invalid
versions/modes/styles/player requirements, failed guest reads and storage bounds.
These tests establish the implemented service profile, not successful game frames.
