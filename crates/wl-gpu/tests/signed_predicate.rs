use wl_gpu::shader_program::{FragmentProgram, IntegerComparison, Operation, VertexProgram};
const BIN: &[u8] = include_bytes!("fixtures/maxwell/signed-predicate.bin");
fn change(b: &mut [u8], off: usize, f: impl FnOnce(u64) -> u64) {
    let word = u64::from_le_bytes(b[off..off + 8].try_into().unwrap());
    b[off..off + 8].copy_from_slice(&f(word).to_le_bytes());
}
fn error(b: &[u8], text: &str) {
    let e = FragmentProgram::decode(b).unwrap_err().to_string();
    assert!(e.contains(text), "{e}");
}
fn validate(wgsl: &str) {
    let m = naga::front::wgsl::parse_str(wgsl).unwrap();
    naga::valid::Validator::new(
        naga::valid::ValidationFlags::all(),
        naga::valid::Capabilities::all(),
    )
    .validate(&m)
    .unwrap();
}
#[test]
fn independent_signed_forms_decode_and_bitcast_in_both_stages() {
    // Independently assembled gm107 words: P0,PT,R4,R5,AND PT, not decoder output.
    for (word, comparison, symbol) in [
        (0x5b63038000570407, IntegerComparison::LessSigned, "<"),
        (0x5b65038000570407, IntegerComparison::EqualSigned, "=="),
        (0x5b67038000570407, IntegerComparison::LessEqualSigned, "<="),
        (0x5b69038000570407, IntegerComparison::GreaterSigned, ">"),
        (0x5b6b038000570407, IntegerComparison::NotEqualSigned, "!="),
        (
            0x5b6d038000570407,
            IntegerComparison::GreaterEqualSigned,
            ">=",
        ),
    ] {
        let mut b = BIN.to_vec();
        change(&mut b, 0x80, |_| word);
        let p = FragmentProgram::decode(&b).unwrap();
        assert_eq!(p.required_registers(), 8);
        assert_eq!(
            p.operations()[4],
            Operation::PredicateCompare {
                left: 4,
                right: 5,
                comparison
            }
        );
        assert!(p.wgsl().contains(&format!(
            "p0=bitcast<i32>(r[4]) {symbol} bitcast<i32>(r[5]);"
        )));
        validate(&p.wgsl());
        let mut v = include_bytes!("fixtures/maxwell/vertex-signed-predicate.bin").to_vec();
        // Same instruction form, with initialized vertex operands R0/R6.
        change(&mut v, 0x78, |_| {
            (word & !((255 << 8) | (255 << 20))) | (6 << 20)
        });
        let v = VertexProgram::decode(&v).unwrap();
        assert_eq!(v.required_registers(), 7);
        validate(&v.wgsl());
    }
    let p = FragmentProgram::decode(include_bytes!("fixtures/maxwell/signed-select-bytes.bin"))
        .unwrap();
    assert_eq!(p.required_registers(), 16);
    assert_eq!(p.constants().len(), 4);
    validate(&p.wgsl());
}
#[test]
fn signedness_is_per_operation_and_legacy_unsigned_shape_is_retained() {
    let mut b = BIN.to_vec();
    change(&mut b, 0xa0, |w| w & !(1 << 48));
    let p = FragmentProgram::decode(&b).unwrap();
    assert!(
        p.wgsl()
            .contains("p0=bitcast<i32>(r[4]) < bitcast<i32>(r[5]);")
    );
    assert!(p.wgsl().contains("p0=r[4] >= r[5];"));
    validate(&p.wgsl());
    let old = FragmentProgram::decode(include_bytes!("fixtures/maxwell/predicate.bin")).unwrap();
    assert!(!old.wgsl().contains("bitcast<i32>"));
}
#[test]
fn signed_compare_retains_readiness_rz_and_predicate_latency() {
    for field in [8, 20] {
        let mut b = BIN.to_vec();
        change(&mut b, 0x80, |w| (w & !(255 << field)) | (8 << field));
        error(&b, "unready predicate operand");
        change(&mut b, 0x80, |w| w | (255 << field));
        validate(&FragmentProgram::decode(&b).unwrap().wgsl());
    }
    let mut b = BIN.to_vec();
    change(&mut b, 0x70, |w| (w & !(31 << 21)) | (6 << 21));
    error(&b, "fixed-latency schedule");
    let mut b = BIN.to_vec();
    change(&mut b, 0x70, |w| w & !(7 << (21 + 5)));
    error(&b, "unready predicate operand or unsupported schedule");
    // Removing the latest wait cannot cause the compare to clear pending R7.
    let mut b = BIN.to_vec();
    change(&mut b, 0x70, |w| w & !(1 << (21 + 11)));
    error(&b, "unready selector operand");
    change(&mut b, 0x80, |w| (w & !(255 << 8)) | (7 << 8));
    error(&b, "unready predicate operand");
}
#[test]
fn signed_extended_boolean_predicate_and_alternate_forms_are_explicitly_rejected() {
    for word in [
        0x5b61038000570407, // F, outside current comparison subset.
        0x5b6f038000570407, // T.
        0x5b630b8000570407, // .X, independent assembly.
        0x5b63238000570407, // OR PT.
        0x5b63438000570407, // XOR PT.
        0x5b6303800057040f, // primary P1.
        0x5b63038000570401, // secondary P1.
        0x5b63000000570407, // accumulator P0.
        0x5b63078000570407, // inverted accumulator PT.
        0x5b63038000500407, // execution P0.
        0x3663038000570407, // immediate form, deliberately unimplemented.
        0x4b63038000570407, // constant form, deliberately unimplemented.
    ] {
        let mut b = BIN.to_vec();
        change(&mut b, 0x80, |_| word);
        if word >> 52 != 0x5b6 {
            // The strict decoder has not admitted these opcode families. Use
            // its ordinary six-stall gate to exercise actual opcode rejection.
            change(&mut b, 0x70, |w| (w & !(31 << 21)) | (6 << 21));
            error(&b, "unsupported instruction");
        } else {
            assert!(FragmentProgram::decode(&b).is_err(), "accepted {word:#x}");
        }
    }
}
#[test]
fn signed_predicate_does_not_erase_selection_resource_provenance_rules() {
    // Existing adversarial selection corpus inserts a real ISETP/SEL pair before
    // TEX and perspective metadata consumers. Change only the independent ISETP
    // signedness bit; selection still must erase all special provenance.
    for (bin, at, reg, message) in [
        (
            include_bytes!("fixtures/maxwell/tex.bin").as_slice(),
            3,
            6,
            "TEX handle",
        ),
        (
            include_bytes!("fixtures/maxwell/perspective.bin").as_slice(),
            2,
            7,
            "PositionW",
        ),
        (
            include_bytes!("fixtures/maxwell/perspective.bin").as_slice(),
            3,
            7,
            "reciprocal",
        ),
    ] {
        let mut items: Vec<(u64, u64)> = bin[80..]
            .chunks_exact(32)
            .flat_map(|b| {
                let c = u64::from_le_bytes(b[..8].try_into().unwrap());
                (0..3).map(move |n| {
                    (
                        u64::from_le_bytes(b[8 + n * 8..16 + n * 8].try_into().unwrap()),
                        (c >> (n * 21)) & 0x1fffff,
                    )
                })
            })
            .collect();
        items.insert(at, (0x5b6303800ff7ff07, 0xfed)); // signed LT RZ,RZ.
        items.insert(
            at + 1,
            (0x5ca0000000070000 | (reg << 20) | (reg << 8) | reg, 0x7e6),
        );
        let mut b = bin[..80].to_vec();
        for group in items.chunks(3) {
            let c = (0..3).fold(0, |c, n| {
                c | (group.get(n).map_or(0x7e6, |x| x.1) << (n * 21))
            });
            b.extend(u64::to_le_bytes(c));
            for n in 0..3 {
                b.extend(
                    group
                        .get(n)
                        .map_or(0x50b0000000070f00, |x| x.0)
                        .to_le_bytes(),
                );
            }
        }
        error(&b, message);
    }
}
