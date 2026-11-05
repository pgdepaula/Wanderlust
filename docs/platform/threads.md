# Horizon address arbitration

The hosted scheduler implements SVC `WaitForAddress` with signed 32-bit
less-than predicates, 32-bit equality and 64-bit equality. A failed predicate
returns `InvalidState` without blocking or changing guest memory. Predicate
failure takes precedence over a zero timeout.

`DecrementAndWaitIfLessThan` compares the original signed word. It decrements
only when that comparison succeeds, using 32-bit wrapping arithmetic. The
mutation precedes timeout handling: an eligible zero-timeout call still changes
the word and returns `TimedOut`. Ineligible calls do not require write access.
Eligible calls on unwritable storage fail with `InvalidCurrentMemory` without
changing the word. Blocking and wakeup use the existing ordered address waiters.

These semantics were checked against Atmosphère 1.9.5 revision
[`de9b02007bbfc62f2f9ee2abf4a96bc337f5f86a`](https://github.com/Atmosphere-NX/Atmosphere/blob/de9b02007bbfc62f2f9ee2abf4a96bc337f5f86a/libraries/libmesosphere/source/kern_k_address_arbiter.cpp),
`WaitIfLessThan`, `WaitIfEqual` and `WaitIfEqual64`. It was used as a behavioral
reference; no GPL implementation was imported.

`tests/threads.rs` exercises signed boundaries, equality mismatch, zero timeout,
read-only storage, actual blocking/wakeup, unchanged words on rejected waits and
ABI output-register clearing. The runtime remains a serialized cooperative
scheduler rather than a hardware multicore kernel.

Conditional `SignalToAddress` operations reject a mismatching word with
`InvalidState` before waking any waiter. Increment-if-equal wraps the 32-bit word
only after a successful match. Modify-by-waiter-count increments when no waiter
exists, decrements when all waiters fit within the requested count (or all are
requested), and leaves the word unchanged when more waiters remain. An unchanged
word requires no write permission. Woken threads retain priority/FIFO ordering.
The same pinned `SignalAndIncrementIfEqual` and
`SignalAndModifyByWaitingCountIfEqual` reference establishes this truth table.
Tests include actual blocked threads, mismatch failure atomicity, count zero and
negative counts, partial wakeup and readable-only unchanged storage.
