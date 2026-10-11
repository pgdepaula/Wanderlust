mod support;
use support::*;
use wl_common::GuestAddr;
use wl_loader::{LinkMode, Process};
use wl_nso::dynamic::*;

const BASE: u64 = 0x800_0000;
fn build(modules: Vec<(&str, Spec)>, base: u64, mode: LinkMode) -> Process {
    let inputs: Vec<_> = modules
        .into_iter()
        .map(|(n, s)| (n.to_string(), module(&s)))
        .collect();
    Process::build(&inputs, GuestAddr(base), mode).unwrap()
}
fn word(p: &Process, module: &str, offset: u64) -> u64 {
    let address = p.address(module, wl_common::ModuleOffset(offset)).unwrap();
    p.read_u64(address).unwrap()
}
fn kinds(p: &Process) -> Vec<&str> {
    p.link.diagnostics.iter().map(|d| d.kind).collect()
}

#[test]
fn rela_relative_is_base_plus_addend_regardless_of_stored_word() {
    for base in [BASE, 0x9000_0000] {
        let spec = Spec {
            data: vec![0xdead_beef, 7],
            relocations: vec![rela(DATA_WORDS, R_AARCH64_RELATIVE, 0, 0x40)],
            rela_count: 1,
            ..Spec::default()
        };
        let p = build(vec![("main", spec)], base, LinkMode::Host);
        assert_eq!(word(&p, "main", DATA_WORDS), base + 0x40);
        assert_eq!(
            word(&p, "main", DATA_WORDS + 8),
            7,
            "neighbouring word untouched"
        );
        assert_eq!(p.link.applied["R_AARCH64_RELATIVE"], 1);
        assert!(p.execution_blocker.is_none());
    }
}
#[test]
fn rel_relative_adds_base_to_implicit_addend() {
    let spec = Spec {
        data: vec![0x40],
        relocations: vec![rel(DATA_WORDS, R_AARCH64_RELATIVE, 0)],
        rel_count: 1,
        ..Spec::default()
    };
    let p = build(vec![("main", spec)], BASE, LinkMode::Host);
    assert_eq!(word(&p, "main", DATA_WORDS), BASE + 0x40);
}
#[test]
fn relative_outside_count_prefix_is_reported_not_applied() {
    let spec = Spec {
        data: vec![0x40],
        relocations: vec![rela(DATA_WORDS, R_AARCH64_RELATIVE, 0, 0x40)],
        rela_count: 0,
        ..Spec::default()
    };
    let p = build(vec![("main", spec)], BASE, LinkMode::Host);
    assert_eq!(word(&p, "main", DATA_WORDS), 0x40);
    assert_eq!(kinds(&p), ["relative-outside-count"]);
}
#[test]
fn non_relative_type_inside_count_prefix_is_ignored() {
    let spec = Spec {
        symbols: vec![defined("f", 0x8)],
        relocations: vec![rela(DATA_WORDS, R_AARCH64_ABS64, 1, 0)],
        rela_count: 1,
        ..Spec::default()
    };
    let p = build(vec![("main", spec)], BASE, LinkMode::Host);
    assert_eq!(word(&p, "main", DATA_WORDS), 0);
    assert_eq!(kinds(&p), ["non-relative-in-count"]);
}
#[test]
fn global_lookup_uses_loader_order_not_input_order() {
    let importer = Spec {
        symbols: vec![undefined("f")],
        relocations: vec![
            rela(DATA_WORDS, R_AARCH64_GLOB_DAT, 1, 0),
            rela(DATA_WORDS + 8, R_AARCH64_ABS64, 1, 8),
            Rel {
                table: Table::Plt,
                offset: DATA_WORDS + 16,
                kind: R_AARCH64_JUMP_SLOT,
                symbol: 1,
                addend: 0,
            },
        ],
        ..Spec::default()
    };
    let first = Spec {
        symbols: vec![defined("f", 0x10)],
        ..Spec::default()
    };
    let second = Spec {
        symbols: vec![defined("f", 0x20)],
        ..Spec::default()
    };
    let p = build(
        vec![("sdk", second), ("main", importer), ("subsdk0", first)],
        BASE,
        LinkMode::Host,
    );
    let names: Vec<_> = p.modules.iter().map(|m| m.name.as_str()).collect();
    assert_eq!(names, ["main", "subsdk0", "sdk"]);
    let f = p.modules[1].base.0 + 0x10;
    assert_eq!(word(&p, "main", DATA_WORDS), f);
    assert_eq!(word(&p, "main", DATA_WORDS + 8), f + 8);
    assert_eq!(word(&p, "main", DATA_WORDS + 16), f);
    assert!(
        p.code_pointers
            .iter()
            .any(|c| c.address.0 == f && c.source == "relocation")
    );
}
#[test]
fn rel_jump_slot_ignores_lazy_stub_word() {
    let importer = Spec {
        symbols: vec![undefined("f")],
        data: vec![0x80],
        plt_rela: false,
        relocations: vec![Rel {
            table: Table::Plt,
            offset: DATA_WORDS,
            kind: R_AARCH64_JUMP_SLOT,
            symbol: 1,
            addend: 0,
        }],
        ..Spec::default()
    };
    let p = build(
        vec![
            ("main", importer),
            (
                "sdk",
                Spec {
                    symbols: vec![defined("f", 0x8)],
                    ..Spec::default()
                },
            ),
        ],
        BASE,
        LinkMode::Host,
    );
    assert_eq!(word(&p, "main", DATA_WORDS), p.modules[1].base.0 + 0x8);
}
#[test]
fn hidden_reference_binds_within_its_module() {
    let mut hidden = defined("f", 0x30);
    hidden.visibility = 2;
    let spec = Spec {
        symbols: vec![hidden],
        relocations: vec![rela(DATA_WORDS, R_AARCH64_GLOB_DAT, 1, 0)],
        ..Spec::default()
    };
    let earlier = Spec {
        symbols: vec![defined("f", 0x10)],
        ..Spec::default()
    };
    let p = build(vec![("main", earlier), ("sdk", spec)], BASE, LinkMode::Host);
    assert_eq!(word(&p, "sdk", DATA_WORDS), p.modules[1].base.0 + 0x30);
}
#[test]
fn abs32_writes_four_bytes_and_checks_range() {
    let spec = |base_symbol| Spec {
        symbols: vec![defined("f", base_symbol)],
        data: vec![0xffff_ffff_0000_0000],
        relocations: vec![rela(DATA_WORDS, R_AARCH64_ABS32, 1, 4)],
        ..Spec::default()
    };
    let p = build(vec![("main", spec(0x8))], BASE, LinkMode::Host);
    assert_eq!(
        word(&p, "main", DATA_WORDS),
        0xffff_ffff_0000_0000 | (BASE + 0xc)
    );
    let p = build(vec![("main", spec(0x8))], 0x1_0000_0000, LinkMode::Host);
    assert_eq!(kinds(&p), ["relocation-overflow"]);
    assert!(p.execution_blocker.is_some());
}
#[test]
fn unresolved_strong_symbols_block_execution_and_weak_ones_warn() {
    let spec = |bind| Spec {
        symbols: vec![Sym {
            bind,
            ..undefined("missing")
        }],
        data: vec![0x55],
        relocations: vec![rela(DATA_WORDS, R_AARCH64_GLOB_DAT, 1, 0)],
        ..Spec::default()
    };
    let p = build(vec![("main", spec(1))], BASE, LinkMode::Host);
    assert_eq!(p.link.unresolved_symbols, 1);
    assert_eq!(
        word(&p, "main", DATA_WORDS),
        0x55,
        "never substituted with zero"
    );
    assert_eq!(p.link.diagnostics[0].severity, "error");
    assert!(p.execution_blocker.is_some());
    let p = build(vec![("main", spec(2))], BASE, LinkMode::Host);
    assert_eq!(p.link.diagnostics[0].severity, "warning");
    assert!(p.execution_blocker.is_none());
}
#[test]
fn unsupported_and_misplaced_relocations_are_errors() {
    for (relocation, rela_count, kind) in [
        (
            rela(DATA_WORDS, R_AARCH64_TLS_TPREL64, 0, 0),
            0,
            "unsupported-relocation",
        ),
        (
            rela(RO + 0x10, R_AARCH64_RELATIVE, 0, 0),
            1,
            "relocation-target-not-writable",
        ),
        (
            rela(0x10_0000, R_AARCH64_RELATIVE, 0, 0),
            1,
            "relocation-target-unmapped",
        ),
    ] {
        let spec = Spec {
            relocations: vec![relocation],
            rela_count,
            ..Spec::default()
        };
        let p = build(vec![("main", spec)], BASE, LinkMode::Host);
        assert_eq!(kinds(&p), [kind]);
        assert!(p.execution_blocker.is_some());
    }
}
#[test]
fn layout_follows_loader_order_and_page_rounding() {
    let p = build(
        vec![
            ("sdk", Spec::default()),
            (
                "main",
                Spec {
                    bss: 0x1001,
                    ..Spec::default()
                },
            ),
        ],
        BASE,
        LinkMode::Host,
    );
    assert_eq!(p.entry, GuestAddr(BASE));
    assert_eq!(p.modules[0].name, "main");
    // main: data at 0x2000, 0x200 dynamic + 64 data bytes + 0x1001 BSS -> 0x2000 + 0x1241 -> 0x4000.
    assert_eq!(p.modules[0].size, 0x4000);
    assert_eq!(p.modules[1].base, GuestAddr(BASE + 0x4000));
    assert_eq!(p.size, 0x4000 + p.modules[1].size);
    let perms: Vec<_> = p.modules[0]
        .segments
        .iter()
        .map(|s| s.permissions.as_str())
        .collect();
    assert_eq!(perms, ["r-x", "r--", "rw-"]);
    let data = p.mappings.iter().find(|m| m.base.0 == BASE + DATA).unwrap();
    assert_eq!(
        data.bytes.len(),
        0x2000,
        "data and BSS share one page-rounded RW mapping"
    );
    assert!(data.bytes[0x240..].iter().all(|b| *b == 0));
}
#[test]
fn rejects_invalid_process_inputs() {
    let m = module(&Spec::default());
    for (inputs, base, message) in [
        (
            vec![("game".to_string(), m.clone())],
            BASE,
            "not an ExeFS module name",
        ),
        (
            vec![("main".into(), m.clone()), ("main".into(), m.clone())],
            BASE,
            "duplicate",
        ),
        (vec![("main".into(), m.clone())], BASE + 4, "page aligned"),
        (vec![], BASE, "at least one module"),
    ] {
        let e = Process::build(&inputs, GuestAddr(base), LinkMode::Host).unwrap_err();
        assert!(e.contains(message), "{e}");
    }
}
#[test]
fn rtld_requires_guest_linking_and_guest_mode_leaves_image_unrelocated() {
    let spec = || Spec {
        data: vec![0x8],
        relocations: vec![rela(DATA_WORDS, R_AARCH64_RELATIVE, 0, 0x8)],
        rela_count: 1,
        ..Spec::default()
    };
    let host = build(
        vec![("rtld", spec()), ("main", spec())],
        BASE,
        LinkMode::Host,
    );
    assert!(host.execution_blocker.as_ref().unwrap().contains("rtld"));
    let guest = build(
        vec![("rtld", spec()), ("main", spec())],
        BASE,
        LinkMode::Guest,
    );
    assert!(guest.execution_blocker.is_none());
    assert!(guest.link.applied.is_empty());
    assert_eq!(word(&guest, "main", DATA_WORDS), 0x8);
    // Analysis roots still reflect the relocated value.
    let main = guest.modules[1].base.0;
    assert!(
        guest
            .code_pointers
            .iter()
            .any(|c| c.address.0 == main + 0x8)
    );
}
#[test]
fn initializers_run_in_reverse_load_order() {
    let spec = || Spec {
        extra_dynamic: vec![(DT_INIT, 0x8)],
        ..Spec::default()
    };
    let p = build(
        vec![("main", spec()), ("sdk", spec()), ("rtld", spec())],
        BASE,
        LinkMode::Guest,
    );
    let order: Vec<_> = p
        .link
        .init_order
        .iter()
        .map(|i| i.module.as_str())
        .collect();
    assert_eq!(order, ["sdk", "main", "rtld"]);
    assert_eq!(p.link.init_order[0].address.0, p.modules[2].base.0 + 0x8);
}
#[test]
fn relr_adds_the_base_to_address_and_bitmap_entries() {
    // RELR table (inside the data words): address entry DATA_WORDS, then bitmap 0b1011
    // selecting DATA_WORDS + 8 and DATA_WORDS + 24 (bit 0 marks a bitmap).
    let relr = DATA_WORDS + 32;
    let spec = Spec {
        data: vec![0x100, 0x200, 7, 0x300, DATA_WORDS, 0b1011],
        extra_dynamic: vec![(36, relr), (35, 16), (37, 8)],
        ..Spec::default()
    };
    let p = build(vec![("main", spec.clone())], BASE, LinkMode::Host);
    let words: Vec<u64> = (0..6)
        .map(|i| word(&p, "main", DATA_WORDS + 8 * i))
        .collect();
    assert_eq!(
        words,
        [
            BASE + 0x100,
            BASE + 0x200,
            7,
            BASE + 0x300,
            DATA_WORDS,
            0b1011
        ]
    );
    assert_eq!(p.link.applied["DT_RELR (relative)"], 3);
    assert!(p.execution_blocker.is_none(), "{:?}", kinds(&p));
    // Guest linking leaves the words for the guest's own crt0 to relocate.
    let g = build(vec![("main", spec)], BASE, LinkMode::Guest);
    assert_eq!(word(&g, "main", DATA_WORDS), 0x100);
}
#[test]
fn relr_bitmap_before_an_address_is_malformed() {
    let spec = Spec {
        data: vec![0, 0b11],
        extra_dynamic: vec![(36, DATA_WORDS + 8), (35, 8), (37, 8)],
        ..Spec::default()
    };
    let p = build(vec![("main", spec)], BASE, LinkMode::Host);
    assert!(kinds(&p).contains(&"relr-malformed"), "{:?}", kinds(&p));
    assert!(p.execution_blocker.is_some());
    assert_eq!(word(&p, "main", DATA_WORDS), 0);
}
