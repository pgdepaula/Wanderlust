use wl_gpu::{gpfifo::*, maxwell::Channel, memory::*};
fn raw(lo: u32, hi: u32) -> [u8; 8] {
    let mut b = [0; 8];
    b[..4].copy_from_slice(&lo.to_le_bytes());
    b[4..].copy_from_slice(&hi.to_le_bytes());
    b
}
#[test]
fn literal_field_boundaries_and_control_rejection() {
    let e = Entry::decode([0xfc, 0xff, 0xff, 0xff, 0xab, 0x04, 0, 0]).unwrap();
    assert_eq!(e.address(), GpuAddress(0x00ab_ffff_fffc));
    assert_eq!((e.words(), e.byte_len()), (1, 4));
    let e = Entry::decode(raw(0, 0x7fff_fc00)).unwrap();
    assert_eq!((e.words(), e.byte_len()), (WIRE_MAX_WORDS, 8_388_604));
    assert!(Entry::decode(raw(0xffff_fffc, 0x4ff)).is_ok());
    assert_eq!(
        Entry::decode(raw(0xffff_fffc, 0x8ff)),
        Err(EntryError::AddressRange)
    );
    for (lo, hi, error) in [
        (1, 1024, EntryError::ConditionalFetch),
        (2, 1024, EntryError::ReservedBit),
        (0, 1280, EntryError::Privileged),
        (0, 0, EntryError::ControlEntry),
        (0, 3, EntryError::ControlEntry),
    ] {
        assert_eq!(Entry::decode(raw(lo, hi)), Err(error));
    }
}
#[test]
fn level_and_sync_are_distinct_from_address_length_privilege_and_control() {
    for level in [0, 0x200] {
        for sync in [0, 0x8000_0000] {
            let e = Entry::decode(raw(0x1234, 0x401 | level | sync)).unwrap();
            assert_eq!(e.address(), GpuAddress(0x100001234));
            assert_eq!(e.byte_len(), 4);
            assert_eq!(e.subroutine_level(), level != 0);
            assert_eq!(e.sync_wait(), sync != 0);
            assert_eq!(
                Entry::decode(raw(0x1234, 0x501 | level | sync)),
                Err(EntryError::Privileged)
            );
            assert_eq!(
                Entry::decode(raw(0x1234, 1 | level | sync)),
                Err(EntryError::ControlEntry)
            );
        }
    }
}
#[test]
fn metadata_budget_and_trailing_bytes() {
    assert_eq!(Submission::new(&[]).unwrap().entry_count(), 0);
    assert!(Submission::new(&[0; 7]).is_err());
    let one = raw(0, MAX_WORDS as u32 * 1024);
    assert_eq!(Submission::new(&one).unwrap().word_count(), MAX_WORDS);
    assert!(Submission::new(&raw(0, (MAX_WORDS as u32 + 1) * 1024)).is_err());
    let mut many = raw(0, 1024).repeat(MAX_ENTRIES);
    assert_eq!(Submission::new(&many).unwrap().entry_count(), MAX_ENTRIES);
    many.extend(raw(0, 1024));
    assert!(Submission::new(&many).is_err());
    assert!(Submission::new(&[one.as_slice(), raw(0, 1024).as_slice()].concat()).is_err());
}
#[test]
fn mapped_entry_must_fit_one_contiguous_mapping() {
    let mut m = AddressMap::default();
    m.map(GpuAddress(0x1000), GuestAddress(0), 4).unwrap();
    m.map(GpuAddress(0x1004), GuestAddress(4), 4).unwrap();
    let e = Entry::decode(raw(0x1000, 2048)).unwrap();
    assert!(m.resolve(e.address(), e.byte_len()).is_err());
    assert!(m.resolve(GpuAddress(0x1000), 4).is_ok());
    assert!(m.resolve(GpuAddress(0x2000), 4).is_err());
}
#[test]
fn channel_continues_packet_and_latches_real_engine_failure() {
    let mut c = Channel::default();
    c.push_word(0x20010000).unwrap(); // SET_OBJECT header, value in another entry.
    assert!(c.fifo.finish().is_err());
    c.push_word(0xb197).unwrap();
    c.finish().unwrap();
    c.push_word(0x20010000).unwrap();
    assert!(c.push_word(0xdead).is_err());
    assert!(c.is_faulted());
    assert!(c.push_word(0).is_err());
    assert!(c.finish().is_err());
}
