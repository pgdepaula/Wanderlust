# Hosted audio initialization

Horizon-mode generated executables install a headless `audren:u` implementation.
It supports renderer discovery, work-buffer sizing, transfer-memory-backed opening,
configuration queries, start/stop, copied kernel event handles, device-name and
volume queries, and bounded silent renderer updates. It does not produce audible
output. Active voice playback, sample decoding, enabled DSP effects and circular
sinks fail explicitly; this is not a complete audio renderer.

## Wire protocol and ownership

Requests use the existing HIPC/CMIF decoder and checked guest memory accessors.
No guest pointer is used as a host pointer. Open validates the 52-byte renderer
parameters, 72-byte request payload, supplied transfer-memory handle and size,
and copied current-process handle. The reported 64 KiB work buffer is an
implementation-owned initialization requirement, not Nintendo's DSP workspace
formula. Renderer state resides on the host and belongs to one process.

Renderer commands 0–3 query configuration, 4/10 perform supported updates,
5/6 start/stop, 7 returns a copied event handle, and 8/9 set/query the time limit.
Device commands expose one `AudioTvOutput` endpoint. Gain writes for an unknown
well-formed device name succeed without changing the advertised device; its gain
query returns zero. Unterminated or malformed name buffers and unsupported commands
are rejected. At most four renderer instances can be live;
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

REV5–REV11 splitter descriptors are accepted for silent/stopped-voice updates.
The bounded implementation permits 1024 splitter states and 8192 destinations;
these are implementation budgets, not hardware maxima. It validates SNDH/SNDI/SNDD
signatures, variable destination lists, cyclic lists, mix IDs, finite volumes and
the 112-byte destination format. Twelve unused bytes follow each ID list; unused
fields and final alignment padding need not be zero. Newer filtered destination
formats remain unsupported. No audio samples are routed: an update with an active
voice or enabled DSP effect still fails before publication.

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

Splitter records follow
[`SplitterInParameterHeader`](https://github.com/alula/Ryujinx/blob/4930dced25c0d9ec33096b0f2601d262daa477bf/src/Ryujinx.Audio/Renderer/Parameter/SplitterInParameterHeader.cs),
[`SplitterInParameter`](https://github.com/alula/Ryujinx/blob/4930dced25c0d9ec33096b0f2601d262daa477bf/src/Ryujinx.Audio/Renderer/Parameter/SplitterInParameter.cs),
[`SplitterDestinationInParameterVersion1`](https://github.com/alula/Ryujinx/blob/4930dced25c0d9ec33096b0f2601d262daa477bf/src/Ryujinx.Audio/Renderer/Parameter/SplitterDestinationInParameterVersion1.cs)
and `SplitterContext.UpdateState` at that revision. Only wire semantics were used;
the bounds-checked Rust reader is independently implemented.

Runtime tests exercise real service discovery and CMIF dispatch, copied transfer
memory and event handles, timed signaling, close/clone lifetime, malformed requests,
silent update bytes and preflight failure without partial output. The explicit
`wl-cli --test audio -- --ignored` test builds the public synthetic
[`audio.c`](../../tests/horizon/audio.c) client with devkitA64/libnx, recompiles its
AArch64 executable and checks two real guest event waits. It requires that toolchain;
the default test run does not count it as executed. No commercial code is included.

## Hosted Opus decoding

The native launcher registers a process-local `hwopus` service. Manager commands
0/1 open a mono/stereo decoder using a copied transfer-memory handle or query its
required work size. Extended manager commands 4/5 use the 16-byte parameter
structure, including the large-frame flag and reserved word. Unknown flags and
nonzero reserved fields are explicitly unsupported. Decoder commands 0, 4, 6 and 8
decode interleaved signed 16-bit PCM; commands 6/8 additionally accept a reset flag.
Ordinary decoders permit 40 ms packets; extended decoders with large-frame flag 1
permit 120 ms. Work-size reporting covers the software decoder workspace; PCM
scratch is host-owned and does not reside in guest transfer memory. Multistream and other commands
remain unsupported. The workspace requirement is the selected host decoder size
plus 4096 bytes, not a claim about Nintendo's hardware workspace formula.

Decoding uses the actual system libopus implementation. Linux requires
`libopus.so.0`; Windows requires an application-local `opus.dll` or
`libopus-0.dll`, loaded by absolute path without searching the working directory.
The library stays loaded while decoder function pointers can be used. Guest
pointers are never passed to the C library: bounded packets are acquired through
checked guest memory, PCM uses owned host storage and is written explicitly as
little-endian samples. The eight-byte packet header is big-endian payload length
and final range; a zero range disables that check. Output bounds are validated
before decoding. Decoder state is restored if decoding, final-range validation or
output publication fails. Performance replies report zero DSP time; this software
backend does not measure Nintendo hardware cycles. Decoders have process-local,
non-reused identities with 256 simultaneous and 4096 lifetime limits.

Horizon SVC-containing compilation units call a small typed service bridge built
against the exact same runtime archive. The launcher installs its handler before
execution. Unchanged units that contain no SVC retain their existing cache keys;
bridge-dependent units include the bridge identity in their keys. The handler
binds only the native-owned codec port for the duration of SendSyncRequest and
restores the port endpoint on success or error. Service-manager permissions and
the existing CMIF/domain/handle machinery still apply. Internally, opaque codec
indices use the hosted audio transport namespace, so legacy diagnostic interface
labels can say `IAudioRenderer`; this does not implement a generic guest IPC server.
The bridge is a matched-toolchain Rust interface, not a stable binary plug-in ABI.

Command layouts follow [libnx hwopus.c](https://github.com/switchbrew/libnx/blob/feebd026ca0f5dcc2119f46ad8e0d16ad3dd4973/nx/source/services/hwopus.c)
and [hwopus.h](https://github.com/switchbrew/libnx/blob/feebd026ca0f5dcc2119f46ad8e0d16ad3dd4973/nx/include/switch/services/hwopus.h).
The public [libopus decoder API](https://opus-codec.org/docs/opus_api-1.5/group__opus__decoder.html)
defines decoder state as contiguous and shallow-copyable, which permits bounded
rollback using `opus_decoder_get_size`. MacNativeGames revision
`66b89f3a7338e165d145ff8f33e2690ff758fe7d`, `games/totk/runtime/opus_codec.c`
(MIT), supplied a secondary reference for packet framing and host-codec ownership;
its SDK interception is distinct from this implementation's guest IPC path.

The explicit `wl-runtime --test audio -- --include-ignored` run exercises actual
mono/stereo decoding and rejected final-range rollback. The explicit
`wl-cli --test opus -- --ignored` run builds the synthetic libnx
[`opus.c`](../../tests/horizon/opus.c), recompiles its AArch64 instructions and
checks service discovery, transfer memory, exact PCM, output bounds and teardown.
These tests require system libopus, and the latter also requires devkitA64/libnx.
They contain no commercial game data.

Extended parameter layout, command identities and the large-frame flag were checked
against `HardwareOpusDecoderManager`, `HardwareOpusDecoderParameterInternalEx`,
`OpusDecoderFlags` and `HardwareOpusDecoder` under
[Ryujinx Codec/Detail](https://github.com/alula/Ryujinx/tree/4930dced25c0d9ec33096b0f2601d262daa477bf/src/Ryujinx.Horizon/Sdk/Codec/Detail)
(MIT, same pinned revision as above). A literal three-frame CBR DTX packet checks
60 ms decoding is rejected without publication under the ordinary limit and succeeds
with exactly 2880 stereo samples under the large-frame limit. The packet layout
follows RFC 6716; expected PCM and untouched tail bytes are independently specified.

Device output gain is stored and returned as the finite `f32` supplied by the
client; it is not clamped to `[0,1]`. Nonfinite gains remain explicitly unsupported
and do not change stored state. This follows the pass-through device gain in
MacNativeGames `audio_mixer.c::audio_device_handler` at the pinned revision and
Ryujinx `AudioDevice.SetAudioDeviceOutputVolume` at the pinned Codec/Audio revision.
Bit-level CMIF tests cover gains above one, negative gains, negative zero and failed
nonfinite writes followed by unchanged queries. This service does not claim audible
playback or define a hardware clipping curve.

`WL_AUDIO_TRACE=1` logs accepted device gain and its exact bit representation for
protocol diagnosis; it does not change the value or audio state.

Unknown-name gain behavior follows `AudioDevice.SetAudioDeviceOutputVolume` and
`GetAudioDeviceOutputVolume` in the same pinned Ryujinx implementation. Names are
still acquired through checked guest buffers; accepting an absent device does not
add it to enumeration or redirect its gain to `AudioTvOutput`.

## Cache-save enumeration through the hosted transport

The launcher service bridge also handles `fsp-srv` command 62,
`OpenSaveDataInfoReaderOnlyCacheStorage`. With no configured cache-save store,
it returns a real kernel-managed reader session/domain object. Reader command 0
returns an eight-byte signed zero count and preserves every output-buffer byte;
no cache entries exist. Descriptor bounds and output permissions are checked.
This implements empty cache enumeration, not persistent saves or general files.

The extension reuses the indexed hosted callback transport. Its opaque indices
are separate from audio renderers and Opus decoders. FS domain-ID allocation is
scoped to the FS server during creation, conversion and close; the original target
and allocator are restored on success or failure. The stock kernel remains
responsible for HIPC/CMIF validation, handles, domain IDs and replies. Classification
only selects this extension's calls and does not execute raw guest request bytes.
No runtime structure layout or frozen CPU-unit ABI is changed.

Command 62's object result follows Ryujinx `IFileSystemProxy` at revision
`4930dced25c0d9ec33096b0f2601d262daa477bf` (MIT); reader command 0's MapAlias output
and signed count follow libnx `fsSaveDataInfoReaderRead` at revision
`feebd026ca0f5dcc2119f46ad8e0d16ad3dd4973` (ISC). Model tests check real sessions,
unchanged output, FS/audio domain-ID separation, failed opening and close/reuse.
The explicit `wl-cli --test fs_cache -- --ignored` run builds the synthetic
[`fs_cache.c`](../../tests/horizon/fs_cache.c) client, recompiles its AArch64 code
and exercises two domain reader lifetimes. It requires devkitA64/libnx.

For the existing unconfigured save/SD `IFileSystem` objects, the hosted extension
also handles `GetEntryType` (7), `OpenFile` (8) and `OpenDirectory` (9). These empty
stores return FS `PathNotFound` (`0x202`) after checking the bounded absolute,
NUL-terminated pointer-buffer path and open flags. They never return success
without the required file/directory object. The mounted RomFS uses a distinct
`RomFsStorage` object and retains its real image-backed reads. This is not a
persistent save implementation; create/write operations are not added here.

Path/open wire layouts follow the same pinned libnx `fs.c`; the result encoding
is documented in [Switchbrew's error table](https://switchbrew.org/wiki/Error_codes).
Tests exercise both moved sessions and domain children, restoration after invalid
paths, and an actual recompiled libnx client making all three missing-path calls.

### Volatile application cache

The hosted extension implements `IApplicationFunctions::CreateCacheStorage` (27)
with a real, per-process RAM volume. Its 24-byte input contains a 16-bit index
and two aligned signed 64-bit sizes; the 16-byte output contains storage target
1 and zero additional required space after successful allocation of the volume
metadata. Sizes are checked before state changes. The combined declared data and
journal budgets are limited to 128 MiB across at most 16 cache volumes. Journal
space is accounted for but no persistent NAND image or crash-recovery journal is
implemented. Cache contents disappear when the process exits; this interface
must not be used as durable user-save storage.

`fsp-srv` 51/53 recognize current-application, primary-rank cache attributes and
return real writable/read-only mount objects. Command 62 creates an independent
snapshot reader of the actually created cache volumes, including their index,
application and declared size. File/directory objects have distinct bounded,
non-recycled opaque identities and use the stock session/domain close machinery.
Filesystem operations include file/directory creation, file deletion, entry type,
file/directory opening, RAM-cache commit, and free/total data capacity. File
operations implement bounded read/write, flush, resize and size; directory reads
return sorted 0x310-byte entries with independent cursors. Writes are checked
against access/append permissions and quota, acquire all source bytes before
mutating the file, and cannot replace an open file through deletion. Failed reads
or writes preserve file contents and enumeration cursors. Unknown operations and
unsupported metadata remain explicit errors.

The request shapes follow pinned libnx `fs.c`/`fs.h` above. AM command 27 is
independently corroborated by Ryujinx `IApplicationFunctions.cs`, revision
`4930dced25c0d9ec33096b0f2601d262daa477bf` (MIT), and the
[AM command list](https://switchbrew.org/wiki/AM_services#IApplicationFunctions).
The recompiled libnx client exercises AM and FS domain objects together, cache
creation/mount, byte-exact file writes/reads with EOF tail preservation, reopening,
directory enumeration and cache-save enumeration. Model tests additionally
inject invalid sizes, cross-application mounts, quota overruns, unmapped transfer
buffers, conflicting file handles, read-only access and stale closed objects.

### Explicit volatile-save launcher policy

`fsp-srv` commands 51 and 53 validate the complete mount request. Missing volumes
and saves belonging to another application return `TargetNotFound` (`0x7d402`),
without a fabricated filesystem object. Truncated requests cannot fall through
to a success-only stub. By default, application saves remain unconfigured.

A launcher may explicitly set `WL_VOLATILE_SAVES=1` to enable empty, per-process
RAM saves for the current application. Writable account/device mounts then create
bounded volumes on first use; read-only mounts never create volumes. Identity
includes save type, user ID, primary rank and index. Repeated mounts share that
volume, different users/types remain isolated, and unrelated application IDs
cannot access it. At most four 32 MiB save volumes exist; cache volumes have
separate IDs and remain the only entries in cache-save enumeration. **This policy
does not persist user saves, import existing saves, or implement the console's
NACP-controlled save provisioning.** A new process starts empty. It is intended
for controlled execution without a durable save provider.

The same bounded filesystem also supports file/directory rename, empty and
recursive directory deletion, and recursive directory cleaning. Two-path requests
validate both paths before mutation. Open files prevent deletion or renaming of
their containing subtree; failed operations preserve existing bytes/names. Moving
a directory into itself, deleting the root and overlong resulting paths are
rejected. The native libnx fixture selects HOS 16 wire wrappers explicitly and
checks writable/read-only mount aliases, cross-title absence, file/directory
rename, cleaning and deletion. Additional model tests verify user/type isolation,
process-lifetime isolation and failed rename atomicity.

### Temporary save storage

The original-format `OpenSaveDataFileSystem` request can select temporary storage:
space ID **3**, save type **4**, primary rank, zero UID/system ID and zero index.
This is distinct from user-space account/device saves and from cache type 5.
Writable opens create a bounded per-process temporary RAM volume lazily; read-only
opens require an existing volume. It uses the same checked file, directory,
permission and quota implementation. Temporary data does not enable the optional
volatile account/device-save policy and is not included in cache-save enumeration.

Storage space participates in identity. No SD-backed store is configured, so SD
user-space mounts return `TargetNotFound` instead of aliasing internal storage.
The enum values and mount layout follow pinned libnx `fs.h`/`fs.c` above. Tests
cover the literal `(space=3,type=4)` request without the optional save policy,
read-only reuse, absent-volume results, invalid space/type pairing, process
isolation and separation from account/device/cache storage. The recompiled
AArch64 client writes and reads independent bytes through temporary mount aliases.
