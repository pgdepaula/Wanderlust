#[path = "common/allocations.rs"]
mod allocations;
use allocations::{Measurement, ThreadAllocator};
use stats_alloc::{INSTRUMENTED_SYSTEM, Region};
use std::sync::atomic::{AtomicBool, Ordering};
use wl_gpu::fifo::{DecodeError, Decoder};

#[global_allocator]
static ALLOCATOR: ThreadAllocator = ThreadAllocator;

// A single test plus per-thread gating excludes libtest/background traffic.
// Calibrate both inclusion and exclusion before measuring actual decoder work.
#[test]
fn method_decoding_and_faults_allocate_no_heap_storage() {
    let begin = std::sync::Arc::new(AtomicBool::new(false));
    let done = std::sync::Arc::new(AtomicBool::new(false));
    let worker = {
        let begin = begin.clone();
        let done = done.clone();
        std::thread::spawn(move || {
            while !begin.load(Ordering::Acquire) {
                std::thread::yield_now();
            }
            let bytes = std::hint::black_box(Vec::<u8>::with_capacity(127));
            std::hint::black_box(bytes);
            done.store(true, Ordering::Release);
        })
    };
    let measurement = Measurement::start();
    let region = Region::new(&INSTRUMENTED_SYSTEM);
    begin.store(true, Ordering::Release);
    while !done.load(Ordering::Acquire) {
        std::thread::yield_now();
    }
    assert_eq!(
        region.change().allocations,
        0,
        "another thread must be excluded"
    );
    let local = std::hint::black_box(Vec::<u8>::with_capacity(37));
    assert_eq!(
        region.change().allocations,
        1,
        "this thread must be counted"
    );
    assert_eq!(region.change().bytes_allocated, 37);
    std::hint::black_box(local);
    drop(measurement);
    worker.join().unwrap();

    let measurement = Measurement::start();
    let region = Region::new(&INSTRUMENTED_SYSTEM);
    let mut d = Decoder::default();
    let mut writes = 0;
    for _ in 0..1000 {
        for word in [0, 0xa003_0001, 5, 6, 7, 0x8002_0003] {
            writes += usize::from(d.push_word(word).unwrap().is_some());
        }
        assert_eq!(
            d.push_word(0xc000_0000),
            Err(DecodeError::UnsupportedOpcode(6))
        );
        d.finish().unwrap();
        assert_eq!(d.push_word(0x2002_0fff), Err(DecodeError::MethodOverflow));
        d.push_word(0x2001_0000).unwrap();
        assert_eq!(d.finish(), Err(DecodeError::Truncated { remaining: 1 }));
        writes += usize::from(d.push_word(8).unwrap().is_some());
    }
    let stats = region.change();
    drop(measurement);
    assert_eq!(writes, 5000);
    assert_eq!(stats.allocations, 0);
    assert_eq!(stats.reallocations, 0);
    assert_eq!(stats.bytes_allocated, 0);
}
