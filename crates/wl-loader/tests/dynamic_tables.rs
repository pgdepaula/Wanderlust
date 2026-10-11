//! Malformed MOD0 dynamic metadata must be rejected by `wl_nso::Module::parse`.
mod support;
use support::*;
use wl_nso::dynamic::*;

fn with(extra: Vec<(i64, u64)>, omit: Vec<i64>, relocations: Vec<Rel>) -> Spec {
    Spec {
        symbols: vec![defined("f", 8)],
        relocations,
        extra_dynamic: extra,
        omit,
        ..Spec::default()
    }
}
fn one() -> Vec<Rel> {
    vec![rela(DATA_WORDS, R_AARCH64_RELATIVE, 0, 0)]
}
#[test]
fn parses_well_formed_tables() {
    let m = module(&Spec {
        symbols: vec![defined("f", 8), undefined("g")],
        relocations: vec![rela(DATA_WORDS, R_AARCH64_GLOB_DAT, 2, 4)],
        ..Spec::default()
    });
    let mod0 = m.mod0.unwrap();
    let d = &mod0.dynamic;
    assert_eq!(mod0.module_object, mod0.bss_start);
    assert_eq!(d.symbols.len(), 3);
    assert_eq!(d.symbols[2].name, "g");
    assert!(!d.symbols[2].is_definition());
    assert_eq!(d.lookup("f").unwrap().value, 8);
    assert!(
        d.lookup("g").is_none(),
        "undefined symbols never satisfy lookups"
    );
    let r = d.relocations[0];
    assert_eq!(
        (r.kind, r.symbol, r.addend),
        (R_AARCH64_GLOB_DAT, 2, Some(4))
    );
}
#[test]
fn symbol_count_falls_back_to_nso_dynsym_range_without_hash() {
    let m = module(&Spec {
        symbols: vec![defined("f", 8)],
        hash: false,
        ..Spec::default()
    });
    assert_eq!(m.mod0.unwrap().dynamic.symbols.len(), 2);
}
#[test]
fn elf_hash_matches_reference_values() {
    // Values from the System V ABI hash function definition.
    assert_eq!(elf_hash(b""), 0);
    assert_eq!(elf_hash(b"printf"), 0x077905a6);
    assert_eq!(elf_hash(b"exit"), 0x0006cf04);
}
#[test]
fn rejects_malformed_dynamic_tables() {
    let cases: Vec<(Spec, &str)> = vec![
        (
            with(vec![(DT_RELAENT, 16)], vec![DT_RELAENT], one()),
            "DT_RELAENT",
        ),
        (with(vec![], vec![DT_RELASZ], one()), "lacks its size tag"),
        (
            with(vec![(DT_RELA, 0xf000)], vec![DT_RELA], one()),
            "not mapped",
        ),
        (
            with(vec![(DT_RELASZ, 12)], vec![DT_RELASZ], one()),
            "not a multiple",
        ),
        (
            with(
                vec![],
                vec![],
                vec![rela(DATA_WORDS, R_AARCH64_GLOB_DAT, 9, 0)],
            ),
            "outside the",
        ),
        (
            with(vec![(DT_STRSZ, 0x10_0000)], vec![DT_STRSZ], vec![]),
            "DT_STRTAB is not mapped",
        ),
        (with(vec![(DT_STRSZ, 2)], vec![DT_STRSZ], vec![]), "string"),
        (
            with(vec![(DT_RELACOUNT, 2)], vec![0x6fff_fff9], one()),
            "exceeds",
        ),
        (with(vec![(DT_HASH, RO)], vec![DT_HASH], vec![]), "DT_HASH"),
        (
            with(
                vec![],
                vec![DT_PLTREL],
                vec![Rel {
                    table: Table::Plt,
                    offset: DATA_WORDS,
                    kind: R_AARCH64_JUMP_SLOT,
                    symbol: 1,
                    addend: 0,
                }],
            ),
            "DT_PLTREL",
        ),
        (with(vec![(DT_PLTREL, 3)], vec![], vec![]), "neither"),
        (
            with(vec![(DT_SYMENT, 16)], vec![DT_SYMENT], vec![]),
            "DT_SYMENT",
        ),
        (
            with(
                vec![(DT_INIT_ARRAY, DATA_WORDS), (DT_INIT_ARRAYSZ, 12)],
                vec![],
                vec![],
            ),
            "multiple of 8",
        ),
        (
            with(vec![(DT_NEEDED, 0x9999)], vec![], vec![]),
            "outside DT_STRTAB",
        ),
    ];
    for (spec, message) in cases {
        let e = parse_error(&spec);
        assert!(e.contains(message), "expected {message:?}, got {e:?}");
    }
}
