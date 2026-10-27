use std::sync::Arc;
use wl_runtime::Memory;

fn oracle(memory: &Memory, address: u64) -> Option<usize> {
    memory
        .regions
        .iter()
        .position(|r| address >= r.base && address - r.base < r.bytes.len() as u64)
}
#[test]
fn cache_revalidates_insert_remove_split_and_backing_replacement() {
    let mut memory = Memory::default();
    memory.map(0x1000, vec![1; 64], true, true, true).unwrap();
    memory.map(0x3000, vec![3; 64], true, true, true).unwrap();
    for _ in 0..32 {
        assert_eq!(memory.load(0x3001, 1).unwrap(), 3);
        assert_eq!(memory.load(0x1001, 1).unwrap(), 1);
    }
    memory.map(0x2000, vec![2; 64], true, true, true).unwrap();
    assert_eq!(memory.load(0x3001, 1).unwrap(), 3);
    memory.regions.remove(0);
    assert!(memory.region_index(0x1001).is_none());
    memory.regions[1].bytes = vec![7; 64];
    assert_eq!(memory.load(0x3001, 1).unwrap(), 7);
    assert!(memory.split_at(0x3020));
    for address in [
        0x1000,
        0x2000,
        0x203f,
        0x2040,
        0x3000,
        0x301f,
        0x3020,
        0x303f,
        0x3040,
        u64::MAX,
    ] {
        assert_eq!(memory.region_index(address), oracle(&memory, address));
    }
}
#[test]
fn cached_index_never_caches_access_permissions() {
    let mut memory = Memory::default();
    memory.map(0x1000, vec![0; 64], true, true, true).unwrap();
    memory.check_execute(0x1000).unwrap();
    memory.store(0x1000, 1, 7).unwrap();
    memory.regions[0].write = false;
    memory.regions[0].execute = false;
    assert!(memory.store(0x1000, 1, 9).is_err());
    assert!(memory.check_execute(0x1000).is_err());
    assert_eq!(memory.load(0x1000, 1).unwrap(), 7);
    memory.regions[0].read = false;
    assert!(memory.load(0x1000, 1).is_err());
}
#[test]
fn deterministic_mapping_mutations_match_an_independent_linear_oracle() {
    let mut memory = Memory::default();
    let mut seed = 0x162e3d73u64;
    for step in 0..2000 {
        seed = seed.wrapping_mul(6364136223846793005).wrapping_add(1);
        let page = (seed >> 32) % 64;
        let base = 0x1000 + page * 0x100;
        if let Some(index) = memory.regions.iter().position(|r| r.base == base) {
            if step % 3 == 0 {
                memory.regions.remove(index);
            } else {
                memory.regions[index].bytes.fill(step as u8);
            }
        } else {
            memory
                .map(base, vec![step as u8; 32], true, true, false)
                .unwrap();
        }
        for address in [base, base + 31, base + 32, 0x1000, 0x5000] {
            assert_eq!(
                memory.region_index(address),
                oracle(&memory, address),
                "step{step}"
            );
        }
    }
}
#[test]
fn concurrent_readers_can_race_cache_updates_without_sharing_stale_indices() {
    let mut memory = Memory::default();
    for page in 0..64u64 {
        memory
            .map(
                0x1000 + page * 0x100,
                vec![page as u8; 32],
                true,
                false,
                false,
            )
            .unwrap();
    }
    let memory = Arc::new(memory);
    std::thread::scope(|scope| {
        for worker in 0..4u64 {
            let memory = Arc::clone(&memory);
            scope.spawn(move || {
                for i in 0..10000 {
                    let page = (i * 17 + worker * 7) % 64;
                    assert_eq!(memory.load(0x1000 + page * 0x100, 1).unwrap(), page);
                }
            });
        }
    });
}
