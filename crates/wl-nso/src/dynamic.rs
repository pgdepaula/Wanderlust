//! ELF dynamic metadata embedded in a MOD0 module.
//!
//! NSO modules are linked at address zero, so every `d_ptr` value, symbol value and
//! relocation offset in this module is a [`ModuleOffset`]. Nothing here applies a
//! relocation; `wl-loader` owns placement and linking.
use crate::{Module, u32_at};
use serde::Serialize;
use wl_common::{ModuleOffset, Result};

pub const DT_NULL: i64 = 0;
pub const DT_NEEDED: i64 = 1;
pub const DT_PLTRELSZ: i64 = 2;
pub const DT_PLTGOT: i64 = 3;
pub const DT_HASH: i64 = 4;
pub const DT_STRTAB: i64 = 5;
pub const DT_SYMTAB: i64 = 6;
pub const DT_RELA: i64 = 7;
pub const DT_RELASZ: i64 = 8;
pub const DT_RELAENT: i64 = 9;
pub const DT_STRSZ: i64 = 10;
pub const DT_SYMENT: i64 = 11;
pub const DT_INIT: i64 = 12;
pub const DT_FINI: i64 = 13;
pub const DT_SONAME: i64 = 14;
pub const DT_SYMBOLIC: i64 = 16;
pub const DT_REL: i64 = 17;
pub const DT_RELSZ: i64 = 18;
pub const DT_RELENT: i64 = 19;
pub const DT_PLTREL: i64 = 20;
pub const DT_DEBUG: i64 = 21;
pub const DT_TEXTREL: i64 = 22;
pub const DT_JMPREL: i64 = 23;
pub const DT_BIND_NOW: i64 = 24;
pub const DT_INIT_ARRAY: i64 = 25;
pub const DT_FINI_ARRAY: i64 = 26;
pub const DT_INIT_ARRAYSZ: i64 = 27;
pub const DT_FINI_ARRAYSZ: i64 = 28;
pub const DT_FLAGS: i64 = 30;
pub const DT_RELRSZ: i64 = 35;
pub const DT_RELR: i64 = 36;
pub const DT_RELRENT: i64 = 37;
pub const DT_GNU_HASH: i64 = 0x6fff_fef5;
pub const DT_RELACOUNT: i64 = 0x6fff_fff9;
pub const DT_RELCOUNT: i64 = 0x6fff_fffa;
pub const DT_FLAGS_1: i64 = 0x6fff_fffb;

pub const R_AARCH64_NONE: u32 = 0;
pub const R_AARCH64_ABS64: u32 = 257;
pub const R_AARCH64_ABS32: u32 = 258;
pub const R_AARCH64_GLOB_DAT: u32 = 1025;
pub const R_AARCH64_JUMP_SLOT: u32 = 1026;
pub const R_AARCH64_RELATIVE: u32 = 1027;
pub const R_AARCH64_TLS_DTPMOD64: u32 = 1028;
pub const R_AARCH64_TLS_DTPREL64: u32 = 1029;
pub const R_AARCH64_TLS_TPREL64: u32 = 1030;
pub const R_AARCH64_TLSDESC: u32 = 1031;
pub const R_AARCH64_IRELATIVE: u32 = 1032;

pub const SHN_UNDEF: u16 = 0;
pub const SHN_ABS: u16 = 0xfff1;
pub const SHN_COMMON: u16 = 0xfff2;
const SYM_SIZE: u64 = 24;
const RELA_SIZE: u64 = 24;
const REL_SIZE: u64 = 16;
/// Defensive bound on symbols/relocations per table; larger tables are rejected, not truncated.
pub const MAX_TABLE_ENTRIES: u64 = 16 * 1024 * 1024;

pub fn relocation_name(kind: u32) -> &'static str {
    match kind {
        R_AARCH64_NONE => "R_AARCH64_NONE",
        R_AARCH64_ABS64 => "R_AARCH64_ABS64",
        R_AARCH64_ABS32 => "R_AARCH64_ABS32",
        R_AARCH64_GLOB_DAT => "R_AARCH64_GLOB_DAT",
        R_AARCH64_JUMP_SLOT => "R_AARCH64_JUMP_SLOT",
        R_AARCH64_RELATIVE => "R_AARCH64_RELATIVE",
        R_AARCH64_TLS_DTPMOD64 => "R_AARCH64_TLS_DTPMOD64",
        R_AARCH64_TLS_DTPREL64 => "R_AARCH64_TLS_DTPREL64",
        R_AARCH64_TLS_TPREL64 => "R_AARCH64_TLS_TPREL64",
        R_AARCH64_TLSDESC => "R_AARCH64_TLSDESC",
        R_AARCH64_IRELATIVE => "R_AARCH64_IRELATIVE",
        _ => "unknown",
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
pub enum TableKind {
    /// `DT_RELA`: explicit addend.
    Rela,
    /// `DT_REL`: implicit addend stored at the relocation target.
    Rel,
    /// `DT_JMPREL`: PLT relocations; REL or RELA according to `DT_PLTREL`.
    PltRela,
    PltRel,
}
impl TableKind {
    pub fn explicit_addend(self) -> bool {
        matches!(self, Self::Rela | Self::PltRela)
    }
}
#[derive(Clone, Copy, Debug, Serialize)]
pub struct Relocation {
    pub table: TableKind,
    pub index: u32,
    pub offset: ModuleOffset,
    pub kind: u32,
    pub symbol: u32,
    /// `None` for REL entries, whose addend is the existing target contents.
    pub addend: Option<i64>,
}
#[derive(Clone, Debug, Serialize)]
pub struct Symbol {
    pub index: u32,
    pub name: String,
    /// Module-relative for ordinary defined symbols; absolute for `SHN_ABS`.
    pub value: u64,
    pub size: u64,
    pub kind: u8,
    pub bind: u8,
    pub visibility: u8,
    pub section: u16,
}
impl Symbol {
    /// The definition test used by Horizon's rtld: undefined and common symbols never satisfy a lookup.
    pub fn is_definition(&self) -> bool {
        self.section != SHN_UNDEF && self.section != SHN_COMMON
    }
    pub fn is_function(&self) -> bool {
        self.kind == 2
    }
}
#[derive(Clone, Debug, Serialize)]
pub struct HashTable {
    pub offset: ModuleOffset,
    pub buckets: Vec<u32>,
    pub chains: Vec<u32>,
}
#[derive(Clone, Copy, Debug, Serialize)]
pub struct Range {
    pub offset: ModuleOffset,
    pub size: u64,
}
#[derive(Clone, Debug, Default, Serialize)]
pub struct Dynamic {
    pub needed: Vec<String>,
    pub soname: Option<String>,
    pub strtab: Option<Range>,
    pub symtab: Option<ModuleOffset>,
    pub hash: Option<HashTable>,
    pub gnu_hash: Option<ModuleOffset>,
    pub pltgot: Option<ModuleOffset>,
    pub rela: Option<Range>,
    pub rel: Option<Range>,
    pub jmprel: Option<Range>,
    pub pltrel: Option<TableKind>,
    pub relr: Option<Range>,
    pub rela_count: u64,
    pub rel_count: u64,
    pub init: Option<ModuleOffset>,
    pub fini: Option<ModuleOffset>,
    pub init_array: Option<Range>,
    pub fini_array: Option<Range>,
    pub flags: u64,
    pub flags_1: u64,
    pub symbolic: bool,
    pub bind_now: bool,
    pub textrel: bool,
    /// Tags not interpreted by this parser, retained so callers can report them.
    pub uninterpreted_tags: Vec<i64>,
    pub symbols: Vec<Symbol>,
    pub relocations: Vec<Relocation>,
}

impl Module {
    /// Reads `size` bytes at a module-relative offset that must lie inside one stored segment.
    pub fn read(&self, offset: ModuleOffset, size: u64) -> Result<&[u8]> {
        let size = usize::try_from(size).map_err(|_| "read size exceeds host range")?;
        self.bytes_at(offset.0, size)
    }
}

fn table_count(range: Range, entry: u64, what: &str) -> Result<u64> {
    if range.size % entry != 0 {
        return Err(format!(
            "{what}: size {:#x} is not a multiple of entry size {entry:#x}",
            range.size
        ));
    }
    let count = range.size / entry;
    if count > MAX_TABLE_ENTRIES {
        return Err(format!("{what}: {count} entries exceed the table limit"));
    }
    Ok(count)
}
fn string(table: &[u8], offset: u32, what: &str) -> Result<String> {
    let start = offset as usize;
    let tail = table
        .get(start..)
        .ok_or_else(|| format!("{what}: string offset {offset:#x} is outside DT_STRTAB"))?;
    let end = tail
        .iter()
        .position(|b| *b == 0)
        .ok_or_else(|| format!("{what}: string at {offset:#x} is not NUL-terminated"))?;
    Ok(String::from_utf8_lossy(&tail[..end]).into_owned())
}
/// SysV ELF hash, as used for `DT_HASH` bucket selection.
pub fn elf_hash(name: &[u8]) -> u32 {
    let mut h: u32 = 0;
    for &c in name {
        h = (h << 4).wrapping_add(u32::from(c));
        let g = h & 0xf000_0000;
        if g != 0 {
            h ^= g >> 24;
        }
        h &= !g;
    }
    h
}

impl Dynamic {
    pub fn parse(module: &Module, entries: &[crate::DynamicEntry]) -> Result<Self> {
        let mut d = Self::default();
        let mut strsz = None;
        let mut sizes = [None::<u64>; 6]; // relasz, relsz, pltrelsz, relrsz, init_arraysz, fini_arraysz
        let mut starts = [None::<u64>; 6]; // rela, rel, jmprel, relr, init_array, fini_array
        let mut strtab = None;
        let mut hash = None;
        let mut needed = Vec::new();
        let mut soname = None;
        for e in entries {
            let v = e.value;
            match e.tag {
                DT_NEEDED => needed.push(v),
                DT_SONAME => soname = Some(v),
                DT_STRTAB => strtab = Some(v),
                DT_STRSZ => strsz = Some(v),
                DT_SYMTAB => d.symtab = Some(ModuleOffset(v)),
                DT_SYMENT if v != SYM_SIZE => {
                    return Err(format!("DT_SYMENT {v} is not the ELF64 symbol size 24"));
                }
                DT_RELAENT if v != RELA_SIZE => {
                    return Err(format!("DT_RELAENT {v} is not the ELF64 RELA size 24"));
                }
                DT_RELENT if v != REL_SIZE => {
                    return Err(format!("DT_RELENT {v} is not the ELF64 REL size 16"));
                }
                DT_RELRENT if v != 8 => {
                    return Err(format!("DT_RELRENT {v} is not 8"));
                }
                DT_SYMENT | DT_RELAENT | DT_RELENT | DT_RELRENT => {}
                DT_HASH => hash = Some(v),
                DT_GNU_HASH => d.gnu_hash = Some(ModuleOffset(v)),
                DT_PLTGOT => d.pltgot = Some(ModuleOffset(v)),
                DT_RELA => starts[0] = Some(v),
                DT_RELASZ => sizes[0] = Some(v),
                DT_REL => starts[1] = Some(v),
                DT_RELSZ => sizes[1] = Some(v),
                DT_JMPREL => starts[2] = Some(v),
                DT_PLTRELSZ => sizes[2] = Some(v),
                DT_RELR => starts[3] = Some(v),
                DT_RELRSZ => sizes[3] = Some(v),
                DT_INIT_ARRAY => starts[4] = Some(v),
                DT_INIT_ARRAYSZ => sizes[4] = Some(v),
                DT_FINI_ARRAY => starts[5] = Some(v),
                DT_FINI_ARRAYSZ => sizes[5] = Some(v),
                DT_PLTREL => {
                    d.pltrel = Some(match v as i64 {
                        DT_RELA => TableKind::PltRela,
                        DT_REL => TableKind::PltRel,
                        _ => return Err(format!("DT_PLTREL {v} is neither DT_REL nor DT_RELA")),
                    })
                }
                DT_RELACOUNT => d.rela_count = v,
                DT_RELCOUNT => d.rel_count = v,
                DT_INIT => d.init = Some(ModuleOffset(v)),
                DT_FINI => d.fini = Some(ModuleOffset(v)),
                DT_FLAGS => d.flags = v,
                DT_FLAGS_1 => d.flags_1 = v,
                DT_SYMBOLIC => d.symbolic = true,
                DT_BIND_NOW => d.bind_now = true,
                DT_TEXTREL => d.textrel = true,
                DT_DEBUG => {}
                tag => d.uninterpreted_tags.push(tag),
            }
        }
        let range = |i: usize, name: &str| -> Result<Option<Range>> {
            match (starts[i], sizes[i]) {
                (None, None) => Ok(None),
                (Some(offset), Some(size)) => {
                    module.read(ModuleOffset(offset), size).map_err(|e| {
                        format!("{name} range {offset:#x}+{size:#x} is not mapped: {e}")
                    })?;
                    Ok(Some(Range {
                        offset: ModuleOffset(offset),
                        size,
                    }))
                }
                (Some(_), None) => Err(format!("{name} lacks its size tag")),
                (None, Some(0)) => Ok(None),
                (None, Some(_)) => Err(format!("{name} size is present without its address tag")),
            }
        };
        d.rela = range(0, "DT_RELA")?;
        d.rel = range(1, "DT_REL")?;
        d.jmprel = range(2, "DT_JMPREL")?;
        d.relr = range(3, "DT_RELR")?;
        d.init_array = range(4, "DT_INIT_ARRAY")?;
        d.fini_array = range(5, "DT_FINI_ARRAY")?;
        if d.jmprel.is_some() && d.pltrel.is_none() {
            return Err("DT_JMPREL is present without DT_PLTREL".into());
        }
        for (name, array) in [
            ("DT_INIT_ARRAY", d.init_array),
            ("DT_FINI_ARRAY", d.fini_array),
        ] {
            if array.is_some_and(|a| a.size % 8 != 0) {
                return Err(format!("{name} size is not a multiple of 8"));
            }
        }
        let strings: &[u8] = match (strtab, strsz) {
            (Some(offset), Some(size)) => {
                let s = module
                    .read(ModuleOffset(offset), size)
                    .map_err(|e| format!("DT_STRTAB is not mapped: {e}"))?;
                d.strtab = Some(Range {
                    offset: ModuleOffset(offset),
                    size,
                });
                s
            }
            (None, None) => &[],
            _ => return Err("DT_STRTAB and DT_STRSZ must appear together".into()),
        };
        for n in needed {
            let n = u32::try_from(n).map_err(|_| "DT_NEEDED offset exceeds 32 bits")?;
            d.needed.push(string(strings, n, "DT_NEEDED")?);
        }
        if let Some(n) = soname {
            let n = u32::try_from(n).map_err(|_| "DT_SONAME offset exceeds 32 bits")?;
            d.soname = Some(string(strings, n, "DT_SONAME")?);
        }
        if let Some(offset) = hash {
            let header = module
                .read(ModuleOffset(offset), 8)
                .map_err(|e| format!("DT_HASH header is not mapped: {e}"))?;
            let nbucket = u64::from(u32_at(header, 0)?);
            let nchain = u64::from(u32_at(header, 4)?);
            if nbucket == 0 || nbucket > MAX_TABLE_ENTRIES || nchain > MAX_TABLE_ENTRIES {
                return Err(format!(
                    "DT_HASH has invalid bucket/chain counts {nbucket}/{nchain}"
                ));
            }
            let words = module
                .read(ModuleOffset(offset), 8 + 4 * (nbucket + nchain))
                .map_err(|e| format!("DT_HASH table is not mapped: {e}"))?;
            let word = |i: u64| u32_at(words, 8 + 4 * i as usize);
            let buckets = (0..nbucket).map(word).collect::<Result<Vec<_>>>()?;
            let chains = (nbucket..nbucket + nchain)
                .map(word)
                .collect::<Result<Vec<_>>>()?;
            if buckets
                .iter()
                .chain(&chains)
                .any(|&i| u64::from(i) >= nchain)
            {
                return Err("DT_HASH references a symbol index outside nchain".into());
            }
            d.hash = Some(HashTable {
                offset: ModuleOffset(offset),
                buckets,
                chains,
            });
        }
        // rtld sizes the symbol table by DT_HASH nchain. Without DT_HASH, the NSO header's
        // dynsym range is the only bounded source; a symbol table with neither is rejected.
        let symbol_count = match (&d.hash, d.symtab) {
            (_, None) => 0,
            (Some(h), Some(_)) => h.chains.len() as u64,
            (None, Some(offset)) => {
                let ro = &module.ro_metadata[2];
                let rodata = module.segments[1].guest_offset.0;
                if ro.size == 0 || rodata + u64::from(ro.offset) != offset.0 {
                    return Err("DT_SYMTAB has no DT_HASH and no matching NSO dynsym range".into());
                }
                table_count(
                    Range {
                        offset,
                        size: u64::from(ro.size),
                    },
                    SYM_SIZE,
                    "dynsym",
                )?
            }
        };
        if let Some(offset) = d.symtab {
            let table = module
                .read(offset, symbol_count * SYM_SIZE)
                .map_err(|e| format!("DT_SYMTAB is not mapped for {symbol_count} symbols: {e}"))?;
            for (index, raw) in table.chunks_exact(SYM_SIZE as usize).enumerate() {
                let name = u32_at(raw, 0)?;
                let info = raw[4];
                d.symbols.push(Symbol {
                    index: index as u32,
                    name: if name == 0 {
                        String::new()
                    } else {
                        string(strings, name, "dynsym name")?
                    },
                    value: u64::from_le_bytes(raw[8..16].try_into().map_err(|_| "sym value")?),
                    size: u64::from_le_bytes(raw[16..24].try_into().map_err(|_| "sym size")?),
                    kind: info & 15,
                    bind: info >> 4,
                    visibility: raw[5] & 3,
                    section: u16::from_le_bytes([raw[6], raw[7]]),
                });
            }
        }
        let mut tables = Vec::new();
        if let Some(r) = d.rela {
            tables.push((TableKind::Rela, r));
        }
        if let Some(r) = d.rel {
            tables.push((TableKind::Rel, r));
        }
        if let (Some(r), Some(kind)) = (d.jmprel, d.pltrel) {
            tables.push((kind, r));
        }
        for (kind, range) in tables {
            let entry = if kind.explicit_addend() {
                RELA_SIZE
            } else {
                REL_SIZE
            };
            let count = table_count(range, entry, "relocation table")?;
            let bytes = module.read(range.offset, range.size)?;
            for (index, raw) in bytes.chunks_exact(entry as usize).enumerate() {
                let offset = u64::from_le_bytes(raw[..8].try_into().map_err(|_| "r_offset")?);
                let info = u64::from_le_bytes(raw[8..16].try_into().map_err(|_| "r_info")?);
                let symbol = (info >> 32) as u32;
                if u64::from(symbol) >= d.symbols.len() as u64 && symbol != 0 {
                    return Err(format!(
                        "{kind:?}[{index}] references symbol {symbol}, outside the {}-entry DT_SYMTAB",
                        d.symbols.len()
                    ));
                }
                d.relocations.push(Relocation {
                    table: kind,
                    index: index as u32,
                    offset: ModuleOffset(offset),
                    kind: info as u32,
                    symbol,
                    addend: if kind.explicit_addend() {
                        Some(i64::from_le_bytes(
                            raw[16..24].try_into().map_err(|_| "r_addend")?,
                        ))
                    } else {
                        None
                    },
                });
            }
            debug_assert_eq!(count as usize, bytes.len() / entry as usize);
        }
        for (name, count, range) in [
            ("DT_RELACOUNT", d.rela_count, d.rela),
            ("DT_RELCOUNT", d.rel_count, d.rel),
        ] {
            let entries = range.map_or(0, |r| {
                r.size
                    / if name == "DT_RELACOUNT" {
                        RELA_SIZE
                    } else {
                        REL_SIZE
                    }
            });
            if count > entries {
                return Err(format!(
                    "{name} {count} exceeds the {entries} entries in its table"
                ));
            }
        }
        Ok(d)
    }
    pub fn symbol(&self, index: u32) -> Option<&Symbol> {
        self.symbols.get(index as usize)
    }
    /// Name lookup through `DT_HASH`, matching rtld: the first definition in the bucket chain wins.
    pub fn lookup(&self, name: &str) -> Option<&Symbol> {
        let found = |s: &&Symbol| s.is_definition() && s.name == name;
        match &self.hash {
            Some(h) => {
                let mut i = h.buckets[elf_hash(name.as_bytes()) as usize % h.buckets.len()];
                // Bounded walk: a malformed cyclic chain cannot loop forever.
                for _ in 0..h.chains.len() {
                    if i == 0 {
                        return None;
                    }
                    let s = self.symbols.get(i as usize)?;
                    if found(&s) {
                        return Some(s);
                    }
                    i = h.chains[i as usize];
                }
                None
            }
            None => self.symbols.iter().skip(1).find(found),
        }
    }
}
