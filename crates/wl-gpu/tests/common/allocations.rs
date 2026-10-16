use stats_alloc::INSTRUMENTED_SYSTEM;
use std::alloc::{GlobalAlloc, Layout, System};
use std::cell::Cell;

thread_local! {
    // Constant TLS initialization does not allocate inside the allocator.
    static MEASURE: Cell<bool> = const { Cell::new(false) };
}
pub struct ThreadAllocator;
fn measuring() -> bool {
    MEASURE.try_with(Cell::get).unwrap_or(false)
}
// SAFETY: Both branches delegate the original pointer/layout to System (the
// instrumented branch only adds StatsAlloc counters). No allocation ownership
// or layout changes when measurement is enabled/disabled on this thread.
unsafe impl GlobalAlloc for ThreadAllocator {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        // SAFETY: Forward the caller's GlobalAlloc contract unchanged.
        unsafe {
            if measuring() {
                INSTRUMENTED_SYSTEM.alloc(layout)
            } else {
                System.alloc(layout)
            }
        }
    }
    unsafe fn alloc_zeroed(&self, layout: Layout) -> *mut u8 {
        // SAFETY: Forward the caller's GlobalAlloc contract unchanged.
        unsafe {
            if measuring() {
                INSTRUMENTED_SYSTEM.alloc_zeroed(layout)
            } else {
                System.alloc_zeroed(layout)
            }
        }
    }
    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        // SAFETY: Both allocation paths use System with the original layout.
        unsafe {
            if measuring() {
                INSTRUMENTED_SYSTEM.dealloc(ptr, layout)
            } else {
                System.dealloc(ptr, layout)
            }
        }
    }
    unsafe fn realloc(&self, ptr: *mut u8, layout: Layout, size: usize) -> *mut u8 {
        // SAFETY: Both allocation paths use System; preserve pointer and sizes.
        unsafe {
            if measuring() {
                INSTRUMENTED_SYSTEM.realloc(ptr, layout, size)
            } else {
                System.realloc(ptr, layout, size)
            }
        }
    }
}
// The TLS scope must be dropped on its originating thread. This marker is
// zero-sized and prevents moving the guard to another thread.
pub struct Measurement(std::marker::PhantomData<std::rc::Rc<()>>);
impl Measurement {
    pub fn start() -> Self {
        MEASURE.with(|v| assert!(!v.replace(true), "nested allocation measurement"));
        Self(std::marker::PhantomData)
    }
}
impl Drop for Measurement {
    fn drop(&mut self) {
        MEASURE.with(|v| v.set(false));
    }
}
