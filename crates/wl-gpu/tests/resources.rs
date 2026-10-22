use wl_gpu::{
    fifo::{Decoder, MethodWrite},
    maxwell::*,
    memory::*,
    shader_program::*,
};
const LDC: &[u8] = include_bytes!("fixtures/maxwell/ldc.bin");
#[test]
fn independent_ldc_dataflow_and_naga() {
    let program = FragmentProgram::decode(LDC).unwrap();
    assert_eq!(
        program.constants(),
        &[
            ConstantRef {
                slot: 0,
                offset: 16
            },
            ConstantRef {
                slot: 0,
                offset: 20
            },
            ConstantRef {
                slot: 0,
                offset: 24
            },
            ConstantRef {
                slot: 0,
                offset: 28
            }
        ]
    );
    assert_eq!(program.operations().len(), 5);
    let module = naga::front::wgsl::parse_str(&program.wgsl()).unwrap();
    naga::valid::Validator::new(
        naga::valid::ValidationFlags::all(),
        naga::valid::Capabilities::empty(),
    )
    .validate(&module)
    .unwrap();
    for binary in [
        include_bytes!("fixtures/maxwell/red.bin"),
        include_bytes!("fixtures/maxwell/quarters.bin"),
    ] {
        FragmentProgram::decode(binary).unwrap();
    }
}
#[test]
fn rejected_resource_instruction_and_scoreboard_matrix() {
    for prefix in 0..LDC.len() {
        assert!(FragmentProgram::decode(&LDC[..prefix]).is_err());
    }
    for (offset, bit) in [
        (0, 1),
        (88, 1 << 16),
        (88, 1u64 << 49), // size4 B32 -> unverified size6 B128
        (88, 1u64 << 8),
        (88, 1u64 << 20),
        (88, 1u64 << 35),
        (80, 1u64 << 5),
        (80, 1u64 << 32),
        (80, 1u64 << 17),
        (120, 1u64 << 11),
        (120, 1u64 << 16),
        (136, 1u64 << 20),
    ] {
        let mut b = LDC.to_vec();
        let w = u64::from_le_bytes(b[offset..offset + 8].try_into().unwrap()) ^ bit;
        b[offset..offset + 8].copy_from_slice(&w.to_le_bytes());
        assert!(
            FragmentProgram::decode(&b).is_err(),
            "offset {offset} bit {bit:#x}"
        );
    }
    // Missing wait before a consuming MOV (slot1 in the second bundle).
    let mut b = LDC.to_vec();
    let ctrl = u64::from_le_bytes(b[112..120].try_into().unwrap()) & !(1u64 << 32);
    b[112..120].copy_from_slice(&ctrl.to_le_bytes());
    assert!(FragmentProgram::decode(&b).is_err());
    // Size4->size5 is now a verified B64 pair, not an unsupported mutation.
    // The later scalar R1 load intentionally overwrites its ready upper lane.
    let mut paired = LDC.to_vec();
    let word = u64::from_le_bytes(paired[88..96].try_into().unwrap()) ^ (1u64 << 48);
    paired[88..96].copy_from_slice(&word.to_le_bytes());
    let decoded = FragmentProgram::decode(&paired).unwrap();
    assert_eq!(
        decoded.constant_pairs(),
        &[ConstantRef {
            slot: 0,
            offset: 16
        }]
    );
    assert_eq!(
        decoded.constants(),
        FragmentProgram::decode(LDC).unwrap().constants()
    );
}
#[test]
fn fifo_constant_binding_snapshot_and_checked_access() {
    let mut fifo = Decoder::default();
    let mut engine = Maxwell3d::default();
    // Selector methods 0x2380/84/88; fragment binding group4 method0x2490, slot0.
    for word in [0x20010000, CLASS, 0x200308e0, 32, 0, 0x10000, 0x20010924, 1] {
        if let Some(w) = fifo.push_word(word).unwrap() {
            engine.apply(w).unwrap();
        }
    }
    fifo.finish().unwrap();
    engine
        .apply(MethodWrite {
            subchannel: 0,
            method: 0x2388,
            value: 0x20000,
        })
        .unwrap();
    let cb = engine.constant_buffer(4, 0).unwrap();
    assert_eq!(cb.address, GpuAddress(0x10000));
    let mut map = AddressMap::default();
    map.map(cb.address, GuestAddress(0), 32).unwrap();
    let bytes = [0xab; 32];
    let memory = SliceMemory {
        base: GuestAddress(0),
        bytes: &bytes,
    };
    assert_eq!(cb.read_word(28, &map, &memory).unwrap(), 0xabababab);
    for offset in [1, 30, 32, u32::MAX] {
        assert!(cb.read_word(offset, &map, &memory).is_err());
    }
    assert!(engine.constant_buffer(5, 0).is_err());
    assert!(engine.constant_buffer(4, 32).is_err());
    assert!(engine.constant_buffer(4, 1).is_err());
    engine
        .apply(MethodWrite {
            subchannel: 0,
            method: 0x2490,
            value: 0,
        })
        .unwrap();
    assert!(engine.constant_buffer(4, 0).is_err());
    let short = SliceMemory {
        base: GuestAddress(0),
        bytes: &bytes[..4],
    };
    assert!(cb.read_word(16, &map, &short).is_err());
    let bad = ConstantBuffer {
        address: GpuAddress(u64::MAX - 1),
        size: 32,
    };
    assert!(bad.read_word(4, &map, &memory).is_err());
}
#[test]
fn independently_assembled_tex_ipa_and_resource_provenance() {
    let binary = include_bytes!("fixtures/maxwell/tex.bin");
    let program = FragmentProgram::decode(binary).unwrap();
    assert_eq!(program.texture_input(), Some(0));
    assert_eq!(program.constants(), &[ConstantRef { slot: 0, offset: 0 }]);
    let m = naga::front::wgsl::parse_str(&program.wgsl()).unwrap();
    naga::valid::Validator::new(
        naga::valid::ValidationFlags::all(),
        naga::valid::Capabilities::empty(),
    )
    .validate(&m)
    .unwrap();
    for (offset, bit) in [
        (24, 1u64),
        (96, 1u64 << 54),
        (96, 1u64 << 51),
        (96, 1u64 << 28),
        (120, 1u64 << 37),
        (120, 1u64 << 31),
        (120, 1u64 << 28),
        (120, 1u64 << 50),
        (120, 1u64 << 16),
        (120, 1u64 << 20),
        (112, 1u64 << 11),
    ] {
        let mut b = binary.to_vec();
        let w = u64::from_le_bytes(b[offset..offset + 8].try_into().unwrap()) ^ bit;
        b[offset..offset + 8].copy_from_slice(&w.to_le_bytes());
        assert!(
            FragmentProgram::decode(&b).is_err(),
            "offset {offset} bit {bit:#x}"
        );
    }
}
#[test]
fn literal_pixel_program_state_and_faults() {
    let mut engine = Maxwell3d::default();
    let mut fifo = Decoder::default();
    for word in [
        0x20010000, CLASS, 0x20020582, 0, 0x50000, 0x20020850, 0x51, 0x100, 0x20020853, 8, 4,
    ] {
        if let Some(w) = fifo.push_word(word).unwrap() {
            engine.apply(w).unwrap();
        }
    }
    fifo.finish().unwrap();
    assert_eq!(
        engine.fragment_program().unwrap(),
        FragmentSource {
            address: GpuAddress(0x50100),
            registers: 8,
            group: 4
        }
    );
    assert!(
        engine
            .apply(MethodWrite {
                subchannel: 0,
                method: 0x2140,
                value: 0x41
            })
            .is_err()
    );
    assert!(
        engine
            .apply(MethodWrite {
                subchannel: 0,
                method: 0x2150,
                value: 5
            })
            .is_err()
    );
    engine
        .apply(MethodWrite {
            subchannel: 0,
            method: 0x2140,
            value: 0x50,
        })
        .unwrap();
    assert!(engine.fragment_program().is_err());
    engine
        .apply(MethodWrite {
            subchannel: 0,
            method: 0x2140,
            value: 0x51,
        })
        .unwrap();
    engine
        .apply(MethodWrite {
            subchannel: 0,
            method: 0x1608,
            value: 0xff,
        })
        .unwrap();
    engine
        .apply(MethodWrite {
            subchannel: 0,
            method: 0x160c,
            value: 0xffffff80,
        })
        .unwrap();
    assert!(engine.fragment_program().is_err());
    assert_eq!(
        FragmentProgram::decode(include_bytes!("fixtures/maxwell/tex.bin"))
            .unwrap()
            .required_registers(),
        7
    );
}

#[test]
fn inline_constant_ports_share_checked_auto_incrementing_selector_offset() {
    let mut engine = Maxwell3d::default();
    let mut apply = |method, value| {
        engine.apply(MethodWrite {
            subchannel: 0,
            method,
            value,
        })
    };
    apply(0, CLASS).unwrap();
    apply(0x238c, 0).unwrap();
    assert_eq!(apply(0x2390, 7), Err(MethodError::IncompleteConstantBuffer));
    for (m, v) in [(0x2380, 64), (0x2384, 0), (0x2388, 0x10000)] {
        apply(m, v).unwrap();
    }
    apply(0x2490, 1).unwrap(); // Snapshot remains independent of later selectors.
    // Every port advances the same offset; port number is not a byte address.
    for i in 0..16u16 {
        assert_eq!(
            apply(0x2390 + i * 4, u32::from(i)),
            Ok(MethodEffect::ConstantWrite {
                address: GpuAddress(0x10000 + u64::from(i) * 4),
                value: u32::from(i)
            })
        );
    }
    assert!(apply(0x2390, 0).is_err());
    apply(0x238c, 60).unwrap();
    for value in [1, 65536, u32::MAX] {
        assert!(apply(0x238c, value).is_err());
    }
    assert_eq!(
        apply(0x2390, 99),
        Ok(MethodEffect::ConstantWrite {
            address: GpuAddress(0x1003c),
            value: 99
        })
    );
    assert!(apply(0x23d0, 0).is_err());
    // Inclusive last valid 40-bit word, and overflowing selector range.
    for (m, v) in [
        (0x2380, 4),
        (0x2384, 255),
        (0x2388, 0xffff_fffc),
        (0x238c, 0),
    ] {
        apply(m, v).unwrap();
    }
    assert_eq!(
        apply(0x2390, 1),
        Ok(MethodEffect::ConstantWrite {
            address: GpuAddress((1 << 40) - 4),
            value: 1
        })
    );
    apply(0x2380, 8).unwrap();
    apply(0x238c, 0).unwrap();
    assert!(apply(0x2390, 1).is_err());
    assert_eq!(
        engine.constant_buffer(4, 0).unwrap().address,
        GpuAddress(0x10000)
    );
}
