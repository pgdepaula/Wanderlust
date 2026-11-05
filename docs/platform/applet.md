# Library-applet availability event

The hosted launcher implements `ISelfController::GetLibraryAppletLaunchableEvent`
(command 9) as a copied, real kernel readable-event handle. Repeated requests
reference the same process-owned event. In the current profile no library applet
can be started, so the launch slot is free and the event begins signaled.

This event neither starts an applet nor supplies an applet result. Library-applet
creation/execution remains unsupported. An implementation that starts applets must
also clear/re-signal availability as that slot becomes occupied/free; the current
event alone does not implement that lifecycle.

Routing reuses the scoped hosted transport for both sessions and domain objects.
The SelfController identity and domain-ID allocator are restored after successful
and rejected calls. No kernel structure or frozen runtime archive ABI is changed.
PID, handle and buffer-bearing variants are rejected before event allocation.

The wire command and copied-event/manual-reset client behavior are established by
[libnx feebd026, applet.c](https://github.com/switchbrew/libnx/blob/feebd026ca0f5dcc2119f46ad8e0d16ad3dd4973/nx/source/services/applet.c),
`_appletWaitLibraryAppletLaunchableEvent` (ISC). Tests exercise rejection atomicity,
repeated copied-handle identity, signaled state and real domain dispatch.
