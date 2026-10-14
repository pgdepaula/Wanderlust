//! SIMD&FP decoding: scalar floating point, Advanced SIMD and SIMD&FP loads/stores (ARMv8.0).
//!
//! Operation selectors for vector instructions are the constants of the runtime's `simd`
//! module (shared numbering), so lowering can name a helper and a constant directly.
use crate::{Index, Op, Reg, Width, reg, signed, target, unsupported};
use serde::Serialize;
use wl_common::{GuestAddr, Result};

/// Lanes written by an operation: `esize`-bit elements, `lanes` of them. Destination bits
/// above `esize * lanes` are zeroed (64-bit vectors and scalar results).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
pub struct Shape {
    pub esize: u8,
    pub lanes: u8,
}
impl Shape {
    pub fn total(self) -> u32 {
        u32::from(self.esize) * u32::from(self.lanes)
    }
    fn vector(esize: u32, q: bool) -> Self {
        Self {
            esize: esize as u8,
            lanes: ((if q { 128 } else { 64 }) / esize) as u8,
        }
    }
    fn scalar(esize: u32) -> Self {
        Self {
            esize: esize as u8,
            lanes: 1,
        }
    }
}
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
pub enum FpBinary {
    Mul,
    Div,
    Add,
    Sub,
    Max,
    Min,
    MaxNum,
    MinNum,
    NegMul,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
pub enum FpUnary {
    Mov,
    Abs,
    Neg,
    Sqrt,
    /// FRINTN/P/M/Z/A/I with the runtime's rounding selector.
    Round(u8),
    /// FRINTX: FPCR rounding, signals inexact.
    RoundExact,
}
/// Scalar floating-point operations. `bits` is 16, 32 or 64.
#[derive(Clone, Copy, Debug, Serialize)]
pub enum FpOp {
    Binary {
        bits: u8,
        op: FpBinary,
        d: u8,
        n: u8,
        m: u8,
    },
    Unary {
        bits: u8,
        op: FpUnary,
        d: u8,
        n: u8,
    },
    Convert {
        from: u8,
        to: u8,
        d: u8,
        n: u8,
    },
    /// d = (negate_addend ? -a : a) + (negate_product ? -n : n) * m, one rounding.
    MulAdd {
        bits: u8,
        negate_addend: bool,
        negate_product: bool,
        d: u8,
        n: u8,
        m: u8,
        a: u8,
    },
    /// FCMP/FCMPE; `m = None` compares with +0.0.
    Compare {
        bits: u8,
        n: u8,
        m: Option<u8>,
        signal: bool,
    },
    CondCompare {
        bits: u8,
        n: u8,
        m: u8,
        condition: u8,
        nzcv: u8,
        signal: bool,
    },
    Select {
        bits: u8,
        d: u8,
        n: u8,
        m: u8,
        condition: u8,
    },
    Immediate {
        bits: u8,
        d: u8,
        imm8: u8,
    },
    /// FCVT[NPMZA][SU] and fixed-point FCVTZ[SU] to a general register.
    ToInt {
        bits: u8,
        dst: Reg,
        n: u8,
        width: Width,
        signed: bool,
        rounding: u8,
        fbits: u8,
    },
    /// SCVTF/UCVTF from a general register.
    FromInt {
        bits: u8,
        d: u8,
        src: Reg,
        width: Width,
        signed: bool,
        fbits: u8,
    },
    /// FMOV general <- SIMD&FP (`top` selects V.D[1]).
    MoveToGeneral {
        dst: Reg,
        n: u8,
        width: Width,
        top: bool,
    },
    /// FMOV SIMD&FP <- general (`top` writes V.D[1], preserving bits 63:0).
    MoveFromGeneral {
        d: u8,
        src: Reg,
        width: Width,
        top: bool,
    },
}
#[derive(Clone, Copy, Debug, Serialize)]
pub enum SimdOp {
    /// MOVI/MVNI (kind 0, value already inverted for MVNI), ORR (1), BIC (2) immediate.
    Immediate {
        d: u8,
        total: u8,
        value: u64,
        kind: u8,
    },
    DupElement {
        shape: Shape,
        d: u8,
        n: u8,
        index: u8,
    },
    DupGeneral {
        shape: Shape,
        d: u8,
        src: Reg,
    },
    InsertElement {
        esize: u8,
        d: u8,
        index: u8,
        n: u8,
        source_index: u8,
    },
    InsertGeneral {
        esize: u8,
        d: u8,
        index: u8,
        src: Reg,
    },
    /// UMOV/SMOV.
    MoveToGeneral {
        esize: u8,
        dst: Reg,
        n: u8,
        index: u8,
        signed: bool,
        width: Width,
    },
    Int {
        shape: Shape,
        op: u8,
        d: u8,
        n: u8,
        m: u8,
    },
    IntPairwise {
        shape: Shape,
        op: u8,
        d: u8,
        n: u8,
        m: u8,
    },
    Logical {
        total: u8,
        op: u8,
        d: u8,
        n: u8,
        m: u8,
    },
    Fp {
        shape: Shape,
        op: u8,
        d: u8,
        n: u8,
        m: u8,
    },
    FpPairwise {
        shape: Shape,
        op: u8,
        d: u8,
        n: u8,
        m: u8,
    },
    IntUnary {
        shape: Shape,
        op: u8,
        d: u8,
        n: u8,
    },
    /// `mode`: rounding selector for FRINT/FCVT forms; `fbits`: fixed-point fraction bits.
    FpUnary {
        shape: Shape,
        op: u8,
        d: u8,
        n: u8,
        mode: u8,
        fbits: u8,
    },
    ShiftImm {
        shape: Shape,
        op: u8,
        d: u8,
        n: u8,
        amount: u8,
    },
    /// SSHLL/USHLL[2]: `esize` source lanes widened to 2*esize.
    Widen {
        esize: u8,
        lanes: u8,
        d: u8,
        n: u8,
        upper: bool,
        signed: bool,
        amount: u8,
    },
    /// XTN/SQXTN/UQXTN/SQXTUN/SHRN/RSHRN[2]: 2*esize source lanes narrowed to esize.
    Narrow {
        esize: u8,
        lanes: u8,
        op: u8,
        d: u8,
        n: u8,
        upper: bool,
        amount: u8,
    },
    /// FCVTL[2] (`esize` is the source element size).
    FpWiden {
        esize: u8,
        lanes: u8,
        d: u8,
        n: u8,
        upper: bool,
    },
    /// FCVTN[2] (`esize` is the destination element size).
    FpNarrow {
        esize: u8,
        lanes: u8,
        d: u8,
        n: u8,
        upper: bool,
    },
    IntElement {
        shape: Shape,
        op: u8,
        d: u8,
        n: u8,
        m: u8,
        index: u8,
    },
    FpElement {
        shape: Shape,
        op: u8,
        d: u8,
        n: u8,
        m: u8,
        index: u8,
    },
    /// Reduction of `lanes` elements to a scalar (`long`: result is 2*esize).
    Reduce {
        esize: u8,
        lanes: u8,
        op: u8,
        d: u8,
        n: u8,
    },
    FpReduce {
        esize: u8,
        lanes: u8,
        op: u8,
        d: u8,
        n: u8,
    },
    Permute {
        shape: Shape,
        op: u8,
        d: u8,
        n: u8,
        m: u8,
    },
    Ext {
        total: u8,
        d: u8,
        n: u8,
        m: u8,
        index: u8,
    },
    Table {
        total: u8,
        d: u8,
        n: u8,
        regs: u8,
        m: u8,
        keep: bool,
    },
    /// MULL/MLAL/MLSL by element (runtime `simd::long3_element`; `esize` is the source size).
    LongElement {
        esize: u8,
        lanes: u8,
        op: u8,
        d: u8,
        n: u8,
        m: u8,
        index: u8,
        upper: bool,
        signed: bool,
    },
    /// SADDLP/UADDLP/SADALP/UADALP (`esize` is the source size, `lanes` the result count).
    AddLongPairwise {
        esize: u8,
        lanes: u8,
        d: u8,
        n: u8,
        signed: bool,
        accumulate: bool,
    },
    /// Three registers of different widths (runtime `simd::long3`).
    Long3 {
        esize: u8,
        lanes: u8,
        op: u8,
        d: u8,
        n: u8,
        m: u8,
        upper: bool,
        signed: bool,
    },
}
/// Post-index update of a structure load/store.
#[derive(Clone, Copy, Debug, Serialize)]
pub enum PostIndex {
    Immediate(u64),
    Register(Reg),
}
/// LD1-LD4/ST1-ST4 (multiple structures), LD1R-LD4R and single-lane forms.
#[derive(Clone, Copy, Debug, Serialize)]
pub struct Structures {
    pub load: bool,
    /// Registers per repetition (structure size for LDn/STn).
    pub selem: u8,
    /// Repetitions (registers for LD1/ST1 with several registers).
    pub repeats: u8,
    pub esize: u8,
    pub total: u8,
    pub t: u8,
    pub base: Reg,
    pub post: Option<PostIndex>,
    /// Single-structure form: Some(lane) for one lane, None with `replicate` for LDnR.
    pub lane: Option<u8>,
    pub replicate: bool,
}

// Runtime operation selectors (crates/wl-runtime/src/simd.rs).
mod rt {
    pub const ADD: u8 = 0;
    pub const SUB: u8 = 1;
    pub const MUL: u8 = 2;
    pub const MLA: u8 = 3;
    pub const MLS: u8 = 4;
    pub const CMGT: u8 = 5;
    pub const CMHI: u8 = 6;
    pub const CMGE: u8 = 7;
    pub const CMHS: u8 = 8;
    pub const CMTST: u8 = 9;
    pub const CMEQ: u8 = 10;
    pub const SSHL: u8 = 11;
    pub const USHL: u8 = 12;
    pub const SMAX: u8 = 13;
    pub const UMAX: u8 = 14;
    pub const SMIN: u8 = 15;
    pub const UMIN: u8 = 16;
    pub const SABD: u8 = 17;
    pub const UABD: u8 = 18;
    pub const SHADD: u8 = 19;
    pub const UHADD: u8 = 20;
    pub const SRHADD: u8 = 21;
    pub const URHADD: u8 = 22;
    pub const SHSUB: u8 = 23;
    pub const UHSUB: u8 = 24;
    pub const SQADD: u8 = 25;
    pub const UQADD: u8 = 26;
    pub const SQSUB: u8 = 27;
    pub const UQSUB: u8 = 28;
    pub const SABA: u8 = 29;
    pub const UABA: u8 = 30;
    pub const SQDMULH: u8 = 31;
    pub const SQRDMULH: u8 = 32;
    pub const FADD: u8 = 0;
    pub const FSUB: u8 = 1;
    pub const FMUL: u8 = 2;
    pub const FDIV: u8 = 3;
    pub const FMAX: u8 = 4;
    pub const FMIN: u8 = 5;
    pub const FMAXNM: u8 = 6;
    pub const FMINNM: u8 = 7;
    pub const FABD: u8 = 8;
    pub const FMULX: u8 = 9;
    pub const FMLA: u8 = 10;
    pub const FMLS: u8 = 11;
    pub const FCMEQ: u8 = 12;
    pub const FCMGE: u8 = 13;
    pub const FCMGT: u8 = 14;
    pub const FACGE: u8 = 15;
    pub const FACGT: u8 = 16;
    pub const FRECPS: u8 = 17;
    pub const FRSQRTS: u8 = 18;
    pub const NOT: u8 = 0;
    pub const NEG: u8 = 1;
    pub const ABS: u8 = 2;
    pub const CNT: u8 = 3;
    pub const RBIT: u8 = 4;
    pub const REV16: u8 = 5;
    pub const REV32: u8 = 6;
    pub const REV64: u8 = 7;
    pub const CMGT0: u8 = 8;
    pub const CMGE0: u8 = 9;
    pub const CMEQ0: u8 = 10;
    pub const CMLE0: u8 = 11;
    pub const CMLT0: u8 = 12;
    pub const CLZ: u8 = 13;
    pub const CLS: u8 = 14;
    pub const SQABS: u8 = 15;
    pub const SQNEG: u8 = 16;
    pub const FNEG: u8 = 0;
    pub const FABS: u8 = 1;
    pub const FSQRT: u8 = 2;
    pub const FRINT: u8 = 3;
    pub const FRINTX: u8 = 4;
    pub const FCVT_S: u8 = 5;
    pub const FCVT_U: u8 = 6;
    pub const SCVTF: u8 = 7;
    pub const UCVTF: u8 = 8;
    pub const FCMGT0: u8 = 9;
    pub const FCMGE0: u8 = 10;
    pub const FCMEQ0: u8 = 11;
    pub const FCMLE0: u8 = 12;
    pub const FCMLT0: u8 = 13;
    pub const SHL: u8 = 0;
    pub const USHR: u8 = 1;
    pub const SSHR: u8 = 2;
    pub const USRA: u8 = 3;
    pub const SSRA: u8 = 4;
    pub const URSHR: u8 = 5;
    pub const SRSHR: u8 = 6;
    pub const URSRA: u8 = 7;
    pub const SRSRA: u8 = 8;
    pub const SLI: u8 = 9;
    pub const SRI: u8 = 10;
    pub const XTN: u8 = 0;
    pub const SQXTN: u8 = 1;
    pub const UQXTN: u8 = 2;
    pub const SQXTUN: u8 = 3;
    pub const SHRN: u8 = 4;
    pub const RSHRN: u8 = 5;
    pub const SQSHRN: u8 = 6;
    pub const SQRSHRN: u8 = 7;
    pub const UQSHRN: u8 = 8;
    pub const UQRSHRN: u8 = 9;
    pub const SQSHRUN: u8 = 10;
    pub const SQRSHRUN: u8 = 11;
    // `simd::long3` selectors.
    pub const MLAL: u8 = 6;
    pub const MLSL: u8 = 7;
    pub const MULL: u8 = 8;
    pub const ADDV: u8 = 0;
    pub const SMAXV: u8 = 1;
    pub const UMAXV: u8 = 2;
    pub const SMINV: u8 = 3;
    pub const UMINV: u8 = 4;
    pub const SADDLV: u8 = 5;
    pub const UADDLV: u8 = 6;
    // Rounding selectors (crates/wl-runtime/src/fp.rs).
    pub const RN: u8 = 0;
    pub const RP: u8 = 1;
    pub const RM: u8 = 2;
    pub const RZ: u8 = 3;
    pub const RA: u8 = 4;
    pub const RFPCR: u8 = 5;
}

type Decoded = Result<Option<Op>>;
fn bit(w: u32, n: u32) -> bool {
    (w >> n) & 1 != 0
}
fn field(w: u32, lo: u32, len: u32) -> u32 {
    (w >> lo) & ((1 << len) - 1)
}
/// Scalar FP element size from `ftype`: 00 S, 01 D, 11 H; 10 reserved.
fn ftype_bits(ftype: u32) -> Option<u8> {
    match ftype {
        0 => Some(32),
        1 => Some(64),
        3 => Some(16),
        _ => None,
    }
}

/// Decodes SIMD&FP encodings. `Ok(None)`: not a SIMD&FP encoding handled here.
pub(crate) fn decode(w: u32, pc: GuestAddr) -> Decoded {
    let rd = (w & 31) as u8;
    let rn = field(w, 5, 5) as u8;
    let rm = field(w, 16, 5) as u8;
    // SIMD&FP load/store register forms (V = 1).
    if w & 0x3f000000 == 0x3d000000
        || (w & 0x3f200000 == 0x3c000000 && field(w, 10, 2) != 2)
        || w & 0x3f200c00 == 0x3c200800
    {
        return memory(w, pc, rd, rn, rm);
    }
    // LDP/STP (SIMD&FP): bits 29:25 = 10110.
    if w & 0x3e000000 == 0x2c000000 {
        return pair(w, pc, rd, rn);
    }
    if w & 0x3f000000 == 0x1c000000 {
        let bytes = match w >> 30 {
            0 => 4,
            1 => 8,
            2 => 16,
            _ => return unsupported(w, pc),
        };
        return Ok(Some(Op::VecLiteral {
            vt: rd,
            address: target(pc, signed(field(w, 5, 19) << 2, 21))?,
            bytes,
        }));
    }
    if w & 0xbf9f0000 == 0x0c000000 || w & 0xbf800000 == 0x0c800000 {
        return structures_multiple(w, pc, rd, rn, rm);
    }
    if w & 0xbf9f0000 == 0x0d000000 || w & 0xbf800000 == 0x0d800000 {
        return structures_single(w, pc, rd, rn, rm);
    }
    // Scalar floating point and FP<->integer conversions.
    if w & 0x5f000000 == 0x1f000000 {
        return fp_three_source(w, pc, rd, rn, rm);
    }
    if w & 0x5f200000 == 0x1e200000 {
        return fp_scalar(w, pc, rd, rn, rm);
    }
    if w & 0x5f200000 == 0x1e000000 {
        return fp_fixed(w, pc, rd, rn);
    }
    // Advanced SIMD, vector forms.
    if w & 0x9f200400 == 0x0e200400 {
        return three_same(w, pc, rd, rn, rm, false);
    }
    if w & 0x9f3e0c00 == 0x0e200800 {
        return two_misc(w, pc, rd, rn, false);
    }
    if w & 0x9f200c00 == 0x0e200000 {
        return three_different(w, pc, rd, rn, rm);
    }
    if w & 0x9f3e0c00 == 0x0e300800 {
        return across(w, pc, rd, rn);
    }
    if w & 0x9fe08400 == 0x0e000400 {
        return copy(w, pc, rd, rn, false);
    }
    if w & 0xbf208c00 == 0x0e000800 {
        let q = bit(w, 30);
        let size = field(w, 22, 2);
        if size == 3 && !q || field(w, 12, 3) & 3 == 0 {
            return unsupported(w, pc);
        }
        return Ok(Some(Op::Simd(SimdOp::Permute {
            shape: Shape::vector(8 << size, q),
            op: field(w, 12, 3) as u8,
            d: rd,
            n: rn,
            m: rm,
        })));
    }
    if w & 0xbfe08400 == 0x2e000000 {
        let q = bit(w, 30);
        let index = field(w, 11, 4) as u8;
        if !q && index >= 8 {
            return unsupported(w, pc);
        }
        return Ok(Some(Op::Simd(SimdOp::Ext {
            total: if q { 128 } else { 64 },
            d: rd,
            n: rn,
            m: rm,
            index,
        })));
    }
    if w & 0xbfe08c00 == 0x0e000000 {
        return Ok(Some(Op::Simd(SimdOp::Table {
            total: if bit(w, 30) { 128 } else { 64 },
            d: rd,
            n: rn,
            regs: field(w, 13, 2) as u8 + 1,
            m: rm,
            keep: bit(w, 12),
        })));
    }
    if w & 0x9ff80400 == 0x0f000400 {
        return modified_immediate(w, pc, rd);
    }
    if w & 0x9f800400 == 0x0f000400 {
        return shift_immediate(w, pc, rd, rn, false);
    }
    if w & 0x9f000400 == 0x0f000000 {
        return by_element(w, pc, rd, rn, false);
    }
    // Advanced SIMD, scalar forms.
    if w & 0xdf200400 == 0x5e200400 {
        return three_same(w, pc, rd, rn, rm, true);
    }
    if w & 0xdf3e0c00 == 0x5e200800 {
        return two_misc(w, pc, rd, rn, true);
    }
    if w & 0xdf3e0c00 == 0x5e300800 {
        return scalar_pairwise(w, pc, rd, rn);
    }
    if w & 0xdfe08400 == 0x5e000400 {
        return copy(w, pc, rd, rn, true);
    }
    if w & 0xdf800400 == 0x5f000400 {
        return shift_immediate(w, pc, rd, rn, true);
    }
    if w & 0xdf000400 == 0x5f000000 {
        return by_element(w, pc, rd, rn, true);
    }
    Ok(None)
}

fn memory(w: u32, pc: GuestAddr, rd: u8, rn: u8, rm: u8) -> Decoded {
    let size = field(w, 30, 2);
    let opc = field(w, 22, 2);
    let bytes: u8 = if opc & 2 != 0 {
        if size != 0 {
            return unsupported(w, pc);
        }
        16
    } else {
        1 << size
    };
    let load = opc & 1 != 0;
    let unsigned = w & 0x3f000000 == 0x3d000000;
    let register = !unsigned && bit(w, 21);
    let mode = if unsigned || register {
        0
    } else {
        field(w, 10, 2) as u8
    };
    let scale = bytes.trailing_zeros() as u8;
    let index = if register {
        let option = field(w, 13, 3) as u8;
        if option & 2 == 0 {
            return unsupported(w, pc);
        }
        Some(Index {
            reg: reg(rm, false),
            extend: option,
            shift: if bit(w, 12) { scale } else { 0 },
        })
    } else {
        None
    };
    Ok(Some(Op::VecMemory {
        load,
        bytes,
        vt: rd,
        base: reg(rn, true),
        offset: if unsigned {
            i64::from(field(w, 10, 12)) * i64::from(bytes)
        } else if register {
            0
        } else {
            signed(field(w, 12, 9), 9)
        },
        mode,
        index,
    }))
}
fn pair(w: u32, pc: GuestAddr, rd: u8, rn: u8) -> Decoded {
    let bytes: u8 = match w >> 30 {
        0 => 4,
        1 => 8,
        2 => 16,
        _ => return unsupported(w, pc),
    };
    let load = bit(w, 22);
    let rt2 = field(w, 10, 5) as u8;
    let mode = match field(w, 23, 2) as u8 {
        0 => 2,
        m => m,
    };
    if load && rd == rt2 {
        return unsupported(w, pc);
    }
    Ok(Some(Op::VecPair {
        load,
        bytes,
        first: rd,
        second: rt2,
        base: reg(rn, true),
        offset: signed(field(w, 15, 7), 7) * i64::from(bytes),
        mode,
    }))
}
fn post_index(w: u32, rn: u8, rm: u8, total: u64) -> Option<PostIndex> {
    if !bit(w, 23) {
        return None;
    }
    let _ = rn;
    Some(if rm == 31 {
        PostIndex::Immediate(total)
    } else {
        PostIndex::Register(reg(rm, false))
    })
}
fn structures_multiple(w: u32, pc: GuestAddr, rd: u8, rn: u8, rm: u8) -> Decoded {
    let q = bit(w, 30);
    let size = field(w, 10, 2);
    let (selem, repeats) = match field(w, 12, 4) {
        0b0000 => (4, 1),
        0b0010 => (1, 4),
        0b0100 => (3, 1),
        0b0110 => (1, 3),
        0b0111 => (1, 1),
        0b1000 => (2, 1),
        0b1010 => (1, 2),
        _ => return unsupported(w, pc),
    };
    if size == 3 && !q && selem != 1 {
        return unsupported(w, pc);
    }
    let total = if q { 128u64 } else { 64 };
    Ok(Some(Op::VecStructures(Structures {
        load: bit(w, 22),
        selem,
        repeats,
        esize: 8 << size,
        total: total as u8,
        t: rd,
        base: reg(rn, true),
        post: post_index(w, rn, rm, total / 8 * u64::from(selem * repeats)),
        lane: None,
        replicate: false,
    })))
}
fn structures_single(w: u32, pc: GuestAddr, rd: u8, rn: u8, rm: u8) -> Decoded {
    let q = bit(w, 30);
    let load = bit(w, 22);
    let selem = ((field(w, 13, 1) << 1) | field(w, 21, 1)) as u8 + 1;
    let opcode = field(w, 13, 3);
    let s = field(w, 12, 1);
    let size = field(w, 10, 2);
    let (esize, lane, replicate): (u32, Option<u32>, bool) = match opcode >> 1 {
        0 => (8, Some((q as u32) << 3 | s << 2 | size), false),
        1 => {
            if size & 1 != 0 {
                return unsupported(w, pc);
            }
            (16, Some((q as u32) << 2 | s << 1 | size >> 1), false)
        }
        2 => match size {
            0 => (32, Some((q as u32) << 1 | s), false),
            1 if s == 0 => (64, Some(q as u32), false),
            _ => return unsupported(w, pc),
        },
        _ => {
            if !load || s != 0 {
                return unsupported(w, pc);
            }
            (8 << size, None, true)
        }
    };
    let total = if q { 128u8 } else { 64 };
    Ok(Some(Op::VecStructures(Structures {
        load,
        selem,
        repeats: 1,
        esize: esize as u8,
        total,
        t: rd,
        base: reg(rn, true),
        post: post_index(w, rn, rm, u64::from(esize / 8) * u64::from(selem)),
        lane: lane.map(|l| l as u8),
        replicate,
    })))
}

fn fp_three_source(w: u32, pc: GuestAddr, rd: u8, rn: u8, rm: u8) -> Decoded {
    if bit(w, 31) || bit(w, 29) {
        return unsupported(w, pc);
    }
    let bits = match field(w, 22, 2) {
        0 => 32,
        1 => 64,
        _ => return unsupported(w, pc),
    };
    let o1 = bit(w, 21);
    let o0 = bit(w, 15);
    Ok(Some(Op::Fp(FpOp::MulAdd {
        bits,
        negate_addend: o1,
        negate_product: o1 != o0,
        d: rd,
        n: rn,
        m: rm,
        a: field(w, 10, 5) as u8,
    })))
}
fn fp_scalar(w: u32, pc: GuestAddr, rd: u8, rn: u8, rm: u8) -> Decoded {
    let ftype = field(w, 22, 2);
    if field(w, 10, 6) == 0 {
        return fp_int(w, pc, rd, rn);
    }
    if bit(w, 31) || bit(w, 29) {
        return unsupported(w, pc);
    }
    let Some(bits) = ftype_bits(ftype) else {
        return unsupported(w, pc);
    };
    // Half-precision arithmetic is ARMv8.2; only conversions accept half operands.
    let arithmetic = |bits: u8| -> Result<u8> {
        if bits == 16 {
            unsupported(w, pc)
        } else {
            Ok(bits)
        }
    };
    let op = if field(w, 10, 5) == 0b10000 {
        let opcode = field(w, 15, 6);
        match opcode {
            0..=3 => Op::Fp(FpOp::Unary {
                bits: arithmetic(bits)?,
                op: [FpUnary::Mov, FpUnary::Abs, FpUnary::Neg, FpUnary::Sqrt][opcode as usize],
                d: rd,
                n: rn,
            }),
            4 | 5 | 7 => {
                let to = [32, 64, 0, 16][(opcode - 4) as usize];
                if to == bits {
                    return unsupported(w, pc);
                }
                Op::Fp(FpOp::Convert {
                    from: bits,
                    to,
                    d: rd,
                    n: rn,
                })
            }
            8..=12 | 14 | 15 => Op::Fp(FpOp::Unary {
                bits: arithmetic(bits)?,
                op: match opcode {
                    8 => FpUnary::Round(rt::RN),
                    9 => FpUnary::Round(rt::RP),
                    10 => FpUnary::Round(rt::RM),
                    11 => FpUnary::Round(rt::RZ),
                    12 => FpUnary::Round(rt::RA),
                    14 => FpUnary::RoundExact,
                    _ => FpUnary::Round(rt::RFPCR),
                },
                d: rd,
                n: rn,
            }),
            _ => return unsupported(w, pc),
        }
    } else if field(w, 10, 4) == 0b1000 {
        let opcode2 = field(w, 0, 5);
        if field(w, 14, 2) != 0 || opcode2 & 7 != 0 {
            return unsupported(w, pc);
        }
        let zero = opcode2 & 8 != 0;
        if zero && rm != 0 {
            return unsupported(w, pc);
        }
        Op::Fp(FpOp::Compare {
            bits: arithmetic(bits)?,
            n: rn,
            m: (!zero).then_some(rm),
            signal: opcode2 & 16 != 0,
        })
    } else if field(w, 10, 3) == 0b100 {
        if field(w, 5, 5) != 0 {
            return unsupported(w, pc);
        }
        Op::Fp(FpOp::Immediate {
            bits: arithmetic(bits)?,
            d: rd,
            imm8: field(w, 13, 8) as u8,
        })
    } else {
        match field(w, 10, 2) {
            1 => Op::Fp(FpOp::CondCompare {
                bits: arithmetic(bits)?,
                n: rn,
                m: rm,
                condition: field(w, 12, 4) as u8,
                nzcv: (w & 15) as u8,
                signal: bit(w, 4),
            }),
            2 => {
                let op = match field(w, 12, 4) {
                    0 => FpBinary::Mul,
                    1 => FpBinary::Div,
                    2 => FpBinary::Add,
                    3 => FpBinary::Sub,
                    4 => FpBinary::Max,
                    5 => FpBinary::Min,
                    6 => FpBinary::MaxNum,
                    7 => FpBinary::MinNum,
                    8 => FpBinary::NegMul,
                    _ => return unsupported(w, pc),
                };
                Op::Fp(FpOp::Binary {
                    bits: arithmetic(bits)?,
                    op,
                    d: rd,
                    n: rn,
                    m: rm,
                })
            }
            _ => Op::Fp(FpOp::Select {
                bits: arithmetic(bits)?,
                d: rd,
                n: rn,
                m: rm,
                condition: field(w, 12, 4) as u8,
            }),
        }
    };
    Ok(Some(op))
}
fn fp_int(w: u32, pc: GuestAddr, rd: u8, rn: u8) -> Decoded {
    let sf = bit(w, 31);
    let width = if sf { Width::X64 } else { Width::W32 };
    let ftype = field(w, 22, 2);
    let rmode = field(w, 19, 2);
    let opcode = field(w, 16, 3);
    if bit(w, 29) {
        return unsupported(w, pc);
    }
    // FMOV X <-> V.D[1].
    if ftype == 2 {
        return match (sf, rmode, opcode) {
            (true, 1, 6) => Ok(Some(Op::Fp(FpOp::MoveToGeneral {
                dst: reg(rd, false),
                n: rn,
                width,
                top: true,
            }))),
            (true, 1, 7) => Ok(Some(Op::Fp(FpOp::MoveFromGeneral {
                d: rd,
                src: reg(rn, false),
                width,
                top: true,
            }))),
            _ => unsupported(w, pc),
        };
    }
    let Some(bits) = ftype_bits(ftype) else {
        return unsupported(w, pc);
    };
    if bits == 16 {
        // FP16 conversions and moves to/from general registers are ARMv8.2.
        return unsupported(w, pc);
    }
    Ok(Some(Op::Fp(match (rmode, opcode) {
        (_, 0 | 1) | (0, 4 | 5) => FpOp::ToInt {
            bits,
            dst: reg(rd, false),
            n: rn,
            width,
            signed: opcode & 1 == 0,
            rounding: if opcode >= 4 {
                rt::RA
            } else {
                [rt::RN, rt::RP, rt::RM, rt::RZ][rmode as usize]
            },
            fbits: 0,
        },
        (0, 2 | 3) => FpOp::FromInt {
            bits,
            d: rd,
            src: reg(rn, false),
            width,
            signed: opcode == 2,
            fbits: 0,
        },
        (0, 6) if u32::from(bits) == u32::from(width.bits()) => FpOp::MoveToGeneral {
            dst: reg(rd, false),
            n: rn,
            width,
            top: false,
        },
        (0, 7) if u32::from(bits) == u32::from(width.bits()) => FpOp::MoveFromGeneral {
            d: rd,
            src: reg(rn, false),
            width,
            top: false,
        },
        _ => return unsupported(w, pc),
    })))
}
fn fp_fixed(w: u32, pc: GuestAddr, rd: u8, rn: u8) -> Decoded {
    let sf = bit(w, 31);
    let scale = field(w, 10, 6);
    let Some(bits) = ftype_bits(field(w, 22, 2)) else {
        return unsupported(w, pc);
    };
    if bit(w, 29) || bits == 16 || (!sf && scale < 32) {
        return unsupported(w, pc);
    }
    let width = if sf { Width::X64 } else { Width::W32 };
    let fbits = (64 - scale) as u8;
    Ok(Some(Op::Fp(match (field(w, 19, 2), field(w, 16, 3)) {
        (3, 0 | 1) => FpOp::ToInt {
            bits,
            dst: reg(rd, false),
            n: rn,
            width,
            signed: !bit(w, 16),
            rounding: rt::RZ,
            fbits,
        },
        (0, 2 | 3) => FpOp::FromInt {
            bits,
            d: rd,
            src: reg(rn, false),
            width,
            signed: !bit(w, 16),
            fbits,
        },
        _ => return unsupported(w, pc),
    })))
}

fn three_same(w: u32, pc: GuestAddr, rd: u8, rn: u8, rm: u8, scalar: bool) -> Decoded {
    let q = bit(w, 30);
    let u = bit(w, 29);
    let size = field(w, 22, 2);
    let opcode = field(w, 11, 5);
    if opcode >= 0x18 {
        let a = size >> 1;
        let sz = size & 1;
        let esize = 32 << sz;
        if !scalar && sz == 1 && !q {
            return unsupported(w, pc);
        }
        let shape = if scalar {
            Shape::scalar(esize)
        } else {
            Shape::vector(esize, q)
        };
        let (op, pairwise) = match (u, a, opcode) {
            (false, 0, 0x18) => (rt::FMAXNM, false),
            (false, 0, 0x19) => (rt::FMLA, false),
            (false, 0, 0x1a) => (rt::FADD, false),
            (false, 0, 0x1b) => (rt::FMULX, false),
            (false, 0, 0x1c) => (rt::FCMEQ, false),
            (false, 0, 0x1e) => (rt::FMAX, false),
            (false, 0, 0x1f) => (rt::FRECPS, false),
            (false, 1, 0x18) => (rt::FMINNM, false),
            (false, 1, 0x19) => (rt::FMLS, false),
            (false, 1, 0x1a) => (rt::FSUB, false),
            (false, 1, 0x1e) => (rt::FMIN, false),
            (false, 1, 0x1f) => (rt::FRSQRTS, false),
            (true, 0, 0x18) => (rt::FMAXNM, true),
            (true, 0, 0x1a) => (rt::FADD, true),
            (true, 0, 0x1b) => (rt::FMUL, false),
            (true, 0, 0x1c) => (rt::FCMGE, false),
            (true, 0, 0x1d) => (rt::FACGE, false),
            (true, 0, 0x1e) => (rt::FMAX, true),
            (true, 0, 0x1f) => (rt::FDIV, false),
            (true, 1, 0x18) => (rt::FMINNM, true),
            (true, 1, 0x1a) => (rt::FABD, false),
            (true, 1, 0x1c) => (rt::FCMGT, false),
            (true, 1, 0x1d) => (rt::FACGT, false),
            (true, 1, 0x1e) => (rt::FMIN, true),
            _ => return unsupported(w, pc),
        };
        // Scalar three-same FP exists only for FMULX, compares, FRECPS/FRSQRTS and FABD.
        if scalar
            && (pairwise
                || !matches!(
                    op,
                    rt::FMULX
                        | rt::FCMEQ
                        | rt::FCMGE
                        | rt::FCMGT
                        | rt::FACGE
                        | rt::FACGT
                        | rt::FRECPS
                        | rt::FRSQRTS
                        | rt::FABD
                ))
        {
            return unsupported(w, pc);
        }
        return Ok(Some(Op::Simd(if pairwise {
            SimdOp::FpPairwise {
                shape,
                op,
                d: rd,
                n: rn,
                m: rm,
            }
        } else {
            SimdOp::Fp {
                shape,
                op,
                d: rd,
                n: rn,
                m: rm,
            }
        })));
    }
    if opcode == 0x03 {
        if scalar {
            return unsupported(w, pc);
        }
        return Ok(Some(Op::Simd(SimdOp::Logical {
            total: if q { 128 } else { 64 },
            op: (u as u8) << 2 | size as u8,
            d: rd,
            n: rn,
            m: rm,
        })));
    }
    let esize = 8u32 << size;
    let (op, pairwise, allows_64) = match (u, opcode) {
        (false, 0x00) => (rt::SHADD, false, false),
        (true, 0x00) => (rt::UHADD, false, false),
        (false, 0x01) => (rt::SQADD, false, true),
        (true, 0x01) => (rt::UQADD, false, true),
        (false, 0x02) => (rt::SRHADD, false, false),
        (true, 0x02) => (rt::URHADD, false, false),
        (false, 0x04) => (rt::SHSUB, false, false),
        (true, 0x04) => (rt::UHSUB, false, false),
        (false, 0x05) => (rt::SQSUB, false, true),
        (true, 0x05) => (rt::UQSUB, false, true),
        (false, 0x06) => (rt::CMGT, false, true),
        (true, 0x06) => (rt::CMHI, false, true),
        (false, 0x07) => (rt::CMGE, false, true),
        (true, 0x07) => (rt::CMHS, false, true),
        (false, 0x08) => (rt::SSHL, false, true),
        (true, 0x08) => (rt::USHL, false, true),
        (false, 0x0c) => (rt::SMAX, false, false),
        (true, 0x0c) => (rt::UMAX, false, false),
        (false, 0x0d) => (rt::SMIN, false, false),
        (true, 0x0d) => (rt::UMIN, false, false),
        (false, 0x0e) => (rt::SABD, false, false),
        (true, 0x0e) => (rt::UABD, false, false),
        (false, 0x0f) => (rt::SABA, false, false),
        (true, 0x0f) => (rt::UABA, false, false),
        (false, 0x10) => (rt::ADD, false, true),
        (true, 0x10) => (rt::SUB, false, true),
        (false, 0x11) => (rt::CMTST, false, true),
        (true, 0x11) => (rt::CMEQ, false, true),
        (false, 0x12) => (rt::MLA, false, false),
        (true, 0x12) => (rt::MLS, false, false),
        (false, 0x13) => (rt::MUL, false, false),
        (false, 0x14) => (rt::SMAX, true, false),
        (true, 0x14) => (rt::UMAX, true, false),
        (false, 0x15) => (rt::SMIN, true, false),
        (true, 0x15) => (rt::UMIN, true, false),
        (false, 0x17) => (rt::ADD, true, true),
        (false, 0x16) => (rt::SQDMULH, false, false),
        (true, 0x16) => (rt::SQRDMULH, false, false),
        _ => return unsupported(w, pc),
    };
    let doubling = matches!(op, rt::SQDMULH | rt::SQRDMULH);
    if doubling && !(size == 1 || size == 2) {
        return unsupported(w, pc);
    }
    if size == 3 && (!allows_64 || (!scalar && !q)) {
        return unsupported(w, pc);
    }
    if scalar {
        // Scalar integer three-same: 64-bit add/sub/compare/shift, saturating at any size.
        let saturating = matches!(op, rt::SQADD | rt::UQADD | rt::SQSUB | rt::UQSUB);
        if pairwise || (!saturating && !doubling && size != 3) || (!allows_64 && !doubling) {
            return unsupported(w, pc);
        }
    }
    let shape = if scalar {
        Shape::scalar(esize)
    } else {
        Shape::vector(esize, q)
    };
    Ok(Some(Op::Simd(if pairwise {
        SimdOp::IntPairwise {
            shape,
            op,
            d: rd,
            n: rn,
            m: rm,
        }
    } else {
        SimdOp::Int {
            shape,
            op,
            d: rd,
            n: rn,
            m: rm,
        }
    })))
}

fn three_different(w: u32, pc: GuestAddr, rd: u8, rn: u8, rm: u8) -> Decoded {
    let q = bit(w, 30);
    let u = bit(w, 29);
    let size = field(w, 22, 2);
    if size == 3 {
        return unsupported(w, pc);
    }
    let (op, signed) = match (field(w, 12, 4), u) {
        (0x0, _) => (0, !u),
        (0x1, _) => (1, !u),
        (0x2, _) => (2, !u),
        (0x3, _) => (3, !u),
        (0x4, false) => (9, false),
        (0x4, true) => (11, false),
        (0x5, _) => (4, !u),
        (0x6, false) => (10, false),
        (0x6, true) => (12, false),
        (0x7, _) => (5, !u),
        (0x8, _) => (6, !u),
        (0xa, _) => (7, !u),
        (0xc, _) => (8, !u),
        _ => return unsupported(w, pc),
    };
    let esize = 8u32 << size;
    Ok(Some(Op::Simd(SimdOp::Long3 {
        esize: esize as u8,
        lanes: (64 / esize) as u8,
        op,
        d: rd,
        n: rn,
        m: rm,
        upper: q,
        signed,
    })))
}
fn two_misc(w: u32, pc: GuestAddr, rd: u8, rn: u8, scalar: bool) -> Decoded {
    let q = bit(w, 30);
    let u = bit(w, 29);
    let size = field(w, 22, 2);
    let opcode = field(w, 12, 5);
    let a = size >> 1;
    let sz = size & 1;
    // SADDLP/UADDLP (0x02) and SADALP/UADALP (0x06).
    if !scalar && (opcode == 0x02 || opcode == 0x06) {
        if size == 3 {
            return unsupported(w, pc);
        }
        let esize = 8u32 << size;
        return Ok(Some(Op::Simd(SimdOp::AddLongPairwise {
            esize: esize as u8,
            lanes: ((if q { 128 } else { 64 }) / (2 * esize)) as u8,
            d: rd,
            n: rn,
            signed: !u,
            accumulate: opcode == 0x06,
        })));
    }
    // Narrowing (integer): XTN/SQXTN/UQXTN/SQXTUN.
    if matches!(
        (u, opcode),
        (false, 0x12) | (true, 0x12) | (false, 0x14) | (true, 0x14)
    ) {
        if size == 3 || (scalar && opcode == 0x12 && !u) {
            return unsupported(w, pc);
        }
        let esize = 8u32 << size;
        let op = match (u, opcode) {
            (false, 0x12) => rt::XTN,
            (true, 0x12) => rt::SQXTUN,
            (false, 0x14) => rt::SQXTN,
            _ => rt::UQXTN,
        };
        return Ok(Some(Op::Simd(SimdOp::Narrow {
            esize: esize as u8,
            lanes: if scalar { 1 } else { (64 / esize) as u8 },
            op,
            d: rd,
            n: rn,
            upper: q && !scalar,
            amount: 0,
        })));
    }
    // FCVTN/FCVTL.
    if !scalar && !u && a == 0 && (opcode == 0x16 || opcode == 0x17) {
        let small = 16u32 << sz;
        let lanes = (64 / small) as u8;
        return Ok(Some(Op::Simd(if opcode == 0x16 {
            SimdOp::FpNarrow {
                esize: small as u8,
                lanes,
                d: rd,
                n: rn,
                upper: q,
            }
        } else {
            SimdOp::FpWiden {
                esize: small as u8,
                lanes,
                d: rd,
                n: rn,
                upper: q,
            }
        })));
    }
    let fp = opcode >= 0x0c && (opcode != 0x12 && opcode != 0x14) && !(opcode <= 0x0f && a == 0);
    if fp {
        let esize = 32u32 << sz;
        if !scalar && sz == 1 && !q {
            return unsupported(w, pc);
        }
        let shape = if scalar {
            Shape::scalar(esize)
        } else {
            Shape::vector(esize, q)
        };
        let (op, mode) = match (u, a, opcode) {
            (false, 1, 0x0c) => (rt::FCMGT0, 0),
            (true, 1, 0x0c) => (rt::FCMGE0, 0),
            (false, 1, 0x0d) => (rt::FCMEQ0, 0),
            (true, 1, 0x0d) => (rt::FCMLE0, 0),
            (false, 1, 0x0e) => (rt::FCMLT0, 0),
            (false, 1, 0x0f) if !scalar => (rt::FABS, 0),
            (true, 1, 0x0f) if !scalar => (rt::FNEG, 0),
            (true, 1, 0x1f) if !scalar => (rt::FSQRT, 0),
            (false, 0, 0x18) if !scalar => (rt::FRINT, rt::RN),
            (true, 0, 0x18) if !scalar => (rt::FRINT, rt::RA),
            (false, 0, 0x19) if !scalar => (rt::FRINT, rt::RM),
            (true, 0, 0x19) if !scalar => (rt::FRINTX, rt::RFPCR),
            (false, 1, 0x18) if !scalar => (rt::FRINT, rt::RP),
            (false, 1, 0x19) if !scalar => (rt::FRINT, rt::RZ),
            (true, 1, 0x19) if !scalar => (rt::FRINT, rt::RFPCR),
            (false, 0, 0x1a) => (rt::FCVT_S, rt::RN),
            (true, 0, 0x1a) => (rt::FCVT_U, rt::RN),
            (false, 0, 0x1b) => (rt::FCVT_S, rt::RM),
            (true, 0, 0x1b) => (rt::FCVT_U, rt::RM),
            (false, 0, 0x1c) => (rt::FCVT_S, rt::RA),
            (true, 0, 0x1c) => (rt::FCVT_U, rt::RA),
            (false, 0, 0x1d) => (rt::SCVTF, 0),
            (true, 0, 0x1d) => (rt::UCVTF, 0),
            (false, 1, 0x1a) => (rt::FCVT_S, rt::RP),
            (true, 1, 0x1a) => (rt::FCVT_U, rt::RP),
            (false, 1, 0x1b) => (rt::FCVT_S, rt::RZ),
            (true, 1, 0x1b) => (rt::FCVT_U, rt::RZ),
            _ => return unsupported(w, pc),
        };
        return Ok(Some(Op::Simd(SimdOp::FpUnary {
            shape,
            op,
            d: rd,
            n: rn,
            mode,
            fbits: 0,
        })));
    }
    let esize = 8u32 << size;
    let op = match (u, opcode) {
        (false, 0x00) if size < 3 => rt::REV64,
        (true, 0x00) if size < 2 => rt::REV32,
        (false, 0x01) if size == 0 => rt::REV16,
        (false, 0x04) if size < 3 => rt::CLS,
        (true, 0x04) if size < 3 => rt::CLZ,
        (false, 0x05) if size == 0 => rt::CNT,
        (true, 0x05) if size == 0 => rt::NOT,
        (true, 0x05) if size == 1 => rt::RBIT,
        (false, 0x07) => rt::SQABS,
        (true, 0x07) => rt::SQNEG,
        (false, 0x08) => rt::CMGT0,
        (true, 0x08) => rt::CMGE0,
        (false, 0x09) => rt::CMEQ0,
        (true, 0x09) => rt::CMLE0,
        (false, 0x0a) => rt::CMLT0,
        (false, 0x0b) => rt::ABS,
        (true, 0x0b) => rt::NEG,
        _ => return unsupported(w, pc),
    };
    // RBIT/NOT operate on bytes regardless of the size field.
    let esize = if op == rt::RBIT { 8 } else { esize };
    if scalar {
        let ok = match op {
            rt::SQABS | rt::SQNEG => true,
            rt::CMGT0 | rt::CMGE0 | rt::CMEQ0 | rt::CMLE0 | rt::CMLT0 | rt::ABS | rt::NEG => {
                size == 3
            }
            _ => false,
        };
        if !ok {
            return unsupported(w, pc);
        }
    } else if size == 3 && !q {
        return unsupported(w, pc);
    }
    Ok(Some(Op::Simd(SimdOp::IntUnary {
        shape: if scalar {
            Shape::scalar(esize)
        } else {
            Shape::vector(esize, q)
        },
        op,
        d: rd,
        n: rn,
    })))
}

fn across(w: u32, pc: GuestAddr, rd: u8, rn: u8) -> Decoded {
    let q = bit(w, 30);
    let u = bit(w, 29);
    let size = field(w, 22, 2);
    let opcode = field(w, 12, 5);
    if opcode == 0x0c || opcode == 0x0f {
        // FMAXNMV/FMINNMV/FMAXV/FMINV: only the 4S arrangement exists in ARMv8.0.
        if !u || !q || size & 1 != 0 {
            return unsupported(w, pc);
        }
        let max = size >> 1 == 0;
        let op = match (opcode, max) {
            (0x0c, true) => rt::FMAXNM,
            (0x0c, false) => rt::FMINNM,
            (_, true) => rt::FMAX,
            _ => rt::FMIN,
        };
        return Ok(Some(Op::Simd(SimdOp::FpReduce {
            esize: 32,
            lanes: 4,
            op,
            d: rd,
            n: rn,
        })));
    }
    if size == 3 || (size == 2 && !q) {
        return unsupported(w, pc);
    }
    let op = match (u, opcode) {
        (false, 0x1b) => rt::ADDV,
        (false, 0x0a) => rt::SMAXV,
        (true, 0x0a) => rt::UMAXV,
        (false, 0x1a) => rt::SMINV,
        (true, 0x1a) => rt::UMINV,
        (false, 0x03) => rt::SADDLV,
        (true, 0x03) => rt::UADDLV,
        _ => return unsupported(w, pc),
    };
    let esize = 8u32 << size;
    Ok(Some(Op::Simd(SimdOp::Reduce {
        esize: esize as u8,
        lanes: ((if q { 128 } else { 64 }) / esize) as u8,
        op,
        d: rd,
        n: rn,
    })))
}
fn scalar_pairwise(w: u32, pc: GuestAddr, rd: u8, rn: u8) -> Decoded {
    let u = bit(w, 29);
    let size = field(w, 22, 2);
    let opcode = field(w, 12, 5);
    if !u && opcode == 0x1b {
        if size != 3 {
            return unsupported(w, pc);
        }
        return Ok(Some(Op::Simd(SimdOp::Reduce {
            esize: 64,
            lanes: 2,
            op: rt::ADDV,
            d: rd,
            n: rn,
        })));
    }
    if !u {
        return unsupported(w, pc);
    }
    let esize = 32u8 << (size & 1);
    let min = size >> 1 == 1;
    let op = match (opcode, min) {
        (0x0c, false) => rt::FMAXNM,
        (0x0c, true) => rt::FMINNM,
        (0x0d, false) => rt::FADD,
        (0x0f, false) => rt::FMAX,
        (0x0f, true) => rt::FMIN,
        _ => return unsupported(w, pc),
    };
    Ok(Some(Op::Simd(SimdOp::FpReduce {
        esize,
        lanes: 2,
        op,
        d: rd,
        n: rn,
    })))
}
fn copy(w: u32, pc: GuestAddr, rd: u8, rn: u8, scalar: bool) -> Decoded {
    let q = bit(w, 30);
    let op = bit(w, 29);
    let imm5 = field(w, 16, 5);
    let imm4 = field(w, 11, 4);
    if imm5 & 15 == 0 {
        return unsupported(w, pc);
    }
    let size = imm5.trailing_zeros();
    let esize = 8u32 << size;
    let index = (imm5 >> (size + 1)) as u8;
    if scalar {
        // DUP (element) to scalar, alias MOV.
        if op || imm4 != 0 {
            return unsupported(w, pc);
        }
        return Ok(Some(Op::Simd(SimdOp::DupElement {
            shape: Shape::scalar(esize),
            d: rd,
            n: rn,
            index,
        })));
    }
    if op {
        if !q {
            return unsupported(w, pc);
        }
        return Ok(Some(Op::Simd(SimdOp::InsertElement {
            esize: esize as u8,
            d: rd,
            index,
            n: rn,
            source_index: (imm4 >> size) as u8,
        })));
    }
    Ok(Some(Op::Simd(match imm4 {
        0b0000 | 0b0001 => {
            if size == 3 && !q {
                return unsupported(w, pc);
            }
            if imm4 == 0 {
                SimdOp::DupElement {
                    shape: Shape::vector(esize, q),
                    d: rd,
                    n: rn,
                    index,
                }
            } else {
                SimdOp::DupGeneral {
                    shape: Shape::vector(esize, q),
                    d: rd,
                    src: reg(rn, false),
                }
            }
        }
        0b0011 if q => SimdOp::InsertGeneral {
            esize: esize as u8,
            d: rd,
            index,
            src: reg(rn, false),
        },
        0b0101 if (q && size < 3) || (!q && size < 2) => SimdOp::MoveToGeneral {
            esize: esize as u8,
            dst: reg(rd, false),
            n: rn,
            index,
            signed: true,
            width: if q { Width::X64 } else { Width::W32 },
        },
        0b0111 if (q && size == 3) || (!q && size < 3) => SimdOp::MoveToGeneral {
            esize: esize as u8,
            dst: reg(rd, false),
            n: rn,
            index,
            signed: false,
            width: if q { Width::X64 } else { Width::W32 },
        },
        _ => return unsupported(w, pc),
    })))
}
/// `AdvSIMDExpandImm`.
fn expand_imm(op: bool, cmode: u32, imm8: u64) -> Option<u64> {
    let rep2 = |x: u64| x | (x << 32);
    let rep4 = |x: u64| x | (x << 16) | (x << 32) | (x << 48);
    Some(match cmode >> 1 {
        0 => rep2(imm8),
        1 => rep2(imm8 << 8),
        2 => rep2(imm8 << 16),
        3 => rep2(imm8 << 24),
        4 => rep4(imm8),
        5 => rep4(imm8 << 8),
        6 => {
            if cmode & 1 == 0 {
                rep2((imm8 << 8) | 0xff)
            } else {
                rep2((imm8 << 16) | 0xffff)
            }
        }
        _ => match (cmode & 1, op) {
            (0, false) => imm8 * 0x0101_0101_0101_0101,
            (0, true) => (0..8).fold(0, |v, i| {
                v | if (imm8 >> i) & 1 != 0 {
                    0xff << (8 * i)
                } else {
                    0
                }
            }),
            (_, false) => {
                let a = (imm8 >> 7) & 1;
                let b = (imm8 >> 6) & 1;
                let imm32 = (a << 31)
                    | ((1 - b) << 30)
                    | (if b == 1 { 0x1f << 25 } else { 0 })
                    | ((imm8 & 0x3f) << 19);
                rep2(imm32)
            }
            _ => {
                let a = (imm8 >> 7) & 1;
                let b = (imm8 >> 6) & 1;
                (a << 63)
                    | ((1 - b) << 62)
                    | (if b == 1 { 0xff << 54 } else { 0 })
                    | ((imm8 & 0x3f) << 48)
            }
        },
    })
}
fn modified_immediate(w: u32, pc: GuestAddr, rd: u8) -> Decoded {
    let q = bit(w, 30);
    let op = bit(w, 29);
    let cmode = field(w, 12, 4);
    if bit(w, 11) || (op && cmode == 0b1111 && !q) {
        return unsupported(w, pc);
    }
    let imm8 = u64::from((field(w, 16, 3) << 5) | field(w, 5, 5));
    let Some(value) = expand_imm(op, cmode, imm8) else {
        return unsupported(w, pc);
    };
    // ORR/BIC exist for the 32-bit (0xx1) and 16-bit (10x1) shifted forms.
    let logical = cmode & 1 == 1 && cmode < 0b1100;
    let kind = if logical { 1 + op as u8 } else { 0 };
    let inverted = op && !logical && cmode != 0b1110 && cmode != 0b1111;
    Ok(Some(Op::Simd(SimdOp::Immediate {
        d: rd,
        total: if q { 128 } else { 64 },
        value: if inverted { !value } else { value },
        kind,
    })))
}
fn shift_immediate(w: u32, pc: GuestAddr, rd: u8, rn: u8, scalar: bool) -> Decoded {
    let q = bit(w, 30);
    let u = bit(w, 29);
    let immh = field(w, 19, 4);
    let immhb = field(w, 16, 7);
    let opcode = field(w, 11, 5);
    let esize = 8u32 << (31 - immh.leading_zeros());
    let right = 2 * esize - immhb;
    let left = immhb - esize;
    // Narrowing and widening forms interpret immh as the narrow element size.
    match opcode {
        0x10..=0x13 if u || opcode >= 0x12 => {
            // SQSHRN/SQRSHRN (U=0), UQSHRN/UQRSHRN (U=1, 0x12/0x13), SQSHRUN/SQRSHRUN (U=1).
            if immh & 8 != 0 {
                return unsupported(w, pc);
            }
            let op = match (u, opcode) {
                (false, 0x12) => rt::SQSHRN,
                (false, 0x13) => rt::SQRSHRN,
                (true, 0x12) => rt::UQSHRN,
                (true, 0x13) => rt::UQRSHRN,
                (true, 0x10) => rt::SQSHRUN,
                _ => rt::SQRSHRUN,
            };
            return Ok(Some(Op::Simd(SimdOp::Narrow {
                esize: esize as u8,
                lanes: if scalar { 1 } else { (64 / esize) as u8 },
                op,
                d: rd,
                n: rn,
                upper: q && !scalar,
                amount: right as u8,
            })));
        }
        0x10 | 0x11 if !u => {
            if immh & 8 != 0 || scalar {
                return unsupported(w, pc);
            }
            return Ok(Some(Op::Simd(SimdOp::Narrow {
                esize: esize as u8,
                lanes: (64 / esize) as u8,
                op: if opcode == 0x10 { rt::SHRN } else { rt::RSHRN },
                d: rd,
                n: rn,
                upper: q,
                amount: right as u8,
            })));
        }
        0x14 => {
            if immh & 8 != 0 || scalar {
                return unsupported(w, pc);
            }
            return Ok(Some(Op::Simd(SimdOp::Widen {
                esize: esize as u8,
                lanes: (64 / esize) as u8,
                d: rd,
                n: rn,
                upper: q,
                signed: !u,
                amount: left as u8,
            })));
        }
        0x1c | 0x1f => {
            if esize < 32 || (!scalar && esize == 64 && !q) {
                return unsupported(w, pc);
            }
            let shape = if scalar {
                Shape::scalar(esize)
            } else {
                Shape::vector(esize, q)
            };
            let op = match (opcode, u) {
                (0x1c, false) => rt::SCVTF,
                (0x1c, true) => rt::UCVTF,
                (_, false) => rt::FCVT_S,
                _ => rt::FCVT_U,
            };
            return Ok(Some(Op::Simd(SimdOp::FpUnary {
                shape,
                op,
                d: rd,
                n: rn,
                mode: rt::RZ,
                fbits: right as u8,
            })));
        }
        _ => {}
    }
    let (op, amount) = match (u, opcode) {
        (false, 0x00) => (rt::SSHR, right),
        (true, 0x00) => (rt::USHR, right),
        (false, 0x02) => (rt::SSRA, right),
        (true, 0x02) => (rt::USRA, right),
        (false, 0x04) => (rt::SRSHR, right),
        (true, 0x04) => (rt::URSHR, right),
        (false, 0x06) => (rt::SRSRA, right),
        (true, 0x06) => (rt::URSRA, right),
        (true, 0x08) => (rt::SRI, right),
        (false, 0x0a) => (rt::SHL, left),
        (true, 0x0a) => (rt::SLI, left),
        _ => return unsupported(w, pc),
    };
    if scalar && esize != 64 || (!scalar && esize == 64 && !q) {
        return unsupported(w, pc);
    }
    Ok(Some(Op::Simd(SimdOp::ShiftImm {
        shape: if scalar {
            Shape::scalar(esize)
        } else {
            Shape::vector(esize, q)
        },
        op,
        d: rd,
        n: rn,
        amount: amount as u8,
    })))
}
fn by_element(w: u32, pc: GuestAddr, rd: u8, rn: u8, scalar: bool) -> Decoded {
    let q = bit(w, 30);
    let u = bit(w, 29);
    let size = field(w, 22, 2);
    let l = field(w, 21, 1);
    let m_bit = field(w, 20, 1);
    let h = field(w, 11, 1);
    let opcode = field(w, 12, 4);
    let rm4 = field(w, 16, 4);
    let fp = matches!(
        (u, opcode),
        (false, 0x1) | (false, 0x5) | (false, 0x9) | (true, 0x9)
    );
    if fp {
        if size < 2 {
            // Half-precision by element is ARMv8.2.
            return unsupported(w, pc);
        }
        let sz = size & 1;
        let (esize, index) = if sz == 0 {
            (32u32, h << 1 | l)
        } else {
            if l != 0 || (!scalar && !q) {
                return unsupported(w, pc);
            }
            (64, h)
        };
        let op = match (u, opcode) {
            (false, 0x1) => rt::FMLA,
            (false, 0x5) => rt::FMLS,
            (false, 0x9) => rt::FMUL,
            _ => rt::FMULX,
        };
        return Ok(Some(Op::Simd(SimdOp::FpElement {
            shape: if scalar {
                Shape::scalar(esize)
            } else {
                Shape::vector(esize, q)
            },
            op,
            d: rd,
            n: rn,
            m: (m_bit << 4 | rm4) as u8,
            index: index as u8,
        })));
    }
    // SQDMULH/SQRDMULH by element (vector and scalar), 16- and 32-bit lanes.
    if !u && (opcode == 0xc || opcode == 0xd) {
        let (esize, index, m) = match size {
            1 => (16u32, h << 2 | l << 1 | m_bit, rm4),
            2 => (32, h << 1 | l, m_bit << 4 | rm4),
            _ => return unsupported(w, pc),
        };
        return Ok(Some(Op::Simd(SimdOp::IntElement {
            shape: if scalar {
                Shape::scalar(esize)
            } else {
                Shape::vector(esize, q)
            },
            op: if opcode == 0xc {
                rt::SQDMULH
            } else {
                rt::SQRDMULH
            },
            d: rd,
            n: rn,
            m: m as u8,
            index: index as u8,
        })));
    }
    if scalar {
        return unsupported(w, pc);
    }
    // SMULL/UMULL, SMLAL/UMLAL, SMLSL/UMLSL by element (Q selects the "2" upper forms).
    if matches!(opcode, 0x2 | 0x6 | 0xa) {
        let (esize, index, m) = match size {
            1 => (16u32, h << 2 | l << 1 | m_bit, rm4),
            2 => (32, h << 1 | l, m_bit << 4 | rm4),
            _ => return unsupported(w, pc),
        };
        return Ok(Some(Op::Simd(SimdOp::LongElement {
            esize: esize as u8,
            lanes: (64 / esize) as u8,
            op: match opcode {
                0x2 => rt::MLAL,
                0x6 => rt::MLSL,
                _ => rt::MULL,
            },
            d: rd,
            n: rn,
            m: m as u8,
            index: index as u8,
            upper: q,
            signed: !u,
        })));
    }
    let op = match (u, opcode) {
        (false, 0x8) => rt::MUL,
        (true, 0x0) => rt::MLA,
        (true, 0x4) => rt::MLS,
        _ => return unsupported(w, pc),
    };
    let (esize, index, m) = match size {
        1 => (16u32, h << 2 | l << 1 | m_bit, rm4),
        2 => (32, h << 1 | l, m_bit << 4 | rm4),
        _ => return unsupported(w, pc),
    };
    Ok(Some(Op::Simd(SimdOp::IntElement {
        shape: Shape::vector(esize, q),
        op,
        d: rd,
        n: rn,
        m: m as u8,
        index: index as u8,
    })))
}
