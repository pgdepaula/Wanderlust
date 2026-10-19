//! Process-image construction for Horizon executable modules.
//!
//! Layout follows the system loader (`ldr`): modules are placed contiguously, in a fixed
//! name order, at page-rounded sizes. Linking follows the observable rules of the runtime
//! linker (`rtld`): relative relocations come only from the `DT_RELCOUNT`/`DT_RELACOUNT`
//! prefix, symbol relocations are resolved afterwards through a load-order global scope,
//! and module initializers run in reverse load order. See `docs/platform/module-loading.md`.
use serde::Serialize;
use std::collections::BTreeMap;
use wl_common::{GuestAddr, ModuleOffset, Result};
use wl_nso::Module;
use wl_nso::dynamic::{
    Dynamic, R_AARCH64_ABS32, R_AARCH64_ABS64, R_AARCH64_GLOB_DAT, R_AARCH64_JUMP_SLOT,
    R_AARCH64_NONE, R_AARCH64_RELATIVE, Relocation, TableKind, relocation_name,
};

pub const PAGE: u64 = 0x1000;
/// Start of the 39-bit application address-space code region. ASLR would add a
/// block-aligned slide; translation fixes the slide at zero so addresses are reproducible.
pub const DEFAULT_CODE_BASE: u64 = 0x800_0000;
/// Size of the 39-bit code region available to `ldr`.
pub const CODE_REGION_SIZE: u64 = 0x7f_f800_0000;
/// `ldr` load order. Browser-core (`wkc*`) modules are not supported.
pub const MODULE_ORDER: [&str; 12] = [
    "rtld", "main", "subsdk0", "subsdk1", "subsdk2", "subsdk3", "subsdk4", "subsdk5", "subsdk6",
    "subsdk7", "subsdk8", "subsdk9",
];
const SDK: &str = "sdk";
/// Symbols through which rtld publishes its own state into loaded modules.
const RTLD_PUBLISHED: [&str; 5] = [
    "_ZN2nn2ro6detail15g_pAutoLoadListE",
    "_ZN2nn2ro6detail17g_pManualLoadListE",
    "_ZN2nn2ro6detail14g_pRoDebugFlagE",
    "_ZN2nn2ro6detail34g_pLookupGlobalAutoFunctionPointerE",
    "_ZN2nn2ro6detail36g_pLookupGlobalManualFunctionPointerE",
];

fn rank(name: &str) -> Option<usize> {
    if name == SDK {
        return Some(MODULE_ORDER.len());
    }
    MODULE_ORDER.iter().position(|n| *n == name)
}
fn align_up(value: u64) -> Result<u64> {
    value
        .checked_add(PAGE - 1)
        .map(|v| v & !(PAGE - 1))
        .ok_or_else(|| "page alignment overflow".into())
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum LinkMode {
    /// Wanderlust applies rtld-equivalent relocation before execution. Valid only when no
    /// guest code will relocate the image again.
    Host,
    /// Images are mapped exactly as `ldr` leaves them; guest code (rtld or a self-relocating
    /// crt0) must perform dynamic linking.
    Guest,
}

#[derive(Clone, Debug)]
pub struct Mapping {
    pub base: GuestAddr,
    pub bytes: Vec<u8>,
    pub read: bool,
    pub write: bool,
    pub execute: bool,
}
impl Mapping {
    fn contains(&self, address: u64, size: u64) -> Option<usize> {
        let at = address.checked_sub(self.base.0)?;
        let end = at.checked_add(size)?;
        (end <= self.bytes.len() as u64).then_some(at as usize)
    }
}
#[derive(Clone, Debug, Serialize)]
pub struct SegmentPlacement {
    pub name: &'static str,
    pub start: GuestAddr,
    pub size: u64,
    pub permissions: String,
}
#[derive(Clone, Debug, Serialize)]
pub struct LoadedModule {
    pub name: String,
    pub sha256: String,
    pub module_id: String,
    pub base: GuestAddr,
    pub size: u64,
    pub segments: Vec<SegmentPlacement>,
    pub mod0: bool,
    pub needed: Vec<String>,
    pub soname: Option<String>,
    pub symbols: usize,
    pub defined_symbols: usize,
    pub undefined_symbols: usize,
    pub relocations: BTreeMap<String, u64>,
    pub rela_count: u64,
    pub rel_count: u64,
    pub init: Option<GuestAddr>,
    pub fini: Option<GuestAddr>,
    pub init_array_entries: u64,
}
#[derive(Clone, Debug, Serialize)]
pub struct Diagnostic {
    pub severity: &'static str,
    pub module: String,
    pub kind: &'static str,
    pub detail: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub address: Option<GuestAddr>,
}
#[derive(Clone, Debug, Serialize)]
pub struct CodePointer {
    pub address: GuestAddr,
    pub source: &'static str,
    pub module: String,
}
#[derive(Clone, Debug, Serialize)]
pub struct InitCall {
    pub module: String,
    pub address: GuestAddr,
}
#[derive(Clone, Debug, Serialize)]
pub struct LinkSummary {
    pub mode: LinkMode,
    /// Relocations written into the image, by type. Empty in guest mode.
    pub applied: BTreeMap<&'static str, u64>,
    pub unresolved_symbols: usize,
    pub diagnostics: Vec<Diagnostic>,
    /// rtld's `DT_INIT` call order (reverse load order). Reported, not executed by the loader.
    pub init_order: Vec<InitCall>,
}
#[derive(Clone, Debug, Serialize)]
pub struct Process {
    pub base: GuestAddr,
    pub size: u64,
    pub entry: GuestAddr,
    pub modules: Vec<LoadedModule>,
    pub link: LinkSummary,
    /// Present when the image must not be executed as-is (for example, host-linked rtld).
    pub execution_blocker: Option<String>,
    /// Executable addresses produced by relocation, dynamic symbols and initializers.
    /// They are candidate analysis roots, not proven instruction starts.
    pub code_pointers: Vec<CodePointer>,
    /// Final 64-bit relocation cells used only for analysis. Guest-mode
    /// mappings remain unrelocated; this index never changes execution bytes.
    #[serde(skip)]
    pub relocation_cells: std::sync::Arc<BTreeMap<GuestAddr, u64>>,
    #[serde(skip)]
    pub mappings: Vec<Mapping>,
}

struct Placed<'a> {
    name: String,
    module: &'a Module,
    base: u64,
    size: u64,
    dynamic: Option<&'a Dynamic>,
}

struct Linker<'a> {
    modules: &'a [Placed<'a>],
    mappings: Vec<Mapping>,
    applied: BTreeMap<&'static str, u64>,
    diagnostics: Vec<Diagnostic>,
    unresolved: usize,
    pointers: Vec<(u64, usize, u64)>,
}
impl Linker<'_> {
    fn diag(
        &mut self,
        severity: &'static str,
        m: usize,
        kind: &'static str,
        detail: String,
        address: Option<u64>,
    ) {
        self.diagnostics.push(Diagnostic {
            severity,
            module: self.modules[m].name.clone(),
            kind,
            detail,
            address: address.map(GuestAddr),
        });
    }
    fn locate(&self, address: u64, size: u64) -> Option<(usize, usize)> {
        self.mappings
            .iter()
            .enumerate()
            .find_map(|(i, m)| m.contains(address, size).map(|at| (i, at)))
    }
    fn target(&mut self, m: usize, r: &Relocation, size: u64) -> Option<(usize, usize, u64)> {
        let address = self.modules[m].base.checked_add(r.offset.0);
        match address.and_then(|a| self.locate(a, size).map(|(i, at)| (i, at, a))) {
            Some((i, at, a)) if self.mappings[i].write => Some((i, at, a)),
            Some((_, _, a)) => {
                self.diag(
                    "error",
                    m,
                    "relocation-target-not-writable",
                    format!(
                        "{} {:?}[{}] targets a non-writable segment; not applied",
                        relocation_name(r.kind),
                        r.table,
                        r.index
                    ),
                    Some(a),
                );
                None
            }
            None => {
                self.diag(
                    "error",
                    m,
                    "relocation-target-unmapped",
                    format!(
                        "{} {:?}[{}] at module offset {:#x} is outside the process image",
                        relocation_name(r.kind),
                        r.table,
                        r.index,
                        r.offset.0
                    ),
                    address,
                );
                None
            }
        }
    }
    fn read64(&self, i: usize, at: usize) -> u64 {
        u64::from_le_bytes(
            self.mappings[i].bytes[at..at + 8]
                .try_into()
                .unwrap_or([0; 8]),
        )
    }
    fn write(&mut self, i: usize, at: usize, bytes: &[u8]) {
        self.mappings[i].bytes[at..at + bytes.len()].copy_from_slice(bytes);
    }
    fn pointer(&mut self, value: u64, m: usize, address: u64) {
        self.pointers.push((value, m, address));
    }
    /// rtld `TryResolveSymbol`: non-default visibility binds within the module; otherwise the
    /// first defining, non-local symbol in load order wins.
    fn resolve(&mut self, m: usize, symbol: u32) -> Option<u64> {
        if symbol == 0 {
            // STN_UNDEF: the ELF symbol value is zero.
            return Some(0);
        }
        let dynamic = self.modules[m].dynamic?;
        let s = dynamic.symbol(symbol)?;
        if s.visibility != 0 {
            if let Some(found) = dynamic.lookup(&s.name) {
                return Some(self.modules[m].base.wrapping_add(found.value));
            }
            return (s.bind == 2).then_some(0);
        }
        self.modules.iter().find_map(|p| {
            let found = p.dynamic?.lookup(&s.name)?;
            (found.bind != 0).then(|| p.base.wrapping_add(found.value))
        })
    }
    /// `DT_RELR` packed relative relocations (ELF gABI): an even entry is the module offset
    /// of a word to relocate and starts a run; an odd entry is a bitmap whose bit i (i >= 1)
    /// relocates word i-1 after the previous run position, which then advances by 63 words.
    /// Each relocated word receives `*P += B`. Nintendo's rtld does not process RELR; libnx's
    /// self-relocation does, so this is applied as a documented extension.
    fn relr(&mut self, m: usize, range: wl_nso::dynamic::Range) {
        let base = self.modules[m].base;
        let module = self.modules[m].module;
        let Ok(bytes) = module.read(range.offset, range.size) else {
            self.diag(
                "error",
                m,
                "relr-unmapped",
                "DT_RELR table is not mapped".into(),
                None,
            );
            return;
        };
        let mut next: Option<u64> = None;
        let mut count = 0u64;
        for entry in bytes.chunks_exact(8) {
            let word = u64::from_le_bytes(entry.try_into().unwrap_or([0; 8]));
            let targets: Vec<u64> = if word & 1 == 0 {
                next = Some(word + 8);
                vec![word]
            } else {
                let Some(start) = next else {
                    self.diag(
                        "error",
                        m,
                        "relr-malformed",
                        "DT_RELR bitmap precedes any address entry".into(),
                        None,
                    );
                    return;
                };
                next = Some(start + 63 * 8);
                (1..64)
                    .filter(|i| (word >> i) & 1 == 1)
                    .map(|i| start + (i - 1) * 8)
                    .collect()
            };
            for offset in targets {
                let address = base.wrapping_add(offset);
                match self.locate(address, 8) {
                    Some((i, at)) if self.mappings[i].write => {
                        let value = base.wrapping_add(self.read64(i, at));
                        self.write(i, at, &value.to_le_bytes());
                        self.pointer(value, m, address);
                        count += 1;
                    }
                    _ => {
                        self.diag("error", m, "relocation-target-unmapped", format!("DT_RELR target at module offset {offset:#x} is not writable mapped memory"), Some(address));
                    }
                }
            }
        }
        *self.applied.entry("DT_RELR (relative)").or_default() += count;
        self.diag("note", m, "relr-extension", format!("{count} DT_RELR relocations applied; Horizon's rtld does not process DT_RELR (libnx's self-relocation does)"), None);
    }
    fn relative_prefix(&mut self, m: usize) {
        let Some(d) = self.modules[m].dynamic else {
            return;
        };
        let base = self.modules[m].base;
        for r in &d.relocations {
            let counted = match r.table {
                TableKind::Rela => u64::from(r.index) < d.rela_count,
                TableKind::Rel => u64::from(r.index) < d.rel_count,
                _ => false,
            };
            if !counted {
                if r.kind == R_AARCH64_RELATIVE {
                    self.diag("warning", m, "relative-outside-count", format!("{:?}[{}] is R_AARCH64_RELATIVE outside the DT_REL(A)COUNT prefix; rtld does not apply it", r.table, r.index), base.checked_add(r.offset.0));
                }
                continue;
            }
            if r.kind != R_AARCH64_RELATIVE {
                self.diag(
                    "warning",
                    m,
                    "non-relative-in-count",
                    format!(
                        "{:?}[{}] is {} inside the relative prefix; rtld ignores it",
                        r.table,
                        r.index,
                        relocation_name(r.kind)
                    ),
                    base.checked_add(r.offset.0),
                );
                continue;
            }
            let Some((i, at, address)) = self.target(m, r, 8) else {
                continue;
            };
            // RELA: B + A. REL: the implicit addend is the stored word, so B + *P.
            let value = match r.addend {
                Some(a) => base.wrapping_add_signed(a),
                None => base.wrapping_add(self.read64(i, at)),
            };
            self.write(i, at, &value.to_le_bytes());
            self.pointer(value, m, address);
            *self.applied.entry("R_AARCH64_RELATIVE").or_default() += 1;
        }
    }
    fn symbolic(&mut self, m: usize) {
        let Some(d) = self.modules[m].dynamic else {
            return;
        };
        for r in &d.relocations {
            let counted = match r.table {
                TableKind::Rela => u64::from(r.index) < d.rela_count,
                TableKind::Rel => u64::from(r.index) < d.rel_count,
                _ => false,
            };
            let plt = matches!(r.table, TableKind::PltRel | TableKind::PltRela);
            if counted || r.kind == R_AARCH64_RELATIVE || r.kind == R_AARCH64_NONE {
                continue;
            }
            let expected = if plt {
                r.kind == R_AARCH64_JUMP_SLOT
            } else {
                matches!(
                    r.kind,
                    R_AARCH64_ABS64 | R_AARCH64_ABS32 | R_AARCH64_GLOB_DAT
                )
            };
            if !expected {
                self.diag(
                    "error",
                    m,
                    "unsupported-relocation",
                    format!(
                        "{} (type {}) in {:?}[{}] is not processed by rtld; not applied",
                        relocation_name(r.kind),
                        r.kind,
                        r.table,
                        r.index
                    ),
                    self.modules[m].base.checked_add(r.offset.0),
                );
                continue;
            }
            let size = if r.kind == R_AARCH64_ABS32 { 4 } else { 8 };
            let Some((i, at, address)) = self.target(m, r, size) else {
                continue;
            };
            let name = d
                .symbol(r.symbol)
                .map(|s| s.name.clone())
                .unwrap_or_default();
            let weak = d.symbol(r.symbol).is_some_and(|s| s.bind == 2);
            let Some(s) = self.resolve(m, r.symbol) else {
                self.unresolved += 1;
                let lazy = if plt {
                    // rtld's lazy initialization leaves the slot pointing at the PLT resolver stub.
                    let stub = self.modules[m].base.wrapping_add(self.read64(i, at));
                    self.write(i, at, &stub.to_le_bytes());
                    format!("; slot left at PLT stub {stub:#x}")
                } else {
                    "; target left unchanged".into()
                };
                self.diag(
                    if weak { "warning" } else { "error" },
                    m,
                    "unresolved-symbol",
                    format!(
                        "{} {:?} for {}{lazy}",
                        relocation_name(r.kind),
                        name,
                        if weak {
                            "weak reference"
                        } else {
                            "strong reference"
                        }
                    ),
                    Some(address),
                );
                continue;
            };
            if r.kind == R_AARCH64_ABS32 {
                let value = i128::from(s)
                    + i128::from(r.addend.unwrap_or_else(|| {
                        i64::from(u32::from_le_bytes(
                            self.mappings[i].bytes[at..at + 4]
                                .try_into()
                                .unwrap_or([0; 4]),
                        ) as i32)
                    }));
                // AArch64 ELF ABI overflow check for ABS32: -2^31 <= X < 2^32.
                if !(-(1i128 << 31)..(1i128 << 32)).contains(&value) {
                    self.diag(
                        "error",
                        m,
                        "relocation-overflow",
                        format!(
                            "R_AARCH64_ABS32 value {value:#x} for {name:?} does not fit 32 bits"
                        ),
                        Some(address),
                    );
                    continue;
                }
                self.write(i, at, &(value as u32).to_le_bytes());
            } else {
                let value = match r.addend {
                    Some(a) => s.wrapping_add_signed(a),
                    // A REL jump slot holds the lazy PLT stub address, not an addend; rtld's
                    // lazy binder stores S alone. Other REL types use the stored implicit addend.
                    None if plt => s,
                    None => self.read64(i, at).wrapping_add(s),
                };
                self.write(i, at, &value.to_le_bytes());
                self.pointer(value, m, address);
            }
            *self.applied.entry(relocation_name(r.kind)).or_default() += 1;
        }
    }
}

impl Process {
    /// Places named modules in `ldr` order at `base` and links them according to `mode`.
    pub fn build(inputs: &[(String, Module)], base: GuestAddr, mode: LinkMode) -> Result<Self> {
        if inputs.is_empty() {
            return Err("a process requires at least one module".into());
        }
        if base.0 % PAGE != 0 {
            return Err(format!("process base {:#x} is not page aligned", base.0));
        }
        let mut order: Vec<(usize, &String, &Module)> = Vec::new();
        for (name, module) in inputs {
            let r = rank(name).ok_or_else(|| {
                format!(
                    "module name {name:?} is not an ExeFS module name (rtld, main, subsdk0-9, sdk)"
                )
            })?;
            if order.iter().any(|(o, _, _)| *o == r) {
                return Err(format!("duplicate module {name:?}"));
            }
            order.push((r, name, module));
        }
        order.sort_by_key(|(r, _, _)| *r);
        let mut placed = Vec::new();
        let mut mappings = Vec::new();
        let mut cursor = base.0;
        for (_, name, module) in &order {
            let [text, ro, rw] = [
                &module.segments[0],
                &module.segments[1],
                &module.segments[2],
            ];
            if text.guest_offset.0 != 0
                || ro.guest_offset.0 % PAGE != 0
                || rw.guest_offset.0 % PAGE != 0
            {
                return Err(format!(
                    "{name}: ldr requires text at offset 0 and page-aligned rodata/data offsets"
                ));
            }
            let end = |s: &wl_nso::Segment| s.guest_offset.0 + u64::from(s.memory_size);
            let image_end = end(text).max(end(ro)).max(end(rw));
            let size = align_up(image_end + u64::from(module.bss_size))?;
            for (s, next) in [(text, ro.guest_offset.0), (ro, rw.guest_offset.0)] {
                if align_up(end(s))? > next {
                    return Err(format!(
                        "{name}: {} overlaps the following segment after page rounding",
                        s.name
                    ));
                }
            }
            for (s, extra) in [(text, 0), (ro, 0), (rw, u64::from(module.bss_size))] {
                let length = align_up(u64::from(s.memory_size) + extra)?;
                if length == 0 {
                    continue;
                }
                let mut bytes = Vec::new();
                bytes
                    .try_reserve_exact(length as usize)
                    .map_err(|e| format!("{name}: {e}"))?;
                bytes.extend_from_slice(&s.bytes);
                bytes.resize(length as usize, 0);
                mappings.push(Mapping {
                    base: GuestAddr(
                        cursor
                            .checked_add(s.guest_offset.0)
                            .ok_or("mapping overflow")?,
                    ),
                    bytes,
                    read: s.readable,
                    write: s.writable,
                    execute: s.executable,
                });
            }
            placed.push(Placed {
                name: (*name).clone(),
                module,
                base: cursor,
                size,
                dynamic: module.mod0.as_ref().map(|m| &m.dynamic),
            });
            cursor = cursor.checked_add(size).ok_or("process image overflow")?;
        }
        let total = cursor - base.0;
        if total > CODE_REGION_SIZE || base.0.checked_add(total).is_none() {
            return Err(format!(
                "process image of {total:#x} bytes exceeds the 39-bit code region"
            ));
        }
        // Linking always runs on a copy so that guest-mode analysis still sees relocated pointers.
        let mut linker = Linker {
            modules: &placed,
            mappings: mappings.clone(),
            applied: BTreeMap::new(),
            diagnostics: Vec::new(),
            unresolved: 0,
            pointers: Vec::new(),
        };
        for (m, p) in placed.iter().enumerate() {
            if let Some(d) = p.dynamic {
                if let Some(range) = d.relr {
                    linker.relr(m, range);
                }
                for name in RTLD_PUBLISHED {
                    if d.lookup(name).is_some() && mode == LinkMode::Host {
                        linker.diag("warning", m, "rtld-state-not-published", format!("{name} is defined; host linking does not publish rtld state into it"), None);
                    }
                }
            }
            linker.relative_prefix(m);
        }
        for m in 0..placed.len() {
            linker.symbolic(m);
        }
        let mut init_order = Vec::new();
        let mut code_pointers = Vec::new();
        for p in placed.iter().rev() {
            if let Some(init) = p.dynamic.and_then(|d| d.init) {
                init_order.push(InitCall {
                    module: p.name.clone(),
                    address: GuestAddr(p.base + init.0),
                });
            }
        }
        let executable = |mappings: &[Mapping], a: u64| {
            a % 4 == 0
                && mappings
                    .iter()
                    .any(|m| m.execute && m.contains(a, 4).is_some())
        };
        // Read the final analysis bytes rather than an earlier relocation's
        // value: later relocations (including partial writes) may replace it.
        let relocation_cells = std::sync::Arc::new(
            linker
                .pointers
                .iter()
                .filter_map(|&(_, _, address)| {
                    let (mapping, at) = linker.locate(address, 8)?;
                    let value = linker.read64(mapping, at);
                    Some((GuestAddr(address), value))
                })
                .collect(),
        );
        let mut seen = std::collections::BTreeSet::new();
        let mut push =
            |address: u64, source: &'static str, module: &str, list: &mut Vec<CodePointer>| {
                if executable(&linker.mappings, address) && seen.insert((address, source)) {
                    list.push(CodePointer {
                        address: GuestAddr(address),
                        source,
                        module: module.into(),
                    });
                }
            };
        for (value, m, _) in &linker.pointers {
            push(*value, "relocation", &placed[*m].name, &mut code_pointers);
        }
        let mut pointer_array_entries = 0u64;
        for p in &placed {
            if let Some(d) = p.dynamic {
                for s in d
                    .symbols
                    .iter()
                    .filter(|s| s.is_definition() && s.is_function())
                {
                    push(
                        p.base.wrapping_add(s.value),
                        "dynamic-symbol",
                        &p.name,
                        &mut code_pointers,
                    );
                }
                for init in [d.init, d.fini].into_iter().flatten() {
                    push(p.base + init.0, "initializer", &p.name, &mut code_pointers);
                }
                for (source, array) in [
                    ("initializer-array", d.init_array),
                    ("finalizer-array", d.fini_array),
                ] {
                    let Some(array) = array else { continue };
                    if array.size % 8 != 0 {
                        return Err(
                            "initializer/finalizer array size is not a multiple of 8".into()
                        );
                    }
                    pointer_array_entries = pointer_array_entries
                        .checked_add(array.size / 8)
                        .filter(|n| *n <= 1_048_576)
                        .ok_or("initializer/finalizer arrays exceed the analysis entry budget")?;
                    let start = p
                        .base
                        .checked_add(array.offset.0)
                        .ok_or("initializer/finalizer array address overflow")?;
                    for offset in (0..array.size).step_by(8) {
                        let address = start
                            .checked_add(offset)
                            .ok_or("initializer/finalizer entry address overflow")?;
                        let (mapping, at) = linker
                            .locate(address, 8)
                            .ok_or("initializer/finalizer entry is not mapped in one segment")?;
                        // The analysis copy is already relocated, even in guest
                        // mode. Never rebase the value again or modify the
                        // guest image; guest code still executes these calls.
                        push(
                            linker.read64(mapping, at),
                            source,
                            &p.name,
                            &mut code_pointers,
                        );
                    }
                }
            }
        }
        code_pointers.sort_by_key(|c| (c.address, c.source));
        let modules = placed
            .iter()
            .map(|p| summarize(p, &linker.mappings))
            .collect();
        let entry = GuestAddr(placed[0].base);
        let has_rtld = placed[0].name == "rtld";
        let needs_linking = placed
            .iter()
            .any(|p| p.dynamic.is_some_and(|d| !d.relocations.is_empty()));
        let execution_blocker = match mode {
            LinkMode::Host if has_rtld => Some("host-linked image contains rtld, which would relocate it again (REL relative and lazy JUMP_SLOT initialization are not idempotent); use guest linking".into()),
            LinkMode::Host if linker.diagnostics.iter().any(|d| d.severity == "error") => Some("host linking reported errors".into()),
            _ => None,
        };
        if mode == LinkMode::Guest && !has_rtld && needs_linking {
            linker.diag("note", 0, "guest-linker-required", "no rtld module is present; relocations are applied only if the entry code relocates itself".into(), None);
        }
        let (applied, final_mappings) = match mode {
            LinkMode::Host => (linker.applied.clone(), linker.mappings),
            LinkMode::Guest => (BTreeMap::new(), mappings),
        };
        Ok(Self {
            base,
            size: total,
            entry,
            modules,
            link: LinkSummary {
                mode,
                applied,
                unresolved_symbols: linker.unresolved,
                diagnostics: linker.diagnostics,
                init_order,
            },
            execution_blocker,
            code_pointers,
            relocation_cells,
            mappings: final_mappings,
        })
    }
    /// Reads a little-endian u64 from the process image (diagnostics and tests).
    pub fn read_u64(&self, address: GuestAddr) -> Option<u64> {
        self.mappings.iter().find_map(|m| {
            let at = m.contains(address.0, 8)?;
            Some(u64::from_le_bytes(m.bytes[at..at + 8].try_into().ok()?))
        })
    }
    /// Translates a module-relative offset of a named module into a guest address.
    pub fn address(&self, module: &str, offset: ModuleOffset) -> Option<GuestAddr> {
        let m = self.modules.iter().find(|m| m.name == module)?;
        (offset.0 < m.size).then(|| GuestAddr(m.base.0 + offset.0))
    }
}

fn summarize(p: &Placed, mappings: &[Mapping]) -> LoadedModule {
    let module = p.module;
    let mut relocations = BTreeMap::new();
    let mut segments = Vec::new();
    for s in &module.segments {
        let start = p.base + s.guest_offset.0;
        let size = mappings
            .iter()
            .find(|m| m.base.0 == start)
            .map_or(0, |m| m.bytes.len() as u64);
        segments.push(SegmentPlacement {
            name: s.name,
            start: GuestAddr(start),
            size,
            permissions: format!(
                "{}{}{}",
                if s.readable { "r" } else { "-" },
                if s.writable { "w" } else { "-" },
                if s.executable { "x" } else { "-" }
            ),
        });
    }
    let d = p.dynamic;
    if let Some(d) = d {
        for r in &d.relocations {
            let key = match relocation_name(r.kind) {
                "unknown" => format!("type-{}", r.kind),
                name => name.to_string(),
            };
            *relocations.entry(key).or_default() += 1;
        }
    }
    let symbols = d.map_or(&[][..], |d| &d.symbols[..]);
    LoadedModule {
        name: p.name.clone(),
        sha256: module.sha256.clone(),
        module_id: module.module_id.clone(),
        base: GuestAddr(p.base),
        size: p.size,
        segments,
        mod0: module.mod0.is_some(),
        needed: d.map(|d| d.needed.clone()).unwrap_or_default(),
        soname: d.and_then(|d| d.soname.clone()),
        symbols: symbols.len(),
        defined_symbols: symbols.iter().filter(|s| s.is_definition()).count(),
        undefined_symbols: symbols
            .iter()
            .skip(1)
            .filter(|s| !s.is_definition())
            .count(),
        relocations,
        rela_count: d.map_or(0, |d| d.rela_count),
        rel_count: d.map_or(0, |d| d.rel_count),
        init: d.and_then(|d| d.init).map(|o| GuestAddr(p.base + o.0)),
        fini: d.and_then(|d| d.fini).map(|o| GuestAddr(p.base + o.0)),
        init_array_entries: d.and_then(|d| d.init_array).map_or(0, |a| a.size / 8),
    }
}
