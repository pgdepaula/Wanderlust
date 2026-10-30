use wl_gpu::shader_program::{FragmentProgram, Operation, VertexProgram};
const BIN: &[u8] = include_bytes!("fixtures/maxwell/texs-perspective.bin");
#[test]
fn independent_bound_texs_profile_and_runtime_handle_offset() {
    let p = FragmentProgram::decode(BIN).unwrap();
    assert!(p.uses_texture());
    assert_eq!(p.texture_input(), None);
    assert_eq!(p.bound_texture_offset(), Some(16));
    assert!(p.constants().is_empty());
    assert!(p.operations().contains(&Operation::SampleBound2d {
        destinations: [0, 2],
        u: 10,
        v: 11
    }));
    assert!(p.perspective());
    assert_eq!(p.required_registers(), 12);
    let text = p.wgsl();
    assert!(text.contains("textureSample(image,image_sampler"));
    assert!(!text.contains("textureSampleLevel"));
    #[cfg(feature = "wgpu-backend")]
    {
        let module = naga::front::wgsl::parse_str(&text).unwrap();
        naga::valid::Validator::new(
            naga::valid::ValidationFlags::all(),
            naga::valid::Capabilities::empty(),
        )
        .validate(&module)
        .unwrap();
    }
    let mut maximum = BIN.to_vec();
    let word = u64::from_le_bytes(maximum[152..160].try_into().unwrap());
    maximum[152..160]
        .copy_from_slice(&((word & !(0x1fffu64 << 36)) | (0x1fffu64 << 36)).to_le_bytes());
    assert_eq!(
        FragmentProgram::decode(&maximum)
            .unwrap()
            .bound_texture_offset(),
        Some(32764)
    );
    assert!(VertexProgram::decode(BIN).is_err());
}
#[test]
fn texs_profile_rejects_each_dependency_change_and_operand_modifier() {
    // Every used control bit belongs to the bounded profile, including waits,
    // read barriers, reuse hints, stalls and yield. No silent ignored fields.
    for (bundle, count) in [(0usize, 3usize), (1, 3), (2, 2)] {
        for slot in 0..count {
            for bit in 0..21 {
                let mut b = BIN.to_vec();
                let at = 80 + bundle * 32;
                let word = u64::from_le_bytes(b[at..at + 8].try_into().unwrap())
                    ^ (1 << (slot * 21 + bit));
                b[at..at + 8].copy_from_slice(&word.to_le_bytes());
                assert!(
                    FragmentProgram::decode(&b).is_err(),
                    "control {bundle}/{slot}/{bit}"
                );
            }
        }
    }
    for bit in [0u32, 16, 19, 28, 49, 50, 51, 52, 53, 57, 59] {
        let mut b = BIN.to_vec();
        let word = u64::from_le_bytes(b[152..160].try_into().unwrap()) ^ (1 << bit);
        b[152..160].copy_from_slice(&word.to_le_bytes());
        assert!(FragmentProgram::decode(&b).is_err(), "TEXS field {bit}");
    }
    // RCP must originate in position W; each multiply must use the correct IPA.
    for at in [121usize, 129, 137, 153, 154] {
        let mut b = BIN.to_vec();
        b[at] ^= 1;
        assert!(FragmentProgram::decode(&b).is_err(), "operand {at}");
    }
}
