# Hosted audio initialization

Horizon-mode generated executables install a headless `audren:u` implementation.
It supports renderer discovery, work-buffer sizing, transfer-memory-backed opening,
configuration queries, start/stop, copied kernel event handles, device-name and
volume queries, and bounded silent renderer updates. It does not produce audible
output. Active voice playback, sample decoding, DSP effects, circular sinks and
splitters fail explicitly; this is not a complete audio renderer.

## Wire protocol and ownership

Requests use the existing HIPC/CMIF decoder and checked guest memory accessors.
No guest pointer is used as a host pointer. Open validates the 52-byte renderer
parameters, 72-byte request payload, supplied transfer-memory handle and size,
and copied current-process handle. The reported 64 KiB work buffer is an
implementation-owned initialization requirement, not Nintendo's DSP workspace
formula. Renderer state resides on the host and belongs to one process.

Renderer commands 0–3 query configuration, 4/10 perform supported updates,
5/6 start/stop, 7 returns a copied event handle, and 8/9 set/query the time limit.
Device commands expose one `AudioTvOutput` endpoint. Unknown device names and
unsupported commands are rejected. At most four renderer instances can be live;
instance identities are not reused, with a 4096-lifetime implementation bound.
Closing the last session or domain reference removes the renderer clock.

Silent updates validate section totals, memory-pool ranges, voice/sink/mix records
and output ranges before publication. AutoSelect B/C descriptors retain their slot
indices, including empty slots. Status and performance outputs must not overlap.
Pool attach/detach statuses are returned; voice results contain no consumed samples.
Inactive effect slots return wire `EffectState::Disabled` (4); this is distinct
from the server's internal usage-state enum. Enabled DSP effects are rejected.
Elapsed-render information is emitted from REV5, and larger effect results from
REV9. Parameter revisions REV1–REV15 are recognized subject to the supported
geometry and silent-update restrictions; this is not full compatibility with each
revision's features.

## Event timing

A running renderer advances on a 5 ms virtual-time period (240 samples at 48 kHz,
or 160 at 32 kHz). The scheduler polls its clock and includes the next deadline
when all guest threads are waiting. Start arms the clock and Stop disarms it.
Events are kernel events, not always-ready replacements. A wait observes the same
virtual-time scheduling contract as other runtime events; no host audio device or
wall-clock playback synchronization is provided.

## Launcher linkage and cache identity

[`src/audio.rs`](../../crates/wl-runtime/src/audio.rs) defines the runtime callback
contract; [`hosted/audio.rs`](../../crates/wl-runtime/hosted/audio.rs) contains the
concrete implementation included in generated launcher source. Installation must
precede process creation. Each process receives separate backend state. Reinstalling
the same static factory is harmless; replacing it with another factory is rejected.

The callback contract, kernel integration and scheduler remain in the hashed runtime
archive. Changing them invalidates native CPU-unit caches. Changing only the concrete
backend changes the launcher source and requires recompiling/linking that launcher,
without invalidating unchanged CPU units. This is a matched-toolchain Rust contract,
not a stable C ABI, dynamic-library interface or permission to mix incompatible Rust
metadata. Runtime archive identity checks remain mandatory.

For launcher diagnosis, `WL_THREAD_TRACE=1` prints saved thread IDs, priorities,
states, PCs, pending SVCs and wait reasons to stderr at most once every five host
seconds. Observation runs at the service-poll scheduler boundary and does not wake
threads, change guest state or count instruction growth as main-thread progress.
It is disabled by default. This avoids depending on private native memory offsets
when investigating a process whose worker threads run while its main thread waits.

## Sources and executed validation

Wire commands and structures follow [libnx audren.h](https://github.com/switchbrew/libnx/blob/feebd026ca0f5dcc2119f46ad8e0d16ad3dd4973/nx/include/switch/services/audren.h),
[audren.c](https://github.com/switchbrew/libnx/blob/feebd026ca0f5dcc2119f46ad8e0d16ad3dd4973/nx/source/services/audren.c)
and [auddev.c](https://github.com/switchbrew/libnx/blob/feebd026ca0f5dcc2119f46ad8e0d16ad3dd4973/nx/source/services/auddev.c)
(ISC). Active-only event signaling and revision features were checked against
Ryujinx revision `4930dced25c0d9ec33096b0f2601d262daa477bf`,
`AudioRenderSystem.SendCommands` and `BehaviourContext` (MIT). The implementation
is independently written Rust, not imported emulator source.

Effect response values were checked against
[`EffectState`](https://github.com/alula/Ryujinx/blob/4930dced25c0d9ec33096b0f2601d262daa477bf/src/Ryujinx.Audio/Renderer/Parameter/EffectState.cs)
and [`IEffectOutStatus`](https://github.com/alula/Ryujinx/blob/4930dced25c0d9ec33096b0f2601d262daa477bf/src/Ryujinx.Audio/Renderer/Parameter/IEffectOutStatus.cs)
at the same revision.

Runtime tests exercise real service discovery and CMIF dispatch, copied transfer
memory and event handles, timed signaling, close/clone lifetime, malformed requests,
silent update bytes and preflight failure without partial output. The explicit
`wl-cli --test audio -- --ignored` test builds the public synthetic
[`audio.c`](../../tests/horizon/audio.c) client with devkitA64/libnx, recompiles its
AArch64 executable and checks two real guest event waits. It requires that toolchain;
the default test run does not count it as executed. No commercial code is included.
