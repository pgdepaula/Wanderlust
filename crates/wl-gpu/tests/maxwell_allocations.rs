mod common;
use stats_alloc::{INSTRUMENTED_SYSTEM, Region, StatsAlloc};
use std::alloc::System;
use wl_gpu::{
    fifo::{Decoder, MethodWrite},
    maxwell::{CLASS, Maxwell3d},
};
#[global_allocator]
static ALLOCATOR: &StatsAlloc<System> = &INSTRUMENTED_SYSTEM;
#[test]
fn fifo_and_engine_method_hot_path_allocate_nothing() {
    use common::maxwell::*;
    use wl_gpu::memory::*;
    let mut map = AddressMap::default();
    map.map(GpuAddress(0x30000), GuestAddress(0x80000), 32)
        .unwrap();
    map.map(GpuAddress(0x31000), GuestAddress(0x80020), 32)
        .unwrap();
    let bytes = descriptor_bytes(TIC_RGBA, TSC_NEAREST);
    let memory = SliceMemory {
        base: GuestAddress(0x80000),
        bytes: &bytes,
    };
    let resolved_engine = configured();
    let region = Region::new(ALLOCATOR);
    let mut fifo = Decoder::default();
    let mut engine = Maxwell3d::default();
    let mut writes = 0;
    for _ in 0..1000 {
        for word in [
            0x20010000, CLASS, 0x200103c4, 1, 0x2003055d, 0, 0x30000, 0, 0x2001050a, 0,
        ] {
            if let Some(write) = fifo.push_word(word).unwrap() {
                engine.apply(write).unwrap();
                writes += 1;
            }
        }
        assert!(
            engine
                .apply(MethodWrite {
                    subchannel: 0,
                    method: 0x4000,
                    value: 0
                })
                .is_err()
        );
        fifo.finish().unwrap();
        let fragment = wl_gpu::shader::ConstantFragment::decode(include_bytes!(
            "fixtures/maxwell/quarters.bin"
        ))
        .unwrap();
        assert_eq!(
            fragment.output_bits(),
            [0x3e800000, 0x3f000000, 0x3f400000, 0x3f800000]
        );
        assert_eq!(
            resolved_engine
                .resolve_texture(0, 0, 0, &map, &memory)
                .unwrap()
                .width,
            37
        );
    }
    let stats = region.change();
    assert_eq!(writes, 6000);
    assert_eq!(stats.allocations, 0);
    assert_eq!(stats.reallocations, 0);
    assert_eq!(stats.bytes_allocated, 0);
}
