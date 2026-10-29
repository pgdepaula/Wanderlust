use wl_gpu::shader_program::{FragmentProgram, Operation};
const BIN: &[u8] = include_bytes!("fixtures/maxwell/multiply-rtz.bin");
#[test]
fn independently_assembled_rtz_product_has_typed_dataflow() {
    let p = FragmentProgram::decode(BIN).unwrap();
    assert!(p.operations().contains(&Operation::MultiplyRtz {
        dst: 4,
        left: 4,
        right: 5
    }));
    assert_eq!(p.required_registers(), 8);
    assert_eq!(p.constants().len(), 3);
    let text = p.wgsl();
    assert!(text.contains("r[4]=mul_ftz_rtz(r[4],r[5]);"));
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
}
#[test]
fn rtz_form_rejects_other_modifiers_unready_sources_and_zero_destination() {
    // Second bundle's first instruction, following three LDCs.
    for bit in [32u32, 40, 44, 45, 48, 49, 50, 13, 14] {
        let mut b = BIN.to_vec();
        let word = u64::from_le_bytes(b[120..128].try_into().unwrap()) ^ (1 << bit);
        b[120..128].copy_from_slice(&word.to_le_bytes());
        assert!(FragmentProgram::decode(&b).is_err(), "modifier {bit}");
    }
    let mut b = BIN.to_vec();
    b[120] = 255;
    assert!(FragmentProgram::decode(&b).is_err());
    let mut b = BIN.to_vec();
    b[121] = 42;
    assert!(FragmentProgram::decode(&b).is_err());
    let mut b = BIN.to_vec();
    b[112..120].copy_from_slice(
        &(u64::from_le_bytes(BIN[112..120].try_into().unwrap()) & !0x800).to_le_bytes(),
    );
    assert!(FragmentProgram::decode(&b).is_err());
}
