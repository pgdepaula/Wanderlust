use std::{
    fs,
    path::PathBuf,
    sync::atomic::{AtomicU64, Ordering},
};
use wl_runtime::{
    Memory,
    storage::{MAX_READ, RomFs},
};

static NEXT: AtomicU64 = AtomicU64::new(0);
struct Image(PathBuf);
impl Image {
    fn new(bytes: &[u8]) -> Self {
        let path = std::env::temp_dir().join(format!(
            "wanderlust-romfs-{}-{}.bin",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        fs::write(&path, bytes).unwrap();
        Self(path)
    }
}
impl Drop for Image {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.0);
    }
}
fn image() -> Vec<u8> {
    // Literal container: metadata follows file data, which is legal.
    let fields = [0x50u64, 0x100, 4, 0x104, 24, 0x11c, 4, 0x120, 0, 0x50];
    let mut bytes = (0..0x120).map(|n| (n * 37 + 11) as u8).collect::<Vec<_>>();
    for (at, value) in fields.into_iter().enumerate() {
        bytes[at * 8..at * 8 + 8].copy_from_slice(&value.to_le_bytes());
    }
    bytes
}
fn guest_memory() -> Memory {
    let mut memory = Memory::default();
    memory
        .map(0x1000, vec![0xa5; 0x1000], true, true, false)
        .unwrap();
    memory
}

#[test]
fn file_backed_storage_reads_exact_bytes_and_signed_size() {
    let data = image();
    let file = Image::new(&data);
    let storage = RomFs::open(&file.0).unwrap();
    assert_eq!(storage.size(), 0x120);
    let mut memory = guest_memory();
    storage
        .read_into_guest(0x67, 37, 0x1013, 40, &mut memory)
        .unwrap();
    assert_eq!(memory.read_bytes(0x1013, 37).unwrap(), &data[0x67..0x8c]);
    assert_eq!(memory.load(0x1012, 1).unwrap(), 0xa5);
    assert_eq!(memory.load(0x1038, 1).unwrap(), 0xa5);
    storage
        .read_into_guest(0x120, 0, 0, 0, &mut memory)
        .unwrap();
    // Clones preserve the original open-file identity, not a fresh path lookup.
    storage
        .clone()
        .read_into_guest(0x50, 8, 0x1000, 8, &mut memory)
        .unwrap();
    assert_eq!(memory.read_bytes(0x1000, 8).unwrap(), &data[0x50..0x58]);
}

#[test]
fn malformed_geometry_and_tables_are_rejected() {
    for (field, value) in [
        (0, 0x40),
        (1, u64::MAX),
        (2, 3),
        (3, 0x100),
        (4, 23),
        (5, 0x500),
        (6, 0),
        (9, 0x121),
        (9, 0),
    ] {
        let mut data = image();
        data[field * 8..field * 8 + 8].copy_from_slice(&value.to_le_bytes());
        let file = Image::new(&data);
        assert!(RomFs::open(&file.0).is_err(), "field={field} value={value}");
    }
    let file = Image::new(&[0; 79]);
    assert!(RomFs::open(&file.0).is_err());
}

#[test]
fn invalid_reads_never_partially_write_guest_memory() {
    let file = Image::new(&image());
    let storage = RomFs::open(&file.0).unwrap();
    let mut memory = guest_memory();
    for (offset, size, address, buffer) in [
        (-1, 1, 0x1000, 1),
        (0, MAX_READ + 1, 0x1000, MAX_READ + 1),
        (0, 10, 0x1000, 9),
        (0x120, 1, 0x1000, 1),
        (i64::MAX, u64::MAX, 0x1000, u64::MAX),
        (0, 2, 0x1fff, 2),
        (0, 8, u64::MAX - 3, 8),
    ] {
        assert!(
            storage
                .read_into_guest(offset, size, address, buffer, &mut memory)
                .is_err()
        );
        assert!(
            memory
                .read_bytes(0x1000, 0x1000)
                .unwrap()
                .iter()
                .all(|b| *b == 0xa5)
        );
    }
}

#[test]
fn cross_region_write_is_checked_before_io_and_is_atomic() {
    let file = Image::new(&image());
    let storage = RomFs::open(&file.0).unwrap();
    let mut memory = guest_memory();
    memory
        .map(0x2000, vec![0x88; 0x1000], true, false, false)
        .unwrap();
    assert!(
        storage
            .read_into_guest(0x50, 8, 0x1ffc, 8, &mut memory)
            .is_err()
    );
    assert_eq!(memory.read_bytes(0x1ffc, 4).unwrap(), &[0xa5; 4]);
    let mut writable = guest_memory();
    writable
        .map(0x2000, vec![0x88; 0x1000], true, true, false)
        .unwrap();
    storage
        .read_into_guest(0x50, 8, 0x1ffc, 8, &mut writable)
        .unwrap();
    let mut actual = [0; 8];
    writable.read_into(0x1ffc, &mut actual).unwrap();
    assert_eq!(actual, image()[0x50..0x58]);
}

#[test]
fn truncated_open_file_does_not_zero_fill_or_modify_guest_memory() {
    let file = Image::new(&image());
    let storage = RomFs::open(&file.0).unwrap();
    fs::OpenOptions::new()
        .write(true)
        .open(&file.0)
        .unwrap()
        .set_len(0x80)
        .unwrap();
    let mut memory = guest_memory();
    assert!(
        storage
            .read_into_guest(0x50, 8, 0x1000, 8, &mut memory)
            .is_err()
    );
    assert_eq!(memory.read_bytes(0x1000, 8).unwrap(), &[0xa5; 8]);
}
