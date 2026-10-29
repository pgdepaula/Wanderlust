use wl_gpu::{initial_capture::*, memory::*};
fn wire() -> Vec<u8> {
    let mut b = b"WLGPFC01".to_vec();
    b.extend(17u32.to_le_bytes());
    b.extend(2u32.to_le_bytes());
    for v in [23u64, 29, 1, 31] {
        b.extend(v.to_le_bytes());
    }
    // One incrementing method packet split across GPFIFO entries.
    for (address, words) in [
        (0x1000u64, vec![0x20020010u32, 0x1234]),
        (0x2000, vec![0x5678]),
    ] {
        b.extend((address | ((words.len() as u64) << 42)).to_le_bytes());
        b.extend((words.len() as u32 * 4).to_le_bytes());
        for word in words {
            b.extend(word.to_le_bytes());
        }
    }
    b.extend(b"WLGPFEND");
    b
}
#[test]
fn literal_file_preserves_entry_boundaries_and_packet_continuation() {
    let snapshot = CommandSnapshot::decode(wire()).unwrap();
    assert_eq!(
        snapshot.identity(),
        SnapshotIdentity {
            process: 17,
            channel: 23,
            space: 29,
            sequence: 1,
            mapping_epoch: 31
        }
    );
    let mut decoder = wl_gpu::fifo::Decoder::default();
    let mut methods = Vec::new();
    for (_, payload) in snapshot.entries() {
        for raw in payload.chunks_exact(4) {
            if let Some(w) = decoder
                .push_word(u32::from_le_bytes(raw.try_into().unwrap()))
                .unwrap()
            {
                methods.push((w.method, w.value));
            }
        }
    }
    decoder.finish().unwrap();
    assert_eq!(methods, [(0x40, 0x1234), (0x44, 0x5678)]);
}
#[test]
fn malformed_files_are_rejected_before_exposing_payloads() {
    let b = wire();
    for end in 0..b.len() {
        assert!(
            CommandSnapshot::decode(b[..end].to_vec()).is_err(),
            "truncated {end}"
        );
    }
    for (offset, value) in [(0, b'?'), (12, 255), (48, 1), (56, 255)] {
        let mut bad = b.clone();
        bad[offset] = value;
        assert!(CommandSnapshot::decode(bad).is_err());
    }
    let mut bad = b;
    bad.push(0);
    assert!(CommandSnapshot::decode(bad).is_err());
    assert!(CommandSnapshot::decode(vec![0; 8 * 1024 * 1024 + 1]).is_err());
}
struct Memory(Vec<u8>);
impl GuestMemory for Memory {
    fn read(&self, a: GuestAddress, n: usize) -> wl_gpu::Result<&[u8]> {
        self.0
            .get(a.0 as usize..a.0 as usize + n)
            .ok_or_else(|| wl_gpu::Error("unmapped".into()))
    }
}
#[test]
fn writer_borrows_payload_and_failed_capture_cannot_publish_partial_file() {
    let dir = std::env::temp_dir().join(format!("wl-capture-unit-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let mut map = AddressMap::default();
    map.map(GpuAddress(0x1000), GuestAddress(0), 4096).unwrap();
    let entry = (0x1000u64 | (2u64 << 42)).to_le_bytes();
    let id = SnapshotIdentity {
        process: std::process::id(),
        channel: 1,
        space: 1,
        sequence: 1,
        mapping_epoch: 1,
    };
    let mut capture = InitialCapture::new(&dir);
    let memory = Memory(vec![1, 2, 3, 4, 5, 6, 7, 8]);
    capture.record(id, &entry, &map, &memory);
    assert_eq!(capture.counters().files, 1);
    assert_eq!(capture.counters().guest_borrowed_bytes, 8);
    let path = std::fs::read_dir(&dir)
        .unwrap()
        .next()
        .unwrap()
        .unwrap()
        .path();
    let parsed = CommandSnapshot::read(path).unwrap();
    assert_eq!(parsed.entries().next().unwrap().1, memory.0);
    // Same identity is never overwritten; a failure disables further captures.
    capture.record(id, &entry, &map, &Memory(vec![9; 8]));
    assert_eq!(capture.counters().failures, 1);
    capture.record(
        SnapshotIdentity { sequence: 2, ..id },
        &entry,
        &map,
        &memory,
    );
    assert_eq!(capture.counters().files, 1);
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn invalid_ranges_lengths_and_aggregate_work_are_not_diagnostic_successes() {
    let mut b = wire();
    // A command beginning at the final aligned 40-bit address with two words
    // would cross the GPU address domain even though its stored payload fits.
    b[48..56].copy_from_slice(&(((1u64 << 40) - 4) | (2u64 << 42)).to_le_bytes());
    assert!(CommandSnapshot::decode(b).is_err());
    let mut b = wire();
    b[56..60].copy_from_slice(&u32::MAX.to_le_bytes());
    assert!(CommandSnapshot::decode(b).is_err());
    // Declared entry count alone cannot force a huge allocation.
    let mut b = wire();
    b[12..16].copy_from_slice(&u32::MAX.to_le_bytes());
    assert!(CommandSnapshot::decode(b).is_err());
    // Large valid wire length is still outside this submission's work budget.
    let mut b = wire()[..48].to_vec();
    b[12..16].copy_from_slice(&1u32.to_le_bytes());
    b.extend((0x1000u64 | ((wl_gpu::gpfifo::MAX_WORDS + 1) << 42)).to_le_bytes());
    b.extend(((wl_gpu::gpfifo::MAX_WORDS + 1) as u32 * 4).to_le_bytes());
    b.extend(b"WLGPFEND");
    assert!(CommandSnapshot::decode(b).is_err());
}

#[test]
fn short_guest_read_removes_partial_record_without_retaining_borrow() {
    let dir = std::env::temp_dir().join(format!("wl-capture-short-{}", std::process::id()));
    let mut map = AddressMap::default();
    map.map(GpuAddress(0x1000), GuestAddress(0), 4096).unwrap();
    let mut capture = InitialCapture::new(&dir);
    let id = SnapshotIdentity {
        process: std::process::id(),
        channel: 2,
        space: 1,
        sequence: 1,
        mapping_epoch: 1,
    };
    let entry = (0x1000u64 | (2u64 << 42)).to_le_bytes();
    capture.record(id, &entry, &map, &Memory(vec![0; 4]));
    assert_eq!(capture.counters().files, 0);
    assert_eq!(capture.counters().failures, 1);
    assert_eq!(std::fs::read_dir(&dir).unwrap().count(), 0);
    std::fs::remove_dir_all(dir).unwrap();
}
