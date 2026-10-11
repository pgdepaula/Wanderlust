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
