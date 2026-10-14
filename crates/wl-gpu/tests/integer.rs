use wl_gpu::shader_program::{FragmentProgram, IntegerOperator, Operation, VertexProgram};
const BIN: &[u8] = include_bytes!("fixtures/maxwell/integer.bin");
fn change(bytes: &mut [u8], offset: usize, f: impl FnOnce(u64) -> u64) {
    let w = u64::from_le_bytes(bytes[offset..offset + 8].try_into().unwrap());
    bytes[offset..offset + 8].copy_from_slice(&f(w).to_le_bytes());
}
#[test]
fn independent_integer_fields_and_raw_u32_lowering() {
    let p = FragmentProgram::decode(BIN).unwrap();
    assert_eq!(p.required_registers(), 9);
    for (n, (dst, left, right, operator)) in [
        (0, 4, 5, IntegerOperator::Add),
        (1, 7, 4, IntegerOperator::And),
        (2, 0, 6, IntegerOperator::Or),
        (3, 8, 6, IntegerOperator::Xor),
    ]
    .into_iter()
    .enumerate()
    {
        assert_eq!(
            p.operations()[n + 5],
            Operation::Integer {
                dst,
                left,
                right,
                operator
            }
        );
    }
    for line in [
        "r[0]=r[4] + r[5];",
        "r[1]=r[7] & r[4];",
        "r[2]=r[0] | r[6];",
        "r[3]=r[8] ^ r[6];",
    ] {
        assert!(p.wgsl().contains(line));
    }
    assert!(VertexProgram::decode(BIN).is_err());
}
#[test]
fn independently_assembled_integer_modifiers_and_predicate_outputs_reject() {
    // Exact envyas-produced negatives: carry, CC, saturation, PO, execution predicates.
    for word in [
        0x5c10080000570400,
        0x5c10800000570400,
        0x5c14000000570400,
        0x5c13000000570400, // both negation bits select unsupported IADD.PO
        0x5c10000000500400,
        0x5c10000000580400,
    ] {
        let mut b = BIN.to_vec();
        change(&mut b, 0x88, |_| word);
        let e = FragmentProgram::decode(&b).unwrap_err().to_string();
        assert!(e.contains("unsupported instruction"), "{e}");
    }
    // LOP CC, X, non-PT predicate destinations/modes, input inversion and PASS_B.
    for word in [
        0x5c47800000570400,
        0x5c47080000570400,
        0x5c40000000570400,
        0x5c47100000570400,
        0x5c47200000570400,
        0x5c47300000570400,
        0x5c47008000570400,
        0x5c47010000570400,
        0x5c47060000570400,
    ] {
        let mut b = BIN.to_vec();
        change(&mut b, 0x98, |_| word);
        let e = FragmentProgram::decode(&b).unwrap_err().to_string();
        assert!(e.contains("unsupported instruction"), "{e}");
    }
}
#[test]
fn integer_sources_scoreboard_and_zero_register_are_checked() {
    let mut pending = BIN.to_vec();
    change(&mut pending, 0x70, |w| w & !(1 << (42 + 11)));
    change(&mut pending, 0x88, |w| (w & !(255 << 8)) | (8 << 8));
    assert!(
        FragmentProgram::decode(&pending)
            .unwrap_err()
            .to_string()
            .contains("unready integer")
    );
    for shift in [8, 20] {
        let mut b = BIN.to_vec();
        change(&mut b, 0x88, |w| (w & !(255 << shift)) | (9 << shift));
        assert!(FragmentProgram::decode(&b).is_err());
        let mut b = BIN.to_vec();
        change(&mut b, 0x88, |w| w | (255 << shift));
        assert!(FragmentProgram::decode(&b).is_ok());
    }
    let mut b = BIN.to_vec();
    change(&mut b, 0x70, |w| w & !(1 << (42 + 11)));
    change(&mut b, 0x88, |w| (w & !255) | 8);
    assert!(
        FragmentProgram::decode(&b)
            .unwrap_err()
            .to_string()
            .contains("write to pending register")
    );
    let mut b = BIN.to_vec();
    change(&mut b, 0x88, |w| w | 255);
    assert!(FragmentProgram::decode(&b).is_err());
}
#[test]
fn integer_identities_cannot_hide_fmul_to_fadd_rounding_hazards() {
    // FMUL R1 is followed by an integer identity producing R2, then FADD R3,R2,RZ.
    for base in [
        0x5c10000000070000u64,
        0x5c47000000070000,
        0x5c47020000070000,
        0x5c47040000070000,
    ] {
        for (left, right) in [(1, 255), (255, 1), (1, 1)] {
            let mut b = include_bytes!("fixtures/maxwell/alu.bin").to_vec();
            change(&mut b, 0x88, |_| base | (right << 20) | (left << 8) | 2);
            change(&mut b, 0x98, |_| 0x5c5800000ff70203);
            assert!(
                FragmentProgram::decode(&b)
                    .unwrap_err()
                    .to_string()
                    .contains("unfused rounding")
            );
        }
    }
}

#[test]
fn independent_integer_vertex_stage_exports_and_cache_identity() {
    let b = include_bytes!("fixtures/maxwell/vertex-integer.bin");
    let p = VertexProgram::decode(b).unwrap();
    assert_eq!(p.required_registers(), 7);
    assert!(p.operations().contains(&Operation::Integer {
        dst: 0,
        left: 0,
        right: 6,
        operator: IntegerOperator::Add
    }));
    assert!(p.operations().contains(&Operation::Integer {
        dst: 1,
        left: 1,
        right: 6,
        operator: IntegerOperator::Xor
    }));
    assert!(p.wgsl().contains("r[0]=r[0] + r[6];"));
    assert!(p.wgsl().contains("r[1]=r[1] ^ r[6];"));
    assert!(FragmentProgram::decode(b).is_err());
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
#[test]
fn integer_results_cannot_retain_special_resource_or_interpolation_provenance() {
    let b = insert_identity(include_bytes!("fixtures/maxwell/tex.bin"), 3, 6);
    assert!(
        FragmentProgram::decode(&b)
            .unwrap_err()
            .to_string()
            .contains("TEX handle")
    );
    let bin = include_bytes!("fixtures/maxwell/perspective.bin");
    let b = insert_identity(bin, 2, 7);
    assert!(
        FragmentProgram::decode(&b)
            .unwrap_err()
            .to_string()
            .contains("PositionW")
    );
    let b = insert_identity(bin, 3, 7);
    assert!(
        FragmentProgram::decode(&b)
            .unwrap_err()
            .to_string()
            .contains("reciprocal")
    );
}
