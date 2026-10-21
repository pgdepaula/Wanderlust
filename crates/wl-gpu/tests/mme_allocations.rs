use stats_alloc::{INSTRUMENTED_SYSTEM, Region, StatsAlloc};
use std::alloc::System;
use wl_gpu::mme::MacroEngine;
#[global_allocator]
static ALLOCATOR: &StatsAlloc<System> = &INSTRUMENTED_SYSTEM;

#[test]
fn repeated_macro_invocations_reuse_uploaded_owned_instruction_ram() {
    let cold_region = Region::new(ALLOCATOR);
    let mut mme = MacroEngine::default();
    mme.upload(0x11c, 0).unwrap();
    mme.upload(0x120, 0).unwrap();
    mme.upload(0x114, 0).unwrap();
    for bytes in include_bytes!("fixtures/mme/state-mask.bin").chunks_exact(4) {
        mme.upload(0x118, u32::from_le_bytes(bytes.try_into().unwrap()))
            .unwrap();
    }
    let cold = cold_region.change();
    assert_eq!(cold.allocations, 1);
    let region = Region::new(ALLOCATOR);
    let start = std::time::Instant::now();
    for parameter in 0..10000 {
        assert_eq!(
            std::hint::black_box(mme.execute(0, parameter)).unwrap(),
            (0x1a00, parameter)
        );
    }
    let elapsed = start.elapsed();
    let stats = region.change();
    assert_eq!((stats.allocations, stats.reallocations), (0, 0));
    eprintln!(
        "MME upload allocations={}, bytes={}; 10000 invocations: {elapsed:?}, zero measured Rust allocations/reallocations",
        cold.allocations, cold.bytes_allocated
    );
}
