//! Runtime-owned registration guards, not emulated NVIDIA/kernel page pinning.
//! These tests use no title identity, game data, renderer or fabricated fences.
use wl_runtime::horizon::{
    self, HEAP_ALIGNMENT, Process, ProcessConfig, Range, attribute, result, state,
};
use wl_runtime::{Cpu, Memory};

const CODE: u64 = 0x0800_0000;
const BASE: u64 = 0x1000_0000;
const PAGE: u64 = 0x1000;

#[derive(Debug, PartialEq, Eq)]
struct RegionSnapshot {
    base: u64,
    bytes: Vec<u8>,
    permissions: (bool, bool, bool),
    state: u32,
    attribute: u32,
}
fn snapshot(memory: &Memory) -> Vec<RegionSnapshot> {
    memory
        .regions
        .iter()
        .map(|r| RegionSnapshot {
            base: r.base,
            bytes: r.bytes.clone(),
            permissions: (r.read, r.write, r.execute),
            state: r.state,
            attribute: r.attribute,
        })
        .collect()
}
fn normal(memory: &mut Memory, base: u64, size: usize) {
    memory
        .map_state(base, vec![0xa5; size], true, true, false, state::NORMAL)
        .unwrap();
}
fn process() -> (Process, Cpu) {
    let mut memory = Memory::default();
    memory
        .map_state(CODE, vec![0; 4096], true, false, true, state::CODE)
        .unwrap();
    let config = ProcessConfig {
        program_id: 0x0100_0000_0000_2000,
        process_id: 0x51,
        main_thread_id: 0x72,
        main_thread_stack_size: 0x10000,
        main_thread_priority: 44,
        ideal_core: 0,
        core_mask: 1,
        priority_mask: u64::MAX,
        memory_limit: 0x2000_0000,
        system_resource_size: 0,
        handle_table_size: 1023,
        thread_limit: 32,
        is_application: true,
        svc_permissions: [u64::MAX; 3],
        service_access: &[(false, "*"), (true, "*")],
    };
    Process::horizon(
        memory,
        CODE,
        Range {
            start: CODE,
            end: CODE + PAGE,
        },
        config,
    )
    .unwrap()
}
fn svc(process: &mut Process, cpu: &mut Cpu, id: u16, args: &[u64]) -> u32 {
    cpu.x[..args.len()].copy_from_slice(args);
    assert_eq!(horizon::dispatch(cpu, process, id).unwrap(), None);
    cpu.x[0] as u32
}

#[test]
fn pins_preserve_real_bytes_and_independent_range_owners() {
    let mut memory = Memory::default();
    normal(&mut memory, BASE, 3 * 4096);
    let first = memory.pin_gpu_range(BASE, PAGE).unwrap();
    let second = memory.pin_gpu_range(BASE + PAGE, PAGE).unwrap();
    // Distinct registrations may share one storage region, as different GPU
    // channels/allocations can. Releasing one must not release the other.
    assert!(memory.pin_gpu_range(BASE, PAGE).is_err());
    assert!(memory.pin_gpu_range(BASE, 2 * PAGE).is_err());
    memory.store(BASE + 8, 8, 0x0123_4567_89ab_cdef).unwrap();
    assert_eq!(memory.load(BASE + 8, 8).unwrap(), 0x0123_4567_89ab_cdef);
    assert_eq!(memory.read_bytes(BASE, 8).unwrap(), [0xa5; 8]);
    memory.release_gpu_pin(first).unwrap();
    assert!(memory.release_gpu_pin(first).is_err());
    let replacement = memory.pin_gpu_range(BASE, PAGE).unwrap();
    assert!(memory.release_gpu_pin(first).is_err());
    assert!(memory.pin_gpu_range(BASE, PAGE).is_err());
    assert!(memory.pin_gpu_range(BASE + PAGE, PAGE).is_err());
    memory.release_gpu_pin(second).unwrap();
    memory.release_gpu_pin(replacement).unwrap();
    assert_eq!(memory.load(BASE + 8, 8).unwrap(), 0x0123_4567_89ab_cdef);
}

#[test]
fn foreign_memory_pin_cannot_release_equal_numeric_address_registration() {
    let mut left = Memory::default();
    let mut right = Memory::default();
    normal(&mut left, BASE, 4096);
    normal(&mut right, BASE, 4096);
    let left_pin = left.pin_gpu_range(BASE, PAGE).unwrap();
    let right_pin = right.pin_gpu_range(BASE, PAGE).unwrap();
    assert!(left.release_gpu_pin(right_pin).is_err());
    assert!(right.release_gpu_pin(left_pin).is_err());
    assert!(left.pin_gpu_range(BASE, PAGE).is_err());
    assert!(right.pin_gpu_range(BASE, PAGE).is_err());
    left.release_gpu_pin(left_pin).unwrap();
    right.release_gpu_pin(right_pin).unwrap();
}

#[test]
fn invalid_geometry_permissions_state_and_cross_region_pins_do_not_mutate_memory() {
    let mut memory = Memory::default();
    normal(&mut memory, BASE, 4096);
    normal(&mut memory, BASE + 2 * PAGE, 4096);
    for (address, size) in [
        (BASE, 0),
        (BASE + 1, PAGE),
        (BASE, PAGE - 1),
        (BASE + PAGE, PAGE),
        (BASE, 2 * PAGE),
        (!(PAGE - 1), 2 * PAGE),
    ] {
        let before = snapshot(&memory);
        assert!(
            memory.pin_gpu_range(address, size).is_err(),
            "{address:x}/{size:x}"
        );
        assert_eq!(snapshot(&memory), before);
    }
    // Positive control after every rejection: failed preflight did not reserve it.
    let pin = memory.pin_gpu_range(BASE, PAGE).unwrap();
    memory.release_gpu_pin(pin).unwrap();
    for (read, write, execute, region_state, attr) in [
        (false, true, false, state::NORMAL, 0),
        (true, false, false, state::NORMAL, 0),
        (true, true, true, state::NORMAL, 0),
        (true, true, false, state::STACK, 0),
        (true, true, false, state::SHARED_MEMORY, 0),
        (true, true, false, state::NORMAL, attribute::LOCKED),
    ] {
        let mut candidate = Memory::default();
        candidate
            .map_state(BASE, vec![0x39; 4096], read, write, execute, region_state)
            .unwrap();
        candidate.regions[0].attribute = attr; // Fixture initialization, never after a pin.
        let before = snapshot(&candidate);
        assert!(candidate.pin_gpu_range(BASE, PAGE).is_err());
        assert_eq!(snapshot(&candidate), before);
    }
}

#[test]
fn adjacent_normal_regions_are_consolidated_once_without_changing_guest_bytes() {
    for attributes in [0, attribute::UNCACHED] {
        let mut memory = Memory::default();
        memory
            .map_state(BASE, vec![0x31; 8192], true, true, false, state::NORMAL)
            .unwrap();
        memory
            .map_state(
                BASE + 8192,
                vec![0xa7; 8192],
                true,
                true,
                false,
                state::NORMAL,
            )
            .unwrap();
        for r in &mut memory.regions {
            r.attribute = attributes;
        }
        let left_pin = memory.pin_gpu_range(BASE, PAGE).unwrap();
        let right_pin = memory.pin_gpu_range(BASE + 3 * PAGE, PAGE).unwrap();
        assert!(memory.read_bytes(BASE + PAGE, 8192).is_err());
        let pin = memory.pin_gpu_range(BASE + PAGE, 8192).unwrap();
        assert_eq!(memory.read_bytes(BASE + PAGE, 4096).unwrap(), &[0x31; 4096]);
        let borrowed = memory.read_bytes(BASE + PAGE, 8192).unwrap();
        assert_eq!(&borrowed[..4096], &[0x31; 4096]);
        assert_eq!(&borrowed[4096..], &[0xa7; 4096]);
        assert_eq!(memory.regions.len(), 3);
        assert!(memory.regions.iter().all(|r| r.attribute == attributes
            && r.state == state::NORMAL
            && r.read
            && r.write
            && !r.execute));
        assert_eq!(memory.read_bytes(BASE, 4096).unwrap(), &[0x31; 4096]);
        assert_eq!(
            memory.read_bytes(BASE + 3 * PAGE, 4096).unwrap(),
            &[0xa7; 4096]
        );
        memory.write_bytes(BASE + 2 * PAGE, &[1, 2, 3, 4]).unwrap();
        assert_eq!(
            memory.read_bytes(BASE + 2 * PAGE, 4).unwrap(),
            &[1, 2, 3, 4]
        );
        assert!(memory.ensure_gpu_unpinned(BASE + PAGE, 8192).is_err());
        assert!(!memory.split_at(BASE + 2 * PAGE));
        memory.release_gpu_pin(pin).unwrap();
        assert!(memory.split_at(BASE + 2 * PAGE));
        assert!(memory.ensure_gpu_unpinned(BASE, PAGE).is_err());
        assert!(memory.ensure_gpu_unpinned(BASE + 3 * PAGE, PAGE).is_err());
        memory.release_gpu_pin(left_pin).unwrap();
        memory.release_gpu_pin(right_pin).unwrap();
    }
}

#[test]
fn incompatible_adjacent_gpu_storage_fails_before_restructuring_memory() {
    for (attributes, writable) in [
        (attribute::UNCACHED, true),
        (attribute::LOCKED, true),
        (0, false),
    ] {
        let mut memory = Memory::default();
        normal(&mut memory, BASE, 4096);
        memory
            .map_state(
                BASE + PAGE,
                vec![0x75; 4096],
                true,
                writable,
                false,
                state::NORMAL,
            )
            .unwrap();
        memory.regions[1].attribute = attributes;
        let before = snapshot(&memory);
        assert!(memory.pin_gpu_range(BASE, 8192).is_err());
        assert_eq!(snapshot(&memory), before);
        let pin = memory.pin_gpu_range(BASE, 4096).unwrap();
        memory.release_gpu_pin(pin).unwrap();
    }
}

#[test]
fn capacity_failure_and_stale_reused_slot_do_not_drop_live_registrations() {
    let capacity = wl_runtime::gpu_memory::MAX_GPU_PINS;
    assert_eq!(capacity, 1024);
    let mut memory = Memory::default();
    normal(&mut memory, BASE, (capacity + 1) * 4096);
    let mut pins = Vec::new();
    for page in 0..capacity {
        pins.push(
            memory
                .pin_gpu_range(BASE + page as u64 * PAGE, PAGE)
                .unwrap(),
        );
    }
    let before = snapshot(&memory);
    let last = BASE + capacity as u64 * PAGE;
    assert!(memory.pin_gpu_range(last, PAGE).is_err());
    assert_eq!(snapshot(&memory), before);
    let stale = pins.remove(17);
    memory.release_gpu_pin(stale).unwrap();
    let replacement = memory.pin_gpu_range(last, PAGE).unwrap();
    assert!(memory.release_gpu_pin(stale).is_err());
    assert!(memory.pin_gpu_range(last, PAGE).is_err());
    assert!(memory.pin_gpu_range(BASE + 17 * PAGE, PAGE).is_err());
    for pin in pins {
        memory.release_gpu_pin(pin).unwrap();
    }
    memory.release_gpu_pin(replacement).unwrap();
    // Capacity fully recovered, and pin teardown never erased stored contents.
    let again = memory
        .pin_gpu_range(BASE, (capacity as u64 + 1) * PAGE)
        .unwrap();
    memory.release_gpu_pin(again).unwrap();
    assert_eq!(snapshot(&memory), before);
}

#[test]
fn direct_split_inside_pin_fails_without_structural_changes_then_succeeds_after_release() {
    let mut memory = Memory::default();
    normal(&mut memory, BASE, 8192);
    let pin = memory.pin_gpu_range(BASE, PAGE).unwrap();
    let before = snapshot(&memory);
    assert!(!memory.split_at(BASE + PAGE / 2));
    assert_eq!(snapshot(&memory), before);
    memory.release_gpu_pin(pin).unwrap();
    assert!(memory.split_at(BASE + PAGE / 2));
    assert_eq!(memory.regions.len(), 2);
    assert_eq!(memory.read_bytes(BASE, 8).unwrap(), [0xa5; 8]);
}

#[test]
fn svc_reprotection_and_partial_pin_attributes_fail_before_split_or_write() {
    for (id, args) in [
        (0x02, vec![BASE, PAGE, 1]),
        (
            0x03,
            vec![
                BASE,
                PAGE,
                u64::from(attribute::UNCACHED),
                u64::from(attribute::UNCACHED),
            ],
        ),
    ] {
        let (mut process, mut cpu) = process();
        normal(&mut process.memory, BASE, 8192);
        let pin = process
            .memory
            .pin_gpu_range(BASE, if id == 0x03 { 2 * PAGE } else { PAGE })
            .unwrap();
        let before = snapshot(&process.memory);
        assert_ne!(svc(&mut process, &mut cpu, id, &args), result::SUCCESS);
        assert_eq!(snapshot(&process.memory), before);
        // Same operation becomes valid after release: it is the registration
        // guard, not unrelated bad SVC arguments, that rejected it.
        process.memory.release_gpu_pin(pin).unwrap();
        assert_eq!(svc(&mut process, &mut cpu, id, &args), result::SUCCESS);
        let region = &process.memory.regions[process.memory.region_index(BASE).unwrap()];
        if id == 0x02 {
            assert_eq!((region.read, region.write), (true, false));
        } else {
            assert_eq!(region.attribute & attribute::UNCACHED, attribute::UNCACHED);
        }
    }
}

#[test]
fn complete_gpu_pin_cache_changes_preserve_bytes_permissions_and_lifetime() {
    let (mut process, mut cpu) = process();
    normal(&mut process.memory, BASE, (4 * PAGE) as usize);
    let pin = process.memory.pin_gpu_range(BASE + PAGE, PAGE).unwrap();
    let original = process
        .memory
        .read_bytes(BASE, (4 * PAGE) as usize)
        .unwrap()
        .to_vec();
    for value in [attribute::UNCACHED, 0, attribute::UNCACHED] {
        assert_eq!(
            svc(
                &mut process,
                &mut cpu,
                3,
                &[
                    BASE + PAGE,
                    PAGE,
                    u64::from(attribute::UNCACHED),
                    u64::from(value)
                ]
            ),
            result::SUCCESS
        );
        assert_eq!(
            process
                .memory
                .read_bytes(BASE + PAGE, PAGE as usize)
                .unwrap(),
            &original[PAGE as usize..(2 * PAGE) as usize]
        );
        let region = &process.memory.regions[process.memory.region_index(BASE + PAGE).unwrap()];
        assert_eq!(region.attribute, value);
        assert_eq!(
            (region.read, region.write, region.execute),
            (true, true, false)
        );
        assert!(
            process
                .memory
                .ensure_gpu_unpinned(BASE + PAGE, PAGE)
                .is_err()
        );
        assert_eq!(
            svc(&mut process, &mut cpu, 2, &[BASE + PAGE, PAGE, 1]),
            result::INVALID_CURRENT_MEMORY
        );
        assert_eq!(
            process.memory.regions[process.memory.region_index(BASE).unwrap()].attribute,
            0
        );
        assert_eq!(
            process.memory.regions[process.memory.region_index(BASE + 2 * PAGE).unwrap()].attribute,
            0
        );
    }
    process.memory.release_gpu_pin(pin).unwrap();
}

#[test]
fn region_split_retains_the_large_allocation_and_exact_byte_order() {
    for split in [PAGE, 3 * PAGE] {
        let mut memory = Memory::default();
        let bytes: Vec<u8> = (0..4 * PAGE)
            .map(|i| (i.wrapping_mul(31) >> 7) as u8)
            .collect();
        memory
            .map_state(BASE, bytes.clone(), true, true, false, state::NORMAL)
            .unwrap();
        let pointer = memory.regions[0].bytes.as_ptr();
        let capacity = memory.regions[0].bytes.capacity();
        assert!(memory.split_at(BASE + split));
        assert_eq!(memory.regions.len(), 2);
        assert_eq!(memory.regions[0].bytes, &bytes[..split as usize]);
        assert_eq!(memory.regions[1].bytes, &bytes[split as usize..]);
        let retained = if split < PAGE * 2 { 1 } else { 0 };
        assert_eq!(memory.regions[retained].bytes.as_ptr(), pointer);
        assert_eq!(memory.regions[retained].bytes.capacity(), capacity);
    }
}

#[test]
fn svc_map_source_cannot_move_registered_storage_or_create_an_alias() {
    let (mut process, mut cpu) = process();
    normal(&mut process.memory, BASE, 8192);
    let destination = process.kernel.as_ref().unwrap().layout.stack.start + 0x200_0000;
    let pin = process.memory.pin_gpu_range(BASE, PAGE).unwrap();
    let before = snapshot(&process.memory);
    let aliases = process.kernel.as_ref().unwrap().aliases.clone();
    assert_ne!(
        svc(&mut process, &mut cpu, 0x04, &[destination, BASE, PAGE]),
        result::SUCCESS
    );
    assert_eq!(snapshot(&process.memory), before);
    assert_eq!(process.kernel.as_ref().unwrap().aliases, aliases);
    process.memory.release_gpu_pin(pin).unwrap();
    assert_eq!(
        svc(&mut process, &mut cpu, 0x04, &[destination, BASE, PAGE]),
        result::SUCCESS
    );
    assert_eq!(
        process.memory.read_bytes(destination, 4096).unwrap(),
        vec![0xa5; 4096]
    );
    assert!(process.memory.read_bytes(BASE, 1).is_err());
    assert_eq!(
        process.kernel.as_ref().unwrap().aliases,
        [(destination, BASE, PAGE)]
    );
    // Legal alias storage is deliberately not a plain NORMAL registration.
    assert!(process.memory.pin_gpu_range(BASE, PAGE).is_err());
    assert!(process.memory.pin_gpu_range(destination, PAGE).is_err());
    assert_eq!(
        svc(&mut process, &mut cpu, 0x05, &[destination, BASE, PAGE]),
        result::SUCCESS
    );
    assert_eq!(
        process.memory.read_bytes(BASE, 4096).unwrap(),
        vec![0xa5; 4096]
    );
}

#[test]
fn transfer_memory_creation_is_blocked_without_allocating_a_handle_or_record() {
    let (mut process, mut cpu) = process();
    normal(&mut process.memory, BASE, 8192);
    let pin = process.memory.pin_gpu_range(BASE, PAGE).unwrap();
    let before = snapshot(&process.memory);
    let records = process.kernel.as_ref().unwrap().transfer_memories.len();
    assert_ne!(
        svc(&mut process, &mut cpu, 0x15, &[0, BASE, PAGE, 0]),
        result::SUCCESS
    );
    assert_eq!(snapshot(&process.memory), before);
    assert_eq!(
        process.kernel.as_ref().unwrap().transfer_memories.len(),
        records
    );
    process.memory.release_gpu_pin(pin).unwrap();
    assert_eq!(
        svc(&mut process, &mut cpu, 0x15, &[0, BASE, PAGE, 0]),
        result::SUCCESS
    );
    assert_eq!(
        process.kernel.as_ref().unwrap().transfer_memories.len(),
        records + 1
    );
    // Bootstrap consumed exactly the main-thread slot/linear ID. A denied
    // transfer creation must not consume the next handle or its linear ID.
    assert_eq!(cpu.x[1], 1021 | (2 << 15));
    assert!(matches!(
        process
            .kernel
            .as_ref()
            .unwrap()
            .handles
            .get(cpu.x[1] as u32),
        Some(horizon::Object::TransferMemory(_))
    ));
}

#[test]
fn heap_shrink_cannot_recycle_live_registered_bytes_and_stale_token_cannot_unlock_new_heap() {
    let (mut process, mut cpu) = process();
    assert_eq!(
        svc(&mut process, &mut cpu, 0x01, &[0, HEAP_ALIGNMENT]),
        result::SUCCESS
    );
    let base = process.kernel.as_ref().unwrap().layout.heap.start;
    let address = base + HEAP_ALIGNMENT - PAGE;
    let pin = process.memory.pin_gpu_range(address, PAGE).unwrap();
    process
        .memory
        .store(address, 8, 0x1122_3344_5566_7788)
        .unwrap();
    let before = snapshot(&process.memory);
    assert_ne!(svc(&mut process, &mut cpu, 0x01, &[0, 0]), result::SUCCESS);
    assert_eq!(snapshot(&process.memory), before);
    assert_eq!(process.kernel.as_ref().unwrap().heap_size, HEAP_ALIGNMENT);
    process.memory.release_gpu_pin(pin).unwrap();
    assert_eq!(svc(&mut process, &mut cpu, 0x01, &[0, 0]), result::SUCCESS);
    assert!(process.memory.load(address, 8).is_err());
    assert_eq!(
        svc(&mut process, &mut cpu, 0x01, &[0, HEAP_ALIGNMENT]),
        result::SUCCESS
    );
    assert_eq!(process.memory.load(address, 8).unwrap(), 0);
    let replacement = process.memory.pin_gpu_range(address, PAGE).unwrap();
    assert!(process.memory.release_gpu_pin(pin).is_err());
    assert_ne!(svc(&mut process, &mut cpu, 0x01, &[0, 0]), result::SUCCESS);
    assert_eq!(process.kernel.as_ref().unwrap().heap_size, HEAP_ALIGNMENT);
    process.memory.release_gpu_pin(replacement).unwrap();
    assert_eq!(svc(&mut process, &mut cpu, 0x01, &[0, 0]), result::SUCCESS);
}

#[test]
fn immediately_adjacent_permission_changes_preserve_registered_middle_page() {
    let (mut process, mut cpu) = process();
    normal(&mut process.memory, BASE, 3 * 4096);
    let pin = process.memory.pin_gpu_range(BASE + PAGE, PAGE).unwrap();
    for address in [BASE, BASE + 2 * PAGE] {
        assert_eq!(
            svc(&mut process, &mut cpu, 0x02, &[address, PAGE, 1]),
            result::SUCCESS
        );
        let index = process.memory.region_index(address).unwrap();
        assert!(process.memory.regions[index].read);
        assert!(!process.memory.regions[index].write);
    }
    process
        .memory
        .store(BASE + PAGE + 4, 4, 0x11223344)
        .unwrap();
    assert_eq!(
        process.memory.read_bytes(BASE + PAGE + 4, 4).unwrap(),
        [0x44, 0x33, 0x22, 0x11]
    );
    assert!(process.memory.pin_gpu_range(BASE + PAGE, PAGE).is_err());
    assert!(
        process
            .memory
            .ensure_gpu_unpinned(BASE + PAGE, PAGE)
            .is_err()
    );
    process.memory.release_gpu_pin(pin).unwrap();
    let replacement = process.memory.pin_gpu_range(BASE + PAGE, PAGE).unwrap();
    process.memory.release_gpu_pin(replacement).unwrap();
}
