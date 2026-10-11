//! Builder for synthetic MOD0 NSO modules with explicit dynamic tables.
//! Layout (module offsets): text 0x0, rodata 0x1000 (MOD0 at 0x1000, dynsym at 0x1020),
//! data 0x2000 (dynamic table at 0x2000, data words from DATA_WORDS).
#![allow(dead_code)]
use wl_nso::Module;

pub const RO: u64 = 0x1000;
pub const DATA: u64 = 0x2000;
pub const DATA_WORDS: u64 = 0x2200;
pub const RET: u32 = 0xd65f03c0;

#[derive(Clone)]
pub struct Sym {
    pub name: &'static str,
    pub value: u64,
    pub bind: u8,
    pub visibility: u8,
    pub section: u16,
    pub kind: u8,
}
pub fn defined(name: &'static str, value: u64) -> Sym {
    Sym {
        name,
        value,
        bind: 1,
        visibility: 0,
        section: 1,
        kind: 2,
    }
}
pub fn undefined(name: &'static str) -> Sym {
    Sym {
        name,
        value: 0,
        bind: 1,
        visibility: 0,
        section: 0,
        kind: 0,
    }
}
#[derive(Clone, Copy)]
pub enum Table {
    Rela,
    Rel,
    Plt,
}
#[derive(Clone, Copy)]
pub struct Rel {
    pub table: Table,
    pub offset: u64,
    pub kind: u32,
    pub symbol: u32,
    pub addend: i64,
}
pub fn rela(offset: u64, kind: u32, symbol: u32, addend: i64) -> Rel {
    Rel {
        table: Table::Rela,
        offset,
        kind,
        symbol,
        addend,
    }
}
pub fn rel(offset: u64, kind: u32, symbol: u32) -> Rel {
    Rel {
        table: Table::Rel,
        offset,
        kind,
        symbol,
        addend: 0,
    }
}
#[derive(Clone)]
pub struct Spec {
    pub text: Vec<u32>,
    /// Symbol 0 (the null symbol) is added automatically.
    pub symbols: Vec<Sym>,
    pub relocations: Vec<Rel>,
    pub rela_count: u64,
    pub rel_count: u64,
    pub plt_rela: bool,
    pub data: Vec<u64>,
    pub bss: u32,
    pub hash: bool,
    /// Appended after the generated dynamic entries (before DT_NULL).
    pub extra_dynamic: Vec<(i64, u64)>,
    /// Generated tags to omit.
    pub omit: Vec<i64>,
}
impl Default for Spec {
    fn default() -> Self {
        Self {
            text: vec![RET],
            symbols: Vec::new(),
            relocations: Vec::new(),
            rela_count: 0,
            rel_count: 0,
            plt_rela: true,
            data: vec![0; 8],
            bss: 0x40,
            hash: true,
            extra_dynamic: Vec::new(),
            omit: Vec::new(),
        }
    }
}
fn align(v: &mut Vec<u8>, a: usize) {
    while v.len() % a != 0 {
        v.push(0);
    }
}
pub fn bytes(spec: &Spec) -> Vec<u8> {
    let mut text: Vec<u8> = Vec::new();
    text.extend(RET.to_le_bytes());
    text.extend((RO as u32).to_le_bytes()); // MOD0 offset (module-relative)
    for w in &spec.text {
        text.extend(w.to_le_bytes());
    }
    let mut ro = vec![0u8; 0x20];
    let mut strtab = vec![0u8];
    let mut symtab = vec![0u8; 24];
    for s in &spec.symbols {
        let name = strtab.len() as u32;
        strtab.extend(s.name.as_bytes());
        strtab.push(0);
        symtab.extend(name.to_le_bytes());
        symtab.push(s.bind << 4 | s.kind);
        symtab.push(s.visibility);
        symtab.extend(s.section.to_le_bytes());
        symtab.extend(s.value.to_le_bytes());
        symtab.extend(8u64.to_le_bytes());
    }
    let nsyms = spec.symbols.len() as u32 + 1;
    let symtab_at = RO + ro.len() as u64;
    ro.extend(&symtab);
    let strtab_at = RO + ro.len() as u64;
    ro.extend(&strtab);
    align(&mut ro, 8);
    let hash_at = RO + ro.len() as u64;
    // One bucket chaining every symbol from the last to the first.
    ro.extend(1u32.to_le_bytes());
    ro.extend(nsyms.to_le_bytes());
    ro.extend((nsyms - 1).to_le_bytes());
    for i in 0..nsyms {
        ro.extend(i.saturating_sub(1).to_le_bytes());
    }
    align(&mut ro, 8);
    let mut tables = Vec::new();
    for (kind, explicit) in [(0, true), (1, false), (2, spec.plt_rela)] {
        let at = RO + ro.len() as u64;
        let mut n = 0;
        for r in &spec.relocations {
            let matches = matches!(
                (kind, r.table),
                (0, Table::Rela) | (1, Table::Rel) | (2, Table::Plt)
            );
            if !matches {
                continue;
            }
            ro.extend(r.offset.to_le_bytes());
            ro.extend((u64::from(r.symbol) << 32 | u64::from(r.kind)).to_le_bytes());
            if explicit {
                ro.extend(r.addend.to_le_bytes());
            }
            n += 1;
        }
        tables.push((at, n * if explicit { 24 } else { 16 }));
    }
    ro[..4].copy_from_slice(b"MOD0");
    let data_len = (DATA_WORDS - DATA) + 8 * spec.data.len() as u64;
    let bss_start = DATA + data_len;
    for (i, v) in [
        DATA as i64 - RO as i64,
        bss_start as i64 - RO as i64,
        (bss_start + u64::from(spec.bss)) as i64 - RO as i64,
        0,
        0,
        bss_start as i64 - RO as i64,
    ]
    .iter()
    .enumerate()
    {
        ro[4 + 4 * i..8 + 4 * i].copy_from_slice(&(*v as i32).to_le_bytes());
    }
    let mut dynamic: Vec<(i64, u64)> = vec![
        (5, strtab_at),
        (10, strtab.len() as u64),
        (6, symtab_at),
        (11, 24),
    ];
    if spec.hash {
        dynamic.push((4, hash_at));
    }
    if tables[0].1 > 0 {
        dynamic.extend([
            (7, tables[0].0),
            (8, tables[0].1),
            (9, 24),
            (0x6fff_fff9, spec.rela_count),
        ]);
    }
    if tables[1].1 > 0 {
        dynamic.extend([
            (17, tables[1].0),
            (18, tables[1].1),
            (19, 16),
            (0x6fff_fffa, spec.rel_count),
        ]);
    }
    if tables[2].1 > 0 {
        dynamic.extend([
            (23, tables[2].0),
            (2, tables[2].1),
            (20, if spec.plt_rela { 7 } else { 17 }),
        ]);
    }
    dynamic.retain(|(t, _)| !spec.omit.contains(t));
    dynamic.extend(spec.extra_dynamic.iter().copied());
    dynamic.push((0, 0));
    let mut data = Vec::new();
    for (t, v) in dynamic {
        data.extend(t.to_le_bytes());
        data.extend(v.to_le_bytes());
    }
    assert!(
        data.len() as u64 <= DATA_WORDS - DATA,
        "dynamic table too large"
    );
    data.resize((DATA_WORDS - DATA) as usize, 0);
    for w in &spec.data {
        data.extend(w.to_le_bytes());
    }
    let mut out = vec![0u8; 0x100];
    out[..4].copy_from_slice(b"NSO0");
    out[0x3c..0x40].copy_from_slice(&spec.bss.to_le_bytes());
    out[0x40..0x48].copy_from_slice(b"synthbid");
    out[0x98..0x9c].copy_from_slice(&((symtab_at - RO) as u32).to_le_bytes());
    out[0x9c..0xa0].copy_from_slice(&(nsyms * 24).to_le_bytes());
    for (i, (segment, offset)) in [(&text, 0u64), (&ro, RO), (&data, DATA)]
        .into_iter()
        .enumerate()
    {
        let file = out.len() as u32;
        out[0x10 + 16 * i..0x14 + 16 * i].copy_from_slice(&file.to_le_bytes());
        out[0x14 + 16 * i..0x18 + 16 * i].copy_from_slice(&(offset as u32).to_le_bytes());
        out[0x18 + 16 * i..0x1c + 16 * i].copy_from_slice(&(segment.len() as u32).to_le_bytes());
        out.extend(segment.iter());
    }
    out
}
pub fn module(spec: &Spec) -> Module {
    Module::parse(&bytes(spec)).unwrap()
}
pub fn parse_error(spec: &Spec) -> String {
    Module::parse(&bytes(spec)).unwrap_err()
}
