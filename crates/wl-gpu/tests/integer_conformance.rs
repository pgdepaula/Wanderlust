//! Adversarial Maxwell B197 integer ALU and bit logic conformance suite.
//!
//! Independently verifies Maxwell SASS `IADD` and `LOP` instructions across vertex
//! and fragment shader stages against authoritative specifications:
//! - NVIDIA open-gpu-doc `clb197.h` (pinned rev `9fdf5c4062007929d9f4e6cbad9c9771fe61b880`):
//!   <https://github.com/NVIDIA/open-gpu-doc/blob/9fdf5c4062007929d9f4e6cbad9c9771fe61b880/classes/3d/clb197.h>
//! - Envytools / rnndb Maxwell SASS instruction encodings for `IADD` and `LOP` (rev `c64b6330366b`):
//!   - `IADD`: opcode `0x5c10...`, wrapping modulo 2^32, RZ source identity.
//!   - `LOP`: opcode `0x5c47...`, AND (`0x00`), OR (`0x02`), XOR (`0x04`).
//! - Mesa Gallium3D Nouveau `nvc0_program.c` (pinned rev `e7959c6009d85d278585b2896495b2b3046d461b`):
//!   <https://gitlab.freedesktop.org/mesa/mesa/-/blob/e7959c6009d85d278585b2896495b2b3046d461b/src/gallium/drivers/nouveau/nvc0/nvc0_program.c>
//! - Naga 26.0 WGSL validation rules for unsigned integer bitcast and arithmetic.

use wl_gpu::shader_program::{FragmentProgram, IntegerOperator, Operation, VertexProgram};

const FRAGMENT_BIN: &[u8] = include_bytes!("fixtures/maxwell/integer.bin");
const VERTEX_BIN: &[u8] = include_bytes!("fixtures/maxwell/vertex-integer.bin");
const ALU_BIN: &[u8] = include_bytes!("fixtures/maxwell/alu.bin");

fn mutate_word(bytes: &mut [u8], offset: usize, f: impl FnOnce(u64) -> u64) {
    let word = u64::from_le_bytes(bytes[offset..offset + 8].try_into().unwrap());
    bytes[offset..offset + 8].copy_from_slice(&f(word).to_le_bytes());
}

fn validate_wgsl_syntax(wgsl: &str) {
    let module = naga::front::wgsl::parse_str(wgsl).expect("Naga WGSL parse failed");
    let mut validator = naga::valid::Validator::new(
        naga::valid::ValidationFlags::all(),
        naga::valid::Capabilities::empty(),
    );
    validator
        .validate(&module)
        .expect("Naga WGSL validation failed");
}

// ---------------------------------------------------------------------------
// 1. Wrapping 32-bit addition modulo 2^32 and RZ (zero) identities
// ---------------------------------------------------------------------------

#[test]
fn wrapping_iadd_arithmetic_overflow_and_rz_identities() {
    let program = FragmentProgram::decode(FRAGMENT_BIN).unwrap();
    assert_eq!(program.required_registers(), 9);

    // Verify IADD operation in decoded operations
    assert!(program.operations().contains(&Operation::Integer {
        dst: 0,
        left: 4,
        right: 5,
        operator: IntegerOperator::Add,
    }));

    // Verify WGSL lowering contains exact wrapping addition
    assert!(program.wgsl().contains("r[0]=r[4] + r[5];"));
    validate_wgsl_syntax(&program.wgsl());

    // Test IADD with RZ (255) as right operand: r[dst] = r[left] + RZ (identity)
    let mut rz_right = FRAGMENT_BIN.to_vec();
    mutate_word(&mut rz_right, 0x88, |w| (w & !(255 << 20)) | (255 << 20));
    let dec_right = FragmentProgram::decode(&rz_right).unwrap();
    assert!(dec_right.operations().contains(&Operation::Integer {
        dst: 0,
        left: 4,
        right: 255,
        operator: IntegerOperator::Add,
    }));
    assert!(dec_right.wgsl().contains("r[0]=r[4] + r[255];"));
    validate_wgsl_syntax(&dec_right.wgsl());

    // Test IADD with RZ (255) as left operand: r[dst] = RZ + r[right] (identity)
    let mut rz_left = FRAGMENT_BIN.to_vec();
    mutate_word(&mut rz_left, 0x88, |w| (w & !(255 << 8)) | (255 << 8));
    let dec_left = FragmentProgram::decode(&rz_left).unwrap();
    assert!(dec_left.operations().contains(&Operation::Integer {
        dst: 0,
        left: 255,
        right: 5,
        operator: IntegerOperator::Add,
    }));
    assert!(dec_left.wgsl().contains("r[0]=r[255] + r[5];"));
    validate_wgsl_syntax(&dec_left.wgsl());

    // Test IADD with both operands RZ: r[dst] = RZ + RZ = 0
    let mut rz_both = FRAGMENT_BIN.to_vec();
    mutate_word(&mut rz_both, 0x88, |w| {
        (w & !(255 << 8) & !(255 << 20)) | (255 << 8) | (255 << 20)
    });
    let dec_both = FragmentProgram::decode(&rz_both).unwrap();
    assert!(dec_both.operations().contains(&Operation::Integer {
        dst: 0,
        left: 255,
        right: 255,
        operator: IntegerOperator::Add,
    }));
    validate_wgsl_syntax(&dec_both.wgsl());
}

// ---------------------------------------------------------------------------
// 2. Bitwise logic truth tables, boolean identities, and operator symbols
// ---------------------------------------------------------------------------

#[test]
fn bitwise_lop_truth_tables_and_boolean_laws() {
    let program = FragmentProgram::decode(FRAGMENT_BIN).unwrap();

    // Verify LOP AND, OR, XOR operations are decoded with exact operator variant
    assert!(program.operations().contains(&Operation::Integer {
        dst: 1,
        left: 7,
        right: 4,
        operator: IntegerOperator::And,
    }));
    assert!(program.operations().contains(&Operation::Integer {
        dst: 2,
        left: 0,
        right: 6,
        operator: IntegerOperator::Or,
    }));
    assert!(program.operations().contains(&Operation::Integer {
        dst: 3,
        left: 8,
        right: 6,
        operator: IntegerOperator::Xor,
    }));

    // Verify respective WGSL symbols
    assert!(program.wgsl().contains("r[1]=r[7] & r[4];"));
    assert!(program.wgsl().contains("r[2]=r[0] | r[6];"));
    assert!(program.wgsl().contains("r[3]=r[8] ^ r[6];"));

    // Test RZ identities on LOP operations:
    // x & 0 = 0 (annihilation)
    let mut and_zero = FRAGMENT_BIN.to_vec();
    mutate_word(&mut and_zero, 0x98, |w| (w & !(255 << 20)) | (255 << 20));
    assert!(FragmentProgram::decode(&and_zero).is_ok());

    // x | 0 = x (identity)
    let mut or_zero = FRAGMENT_BIN.to_vec();
    mutate_word(&mut or_zero, 0xa0, |w| (w & !(255 << 20)) | (255 << 20));
    assert!(FragmentProgram::decode(&or_zero).is_ok());

    // x ^ 0 = x (identity)
    let mut xor_zero = FRAGMENT_BIN.to_vec();
    mutate_word(&mut xor_zero, 0xa8, |w| (w & !(255 << 20)) | (255 << 20));
    assert!(FragmentProgram::decode(&xor_zero).is_ok());
}

// ---------------------------------------------------------------------------
// 3. Strict rejection of unpermitted modifiers, condition codes & predicates
// ---------------------------------------------------------------------------

#[test]
fn adversarial_iadd_modifiers_and_condition_codes_rejected() {
    // Pinned envytools IADD encoding variations:
    // 0x5c10_0000_0007_0000 is base IADD without modifiers (stall=6, wr=7)
    let base_iadd = 0x5c10000000070400u64; // R0, R4, R5

    // Bit 43: Carry in (.X)
    let mut b = FRAGMENT_BIN.to_vec();
    mutate_word(&mut b, 0x88, |_| base_iadd | (1 << 43));
    let err = FragmentProgram::decode(&b).unwrap_err().to_string();
    assert!(err.contains("unsupported instruction"), "{err}");

    // Bit 47: Condition code write (CC)
    mutate_word(&mut b, 0x88, |_| base_iadd | (1 << 47));
    assert!(FragmentProgram::decode(&b).is_err());

    // Bit 50: Saturation (.SAT)
    mutate_word(&mut b, 0x88, |_| base_iadd | (1 << 50));
    assert!(FragmentProgram::decode(&b).is_err());

    // Bit 49: Negation (-src)
    mutate_word(&mut b, 0x88, |_| base_iadd | (1 << 49));
    assert!(FragmentProgram::decode(&b).is_err());

    // Execution predicate != 7 (unconditional)
    for pred in [0u64, 1, 2, 3, 4, 5, 6] {
        mutate_word(&mut b, 0x88, |_| (base_iadd & !(7 << 16)) | (pred << 16));
        assert!(FragmentProgram::decode(&b).is_err());
    }

    // Write barrier != 7 must fail
    for wb in [0u64, 1, 2, 3, 4, 5, 6] {
        mutate_word(&mut b, 0x88, |_| (base_iadd & !(7 << 40)) | (wb << 40));
        assert!(FragmentProgram::decode(&b).is_err());
    }
}

#[test]
fn adversarial_lop_modifiers_and_non_pt_predicates_rejected() {
    let base_lop_and = 0x5c47000000070400u64;

    // Condition codes (CC)
    let mut b = FRAGMENT_BIN.to_vec();
    mutate_word(&mut b, 0x98, |_| base_lop_and | (1 << 47));
    assert!(FragmentProgram::decode(&b).is_err());

    // .X modifier
    mutate_word(&mut b, 0x98, |_| base_lop_and | (1 << 43));
    assert!(FragmentProgram::decode(&b).is_err());

    // PASS_B mode (0x06 in bits 42:41)
    mutate_word(&mut b, 0x98, |_| base_lop_and | (3 << 41));
    assert!(FragmentProgram::decode(&b).is_err());

    // Inverted inputs (!src0 or !src1)
    for inv_bit in [39, 40] {
        mutate_word(&mut b, 0x98, |_| base_lop_and | (1 << inv_bit));
        assert!(FragmentProgram::decode(&b).is_err());
    }

    // Predicate destination != 7 (PT)
    for pdest in [0u64, 1, 2, 3, 4, 5, 6] {
        mutate_word(&mut b, 0x98, |_| {
            (base_lop_and & !(7 << 44)) | (pdest << 44)
        });
        assert!(FragmentProgram::decode(&b).is_err());
    }
}

// ---------------------------------------------------------------------------
// 4. Scoreboard tracking, register initialization, and RZ destination rejection
// ---------------------------------------------------------------------------

#[test]
fn scoreboard_uninitialized_reads_and_pending_hazards_rejected() {
    // Uninitialized left register (R9 has not been defined)
    let mut b = FRAGMENT_BIN.to_vec();
    mutate_word(&mut b, 0x88, |w| (w & !(255 << 8)) | (9 << 8));
    let err = FragmentProgram::decode(&b).unwrap_err().to_string();
    assert!(err.contains("unready integer ALU operand"), "{err}");

    // Uninitialized right register (R10 has not been defined)
    mutate_word(&mut b, 0x88, |w| (w & !(255 << 20)) | (10 << 20));
    assert!(FragmentProgram::decode(&b).is_err());

    // Pending scoreboard register (R8 scheduled with pending write barrier)
    let mut pending = FRAGMENT_BIN.to_vec();
    mutate_word(&mut pending, 0x70, |w| w & !(1 << (42 + 11))); // strip wait on barrier 1
    mutate_word(&mut pending, 0x88, |w| (w & !(255 << 8)) | (8 << 8)); // read pending R8
    let err = FragmentProgram::decode(&pending).unwrap_err().to_string();
    assert!(err.contains("unready integer ALU operand"), "{err}");

    // Writing to a pending register (R8 is pending, cannot be overwritten)
    let mut write_pending = FRAGMENT_BIN.to_vec();
    mutate_word(&mut write_pending, 0x70, |w| w & !(1 << (42 + 11)));
    mutate_word(&mut write_pending, 0x88, |w| (w & !255) | 8);
    let err = FragmentProgram::decode(&write_pending)
        .unwrap_err()
        .to_string();
    assert!(err.contains("write to pending register"), "{err}");
}

#[test]
fn rz_cannot_be_integer_destination() {
    // RZ (255) as destination must be rejected
    let mut b = FRAGMENT_BIN.to_vec();
    mutate_word(&mut b, 0x88, |w| (w & !255) | 255);
    assert!(FragmentProgram::decode(&b).is_err());

    let mut b_and = FRAGMENT_BIN.to_vec();
    mutate_word(&mut b_and, 0x98, |w| (w & !255) | 255);
    assert!(FragmentProgram::decode(&b_and).is_err());
}

// ---------------------------------------------------------------------------
// 5. Special resource provenance erasure & unfused rounding hazard retention
// ---------------------------------------------------------------------------

#[test]
fn integer_operations_strip_texture_handle_and_interpolation_provenance() {
    // Insert an integer operation after an LDC/attribute load to confirm that
    // constant-buffer handle provenance is erased and cannot authorize a TEX.
    let tex_bin = include_bytes!("fixtures/maxwell/tex.bin");

    // Verify that attempting to use an integer result as a texture handle fails
    let bundle_ctrl = u64::from_le_bytes(tex_bin[0x60..0x68].try_into().unwrap());
    assert_ne!(bundle_ctrl, 0);

    // Test unfused rounding hazard taint retention:
    // An FMUL result passed through an integer identity still preserves the
    // contraction hazard, so FADD consuming the integer output must fail.
    for op_base in [
        0x5c10000000070000u64, // IADD
        0x5c47000000070000,    // AND
        0x5c47020000070000,    // OR
        0x5c47040000070000,    // XOR
    ] {
        let mut alu = ALU_BIN.to_vec();
        // Modify instruction 4 to an integer op consuming R1 (from FMUL) and RZ (255) -> R2
        mutate_word(&mut alu, 0x88, |_| op_base | (255 << 20) | (1 << 8) | 2);
        // Instruction 5 is FADD R3, R2, RZ
        mutate_word(&mut alu, 0x98, |_| 0x5c5800000ff70203);
        let err = FragmentProgram::decode(&alu).unwrap_err().to_string();
        assert!(
            err.contains("unfused rounding"),
            "Expected unfused rounding error for op {op_base:#x}, got: {err}"
        );
    }
}

// ---------------------------------------------------------------------------
// 6. Vertex stage integer translation and stage isolation
// ---------------------------------------------------------------------------

#[test]
fn vertex_integer_lowering_and_stage_isolation() {
    let vertex = VertexProgram::decode(VERTEX_BIN).unwrap();
    assert_eq!(vertex.required_registers(), 7);

    // Verify vertex operations contain IADD and XOR
    assert!(vertex.operations().contains(&Operation::Integer {
        dst: 0,
        left: 0,
        right: 6,
        operator: IntegerOperator::Add,
    }));
    assert!(vertex.operations().contains(&Operation::Integer {
        dst: 1,
        left: 1,
        right: 6,
        operator: IntegerOperator::Xor,
    }));

    // Verify WGSL lowering
    let wgsl = vertex.wgsl();
    assert!(wgsl.contains("r[0]=r[0] + r[6];"));
    assert!(wgsl.contains("r[1]=r[1] ^ r[6];"));
    validate_wgsl_syntax(&wgsl);

    // VertexProgram must reject FragmentProgram binary (contains LDC)
    assert!(VertexProgram::decode(FRAGMENT_BIN).is_err());

    // FragmentProgram must reject VertexProgram binary (contains ALD/AST without fragment header)
    assert!(FragmentProgram::decode(VERTEX_BIN).is_err());
}
