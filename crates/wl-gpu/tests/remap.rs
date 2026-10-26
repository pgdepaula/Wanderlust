use wl_gpu::{driver::*, memory::*};
const P: u64 = 4096;
#[test]
fn replacement_splits_resident_backing_and_preserves_aliases() {
    let mut m = MemoryManager::default();
    let a = m.register_backing(GuestAddress(0x100000), 8 * P).unwrap();
    let b = m.register_backing(GuestAddress(0x200000), 4 * P).unwrap();
    let s = m.create_space().unwrap();
    let r = m.reserve_fixed(s, GpuAddress(0x10000), 8 * P).unwrap();
    let alias = m.reserve_fixed(s, GpuAddress(0x30000), 8 * P).unwrap();
    let old = m.map(s, r, a, 0).unwrap();
    m.map(s, alias, a, 0).unwrap();
    let epoch = m.mapping_epoch(s).unwrap();
    m.apply_map_changes(
        s,
        &[MapChange {
            reservation: r,
            address: GpuAddress(0x12000),
            size: 2 * P,
            backing: Some((b, P)),
        }],
    )
    .unwrap();
    assert!(m.mapping_epoch(s).unwrap() > epoch);
    let map = m.address_map(s).unwrap();
    for (gpu, guest) in [
        (0x10000, 0x100000),
        (0x11000, 0x101000),
        (0x12000, 0x201000),
        (0x13000, 0x202000),
        (0x14000, 0x104000),
        (0x17000, 0x107000),
        (0x32000, 0x102000),
    ] {
        assert_eq!(
            map.resolve(GpuAddress(gpu), 1).unwrap(),
            GuestAddress(guest)
        );
    }
    assert_eq!(m.mappings(s, r).unwrap().len(), 3);
    assert!(m.unmap(s, old).is_err());
    assert!(m.release_backing(a).is_err());
    assert!(m.release_backing(b).is_err());
    m.apply_map_changes(
        s,
        &[MapChange {
            reservation: r,
            address: GpuAddress(0x13000),
            size: 2 * P,
            backing: None,
        }],
    )
    .unwrap();
    let map = m.address_map(s).unwrap();
    assert!(map.resolve(GpuAddress(0x13000), 1).is_err());
    assert!(map.resolve(GpuAddress(0x14000), 1).is_err());
    assert_eq!(
        map.resolve(GpuAddress(0x15000), 1).unwrap(),
        GuestAddress(0x105000)
    );
    assert_eq!(
        map.resolve(GpuAddress(0x12000), 1).unwrap(),
        GuestAddress(0x201000)
    );
    assert_eq!(
        map.resolve(GpuAddress(0x33000), 1).unwrap(),
        GuestAddress(0x103000)
    );
}
#[test]
fn later_invalid_entry_keeps_original_ids_epoch_and_all_backings() {
    let mut m = MemoryManager::default();
    let a = m.register_backing(GuestAddress(0x100000), 8 * P).unwrap();
    let b = m.register_backing(GuestAddress(0x200000), P).unwrap();
    let s = m.create_space().unwrap();
    let r = m.reserve_fixed(s, GpuAddress(0x10000), 8 * P).unwrap();
    let id = m.map(s, r, a, 0).unwrap();
    let before = m.mappings(s, r).unwrap();
    let epoch = m.mapping_epoch(s).unwrap();
    for bad in [
        MapChange {
            reservation: r,
            address: GpuAddress(0x12000),
            size: 2 * P,
            backing: Some((b, 0)),
        },
        MapChange {
            reservation: r,
            address: GpuAddress(0x18000),
            size: P,
            backing: None,
        },
        MapChange {
            reservation: r,
            address: GpuAddress(0x12001),
            size: P,
            backing: None,
        },
        MapChange {
            reservation: r,
            address: GpuAddress(0x12000),
            size: 0,
            backing: None,
        },
        MapChange {
            reservation: r,
            address: GpuAddress(u64::MAX - P + 1),
            size: 2 * P,
            backing: None,
        },
    ] {
        assert!(
            m.apply_map_changes(
                s,
                &[
                    MapChange {
                        reservation: r,
                        address: GpuAddress(0x11000),
                        size: P,
                        backing: Some((b, 0))
                    },
                    bad
                ]
            )
            .is_err()
        );
        assert_eq!(m.mapping_epoch(s).unwrap(), epoch);
        assert_eq!(m.mappings(s, r).unwrap(), before);
        assert_eq!(
            m.address_map(s)
                .unwrap()
                .resolve(GpuAddress(0x11000), 1)
                .unwrap(),
            GuestAddress(0x101000)
        );
    }
    m.unmap(s, id).unwrap();
    m.release_backing(a).unwrap();
    m.release_backing(b).unwrap();
}
#[test]
fn reservation_ownership_and_ordered_replacements_are_checked() {
    let mut m = MemoryManager::default();
    let a = m.register_backing(GuestAddress(0x100000), P).unwrap();
    let b = m.register_backing(GuestAddress(0x200000), P).unwrap();
    let s = m.create_space().unwrap();
    let other = m.create_space().unwrap();
    let r = m.reserve_fixed(s, GpuAddress(0x10000), 2 * P).unwrap();
    let change = MapChange {
        reservation: r,
        address: GpuAddress(0x10000),
        size: P,
        backing: Some((a, 0)),
    };
    assert!(m.apply_map_changes(other, &[change]).is_err());
    assert!(m.mappings(s, r).unwrap().is_empty());
    m.apply_map_changes(
        s,
        &[
            change,
            MapChange {
                backing: Some((b, 0)),
                ..change
            },
        ],
    )
    .unwrap();
    assert_eq!(
        m.address_map(s)
            .unwrap()
            .resolve(GpuAddress(0x10000), 1)
            .unwrap(),
        GuestAddress(0x200000)
    );
    let epoch = m.mapping_epoch(s).unwrap();
    m.apply_map_changes(s, &[]).unwrap();
    assert_eq!(m.mapping_epoch(s).unwrap(), epoch);
    assert!(
        m.apply_map_changes(s, &vec![change; MAX_OBJECTS + 1])
            .is_err()
    );
    assert_eq!(m.mapping_epoch(s).unwrap(), epoch);
    m.apply_map_changes(
        s,
        &[MapChange {
            backing: None,
            ..change
        }],
    )
    .unwrap();
    assert!(
        m.address_map(s)
            .unwrap()
            .resolve(GpuAddress(0x10000), 1)
            .is_err()
    );
}
