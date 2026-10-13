use wl_gpu::shader_program::{FragmentProgram, Operation, UnsignedComparison, VertexProgram};
const BIN: &[u8] = include_bytes!("fixtures/maxwell/predicate.bin");
fn change(b: &mut [u8], off: usize, f: impl FnOnce(u64) -> u64) {
    let w = u64::from_le_bytes(b[off..off + 8].try_into().unwrap());
    b[off..off + 8].copy_from_slice(&f(w).to_le_bytes());
}
fn error(b: &[u8], text: &str) {
    let e = FragmentProgram::decode(b).unwrap_err().to_string();
    assert!(e.contains(text), "{e}");
}
fn validate(source: &str) {
    let m = naga::front::wgsl::parse_str(source).unwrap();
    naga::valid::Validator::new(
        naga::valid::ValidationFlags::all(),
        naga::valid::Capabilities::all(),
    )
    .validate(&m)
    .unwrap();
}
#[test]
fn independent_predicate_pair_and_selection_fields_link_both_stages() {
    let p = FragmentProgram::decode(BIN).unwrap();
    assert_eq!(p.required_registers(), 8);
    assert_eq!(
        p.operations()[4],
        Operation::PredicateCompare {
            left: 4,
            right: 5,
            comparison: UnsignedComparison::Less
        }
    );
    assert_eq!(
        p.operations()[5],
        Operation::Select {
            dst: 0,
            left: 6,
            right: 7,
            invert: false
        }
    );
    assert_eq!(
        p.operations()[6],
        Operation::Select {
            dst: 1,
            left: 6,
            right: 7,
            invert: true
        }
    );
    assert_eq!(
        p.operations()[7],
        Operation::PredicateCompare {
            left: 4,
            right: 5,
            comparison: UnsignedComparison::Equal
        }
    );
    let s = p.wgsl();
    for line in [
        "var p0:bool;",
        "p0=r[4] < r[5];",
        "p0=r[4] == r[5];",
        "r[0]=select(r[7],r[6],p0);",
        "r[1]=select(r[7],r[6],!p0);",
    ] {
        assert!(s.contains(line));
    }
    validate(&s);
    let v = VertexProgram::decode(include_bytes!("fixtures/maxwell/vertex-predicate.bin")).unwrap();
    assert_eq!(v.required_registers(), 7); // P0/PT pair must not count as GPR R7.
    validate(&v.wgsl());
}
#[test]
fn six_unsigned_comparison_modes_and_reserved_modes() {
    for (mode, comparison) in [
        (1, UnsignedComparison::Less),
        (2, UnsignedComparison::Equal),
        (3, UnsignedComparison::LessEqual),
        (4, UnsignedComparison::Greater),
        (5, UnsignedComparison::NotEqual),
        (6, UnsignedComparison::GreaterEqual),
    ] {
        let mut b = BIN.to_vec();
        change(&mut b, 0x80, |w| (w & !(7 << 49)) | (mode << 49));
        let p = FragmentProgram::decode(&b).unwrap();
        assert_eq!(
            p.operations()[4],
            Operation::PredicateCompare {
                left: 4,
                right: 5,
                comparison
            }
        );
        validate(&p.wgsl());
    }
    for mode in [0, 7] {
        let mut b = BIN.to_vec();
        change(&mut b, 0x80, |w| (w & !(7 << 49)) | (mode << 49));
        error(&b, "unsupported unsigned comparison");
    }
}
#[test]
fn predicate_latency_is_separate_from_gpr_latency() {
    let mut b = BIN.to_vec();
    change(&mut b, 0x70, |w| (w & !(31 << 21)) | (6 << 21));
    error(&b, "fixed-latency schedule");
    let mut b = BIN.to_vec();
    change(&mut b, 0x70, |w| (w & !(31 << 42)) | (13 << 42));
    error(&b, "fixed-latency schedule");
    let mut b = BIN.to_vec();
    change(&mut b, 0x70, |w| w & !(7 << (21 + 5)));
    error(&b, "unready predicate operand or unsupported schedule");
}
#[test]
fn p0_initialization_and_gpr_metadata_are_independent() {
    let mut b = BIN.to_vec();
    // Replace ISETP with an ordinary initialized MOV32I, with its six-cycle control.
    let mov = u64::from_le_bytes(BIN[0xb8..0xc0].try_into().unwrap());
    change(&mut b, 0x80, |_| (mov & !255) | 4);
    change(&mut b, 0x70, |w| (w & !(31 << 21)) | (6 << 21));
    error(&b, "uninitialized selector predicate");
    // Load R8 instead of R7. ISETP's low byte 7 must not initialize GPR R7.
    let mut b = BIN.to_vec();
    change(&mut b, 0x78, |w| (w & !255) | 8);
    error(&b, "unready selector operand");
    // Without a wait, ISETP reads ready R4/R5 but must leave pending R7 alone.
    let mut b = BIN.to_vec();
    change(&mut b, 0x70, |w| w & !(1 << (21 + 11)));
    error(&b, "unready selector operand");
    // A pending source of the compare is independently rejected.
    change(&mut b, 0x80, |w| (w & !(255 << 8)) | (7 << 8));
    error(&b, "unready predicate operand");
    // A ready selection cannot overwrite pending R7 just because its predicate is ready.
    let mut b = BIN.to_vec();
    change(&mut b, 0x70, |w| w & !(1 << (21 + 11)));
    change(&mut b, 0x88, |w| (w & !(255 | (255 << 20))) | 7 | (6 << 20));
    error(&b, "write to pending register");
    // Keep an LDC-derived texture handle in R7 across ISETP's predicate write.
    let mut bin = include_bytes!("fixtures/maxwell/tex.bin").to_vec();
    change(&mut bin, 0x58, |w| (w & !255) | 7);
    change(&mut bin, 0x78, |w| (w & !(255 << 20)) | (7 << 20));
    let b = inject_select(&bin, 3, 8, 255, 255);
    assert_eq!(
        FragmentProgram::decode(&b).unwrap().texture_input(),
        Some(0)
    );
}
#[test]
fn unsupported_predicate_boolean_signed_and_execution_forms_reject() {
    for bit in [3, 39, 42, 43, 45, 48] {
        let mut b = BIN.to_vec();
        change(&mut b, 0x80, |w| w ^ (1 << bit));
        error(&b, "unsupported instruction");
    }
    let mut b = BIN.to_vec();
    change(&mut b, 0x80, |w| w & !7);
    error(&b, "unsupported instruction");
    for off in [0x80, 0x88] {
        let mut b = BIN.to_vec();
        change(&mut b, off, |w| w & !(15 << 16));
        error(&b, "unsupported instruction");
    }
    let mut b = BIN.to_vec();
    change(&mut b, 0x88, |w| w | (1 << 39));
    error(&b, "unsupported instruction");
    let mut b = BIN.to_vec();
    change(&mut b, 0x88, |w| w | 255);
    error(&b, "RZ destination");
}
fn inject_select(bin: &[u8], at: usize, dst: u8, left: u8, right: u8) -> Vec<u8> {
    let mut items: Vec<(u64, u64)> = bin[80..]
        .chunks_exact(32)
        .flat_map(|bundle| {
            let c = u64::from_le_bytes(bundle[..8].try_into().unwrap());
            (0..3).map(move |n| {
                (
                    u64::from_le_bytes(bundle[8 + n * 8..16 + n * 8].try_into().unwrap()),
                    (c >> (n * 21)) & 0x1fffff,
                )
            })
        })
        .collect();
    items.insert(at, (0x5b6203800ff7ff07, 0xfed)); // ISETP.LT RZ,RZ -> false P0.
    items.insert(
        at + 1,
        (
            0x5ca0000000070000 | (u64::from(right) << 20) | (u64::from(left) << 8) | u64::from(dst),
            0x7e6,
        ),
    );
    let mut out = bin[..80].to_vec();
    for group in items.chunks(3) {
        out.extend(
            (0..3)
                .fold(0u64, |a, n| {
                    a | (group.get(n).map_or(0x7e6, |v| v.1) << (n * 21))
                })
                .to_le_bytes(),
        );
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
fn selection_cannot_retain_special_resource_provenance() {
    let b = inject_select(include_bytes!("fixtures/maxwell/tex.bin"), 3, 6, 6, 6);
    error(&b, "TEX handle");
    let bin = include_bytes!("fixtures/maxwell/perspective.bin");
    for (at, message) in [(2, "PositionW"), (3, "reciprocal")] {
        let b = inject_select(bin, at, 7, 7, 7);
        error(&b, message);
    }
}
#[test]
fn selection_keeps_taint_from_candidates_and_predicate() {
    for (left, right) in [(1, 255), (255, 1)] {
        let mut b = include_bytes!("fixtures/maxwell/alu.bin").to_vec();
        change(&mut b, 0x98, |_| 0x5c5800000ff70203);
        let b = inject_select(&b, 6, 2, left, right);
        error(&b, "unfused rounding");
    }
}

#[test]
fn predicate_only_multiply_taint_cannot_hide_rounding_dependency() {
    let mut bin = include_bytes!("fixtures/maxwell/alu.bin").to_vec();
    change(&mut bin, 0x98, |_| 0x5c5800000ff70203);
    let mut b = inject_select(&bin, 6, 2, 4, 4);
    // Compare consumes FMUL R1. Both selected GPR values are untainted LDC R4.
    change(&mut b, 0x98, |w| (w & !(255 << 8)) | (1 << 8));
    error(&b, "unfused rounding");
}
