extern crate wl_runtime as wl_guest_runtime;
#[path = "../src/runtime_adapter.rs"]
mod native;
use wl_gpu::memory::GuestAddress;
use wl_guest_runtime::{
    Memory,
    horizon::{attribute, state},
};
const BASE: u64 = 0x1000_0000;
fn region(m: &mut Memory, base: u64, bytes: usize, value: u8) {
    m.map_state(base, vec![value; bytes], true, true, false, state::NORMAL)
        .unwrap();
}
#[test]
fn registration_preserves_small_pins_and_rejects_incompatible_or_overlapping_memory() {
    // Compile the complete adapter surface without creating a GPU device.
    let _ = native::NativeHost::new;
    let _ = native::NativeHost::graphics_counters;
    let _ = native::NativeHost::driver_counters;
    let _ = native::NativeHost::capture_for_test;
    let mut m = Memory::default();
    region(&mut m, BASE, 8192, 0x31);
    let pin = native::pin_backing(&mut m, GuestAddress(BASE), 4096, 0).unwrap();
    assert!(native::pin_backing(&mut m, GuestAddress(BASE), 8192, u64::MAX).is_err());
    assert_eq!(m.load(BASE, 1).unwrap(), 0x31);
    assert!(!m.split_at(BASE + 2048));
    m.release_gpu_pin(pin).unwrap();
    m.regions[0].attribute = attribute::LOCKED;
    assert!(native::pin_backing(&mut m, GuestAddress(BASE), 8192, u64::MAX).is_err());
    m.regions[0].attribute = 0;
    assert!(native::pin_backing(&mut m, GuestAddress(BASE + 1), 4096, u64::MAX).is_err());
    assert!(native::pin_backing(&mut m, GuestAddress(u64::MAX - 4095), 8192, u64::MAX).is_err());
}
#[test]
#[ignore = "uses approximately 1 GiB peak host RAM; validates real large backing consolidation"]
fn large_registration_preserves_every_byte_and_runtime_pin_lifetime() {
    const PART: usize = 249 * 1024 * 1024;
    let mut m = Memory::default();
    // Both target boundaries lie inside regions shared with existing outside
    // pins. Joining must preserve those guards and their canonical bytes.
    region(&mut m, BASE - 4096, PART + 4096, 0x31);
    region(&mut m, BASE + PART as u64, PART + 4096, 0x97);
    let size = 2 * PART as u64;
    let left = m.pin_gpu_range(BASE - 4096, 4096).unwrap();
    let right = m.pin_gpu_range(BASE + size, 4096).unwrap();
    // Rejected budget must leave source regions and bytes intact.
    assert!(native::pin_backing(&mut m, GuestAddress(BASE), size, 256 * 1024 * 1024).is_err());
    assert_eq!(m.regions.len(), 2);
    let pin = native::pin_backing(&mut m, GuestAddress(BASE), size, 1024 * 1024 * 1024).unwrap();
    assert_eq!(m.regions.len(), 3);
    let bytes = m.read_bytes(BASE, 2 * PART).unwrap();
    assert!(bytes[..PART].iter().all(|v| *v == 0x31));
    assert!(bytes[PART..].iter().all(|v| *v == 0x97));
    assert!(m.ensure_gpu_unpinned(BASE, size).is_err());
    assert!(!m.split_at(BASE + PART as u64));
    assert!(native::pin_backing(&mut m, GuestAddress(BASE), 4096, u64::MAX).is_err());
    m.store(BASE + PART as u64 - 4, 8, 0x0123456789abcdef)
        .unwrap();
    assert_eq!(
        m.load(BASE + PART as u64 - 4, 8).unwrap(),
        0x0123456789abcdef
    );
    m.release_gpu_pin(pin).unwrap();
    assert!(m.release_gpu_pin(pin).is_err());
    assert!(m.split_at(BASE + PART as u64));
    assert_eq!(m.load(BASE - 8, 8).unwrap(), 0x3131313131313131);
    assert_eq!(m.load(BASE + size, 8).unwrap(), 0x9797979797979797);
    assert!(m.ensure_gpu_unpinned(BASE - 4096, 4096).is_err());
    assert!(m.ensure_gpu_unpinned(BASE + size, 4096).is_err());
    m.release_gpu_pin(left).unwrap();
    m.release_gpu_pin(right).unwrap();
}
