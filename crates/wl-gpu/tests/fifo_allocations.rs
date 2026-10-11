use stats_alloc::{INSTRUMENTED_SYSTEM, Region, StatsAlloc};
use std::alloc::System;
use wl_gpu::fifo::{DecodeError, Decoder};

#[global_allocator]
static ALLOCATOR: &StatsAlloc<System> = &INSTRUMENTED_SYSTEM;

// A separate one-test executable keeps unrelated parallel tests outside this region.
#[test]
fn method_decoding_and_faults_allocate_no_heap_storage() {
    let region = Region::new(ALLOCATOR);
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
    assert_eq!(writes, 5000);
    assert_eq!(stats.allocations, 0);
    assert_eq!(stats.reallocations, 0);
    assert_eq!(stats.bytes_allocated, 0);
}
