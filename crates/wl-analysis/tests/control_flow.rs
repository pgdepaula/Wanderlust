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
fn returned_code_addresses_become_speculative_roots() {
    // GNU as/ld 2.46 at 0x10000: ADR X0,target; RET, or
    // ADRP X0,target; ADD X0,X0,:lo12:target; RET.
    // Both getters return a separate MOV W0,#17; RET function.
    for (words, target) in [
        (
            &[0x10000040, 0xd65f03c0, 0x52800220, 0xd65f03c0][..],
            0x10008,
        ),
        (
            &[0x90000000, 0x91003000, 0xd65f03c0, 0x52800220, 0xd65f03c0][..],
            0x1000c,
        ),
    ] {
        let i = image(words);
        let original = i.mappings[0].bytes.clone();
        let p = discover(&i, GuestAddr(0x10000), &[]).unwrap();
        assert_eq!(p.required_roots, 1);
        assert_eq!(p.flow_roots, 1);
        assert!(p.roots.contains(&GuestAddr(target)));
        assert_eq!(p.instruction_count, words.len());
        assert_eq!(i.mappings[0].bytes, original);
    }
}
#[test]
fn non_results_and_non_return_branches_do_not_infer_returned_pointers() {
    // GNU as/ld 2.46: ADR X1,target; RET (wrong result register),
    // ADR X0,target; MOV X0,#0; RET, and ADR X0,target; BR X19.
    for words in [
        &[0x10000041, 0xd65f03c0, 0x52800220, 0xd65f03c0][..],
        &[0x10000060, 0xd2800000, 0xd65f03c0, 0x52800220, 0xd65f03c0][..],
        &[0x10000040, 0xd61f0260, 0x52800220, 0xd65f03c0][..],
    ] {
        let p = discover(&image(words), GuestAddr(0x10000), &[]).unwrap();
        assert_eq!(p.flow_roots, 0);
        assert_eq!(p.instruction_count, words.len() - 2);
    }
}
#[test]
fn returned_candidates_still_require_valid_executable_traversal() {
    for words in [
        &[0x10000060, 0xd65f03c0][..], // ADR X0,0x1000c; RET: unmapped target.
        &[0x10000040, 0xd65f03c0, 0xffffffff][..], // Reserved target instruction.
    ] {
        let p = discover(&image(words), GuestAddr(0x10000), &[]).unwrap();
        assert_eq!(p.instruction_count, 2);
        assert_eq!(p.flow_roots, 0);
    }
}
#[test]
fn loop_is_finite() {
    let i = image(&[0x14000000]);
    let p = discover(&i, GuestAddr(0x10000), &[]).unwrap();
    assert_eq!(p.instruction_count, 1);
}

#[test]
fn impossible_initial_loop_edge_does_not_erase_a_later_table_bound() {
    // GNU as/ld 2.46: start with W24=7, then CMP W24,#2; B.HI change.
    // The change block copies the unknown input W0 and loops. The dispatch
    // must see the later unsigned bound, never the infeasible initial value 7.
    let i = image(&[
        0x528000f8, 0x100001f3, 0x71000b1f, 0x540000a8, 0x100000c9, 0x38786a6a, 0x8b0a0929,
        0xd61f0120, 0x2a0003f8, 0x17fffff9, 0xd28001a0, 0xd65f03c0, 0xd28003a0, 0xd65f03c0,
        0xd28005e0, 0xd65f03c0, 0x00040200,
    ]);
    let p = discover(&i, GuestAddr(0x10000), &[]).unwrap();
    assert_eq!(p.jump_tables.get(&GuestAddr(0x1001c)), Some(&3));
    for pc in [0x10028, 0x10030, 0x10038] {
        assert!(starts(&p).contains(&pc));
    }
    assert!(!starts(&p).contains(&0x10040));
}
#[test]
fn initially_constant_switch_index_can_grow_to_a_guarded_runtime_range() {
    // GNU as/ld 2.46: W24 starts at zero. Case zero copies runtime W0
    // into W24 and loops through CMP W24,#2; B.HI outside. All three
    // table destinations must survive the constant-to-bounded-state join.
    let i = image(&[
        0x52800018, 0x100001f3, 0x71000b1f, 0x54000168, 0x10000089, 0x38786a6a, 0x8b0a0929,
        0xd61f0120, 0x2a0003f8, 0x17fffff9, 0xd28003a0, 0xd65f03c0, 0xd28005e0, 0xd65f03c0,
        0xd2800c60, 0xd65f03c0, 0x00040200,
    ]);
    let p = discover(&i, GuestAddr(0x10000), &[]).unwrap();
    assert_eq!(p.jump_tables.get(&GuestAddr(0x1001c)), Some(&3));
    for pc in [0x10020, 0x10028, 0x10030] {
        assert!(starts(&p).contains(&pc));
    }
    assert!(!starts(&p).contains(&0x10040));
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
        instruction_limit: None,
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
        instruction_limit: None,
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
        instruction_limit: None,
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

// Independent devkitA64 GNU as 2.46 fixture:
// and x9,x0,#3; adr x10,table; adr x11,case0; ldrb w12,[x10,x9];
// add x11,x11,x12,lsl #2; br x11; four MOV/RET cases; .byte 0,2,4,6.
// The input X0 is deliberately unknown: no CMP or caller-specific seed is used.
const MASK_SWITCH: [u32; 15] = [
    0x92400409, 0x100001aa, 0x1000008b, 0x3869694c, 0x8b0c096b, 0xd61f0160, 0xd2800160, 0xd65f03c0,
    0xd28002c0, 0xd65f03c0, 0xd2800420, 0xd65f03c0, 0xd2800580, 0xd65f03c0, 0x06040200,
];

// GNU as 2.46: CMP W0,#3; B.HI default; MOV W8,W0; ADR X9,table;
// ADR X10,case0; LDRB W11,[X9,X8]; ADD X10,X10,X11,LSL #2; BR X10.
// Four MOV/RET cases, a default MOV/RET, and .byte 0,2,4,6 follow.
const MOVED_BOUND_SWITCH: [u32; 19] = [
    0x71000c1f, 0x540001e8, 0x2a0003e8, 0x100001e9, 0x1000008a, 0x3868692b, 0x8b0b094a, 0xd61f0140,
    0xd2800160, 0xd65f03c0, 0xd28002c0, 0xd65f03c0, 0xd2800420, 0xd65f03c0, 0xd2800580, 0xd65f03c0,
    0xd2800c60, 0xd65f03c0, 0x06040200,
];

#[test]
fn narrow_move_promotes_a_low_half_bound_to_a_full_index_bound() {
    let p = discover(&image(&MOVED_BOUND_SWITCH), GuestAddr(0x10000), &[]).unwrap();
    assert_eq!(p.jump_tables.get(&GuestAddr(0x1001c)), Some(&4));
    assert_eq!(p.instruction_count, 18);
    for target in [0x10020, 0x10028, 0x10030, 0x10038, 0x10040] {
        assert!(starts(&p).contains(&target));
    }
}

#[test]
fn immediate_masks_preserve_a_tighter_dominating_switch_bound() {
    // GNU as 2.46.0.20260210: AND X8,X0,#0xffffffff; ANDS X8,X0,#0xffffffff;
    // AND W8,W0,#0xff; AND X8,X0,#7, replacing the independent MOV fixture.
    for word in [0x92407c08, 0xf2407c08, 0x12001c08, 0x92400808] {
        let mut words = MOVED_BOUND_SWITCH;
        words[2] = word;
        let p = discover(&image(&words), GuestAddr(0x10000), &[]).unwrap();
        assert_eq!(
            p.jump_tables.get(&GuestAddr(0x1001c)),
            Some(&4),
            "{word:#x}"
        );
        assert_eq!(p.instruction_count, 18);
        for target in [0x10020, 0x10028, 0x10030, 0x10038, 0x10040] {
            assert!(starts(&p).contains(&target));
        }
        assert!(!starts(&p).contains(&0x10048));
    }
    // The mask can also tighten the comparison: only indices zero and one.
    let mut words = MOVED_BOUND_SWITCH;
    words[2] = 0x92400008; // AND X8,X0,#1.
    let p = discover(&image(&words), GuestAddr(0x10000), &[]).unwrap();
    assert_eq!(p.jump_tables.get(&GuestAddr(0x1001c)), Some(&2));
    assert!(!starts(&p).contains(&0x10030));
    assert!(!starts(&p).contains(&0x10038));
}

#[test]
fn mask_retaining_unknown_upper_bits_cannot_promote_a_low_word_bound() {
    let mut words = MOVED_BOUND_SWITCH;
    words[2] = 0x92408008; // GNU as: AND X8,X0,#0x1ffffffff.
    let p = discover(&image(&words), GuestAddr(0x10000), &[]).unwrap();
    assert!(p.jump_tables.is_empty());
    assert!(!starts(&p).contains(&0x10020));
    // A dominating X comparison proves the full value and permits this mask.
    words[0] = 0xf1000c1f; // GNU as: CMP X0,#3.
    let p = discover(&image(&words), GuestAddr(0x10000), &[]).unwrap();
    assert_eq!(p.jump_tables.get(&GuestAddr(0x1001c)), Some(&4));
}

#[test]
fn unsigned_extraction_proves_zero_upper_bits_before_a_w_comparison() {
    // GNU as 2.46.0.20260210: UBFX X8,X0,#56,#8; CMP W8,#3;
    // B.HI default; MOV X8,X8, followed by the independent byte-table fixture.
    let mut words = vec![0xd378fc08];
    words.extend(MOVED_BOUND_SWITCH);
    words[1] = 0x71000d1f;
    words[3] = 0xaa0803e8;
    let p = discover(&image(&words), GuestAddr(0x10000), &[]).unwrap();
    assert_eq!(p.jump_tables.get(&GuestAddr(0x10020)), Some(&4));
    assert_eq!(p.instruction_count, 19);
    for target in [0x10024, 0x1002c, 0x10034, 0x1003c, 0x10044] {
        assert!(starts(&p).contains(&target));
    }
    assert!(!starts(&p).contains(&0x1004c));
}

#[test]
fn signed_inserted_or_wider_extraction_cannot_fabricate_narrow_index_bounds() {
    for word in [0x9378fc08, 0xb3401c08, 0xd3409c08] {
        // GNU as: SBFX X8,X0,#56,#8; BFI X8,X0,#0,#8;
        // UBFX X8,X0,#0,#40. All can retain nonzero upper 32 bits.
        let mut words = vec![word];
        words.extend(MOVED_BOUND_SWITCH);
        words[1] = 0x71000d1f;
        words[3] = 0xaa0803e8;
        let p = discover(&image(&words), GuestAddr(0x10000), &[]).unwrap();
        assert!(p.jump_tables.is_empty(), "{word:#x}");
        assert!(!starts(&p).contains(&0x10024));
    }
}

#[test]
fn wide_move_does_not_zero_unknown_upper_bits_after_a_narrow_compare() {
    let mut words = MOVED_BOUND_SWITCH;
    words[2] = 0xaa0003e8; // MOV X8,X0: the CMP W0 bounds only the low half.
    let p = discover(&image(&words), GuestAddr(0x10000), &[]).unwrap();
    assert!(p.jump_tables.is_empty());
    assert_eq!(p.instruction_count, 10);
    assert!(!starts(&p).contains(&0x10020));
}

fn relocated_table_image(got: bool) -> Image {
    // Independently assembled GNU-as constructor: ADR table; STR table,[X0];
    // RET, then two MOV/RET callbacks. The GOT variant adds LDR X8,[X8].
    let words: &[u32] = if got {
        &[
            0x10000908, 0xf9400108, 0xf9000008, 0xd65f03c0, 0xd2800160, 0xd65f03c0, 0xd28002c0,
            0xd65f03c0,
        ]
    } else {
        &[
            0x10000808, 0xf9000008, 0xd65f03c0, 0xd2800160, 0xd65f03c0, 0xd28002c0, 0xd65f03c0,
        ]
    };
    let mut i = image(words);
    i.mappings.push(wl_analysis::Mapping {
        base: GuestAddr(0x10100),
        bytes: vec![0; 0x28],
        read: true,
        write: true,
        execute: false,
    });
    let first = if got { 0x10010 } else { 0x1000c };
    let mut cells = std::collections::BTreeMap::from([
        (GuestAddr(0x10100), first),
        (GuestAddr(0x10108), first + 8),
    ]);
    if got {
        cells.insert(GuestAddr(0x10120), 0x10100);
    }
    i.relocation_cells = std::sync::Arc::new(cells);
    i
}

#[test]
fn referenced_relocated_tables_discover_callbacks_without_rewriting_guest_data() {
    for got in [false, true] {
        let i = relocated_table_image(got);
        let p = discover(&i, GuestAddr(0x10000), &[]).unwrap();
        assert_eq!(p.flow_roots, 2);
        assert_eq!(p.instruction_count, if got { 8 } else { 7 });
        assert!(i.mappings[1].bytes.iter().all(|b| *b == 0));
    }
}
#[test]
fn returned_relocated_table_discovers_callbacks_without_rewriting_guest_data() {
    let mut i = relocated_table_image(false);
    // ADR X0,0x10100; NOP; RET. Only relocation provenance supplies
    // the callback addresses; the guest's unrelocated data stays zero.
    i.mappings[0].bytes[..4].copy_from_slice(&0x10000800u32.to_le_bytes());
    i.mappings[0].bytes[4..8].copy_from_slice(&0xd503201fu32.to_le_bytes());
    let p = discover(&i, GuestAddr(0x10000), &[]).unwrap();
    assert_eq!(p.flow_roots, 2);
    assert_eq!(p.instruction_count, 7);
    assert!(i.mappings[1].bytes.iter().all(|&b| b == 0));
}

#[test]
fn absent_unreadable_and_truncated_relocation_sources_are_not_inferred() {
    for case in 0..4 {
        let mut i = relocated_table_image(true);
        match case {
            0 => {
                std::sync::Arc::make_mut(&mut i.relocation_cells).remove(&GuestAddr(0x10120));
            }
            1 => i.mappings[1].read = false,
            2 => i.mappings[1].bytes.truncate(0x24),
            3 => i.mappings[1].bytes.truncate(0x10),
            _ => unreachable!(),
        }
        // Raw bytes cannot substitute for absent relocation provenance.
        if case == 0 {
            i.mappings[1].bytes[0x20..0x28].copy_from_slice(&0x10100u64.to_le_bytes());
        }
        let p = discover(&i, GuestAddr(0x10000), &[]).unwrap();
        assert_eq!(p.flow_roots, 0, "case {case}");
        assert_eq!(p.instruction_count, 4);
    }
}

#[test]
fn function_table_candidates_stop_at_gaps_invalid_targets_and_the_entry_budget() {
    for case in 0..3 {
        let mut i = relocated_table_image(false);
        let cells = std::sync::Arc::make_mut(&mut i.relocation_cells);
        match case {
            0 => {
                cells.remove(&GuestAddr(0x10108));
                cells.insert(GuestAddr(0x10110), 0x10014);
            }
            1 => {
                cells.insert(GuestAddr(0x10108), 0xdead_0000);
            }
            2 => {
                cells.clear();
                for n in 0..257 {
                    cells.insert(
                        GuestAddr(0x10100 + n * 8),
                        if n == 256 { 0x10014 } else { 0x1000c },
                    );
                }
                i.mappings[1].bytes.resize(257 * 8, 0);
            }
            _ => unreachable!(),
        }
        let p = discover(&i, GuestAddr(0x10000), &[]).unwrap();
        assert_eq!(p.flow_roots, 1, "case {case}");
        assert!(!p.roots.contains(&GuestAddr(0x10014)));
    }
}

#[test]
fn long_referenced_tables_retain_methods_beyond_the_first_sixty_four_slots() {
    for last in [63, 64, 123, 255] {
        let mut i = relocated_table_image(false);
        let cells = std::sync::Arc::make_mut(&mut i.relocation_cells);
        cells.clear();
        for n in 0..=last {
            cells.insert(
                GuestAddr(0x10100 + n * 8),
                if n == last { 0x10014 } else { 0x1000c },
            );
        }
        i.mappings[1].bytes.resize((last as usize + 1) * 8, 0);
        let p = discover(&i, GuestAddr(0x10000), &[]).unwrap();
        assert_eq!(p.flow_roots, 2, "last method index {last}");
        assert!(p.roots.contains(&GuestAddr(0x10014)));
    }
}

#[test]
fn unreferenced_relocation_tables_do_not_become_global_roots() {
    let mut i = relocated_table_image(false);
    i.mappings[0].bytes[..4].copy_from_slice(&0xd65f03c0u32.to_le_bytes());
    let p = discover(&i, GuestAddr(0x10000), &[]).unwrap();
    assert_eq!(p.instruction_count, 1);
    assert_eq!(p.flow_roots, 0);
}

#[test]
fn distinct_pointer_table_discovery_has_a_per_pass_work_budget() {
    // The ADR encoder for this bounded fixture was checked against GNU as 2.46
    // for all 1,025 materialized table addresses. No production encoder is used.
    let count = 1025usize;
    let base = 0x10000u64;
    let data = 0x20000u64;
    let first = base + ((count * 2 + 1) * 4) as u64;
    let mut words = Vec::new();
    let mut cells = std::collections::BTreeMap::new();
    for n in 0..count {
        let pc = base + words.len() as u64 * 4;
        let table = data + n as u64 * 16;
        let delta = table - pc;
        words
            .push(0x10000008 | ((delta as u32 & 3) << 29) | (((delta as u32 >> 2) & 0x7ffff) << 5));
        words.push(0xf9000008); // STR X8,[X0].
        cells.insert(GuestAddr(table), if n == 1024 { first + 8 } else { first });
    }
    words.extend([0xd65f03c0, 0xd2800160, 0xd65f03c0, 0xd28002c0, 0xd65f03c0]);
    let mut i = image(&words);
    i.mappings.push(wl_analysis::Mapping {
        base: GuestAddr(data),
        bytes: vec![0; count * 16],
        read: true,
        write: false,
        execute: false,
    });
    i.relocation_cells = std::sync::Arc::new(cells);
    let p = discover(&i, GuestAddr(base), &[]).unwrap();
    assert_eq!(p.flow_roots, 1);
    assert!(p.roots.contains(&GuestAddr(first)));
    assert!(!p.roots.contains(&GuestAddr(first + 8)));
}

#[test]
fn immediate_and_bounds_unknown_indices_without_a_compare() {
    for word in [0x92400409, 0x12000409, 0xf2400409] {
        // AND X, AND W, ANDS X.
        let mut words = MASK_SWITCH;
        words[0] = word;
        let p = discover(&image(&words), GuestAddr(0x10000), &[]).unwrap();
        assert_eq!(p.jump_tables.get(&GuestAddr(0x10014)), Some(&4));
        for target in [0x10018, 0x10020, 0x10028, 0x10030] {
            assert!(starts(&p).contains(&target), "{word:#x}: {target:#x}");
        }
        assert_eq!(p.instruction_count, 14);
        assert!(!starts(&p).contains(&0x10038)); // Table bytes stay data.
    }
}

#[test]
fn masks_do_not_justify_unbounded_or_oversized_table_reads() {
    // ORR does not bound an unknown source; a 17-bit AND exceeds the defensive
    // table-entry limit. Neither may fabricate targets from the four-byte table.
    for word in [0xb2400409, 0x92404009] {
        let mut words = MASK_SWITCH;
        words[0] = word;
        let p = discover(&image(&words), GuestAddr(0x10000), &[]).unwrap();
        assert!(p.jump_tables.is_empty(), "{word:#x}");
        assert_eq!(p.instruction_count, 6);
        assert_eq!(p.indirect_sites, [GuestAddr(0x10014)]);
    }
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
        instruction_limit: None,
    };
    let p = wl_analysis::discover_roots(&image(&SWITCHES), &roots).unwrap();
    assert_eq!(p.required_roots, 2);
    assert_eq!(p.speculative_roots_accepted, 3);
    assert!(p.roots.len() >= 2);
}
#[test]
fn entry_flow_is_resolved_before_speculative_roots_under_the_limit() {
    // With exactly the budget the entry and its own jump table need, a speculative root must
    // not consume it first: the table resolves completely and the speculative root is rejected.
    let base = switches(&[0x10000]);
    let roots = wl_analysis::Roots {
        required: vec![GuestAddr(0x10000)],
        speculative: vec![GuestAddr(0x1005c)],
        allow_unsupported: true,
        instruction_limit: Some(base.instruction_count),
    };
    let p = wl_analysis::discover_roots(&image(&SWITCHES), &roots).unwrap();
    assert_eq!(p.jump_tables.get(&GuestAddr(0x1002c)), Some(&3));
    let starts: Vec<u64> = p.blocks.iter().map(|b| b.start.0).collect();
    for case in [0x10030, 0x10038, 0x10040] {
        assert!(starts.contains(&case));
    }
    assert_eq!(p.instruction_count, base.instruction_count);
    assert_eq!(p.rejected_roots.len(), 1);
    assert!(p.rejected_roots[0].reason.contains("limit"));
}
