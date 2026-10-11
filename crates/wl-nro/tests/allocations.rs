use std::alloc::{GlobalAlloc, Layout, System};
use std::sync::atomic::{AtomicUsize, Ordering};
use wl_nro::{MAGIC_ASET, MAGIC_MOD0, MAGIC_NRO, Nro};

struct CountingAllocator;

static ALLOCATED_BYTES: AtomicUsize = AtomicUsize::new(0);
static ALLOC_COUNT: AtomicUsize = AtomicUsize::new(0);
static TRACKING_ACTIVE: AtomicUsize = AtomicUsize::new(0);

unsafe impl GlobalAlloc for CountingAllocator {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        if TRACKING_ACTIVE.load(Ordering::SeqCst) != 0 {
            ALLOCATED_BYTES.fetch_add(layout.size(), Ordering::SeqCst);
            ALLOC_COUNT.fetch_add(1, Ordering::SeqCst);
        }
        unsafe { System.alloc(layout) }
    }

    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        unsafe { System.dealloc(ptr, layout) }
    }
}

#[global_allocator]
static GLOBAL: CountingAllocator = CountingAllocator;

fn reset_alloc_tracking() {
    ALLOCATED_BYTES.store(0, Ordering::SeqCst);
    ALLOC_COUNT.store(0, Ordering::SeqCst);
}

fn start_alloc_tracking() {
    reset_alloc_tracking();
    TRACKING_ACTIVE.store(1, Ordering::SeqCst);
}

fn stop_alloc_tracking() -> (usize, usize) {
    TRACKING_ACTIVE.store(0, Ordering::SeqCst);
    (
        ALLOCATED_BYTES.load(Ordering::SeqCst),
        ALLOC_COUNT.load(Ordering::SeqCst),
    )
}

fn make_synthetic_nro_with_aset() -> Vec<u8> {
    let mut buf = vec![0u8; 0x3000];
    buf[0x00..0x04].copy_from_slice(&0x14000020u32.to_le_bytes());
    buf[0x04..0x08].copy_from_slice(&0x80u32.to_le_bytes());
    buf[0x10..0x14].copy_from_slice(&MAGIC_NRO);
    buf[0x14..0x18].copy_from_slice(&0u32.to_le_bytes());
    buf[0x18..0x1C].copy_from_slice(&0x3000u32.to_le_bytes());
    buf[0x20..0x24].copy_from_slice(&0u32.to_le_bytes());
    buf[0x24..0x28].copy_from_slice(&0x1000u32.to_le_bytes());
    buf[0x28..0x2C].copy_from_slice(&0x1000u32.to_le_bytes());
    buf[0x2C..0x30].copy_from_slice(&0x1000u32.to_le_bytes());
    buf[0x30..0x34].copy_from_slice(&0x2000u32.to_le_bytes());
    buf[0x34..0x38].copy_from_slice(&0x1000u32.to_le_bytes());
    buf[0x38..0x3C].copy_from_slice(&0x1000u32.to_le_bytes());
    buf[0x80..0x84].copy_from_slice(&MAGIC_MOD0);

    let mut aset = vec![0u8; 0x1000];
    aset[0x00..0x04].copy_from_slice(&MAGIC_ASET);
    aset[0x08..0x10].copy_from_slice(&0x38u64.to_le_bytes());
    aset[0x10..0x18].copy_from_slice(&0x100u64.to_le_bytes());
    aset[0x18..0x20].copy_from_slice(&0x138u64.to_le_bytes());
    aset[0x20..0x28].copy_from_slice(&0x200u64.to_le_bytes());
    buf.extend_from_slice(&aset);
    buf
}

#[test]
fn test_zero_allocations_during_parse_and_slice_access() {
    let bytes = make_synthetic_nro_with_aset();

    start_alloc_tracking();

    let nro = Nro::parse(&bytes).expect("valid NRO must parse");
    let text = nro.text_slice();
    let ro = nro.ro_slice();
    let data = nro.data_slice();
    let assets = nro.assets();
    let nacp = assets.and_then(|a| a.nacp_bytes);

    // Prevent compiler from optimizing away the reads
    std::hint::black_box((
        nro.header.magic,
        text.len(),
        ro.len(),
        data.len(),
        nacp.map(|n| n.len()),
    ));

    let (bytes_allocated, alloc_count) = stop_alloc_tracking();

    assert_eq!(
        alloc_count, 0,
        "Nro::parse and slice access must perform zero allocations, but performed {alloc_count} allocations"
    );
    assert_eq!(
        bytes_allocated, 0,
        "Nro::parse and slice access must allocate 0 bytes, but allocated {bytes_allocated} bytes"
    );
}
