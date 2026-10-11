use wl_gpu::{Counters, command::*, format::Format, layout::BlockLinear2d, memory::*};

#[test]
fn mapped_addresses_borrow_and_alias_without_a_copy() {
    let data: Vec<u8> = (0..128).collect();
    let memory = SliceMemory {
        base: GuestAddress(0x1000),
        bytes: &data,
    };
    let mut map = AddressMap::default();
    map.map(GpuAddress(0x2000), GuestAddress(0x1000), 128)
        .unwrap();
    map.map(GpuAddress(0x3000), GuestAddress(0x1000), 128)
        .unwrap();
    let a = map.read(&memory, GpuAddress(0x2004), 8).unwrap();
    let b = map.read(&memory, GpuAddress(0x3004), 8).unwrap();
    assert_eq!(a.as_ptr(), b.as_ptr());
    assert_eq!(a, &data[4..12]);
    assert!(map.map(GpuAddress(0x207f), GuestAddress(0), 2).is_err());
    assert!(map.read(&memory, GpuAddress(0x207f), 2).is_err());
    assert!(map.map(GpuAddress(u64::MAX), GuestAddress(0), 2).is_err());
    assert!(map.map(GpuAddress(0), GuestAddress(u64::MAX), 2).is_err());
    assert!(memory.read(GuestAddress(u64::MAX), 2).is_err());
    assert!(memory.read(GuestAddress(0xfff), 1).is_err());
    assert!(memory.read(GuestAddress(0x1000), 129).is_err());
}

#[test]
fn dirty_ranges_coalesce_and_preserve_gaps() {
    let mut dirty = DirtyRanges::default();
    for r in [8..12, 0..4, 4..8, 16..20, 2..6] {
        dirty.insert(r).unwrap();
    }
    assert_eq!(dirty.ranges(), &[0..12, 16..20]);
    assert!(dirty.covers(2..10));
    assert!(!dirty.covers(8..17));
    assert!(dirty.insert(2..2).is_err());
    dirty.clear();
    assert!(dirty.ranges().is_empty());
}

#[test]
fn dirty_ranges_match_byte_oracle_for_all_small_intervals() {
    for a in 0..16 {
        for b in a + 1..=16 {
            for c in 0..16 {
                for d in c + 1..=16 {
                    let mut ranges = DirtyRanges::default();
                    ranges.insert(a..b).unwrap();
                    ranges.insert(c..d).unwrap();
                    for x in 0..16 {
                        assert_eq!(
                            ranges.covers(x..x + 1),
                            (a <= x && x < b) || (c <= x && x < d)
                        );
                    }
                }
            }
        }
    }
}

#[test]
fn format_pitch_and_compressed_edge_blocks() {
    let l = Format::Rgba8Unorm.layout(66, 19, 256).unwrap();
    assert_eq!(
        (l.row_bytes, l.row_pitch, l.rows, l.size),
        (264, 512, 19, 9728)
    );
    let l = Format::Bc1Rgba.layout(7, 5, 1).unwrap();
    assert_eq!((l.row_bytes, l.rows, l.size), (16, 2, 32));
    assert_eq!(Format::Astc4x4.layout(7, 5, 1).unwrap().size, 64);
    assert_eq!(Format::Rgba8Srgb.block(), Format::Rgba8Unorm.block());
    assert!(Format::Rgba8Unorm.layout(u32::MAX, 1, 256).is_err());
    assert!(Format::Rgba8Unorm.layout(0, 1, 256).is_err());
    assert!(Format::Rgba8Unorm.layout(4, 4, 3).is_err());
}

fn full_stream<'a>(v: &'a [u8], i: &'a [u8]) -> Vec<Command<'a>> {
    vec![
        Command::CreateBuffer {
            id: BufferId(1),
            size: v.len() as u64,
            kind: BufferKind::Vertex,
        },
        Command::CreateBuffer {
            id: BufferId(2),
            size: i.len() as u64,
            kind: BufferKind::Index,
        },
        Command::Upload {
            id: BufferId(1),
            offset: 0,
            data: v,
        },
        Command::Upload {
            id: BufferId(2),
            offset: 0,
            data: i,
        },
        Command::DrawIndexed {
            vertex: BufferId(1),
            index: BufferId(2),
            index_count: 12,
        },
    ]
}

#[test]
fn command_bounds_usage_initialization_and_ordering() {
    let (v, i) = test_geometry();
    let stream = full_stream(&v, &i);
    let state = validate(&stream, &ResourceState::new(), 4096).unwrap();
    assert_eq!(state.len(), 2);
    assert!(validate(&stream, &state, 4096).is_err()); // duplicate identity
    assert!(validate(&stream, &ResourceState::new(), 128).is_err()); // budget
    for cmd in [
        Command::Upload {
            id: BufferId(9),
            offset: 0,
            data: &[0; 4],
        },
        Command::Upload {
            id: BufferId(1),
            offset: 1,
            data: &[0; 4],
        },
        Command::Upload {
            id: BufferId(1),
            offset: u64::MAX - 3,
            data: &[0; 8],
        },
        Command::Upload {
            id: BufferId(1),
            offset: 192,
            data: &[0; 4],
        },
        Command::Upload {
            id: BufferId(1),
            offset: 0,
            data: &[0; 3],
        },
        Command::CreateBuffer {
            id: BufferId(3),
            size: 0,
            kind: BufferKind::Index,
        },
        Command::DrawIndexed {
            vertex: BufferId(2),
            index: BufferId(1),
            index_count: 12,
        },
        Command::DrawIndexed {
            vertex: BufferId(1),
            index: BufferId(2),
            index_count: 15,
        },
        Command::DrawIndexed {
            vertex: BufferId(1),
            index: BufferId(2),
            index_count: 1,
        },
        Command::DestroyBuffer { id: BufferId(9) },
    ] {
        assert!(validate(&[cmd], &state, 4096).is_err());
    }
    let mut late = stream.clone();
    late.push(Command::Upload {
        id: BufferId(1),
        offset: 0,
        data: &[0; 4],
    });
    assert!(validate(&late, &ResourceState::new(), 4096).is_err());
    let mut dead = stream.clone();
    dead.insert(4, Command::DestroyBuffer { id: BufferId(1) });
    assert!(validate(&dead, &ResourceState::new(), 4096).is_err());
    let mut uninitialized = stream.clone();
    uninitialized.remove(2);
    assert!(validate(&uninitialized, &ResourceState::new(), 4096).is_err());
    assert!(ResourceState::new().is_empty()); // preflight does not mutate caller
}

// Independent scalar byte-address oracle; test deliberately crosses GOB/block edges.
fn address(x: usize, y: usize, width_bytes: usize, bh: usize) -> usize {
    let gob = (y / (8 * bh)) * 512 * bh * width_bytes.div_ceil(64)
        + (x / 64) * 512 * bh
        + (y % (8 * bh) / 8) * 512;
    gob + (x % 64 / 32) * 256 + (y % 8 / 2) * 64 + (x % 32 / 16) * 32 + (y % 2) * 16 + x % 16
}

#[test]
fn tegra_conversion_matches_independent_address_oracle() {
    for (format, w, h) in [
        (Format::Rgba8Unorm, 37, 19),
        (Format::Bc1Rgba, 35, 41),
        (Format::Astc4x4, 29, 21),
    ] {
        for log2 in 0..=5 {
            let desc = BlockLinear2d::new(w, h, format, log2).unwrap();
            let (bw, bh, b) = format.block();
            let row = w.div_ceil(bw) as usize * b as usize;
            let rows = h.div_ceil(bh) as usize;
            let mut tiled = vec![0x99; desc.tiled_size];
            let mut linear = Vec::new();
            for y in 0..rows {
                for x in 0..row {
                    let value = ((x * 17 + y * 31) % 251) as u8;
                    tiled[address(x, y, row, 1 << log2)] = value;
                    linear.push(value);
                }
            }
            let mut counts = Counters::default();
            assert_eq!(desc.to_linear(&tiled, &mut counts).unwrap(), linear);
            assert_eq!(counts.texture_conversions, 1);
            assert_eq!(counts.guest_to_host_bytes, desc.linear_size as u64);
            assert!(
                desc.to_linear(&tiled[..tiled.len() - 1], &mut counts)
                    .is_err()
            );
        }
    }
    assert!(BlockLinear2d::new(u32::MAX, u32::MAX, Format::Astc4x4, 5).is_err());
    assert!(BlockLinear2d::new(4, 4, Format::Rgba8Unorm, 6).is_err());
}

#[test]
fn controlled_shader_validates_without_an_adapter() {
    let module = naga::front::wgsl::parse_str(include_str!("../src/draw.wgsl")).unwrap();
    naga::valid::Validator::new(
        naga::valid::ValidationFlags::all(),
        naga::valid::Capabilities::empty(),
    )
    .validate(&module)
    .unwrap();
}
