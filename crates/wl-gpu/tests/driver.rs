use wl_gpu::{driver::*, maxwell::Channel, memory::*};
fn request(entries: &[u8], legacy: bool) -> (u32, Vec<u8>) {
    let mut p = vec![0; 24];
    p[..8].copy_from_slice(&u64::MAX.to_le_bytes()); // ignored pointer
    p[8..12].copy_from_slice(&((entries.len() / 8) as u32).to_le_bytes());
    p[12..16].copy_from_slice(&4u32.to_le_bytes());
    if legacy {
        p.extend(entries);
        (0xc0004808 | ((p.len() as u32) << 16), p)
    } else {
        (KICKOFF_PB, p)
    }
}
#[test]
fn ioctl_wire_layout_and_strict_rejection() {
    let e = (0x10000u64 | (1 << 42)).to_le_bytes();
    for legacy in [false, true] {
        let (code, p) = request(&e, legacy);
        let extra = (!legacy).then_some(e.as_slice());
        assert_eq!(SubmitRequest::decode(code, &p, extra).unwrap().entries(), e);
        for n in 0..p.len() {
            assert!(SubmitRequest::decode(code, &p[..n], extra).is_err());
        }
        for bit in [0, 1, 2, 16, 29, 30, 31] {
            assert!(SubmitRequest::decode(code ^ (1 << bit), &p, extra).is_err());
        }
        for offset in [8, 12, 16, 20] {
            let mut bad = p.clone();
            bad[offset..offset + 4].copy_from_slice(&u32::MAX.to_le_bytes());
            assert!(SubmitRequest::decode(code, &bad, extra).is_err());
        }
        for flag in [0u32, 1, 2, 8, 16, 32, 256, 0x104] {
            let mut bad = p.clone();
            bad[12..16].copy_from_slice(&flag.to_le_bytes());
            assert!(SubmitRequest::decode(code, &bad, extra).is_err());
        }
        assert!(SubmitRequest::decode(code, &p, Some(&[])).is_err());
    }
    let entries = e.repeat(2048);
    let (code, p) = request(&entries, false);
    assert!(SubmitRequest::decode(code, &p, Some(&entries)).is_ok());
    let entries = e.repeat(2049);
    let (code, p) = request(&entries, false);
    assert!(SubmitRequest::decode(code, &p, Some(&entries)).is_err());
    let entries = e.repeat(513);
    let (code, p) = request(&entries, true);
    assert!(SubmitRequest::decode(code, &p, None).is_err());
}
#[test]
fn page_reservation_alias_mapping_and_generation() {
    let mut m = MemoryManager::default();
    let b = m.register_backing(GuestAddress(0x2000), 0x4000).unwrap();
    let s = m.create_space().unwrap();
    let t = m.create_space().unwrap();
    let r = m.reserve_fixed(s, GpuAddress(0x10000), 0x2000).unwrap();
    assert!(m.reserve_fixed(s, GpuAddress(0x11000), 0x1000).is_err());
    assert!(m.register_backing(GuestAddress(0x3000), 0x1000).is_err());
    for (a, n) in [
        (1, 4096),
        (4096, 0),
        (4096, 4095),
        (u64::MAX - 4095, 4096),
        (1 << 40, 4096),
    ] {
        assert!(m.reserve_fixed(s, GpuAddress(a), n).is_err());
    }
    assert!(m.map(t, r, b, 0).is_err());
    assert!(m.map(s, r, b, 1).is_err());
    assert!(m.map(s, r, b, 0x3000).is_err());
    let id = m.map(s, r, b, 0x1000).unwrap();
    assert_eq!(m.mapping_epoch(s).unwrap(), 1);
    assert_eq!(
        m.address_map(s)
            .unwrap()
            .resolve(GpuAddress(0x10004), 4)
            .unwrap(),
        GuestAddress(0x3004)
    );
    assert!(m.map(s, r, b, 0).is_err());
    assert!(m.release_backing(b).is_err());
    assert!(m.free_reservation(s, r).is_err());
    assert!(m.destroy_space(s).is_err());
    let alias = m.reserve_fixed(s, GpuAddress(0x20000), 0x2000).unwrap();
    let aid = m.map(s, alias, b, 0x1000).unwrap();
    assert_eq!(
        m.address_map(s)
            .unwrap()
            .resolve(GpuAddress(0x20004), 4)
            .unwrap(),
        GuestAddress(0x3004)
    );
    assert!(m.unmap(t, id).is_err());
    m.unmap(s, id).unwrap();
    assert!(m.unmap(s, id).is_err());
    assert!(
        m.address_map(s)
            .unwrap()
            .resolve(GpuAddress(0x10000), 4)
            .is_err()
    );
    let other = m.register_backing(GuestAddress(0x9000), 0x2000).unwrap();
    let newer = m.map(s, r, other, 0).unwrap();
    assert_ne!(newer, id);
    assert_eq!(
        m.address_map(s)
            .unwrap()
            .resolve(GpuAddress(0x10004), 4)
            .unwrap(),
        GuestAddress(0x9004)
    );
    assert_eq!(m.mapping_epoch(s).unwrap(), 4);
    m.unmap(s, newer).unwrap();
    m.unmap(s, aid).unwrap();
    m.free_reservation(s, r).unwrap();
    m.free_reservation(s, alias).unwrap();
    m.release_backing(b).unwrap();
    assert!(m.backing_range(b, 0, 4).is_err());
    m.destroy_space(s).unwrap();
    assert!(m.address_map(s).is_err());
}
#[test]
fn adjacent_mappings_remain_distinct_and_ids_cross_manager_fail() {
    let mut m = MemoryManager::default();
    let mut other = MemoryManager::default();
    let s = m.create_space().unwrap();
    let foreign = other.create_space().unwrap();
    assert!(m.address_map(foreign).is_err());
    let b = m.register_backing(GuestAddress(0), 8192).unwrap();
    for (a, o) in [(0x10000, 0), (0x11000, 4096)] {
        let r = m.reserve_fixed(s, GpuAddress(a), 4096).unwrap();
        m.map(s, r, b, o).unwrap();
    }
    assert!(
        m.address_map(s)
            .unwrap()
            .resolve(GpuAddress(0x10ffc), 8)
            .is_err()
    );
    assert!(m.backing_range(b, u64::MAX, 4).is_err());
    assert!(
        m.register_backing(GuestAddress(u64::MAX - 4095), 4096)
            .is_err()
    );
}
#[test]
fn literal_incrementing_blend_packet_includes_disabled_color_key() {
    // INC from 0x1340, count 7. The sixth word is COLOR_KEY, not a blend factor.
    let good = [0x200704d0, 1, 5, 6, 1, 2, 0, 6];
    let mut c = Channel::default();
    for w in [0x20010000, 0xb197] {
        c.push_word(w).unwrap();
    }
    for w in good {
        c.push_word(w).unwrap();
    }
    c.finish().unwrap();
    for key in [1, 2, u32::MAX] {
        let mut c = Channel::default();
        for w in [0x20010000, 0xb197] {
            c.push_word(w).unwrap();
        }
        for w in &good[..6] {
            c.push_word(*w).unwrap();
        }
        assert!(c.push_word(key).is_err());
        assert!(c.is_faulted());
    }
}
