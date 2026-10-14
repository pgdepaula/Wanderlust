//! Recursive traversal from explicit and metadata-derived roots; no heuristic function-boundary claims.
//! Indirect control flow is resolved only where constant propagation proves the targets
//! (jump tables, code pointers formed in code; see [`flow`]).
pub mod flow;
use serde::Serialize;
use std::collections::{BTreeMap, BTreeSet, VecDeque};
use wl_aarch64::{Instruction, Op, decode};
use wl_common::{GuestAddr, Result};
pub use wl_loader::Mapping;
use wl_loader::{LinkMode, Process};
use wl_nso::Module;

pub const RAW_BASE: u64 = 0x10000;
pub const MAX_INSTRUCTIONS: usize = 1_000_000;
/// Linear survey bound: 512 MiB of executable text.
pub const MAX_SURVEY_WORDS: usize = 128 * 1024 * 1024;

/// Whether the mapped bytes are in the state the guest expects to execute.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum RelocationState {
    /// No dynamic metadata is known to require relocation.
    None,
    /// A MOD0 module mapped without relocation and without a guest linker: unsafe to execute.
    NotApplied,
    /// Relocations were applied by the host loader.
    AppliedByHost,
    /// Guest code (rtld or a self-relocating crt0) is responsible for relocation.
    GuestLinker,
}
#[derive(Clone, Debug)]
pub struct Image {
    pub mappings: Vec<Mapping>,
    pub module_id: String,
    pub sha256: String,
    pub relocation: RelocationState,
}
impl Image {
    pub fn raw(bytes: Vec<u8>, base: GuestAddr) -> Result<Self> {
        if bytes.is_empty() || bytes.len() % 4 != 0 || base.0 % 4 != 0 {
            return Err("raw A64 input must be nonempty and instruction aligned".into());
        }
        base.0
            .checked_add(bytes.len() as u64)
            .ok_or("raw image address overflow")?;
        Ok(Self {
            mappings: vec![Mapping {
                base,
                bytes,
                read: true,
                write: false,
                execute: true,
            }],
            module_id: "raw-aarch64".into(),
            sha256: String::new(),
            relocation: RelocationState::None,
        })
    }
    /// A single module at an explicit base, without process layout or relocation.
    pub fn nso(module: &Module, base: GuestAddr) -> Result<Self> {
        let mut mappings = Vec::new();
        for s in &module.segments {
            let address = base
                .0
                .checked_add(s.guest_offset.0)
                .ok_or("NSO base-address overflow")?;
            address
                .checked_add(s.bytes.len() as u64)
                .ok_or("NSO segment-address overflow")?;
            mappings.push(Mapping {
                base: GuestAddr(address),
                bytes: s.bytes.clone(),
                read: s.readable,
                write: s.writable,
                execute: s.executable,
            });
        }
        let bss = base
            .0
            .checked_add(module.bss_offset.0)
            .ok_or("NSO BSS-address overflow")?;
        bss.checked_add(u64::from(module.bss_size))
            .ok_or("NSO BSS-end overflow")?;
        let mut bytes = Vec::new();
        bytes
            .try_reserve_exact(module.bss_size as usize)
            .map_err(|e| e.to_string())?;
        bytes.resize(module.bss_size as usize, 0);
        // BSS joins data so ordinary accesses across the data/BSS boundary remain contiguous.
        mappings[2].bytes.extend(bytes);
        let has_relocations = module
            .mod0
            .as_ref()
            .is_some_and(|m| !m.dynamic.relocations.is_empty() || m.dynamic.relr.is_some());
        Ok(Self {
            mappings,
            module_id: module.module_id.clone(),
            sha256: module.sha256.clone(),
            // A MOD0 module whose dynamic table holds no relocation entries needs none.
            relocation: if has_relocations {
                RelocationState::NotApplied
            } else {
                RelocationState::None
            },
        })
    }
    /// The memory image of a loaded process.
    pub fn process(process: &Process) -> Self {
        let ids: Vec<_> = process
            .modules
            .iter()
            .map(|m| format!("{}:{}", m.name, m.module_id))
            .collect();
        Self {
            mappings: process.mappings.clone(),
            module_id: ids.join(","),
            sha256: String::new(),
            relocation: match process.link.mode {
                LinkMode::Host => RelocationState::AppliedByHost,
                LinkMode::Guest => RelocationState::GuestLinker,
            },
        }
    }
    /// A little-endian value of 1, 2, 4 or 8 bytes from any readable mapping.
    pub fn read_data(&self, address: u64, bytes: u8) -> Option<u64> {
        let m = self
            .mappings
            .iter()
            .find(|m| m.read && address >= m.base.0 && address - m.base.0 < m.bytes.len() as u64)?;
        let at = usize::try_from(address - m.base.0).ok()?;
        let slice = m.bytes.get(at..at.checked_add(usize::from(bytes))?)?;
        let mut value = [0u8; 8];
        value[..slice.len()].copy_from_slice(slice);
        Some(u64::from_le_bytes(value))
    }
    /// Whether `address` is an instruction-aligned address inside an executable mapping.
    pub fn is_code(&self, address: u64) -> bool {
        address % 4 == 0
            && self.mappings.iter().any(|m| {
                m.execute && address >= m.base.0 && address - m.base.0 < m.bytes.len() as u64
            })
    }
    fn word(&self, pc: GuestAddr) -> Result<u32> {
        if pc.0 % 4 != 0 {
            return Err(format!("unaligned control-flow target {:#x}", pc.0));
        }
        for m in &self.mappings {
            if !m.execute {
                continue;
            }
            if let Some(at) =
                pc.0.checked_sub(m.base.0)
                    .and_then(|v| usize::try_from(v).ok())
            {
                if let Some(bytes) = at.checked_add(4).and_then(|end| m.bytes.get(at..end)) {
                    return Ok(u32::from_le_bytes(
                        bytes.try_into().map_err(|_| "truncated opcode")?,
                    ));
                }
            }
        }
        Err(format!(
            "control-flow target {:#x} is outside executable mappings",
            pc.0
        ))
    }
}
#[derive(Clone, Debug, Serialize)]
pub struct UnsupportedSite {
    pub pc: GuestAddr,
    pub word: u32,
    pub reason: String,
}
#[derive(Clone, Debug, Serialize)]
pub struct Block {
    pub start: GuestAddr,
    pub instructions: Vec<Instruction>,
    /// The block ends at this undecodable or unsupported instruction; executing it faults.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub unsupported: Option<UnsupportedSite>,
}
#[derive(Clone, Debug, Serialize)]
pub struct RejectedRoot {
    pub address: GuestAddr,
    pub reason: String,
}
#[derive(Clone, Debug, Serialize)]
pub struct Program {
    pub entry: GuestAddr,
    pub roots: Vec<GuestAddr>,
    /// Required roots (entry and seeds) as given.
    pub required_roots: usize,
    pub speculative_roots_accepted: usize,
    pub rejected_roots: Vec<RejectedRoot>,
    /// Supported instructions reached by traversal.
    pub instruction_count: usize,
    /// Reached instructions that cannot be translated; each becomes a precise runtime fault.
    pub unsupported_count: usize,
    pub blocks: Vec<Block>,
    pub indirect_sites: Vec<GuestAddr>,
    /// Indirect branches resolved as jump tables, with their entry count.
    pub jump_tables: BTreeMap<GuestAddr, usize>,
    /// Roots added from jump-table entries and constant code pointers.
    pub flow_roots: usize,
}
#[derive(Clone, Debug, Default)]
pub struct Roots {
    /// Required roots: the entry and explicit seeds. Failure to traverse them is an error.
    pub required: Vec<GuestAddr>,
    /// Candidate roots from metadata (relocated code pointers, dynamic symbols). A candidate
    /// whose traversal leaves executable memory is discarded with all code reached only from it.
    pub speculative: Vec<GuestAddr>,
    /// Record unsupported instructions as fault sites instead of failing.
    pub allow_unsupported: bool,
    /// Maximum decoded instructions (default `MAX_INSTRUCTIONS`). Roots that would exceed it
    /// are rejected; required roots and the flow resolution of their code come first.
    pub instruction_limit: Option<usize>,
}
#[derive(Clone, Debug)]
enum Site {
    Instruction(Instruction),
    Unsupported(UnsupportedSite),
}
fn is_end(op: Op) -> bool {
    matches!(
        op,
        Op::Trap { .. }
            | Op::Branch { .. }
            | Op::Conditional { .. }
            | Op::CompareBranch { .. }
            | Op::TestBranch { .. }
            | Op::Indirect { .. }
            | Op::Svc { .. }
    )
}
fn successors(instruction: &Instruction) -> Result<Vec<GuestAddr>> {
    let next = GuestAddr(
        instruction
            .pc
            .0
            .checked_add(4)
            .ok_or("instruction fallthrough overflow")?,
    );
    Ok(match instruction.op {
        Op::Branch { target, link } => {
            if link {
                vec![target, next]
            } else {
                vec![target]
            }
        }
        Op::Conditional { target, .. }
        | Op::CompareBranch { target, .. }
        | Op::TestBranch { target, .. } => vec![target, next],
        Op::Indirect { link, .. } => {
            if link {
                vec![next]
            } else {
                vec![]
            }
        }
        Op::Svc { id: 7 } | Op::Trap { .. } => vec![],
        _ => vec![next],
    })
}
/// Traverses from `root`, adding newly decoded sites to `out`. `known` is never modified.
fn walk(
    image: &Image,
    root: GuestAddr,
    known: &BTreeMap<GuestAddr, Site>,
    out: &mut BTreeMap<GuestAddr, Site>,
    allow_unsupported: bool,
    limit: usize,
) -> Result<()> {
    let mut queue = VecDeque::from([root]);
    while let Some(pc) = queue.pop_front() {
        if known.contains_key(&pc) || out.contains_key(&pc) {
            continue;
        }
        if known.len() + out.len() >= limit {
            return Err(format!(
                "analysis exceeds {limit} reachable-instruction limit"
            ));
        }
        let word = image.word(pc)?;
        match decode(word, pc) {
            Ok(instruction) => {
                queue.extend(successors(&instruction)?);
                out.insert(pc, Site::Instruction(instruction));
            }
            Err(reason) if allow_unsupported => {
                out.insert(pc, Site::Unsupported(UnsupportedSite { pc, word, reason }));
            }
            Err(reason) => return Err(reason),
        }
    }
    Ok(())
}

/// Strict traversal from an entry and explicit seeds.
pub fn discover(image: &Image, entry: GuestAddr, seeds: &[GuestAddr]) -> Result<Program> {
    discover_roots(
        image,
        &Roots {
            required: std::iter::once(entry)
                .chain(seeds.iter().copied())
                .collect(),
            ..Roots::default()
        },
    )
}

fn leaders(
    decoded: &BTreeMap<GuestAddr, Site>,
    entries: &BTreeSet<GuestAddr>,
    tables: &BTreeMap<GuestAddr, Vec<GuestAddr>>,
) -> Result<BTreeSet<GuestAddr>> {
    let mut leaders = entries.clone();
    for site in decoded.values() {
        if let Site::Instruction(i) = site {
            if is_end(i.op) {
                leaders.extend(successors(i)?);
            }
        }
    }
    leaders.extend(tables.values().flatten().copied());
    Ok(leaders)
}
fn build_blocks(
    decoded: &BTreeMap<GuestAddr, Site>,
    leaders: &BTreeSet<GuestAddr>,
) -> Result<Vec<Block>> {
    let mut blocks: Vec<Block> = Vec::new();
    for (&pc, site) in decoded {
        let extend = blocks.last().is_some_and(|b| {
            b.unsupported.is_none()
                && b.instructions
                    .last()
                    .is_some_and(|p| !is_end(p.op) && p.pc.0 + 4 == pc.0)
        }) && !leaders.contains(&pc);
        if !extend {
            blocks.push(Block {
                start: pc,
                instructions: Vec::new(),
                unsupported: None,
            });
        }
        let block = blocks.last_mut().ok_or("missing block")?;
        match site {
            Site::Instruction(i) => block.instructions.push(i.clone()),
            Site::Unsupported(u) => block.unsupported = Some(u.clone()),
        }
    }
    Ok(blocks)
}

pub fn discover_roots(image: &Image, roots: &Roots) -> Result<Program> {
    let entry = *roots.required.first().ok_or("analysis requires an entry")?;
    let limit = roots.instruction_limit.unwrap_or(MAX_INSTRUCTIONS);
    let mut decoded = BTreeMap::new();
    for &root in &roots.required {
        let mut found = BTreeMap::new();
        walk(
            image,
            root,
            &decoded,
            &mut found,
            roots.allow_unsupported,
            limit,
        )?;
        decoded.append(&mut found);
    }
    let mut accepted = Vec::new();
    let mut rejected_roots = Vec::new();
    let mut try_root = |root: GuestAddr, decoded: &mut BTreeMap<GuestAddr, Site>| {
        let mut found = BTreeMap::new();
        match walk(
            image,
            root,
            decoded,
            &mut found,
            roots.allow_unsupported,
            limit,
        ) {
            Ok(()) => {
                decoded.append(&mut found);
                true
            }
            Err(reason) => {
                rejected_roots.push(RejectedRoot {
                    address: root,
                    reason,
                });
                false
            }
        }
    };
    // Resolve jump tables and constant code pointers until no new code appears: first for
    // code reachable from the required roots, then again after the speculative roots, so a
    // large speculative set cannot exhaust the instruction limit before the entry's own
    // indirect control flow is resolved.
    let mut entries: BTreeSet<GuestAddr> = roots.required.iter().copied().collect();
    let mut tables = BTreeMap::new();
    let mut tried = BTreeSet::new();
    let mut flow_roots = 0;
    for phase in 0..2 {
        if phase == 1 {
            for &root in &roots.speculative {
                if try_root(root, &mut decoded) {
                    accepted.push(root);
                    entries.insert(root);
                }
            }
        }
        loop {
            let blocks = build_blocks(&decoded, &leaders(&decoded, &entries, &tables)?)?;
            let found = flow::propagate(image, &blocks, &entries, &tables);
            let mut changed = false;
            // Constant code pointers are function entries; jump-table targets are only block
            // leaders inside the function that owns the table.
            let mut candidates: Vec<(GuestAddr, bool)> =
                found.pointers.into_iter().map(|p| (p, true)).collect();
            for (branch, targets) in found.tables {
                if tables.get(&branch) != Some(&targets) {
                    candidates.extend(targets.iter().map(|&t| (t, false)));
                    tables.insert(branch, targets);
                    changed = true;
                }
            }
            for (root, function) in candidates {
                if tried.insert(root) && try_root(root, &mut decoded) {
                    flow_roots += 1;
                    if function {
                        entries.insert(root);
                    }
                    changed = true;
                }
            }
            if !changed {
                break;
            }
        }
    }
    let speculative_roots_accepted = accepted.len();
    let mut indirect_sites = Vec::new();
    let mut instruction_count = 0;
    let mut unsupported_count = 0;
    for site in decoded.values() {
        match site {
            Site::Instruction(i) => {
                instruction_count += 1;
                if matches!(i.op, Op::Indirect { .. }) {
                    indirect_sites.push(i.pc);
                }
            }
            Site::Unsupported(_) => unsupported_count += 1,
        }
    }
    let blocks = build_blocks(&decoded, &leaders(&decoded, &entries, &tables)?)?;
    let mut all_roots = roots.required.clone();
    all_roots.extend(accepted.iter().copied());
    let found: Vec<GuestAddr> = tried
        .iter()
        .filter(|r| decoded.contains_key(r) && !all_roots.contains(r))
        .copied()
        .collect();
    all_roots.extend(found);
    Ok(Program {
        entry,
        required_roots: roots.required.len(),
        roots: all_roots,
        speculative_roots_accepted,
        rejected_roots,
        instruction_count,
        unsupported_count,
        blocks,
        indirect_sites,
        jump_tables: tables.iter().map(|(b, t)| (*b, t.len())).collect(),
        flow_roots,
    })
}

/// Linear text survey for intake triage. Counts data/padding too; it is not CFG coverage.
#[derive(Debug, Serialize)]
pub struct Survey {
    pub method: &'static str,
    pub words: usize,
    pub supported: usize,
    pub families: BTreeMap<&'static str, usize>,
    pub unsupported_words: usize,
    pub unsupported_unique: usize,
    /// Unsupported words by Arm ARM encoding class (`wl_aarch64::encoding_class`).
    pub unsupported_classes: BTreeMap<&'static str, usize>,
    pub unsupported_examples: Vec<UnsupportedWord>,
}
#[derive(Debug, Serialize)]
pub struct UnsupportedWord {
    pub opcode: u32,
    pub count: usize,
    pub first_pc: GuestAddr,
}
/// `examples` bounds the number of distinct unsupported opcodes listed (most frequent first).
pub fn survey(image: &Image, examples: usize) -> Result<Survey> {
    let mut report = Survey {
        method: "linear sweep of executable mappings; includes possible data/padding; not reachable-code coverage",
        words: 0,
        supported: 0,
        families: BTreeMap::new(),
        unsupported_words: 0,
        unsupported_unique: 0,
        unsupported_classes: BTreeMap::new(),
        unsupported_examples: Vec::new(),
    };
    let mut unknown: BTreeMap<u32, UnsupportedWord> = BTreeMap::new();
    for mapping in image.mappings.iter().filter(|m| m.execute) {
        for (index, bytes) in mapping.bytes.chunks_exact(4).enumerate() {
            if report.words >= MAX_SURVEY_WORDS {
                return Err(format!(
                    "instruction survey exceeds {MAX_SURVEY_WORDS}-word limit"
                ));
            }
            let pc = GuestAddr(
                mapping
                    .base
                    .0
                    .checked_add(index as u64 * 4)
                    .ok_or("survey address overflow")?,
            );
            let word = u32::from_le_bytes(bytes.try_into().map_err(|_| "survey opcode truncated")?);
            report.words += 1;
            match decode(word, pc) {
                Ok(instruction) => {
                    let family = match instruction.op {
                        Op::Nop => "hint-prefetch",
                        Op::Carry { .. } => "carry",
                        Op::Bitfield { .. } => "bitfield",
                        Op::Extract { .. } => "extract",
                        Op::Select { .. } => "conditional-select",
                        Op::CondCompare { .. } => "conditional-compare",
                        Op::Divide { .. } => "divide",
                        Op::ShiftVariable { .. } => "variable-shift",
                        Op::MultiplyAdd { .. }
                        | Op::MultiplyLong { .. }
                        | Op::MultiplyHigh { .. } => "multiply",
                        Op::Unary { .. } => "bit-byte-reverse-count",
                        Op::LoadLiteral { .. } => "literal-load",
                        Op::Exclusive { .. } => "exclusive-ordered-memory",
                        Op::Barrier { .. } | Op::ClearExclusive => "barrier",
                        Op::SystemRegister { .. } => "system-register",
                        Op::ZeroBlock { .. } | Op::CacheMaintenance => "cache-operation",
                        Op::Trap { .. } => "trap",
                        Op::VecMemory { .. } | Op::VecPair { .. } | Op::VecLiteral { .. } => {
                            "simd-fp-memory"
                        }
                        Op::VecStructures(_) => "simd-structure-memory",
                        Op::Fp(_) => "fp-scalar",
                        Op::Simd(_) => "simd",
                        Op::Extension(wl_aarch64::ExtensionOp::Counter(_)) => "system-counter",
                        Op::Extension(wl_aarch64::ExtensionOp::Crc(_)) => "crc32",
                        Op::Extension(wl_aarch64::ExtensionOp::Crypto(_)) => "crypto",
                        Op::Wide { .. } => "move-wide",
                        Op::Arithmetic { .. } => "arithmetic",
                        Op::Logical { .. } => "logical-shifted",
                        Op::Address { .. } => "pc-relative-address",
                        Op::Branch { .. } => "direct-branch",
                        Op::Conditional { .. } => "conditional-branch",
                        Op::CompareBranch { .. } => "compare-branch",
                        Op::TestBranch { .. } => "test-branch",
                        Op::Indirect { .. } => "indirect-branch",
                        Op::Memory { .. } => "scalar-memory",
                        Op::Pair { .. } => "pair-memory",
                        Op::Svc { .. } => "svc",
                    };
                    *report.families.entry(family).or_default() += 1;
                    report.supported += 1;
                }
                Err(_) => {
                    report.unsupported_words += 1;
                    *report
                        .unsupported_classes
                        .entry(wl_aarch64::encoding_class(word))
                        .or_default() += 1;
                    unknown
                        .entry(word)
                        .and_modify(|entry| entry.count += 1)
                        .or_insert(UnsupportedWord {
                            opcode: word,
                            count: 1,
                            first_pc: pc,
                        });
                }
            }
        }
    }
    report.unsupported_unique = unknown.len();
    let limit = examples;
    let mut examples: Vec<_> = unknown.into_values().collect();
    examples.sort_by_key(|entry| (std::cmp::Reverse(entry.count), entry.opcode));
    examples.truncate(limit);
    report.unsupported_examples = examples;
    Ok(report)
}
