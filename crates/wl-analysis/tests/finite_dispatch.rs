use wl_analysis::{Image, Program, discover};
use wl_common::GuestAddr;

// Independently assembled with GNU as 2.46.0.20260210. Unknown W0/W1 are
// masked to 0..3 and 0..1, multiplied, then decremented modulo 2^32.
// ADR table; LDRB; ADR case0; ADD shifted; BR implements the dispatch.
// The three valid table bytes are 0,2,4; the underflow index stays unknown.
const WORDS: [u32; 16] = [
    0x12000408, 0x12000026, 0x1b067d08, 0x51000508, 0x10000169, 0x3868692b, 0x1000006a, 0x8b0b094a,
    0xd61f0140, 0x52800160, 0xd65f03c0, 0x528002c0, 0xd65f03c0, 0x52800420, 0xd65f03c0, 0x00040200,
];
fn analyze(words: &[u32]) -> Program {
    let bytes: Vec<_> = words.iter().flat_map(|w| w.to_le_bytes()).collect();
    let image = Image::raw(bytes.clone(), GuestAddr(0x10000)).unwrap();
    let program = discover(&image, GuestAddr(0x10000), &[]).unwrap();
    assert_eq!(image.mappings[0].bytes, bytes);
    program
}
fn assert_candidates(program: &Program) {
    for target in [0x10024, 0x1002c, 0x10034] {
        assert!(program.blocks.iter().any(|b| b.start.0 == target));
    }
    assert!(program.indirect_sites.contains(&GuestAddr(0x10020)));
    // Unknown/out-of-image indices must not be advertised as a complete table.
    assert!(program.jump_tables.is_empty());
}
#[test]
fn masked_product_and_modular_decrement_discover_all_mapped_cases() {
    assert_candidates(&analyze(&WORDS));
}
#[test]
fn wide_modular_decrement_preserves_candidates_without_hiding_invalid_case() {
    let mut words = WORDS;
    // GNU as: AND X8,X0,#3; AND X6,X1,#1; MUL X8,X8,X6; SUB X8,X8,#1.
    words[..4].copy_from_slice(&[0x92400408, 0x92400026, 0x9b067d08, 0xd1000508]);
    assert_candidates(&analyze(&words));
}
#[test]
fn unbounded_multiplicand_does_not_invent_dispatch_targets() {
    let mut words = WORDS;
    words[1] = 0x2a0103e6; // GNU as: MOV W6,W1; unknown operand.
    let program = analyze(&words);
    assert!(program.jump_tables.is_empty());
    assert!(!program.blocks.iter().any(|b| b.start.0 == 0x10034));
}
#[test]
fn subtract_product_does_not_become_an_unsigned_positive_bound() {
    let mut words = WORDS;
    words[2] = 0x1b06fd08; // GNU as: MSUB W8,W8,W6,WZR.
    let program = analyze(&words);
    assert!(program.jump_tables.is_empty());
    assert!(!program.blocks.iter().any(|b| b.start.0 == 0x10034));
}

#[test]
fn whole_image_scan_covers_disconnected_dispatch_families() {
    let words: Vec<_> = WORDS.into_iter().chain(WORDS).collect();
    let bytes: Vec<_> = words.iter().flat_map(|w| w.to_le_bytes()).collect();
    let image = Image::raw(bytes.clone(), GuestAddr(0x10000)).unwrap();
    let scan = wl_analysis::flow::scan_indirect_candidates(&image).unwrap();
    assert_eq!(scan.executable_words, 32);
    assert_eq!(scan.indirect_sites, 2);
    assert_eq!(scan.complete_local_tables, 0);
    assert_eq!(
        scan.unresolved_sites,
        [GuestAddr(0x10020), GuestAddr(0x10060)]
    );
    for target in [0x10024, 0x1002c, 0x10034, 0x10064, 0x1006c, 0x10074] {
        assert!(scan.candidates.contains(&GuestAddr(target)));
    }
    assert_eq!(image.mappings[0].bytes, bytes);
}

#[test]
fn scan_does_not_carry_facts_from_bypassed_mask_instructions() {
    // GNU as: CBZ W2,dispatch; the taken path bypasses all index masks/MUL.
    let words: Vec<_> = std::iter::once(0x34000082).chain(WORDS).collect();
    let bytes = words.iter().flat_map(|w| w.to_le_bytes()).collect();
    let image = Image::raw(bytes, GuestAddr(0x10000)).unwrap();
    let scan = wl_analysis::flow::scan_indirect_candidates(&image).unwrap();
    assert_eq!(scan.indirect_sites, 1);
    assert!(scan.candidates.is_empty());
    assert_eq!(scan.unresolved_sites, [GuestAddr(0x10024)]);
}

#[test]
fn scan_checks_mapping_arithmetic_before_decoding() {
    let mut image = Image::raw(vec![0; 8], GuestAddr(0x10000)).unwrap();
    image.mappings[0].base = GuestAddr(u64::MAX - 3);
    assert!(wl_analysis::flow::scan_indirect_candidates(&image).is_err());
}

#[test]
fn scan_rejects_an_excessive_site_count_without_claiming_complete_inventory() {
    let bytes = 0xd61f0000u32
        .to_le_bytes()
        .repeat(wl_analysis::flow::MAX_DISPATCH_SITES + 1);
    let image = Image::raw(bytes, GuestAddr(0x10000)).unwrap();
    assert!(
        wl_analysis::flow::scan_indirect_candidates(&image)
            .unwrap_err()
            .contains("budget")
    );
}

#[test]
fn zero_product_underflow_does_not_turn_into_a_valid_case_zero() {
    let mut words = WORDS;
    words[1] = 0x2a1f03e6; // GNU as: MOV W6,WZR.
    let program = analyze(&words);
    assert!(program.jump_tables.is_empty());
    assert!(!program.blocks.iter().any(|b| b.start.0 == 0x10024));
}

// GNU as 2.46: CMP W0,#3; B.HI default; MOV X8,X0; ADR X9,table;
// ADR X10,case0; LDRH W11,[X9,X8,LSL #1]; ADD X10,X10,X11,LSL #2;
// BR X10. The comparison deliberately leaves X0's upper word unknown.
const LOW_WORD_SLICE: [u32; 20] = [
    0x71000c1f, 0x540001e8, 0xaa0003e8, 0x100001e9, 0x1000008a, 0x7868792b, 0x8b0b094a, 0xd61f0140,
    0x52800160, 0xd65f03c0, 0x528002c0, 0xd65f03c0, 0x52800420, 0xd65f03c0, 0x52800580, 0xd65f03c0,
    0x52800c60, 0xd65f03c0, 0x00020000, 0x00060004,
];

#[test]
fn advisory_scan_enumerates_low_word_slice_without_proving_a_full_index_bound() {
    let bytes: Vec<_> = LOW_WORD_SLICE
        .iter()
        .flat_map(|w| w.to_le_bytes())
        .collect();
    let image = Image::raw(bytes.clone(), GuestAddr(0x10000)).unwrap();
    let strict = discover(&image, GuestAddr(0x10000), &[]).unwrap();
    assert!(strict.jump_tables.is_empty());
    assert!(!strict.blocks.iter().any(|b| b.start.0 == 0x10020));
    let scan = wl_analysis::flow::scan_indirect_candidates(&image).unwrap();
    for pc in [0x10020, 0x10028, 0x10030, 0x10038] {
        assert!(scan.candidates.contains(&GuestAddr(pc)));
    }
    assert_eq!(scan.complete_local_tables, 0);
    assert_eq!(scan.unresolved_sites, [GuestAddr(0x1001c)]);
    assert_eq!(image.mappings[0].bytes, bytes);
}

#[test]
fn advisory_slice_still_requires_a_bound_and_mapped_table_entries() {
    for replacement in [0xd503201f, 0x7100001f] {
        let mut words = LOW_WORD_SLICE;
        words[0] = replacement; // NOP (unbounded), or CMP W0,#0 (one candidate).
        let image = Image::raw(
            words.iter().flat_map(|w| w.to_le_bytes()).collect(),
            GuestAddr(0x10000),
        )
        .unwrap();
        let scan = wl_analysis::flow::scan_indirect_candidates(&image).unwrap();
        assert_eq!(
            scan.candidates.len(),
            usize::from(replacement != 0xd503201f)
        );
        assert_eq!(scan.complete_local_tables, 0);
    }
    let mut words = LOW_WORD_SLICE;
    words[18] = 0xffff0000; // First case valid; second points outside the mapping.
    let image = Image::raw(
        words.iter().flat_map(|w| w.to_le_bytes()).collect(),
        GuestAddr(0x10000),
    )
    .unwrap();
    let scan = wl_analysis::flow::scan_indirect_candidates(&image).unwrap();
    assert_eq!(scan.candidates.len(), 3);
    assert_eq!(scan.complete_local_tables, 0);
}
