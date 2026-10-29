//! Separate caller bookkeeping allocations from asynchronous driver activity.
use std::{
    alloc::{GlobalAlloc, Layout},
    cell::Cell,
};
#[derive(Clone, Copy, Default, Debug)]
pub struct Counts {
    pub allocations: usize,
    pub reallocations: usize,
}
thread_local! {
    static ACTIVE:Cell<bool>=const{Cell::new(false)};
    static COUNTS:Cell<Counts>=const{Cell::new(Counts{allocations:0,reallocations:0})};
}
pub struct Allocator;
fn record(reallocation: bool) {
    let _ = ACTIVE.try_with(|active| {
        if active.get() {
            let _ = COUNTS.try_with(|counts| {
                let mut n = counts.get();
                if reallocation {
                    n.reallocations = n.reallocations.saturating_add(1);
                } else {
                    n.allocations = n.allocations.saturating_add(1);
                }
                counts.set(n);
            });
        }
    });
}
// SAFETY: every operation forwards its pointer/layout unchanged to the existing
// process-wide System instrumentation. TLS observation never allocates or
// accesses another thread's state, including during thread destruction.
unsafe impl GlobalAlloc for Allocator {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        let p = unsafe { stats_alloc::INSTRUMENTED_SYSTEM.alloc(layout) };
        if !p.is_null() {
            record(false);
        }
        p
    }
    unsafe fn alloc_zeroed(&self, layout: Layout) -> *mut u8 {
        let p = unsafe { stats_alloc::INSTRUMENTED_SYSTEM.alloc_zeroed(layout) };
        if !p.is_null() {
            record(false);
        }
        p
    }
    unsafe fn dealloc(&self, p: *mut u8, layout: Layout) {
        unsafe { stats_alloc::INSTRUMENTED_SYSTEM.dealloc(p, layout) }
    }
    unsafe fn realloc(&self, p: *mut u8, layout: Layout, size: usize) -> *mut u8 {
        let p = unsafe { stats_alloc::INSTRUMENTED_SYSTEM.realloc(p, layout, size) };
        if !p.is_null() {
            record(true);
        }
        p
    }
}
pub struct Region;
impl Region {
    pub fn new() -> Self {
        ACTIVE.with(|v| assert!(!v.replace(true), "nested caller allocation scope"));
        COUNTS.with(|v| v.set(Counts::default()));
        Self
    }
    pub fn finish(self) -> Counts {
        ACTIVE.with(|v| v.set(false));
        COUNTS.with(Cell::get)
    }
}
impl Drop for Region {
    fn drop(&mut self) {
        let _ = ACTIVE.try_with(|v| v.set(false));
    }
}

#[test]
fn caller_scope_observes_real_allocation_and_reallocation() {
    let scope = Region::new();
    let layout = Layout::from_size_align(8, 8).unwrap();
    // SAFETY: the original layout accompanies the live allocation, and the new
    // allocation is freed using the corresponding new size and alignment.
    unsafe {
        let p = std::alloc::alloc(layout);
        assert!(!p.is_null());
        let p = std::alloc::realloc(p, layout, 16);
        assert!(!p.is_null());
        std::alloc::dealloc(p, Layout::from_size_align(16, 8).unwrap());
    }
    let counts = scope.finish();
    assert_eq!(counts.allocations, 1);
    assert_eq!(counts.reallocations, 1);
}
