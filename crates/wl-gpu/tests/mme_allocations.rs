#[path = "common/scoped_alloc.rs"]
mod measured_alloc;
use wl_gpu::mme::MacroEngine;
#[global_allocator]
static ALLOCATOR: measured_alloc::Allocator = measured_alloc::Allocator;

#[test]
fn repeated_macro_invocations_reuse_uploaded_owned_instruction_ram() {
    // Only this caller's work is measured. The Rust harness may allocate on
    // another thread while it coordinates test execution/results.
    let cold_region = measured_alloc::Region::new();
    let mut mme = MacroEngine::default();
    mme.upload(0x11c, 0).unwrap();
    mme.upload(0x120, 0).unwrap();
    mme.upload(0x114, 0).unwrap();
    for bytes in include_bytes!("fixtures/mme/state-mask.bin").chunks_exact(4) {
        mme.upload(0x118, u32::from_le_bytes(bytes.try_into().unwrap()))
            .unwrap();
    }
    let cold = cold_region.finish();
    assert_eq!((cold.allocations, cold.reallocations), (1, 0));
    let start = std::time::Instant::now();
    let region = measured_alloc::Region::new();
    for parameter in 0..10000 {
        assert_eq!(
            std::hint::black_box(mme.execute(0, parameter)).unwrap(),
            (0x1a00, parameter)
        );
    }
    let stats = region.finish();
    let elapsed = start.elapsed();
    assert_eq!((stats.allocations, stats.reallocations), (0, 0));
    eprintln!(
        "MME upload caller allocations={}; 10000 invocations: {elapsed:?}, zero caller Rust allocations/reallocations",
        cold.allocations
    );
}
