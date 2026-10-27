# Host GPU backing registration

The native NVIDIA adapter first requests the runtime's normal GPU pin. That path
validates page alignment, checked address arithmetic, complete compatible NORMAL
read/write storage, attributes, registration capacity and overlap with existing
pins. Single-region registration borrows the existing owned allocation without a
consolidation copy.

If the runtime rejects only its 256 MiB consolidation-copy budget, the hosted
adapter permits a bounded copy of up to 1 GiB. These are host staging budgets,
not Nintendo allocation-size limits. It holds exclusive mutable access, repeats
the unpinned-range check, stages all requested bytes and reserves region metadata
before replacing storage. Public checked split operations preserve the surrounding
address ranges. Permissions, attributes, guest addresses and canonical bytes remain
unchanged; only an unpinned range's internal region representation is joined.
Existing pins outside the range retain their guards. No borrowed guest slice
survives this operation.

After joining, the adapter calls the normal runtime pin again on the single owned
region. The runtime still creates, validates and releases the actual opaque pin;
no host pointer or synthetic token substitutes for it. Registration failure does
not turn into success. A resource failure after joining may leave equivalent bytes
in one region rather than several; it must not lose guest data or another pin.
This compatible hosted extension does not change the runtime's structure layout
or require previously compiled CPU units to use a different runtime archive.

The explicit `wl-nvdrv --test registration -- --include-ignored --test-threads=1`
run checks a 498 MiB two-region allocation byte by byte, a rejected copy budget,
retained pin lifetime, prohibited overlap/splitting, cross-boundary CPU writes and
release. The large test needs about 1 GiB peak host RAM. Ordinary tests check
alignment, overflow and incompatible memory without creating a GPU device.
