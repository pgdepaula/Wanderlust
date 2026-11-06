use wl_gpu::shader_program::{FragmentProgram, Operation, VertexProgram};
const BIN: &[u8] = include_bytes!("fixtures/maxwell/kill.bin");
fn instruction(b: &mut [u8], at: usize, word: u64) {
    b[at..at + 8].copy_from_slice(&word.to_le_bytes());
}
#[test]
fn enabled_kill_has_no_gpr_destination_and_lowers_to_valid_fragment_discard() {
    let program = FragmentProgram::decode(BIN).unwrap();
    assert!(
        program
            .operations()
            .contains(&Operation::Discard { condition: 0 })
    );
    assert_eq!(program.required_registers(), 6);
    let source = program.wgsl();
    assert!(source.contains("if (p0) { discard; }"));
    let module = naga::front::wgsl::parse_str(&source).unwrap();
    naga::valid::Validator::new(
        naga::valid::ValidationFlags::all(),
        naga::valid::Capabilities::empty(),
    )
    .validate(&module)
    .unwrap();
    assert!(VertexProgram::decode(BIN).is_err());
}
#[test]
fn disabled_kill_unknown_predicates_and_uninitialized_predicates_fail() {
    let mut bytes = BIN.to_vec();
    bytes[1] &= !0x80;
    let error = FragmentProgram::decode(&bytes).unwrap_err().to_string();
    assert!(error.contains("KIL"), "{error}");
    for condition in [1u64, 2, 6, 9, 14] {
        let mut bytes = BIN.to_vec();
        instruction(&mut bytes, 120, 0xe33000000000000f | condition << 16);
        assert!(FragmentProgram::decode(&bytes).is_err());
    }
    let mut bytes = BIN.to_vec();
    // Replace FSETP by an ordinary initialized GPR write. It cannot define P0.
    instruction(&mut bytes, 104, 0x010000000007f005);
    let control = u64::from_le_bytes(bytes[80..88].try_into().unwrap());
    instruction(&mut bytes, 80, (control & !(31u64 << 42)) | (6u64 << 42));
    let error = FragmentProgram::decode(&bytes).unwrap_err().to_string();
    assert!(error.contains("KIL"), "{error}");
    for condition in [7u64, 15] {
        instruction(&mut bytes, 120, 0xe33000000000000f | condition << 16);
        assert!(FragmentProgram::decode(&bytes).is_ok());
    }
}
#[test]
fn inverted_kill_and_header_without_kill_instruction_are_explicit() {
    let mut bytes = BIN.to_vec();
    instruction(&mut bytes, 120, 0xe33000000008000f);
    let source = FragmentProgram::decode(&bytes).unwrap().wgsl();
    assert!(source.contains("if (!p0) { discard; }"));
    let mut red = include_bytes!("fixtures/maxwell/red.bin").to_vec();
    red[1] |= 0x80;
    assert!(
        !FragmentProgram::decode(&red)
            .unwrap()
            .wgsl()
            .contains("discard;")
    );
}
