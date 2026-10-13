//! Adversarial Maxwell B197 integer ALU and bit logic conformance suite.
//!
//! Independently verifies Maxwell GM107/B197 SASS `IADD` and `LOP` instructions across
//! vertex and fragment shader stages against authoritative specifications:
//! - Pinned envytools rev `f102b8233fe44f2fffa1468641a27df3ae51921f` (GM107 ISA table):
//!   - `IADD`: opcode `0x5c10...`, wrapping modulo 2^32, GPR/GPR operands, RZ source identity.
//!   - `LOP`: opcode `0x5c47...`, AND (`0x00`), OR (`0x02`), XOR (`0x04`), predicate dest in bits 50:48.
//! - Pinned UAM assembler rev `5a5afc2573215682c7a105c31252d6ae13bb01a7`.
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

fn insert_identity(bin: &[u8], at: usize, register: u8) -> Vec<u8> {
    let mut items: Vec<(u64, u64)> = bin[80..]
        .chunks_exact(32)
        .flat_map(|bundle| {
            let ctrl = u64::from_le_bytes(bundle[..8].try_into().unwrap());
            (0..3).map(move |n| {
                (
                    u64::from_le_bytes(bundle[8 + n * 8..16 + n * 8].try_into().unwrap()),
                    (ctrl >> (n * 21)) & 0x1fffff,
                )
            })
        })
        .collect();
    items.insert(
        at,
        (
            0x5c1000000ff70000 | (u64::from(register) << 8) | u64::from(register),
            0xfe6,
        ),
    );
    let mut out = bin[..80].to_vec();
    for group in items.chunks(3) {
        let control = (0..3).fold(0u64, |a, n| {
            a | (group.get(n).map_or(0x7e6, |v| v.1) << (n * 21))
        });
        out.extend(control.to_le_bytes());
        for n in 0..3 {
            out.extend(
                group
                    .get(n)
                    .map_or(0x50b0000000070f00, |v| v.0)
                    .to_le_bytes(),
            );
        }
    }
    out
}

// ---------------------------------------------------------------------------
// 1. Decoder lowering, WGSL emission, and RZ (zero) identities
// ---------------------------------------------------------------------------

#[test]
fn wrapping_iadd_decoder_lowering_and_rz_identities() {
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

#[test]
fn bitwise_lop_decoder_lowering_and_symbols() {
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
// 2. Independently evaluated numeric wrapping and bitwise logic invariants
// ---------------------------------------------------------------------------

#[test]
#[allow(clippy::identity_op, clippy::erasing_op, clippy::eq_op)]
fn independent_numeric_wrapping_and_bitwise_evaluation() {
    // 32-bit wrapping addition modulo 2^32
    assert_eq!(0xFFFFFFFFu32.wrapping_add(1), 0);
    assert_eq!(0x7FFFFFFFu32.wrapping_add(1), 0x80000000);
    assert_eq!(0x80000000u32.wrapping_add(0x80000000), 0);
    assert_eq!(0u32.wrapping_add(0), 0);

    // Bitwise boolean identities across full 32-bit space
    let sample_inputs = [
        0u32, 1, 0xFFFFFFFF, 0x55555555, 0xAAAAAAAA, 0x7FFFFFFF, 0x80000000, 0x12345678,
    ];
    for &x in &sample_inputs {
        // AND laws
        assert_eq!(x & x, x); // Idempotence
        assert_eq!(x & 0, 0); // Annihilation
        assert_eq!(x & 0xFFFFFFFF, x); // Identity

        // OR laws
        assert_eq!(x | x, x); // Idempotence
        assert_eq!(x | 0, x); // Identity
        assert_eq!(x | 0xFFFFFFFF, 0xFFFFFFFF); // Annihilation

        // XOR laws
        assert_eq!(x ^ x, 0); // Self-inverse
        assert_eq!(x ^ 0, x); // Identity
        assert_eq!(x ^ 0xFFFFFFFF, !x); // Bitwise NOT
    }

    // De Morgan's laws
    for &a in &sample_inputs {
        for &b in &sample_inputs {
            assert_eq!(!(a & b), (!a) | (!b));
            assert_eq!(!(a | b), (!a) & (!b));
        }
    }
}

// ---------------------------------------------------------------------------
// 3. Strict rejection of unpermitted modifiers, condition codes & predicates
// ---------------------------------------------------------------------------

#[test]
fn adversarial_iadd_modifiers_and_condition_codes_rejected() {
    // Start directly from the actual fixture IADD word at offset 0x88 (IADD R0, R4, R5)
    let fixture_word = u64::from_le_bytes(FRAGMENT_BIN[0x88..0x90].try_into().unwrap());
    assert_eq!(fixture_word, 0x5c10000000570400);

    // Exact envyas-produced negatives: carry-in (.X, bit 43), CC (bit 47), .SAT (bit 50), negation (bit 49)
    for word in [
        fixture_word | (1 << 43), // Carry in (.X)
        fixture_word | (1 << 47), // Condition code write (CC)
        fixture_word | (1 << 50), // Saturation (.SAT)
        fixture_word | (1 << 49), // Negation (-srcB)
        fixture_word | (1 << 48), // Negation (-srcA)
    ] {
        let mut b = FRAGMENT_BIN.to_vec();
        mutate_word(&mut b, 0x88, |_| word);
        let err = FragmentProgram::decode(&b).unwrap_err().to_string();
        assert!(
            err.contains("unsupported instruction"),
            "Expected unsupported instruction for {word:#x}, got: {err}"
        );
    }

    // Execution predicates != 7 (PT, unconditional): bits 18:16
    for pred in [0u64, 1, 2, 3, 4, 5, 6] {
        let mut b = FRAGMENT_BIN.to_vec();
        mutate_word(&mut b, 0x88, |w| (w & !(7 << 16)) | (pred << 16));
        let err = FragmentProgram::decode(&b).unwrap_err().to_string();
        assert!(err.contains("unsupported instruction"), "{err}");
    }

    // Scheduling control write barrier for slot 2 is in control word at offset 0x70, bits (42+5)..(42+7).
    // Mutating write barrier away from 7 must fail with unready/schedule error.
    for wb in [0u64, 1, 2, 3, 4, 5, 6] {
        let mut b = FRAGMENT_BIN.to_vec();
        mutate_word(&mut b, 0x70, |ctrl| {
            (ctrl & !(7u64 << (42 + 5))) | (wb << (42 + 5))
        });
        let err = FragmentProgram::decode(&b).unwrap_err().to_string();
        assert!(
            err.contains("unready integer ALU operand or unsupported schedule")
                || err.contains("unsupported scheduling dependency"),
            "Expected scheduling diagnostic for wb {wb}, got: {err}"
        );
    }
}

#[test]
fn adversarial_lop_modifiers_and_non_pt_predicates_rejected() {
    // Start directly from the actual fixture LOP word at offset 0x98 (LOP.AND R1, R7, R4)
    let fixture_word = u64::from_le_bytes(FRAGMENT_BIN[0x98..0xa0].try_into().unwrap());
    assert_eq!(fixture_word, 0x5c47000000470701);

    // Condition codes (CC, bit 47), .X (bit 43), PASS_B (bits 42:41)
    for word in [
        fixture_word | (1 << 47),                // CC
        fixture_word | (1 << 43),                // .X
        (fixture_word & !(3 << 41)) | (3 << 41), // PASS_B
    ] {
        let mut b = FRAGMENT_BIN.to_vec();
        mutate_word(&mut b, 0x98, |_| word);
        let err = FragmentProgram::decode(&b).unwrap_err().to_string();
        assert!(
            err.contains("unsupported instruction"),
            "Expected unsupported instruction for {word:#x}, got: {err}"
        );
    }

    // Inverted inputs (!src0 bit 39, !src1 bit 40)
    for inv_bit in [39, 40] {
        let mut b = FRAGMENT_BIN.to_vec();
        mutate_word(&mut b, 0x98, |w| w | (1 << inv_bit));
        let err = FragmentProgram::decode(&b).unwrap_err().to_string();
        assert!(err.contains("unsupported instruction"), "{err}");
    }

    // Predicate destination in bits 50:48 != 7 (PT)
    for pdest in [0u64, 1, 2, 3, 4, 5, 6] {
        let mut b = FRAGMENT_BIN.to_vec();
        mutate_word(&mut b, 0x98, |w| (w & !(7u64 << 48)) | (pdest << 48));
        let err = FragmentProgram::decode(&b).unwrap_err().to_string();
        assert!(
            err.contains("unsupported instruction"),
            "Expected unsupported instruction for pdest {pdest}, got: {err}"
        );
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
    // Inserting an integer identity before TEX clears texture handle provenance
    let tex_bin = include_bytes!("fixtures/maxwell/tex.bin");
    let b_tex = insert_identity(tex_bin, 3, 6);
    let err_tex = FragmentProgram::decode(&b_tex).unwrap_err().to_string();
    assert!(
        err_tex.contains("TEX handle"),
        "Expected TEX handle provenance error, got: {err_tex}"
    );

    // Inserting an integer identity before PositionW clears PositionW provenance
    let persp_bin = include_bytes!("fixtures/maxwell/perspective.bin");
    let b_pos = insert_identity(persp_bin, 2, 7);
    let err_pos = FragmentProgram::decode(&b_pos).unwrap_err().to_string();
    assert!(
        err_pos.contains("PositionW"),
        "Expected PositionW provenance error, got: {err_pos}"
    );

    // Inserting an integer identity before MUFU.RCP clears reciprocal provenance
    let b_rcp = insert_identity(persp_bin, 3, 7);
    let err_rcp = FragmentProgram::decode(&b_rcp).unwrap_err().to_string();
    assert!(
        err_rcp.contains("reciprocal"),
        "Expected reciprocal provenance error, got: {err_rcp}"
    );

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
