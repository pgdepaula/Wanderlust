use wl_gpu::shader_program::{FragmentProgram, Operation, VertexProgram};
const BIN: &[u8] = include_bytes!("fixtures/maxwell/perspective.bin");
fn change(bytes: &mut [u8], offset: usize, f: impl FnOnce(u64) -> u64) {
    let w = u64::from_le_bytes(bytes[offset..offset + 8].try_into().unwrap());
    bytes[offset..offset + 8].copy_from_slice(&f(w).to_le_bytes());
}
#[test]
fn independently_assembled_position_reciprocal_and_perspective_ipa() {
    let p = FragmentProgram::decode(BIN).unwrap();
    assert!(p.perspective());
    assert_eq!(p.required_registers(), 8);
    assert!(p.operations().contains(&Operation::PositionW { dst: 7 }));
    assert!(
        p.operations()
            .contains(&Operation::Reciprocal { dst: 7, src: 7 })
    );
    assert!(p.operations().contains(&Operation::Perspective {
        dst: 4,
        component: 0,
        multiplier: 7
    }));
    assert!(p.wgsl().contains("@interpolate(perspective,center)"));
    assert!(p.wgsl().contains("v.position.w * bitcast<f32>(r[7])"));
    assert!(
        !FragmentProgram::decode(include_bytes!("fixtures/maxwell/tex.bin"))
            .unwrap()
            .perspective()
    );
    assert!(VertexProgram::decode(BIN).is_err());
}
#[test]
fn perspective_header_modifiers_and_variable_latency_waits_are_strict() {
    for offset in [20, 24] {
        let mut b = BIN.to_vec();
        b[offset] ^= 1;
        assert!(FragmentProgram::decode(&b).is_err());
    }
    for (pc, bits) in [
        (0x60, &[16, 19, 39, 51, 52, 54][..]),
        (0x68, &[16, 19, 20, 46, 48, 50][..]),
        (0x78, &[16, 19, 39, 51, 52, 55][..]),
    ] {
        for bit in bits {
            let mut b = BIN.to_vec();
            change(&mut b, pc, |w| w ^ (1 << bit));
            assert!(FragmentProgram::decode(&b).is_err(), "{pc:x}/{bit}");
        }
    }
    // RCP cannot consume IPA's pending result without an actual wait.
    let mut b = BIN.to_vec();
    change(&mut b, 0x50, |w| w & !(1 << (42 + 11)));
    assert!(FragmentProgram::decode(&b).is_err());
    // Generic interpolation cannot consume the pending reciprocal either.
    let mut b = BIN.to_vec();
    change(&mut b, 0x70, |w| w & !(1 << 11));
    assert!(FragmentProgram::decode(&b).is_err());
}
#[test]
fn general_reciprocal_and_unproven_interpolation_multiplier_are_unsupported() {
    let mut b = BIN.to_vec();
    change(&mut b, 0x68, |w| (w & !(255 << 8)) | (6 << 8));
    assert!(
        FragmentProgram::decode(&b)
            .unwrap_err()
            .to_string()
            .contains("PositionW")
    );
    let mut b = BIN.to_vec();
    change(&mut b, 0x78, |w| (w & !(255 << 20)) | (6 << 20));
    assert!(
        FragmentProgram::decode(&b)
            .unwrap_err()
            .to_string()
            .contains("reciprocal")
    );
    let mut b = BIN.to_vec();
    change(&mut b, 0x68, |_| 0x010000000007f007);
    change(&mut b, 0x50, |w| (w & !(7 << (42 + 5))) | (7 << (42 + 5)));
    assert!(FragmentProgram::decode(&b).is_err());
}
fn with_insertions(items: &[(u64, u64)]) -> Vec<u8> {
    let mut out = BIN[..80].to_vec();
    for group in items.chunks(3) {
        let mut control = 0u64;
        for slot in 0..3 {
            control |= group.get(slot).map_or(0x7e6, |v| v.1) << (slot * 21);
        }
        out.extend(control.to_le_bytes());
        for slot in 0..3 {
            out.extend(
                group
                    .get(slot)
                    .map_or(0x50b0000000070f00, |v| v.0)
                    .to_le_bytes(),
            );
        }
    }
    out
}
fn instructions() -> Vec<(u64, u64)> {
    BIN[80..]
        .chunks_exact(32)
        .flat_map(|bundle| {
            let control = u64::from_le_bytes(bundle[..8].try_into().unwrap());
            (0..3).map(move |n| {
                (
                    u64::from_le_bytes(bundle[8 + n * 8..16 + n * 8].try_into().unwrap()),
                    (control >> (21 * n)) & 0x1fffff,
                )
            })
        })
        .take(7)
        .collect()
}
#[test]
fn reciprocal_provenance_survives_mov_and_is_cleared_by_real_overwrite() {
    let mut items = instructions();
    // MOV R8,R7 waits for the reciprocal. Both IPA operands now reference R8.
    items.insert(3, (0x5c98078000770008, 0xfe6));
    for n in [4, 5] {
        items[n].0 = (items[n].0 & !(255 << 20)) | (8 << 20);
    }
    let p = FragmentProgram::decode(&with_insertions(&items)).unwrap();
    assert_eq!(p.required_registers(), 9);
    items.insert(4, (0x010000000007f008, 0x7e6));
    assert!(
        FragmentProgram::decode(&with_insertions(&items))
            .unwrap_err()
            .to_string()
            .contains("reciprocal")
    );
    let mut items = instructions();
    // PositionW provenance also follows MOV before MUFU consumes it.
    items.insert(2, (0x5c98078000770008, 0xfe6));
    items[3].0 = (items[3].0 & !(255 << 8)) | (8 << 8);
    assert!(FragmentProgram::decode(&with_insertions(&items)).is_ok());
    items[2].1 = 0x7e6; // removing the wait cannot be hidden by provenance
    assert!(FragmentProgram::decode(&with_insertions(&items)).is_err());
}
