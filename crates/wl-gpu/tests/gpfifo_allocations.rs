use stats_alloc::{INSTRUMENTED_SYSTEM, Region, StatsAlloc};
use std::alloc::System;
#[global_allocator]
static ALLOCATOR: &StatsAlloc<System> = &INSTRUMENTED_SYSTEM;
use wl_gpu::gpfifo::Submission;
#[test]
fn preflight_has_no_owned_entry_allocations() {
    let mut bytes = Vec::new();
    for raw in [0x1000u64 | (1 << 42), 0x2000 | (2 << 42)] {
        bytes.extend(raw.to_le_bytes());
    }
    let region = Region::new(ALLOCATOR);
    for _ in 0..1000 {
        let s = Submission::new(&bytes).unwrap();
        assert_eq!(s.entries().count(), 2);
    }
    let a = region.change();
    assert_eq!((a.allocations, a.reallocations), (0, 0));
}
