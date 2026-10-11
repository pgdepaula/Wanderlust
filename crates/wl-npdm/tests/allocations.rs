//! Verification test demonstrating zero heap allocations during NPDM parsing and iteration.

use std::alloc::{GlobalAlloc, Layout, System};
use std::sync::atomic::{AtomicUsize, Ordering};
use wl_npdm::*;

struct CountingAlloc;
static ALLOC_COUNT: AtomicUsize = AtomicUsize::new(0);

unsafe impl GlobalAlloc for CountingAlloc {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        ALLOC_COUNT.fetch_add(1, Ordering::SeqCst);
        unsafe { System.alloc(layout) }
    }

    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        unsafe { System.dealloc(ptr, layout) }
    }
}

#[global_allocator]
static GLOBAL: CountingAlloc = CountingAlloc;

fn build_test_npdm() -> Vec<u8> {
    let mut valid = vec![0u8; 0x480];
    valid[0..4].copy_from_slice(b"META");
    valid[0x0C] = 0x07; // 64-bit instruction, 39-bit address space
    valid[0x0E] = 44;
    valid[0x0F] = 1;
    valid[0x14..0x18].copy_from_slice(&0x20_0000u32.to_le_bytes());
    valid[0x1C..0x20].copy_from_slice(&0x10_0000u32.to_le_bytes());
    valid[0x20..0x29].copy_from_slice(b"alloc-tst");
    valid[0x30..0x39].copy_from_slice(b"PROD-0001");
    valid[0x70..0x74].copy_from_slice(&0x380u32.to_le_bytes()); // ACI0 offset (0x380)
    valid[0x74..0x78].copy_from_slice(&0x100u32.to_le_bytes()); // ACI0 size (0x100)
    valid[0x78..0x7C].copy_from_slice(&0x80u32.to_le_bytes()); // ACID offset (0x80)
    valid[0x7C..0x80].copy_from_slice(&0x300u32.to_le_bytes()); // ACID size (0x300)

    // ACID at 0x80
    let acid_base = 0x80;
    valid[acid_base + 0x200..acid_base + 0x204].copy_from_slice(b"ACID");
    valid[acid_base + 0x204..acid_base + 0x208].copy_from_slice(&0x200u32.to_le_bytes()); // payload size (0x100 + 0x200 = 0x300)
    valid[acid_base + 0x20C..acid_base + 0x210].copy_from_slice(&1u32.to_le_bytes()); // retail
    valid[acid_base + 0x210..acid_base + 0x218]
        .copy_from_slice(&0x0100_0000_0000_0000u64.to_le_bytes());
    valid[acid_base + 0x218..acid_base + 0x220]
        .copy_from_slice(&0x0100_0000_0000_FFFFu64.to_le_bytes());

    // ACID subregions at 0x80 + 0x240:
    // FAC at 0x240, size 0x2C
    valid[acid_base + 0x220..acid_base + 0x224].copy_from_slice(&0x240u32.to_le_bytes());
    valid[acid_base + 0x224..acid_base + 0x228].copy_from_slice(&0x2Cu32.to_le_bytes());
    valid[acid_base + 0x240] = 1; // FAC version 1
    valid[acid_base + 0x244..acid_base + 0x24C].copy_from_slice(&0xFFu64.to_le_bytes());

    // ACID KAC at 0x270, size 8
    valid[acid_base + 0x230..acid_base + 0x234].copy_from_slice(&0x270u32.to_le_bytes());
    valid[acid_base + 0x234..acid_base + 0x238].copy_from_slice(&8u32.to_le_bytes());
    valid[acid_base + 0x270..acid_base + 0x274]
        .copy_from_slice(&(0b0111u32 | (50 << 4) | (20 << 10)).to_le_bytes());
    valid[acid_base + 0x274..acid_base + 0x278].copy_from_slice(&0xFFFF_FFFFu32.to_le_bytes());

    // ACI0 at 0x380
    let aci_base = 0x380;
    valid[aci_base..aci_base + 0x04].copy_from_slice(b"ACI0");
    valid[aci_base + 0x10..aci_base + 0x18]
        .copy_from_slice(&0x0100_0000_0000_1000u64.to_le_bytes());

    // ACI0 FAH at 0x40 (>= 0x40 header size), size 0x1C
    valid[aci_base + 0x20..aci_base + 0x24].copy_from_slice(&0x40u32.to_le_bytes());
    valid[aci_base + 0x24..aci_base + 0x28].copy_from_slice(&0x1Cu32.to_le_bytes());
    valid[aci_base + 0x40] = 1; // FAH version 1
    valid[aci_base + 0x44..aci_base + 0x4C].copy_from_slice(&0x0Fu64.to_le_bytes());

    // ACI0 KAC at 0x60, size 8
    valid[aci_base + 0x30..aci_base + 0x34].copy_from_slice(&0x60u32.to_le_bytes());
    valid[aci_base + 0x34..aci_base + 0x38].copy_from_slice(&8u32.to_le_bytes());
    valid[aci_base + 0x60..aci_base + 0x64]
        .copy_from_slice(&(0b0111u32 | (50 << 4) | (20 << 10)).to_le_bytes());
    valid[aci_base + 0x64..aci_base + 0x68].copy_from_slice(&0xFFFF_FFFFu32.to_le_bytes());

    valid
}

#[test]
fn test_zero_allocations_during_parse_and_access() {
    let raw = build_test_npdm();

    ALLOC_COUNT.store(0, Ordering::SeqCst);

    // 1. Parsing
    let npdm = Npdm::parse(&raw).expect("parse valid NPDM");
    let allocs_after_parse = ALLOC_COUNT.load(Ordering::SeqCst);
    assert_eq!(
        allocs_after_parse, 0,
        "Npdm::parse must perform 0 heap allocations"
    );

    // 2. Capabilities iteration
    let mut cap_count = 0;
    for cap in npdm.aci0_capabilities() {
        let _ = cap.unwrap();
        cap_count += 1;
    }
    for cap in npdm.acid_capabilities() {
        let _ = cap.unwrap();
        cap_count += 1;
    }
    assert!(cap_count > 0);

    // 3. Services iteration
    let mut svc_count = 0;
    for svc in npdm.aci0_services() {
        let _ = svc.unwrap();
        svc_count += 1;
    }
    for svc in npdm.acid_services() {
        let _ = svc.unwrap();
        svc_count += 1;
    }
    assert_eq!(svc_count, 0);

    // 4. Filesystem access
    let fs = npdm.filesystem_access().unwrap().expect("valid fs access");
    assert_eq!(fs.effective_permissions, 0x0F);

    let allocs_after_accessors = ALLOC_COUNT.load(Ordering::SeqCst);
    assert_eq!(
        allocs_after_accessors, 0,
        "all iterators and accessors must perform 0 heap allocations"
    );
}
