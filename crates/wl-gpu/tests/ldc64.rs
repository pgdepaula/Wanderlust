use wl_gpu::shader_program::{ConstantRef, FragmentProgram, Operation, VertexProgram};

const BIN: &[u8] = include_bytes!("fixtures/maxwell/ldc64.bin");
const TEX: &[u8] = include_bytes!("fixtures/maxwell/tex-ldc64.bin");
const LOAD: u64 = 0xef9500000007ff00;
const EXIT: u64 = 0xe30000000007000f;
const NOP: u64 = 0x50b0000000070f00;
const WRITE: u64 = 0x706;
const WAIT_WRITE: u64 = 0xf06;
const READY: u64 = 0xfe6;

fn program(header: &[u8], items: &[(u64, u64)]) -> Vec<u8> {
    let mut out = header[..80].to_vec();
    for group in items.chunks(3) {
        let control = (0..3).fold(0, |c, n| {
            c | (group.get(n).map_or(0x7e6, |i| i.1) << (n * 21))
        });
        out.extend(control.to_le_bytes());
        for n in 0..3 {
            out.extend(group.get(n).map_or(NOP, |i| i.0).to_le_bytes());
        }
    }
    out
}
fn load(dst: u8, offset: u16) -> u64 {
    LOAD | u64::from(dst) | (u64::from(offset) << 20)
}
fn mov(dst: u8, src: u8) -> u64 {
    0x5c98078000070000 | u64::from(dst) | (u64::from(src) << 20)
}
fn validate(p: &FragmentProgram) {
    let module = naga::front::wgsl::parse_str(&p.wgsl()).unwrap();
    naga::valid::Validator::new(
        naga::valid::ValidationFlags::all(),
        naga::valid::Capabilities::all(),
    )
    .validate(&module)
    .unwrap();
}

#[test]
fn independently_assembled_pairs_lower_each_word_and_validate() {
    assert_eq!(
        u64::from_le_bytes(BIN[88..96].try_into().unwrap()),
        load(0, 16)
    );
    assert_eq!(
        u64::from_le_bytes(BIN[96..104].try_into().unwrap()),
        load(2, 24)
    );
    let p = FragmentProgram::decode(BIN).unwrap();
    assert_eq!(p.required_registers(), 4);
    assert_eq!(
        p.constant_pairs(),
        &[16, 24].map(|offset| ConstantRef { slot: 0, offset })
    );
    assert_eq!(
        p.constants(),
        &[16, 20, 24, 28].map(|offset| ConstantRef { slot: 0, offset })
    );
    assert_eq!(
        p.operations(),
        &(0..4)
            .map(|dst| Operation::Constant { dst, input: dst })
            .collect::<Vec<_>>()
    );
    validate(&p);
    assert!(VertexProgram::decode(BIN).is_err());
    for prefix in 0..BIN.len() {
        assert!(FragmentProgram::decode(&BIN[..prefix]).is_err());
    }
}

#[test]
fn pair_barrier_covers_both_lanes_and_cannot_be_reused_early() {
    for src in [0, 1] {
        let b = program(
            BIN,
            &[(load(0, 16), WRITE), (mov(2, src), 0x7e6), (EXIT, READY)],
        );
        assert!(
            FragmentProgram::decode(&b)
                .unwrap_err()
                .to_string()
                .contains("unready MOV")
        );
    }
    for dst in [0u8, 1] {
        let b = program(
            BIN,
            &[(load(0, 16), WRITE), (mov(dst, 255), 0x7e6), (EXIT, READY)],
        );
        assert!(
            FragmentProgram::decode(&b)
                .unwrap_err()
                .to_string()
                .contains("write to pending register")
        );
    }
    for schedule in [WRITE, 0x7e6] {
        let b = program(
            BIN,
            &[(load(0, 16), WRITE), (load(2, 24), schedule), (EXIT, READY)],
        );
        assert!(
            FragmentProgram::decode(&b)
                .unwrap_err()
                .to_string()
                .contains("barrier zero")
        );
    }
    let b = program(
        BIN,
        &[
            (load(0, 16), WRITE),
            (load(2, 24), WAIT_WRITE),
            (EXIT, 0x7e6),
        ],
    );
    assert!(
        FragmentProgram::decode(&b)
            .unwrap_err()
            .to_string()
            .contains("unready fragment exports")
    );
    // A single wait releases both lanes; repeated writes are permitted afterward.
    let b = program(
        BIN,
        &[
            (load(0, 16), WRITE),
            (load(0, 24), WAIT_WRITE),
            (mov(2, 0), READY),
            (mov(3, 1), 0x7e6),
            (EXIT, 0x7e6),
        ],
    );
    validate(&FragmentProgram::decode(&b).unwrap());
}

#[test]
fn conservative_alignment_pair_bounds_and_unsupported_modes() {
    for dst in [1, 253, 254, 255] {
        let b = program(
            BIN,
            &[
                (load(dst, 16), WRITE),
                (load(2, 24), WAIT_WRITE),
                (EXIT, READY),
            ],
        );
        assert!(FragmentProgram::decode(&b).is_err());
    }
    for offset in [1, 4, 0x7ffc, 0x8000, 0xfff8] {
        let b = program(
            BIN,
            &[
                (load(0, offset), WRITE),
                (load(2, 24), WAIT_WRITE),
                (EXIT, READY),
            ],
        );
        assert!(FragmentProgram::decode(&b).is_err());
    }
    let b = program(
        BIN,
        &[
            (load(252, 0x7ff8) | (31 << 36), WRITE),
            (load(0, 24), WAIT_WRITE),
            (mov(2, 252), READY),
            (mov(3, 253), 0x7e6),
            (EXIT, 0x7e6),
        ],
    );
    let p = FragmentProgram::decode(&b).unwrap();
    assert_eq!(
        p.constants()[..2],
        [
            ConstantRef {
                slot: 31,
                offset: 0x7ff8
            },
            ConstantRef {
                slot: 31,
                offset: 0x7ffc
            }
        ]
    );
    assert_eq!(p.required_registers(), 254);
    validate(&p);
    for mask in [1 << 16, 1 << 8, 1 << 44, 1 << 45, 3 << 48] {
        // Includes unverified size6 (B128), outer predicate and address GPR.
        let b = program(
            BIN,
            &[
                (load(0, 16) ^ mask, WRITE),
                (load(2, 24), WAIT_WRITE),
                (EXIT, READY),
            ],
        );
        assert!(
            FragmentProgram::decode(&b)
                .unwrap_err()
                .to_string()
                .contains("unsupported instruction")
        );
    }
}

#[test]
fn deduplicated_pair_inputs_need_not_be_consecutive() {
    let b = program(
        BIN,
        &[
            (0xef9400000187ff04, WRITE),
            (load(0, 16), WAIT_WRITE),
            (load(2, 24), WAIT_WRITE),
            (EXIT, READY),
        ],
    );
    let p = FragmentProgram::decode(&b).unwrap();
    assert_eq!(
        p.constants(),
        &[24, 16, 20, 28].map(|offset| ConstantRef { slot: 0, offset })
    );
    assert_eq!(
        p.operations()[3..],
        [
            Operation::Constant { dst: 2, input: 0 },
            Operation::Constant { dst: 3, input: 3 }
        ]
    );
    validate(&p);
}

#[test]
fn texture_handle_tracks_second_lane_and_moves_without_specialization() {
    let p = FragmentProgram::decode(TEX).unwrap();
    assert_eq!(
        p.constants(),
        &[
            ConstantRef { slot: 0, offset: 0 },
            ConstantRef { slot: 0, offset: 4 }
        ]
    );
    assert_eq!(p.texture_input(), Some(1));
    validate(&p);
    // A ready MOV of the high lane retains that lane's CB origin.
    let mut items = Vec::new();
    for bundle in TEX[80..].chunks_exact(32) {
        let control = u64::from_le_bytes(bundle[..8].try_into().unwrap());
        for n in 0..3 {
            items.push((
                u64::from_le_bytes(bundle[8 + n * 8..16 + n * 8].try_into().unwrap()),
                (control >> (21 * n)) & 0x1fffff,
            ));
        }
    }
    items.insert(3, (mov(7, 7), READY));
    assert_eq!(
        FragmentProgram::decode(&program(TEX, &items))
            .unwrap()
            .texture_input(),
        Some(1)
    );
    items[3].0 = mov(7, 6);
    assert_eq!(
        FragmentProgram::decode(&program(TEX, &items))
            .unwrap()
            .texture_input(),
        Some(0)
    );
}

#[test]
fn pair_reference_budget_includes_both_lanes_after_deduplication() {
    let mut items = Vec::new();
    for n in 0..31 {
        items.push((load(0, n * 8), if n == 0 { WRITE } else { WAIT_WRITE }));
    }
    // 62 distinct words plus a scalar reference to the first word of pair31.
    items.push((0xef9400000f87ff04, WAIT_WRITE));
    let tail = items.len();
    items.extend([(load(2, 248), WAIT_WRITE), (EXIT, READY)]);
    let p = FragmentProgram::decode(&program(BIN, &items)).unwrap();
    assert_eq!(p.constants().len(), 64);
    assert_eq!(
        p.operations().last(),
        Some(&Operation::Constant { dst: 3, input: 63 })
    );
    validate(&p);
    // Both fresh words would reach65. Failure at the second lane is a failed
    // decode, not a returned half-program or a wrapped uniform-array index.
    items[tail].0 = load(2, 256);
    assert!(
        FragmentProgram::decode(&program(BIN, &items))
            .unwrap_err()
            .to_string()
            .contains("too many constant references")
    );
}

#[test]
fn pair_overwrite_clears_special_metadata_on_each_lane() {
    let perspective = include_bytes!("fixtures/maxwell/perspective.bin");
    let mut items = Vec::new();
    for bundle in perspective[80..].chunks_exact(32) {
        let control = u64::from_le_bytes(bundle[..8].try_into().unwrap());
        for n in 0..3 {
            items.push((
                u64::from_le_bytes(bundle[8 + n * 8..16 + n * 8].try_into().unwrap()),
                (control >> (n * 21)) & 0x1fffff,
            ));
        }
    }
    // R7 used to contain PositionW or its reciprocal. Replacing R6/R7 with a
    // pair must erase the upper lane's old special interpolation origin.
    for (at, message) in [(2, "PositionW"), (3, "reciprocal")] {
        let mut modified = items.clone();
        modified.insert(at, (load(6, 0), WAIT_WRITE));
        assert!(
            FragmentProgram::decode(&program(perspective, &modified))
                .unwrap_err()
                .to_string()
                .contains(message)
        );
    }
    // Independently encoded FMUL into either lane, followed by an actual B64
    // overwrite, cannot keep a stale multiply-result dependency on a later ADD.
    for dst in [0u8, 1] {
        let b = program(
            BIN,
            &[
                (load(0, 16), WRITE),
                (0x5c68000000170000 | u64::from(dst), READY),
                (load(0, 24), WRITE),
                (0x5c5800000ff70002 | (u64::from(dst) << 8), READY),
                (mov(3, 1), 0x7e6),
                (EXIT, 0x7e6),
            ],
        );
        validate(&FragmentProgram::decode(&b).unwrap());
    }
}
