use wl_gpu::shader_program::{FragmentProgram, VertexProgram};
#[test]
fn unused_position_w_declaration_preserves_fragment_semantics() {
    for original in [
        include_bytes!("fixtures/maxwell/quarters.bin").as_slice(),
        include_bytes!("fixtures/maxwell/ldc.bin").as_slice(),
    ] {
        let expected = FragmentProgram::decode(original).unwrap();
        let mut binary = original.to_vec();
        binary[23] |= 0x80; // SPH word five: ImapSystemValuesB.PositionW.
        let actual = FragmentProgram::decode(&binary).unwrap();
        assert_eq!(actual, expected);
        assert_eq!(actual.wgsl(), expected.wgsl());
        for bit in 0..31 {
            let mut unsupported = binary.clone();
            unsupported[20 + bit / 8] ^= 1 << (bit % 8);
            assert!(FragmentProgram::decode(&unsupported).is_err());
        }
        // A declaration must not enable an IPA instruction outside its verified
        // generic-input mode, or silently replace a register-producing MOV.
        binary[0x58..0x60].copy_from_slice(&0xe003ff87cff7ff00u64.to_le_bytes());
        assert!(FragmentProgram::decode(&binary).is_err());
    }
}
#[test]
fn single_target_mrt_and_pixel_ignored_store_request_fields_preserve_semantics() {
    let original = include_bytes!("fixtures/maxwell/quarters.bin");
    let expected = FragmentProgram::decode(original).unwrap();
    for mrt in [false, true] {
        for start in [0u32, 17, 255] {
            for end in [0u32, 33, 255] {
                let mut binary = original.to_vec();
                let mut word = u32::from_le_bytes(binary[..4].try_into().unwrap());
                word |= u32::from(mrt) << 14;
                binary[..4].copy_from_slice(&word.to_le_bytes());
                binary[16..20].copy_from_slice(&((start << 12) | (end << 24)).to_le_bytes());
                let program = FragmentProgram::decode(&binary).unwrap();
                assert_eq!(program, expected);
                assert_eq!(program.wgsl(), expected.wgsl());
            }
        }
    }
}
#[test]
fn pixel_header_metadata_does_not_enable_other_interfaces_or_side_effects() {
    let original = include_bytes!("fixtures/maxwell/quarters.bin");
    for (offset, bit) in [
        (0usize, 15u32),
        (0, 16),
        (0, 26),
        (0, 27),
        (16, 0),
        (16, 20),
        (72, 4),
        (76, 0),
        (76, 1),
    ] {
        let mut binary = original.to_vec();
        let word = u32::from_le_bytes(binary[offset..offset + 4].try_into().unwrap()) ^ (1 << bit);
        binary[offset..offset + 4].copy_from_slice(&word.to_le_bytes());
        assert!(
            FragmentProgram::decode(&binary).is_err(),
            "field {offset}/{bit}"
        );
    }
    let mut vertex = include_bytes!("fixtures/maxwell/vertex.bin").to_vec();
    vertex[16..20].fill(0);
    assert!(VertexProgram::decode(&vertex).is_err());
}

#[test]
fn parked_self_branch_is_only_accepted_after_unconditional_exit() {
    let original = include_bytes!("fixtures/maxwell/quarters.bin");
    let expected = FragmentProgram::decode(original).unwrap();
    let mut binary = original.to_vec();
    let control = u64::from_le_bytes(binary[0x70..0x78].try_into().unwrap());
    binary[0x70..0x78]
        .copy_from_slice(&((control & !(0x1fffffu64 << 42)) | (0x7ffu64 << 42)).to_le_bytes());
    binary[0x88..0x90].copy_from_slice(&0xe2400fffff87000fu64.to_le_bytes());
    assert_eq!(FragmentProgram::decode(&binary).unwrap(), expected);
    let mut before_exit = binary.clone();
    before_exit[0x58..0x60].copy_from_slice(&0xe2400fffff87000fu64.to_le_bytes());
    assert!(FragmentProgram::decode(&before_exit).is_err());
    let mut different_target = binary.clone();
    different_target[0x88] ^= 0x10;
    assert!(FragmentProgram::decode(&different_target).is_err());
    binary[0x70] ^= 1; // This changes the executable MOV stall, still strict.
    assert!(FragmentProgram::decode(&binary).is_err());
}
