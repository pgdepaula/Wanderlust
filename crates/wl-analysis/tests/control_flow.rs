use wl_analysis::{Image, discover};
use wl_common::GuestAddr;
fn image(words: &[u32]) -> Image {
    Image::raw(
        words.iter().flat_map(|w| w.to_le_bytes()).collect(),
        GuestAddr(0x10000),
    )
    .unwrap()
}
#[test]
fn skips_unreachable_data() {
    let i = image(&[0x14000002, 0xffffffff, 0xd65f03c0]);
    let p = discover(&i, GuestAddr(0x10000), &[]).unwrap();
    assert_eq!(p.instruction_count, 2);
    assert_eq!(p.blocks.len(), 2);
}
#[test]
fn discovers_call_and_return_site() {
    let i = image(&[0x94000002, 0xd65f03c0, 0xd65f03c0]);
    let p = discover(&i, GuestAddr(0x10000), &[]).unwrap();
    assert_eq!(p.instruction_count, 3);
    assert_eq!(p.indirect_sites.len(), 2);
}
#[test]
fn loop_is_finite() {
    let i = image(&[0x14000000]);
    let p = discover(&i, GuestAddr(0x10000), &[]).unwrap();
    assert_eq!(p.instruction_count, 1);
}
#[test]
fn explicit_seeds_recover_indirect_targets() {
    let i = image(&[0xd61f0000, 0xffffffff, 0xd65f03c0]);
    let p = discover(&i, GuestAddr(0x10000), &[GuestAddr(0x10008)]).unwrap();
    assert_eq!(p.instruction_count, 2);
}
#[test]
fn reports_first_reachable_failure() {
    let i = image(&[0xd503201f, 0xffffffff]);
    assert!(
        discover(&i, GuestAddr(0x10000), &[])
            .unwrap_err()
            .contains("0x10004")
    );
    let i = image(&[0x14000002]);
    assert!(
        discover(&i, GuestAddr(0x10000), &[])
            .unwrap_err()
            .contains("outside executable")
    );
}

#[test]
fn survey_distinguishes_linear_words_from_reachability() {
    let i = image(&[0x14000002, 0xffffffff, 0xd65f03c0, 0xffffffff]);
    let s = wl_analysis::survey(&i, 32).unwrap();
    assert_eq!(s.words, 4);
    assert_eq!(s.supported, 2);
    assert_eq!(s.unsupported_words, 2);
    assert_eq!(s.unsupported_unique, 1);
    assert_eq!(s.unsupported_examples[0].first_pc, GuestAddr(0x10004));
}

#[test]
fn speculative_roots_are_rolled_back_on_structural_failure() {
    use wl_analysis::{Roots, discover_roots};
    // 0x10000 RET; 0x10004 NOP; 0x10008 B +0x100 (leaves the mapping); 0x1000c RET.
    let i = image(&[0xd65f03c0, 0xd503201f, 0x14000040, 0xd65f03c0]);
    let roots = Roots {
        required: vec![GuestAddr(0x10000)],
        speculative: vec![GuestAddr(0x10004), GuestAddr(0x1000c), GuestAddr(0x10002)],
        allow_unsupported: false,
    };
    let p = discover_roots(&i, &roots).unwrap();
    assert_eq!(p.speculative_roots_accepted, 1);
    assert_eq!(p.rejected_roots.len(), 2);
    assert!(p.rejected_roots[0].reason.contains("outside executable"));
    assert!(p.rejected_roots[1].reason.contains("unaligned"));
    // NOP and the branch reached only from the rejected root are not part of the program.
    assert_eq!(p.instruction_count, 2);
    assert!(p.blocks.iter().all(|b| b.start != GuestAddr(0x10004)));
}
#[test]
fn unsupported_sites_terminate_blocks_when_allowed() {
    use wl_analysis::{Roots, discover_roots};
    let i = image(&[0xd503201f, 0xffffffff, 0xd65f03c0]);
    let roots = Roots {
        required: vec![GuestAddr(0x10000)],
        allow_unsupported: true,
        ..Roots::default()
    };
    let p = discover_roots(&i, &roots).unwrap();
    assert_eq!(
        (p.instruction_count, p.unsupported_count, p.blocks.len()),
        (1, 1, 1)
    );
    let site = p.blocks[0].unsupported.as_ref().unwrap();
    assert_eq!((site.pc, site.word), (GuestAddr(0x10004), 0xffffffff));
}

/// Assembled with devkitA64 `as` at 0x10000 (source in the test comments):
/// * 0x10000 GCC-style switch: halfword table at 0x100bc addressed by ADRP/ADD into callee-
///   saved X19 before a loop containing a call; `cmp w1, #2; b.hi`; `ldrh w1, [x19, w1, uxtw
///   #1]; adr x2, base; add x1, x2, w1, sxth #2; br x1` (cases 0x10030/38/40).
/// * 0x1005c Clang-style: `and w8, w0, #0xff; cmp w8, #1; b.hi`; byte table 0x100c4 indexed by
///   X8 (64-bit index after a W-register definition); `add x10, x10, x11, lsl #2; br x10`.
/// * 0x10098 the same dispatch without a bounding compare (must stay unresolved).
/// * 0x100ac `adrp x1, helper; add x1, x1, :lo12:helper; str x1, [x0]` (a code pointer).
const SWITCHES: [u32; 50] = [
    0xA9BE7BFD, 0xF9000BF3, 0x90000013, 0x9102F273, 0x94000011, 0x51000401, 0x7100083F, 0x54000168,
    0x78615A61, 0x10000062, 0x8B21A841, 0xD61F0020, 0xD2800140, 0x17FFFFF7, 0xD2800160, 0x17FFFFF5,
    0xD2800180, 0x17FFFFF3, 0xF9400BF3, 0xA8C27BFD, 0xD65F03C0, 0xD2800060, 0xD65F03C0, 0x12001C08,
    0x7100051F, 0x54000168, 0x90000009, 0x91031129, 0x1000008A, 0x3868692B, 0x8B0B094A, 0xD61F0140,
    0xD2800020, 0xD65F03C0, 0xD2800040, 0xD65F03C0, 0xD2800000, 0xD65F03C0, 0x10000169, 0x3860492B,
    0x10FFFF0A, 0x8B0B094A, 0xD61F0140, 0x90000001, 0x91015021, 0xF9000001, 0xD65F03C0, 0x00020000,
    0x00000004, 0x00000200,
];
fn switches(entries: &[u64]) -> wl_analysis::Program {
    let roots = wl_analysis::Roots {
        required: entries.iter().map(|&e| GuestAddr(e)).collect(),
        speculative: vec![],
        allow_unsupported: true,
    };
    wl_analysis::discover_roots(&image(&SWITCHES), &roots).unwrap()
}
fn starts(p: &wl_analysis::Program) -> Vec<u64> {
    p.blocks.iter().map(|b| b.start.0).collect()
}
#[test]
fn resolves_bounded_jump_tables() {
    let p = switches(&[0x10000, 0x1005c]);
    let tables: Vec<(u64, usize)> = p.jump_tables.iter().map(|(b, n)| (b.0, *n)).collect();
    assert_eq!(tables, [(0x1002c, 3), (0x1007c, 2)]);
    let s = starts(&p);
    for case in [0x10030, 0x10038, 0x10040, 0x10080, 0x10088] {
        assert!(s.contains(&case), "{case:#x} is not a block leader");
    }
    // Table data after the code is never decoded as instructions.
    assert!(!s.iter().any(|&b| b >= 0x100bc));
}
#[test]
fn unbounded_index_and_unknown_base_stay_unresolved() {
    let p = switches(&[0x10098]);
    assert!(p.jump_tables.is_empty());
    assert_eq!(p.flow_roots, 0);
    assert_eq!(p.indirect_sites, [GuestAddr(0x100a8)]);
}
#[test]
fn stored_code_address_becomes_a_root() {
    let p = switches(&[0x100ac]);
    assert!(p.roots.contains(&GuestAddr(0x10054)));
    assert!(starts(&p).contains(&0x10054));
    assert_eq!(p.flow_roots, 1);
}
#[test]
fn duplicate_roots_are_counted_without_underflow() {
    // The same address as entry, twice as a speculative root, and reached again by flow.
    let roots = wl_analysis::Roots {
        required: vec![GuestAddr(0x10000), GuestAddr(0x10054)],
        speculative: vec![GuestAddr(0x10000), GuestAddr(0x10054), GuestAddr(0x10054)],
        allow_unsupported: true,
    };
    let p = wl_analysis::discover_roots(&image(&SWITCHES), &roots).unwrap();
    assert_eq!(p.required_roots, 2);
    assert_eq!(p.speculative_roots_accepted, 3);
    assert!(p.roots.len() >= 2);
}
