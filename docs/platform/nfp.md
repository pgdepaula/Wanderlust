# NFC service without attached readers

The native launcher provides `nfp:user` through normal SM access checks and the
existing scoped hosted-service transport. Manager command 0 returns an independent
user interface. Domain conversion and returned domain objects use the runtime's
existing IPC implementation, not guest instruction hooks.

The supported profile has no physical NFC reader and no amiibo/tag emulation.
User-interface command 0 initializes the interface using the kernel-supplied PID,
current application's ARUID and a checked input MCU-version array. Records are
32 bytes with a nonzero version and zero reserved bytes; one through eight records
are supported. Command 1 finalizes; command 19 returns the interface's actual
noninitialized/initialized state (0/1).

Command 2 validates the output device-handle span, returns `DeviceNotFound`
(`0x8073`, module 115, description 64), and leaves the array untouched. It does
not invent a connected reader or return successful tag access. Command 23 returns
a real copied readable event handle for availability changes. With no reader
changes, that event remains unsignaled; repeated requests share its event identity.
Closing an interface invalidates its service identity without invalidating already
copied kernel handles. The process owns at most 64 live interfaces and 4096
interface lifetimes; IDs are not recycled into stale objects.

Device operations before initialization and unimplemented commands fail explicitly.
NFC system/debug ports, Mifare, tag detection, cryptography, application-area data,
controller NFC discovery and real reader attachment are not implemented.

Wire signatures, sizes and state enums come from
[libnx feebd026, nfc.c](https://github.com/switchbrew/libnx/blob/feebd026ca0f5dcc2119f46ad8e0d16ad3dd4973/nx/source/services/nfc.c)
and its `nfc.h` (ISC). The empty-reader result and manager/interface distinction
were corroborated against
[Ryujinx 6f60e102, INfp.cs](https://git.axenov.dev/Museum/ryujinx/src/commit/6f60e102a29aa04fff5c6bcdec37213b2d8ac646/Ryujinx.HLE/HOS/Services/Nfc/Nfp/NfpManager/INfp.cs)
and adjacent `ResultCode.cs` (MIT). The implementation is independently written.

Tests exercise literal SM/domain/CreateUserInterface/Initialize/GetState traffic,
kernel replacement of a spoofed PID, ARUID rejection, owned interface lifetimes,
unsignaled event identity, failed output validation and untouched output bytes.
They establish the no-reader profile, not amiibo compatibility or game rendering.
