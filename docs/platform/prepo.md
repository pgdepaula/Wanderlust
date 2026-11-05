# Local play-report service

Native Horizon launchers register `prepo:u` before guest service requests and
route its port through the existing scoped hosted-service transport. Normal SM
service-access checks, HIPC parsing, kernel PID substitution and session lifetime
validation still apply. Registration does not create a guest server thread or
change the runtime archive ABI.

Commands 10100/10102/10104 save a report; 10101/10103/10105 additionally carry a
nonzero 128-bit user ID. A report requires one input pointer buffer for the room
name and one input map-alias buffer for opaque report bytes. The kernel-provided
PID must identify the current process. Client scalar PID placeholders are never
trusted as identity. Room names are bounded to 31 bytes; empty or blank names
are rejected. Reports are limited to 16 KiB in this implementation.

Each process retains its most recent 64 reports in owned RAM. Old entries are
evicted only after the replacement's checked guest reads succeed. No report
contents are logged, interpreted, persisted to disk or sent over a network.
Debug formatting includes the count rather than report contents. This is a
bounded local sink, not Nintendo telemetry delivery or a persistent report store.

Command 10300 returns transmission status zero: no upload is in progress.
Commands 40100/40101 read/write the process-local agreement-check flag. Other
commands, including immediate transmission and system-session identifiers, remain
explicitly unsupported. System/admin ports, application registration queries and
firmware-specific report interpretation are not implemented.

The command signatures and result module 129/descriptions 1, 5, 9 and 101 were
checked against the source-level interface at
[Ryujinx revision 6f60e102](https://git.axenov.dev/Museum/ryujinx/src/commit/6f60e102a29aa04fff5c6bcdec37213b2d8ac646/Ryujinx.Horizon/Prepo/Ipc/PrepoService.cs)
and its adjacent `PrepoResult.cs` (MIT). The Rust implementation is independently
written. The reference's logging/deserialization implementation is not imported.

Focused tests cover literal HIPC requests through SM and SVC 0x21, spoofed PID
replacement, null UIDs, permission denial, scoped port restoration on failure,
owned input snapshots, capacity eviction and failed-read atomicity. These tests
establish service behavior; they do not establish successful game rendering.
