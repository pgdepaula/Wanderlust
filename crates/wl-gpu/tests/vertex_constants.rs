use wl_gpu::shader_program::{ConstantRef, FloatOperator, Operation, VertexProgram};
const BIN: &[u8] = include_bytes!("fixtures/maxwell/vertex-constant-alu.bin");
#[test]
fn assembled_vertex_constant_operands_remain_runtime_stage_inputs() {
    let program = VertexProgram::decode(BIN).unwrap();
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
            }
        ]
    );
    assert_eq!(
        program.operations()[2],
        Operation::FloatConstant {
            dst: 0,
            src: 0,
            input: 0,
            operator: FloatOperator::Add
        }
    );
    assert_eq!(program.required_registers(), 6);
    assert!(program.needs_uv_input());
    let source = program.wgsl();
    assert!(source.contains("vertex_cb.words[16][0]"));
    assert!(!source.contains("cb.words[0]"));
    let module = naga::front::wgsl::parse_str(&source).unwrap();
    naga::valid::Validator::new(
        naga::valid::ValidationFlags::all(),
        naga::valid::Capabilities::empty(),
    )
    .validate(&module)
    .unwrap();
}
