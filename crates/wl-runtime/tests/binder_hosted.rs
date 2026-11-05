extern crate wl_runtime as wl_guest_runtime;
#[path = "../hosted/binder.rs"]
pub mod binder;
use wl_runtime::{Memory, ipc::Request};
fn words(w: &[u32]) -> Vec<u8> {
    w.iter().flat_map(|v| v.to_le_bytes()).collect()
}
fn parcel(tail: &[u8]) -> Vec<u8> {
    let token = "android.gui.IGraphicBufferProducer";
    let mut p = words(&[0x100, token.len() as u32]);
    p.extend(token.encode_utf16().chain([0]).flat_map(u16::to_le_bytes));
    p.resize(p.len().next_multiple_of(4), 0);
    p.extend_from_slice(tail);
    let mut b = words(&[p.len() as u32, 16, 0, 16 + p.len() as u32]);
    b.extend(p);
    b
}
fn fresh_fence_reply(slot: u32) -> Vec<u8> {
    let mut b = words(&[slot, 1]);
    b.extend(flattened(&[0; 36]));
    b.extend(words(&[0]));
    b
}
fn flattened(b: &[u8]) -> Vec<u8> {
    let mut v = words(&[b.len() as u32, 0]);
    v.extend(b);
    v.resize(v.len().next_multiple_of(4), 0);
    v
}
fn buffer(width: u32, height: u32) -> Vec<u8> {
    words(&[
        0x47424652, width, height, width, 1, 0xb00, 42, 0, 0, 3, 0x1234, 0, 0x1000,
    ])
}
struct Harness {
    service: binder::Service,
    memory: Memory,
}
impl Harness {
    fn new() -> Self {
        let mut memory = Memory::default();
        memory
            .map(0x1000, vec![0; 0x5000], true, true, false)
            .unwrap();
        Self {
            service: binder::Service::default(),
            memory,
        }
    }
    fn call(&mut self, code: u32, tail: &[u8]) -> Vec<u8> {
        self.call_size(code, tail, 4096).unwrap()
    }
    fn call_size(&mut self, code: u32, tail: &[u8], out_size: u64) -> Result<Vec<u8>, String> {
        let b = parcel(tail);
        self.memory.write_bytes(0x1000, &b).unwrap();
        self.memory.write_bytes(0x4000, &vec![0xa5; 4096]).unwrap();
        let mut req = Request::default();
        req.sends = vec![(0, 0), (0x1000, b.len() as u64)];
        req.recvs = vec![(0, 0), (0x4000, out_size)];
        let reply = self
            .service
            .request(&mut self.memory, 3, &req, &words(&[32, code, 0]))?;
        assert_eq!(reply.code, 0);
        let len = self.memory.read_bytes(0x4000, 4).unwrap();
        let n = u32::from_le_bytes(len.try_into().unwrap()) as usize;
        assert_eq!(
            &self.memory.read_bytes(0x4004, 12).unwrap(),
            &words(&[16, 0, 16 + n as u32]).as_slice()
        );
        Ok(self.memory.read_bytes(0x4010, n).unwrap().to_vec())
    }
    fn connect(&mut self) {
        assert_eq!(
            self.call(10, &words(&[0, 1, 0])),
            words(&[1280, 720, 0, 0, 0])
        );
    }
    fn preallocate(&mut self, n: u32) {
        let mut b = words(&[n, 1]);
        b.extend(flattened(&buffer(37, 19)));
        assert!(self.call(14, &b).is_empty());
    }
    fn dequeue(&mut self) -> Vec<u8> {
        self.call(3, &words(&[1, 37, 19, 0, 0x300]))
    }
}
#[test]
fn connect_query_preallocate_dequeue_request_cancel_use_distinct_wire_replies() {
    let mut h = Harness::new();
    h.connect();
    assert_eq!(h.call(9, &words(&[2])), words(&[1, 0]));
    h.preallocate(0);
    h.preallocate(1);
    assert_eq!(h.call(2, &words(&[2])), words(&[0]));
    assert_eq!(h.dequeue(), fresh_fence_reply(0));
    let mut expected = words(&[1]);
    expected.extend(flattened(&buffer(37, 19)));
    expected.extend(words(&[0]));
    assert_eq!(h.call(1, &words(&[0])), expected);
    let mut cancel = words(&[0]);
    cancel.extend(flattened(&[0; 36]));
    assert!(h.call(8, &cancel).is_empty());
    assert_eq!(h.dequeue(), fresh_fence_reply(0));
    // The old generic response would have returned slot 1280 and no real buffer.
    assert_ne!(
        u32::from_le_bytes(h.dequeue()[..4].try_into().unwrap()),
        1280
    );
}
#[test]
fn invalid_slot_and_failed_atomic_reply_do_not_consume_a_free_buffer() {
    let mut h = Harness::new();
    h.connect();
    h.preallocate(0);
    assert_eq!(h.call(1, &words(&[1280])), words(&[0, (-22i32) as u32]));
    assert!(h.call_size(3, &words(&[1, 37, 19, 0, 0x300]), 16).is_err());
    assert_eq!(h.memory.read_bytes(0x4000, 64).unwrap(), vec![0xa5; 64]);
    assert_eq!(h.dequeue(), fresh_fence_reply(0));
    assert_eq!(h.dequeue(), words(&[u32::MAX, 0, (-11i32) as u32]));
    assert!(h.call_size(14, &words(&[32, 1]), 4096).is_err());
    assert_eq!(h.call(2, &words(&[0])), words(&[(-22i32) as u32]));
}
#[test]
fn queued_fences_are_retained_and_consumer_release_is_explicit_and_ordered() {
    let mut h = Harness::new();
    h.connect();
    h.preallocate(0);
    h.preallocate(1);
    h.dequeue();
    let mut input = vec![0u8; 84];
    input[8..12].copy_from_slice(&1u32.to_le_bytes());
    input[20..24].copy_from_slice(&37u32.to_le_bytes());
    input[24..28].copy_from_slice(&19u32.to_le_bytes());
    input[48..52].copy_from_slice(&1u32.to_le_bytes());
    input[52..56].copy_from_slice(&7u32.to_le_bytes());
    input[56..60].copy_from_slice(&123u32.to_le_bytes());
    let mut queue = words(&[0]);
    queue.extend(flattened(&input));
    assert_eq!(h.call(7, &queue), words(&[37, 19, 0, 1, 0]));
    let pending = h.service.pending().unwrap();
    assert_eq!(pending.slot, 0);
    assert_eq!(pending.buffer.stride, 37);
    assert_eq!(pending.input, input);
    assert_eq!(pending.sequence, 1);
    assert_eq!(h.dequeue(), fresh_fence_reply(1));
    assert!(h.service.release_consumed(2).is_err());
    assert_eq!(h.service.pending().unwrap().sequence, 1);
    h.service.release_consumed(1).unwrap();
    assert!(h.service.pending().is_none());
    let mut cancel = words(&[1]);
    cancel.extend(flattened(&[0; 36]));
    h.call(8, &cancel);
    assert_eq!(h.dequeue(), fresh_fence_reply(0));
    assert!(h.service.release_consumed(1).is_err());
}
#[test]
fn canceled_fence_is_forwarded_without_fabricating_gpu_completion() {
    let mut h = Harness::new();
    h.connect();
    h.preallocate(0);
    h.dequeue();
    let mut fence = [0u8; 36];
    fence[..4].copy_from_slice(&1u32.to_le_bytes());
    fence[4..8].copy_from_slice(&12u32.to_le_bytes());
    fence[8..12].copy_from_slice(&0xfffffff0u32.to_le_bytes());
    let mut cancel = words(&[0]);
    cancel.extend(flattened(&fence));
    h.call(8, &cancel);
    let mut expected = words(&[0, 1]);
    expected.extend(flattened(&fence));
    expected.extend(words(&[0]));
    assert_eq!(h.dequeue(), expected);
}
#[test]
fn malformed_parcels_addresses_and_incompatible_buffers_fail_without_state_changes() {
    let mut h = Harness::new();
    h.connect();
    let mut malformed = buffer(37, 19);
    malformed[32..36].copy_from_slice(&1u32.to_le_bytes());
    let mut tail = words(&[0, 1]);
    tail.extend(flattened(&malformed));
    assert!(h.call_size(14, &tail, 4096).is_err());
    assert_eq!(h.dequeue(), words(&[u32::MAX, 0, (-11i32) as u32]));
    h.preallocate(0);
    let mut req = Request::default();
    req.sends = vec![(0x1000, 24)];
    req.recvs = vec![(0x4000, 64)];
    for id in [0, 31, 33, u32::MAX] {
        assert!(
            h.service
                .request(&mut h.memory, 3, &req, &words(&[id, 9, 0]))
                .is_err()
        );
    }
    assert_eq!(h.dequeue(), fresh_fence_reply(0));
}

#[test]
fn native_surface_metadata_has_explicit_offsets_and_rejects_compression_and_planes() {
    let mut bytes = vec![0u8; 364];
    for (at, value) in [
        (0, 0x47424652u32),
        (4, 7),
        (8, 9),
        (12, 16),
        (16, 1),
        (20, 0xb00),
        (24, 42),
        (36, 81),
        (40, u32::MAX),
        (44, 7),
        (52, 0xdaffcaff),
        (56, 42),
        (64, 0xb00),
        (68, 1),
        (72, 1),
        (76, 16),
        (80, 1024),
        (84, 1),
        (92, 7),
        (96, 9),
        (100, 0x00532120),
        (104, 1),
        (108, 3),
        (112, 64),
        (116, 7),
        (120, 0x2000),
        (124, 0xfe),
        (128, 1),
        (144, 0x80000000),
        (148, 1024),
    ] {
        bytes[at..at + 4].copy_from_slice(&value.to_le_bytes());
    }
    let valid = binder::GraphicBuffer {
        bytes,
        width: 7,
        height: 9,
        stride: 16,
        format: 1,
        usage: 0xb00,
    };
    let surface = valid.native_surface().unwrap();
    assert_eq!(
        (
            surface.nvmap_id,
            surface.offset,
            surface.size,
            surface.pitch,
            surface.layout,
            surface.block_height_log2
        ),
        (7, 0x2000, 1024, 64, 3, 1)
    );
    for (at, value) in [
        (44, 0u32),
        (52, 0),
        (84, 2),
        (92, 8),
        (100, 0),
        (108, 2),
        (124, 1),
        (128, 6),
        (132, 1),
        (136, 1),
        (140, 1),
        (156, 1),
        (148, 64 * 1024 * 1024 + 1),
    ] {
        let mut bad = valid.clone();
        bad.bytes[at..at + 4].copy_from_slice(&value.to_le_bytes());
        assert!(bad.native_surface().is_err(), "field at {at:#x}");
    }
    let mut opaque = valid.clone();
    opaque.bytes[356..364].copy_from_slice(&0xdeadbeef12345678u64.to_le_bytes());
    assert_eq!(
        opaque.native_surface().unwrap().nvmap_id,
        7,
        "bookkeeping word is not a pointer to dereference"
    );
    let mut short = valid;
    short.bytes.pop();
    assert!(short.native_surface().is_err());
}
