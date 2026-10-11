use wl_gpu::shader::ConstantFragment;
const RED: &[u8] = include_bytes!("fixtures/maxwell/red.bin");
const QUARTERS: &[u8] = include_bytes!("fixtures/maxwell/quarters.bin");
#[test]
fn independently_assembled_shader_translates_and_validates() {
    for (input, expected) in [(RED, [1., 0., 0., 1.]), (QUARTERS, [0.25, 0.5, 0.75, 1.])] {
        let fragment = ConstantFragment::decode(input).unwrap();
        assert_eq!(fragment.output_bits().map(f32::from_bits), expected);
        assert_eq!(fragment.instruction_count(), 5);
        let module = naga::front::wgsl::parse_str(&fragment.wgsl()).unwrap();
        naga::valid::Validator::new(
            naga::valid::ValidationFlags::all(),
            naga::valid::Capabilities::empty(),
        )
        .validate(&module)
        .unwrap();
    }
}
fn mutate_word(input: &[u8], offset: usize, change: impl FnOnce(u64) -> u64) -> Vec<u8> {
    let mut mutated = input.to_vec();
    let word = u64::from_le_bytes(mutated[offset..offset + 8].try_into().unwrap());
    mutated[offset..offset + 8].copy_from_slice(&change(word).to_le_bytes());
    mutated
}
#[test]
fn shader_input_contract_rejects_unsupported_semantics() {
    for n in 0..RED.len() {
        assert!(ConstantFragment::decode(&RED[..n]).is_err(), "length{n}");
    }
    assert!(ConstantFragment::decode(&vec![0; 0x50 + 4128]).is_err());
    let mut header = RED.to_vec();
    header[0] |= 1;
    assert!(ConstantFragment::decode(&header).is_err());
    let mut outputs = RED.to_vec();
    outputs[0x48] = 7;
    assert!(ConstantFragment::decode(&outputs).is_err());
    for (offset, mask) in [
        (0x50, 1),
        (0x58, 1 << 16),
        (0x58, 1 << 12),
        (0x58, 1 << 56),
        (0x58, 4),
        (0x80, 1 << 16),
    ] {
        assert!(ConstantFragment::decode(&mutate_word(RED, offset, |v| v ^ mask)).is_err());
    }
    // R3 becomes a second R2 write: export must not invent an uninitialized alpha.
    assert!(ConstantFragment::decode(&mutate_word(RED, 0x78, |v| (v & !0xff) | 2)).is_err());
    // Nonfinite raw float immediate; no integer-to-float reinterpretation workaround.
    assert!(
        ConstantFragment::decode(&mutate_word(RED, 0x58, |v| (v & !(u64::from(u32::MAX)
            << 20))
            | (0x7fc00000_u64 << 20)))
        .is_err()
    );
    let no_exit = mutate_word(RED, 0x80, |_| 0x010000000007f000);
    assert!(ConstantFragment::decode(&no_exit).is_err());
    assert!(ConstantFragment::decode(&mutate_word(RED, 0x88, |_| 0x010000000007f000)).is_err());
}
