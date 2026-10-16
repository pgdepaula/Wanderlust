#[path = "common/allocations.rs"]
mod allocations;
use allocations::{Measurement, ThreadAllocator};
use stats_alloc::{INSTRUMENTED_SYSTEM, Region};
#[global_allocator]
static ALLOCATOR: ThreadAllocator = ThreadAllocator;
use wl_gpu::gpfifo::Submission;
#[test]
fn preflight_has_no_owned_entry_allocations() {
    let mut bytes = Vec::new();
    for raw in [0x1000u64 | (1 << 42), 0x2000 | (2 << 42)] {
        bytes.extend(raw.to_le_bytes());
    }
    let measurement = Measurement::start();
    let region = Region::new(&INSTRUMENTED_SYSTEM);
    for _ in 0..1000 {
        let s = Submission::new(&bytes).unwrap();
        assert_eq!(s.entries().count(), 2);
    }
    let a = region.change();
    drop(measurement);
    assert_eq!((a.allocations, a.reallocations), (0, 0));
}
