//! Forward constant and bounds propagation over the decoded control-flow graph, used to find
//! code addresses that plain traversal cannot see:
//!
//! * **Jump tables.** `BR Xd` where `Xd = base + extend(table[i]) << s`, `base` and `table`
//!   are PC-relative constants (ADR/ADRP/ADD/MOV), and `i` is bounded by a dominating
//!   `CMP Wi, #n` with `B.HI`/`B.LS`/`B.HS`/`B.LO`. This covers the GCC and Clang lowerings
//!   of `switch` (byte, halfword and word entries, absolute- or table-relative bases). Every
//!   entry `0..=n` becomes a target and an edge of the branch.
//! * **Code pointers formed in code.** A constant that lies in executable memory and is
//!   stored to memory, passed in X0-X7 to a call, or used as a `BR`/`BLR` target.
//!
//! Function entries (roots and call targets) start with no known register; calls clobber
//! the AAPCS64 caller-saved registers X0-X18 and X30 and the flags, and preserve X19-X29
//! (the guest's own ABI contract). Results are candidates for speculative traversal, never
//! trusted semantics: each one is decoded like any other speculative root, and a resolved
//! table only adds control-flow edges and block leaders.
use super::{Block, Image};
use std::collections::{BTreeMap, BTreeSet, HashMap};
use wl_aarch64::{ExtensionOp, FpOp, Index, Op, Operand, Reg, SimdOp, Width};
use wl_common::GuestAddr;

/// Largest jump table accepted (entries).
pub const MAX_TABLE_ENTRIES: u64 = 65_536;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Val {
    Const(u64),
    /// Unsigned value at most `max`; `full` when the bound covers all 64 bits (not only the
    /// low 32 bits compared by a W-register comparison).
    Bounded {
        max: u64,
        full: bool,
    },
    /// `table[i]` for `i` in `0..count`, entries `bytes` wide at `table + (i << stride)`,
    /// sign-extended when `signed` (to 32 bits when `narrow`, else 64).
    Element(Table),
    /// `base + extend(element, extend) << shift`.
    Target {
        base: u64,
        table: Table,
        extend: u8,
        shift: u8,
    },
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Table {
    address: u64,
    count: u64,
    bytes: u8,
    stride: u8,
    signed: bool,
    narrow: bool,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Compare {
    reg: u8,
    imm: u64,
    width: Width,
}
#[derive(Clone, Debug, Default, PartialEq, Eq)]
struct State {
    regs: BTreeMap<u8, Val>,
    /// Registers whose upper 32 bits are known to be zero.
    narrow: u32,
    flags: Option<Compare>,
}
impl State {
    fn join(&mut self, other: &State) -> bool {
        let before = self.clone();
        self.regs.retain(|r, v| other.regs.get(r) == Some(v));
        self.narrow &= other.narrow;
        if self.flags != other.flags {
            self.flags = None;
        }
        *self != before
    }
    fn get(&self, reg: Reg) -> Option<Val> {
        match reg {
            Reg::Gpr(n) => self.regs.get(&n).copied(),
            Reg::Zero => Some(Val::Const(0)),
            Reg::Sp => None,
        }
    }
    fn set(&mut self, reg: Reg, value: Option<Val>, width: Width) {
        let Reg::Gpr(n) = reg else { return };
        match value {
            Some(v) => self.regs.insert(n, v),
            None => self.regs.remove(&n),
        };
        if width == Width::W32 {
            self.narrow |= 1 << n;
        } else if !matches!(value, Some(Val::Const(c)) if c >> 32 == 0) {
            self.narrow &= !(1 << n);
        } else {
            self.narrow |= 1 << n;
        }
        if self.flags.is_some_and(|f| f.reg == n) {
            self.flags = None;
        }
    }
    fn clobber_call(&mut self) {
        for n in (0..=18).chain([30]) {
            self.set(Reg::Gpr(n), None, Width::X64);
        }
        self.flags = None;
    }
}

fn mask(value: u64, width: Width) -> u64 {
    if width == Width::W32 {
        value & 0xffff_ffff
    } else {
        value
    }
}
fn sign_extend(value: u64, bits: u32) -> u64 {
    if bits >= 64 {
        value
    } else {
        (((value << (64 - bits)) as i64) >> (64 - bits)) as u64
    }
}
/// The A64 `ExtendReg` of an extended-register operand (UXTB..SXTX), before the shift.
fn extend(value: u64, option: u8) -> u64 {
    let bits = 8u32 << (option & 3);
    if option & 4 != 0 {
        sign_extend(value, bits)
    } else if bits == 64 {
        value
    } else {
        value & ((1u64 << bits) - 1)
    }
}

/// General registers written by `op` (with the width that determines their upper bits),
/// other than the link register of calls.
fn defs(op: &Op) -> Vec<(Reg, Width)> {
    let wb = |mode: u8, base: Reg| {
        if mode == 1 || mode == 3 {
            vec![(base, Width::X64)]
        } else {
            vec![]
        }
    };
    match *op {
        Op::Wide { width, dst, .. }
        | Op::Arithmetic { width, dst, .. }
        | Op::Carry { width, dst, .. }
        | Op::Logical { width, dst, .. }
        | Op::Bitfield { width, dst, .. }
        | Op::Extract { width, dst, .. }
        | Op::Select { width, dst, .. }
        | Op::Divide { width, dst, .. }
        | Op::ShiftVariable { width, dst, .. }
        | Op::MultiplyAdd { width, dst, .. }
        | Op::Unary { width, dst, .. } => vec![(dst, width)],
        Op::MultiplyLong { dst, .. } | Op::MultiplyHigh { dst, .. } | Op::Address { dst, .. } => {
            vec![(dst, Width::X64)]
        }
        Op::Memory {
            load,
            dst,
            base,
            mode,
            dst_width,
            ..
        } => {
            let mut d = wb(mode, base);
            if load {
                d.push((dst, dst_width));
            }
            d
        }
        Op::LoadLiteral { dst, .. } => vec![(dst, Width::X64)],
        Op::Pair {
            load,
            first,
            second,
            base,
            mode,
            bytes,
            ..
        } => {
            let mut d = wb(mode, base);
            if load {
                let w = if bytes == 4 { Width::W32 } else { Width::X64 };
                d.extend([(first, w), (second, w)]);
            }
            d
        }
        Op::Exclusive {
            load,
            exclusive,
            pair,
            status,
            first,
            second,
            ..
        } => {
            if load {
                if pair {
                    vec![(first, Width::X64), (second, Width::X64)]
                } else {
                    vec![(first, Width::X64)]
                }
            } else if exclusive {
                vec![(status, Width::W32)]
            } else {
                vec![]
            }
        }
        Op::SystemRegister {
            read: true, reg, ..
        } => vec![(reg, Width::X64)],
        Op::VecMemory { base, mode, .. } | Op::VecPair { base, mode, .. } => wb(mode, base),
        Op::VecStructures(s) => {
            if s.post.is_some() {
                vec![(s.base, Width::X64)]
            } else {
                vec![]
            }
        }
        Op::Fp(FpOp::ToInt { dst, width, .. }) | Op::Fp(FpOp::MoveToGeneral { dst, width, .. }) => {
            vec![(dst, width)]
        }
        Op::Simd(SimdOp::MoveToGeneral { dst, width, .. }) => vec![(dst, width)],
        Op::Extension(ExtensionOp::Counter(c)) => vec![(c.reg, Width::X64)],
        Op::Extension(ExtensionOp::Crc(c)) => vec![(c.dst, Width::W32)],
        _ => vec![],
    }
}
/// Instructions known to leave NZCV unchanged; any other instruction drops a tracked compare.
fn preserves_flags(op: &Op) -> bool {
    match *op {
        Op::Arithmetic { flags, .. } | Op::Carry { flags, .. } => !flags,
        Op::Logical { kind, .. } => kind != 3,
        Op::Nop
        | Op::Wide { .. }
        | Op::Bitfield { .. }
        | Op::Extract { .. }
        | Op::Select { .. }
        | Op::Divide { .. }
        | Op::ShiftVariable { .. }
        | Op::MultiplyAdd { .. }
        | Op::MultiplyLong { .. }
        | Op::MultiplyHigh { .. }
        | Op::Unary { .. }
        | Op::Address { .. }
        | Op::Memory { .. }
        | Op::LoadLiteral { .. }
        | Op::Pair { .. }
        | Op::VecMemory { .. }
        | Op::VecPair { .. }
        | Op::VecLiteral { .. }
        | Op::Barrier { .. }
        | Op::Branch { .. }
        | Op::Conditional { .. }
        | Op::CompareBranch { .. }
        | Op::TestBranch { .. }
        | Op::Indirect { .. }
        | Op::Extension(_) => true,
        _ => false,
    }
}

/// The value of `rhs` added to a constant in a jump-table address computation.
fn table_offset(s: &State, rhs: Operand) -> Option<(Table, u8, u8)> {
    match rhs {
        Operand::Extended { reg, extend, shift } => match s.get(reg)? {
            Val::Element(t) => Some((t, extend, shift)),
            _ => None,
        },
        Operand::Shifted {
            reg,
            kind: 0,
            amount,
        } => match s.get(reg)? {
            Val::Element(t) => Some((t, 3, amount)),
            _ => None,
        },
        _ => None,
    }
}
/// A bounded index register usable as a table index under `index.extend`.
fn bounded_index(s: &State, index: Index) -> Option<u64> {
    let Val::Bounded { max, full } = s.get(index.reg)? else {
        return None;
    };
    let ok = match index.extend {
        2 => max <= u64::from(u32::MAX),
        6 => max <= i32::MAX as u64,
        3 | 7 => full,
        _ => false,
    };
    (ok && max < MAX_TABLE_ENTRIES).then_some(max)
}

fn transfer(state: &mut State, op: &Op) {
    let flags_kept = preserves_flags(op);
    let mut result: Option<(Reg, Option<Val>, Width)> = None;
    let mut compare = None;
    match *op {
        Op::Address { dst, value } => result = Some((dst, Some(Val::Const(value.0)), Width::X64)),
        Op::Wide {
            width,
            dst,
            imm,
            shift,
            keep,
            invert,
        } => {
            let value = if keep {
                match state.get(dst) {
                    Some(Val::Const(old)) => {
                        Some(Val::Const(mask((old & !(0xffff << shift)) | imm, width)))
                    }
                    _ => None,
                }
            } else {
                Some(Val::Const(mask(if invert { !imm } else { imm }, width)))
            };
            result = Some((dst, value, width));
        }
        Op::Arithmetic {
            width,
            dst,
            lhs,
            rhs,
            sub,
            flags,
        } => {
            let value = match (state.get(lhs), rhs) {
                (Some(Val::Const(a)), Operand::Imm(i)) => Some(Val::Const(mask(
                    if sub {
                        a.wrapping_sub(i)
                    } else {
                        a.wrapping_add(i)
                    },
                    width,
                ))),
                (Some(Val::Const(base)), rhs) if !sub && width == Width::X64 => {
                    table_offset(state, rhs).map(|(table, extend, shift)| Val::Target {
                        base,
                        table,
                        extend,
                        shift,
                    })
                }
                (
                    Some(Val::Element(table)),
                    Operand::Shifted {
                        reg,
                        kind: 0,
                        amount: 0,
                    },
                ) if !sub && width == Width::X64 => match state.get(reg) {
                    Some(Val::Const(base)) => Some(Val::Target {
                        base,
                        table,
                        extend: 3,
                        shift: 0,
                    }),
                    _ => None,
                },
                _ => None,
            };
            if flags && sub && dst == Reg::Zero {
                if let (Reg::Gpr(reg), Operand::Imm(imm)) = (lhs, rhs) {
                    compare = Some(Compare { reg, imm, width });
                }
            }
            result = Some((dst, value, width));
        }
        Op::Logical {
            width,
            dst,
            lhs,
            rhs: Operand::Imm(imm),
            kind: 0 | 3,
            invert: false,
        } => {
            // AND/ANDS cannot produce an unsigned value larger than their
            // immediate mask, including when the source register is unknown.
            // This also bounds alignment-dispatch tables without a CMP branch.
            let value = match state.get(lhs) {
                Some(Val::Const(c)) => Val::Const(mask(c & imm, width)),
                _ => Val::Bounded {
                    max: mask(imm, width),
                    full: true,
                },
            };
            result = Some((dst, Some(value), width));
        }
        Op::Logical {
            width,
            dst,
            lhs: Reg::Zero,
            rhs:
                Operand::Shifted {
                    reg,
                    kind: 0,
                    amount: 0,
                },
            kind: 1,
            invert: false,
        } => {
            let value = match state.get(reg) {
                Some(Val::Const(c)) => Some(Val::Const(mask(c, width))),
                v if width == Width::X64 => v,
                _ => None,
            };
            result = Some((dst, value, width));
        }
        Op::Memory {
            load: true,
            bytes,
            dst,
            base,
            offset: 0,
            mode: 0,
            signed,
            dst_width,
            index: Some(index),
        } => {
            let value = match (state.get(base), bounded_index(state, index)) {
                (Some(Val::Const(address)), Some(max))
                    if index.shift == 0 || 1u8 << index.shift == bytes =>
                {
                    Some(Val::Element(Table {
                        address,
                        count: max + 1,
                        bytes,
                        stride: index.shift,
                        signed,
                        narrow: dst_width == Width::W32,
                    }))
                }
                _ => None,
            };
            result = Some((dst, value, dst_width));
        }
        _ => {}
    }
    if let Some((reg, value, width)) = result {
        state.set(reg, value, width);
    } else {
        for (reg, width) in defs(op) {
            state.set(reg, None, width);
        }
    }
    if let Some(c) = compare {
        state.flags = Some(c);
    } else if !flags_kept {
        state.flags = None;
    }
}

/// Refines the compared register on a conditional edge: `taken` selects the branch edge.
fn refine(state: &mut State, condition: u8, taken: bool) {
    let Some(Compare { reg, imm, width }) = state.flags else {
        return;
    };
    // HI 8 / LS 9 compare `reg > imm`; HS 2 / LO 3 compare `reg >= imm` (unsigned).
    let max = match (condition, taken) {
        (8, false) | (9, true) => Some(imm),
        (2, false) | (3, true) => imm.checked_sub(1),
        _ => None,
    };
    let Some(max) = max else { return };
    if matches!(state.regs.get(&reg), Some(Val::Const(_))) {
        return;
    }
    let full = width == Width::X64 || state.narrow & (1 << reg) != 0;
    let narrow = state.narrow;
    state.regs.insert(reg, Val::Bounded { max, full });
    state.narrow = narrow;
}

/// Results of one propagation pass.
#[derive(Debug, Default)]
pub struct Found {
    /// Jump-table edges by branch address.
    pub tables: BTreeMap<GuestAddr, Vec<GuestAddr>>,
    /// Code pointers formed by constants (stored, passed to calls, or branched to).
    pub pointers: BTreeSet<GuestAddr>,
}

fn table_targets(image: &Image, base: u64, t: Table, ext: u8, shift: u8) -> Option<Vec<GuestAddr>> {
    let mut out = Vec::with_capacity(t.count as usize);
    for i in 0..t.count {
        let raw = image.read_data(t.address.checked_add(i << t.stride)?, t.bytes)?;
        let mut v = if t.signed {
            sign_extend(raw, u32::from(t.bytes) * 8)
        } else {
            raw
        };
        if t.narrow {
            v &= 0xffff_ffff;
        }
        let target = base.wrapping_add(extend(v, ext) << shift);
        if !image.is_code(target) {
            return None;
        }
        out.push(GuestAddr(target));
    }
    Some(out)
}

/// Propagates over `blocks` from `entries` (roots and call targets start with no knowledge),
/// following direct edges and the previously resolved `tables`.
pub fn propagate(
    image: &Image,
    blocks: &[Block],
    entries: &BTreeSet<GuestAddr>,
    tables: &BTreeMap<GuestAddr, Vec<GuestAddr>>,
) -> Found {
    let index: HashMap<u64, usize> = blocks
        .iter()
        .enumerate()
        .map(|(i, b)| (b.start.0, i))
        .collect();
    let mut calls: BTreeSet<GuestAddr> = entries.clone();
    let mut has_pred = vec![false; blocks.len()];
    for b in blocks {
        for (target, _) in edges(b, tables) {
            if let Some(&j) = index.get(&target.0) {
                has_pred[j] = true;
            }
        }
        if let Some(Op::Branch { target, link: true }) = b.instructions.last().map(|i| i.op) {
            calls.insert(target);
        }
    }
    let mut input: Vec<Option<State>> = vec![None; blocks.len()];
    let mut work = Vec::new();
    for (i, b) in blocks.iter().enumerate() {
        if calls.contains(&b.start) || !has_pred[i] {
            input[i] = Some(State::default());
            work.push(i);
        }
    }
    let pinned: Vec<bool> = blocks.iter().map(|b| calls.contains(&b.start)).collect();
    while let Some(i) = work.pop() {
        let Some(mut state) = input[i].clone() else {
            continue;
        };
        let b = &blocks[i];
        for instruction in &b.instructions {
            transfer(&mut state, &instruction.op);
        }
        if b.unsupported.is_some() {
            continue;
        }
        for (target, kind) in edges(b, tables) {
            let Some(&j) = index.get(&target.0) else {
                continue;
            };
            if pinned[j] {
                continue;
            }
            let mut out = state.clone();
            match kind {
                Edge::Call => out.clobber_call(),
                Edge::Svc => {
                    for n in 0..=7 {
                        out.set(Reg::Gpr(n), None, Width::X64);
                    }
                    out.flags = None;
                }
                Edge::Taken(c) => refine(&mut out, c, true),
                Edge::Fallthrough(c) => refine(&mut out, c, false),
                Edge::Plain => {}
            }
            let changed = match &mut input[j] {
                Some(existing) => existing.join(&out),
                slot => {
                    *slot = Some(out);
                    true
                }
            };
            if changed {
                work.push(j);
            }
        }
    }
    // Resolution against the fixpoint states.
    let mut found = Found::default();
    for (i, b) in blocks.iter().enumerate() {
        let Some(mut state) = input[i].clone() else {
            continue;
        };
        for instruction in &b.instructions {
            let code = |v: Option<Val>| match v {
                Some(Val::Const(c)) if image.is_code(c) => Some(GuestAddr(c)),
                _ => None,
            };
            match instruction.op {
                Op::Memory {
                    load: false, dst, ..
                } => found.pointers.extend(code(state.get(dst))),
                Op::Pair {
                    load: false,
                    first,
                    second,
                    ..
                } => {
                    found.pointers.extend(code(state.get(first)));
                    found.pointers.extend(code(state.get(second)));
                }
                Op::Branch { link: true, .. } => {
                    for n in 0..8 {
                        found.pointers.extend(code(state.get(Reg::Gpr(n))));
                    }
                }
                Op::Indirect { src, link } => {
                    found.pointers.extend(code(state.get(src)));
                    if link {
                        for n in 0..8 {
                            found.pointers.extend(code(state.get(Reg::Gpr(n))));
                        }
                    } else if let Some(Val::Target {
                        base,
                        table,
                        extend,
                        shift,
                    }) = state.get(src)
                    {
                        if let Some(targets) = table_targets(image, base, table, extend, shift) {
                            found.tables.insert(instruction.pc, targets);
                        }
                    }
                }
                _ => {}
            }
            transfer(&mut state, &instruction.op);
        }
    }
    found
}

#[derive(Clone, Copy)]
enum Edge {
    Plain,
    Call,
    Svc,
    Taken(u8),
    Fallthrough(u8),
}
fn edges(b: &Block, tables: &BTreeMap<GuestAddr, Vec<GuestAddr>>) -> Vec<(GuestAddr, Edge)> {
    let Some(last) = b.instructions.last() else {
        return vec![];
    };
    let next = GuestAddr(last.pc.0.wrapping_add(4));
    match last.op {
        Op::Branch {
            target,
            link: false,
        } => vec![(target, Edge::Plain)],
        Op::Branch { link: true, .. } | Op::Indirect { link: true, .. } => vec![(next, Edge::Call)],
        Op::Conditional { target, condition } => {
            vec![
                (target, Edge::Taken(condition)),
                (next, Edge::Fallthrough(condition)),
            ]
        }
        Op::CompareBranch { target, .. } | Op::TestBranch { target, .. } => {
            vec![(target, Edge::Plain), (next, Edge::Plain)]
        }
        Op::Indirect { link: false, .. } => tables
            .get(&last.pc)
            .map(|t| t.iter().map(|&a| (a, Edge::Plain)).collect())
            .unwrap_or_default(),
        Op::Svc { id: 7 } | Op::Trap { .. } => vec![],
        Op::Svc { .. } => vec![(next, Edge::Svc)],
        _ => vec![(next, Edge::Plain)],
    }
}
