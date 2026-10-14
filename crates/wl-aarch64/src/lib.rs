//! A strict semantic decoder for the ARMv8.0 A64 integer instruction set used by AArch64
//! application code. Encodings outside the implemented set, reserved encodings and
//! constrained-unpredictable forms are rejected rather than given a host-dependent meaning.
pub mod extensions;
pub mod simd;
pub use extensions::{
    CounterRegister, Crc32Op, CryptoOp, ExtensionOp, SystemCounterOp, decode_crc32, decode_crypto,
    decode_extension, decode_system_counter,
};
use serde::Serialize;
pub use simd::{FpBinary, FpOp, FpUnary, PostIndex, Shape, SimdOp, Structures};
use wl_common::{GuestAddr, Result};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
pub enum Width {
    W32,
    X64,
}
impl Width {
    pub fn bits(self) -> u8 {
        if self == Self::W32 { 32 } else { 64 }
    }
}
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
pub enum Reg {
    Gpr(u8),
    Sp,
    Zero,
}
fn reg(n: u8, sp: bool) -> Reg {
    if n == 31 {
        if sp { Reg::Sp } else { Reg::Zero }
    } else {
        Reg::Gpr(n)
    }
}
#[derive(Clone, Copy, Debug, Serialize)]
pub enum Operand {
    Imm(u64),
    Shifted {
        reg: Reg,
        kind: u8,
        amount: u8,
    },
    /// Extended register: `extend` is the 3-bit option (UXTB..SXTX), then shifted left.
    Extended {
        reg: Reg,
        extend: u8,
        shift: u8,
    },
}
/// Register offset of a load/store: `reg` extended by `extend` (option) and shifted.
#[derive(Clone, Copy, Debug, Serialize)]
pub struct Index {
    pub reg: Reg,
    pub extend: u8,
    pub shift: u8,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
pub enum SystemRegister {
    TpidrEl0,
    TpidrroEl0,
    Nzcv,
    Fpcr,
    Fpsr,
    CtrEl0,
    DczidEl0,
}
#[derive(Clone, Copy, Debug, Serialize)]
pub enum Op {
    /// NOP, other hints (executed as NOP by ARMv8.0 hardware) and prefetches.
    Nop,
    Wide {
        width: Width,
        dst: Reg,
        imm: u64,
        shift: u8,
        keep: bool,
        invert: bool,
    },
    Arithmetic {
        width: Width,
        dst: Reg,
        lhs: Reg,
        rhs: Operand,
        sub: bool,
        flags: bool,
    },
    /// ADC/SBC/ADCS/SBCS.
    Carry {
        width: Width,
        dst: Reg,
        lhs: Reg,
        rhs: Reg,
        sub: bool,
        flags: bool,
    },
    Logical {
        width: Width,
        dst: Reg,
        lhs: Reg,
        rhs: Operand,
        kind: u8,
        invert: bool,
    },
    /// SBFM (kind 0), BFM (1), UBFM (2). Masks are DecodeBitMasks(N, imms, immr) results.
    Bitfield {
        width: Width,
        dst: Reg,
        src: Reg,
        kind: u8,
        rotate: u8,
        sign_bit: u8,
        wmask: u64,
        tmask: u64,
    },
    /// EXTR: bits [lsb, lsb + width) of the concatenation high:low.
    Extract {
        width: Width,
        dst: Reg,
        high: Reg,
        low: Reg,
        lsb: u8,
    },
    /// CSEL/CSINC/CSINV/CSNEG: dst = cond ? lhs : f(rhs).
    Select {
        width: Width,
        dst: Reg,
        lhs: Reg,
        rhs: Reg,
        condition: u8,
        invert: bool,
        increment: bool,
    },
    /// CCMP/CCMN: flags of lhs -/+ rhs when the condition holds, else `nzcv`.
    CondCompare {
        width: Width,
        lhs: Reg,
        rhs: Operand,
        condition: u8,
        nzcv: u8,
        negate: bool,
    },
    Divide {
        width: Width,
        dst: Reg,
        lhs: Reg,
        rhs: Reg,
        signed: bool,
    },
    /// LSLV/LSRV/ASRV/RORV (kind as in shifted operands).
    ShiftVariable {
        width: Width,
        dst: Reg,
        src: Reg,
        amount: Reg,
        kind: u8,
    },
    /// MADD/MSUB: dst = acc +/- lhs * rhs.
    MultiplyAdd {
        width: Width,
        dst: Reg,
        lhs: Reg,
        rhs: Reg,
        acc: Reg,
        sub: bool,
    },
    /// SMADDL/SMSUBL/UMADDL/UMSUBL: 32x32 -> 64-bit product accumulated into X.
    MultiplyLong {
        dst: Reg,
        lhs: Reg,
        rhs: Reg,
        acc: Reg,
        sub: bool,
        signed: bool,
    },
    /// SMULH/UMULH: upper 64 bits of the 128-bit product.
    MultiplyHigh {
        dst: Reg,
        lhs: Reg,
        rhs: Reg,
        signed: bool,
    },
    /// RBIT (0), REV16 (1), REV32 (2), REV (3), CLZ (4), CLS (5).
    Unary {
        width: Width,
        dst: Reg,
        src: Reg,
        kind: u8,
    },
    Address {
        dst: Reg,
        value: GuestAddr,
    },
    Branch {
        target: GuestAddr,
        link: bool,
    },
    Conditional {
        target: GuestAddr,
        condition: u8,
    },
    CompareBranch {
        width: Width,
        src: Reg,
        target: GuestAddr,
        nonzero: bool,
    },
    TestBranch {
        src: Reg,
        bit: u8,
        target: GuestAddr,
        nonzero: bool,
    },
    Indirect {
        src: Reg,
        link: bool,
    },
    /// mode: 0 offset, 1 post-index, 3 pre-index (register offsets use mode 0).
    Memory {
        load: bool,
        bytes: u8,
        dst: Reg,
        base: Reg,
        offset: i64,
        mode: u8,
        signed: bool,
        dst_width: Width,
        index: Option<Index>,
    },
    LoadLiteral {
        dst: Reg,
        address: GuestAddr,
        bytes: u8,
        signed: bool,
    },
    /// mode: 1 post-index, 2 offset (including non-temporal), 3 pre-index.
    Pair {
        load: bool,
        bytes: u8,
        first: Reg,
        second: Reg,
        base: Reg,
        offset: i64,
        mode: u8,
        signed: bool,
    },
    /// LDXR/STXR/LDAXR/STLXR (exclusive), LDAR/STLR (ordered), and pair forms.
    Exclusive {
        load: bool,
        exclusive: bool,
        ordered: bool,
        pair: bool,
        bytes: u8,
        status: Reg,
        first: Reg,
        second: Reg,
        base: Reg,
    },
    /// DSB (4), DMB (5), ISB (6).
    Barrier {
        kind: u8,
    },
    ClearExclusive,
    SystemRegister {
        read: bool,
        register: SystemRegister,
        reg: Reg,
    },
    /// DC ZVA: zero the naturally aligned block containing the address.
    ZeroBlock {
        address: Reg,
    },
    /// Data/instruction cache maintenance by address; no guest-visible effect in this model.
    CacheMaintenance,
    /// BRK (0) or HLT (1): a synchronous exception the runtime reports.
    Trap {
        kind: u8,
        imm: u16,
    },
    Svc {
        id: u16,
    },
    /// SIMD&FP register load/store of 1, 2, 4, 8 or 16 bytes (B/H/S/D/Q). A load zeroes the
    /// register bits above the loaded size.
    VecMemory {
        load: bool,
        bytes: u8,
        vt: u8,
        base: Reg,
        offset: i64,
        mode: u8,
        index: Option<Index>,
    },
    /// LDP/STP/LDNP/STNP of S, D or Q registers (mode as for `Pair`).
    VecPair {
        load: bool,
        bytes: u8,
        first: u8,
        second: u8,
        base: Reg,
        offset: i64,
        mode: u8,
    },
    VecLiteral {
        vt: u8,
        address: GuestAddr,
        bytes: u8,
    },
    Fp(FpOp),
    Simd(SimdOp),
    VecStructures(Structures),
    /// System counter reads, CRC32/CRC32C and the recognised crypto-extension forms
    /// (`extensions`).
    Extension(ExtensionOp),
}
#[derive(Clone, Debug, Serialize)]
pub struct Instruction {
    pub pc: GuestAddr,
    pub word: u32,
    pub op: Op,
}
fn signed(value: u32, bits: u8) -> i64 {
    ((value as i64) << (64 - bits)) >> (64 - bits)
}
fn target(pc: GuestAddr, delta: i64) -> Result<GuestAddr> {
    Ok(GuestAddr(pc.0.checked_add_signed(delta).ok_or_else(
        || format!("PC-relative target overflow at {:#x}", pc.0),
    )?))
}
fn replicate(element: u64, esize: u32) -> u64 {
    let mut value = element;
    let mut size = esize;
    while size < 64 {
        value |= value << size;
        size *= 2;
    }
    value
}
fn ones(n: u32) -> u64 {
    if n >= 64 { u64::MAX } else { (1u64 << n) - 1 }
}
/// Arm ARM `DecodeBitMasks(immN, imms, immr, immediate)` for a 64-bit register; the caller
/// masks to 32 bits for W operations. Returns `(wmask, tmask)`, or `None` if reserved.
pub fn decode_bit_masks(n: u32, imms: u32, immr: u32, immediate: bool) -> Option<(u64, u64)> {
    let combined = (n << 6) | (!imms & 0x3f);
    if combined == 0 {
        return None;
    }
    let len = 31 - combined.leading_zeros();
    if len < 1 {
        return None;
    }
    let levels = ones(len) as u32;
    if immediate && imms & levels == levels {
        return None;
    }
    let s = imms & levels;
    let r = immr & levels;
    let d = s.wrapping_sub(r) & levels;
    let esize = 1u32 << len;
    let welem = ones(s + 1);
    let telem = ones(d + 1);
    let emask = ones(esize);
    let rotated = if r == 0 {
        welem
    } else {
        ((welem >> r) | (welem << (esize - r))) & emask
    };
    Some((replicate(rotated, esize), replicate(telem, esize)))
}

pub fn decode(word: u32, pc: GuestAddr) -> Result<Instruction> {
    if pc.0 % 4 != 0 {
        return Err(format!("unaligned instruction address {:#x}", pc.0));
    }
    // The extension decoders match exact encodings that the base decoder rejects.
    if let Some(op) = decode_extension(word, pc)? {
        return Ok(Instruction {
            pc,
            word,
            op: Op::Extension(op),
        });
    }
    let width = if word >> 31 == 0 {
        Width::W32
    } else {
        Width::X64
    };
    let rd = (word & 31) as u8;
    let rn = ((word >> 5) & 31) as u8;
    let rm = ((word >> 16) & 31) as u8;
    let op = if word & 0xfffff01f == 0xd503201f {
        // Hint space: NOP, YIELD, WFE, WFI, SEV, SEVL and allocated-later hints all
        // execute as NOP on ARMv8.0 hardware.
        Op::Nop
    } else if word & 0xfffff01f == 0xd503301f {
        match (word >> 5) & 7 {
            2 => Op::ClearExclusive,
            4..=6 => Op::Barrier {
                kind: ((word >> 5) & 7) as u8,
            },
            _ => return unsupported(word, pc),
        }
    } else if word & 0xffd00000 == 0xd5100000 {
        let read = word & (1 << 21) != 0;
        let register = match (word >> 5) & 0x7fff {
            0x5e82 => SystemRegister::TpidrEl0,
            0x5e83 => SystemRegister::TpidrroEl0,
            0x5a10 => SystemRegister::Nzcv,
            0x5a20 => SystemRegister::Fpcr,
            0x5a21 => SystemRegister::Fpsr,
            0x5801 => SystemRegister::CtrEl0,
            0x5807 => SystemRegister::DczidEl0,
            _ => return unsupported(word, pc),
        };
        // EL0 may not write TPIDRRO_EL0 or the identification registers.
        if !read
            && matches!(
                register,
                SystemRegister::TpidrroEl0 | SystemRegister::CtrEl0 | SystemRegister::DczidEl0
            )
        {
            return unsupported(word, pc);
        }
        Op::SystemRegister {
            read,
            register,
            reg: reg(rd, false),
        }
    } else if word & 0xfff80000 == 0xd5080000 {
        // SYS: only EL0-accessible data/instruction cache operations by VA.
        match (word >> 5) & 0x3fff {
            0x1ba1 => Op::ZeroBlock {
                address: reg(rd, false),
            },
            0x1bd1 | 0x1bd9 | 0x1bf1 | 0x1ba9 => Op::CacheMaintenance,
            _ => return unsupported(word, pc),
        }
    } else if word & 0xffe0001f == 0xd4200000 || word & 0xffe0001f == 0xd4400000 {
        Op::Trap {
            kind: ((word >> 22) & 1) as u8,
            imm: ((word >> 5) & 0xffff) as u16,
        }
    } else if word & 0x1f800000 == 0x12800000 {
        let opc = (word >> 29) & 3;
        let half = ((word >> 21) & 3) as u8;
        if opc == 1 || (width == Width::W32 && half > 1) {
            return unsupported(word, pc);
        }
        let shift = half * 16;
        let imm = u64::from((word >> 5) & 0xffff) << shift;
        Op::Wide {
            width,
            dst: reg(rd, false),
            imm,
            shift,
            keep: opc == 3,
            invert: opc == 0,
        }
    } else if word & 0x1f800000 == 0x11000000 {
        let flags = word & (1 << 29) != 0;
        Op::Arithmetic {
            width,
            dst: reg(rd, !flags),
            lhs: reg(rn, true),
            rhs: Operand::Imm(
                u64::from((word >> 10) & 0xfff) << if word & (1 << 22) != 0 { 12 } else { 0 },
            ),
            sub: word & (1 << 30) != 0,
            flags,
        }
    } else if word & 0x1f800000 == 0x12000000 {
        // Logical (immediate).
        let n = (word >> 22) & 1;
        if width == Width::W32 && n != 0 {
            return unsupported(word, pc);
        }
        let Some((mask, _)) = decode_bit_masks(n, (word >> 10) & 63, (word >> 16) & 63, true)
        else {
            return unsupported(word, pc);
        };
        let kind = ((word >> 29) & 3) as u8;
        Op::Logical {
            width,
            dst: reg(rd, kind != 3),
            lhs: reg(rn, false),
            rhs: Operand::Imm(if width == Width::W32 {
                mask & 0xffff_ffff
            } else {
                mask
            }),
            kind,
            invert: false,
        }
    } else if word & 0x1f800000 == 0x13000000 {
        // Bitfield.
        let kind = ((word >> 29) & 3) as u8;
        let n = (word >> 22) & 1;
        let immr = (word >> 16) & 63;
        let imms = (word >> 10) & 63;
        if kind == 3 || n != (word >> 31) || (width == Width::W32 && (immr | imms) & 32 != 0) {
            return unsupported(word, pc);
        }
        let Some((wmask, tmask)) = decode_bit_masks(n, imms, immr, false) else {
            return unsupported(word, pc);
        };
        let m = if width == Width::W32 {
            0xffff_ffff
        } else {
            u64::MAX
        };
        Op::Bitfield {
            width,
            dst: reg(rd, false),
            src: reg(rn, false),
            kind,
            rotate: immr as u8,
            sign_bit: imms as u8,
            wmask: wmask & m,
            tmask: tmask & m,
        }
    } else if word & 0x1f800000 == 0x13800000 {
        // Extract: only EXTR (op21 = 00, o0 = 0) is allocated.
        let n = (word >> 22) & 1;
        let imms = ((word >> 10) & 63) as u8;
        if word & 0x60200000 != 0 || n != (word >> 31) || (width == Width::W32 && imms >= 32) {
            return unsupported(word, pc);
        }
        Op::Extract {
            width,
            dst: reg(rd, false),
            high: reg(rn, false),
            low: reg(rm, false),
            lsb: imms,
        }
    } else if word & 0x1f200000 == 0x0b000000 {
        let kind = ((word >> 22) & 3) as u8;
        let amount = ((word >> 10) & 63) as u8;
        if kind == 3 || amount >= width.bits() {
            return unsupported(word, pc);
        }
        Op::Arithmetic {
            width,
            dst: reg(rd, false),
            lhs: reg(rn, false),
            rhs: Operand::Shifted {
                reg: reg(rm, false),
                kind,
                amount,
            },
            sub: word & (1 << 30) != 0,
            flags: word & (1 << 29) != 0,
        }
    } else if word & 0x1f200000 == 0x0b200000 {
        // Add/subtract (extended register).
        let shift = ((word >> 10) & 7) as u8;
        if (word >> 22) & 3 != 0 || shift > 4 {
            return unsupported(word, pc);
        }
        let flags = word & (1 << 29) != 0;
        Op::Arithmetic {
            width,
            dst: reg(rd, !flags),
            lhs: reg(rn, true),
            rhs: Operand::Extended {
                reg: reg(rm, false),
                extend: ((word >> 13) & 7) as u8,
                shift,
            },
            sub: word & (1 << 30) != 0,
            flags,
        }
    } else if word & 0x1f000000 == 0x0a000000 {
        let amount = ((word >> 10) & 63) as u8;
        if amount >= width.bits() {
            return unsupported(word, pc);
        }
        Op::Logical {
            width,
            dst: reg(rd, false),
            lhs: reg(rn, false),
            rhs: Operand::Shifted {
                reg: reg(rm, false),
                kind: ((word >> 22) & 3) as u8,
                amount,
            },
            kind: ((word >> 29) & 3) as u8,
            invert: word & (1 << 21) != 0,
        }
    } else if word & 0x1fe0fc00 == 0x1a000000 {
        Op::Carry {
            width,
            dst: reg(rd, false),
            lhs: reg(rn, false),
            rhs: reg(rm, false),
            sub: word & (1 << 30) != 0,
            flags: word & (1 << 29) != 0,
        }
    } else if word & 0x1fe00000 == 0x1a400000 {
        // Conditional compare: S must be 1, o2 and o3 must be 0.
        if word & (1 << 29) == 0 || word & (1 << 10) != 0 || word & (1 << 4) != 0 {
            return unsupported(word, pc);
        }
        Op::CondCompare {
            width,
            lhs: reg(rn, false),
            rhs: if word & (1 << 11) != 0 {
                Operand::Imm(u64::from(rm))
            } else {
                Operand::Shifted {
                    reg: reg(rm, false),
                    kind: 0,
                    amount: 0,
                }
            },
            condition: ((word >> 12) & 15) as u8,
            nzcv: (word & 15) as u8,
            negate: word & (1 << 30) == 0,
        }
    } else if word & 0x1fe00000 == 0x1a800000 {
        if word & (1 << 29) != 0 || word & (1 << 11) != 0 {
            return unsupported(word, pc);
        }
        Op::Select {
            width,
            dst: reg(rd, false),
            lhs: reg(rn, false),
            rhs: reg(rm, false),
            condition: ((word >> 12) & 15) as u8,
            invert: word & (1 << 30) != 0,
            increment: word & (1 << 10) != 0,
        }
    } else if word & 0x7fe00000 == 0x1ac00000 {
        // Data-processing (2 source), S = 0.
        let opcode = (word >> 10) & 63;
        match opcode {
            2 | 3 => Op::Divide {
                width,
                dst: reg(rd, false),
                lhs: reg(rn, false),
                rhs: reg(rm, false),
                signed: opcode == 3,
            },
            8..=11 => Op::ShiftVariable {
                width,
                dst: reg(rd, false),
                src: reg(rn, false),
                amount: reg(rm, false),
                kind: (opcode - 8) as u8,
            },
            _ => return unsupported(word, pc),
        }
    } else if word & 0x7fff0000 == 0x5ac00000 {
        // Data-processing (1 source), S = 0, opcode2 = 0.
        let opcode = ((word >> 10) & 63) as u8;
        let kind = match (width, opcode) {
            (_, 0) => 0,
            (_, 1) => 1,
            (Width::W32, 2) => 3,
            (Width::X64, 2) => 2,
            (Width::X64, 3) => 3,
            (_, 4) => 4,
            (_, 5) => 5,
            _ => return unsupported(word, pc),
        };
        Op::Unary {
            width,
            dst: reg(rd, false),
            src: reg(rn, false),
            kind,
        }
    } else if word & 0x1f000000 == 0x1b000000 {
        // Data-processing (3 source); op54 must be 0.
        let op31 = (word >> 21) & 7;
        let sub = word & (1 << 15) != 0;
        let ra = ((word >> 10) & 31) as u8;
        if word & 0x60000000 != 0 || (width == Width::W32 && op31 != 0) {
            return unsupported(word, pc);
        }
        match op31 {
            0 => Op::MultiplyAdd {
                width,
                dst: reg(rd, false),
                lhs: reg(rn, false),
                rhs: reg(rm, false),
                acc: reg(ra, false),
                sub,
            },
            1 | 5 => Op::MultiplyLong {
                dst: reg(rd, false),
                lhs: reg(rn, false),
                rhs: reg(rm, false),
                acc: reg(ra, false),
                sub,
                signed: op31 == 1,
            },
            2 | 6 if !sub => Op::MultiplyHigh {
                dst: reg(rd, false),
                lhs: reg(rn, false),
                rhs: reg(rm, false),
                signed: op31 == 2,
            },
            _ => return unsupported(word, pc),
        }
    } else if word & 0x1f000000 == 0x10000000 {
        let imm = signed(((word >> 5) & 0x7ffff) << 2 | ((word >> 29) & 3), 21);
        let value = if word & (1 << 31) != 0 {
            target(GuestAddr(pc.0 & !0xfff), imm << 12)?
        } else {
            target(pc, imm)?
        };
        Op::Address {
            dst: reg(rd, false),
            value,
        }
    } else if word & 0x7c000000 == 0x14000000 {
        Op::Branch {
            target: target(pc, signed(word & 0x3ffffff, 26) << 2)?,
            link: word & (1 << 31) != 0,
        }
    } else if word & 0xff000010 == 0x54000000 {
        Op::Conditional {
            target: target(pc, signed((word >> 5) & 0x7ffff, 19) << 2)?,
            condition: (word & 15) as u8,
        }
    } else if word & 0x7e000000 == 0x34000000 {
        Op::CompareBranch {
            width,
            src: reg(rd, false),
            target: target(pc, signed((word >> 5) & 0x7ffff, 19) << 2)?,
            nonzero: word & (1 << 24) != 0,
        }
    } else if word & 0x7e000000 == 0x36000000 {
        Op::TestBranch {
            src: reg(rd, false),
            bit: (((word >> 19) & 31) | ((word >> 26) & 32)) as u8,
            target: target(pc, signed((word >> 5) & 0x3fff, 14) << 2)?,
            nonzero: word & (1 << 24) != 0,
        }
    } else if [0xd61f0000, 0xd63f0000, 0xd65f0000].contains(&(word & 0xfffffc1f)) {
        Op::Indirect {
            src: reg(rn, false),
            link: word & 0xfffffc1f == 0xd63f0000,
        }
    } else if word & 0x3b000000 == 0x18000000 && word & (1 << 26) == 0 {
        // Load register (literal).
        let address = target(pc, signed((word >> 5) & 0x7ffff, 19) << 2)?;
        match word >> 30 {
            3 => Op::Nop,
            // opc 00: LDR Wt (4 bytes); 01: LDR Xt (8 bytes); 10: LDRSW Xt (4 bytes).
            opc => Op::LoadLiteral {
                dst: reg(rd, false),
                address,
                bytes: if opc == 1 { 8 } else { 4 },
                signed: opc == 2,
            },
        }
    } else if word & 0x3f000000 == 0x39000000
        || (word & 0x3f200000 == 0x38000000 && (word >> 10) & 3 != 2)
        || word & 0x3f200c00 == 0x38200800
    {
        // Load/store register: unsigned offset, unscaled, pre/post-index, register offset.
        let size = (word >> 30) & 3;
        let opc = (word >> 22) & 3;
        let unsigned = word & 0x3f000000 == 0x39000000;
        let register = !unsigned && word & (1 << 21) != 0;
        let mode = if unsigned || register {
            0
        } else {
            ((word >> 10) & 3) as u8
        };
        if opc == 2 && size == 3 {
            // PRFM (immediate/register) and PRFUM; pre/post-index forms are unallocated.
            if mode == 1 || mode == 3 {
                return unsupported(word, pc);
            }
            Op::Nop
        } else {
            if (opc == 3 && size >= 2) || (opc == 2 && size == 3) {
                return unsupported(word, pc);
            }
            // Writeback overlap is constrained unpredictable: reject rather than choose a host accident.
            if mode != 0 && rn == rd && rn != 31 {
                return unsupported(word, pc);
            }
            let bytes = 1u8 << size;
            let index = if register {
                let option = ((word >> 13) & 7) as u8;
                if option & 2 == 0 {
                    return unsupported(word, pc);
                }
                Some(Index {
                    reg: reg(rm, false),
                    extend: option,
                    shift: if word & (1 << 12) != 0 { size as u8 } else { 0 },
                })
            } else {
                None
            };
            Op::Memory {
                load: opc != 0,
                bytes,
                dst: reg(rd, false),
                base: reg(rn, true),
                offset: if unsigned {
                    i64::from((word >> 10) & 0xfff) * i64::from(bytes)
                } else if register {
                    0
                } else {
                    signed((word >> 12) & 0x1ff, 9)
                },
                mode,
                signed: opc >= 2,
                dst_width: if opc == 2 || (opc == 1 && size == 3) || (opc == 0 && size == 3) {
                    Width::X64
                } else {
                    Width::W32
                },
                index,
            }
        }
    } else if word & 0x3e000000 == 0x28000000 && word & (1 << 26) == 0 {
        let opc = word >> 30;
        let load = word & (1 << 22) != 0;
        let mode = ((word >> 23) & 3) as u8;
        // opc 01 is LDPSW (load, not non-temporal); opc 11 is unallocated.
        if opc == 3 || (opc == 1 && (!load || mode == 0)) {
            return unsupported(word, pc);
        }
        let bytes = if opc == 2 { 8 } else { 4 };
        let rt2 = ((word >> 10) & 31) as u8;
        // LDNP/STNP (mode 0) have offset-mode addressing semantics.
        let mode = if mode == 0 { 2 } else { mode };
        if (mode != 2 && rn != 31 && (rn == rd || rn == rt2)) || (load && rd == rt2) {
            return unsupported(word, pc);
        }
        Op::Pair {
            load,
            bytes,
            first: reg(rd, false),
            second: reg(rt2, false),
            base: reg(rn, true),
            offset: signed((word >> 15) & 127, 7) * i64::from(bytes),
            mode,
            signed: opc == 1,
        }
    } else if word & 0x3f000000 == 0x08000000 {
        // Load/store exclusive and load-acquire/store-release (ARMv8.0 forms only).
        let size = (word >> 30) & 3;
        let o2 = word & (1 << 23) != 0;
        let load = word & (1 << 22) != 0;
        let pair = word & (1 << 21) != 0;
        let ordered = word & (1 << 15) != 0;
        let rs = rm;
        let rt2 = ((word >> 10) & 31) as u8;
        if (o2 && (pair || !ordered)) || (pair && size < 2) {
            return unsupported(word, pc);
        }
        let exclusive = !o2;
        // Unused register fields must be all ones; overlapping status/data/base registers
        // and LDXP with Rt == Rt2 are constrained unpredictable.
        if (!(exclusive && !load) && rs != 31)
            || (!pair && rt2 != 31)
            || (exclusive && !load && (rs == rd || (pair && rs == rt2) || (rs == rn && rn != 31)))
            || (pair && load && rd == rt2)
        {
            return unsupported(word, pc);
        }
        Op::Exclusive {
            load,
            exclusive,
            ordered,
            pair,
            bytes: 1 << size,
            status: reg(rs, false),
            first: reg(rd, false),
            second: reg(rt2, false),
            base: reg(rn, true),
        }
    } else if word & 0xffe0001f == 0xd4000001 {
        Op::Svc {
            id: ((word >> 5) & 0xffff) as u16,
        }
    } else if let Some(op) = simd::decode(word, pc)? {
        op
    } else {
        return unsupported(word, pc);
    };
    Ok(Instruction { pc, word, op })
}
fn unsupported<T>(word: u32, pc: GuestAddr) -> Result<T> {
    Err(format!(
        "unsupported or reserved AArch64 instruction at guest PC {:#x}: opcode {word:#010x}",
        pc.0
    ))
}

/// Encoding class of an A64 word, following the top-level and second-level decode tables of
/// the Arm Architecture Reference Manual (DDI 0487, section C4.1). It classifies encodings,
/// not validity: a word in a class may still be unallocated, and data also decodes to a class.
pub fn encoding_class(word: u32) -> &'static str {
    let simd = word & (1 << 26) != 0;
    match (word >> 25) & 0xf {
        0b0000 => "reserved",
        0b0001 | 0b0011 => "unallocated",
        0b0010 => "sve",
        0b1000 | 0b1001 => match (word >> 23) & 7 {
            0b000 | 0b001 => "dp-imm/pc-relative",
            0b010 => "dp-imm/add-sub",
            0b011 => "dp-imm/add-sub-tags",
            0b100 => "dp-imm/logical",
            0b101 => "dp-imm/move-wide",
            0b110 => "dp-imm/bitfield",
            _ => "dp-imm/extract",
        },
        0b1010 | 0b1011 => {
            let top = word >> 29;
            if top & 3 == 0 {
                "branch/unconditional-immediate"
            } else if top & 3 == 1 {
                if word & (1 << 25) == 0 {
                    "branch/compare-and-branch"
                } else {
                    "branch/test-and-branch"
                }
            } else if top == 0b010 {
                "branch/conditional"
            } else if top == 0b110 {
                if word & (1 << 25) != 0 {
                    "branch/unconditional-register"
                } else if word & (1 << 24) == 0 {
                    "system/exception-generation"
                } else if word & 0xfffff01f == 0xd503201f {
                    "system/hint"
                } else if word & 0xfffff01f == 0xd503301f {
                    "system/barrier"
                } else if word & 0xfff8f01f == 0xd500401f {
                    "system/pstate"
                } else if word & 0xffd80000 == 0xd5080000 {
                    "system/sys-instruction"
                } else if word & 0xffd00000 == 0xd5100000 {
                    "system/register-move"
                } else {
                    "system/other"
                }
            } else {
                "unallocated"
            }
        }
        0b0100 | 0b0110 | 0b1100 | 0b1110 => {
            if word & 0xbfbf0000 == 0x0c000000 || word & 0xbfa00000 == 0x0c800000 {
                "ldst/simd-multiple-structures"
            } else if word & 0xbf9f0000 == 0x0d000000 || word & 0xbf800000 == 0x0d800000 {
                "ldst/simd-single-structure"
            } else if word & 0x3f000000 == 0x08000000 {
                "ldst/exclusive-ordered"
            } else if word & 0x3b000000 == 0x18000000 {
                if simd {
                    "ldst/literal-simd-fp"
                } else {
                    "ldst/literal"
                }
            } else if word & 0x3a000000 == 0x28000000 {
                if simd {
                    "ldst/pair-simd-fp"
                } else {
                    "ldst/pair"
                }
            } else if word & 0x3b000000 == 0x39000000 {
                if simd {
                    "ldst/unsigned-offset-simd-fp"
                } else {
                    "ldst/unsigned-offset"
                }
            } else if word & 0x3b000000 == 0x38000000 {
                match ((word >> 21) & 1, (word >> 10) & 3) {
                    (0, 0) if simd => "ldst/unscaled-simd-fp",
                    (0, 0) => "ldst/unscaled",
                    (0, 1 | 3) if simd => "ldst/pre-post-index-simd-fp",
                    (0, 1 | 3) => "ldst/pre-post-index",
                    (0, _) => "ldst/unprivileged",
                    (1, 0) => "ldst/atomic-memory",
                    (1, 2) if simd => "ldst/register-offset-simd-fp",
                    (1, 2) => "ldst/register-offset",
                    _ => "ldst/pointer-authentication",
                }
            } else {
                "ldst/other"
            }
        }
        0b0101 | 0b1101 => {
            let op1 = (word >> 28) & 1;
            let op2 = (word >> 21) & 0xf;
            match (op1, op2) {
                (0, 0..=7) => "dp-reg/logical-shifted",
                (0, _) if op2 & 1 == 0 => "dp-reg/add-sub-shifted",
                (0, _) => "dp-reg/add-sub-extended",
                (1, 0b0110) if word & (1 << 30) == 0 => "dp-reg/two-source",
                (1, 0b0110) => "dp-reg/one-source",
                (1, 0b0000) => "dp-reg/carry-rotate-flags",
                (1, 0b0010) => "dp-reg/conditional-compare",
                (1, 0b0100) => "dp-reg/conditional-select",
                (1, 8..=15) => "dp-reg/three-source",
                _ => "dp-reg/unallocated",
            }
        }
        _ => {
            if word & (1 << 28) == 0 {
                "simd/vector"
            } else if word & (1 << 30) != 0 {
                "simd/scalar"
            } else if word & (1 << 24) != 0 {
                "fp/three-source"
            } else if word & (1 << 21) != 0 {
                "fp/scalar"
            } else {
                "fp/conversion-fixed-point"
            }
        }
    }
}
