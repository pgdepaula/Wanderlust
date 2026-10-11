use wl_gpu::fifo::{DecodeError, Decoder, MethodWrite};
use wl_gpu::memory::{AddressMap, GpuAddress, GuestAddress, SliceMemory};

// Literal packets cross-checked against NVIDIA B06F field positions, not an encoder
// implemented by this crate. Payload words deliberately resemble packet headers.
const PACKETS: [u32; 13] = [
    0,
    0x2003_6020,
    0xffff_ffff,
    0x8001_0004,
    0,
    0x6002_a030,
    17,
    18,
    0xa003_e040,
    19,
    20,
    21,
    0x9fff_2050,
];

fn expected() -> Vec<MethodWrite> {
    [
        (3, 0x80, 0xffff_ffff),
        (3, 0x84, 0x8001_0004),
        (3, 0x88, 0),
        (5, 0xc0, 17),
        (5, 0xc0, 18),
        (7, 0x100, 19),
        (7, 0x104, 20),
        (7, 0x104, 21),
        (1, 0x140, 0x1fff),
    ]
    .into_iter()
    .map(|(subchannel, method, value)| MethodWrite {
        subchannel,
        method,
        value,
    })
    .collect()
}

#[test]
fn literal_method_packets_preserve_full_payloads() {
    let mut decoder = Decoder::default();
    let mut writes = Vec::new();
    for word in PACKETS {
        if let Some(write) = decoder.push_word(word).unwrap() {
            writes.push(write);
        }
    }
    assert_eq!(writes, expected());
    decoder.finish().unwrap();
}

#[test]
fn guest_borrow_and_every_word_chunk_split_preserve_packet_state() {
    let bytes: Vec<_> = PACKETS.into_iter().flat_map(u32::to_le_bytes).collect();
    let memory = SliceMemory {
        base: GuestAddress(0x1000),
        bytes: &bytes,
    };
    let mut map = AddressMap::default();
    map.map(GpuAddress(0x2000), GuestAddress(0x1000), bytes.len() as u64)
        .unwrap();
    for split in 0..=PACKETS.len() {
        let mut decoder = Decoder::default();
        let mut writes = Vec::new();
        for (start, end) in [(0, split * 4), (split * 4, bytes.len())] {
            if start == end {
                continue;
            }
            let borrowed = map
                .read(&memory, GpuAddress(0x2000 + start as u64), end - start)
                .unwrap();
            assert_eq!(borrowed.as_ptr(), bytes[start..].as_ptr());
            for word in borrowed.chunks_exact(4) {
                if let Some(write) = decoder
                    .push_word(u32::from_le_bytes(word.try_into().unwrap()))
                    .unwrap()
                {
                    writes.push(write);
                }
            }
        }
        decoder.finish().unwrap();
        assert_eq!(writes, expected(), "split at word {split}");
    }
}

#[test]
fn rejects_unsupported_headers_and_overflow_without_consuming_payloads() {
    let mut d = Decoder::default();
    for (word, opcode) in [(4, 0), (0x4000_0000, 2), (0xc000_0000, 6), (0xe000_0000, 7)] {
        assert_eq!(
            d.push_word(word),
            Err(DecodeError::UnsupportedOpcode(opcode))
        );
        d.finish().unwrap();
    }
    for word in [0x2002_0fff, 0xa002_0fff, 0x3fff_0000] {
        assert_eq!(d.push_word(word), Err(DecodeError::MethodOverflow));
        d.finish().unwrap();
    }
    // Zero-count new-format packets have no payload, even at the last method.
    for word in [0x2000_0fff, 0x6000_0fff, 0xa000_0fff] {
        assert_eq!(d.push_word(word).unwrap(), None);
        d.finish().unwrap();
    }
    for word in [0x2001_0fff, 0xa001_0fff, 0x6001_1fff] {
        // Bit 12 is outside the new-format method/subchannel fields.
        assert_eq!(d.push_word(word).unwrap(), None);
        assert_eq!(d.push_word(123).unwrap().unwrap().method, 0x3ffc);
        d.finish().unwrap();
    }
}

#[test]
fn maximum_counts_and_truncation_are_bounded() {
    let mut d = Decoder::default();
    d.push_word(0x7fff_0fff).unwrap(); // 8191 nonincrementing writes to last method
    for n in 0..8191 {
        let write = d.push_word(n).unwrap().unwrap();
        assert_eq!(write.method, 0x3ffc);
        assert_eq!(write.value, n);
    }
    d.finish().unwrap();
    d.push_word(0x3000_0000).unwrap(); // 4096 incrementing writes, exactly 12-bit span
    for n in 0..4096 {
        assert_eq!(d.push_word(n).unwrap().unwrap().method, (n * 4) as u16);
    }
    d.finish().unwrap();
    d.push_word(0xa003_0000).unwrap();
    assert_eq!(d.finish(), Err(DecodeError::Truncated { remaining: 3 }));
    assert_eq!(d.push_word(10).unwrap().unwrap().method, 0);
    assert_eq!(d.finish(), Err(DecodeError::Truncated { remaining: 2 }));
    assert_eq!(d.push_word(11).unwrap().unwrap().method, 4);
    assert_eq!(d.push_word(12).unwrap().unwrap().method, 4);
    d.finish().unwrap();
}
