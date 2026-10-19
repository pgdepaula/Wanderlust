//! Synthetic ELF64 initialization pointers; no commercial code or host constructor calls.
mod support;
use support::*;
use wl_common::{GuestAddr, ModuleOffset};
use wl_loader::{LinkMode, Process};
use wl_nso::dynamic::*;

fn pointers(p: &Process, kind: &str) -> Vec<u64> {
    p.code_pointers
        .iter()
        .filter(|p| p.source == kind)
        .map(|p| p.address.0)
        .collect()
}

#[test]
fn array_candidates_are_relocated_once_and_guest_storage_is_untouched() {
    for base in [0x800_0000u64, 0x9000_0000] {
        for mode in [LinkMode::Host, LinkMode::Guest] {
            let input = module(&Spec {
                text: vec![RET; 4],
                data: vec![
                    0xdead_beef,
                    0xcafe_babe,
                    base + 16,
                    base + 16,
                    base + 12,
                    0,
                    u64::MAX,
                ],
                relocations: vec![
                    rela(DATA_WORDS, R_AARCH64_RELATIVE, 0, 8),
                    rela(DATA_WORDS + 8, R_AARCH64_RELATIVE, 0, 12),
                ],
                rela_count: 2,
                extra_dynamic: vec![
                    (DT_INIT_ARRAY, DATA_WORDS),
                    (DT_INIT_ARRAYSZ, 32),
                    (DT_FINI_ARRAY, DATA_WORDS + 32),
                    (DT_FINI_ARRAYSZ, 24),
                ],
                ..Spec::default()
            });
            let p = Process::build(&[("main".into(), input)], GuestAddr(base), mode).unwrap();
            assert_eq!(
                pointers(&p, "initializer-array"),
                [base + 8, base + 12, base + 16]
            );
            assert_eq!(pointers(&p, "finalizer-array"), [base + 12]);
            assert_eq!(
                pointers(&p, "relocation"),
                [base + 8, base + 12],
                "retain existing provenance"
            );
            assert_eq!(
                p.relocation_cells.get(&GuestAddr(base + DATA_WORDS)),
                Some(&(base + 8))
            );
            assert_eq!(
                p.relocation_cells.get(&GuestAddr(base + DATA_WORDS + 8)),
                Some(&(base + 12))
            );
            let stored = p.read_u64(GuestAddr(base + DATA_WORDS)).unwrap();
            assert_eq!(
                stored,
                if mode == LinkMode::Guest {
                    0xdead_beef
                } else {
                    base + 8
                }
            );
            assert!(
                p.link.init_order.is_empty(),
                "discovery must not invent a constructor schedule"
            );
        }
    }
}

#[test]
fn symbolic_array_entry_can_target_another_module() {
    let main = module(&Spec {
        symbols: vec![undefined("constructor")],
        data: vec![0],
        relocations: vec![rela(DATA_WORDS, R_AARCH64_ABS64, 1, 0)],
        extra_dynamic: vec![(DT_INIT_ARRAY, DATA_WORDS), (DT_INIT_ARRAYSZ, 8)],
        ..Spec::default()
    });
    let sdk = module(&Spec {
        symbols: vec![defined("constructor", 8)],
        ..Spec::default()
    });
    let p = Process::build(
        &[("main".into(), main), ("sdk".into(), sdk)],
        GuestAddr(0x800_0000),
        LinkMode::Guest,
    )
    .unwrap();
    let expected = p.address("sdk", ModuleOffset(8)).unwrap();
    let init = p
        .code_pointers
        .iter()
        .find(|p| p.source == "initializer-array")
        .unwrap();
    assert_eq!(init.address, expected);
    assert_eq!(
        p.relocation_cells
            .get(&p.address("main", ModuleOffset(DATA_WORDS)).unwrap()),
        Some(&expected.0)
    );
    assert_eq!(
        init.module, "main",
        "provenance is the module containing the array"
    );
    assert_eq!(
        p.read_u64(p.address("main", ModuleOffset(DATA_WORDS)).unwrap())
            .unwrap(),
        0
    );
}

#[test]
fn implicit_addends_are_read_from_the_relocated_analysis_copy() {
    let input = module(&Spec {
        data: vec![8],
        relocations: vec![rel(DATA_WORDS, R_AARCH64_RELATIVE, 0)],
        rel_count: 1,
        extra_dynamic: vec![(DT_INIT_ARRAY, DATA_WORDS), (DT_INIT_ARRAYSZ, 8)],
        ..Spec::default()
    });
    let p = Process::build(
        &[("main".into(), input)],
        GuestAddr(0x800_0000),
        LinkMode::Guest,
    )
    .unwrap();
    assert_eq!(pointers(&p, "initializer-array"), [0x800_0008]);
    assert_eq!(p.read_u64(GuestAddr(0x800_0000 + DATA_WORDS)).unwrap(), 8);
    assert_eq!(
        p.relocation_cells.get(&GuestAddr(0x800_0000 + DATA_WORDS)),
        Some(&0x800_0008)
    );
}

#[test]
fn pointer_cell_index_uses_final_relocation_bytes() {
    for replacement in [DATA_WORDS, 12] {
        let input = module(&Spec {
            text: vec![RET; 4],
            data: vec![0],
            relocations: vec![
                rela(DATA_WORDS, R_AARCH64_RELATIVE, 0, 8),
                rela(DATA_WORDS, R_AARCH64_RELATIVE, 0, replacement as i64),
            ],
            rela_count: 2,
            ..Spec::default()
        });
        let base = 0x800_0000;
        let p =
            Process::build(&[("main".into(), input)], GuestAddr(base), LinkMode::Guest).unwrap();
        assert_eq!(
            p.relocation_cells
                .get(&GuestAddr(base + DATA_WORDS))
                .copied(),
            Some(base + replacement)
        );
        assert_eq!(p.read_u64(GuestAddr(base + DATA_WORDS)), Some(0));
    }
}

#[test]
fn partial_symbolic_write_replaces_the_recorded_pointer_value() {
    for base in [0x800_0000u64, 0x2_0000_0000] {
        let input = module(&Spec {
            data: vec![0],
            relocations: vec![
                rela(DATA_WORDS, R_AARCH64_RELATIVE, 0, 8),
                rela(DATA_WORDS, R_AARCH64_ABS32, 0, 0),
            ],
            rela_count: 1,
            ..Spec::default()
        });
        let p =
            Process::build(&[("main".into(), input)], GuestAddr(base), LinkMode::Guest).unwrap();
        let expected = Some(base & !u64::from(u32::MAX));
        assert_eq!(
            p.relocation_cells
                .get(&GuestAddr(base + DATA_WORDS))
                .copied(),
            expected
        );
        assert_eq!(p.read_u64(GuestAddr(base + DATA_WORDS)), Some(0));
    }
}

#[test]
fn unapplied_relocation_does_not_manufacture_an_initializer_pointer() {
    let input = module(&Spec {
        data: vec![8],
        relocations: vec![rela(DATA_WORDS, R_AARCH64_RELATIVE, 0, 8)],
        rela_count: 0,
        extra_dynamic: vec![(DT_INIT_ARRAY, DATA_WORDS), (DT_INIT_ARRAYSZ, 8)],
        ..Spec::default()
    });
    let p = Process::build(
        &[("main".into(), input)],
        GuestAddr(0x800_0000),
        LinkMode::Guest,
    )
    .unwrap();
    assert!(pointers(&p, "initializer-array").is_empty());
    assert!(
        p.link
            .diagnostics
            .iter()
            .any(|d| d.kind == "relative-outside-count")
    );
}

#[test]
fn malformed_pointer_arrays_and_resource_overflow_are_explicit_errors() {
    for (offset, size) in [(DATA_WORDS, 7), (u64::MAX - 7, 16), (0xf000, 8)] {
        let bytes = bytes(&Spec {
            extra_dynamic: vec![(DT_INIT_ARRAY, offset), (DT_INIT_ARRAYSZ, size)],
            ..Spec::default()
        });
        assert!(wl_nso::Module::parse(&bytes).is_err());
    }
    // Public Module values can also be modified by callers after parsing.
    for (offset, size, expected) in [
        (DATA_WORDS, 8 * (1_048_576 + 1), "entry budget"),
        (u64::MAX, 8, "address overflow"),
        (0xf000, 8, "not mapped"),
        (DATA_WORDS, 7, "multiple of 8"),
    ] {
        let mut input = module(&Spec::default());
        input.mod0.as_mut().unwrap().dynamic.init_array = Some(Range {
            offset: ModuleOffset(offset),
            size,
        });
        let error = Process::build(
            &[("main".into(), input)],
            GuestAddr(0x800_0000),
            LinkMode::Guest,
        )
        .unwrap_err();
        assert!(error.contains(expected), "{error}");
    }
}

#[test]
fn empty_arrays_have_no_candidates_or_guest_side_effects() {
    let input = module(&Spec {
        data: vec![0x1234],
        extra_dynamic: vec![(DT_INIT_ARRAY, DATA_WORDS), (DT_INIT_ARRAYSZ, 0)],
        ..Spec::default()
    });
    let p = Process::build(
        &[("main".into(), input)],
        GuestAddr(0x800_0000),
        LinkMode::Guest,
    )
    .unwrap();
    assert!(pointers(&p, "initializer-array").is_empty());
    assert_eq!(
        p.read_u64(GuestAddr(0x800_0000 + DATA_WORDS)).unwrap(),
        0x1234
    );
}
