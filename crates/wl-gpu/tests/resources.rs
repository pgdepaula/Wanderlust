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
        (88, 1u64 << 48),
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
