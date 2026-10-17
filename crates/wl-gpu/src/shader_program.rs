//! Bounded straight-line Maxwell vertex/fragment dataflow. Registers contain raw bits.
//! Resource data is runtime input, never a specialization of shader code.
use crate::{Error, Result, invalid};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ConstantRef {
    pub slot: u8,
    pub offset: u32,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FloatOperator {
    Add,
    Multiply,
}
/// IEEE f32 predicate conditions. Ordered relations reject NaN; unordered
/// relations accept NaN. Numeric includes infinities and subnormals.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FloatComparison {
    False,
    Less,
    Equal,
    LessEqual,
    Greater,
    NotEqual,
    GreaterEqual,
    Numeric,
    Nan,
    LessUnordered,
    EqualUnordered,
    LessEqualUnordered,
    GreaterUnordered,
    NotEqualUnordered,
    GreaterEqualUnordered,
    True,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LogicalOperator {
    And,
    Or,
    Xor,
    PassB,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IntegerOperator {
    Add,
    Multiply,
    Subtract,
    And,
    Or,
    Xor,
    Logical {
        operator: LogicalOperator,
        invert_left: bool,
        invert_right: bool,
    },
    MinUnsigned,
    MaxUnsigned,
    MinSigned,
    MaxSigned,
    ShiftLeft {
        wrap: bool,
    },
    ShiftRight {
        wrap: bool,
    },
    ShiftRightSigned {
        wrap: bool,
    },
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IntegerComparison {
    Less,
    Equal,
    LessEqual,
    Greater,
    NotEqual,
    GreaterEqual,
    LessSigned,
    EqualSigned,
    LessEqualSigned,
    GreaterSigned,
    NotEqualSigned,
    GreaterEqualSigned,
}
/// Compatibility name for the original unsigned comparison variants.
pub type UnsignedComparison = IntegerComparison;
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PermutationSelector {
    Immediate(u16),
    Register(u8),
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Operation {
    /// Generic SM50 PRMT: select bytes or replicate their sign bits.
    PermuteBytes {
        dst: u8,
        left: u8,
        right: u8,
        selector: PermutationSelector,
    },
    /// Compare into P0; the complementary predicate output is discarded to PT.
    PredicateCompare {
        left: u8,
        right: u8,
        comparison: IntegerComparison,
    },
    /// Unconditional FSETP into P0, complementary output discarded to PT.
    /// AND PT, no FTZ or source modifiers, GPR operands only.
    FloatPredicateCompare {
        left: u8,
        right: u8,
        comparison: FloatComparison,
    },
    /// Unconditionally select GPR bits using initialized P0 or its inversion.
    Select {
        dst: u8,
        left: u8,
        right: u8,
        invert: bool,
    },
    Integer {
        dst: u8,
        left: u8,
        right: u8,
        operator: IntegerOperator,
    },
    /// Ordinary immediate IADD, without carry input/output or saturation.
    AddImmediate {
        dst: u8,
        src: u8,
        bits: u32,
        negate_src: bool,
    },
    /// Immediate BFE control: contained field, no BREV or condition-code output.
    BitfieldExtract {
        dst: u8,
        src: u8,
        offset: u8,
        width: u8,
        signed: bool,
    },
    /// Variable-latency SM50 POPC, GPR/RZ source with optional bit inversion.
    CountBits {
        dst: u8,
        src: u8,
        invert: bool,
    },
    /// SM50 non-SH FLO, index of the highest non-sign bit or raw FFFFFFFF.
    LeadingBit {
        dst: u8,
        src: u8,
        signed: bool,
    },
    /// Scoreboarded SM50 I2F RN: 32-bit integer to exact binary32 raw bits.
    IntToFloat {
        dst: u8,
        src: u8,
        signed: bool,
    },
    /// Scoreboarded SM50 I2I: low 8/16 bits widened to like-signed 32 bits.
    IntWiden {
        dst: u8,
        src: u8,
        bits: u8,
        signed: bool,
    },
    /// GPR FSET, unmodified F32 operands, AND PT, mask or float result.
    FloatSet {
        dst: u8,
        left: u8,
        right: u8,
        comparison: FloatComparison,
        float_result: bool,
    },
    /// Unmodified GPR ISET, AND PT, signed/unsigned relation with mask/BF result.
    IntegerSet {
        dst: u8,
        left: u8,
        right: u8,
        comparison: IntegerComparison,
        float_result: bool,
    },
    /// Unmodified SM50 ISCADD GPR, wrapping (left << shift) + right.
    ScaledAdd {
        dst: u8,
        left: u8,
        right: u8,
        shift: u8,
    },
    /// SM50 LOP3.LUT, three GPR/RZ sources, discarded PT predicate output.
    TernaryLogic {
        dst: u8,
        first: u8,
        second: u8,
        third: u8,
        lut: u8,
    },
    /// Raw 32-bit immediate logic; immediate bits are not GPR operands.
    LogicImmediate {
        dst: u8,
        src: u8,
        bits: u32,
        operator: LogicalOperator,
        invert_src: bool,
        invert_immediate: bool,
    },
    Float {
        dst: u8,
        left: u8,
        right: u8,
        operator: FloatOperator,
    },
    /// IEEE f32 immediate bits. Short encodings omit twelve low mantissa bits;
    /// full-width forms retain all bits. This is not a GPR or integer conversion.
    FloatImmediate {
        dst: u8,
        src: u8,
        bits: u32,
        operator: FloatOperator,
    },
    Immediate {
        dst: u8,
        bits: u32,
    },
    Move {
        dst: u8,
        src: u8,
    },
    /// Full-lane GPR MOV guarded by P0 or !P0. A false guard retains the prior
    /// destination; both possible input values must be initialized and ready.
    ConditionalMove {
        dst: u8,
        src: u8,
        invert: bool,
    },
    Constant {
        dst: u8,
        input: u8,
    },
    AttributeRead {
        dst: u8,
        attribute: u16,
        words: u8,
    },
    AttributeWrite {
        src: u8,
        attribute: u16,
        words: u8,
    },
    Interpolate {
        dst: u8,
        component: u8,
    },
    PositionW {
        dst: u8,
    },
    Reciprocal {
        dst: u8,
        src: u8,
    },
    Perspective {
        dst: u8,
        component: u8,
        multiplier: u8,
    },
    Sample2d {
        dst: u8,
        coordinates: u8,
        handle: u8,
    },
}
#[derive(Debug, Clone, PartialEq, Eq)]
struct Program {
    operations: Vec<Operation>,
    constants: Vec<ConstantRef>,
    constant_pairs: Vec<ConstantRef>,
    texture_input: Option<u8>,
    perspective: bool,
}
impl Program {
    fn decode(input: &[u8], vertex: bool) -> Result<Self> {
        if input.len() < 112 || input.len() > 0x50 + 4096 || (input.len() - 0x50) % 32 != 0 {
            return Err(invalid("invalid Maxwell fragment length"));
        }
        let input_mode = u32::from_le_bytes(input[24..28].try_into().unwrap());
        let interpolated = !vertex && input_mode == 15;
        let perspective = !vertex && input_mode == 10;
        let mut header = [0u8; 80];
        header[..4].copy_from_slice(&if vertex { 0x60461u32 } else { 0x61462u32 }.to_le_bytes());
        header[16..20].copy_from_slice(&0xff000u32.to_le_bytes());
        if vertex {
            header[24] = 0x3f;
            header[50..54].copy_from_slice(&0xf0000000u32.to_le_bytes());
            header[54] = 3;
        } else {
            header[72..76].copy_from_slice(&15u32.to_le_bytes());
            if interpolated {
                header[24] = 15;
            } else if perspective {
                header[20..24].copy_from_slice(&0x80000000u32.to_le_bytes());
                header[24] = 10;
            }
        }
        if input[..80] != header {
            return Err(invalid("unsupported shader stage/SPH interface"));
        }
        let mut out = Self {
            operations: Vec::new(),
            constants: Vec::new(),
            constant_pairs: Vec::new(),
            texture_input: None,
            perspective,
        };
        let mut initialized = [false; 256];
        initialized[255] = true; // RZ
        let mut pending = [false; 256];
        let mut provenance = [None; 256];
        let mut multiply_result = [false; 256];
        // Only the canonical position-W reciprocal used for perspective IPA is
        // supported. This is not a general approximate MUFU implementation.
        let mut position_w = [false; 256];
        let mut reciprocal_w = [false; 256];
        let mut predicate_initialized = false;
        let mut predicate_multiply = false;
        let mut exited = false;
        let mut exports = 0u8;
        for (bundle_index, bundle) in input[0x50..].chunks_exact(32).enumerate() {
            let control = u64::from_le_bytes(bundle[..8].try_into().unwrap());
            if control >> 63 != 0 {
                return Err(invalid("reserved scheduling control bit"));
            }
            for (slot, b) in bundle[8..].chunks_exact(8).enumerate() {
                let pc = 0x50 + bundle_index * 32 + 8 + slot * 8;
                let fail = |msg: &str| Error(format!("Maxwell fragment {pc:#x}: {msg}"));
                let schedule = (control >> (slot * 21)) & 0x1fffff;
                let wait = (schedule >> 11) & 63;
                let write_barrier = (schedule >> 5) & 7;
                if !matches!(schedule & 31, 6 | 13)
                    || (schedule >> 8) & 7 != 7
                    || schedule >> 17 != 0
                    || !matches!(wait, 0 | 1)
                    || !matches!(write_barrier, 0 | 7)
                {
                    return Err(fail("unsupported scheduling dependency"));
                }
                if wait == 1 {
                    pending.fill(false);
                }
                let word = u64::from_le_bytes(b.try_into().unwrap());
                // Predicate-producing SETP has its own conservative fixed latency.
                // Do not generalize the new stall encoding to other instructions.
                if schedule & 31
                    != if matches!(word >> 52, 0x5b6 | 0x5bb) {
                        13
                    } else {
                        6
                    }
                {
                    return Err(fail("unsupported instruction fixed-latency schedule"));
                }
                if exited {
                    if word != 0x50b0000000070f00 || schedule != 0x7e6 {
                        return Err(fail("invalid EXIT padding"));
                    }
                    continue;
                }
                if word == 0xe30000000007000f {
                    if write_barrier != 7
                        || if vertex {
                            exports != 0x3f
                        } else {
                            (0..4).any(|r| !initialized[r] || pending[r])
                        }
                    {
                        return Err(fail("unready fragment exports"));
                    }
                    exited = true;
                    continue;
                }
                let attribute_mask = 255 | (0x3ffu64 << 20) | (3u64 << 47);
                if vertex && word & !attribute_mask == 0xeff07f800007ff00 {
                    let src = (word & 255) as u8;
                    let attribute = ((word >> 20) & 0x3ff) as u16;
                    let words = (((word >> 47) & 3) + 1) as u8;
                    let output = match attribute {
                        0x70..=0x7c => (attribute - 0x70) / 4,
                        0x80..=0x84 => 4 + (attribute - 0x80) / 4,
                        _ => return Err(fail("unsupported AST output")),
                    };
                    if attribute % 4 != 0
                        || output + u16::from(words) > 6
                        || (output < 4 && output + u16::from(words) > 4)
                        || write_barrier != 7
                        || u16::from(src) + u16::from(words) > 256
                        || (0..words).any(|c| {
                            let r = usize::from(src) + usize::from(c);
                            !initialized[r] || pending[r]
                        })
                    {
                        return Err(fail("invalid/unready AST group"));
                    }
                    exports |= ((1u8 << words) - 1) << output;
                    out.operations.push(Operation::AttributeWrite {
                        src,
                        attribute,
                        words,
                    });
                    continue;
                }
                let compare_mask = (15u64 << 48) | (255u64 << 8) | (255u64 << 20);
                if word & !compare_mask == 0x5b60038000070007 {
                    let comparison = match ((word >> 49) & 7, (word >> 48) & 1) {
                        (1, 0) => IntegerComparison::Less,
                        (2, 0) => IntegerComparison::Equal,
                        (3, 0) => IntegerComparison::LessEqual,
                        (4, 0) => IntegerComparison::Greater,
                        (5, 0) => IntegerComparison::NotEqual,
                        (6, 0) => IntegerComparison::GreaterEqual,
                        (1, 1) => IntegerComparison::LessSigned,
                        (2, 1) => IntegerComparison::EqualSigned,
                        (3, 1) => IntegerComparison::LessEqualSigned,
                        (4, 1) => IntegerComparison::GreaterSigned,
                        (5, 1) => IntegerComparison::NotEqualSigned,
                        (6, 1) => IntegerComparison::GreaterEqualSigned,
                        (_, 0) => return Err(fail("unsupported unsigned comparison mode")),
                        _ => return Err(fail("unsupported signed comparison mode")),
                    };
                    let left = ((word >> 8) & 255) as u8;
                    let right = ((word >> 20) & 255) as u8;
                    if write_barrier != 7
                        || [left, right]
                            .iter()
                            .any(|r| !initialized[*r as usize] || pending[*r as usize])
                    {
                        return Err(fail("unready predicate operand or unsupported schedule"));
                    }
                    predicate_initialized = true;
                    predicate_multiply =
                        multiply_result[left as usize] || multiply_result[right as usize];
                    out.operations.push(Operation::PredicateCompare {
                        left,
                        right,
                        comparison,
                    });
                    // Predicate pair bits are not a GPR destination. In particular,
                    // do not initialize/clear R7 metadata from the low-byte value 7.
                    continue;
                }
                if word & !0x000f00000ff0ff00 == 0x5bb0038000070007 {
                    let comparison = match (word >> 48) & 15 {
                        0 => FloatComparison::False,
                        1 => FloatComparison::Less,
                        2 => FloatComparison::Equal,
                        3 => FloatComparison::LessEqual,
                        4 => FloatComparison::Greater,
                        5 => FloatComparison::NotEqual,
                        6 => FloatComparison::GreaterEqual,
                        7 => FloatComparison::Numeric,
                        8 => FloatComparison::Nan,
                        9 => FloatComparison::LessUnordered,
                        10 => FloatComparison::EqualUnordered,
                        11 => FloatComparison::LessEqualUnordered,
                        12 => FloatComparison::GreaterUnordered,
                        13 => FloatComparison::NotEqualUnordered,
                        14 => FloatComparison::GreaterEqualUnordered,
                        _ => FloatComparison::True, // Remaining four-bit mode is 15.
                    };
                    let left = ((word >> 8) & 255) as u8;
                    let right = ((word >> 20) & 255) as u8;
                    if write_barrier != 7
                        || [left, right]
                            .iter()
                            .any(|r| !initialized[*r as usize] || pending[*r as usize])
                    {
                        return Err(fail(
                            "unready floating predicate operand or unsupported schedule",
                        ));
                    }
                    predicate_initialized = true;
                    predicate_multiply =
                        multiply_result[left as usize] || multiply_result[right as usize];
                    out.operations.push(Operation::FloatPredicateCompare {
                        left,
                        right,
                        comparison,
                    });
                    // The low byte is the P0/PT output pair, not GPR R7.
                    continue;
                }
                let dst = (word & 255) as u8;
                if dst == 255 {
                    return Err(fail("RZ destination unsupported"));
                }
                let immediate_mask = (u64::from(u32::MAX) << 20) | 255;
                let move_mask = (255u64 << 20) | 255;
                let ldc_mask = (31u64 << 36) | (0xffffu64 << 20) | 255;
                if !vertex && word & !ldc_mask == 0xef9500000007ff00 {
                    if write_barrier != 0 || pending.iter().any(|v| *v) {
                        return Err(fail("LDC.B64 requires an unused write barrier zero"));
                    }
                    let offset = ((word >> 20) & 0xffff) as u32;
                    if offset & 0x8007 != 0 || dst & 1 != 0 || dst > 252 {
                        return Err(fail("unsupported LDC.B64 offset or register pair"));
                    }
                    let slot = ((word >> 36) & 31) as u8;
                    let pair = ConstantRef { slot, offset };
                    if !out.constant_pairs.contains(&pair) {
                        out.constant_pairs.push(pair);
                    }
                    let mut inputs = [0; 2];
                    for (lane, input) in inputs.iter_mut().enumerate() {
                        *input = constant_input(
                            &mut out.constants,
                            ConstantRef {
                                slot,
                                offset: offset + lane as u32 * 4,
                            },
                        )
                        .ok_or_else(|| fail("too many constant references"))?;
                    }
                    // One binary load and one barrier cover the complete pair.
                    // Lower to two scalar values only after validating the group.
                    for (lane, input) in inputs.into_iter().enumerate() {
                        let register = dst + lane as u8;
                        initialized[register as usize] = true;
                        pending[register as usize] = true;
                        provenance[register as usize] = Some(input);
                        multiply_result[register as usize] = false;
                        position_w[register as usize] = false;
                        reciprocal_w[register as usize] = false;
                        out.operations.push(Operation::Constant {
                            dst: register,
                            input,
                        });
                    }
                    continue;
                }
                let op = if vertex && word & !attribute_mask == 0xefd87f800007ff00 {
                    let attribute = ((word >> 20) & 0x3ff) as u16;
                    let words = (((word >> 47) & 3) + 1) as u8;
                    if !matches!(attribute,0x80..=0x8c | 0x90..=0x94)
                        || attribute % 4 != 0
                        || attribute + u16::from(words) * 4 > 0x98
                        || (attribute < 0x90 && attribute + u16::from(words) * 4 > 0x90)
                        || u16::from(dst) + u16::from(words) > 255
                        || write_barrier != 0
                        || pending.iter().any(|p| *p)
                    {
                        return Err(fail("unsupported ALD group or scoreboard"));
                    }
                    for r in dst..dst + words {
                        initialized[r as usize] = true;
                        pending[r as usize] = true;
                        provenance[r as usize] = None;
                        multiply_result[r as usize] = false;
                        position_w[r as usize] = false;
                        reciprocal_w[r as usize] = false;
                    }
                    Operation::AttributeRead {
                        dst,
                        attribute,
                        words,
                    }
                } else if word & !immediate_mask == 0x010000000007f000 {
                    if write_barrier != 7 {
                        return Err(fail("ALU write barrier unsupported"));
                    }
                    Operation::Immediate {
                        dst,
                        bits: (word >> 20) as u32,
                    }
                } else if word & !move_mask == 0x5c98078000070000 {
                    let src = ((word >> 20) & 255) as u8;
                    if write_barrier != 7 || !initialized[src as usize] || pending[src as usize] {
                        return Err(fail("unready MOV source or unsupported schedule"));
                    }
                    Operation::Move { dst, src }
                } else if word & !0x000000000ff800ff == 0x5c98078000000000 {
                    let src = ((word >> 20) & 255) as u8;
                    if !predicate_initialized {
                        return Err(fail("uninitialized conditional MOV predicate P0"));
                    }
                    // The predicate is runtime data. The false path retains
                    // dst, so a conditional write cannot establish readiness
                    // for an otherwise undefined or pending destination.
                    if write_barrier != 7
                        || [dst, src]
                            .iter()
                            .any(|r| !initialized[*r as usize] || pending[*r as usize])
                    {
                        return Err(fail(
                            "unready conditional MOV operand or unsupported schedule",
                        ));
                    }
                    Operation::ConditionalMove {
                        dst,
                        src,
                        invert: word & (1 << 19) != 0,
                    }
                } else if word & !(255 | (255u64 << 8) | (255u64 << 20) | (1u64 << 42))
                    == 0x5ca0000000070000
                {
                    let left = ((word >> 8) & 255) as u8;
                    let right = ((word >> 20) & 255) as u8;
                    if !predicate_initialized {
                        return Err(fail("uninitialized selector predicate P0"));
                    }
                    if write_barrier != 7
                        || [left, right]
                            .iter()
                            .any(|r| !initialized[*r as usize] || pending[*r as usize])
                    {
                        return Err(fail("unready selector operand or unsupported schedule"));
                    }
                    Operation::Select {
                        dst,
                        left,
                        right,
                        invert: word & (1 << 42) != 0,
                    }
                } else if word & !0x010ffffffff0ffff == 0x1c00000000070000 {
                    let src = ((word >> 8) & 255) as u8;
                    if write_barrier != 7 || !initialized[src as usize] || pending[src as usize] {
                        return Err(fail("unready immediate addition operand/control"));
                    }
                    Operation::AddImmediate {
                        dst,
                        src,
                        bits: (word >> 20) as u32,
                        negate_src: word & (1 << 56) != 0,
                    }
                } else if word & !0x0102007ffff0ffff == 0x3810000000070000 {
                    let src = ((word >> 8) & 255) as u8;
                    if write_barrier != 7 || !initialized[src as usize] || pending[src as usize] {
                        return Err(fail("unready immediate addition operand/control"));
                    }
                    // This form has a signed20 immediate. Bit56 is its sign;
                    // source-A negation lives at bit49, unlike IADD32I.
                    let low = ((word >> 20) & 0x7ffff) as u32;
                    let bits = if word & (1 << 56) != 0 {
                        low | 0xfff80000
                    } else {
                        low
                    };
                    Operation::AddImmediate {
                        dst,
                        src,
                        bits,
                        negate_src: word & (1 << 49) != 0,
                    }
                } else if word & !0x01effffffff0ffff == 0x0400000000070000 {
                    let src = ((word >> 8) & 255) as u8;
                    let operator = match (word >> 53) & 3 {
                        0 => LogicalOperator::And,
                        1 => LogicalOperator::Or,
                        2 => LogicalOperator::Xor,
                        _ => return Err(fail("unsupported instruction: LOP32I operator")),
                    };
                    if write_barrier != 7 || !initialized[src as usize] || pending[src as usize] {
                        return Err(fail("unready immediate logic operand/control"));
                    }
                    Operation::LogicImmediate {
                        dst,
                        src,
                        bits: (word >> 20) as u32,
                        operator,
                        invert_src: word & (1 << 55) != 0,
                        invert_immediate: word & (1 << 56) != 0,
                    }
                } else if word & !0x000007800ff0ffff == 0x5c47000000070000 {
                    let left = ((word >> 8) & 255) as u8;
                    let right = ((word >> 20) & 255) as u8;
                    let invert_left = word & (1 << 39) != 0;
                    let invert_right = word & (1 << 40) != 0;
                    let operator = match (word >> 41) & 3 {
                        0 => LogicalOperator::And,
                        1 => LogicalOperator::Or,
                        2 => LogicalOperator::Xor,
                        _ => LogicalOperator::PassB,
                    };
                    // PASS_B ignores A in hardware. Accept only the independently
                    // assembled canonical RZ form, including NOT through ~B.
                    if operator == LogicalOperator::PassB && (left != 255 || invert_left) {
                        return Err(fail("unsupported instruction: noncanonical LOP PASS_B"));
                    }
                    if write_barrier != 7
                        || !initialized[left as usize]
                        || !initialized[right as usize]
                        || pending[left as usize]
                        || pending[right as usize]
                    {
                        return Err(fail("unready integer operand/control"));
                    }
                    // Preserve the original public IR for plain binary forms.
                    // All forms use common Integer metadata, so identities and
                    // complements retain float hazards and erase resource proof.
                    let operator = if !invert_left && !invert_right {
                        match operator {
                            LogicalOperator::And => IntegerOperator::And,
                            LogicalOperator::Or => IntegerOperator::Or,
                            LogicalOperator::Xor => IntegerOperator::Xor,
                            LogicalOperator::PassB => IntegerOperator::Logical {
                                operator,
                                invert_left,
                                invert_right,
                            },
                        }
                    } else {
                        IntegerOperator::Logical {
                            operator,
                            invert_left,
                            invert_right,
                        }
                    };
                    Operation::Integer {
                        dst,
                        left,
                        right,
                        operator,
                    }
                } else if matches!(
                    word & !0x0100007ffff0ffff,
                    0x3858000000070000 | 0x3868000000070000
                ) || matches!(
                    word & !0x000ffffffff0ffff,
                    0x0800000000070000 | 0x1e00000000070000
                ) {
                    let src = ((word >> 8) & 255) as u8;
                    if write_barrier != 7 || !initialized[src as usize] || pending[src as usize] {
                        return Err(fail("unready floating ALU operand or unsupported schedule"));
                    }
                    let (operator, bits) = if matches!(
                        word & !0x000ffffffff0ffff,
                        0x0800000000070000 | 0x1e00000000070000
                    ) {
                        // F32_20 occupies bits51:20, including the positions
                        // used for rounding and other modifiers in short forms.
                        let operator = if word & !0x000ffffffff0ffff == 0x0800000000070000 {
                            FloatOperator::Add
                        } else {
                            FloatOperator::Multiply
                        };
                        (operator, ((word >> 20) & 0xffff_ffff) as u32)
                    } else {
                        let operator = if word & !0x0100007ffff0ffff == 0x3858000000070000 {
                            FloatOperator::Add
                        } else {
                            FloatOperator::Multiply
                        };
                        // F20_20 contains f32 bits31:12, split between sign bit56
                        // and payload38:20. It is not S20 sign extension.
                        let bits = (((word >> 20) & 0x7ffff) as u32) << 12
                            | (((word >> 56) & 1) as u32) << 31;
                        (operator, bits)
                    };
                    if operator == FloatOperator::Add && multiply_result[src as usize] {
                        return Err(fail("FMUL-derived FADD requires unfused rounding control"));
                    }
                    Operation::FloatImmediate {
                        dst,
                        src,
                        bits,
                        operator,
                    }
                } else if word & !0x00007f800ff0ffff == 0x5bc0000000070000
                    || word & !0x00007f8ffff0ffff == 0x36c0000000070000
                {
                    let left = ((word >> 8) & 255) as u8;
                    let right = ((word >> 39) & 255) as u8;
                    let selector = if word >> 52 == 0x5bc {
                        PermutationSelector::Register(((word >> 20) & 255) as u8)
                    } else {
                        PermutationSelector::Immediate(((word >> 20) & 0xffff) as u16)
                    };
                    let control = match selector {
                        PermutationSelector::Register(r) => r,
                        PermutationSelector::Immediate(_) => 255,
                    };
                    if write_barrier != 7
                        || [left, right, control]
                            .iter()
                            .any(|r| !initialized[*r as usize] || pending[*r as usize])
                    {
                        return Err(fail("unready PRMT operand or unsupported schedule"));
                    }
                    Operation::PermuteBytes {
                        dst,
                        left,
                        right,
                        selector,
                    }
                } else if word & !0x00000f800ff0ffff == 0x5c18000000070000 {
                    let left = ((word >> 8) & 255) as u8;
                    let right = ((word >> 20) & 255) as u8;
                    if write_barrier != 7
                        || [left, right]
                            .iter()
                            .any(|r| !initialized[*r as usize] || pending[*r as usize])
                    {
                        return Err(fail("unready ISCADD operand or unsupported schedule"));
                    }
                    // ISCADD bit43 is the high shift bit, not IADD's X flag.
                    Operation::ScaledAdd {
                        dst,
                        left,
                        right,
                        shift: ((word >> 39) & 31) as u8,
                    }
                } else if word & !0x000f10000ff0ffff == 0x5b50038000070000 {
                    let comparison = match ((word >> 49) & 7, (word >> 48) & 1) {
                        (1, 0) => IntegerComparison::Less,
                        (2, 0) => IntegerComparison::Equal,
                        (3, 0) => IntegerComparison::LessEqual,
                        (4, 0) => IntegerComparison::Greater,
                        (5, 0) => IntegerComparison::NotEqual,
                        (6, 0) => IntegerComparison::GreaterEqual,
                        (1, 1) => IntegerComparison::LessSigned,
                        (2, 1) => IntegerComparison::EqualSigned,
                        (3, 1) => IntegerComparison::LessEqualSigned,
                        (4, 1) => IntegerComparison::GreaterSigned,
                        (5, 1) => IntegerComparison::NotEqualSigned,
                        (6, 1) => IntegerComparison::GreaterEqualSigned,
                        _ => return Err(fail("unsupported ISET comparison")),
                    };
                    let left = ((word >> 8) & 255) as u8;
                    let right = ((word >> 20) & 255) as u8;
                    if write_barrier != 7
                        || [left, right]
                            .iter()
                            .any(|r| !initialized[*r as usize] || pending[*r as usize])
                    {
                        return Err(fail("unready ISET operand or unsupported schedule"));
                    }
                    // ISET has a GPR destination and BF at44, not FSET's52.
                    Operation::IntegerSet {
                        dst,
                        left,
                        right,
                        comparison,
                        float_result: word & (1 << 44) != 0,
                    }
                } else if word & !0x001f00000ff0ffff == 0x5800038000070000 {
                    let comparison = match (word >> 48) & 15 {
                        0 => FloatComparison::False,
                        1 => FloatComparison::Less,
                        2 => FloatComparison::Equal,
                        3 => FloatComparison::LessEqual,
                        4 => FloatComparison::Greater,
                        5 => FloatComparison::NotEqual,
                        6 => FloatComparison::GreaterEqual,
                        7 => FloatComparison::Numeric,
                        8 => FloatComparison::Nan,
                        9 => FloatComparison::LessUnordered,
                        10 => FloatComparison::EqualUnordered,
                        11 => FloatComparison::LessEqualUnordered,
                        12 => FloatComparison::GreaterUnordered,
                        13 => FloatComparison::NotEqualUnordered,
                        14 => FloatComparison::GreaterEqualUnordered,
                        _ => FloatComparison::True, // Remaining four-bit mode is 15.
                    };
                    let left = ((word >> 8) & 255) as u8;
                    let right = ((word >> 20) & 255) as u8;
                    if write_barrier != 7
                        || [left, right]
                            .iter()
                            .any(|r| !initialized[*r as usize] || pending[*r as usize])
                    {
                        return Err(fail("unready FSET operand or unsupported schedule"));
                    }
                    // Low byte is a real GPR destination. P0 stays untouched.
                    Operation::FloatSet {
                        dst,
                        left,
                        right,
                        comparison,
                        float_result: word & (1 << 52) != 0,
                    }
                } else if matches!(
                    word & !0x000000000ff000ff,
                    0x5ce0000000070200
                        | 0x5ce0000000070600
                        | 0x5ce0000000073200
                        | 0x5ce0000000073600
                ) {
                    // Only U8/U16 -> U32 and S8/S16 -> S32, low source lane.
                    // Bits15:8 are types; the source GPR is bits27:20.
                    let src = ((word >> 20) & 255) as u8;
                    if write_barrier != 0
                        || pending.iter().any(|v| *v)
                        || !initialized[src as usize]
                    {
                        return Err(fail(
                            "I2I requires ready source and unused write barrier zero",
                        ));
                    }
                    pending[dst as usize] = true;
                    Operation::IntWiden {
                        dst,
                        src,
                        bits: if word & (1 << 10) == 0 { 8 } else { 16 },
                        signed: word & (1 << 13) != 0,
                    }
                } else if word & !0x000000000ff020ff == 0x5cb8000000070a00 {
                    // Bits 15:8 describe destination/source types, not a GPR.
                    // Only F32 <- U32/S32, RN, ordinary GPR/RZ is supported.
                    let src = ((word >> 20) & 255) as u8;
                    if write_barrier != 0
                        || pending.iter().any(|v| *v)
                        || !initialized[src as usize]
                    {
                        return Err(fail(
                            "I2F requires ready source and unused write barrier zero",
                        ));
                    }
                    pending[dst as usize] = true;
                    Operation::IntToFloat {
                        dst,
                        src,
                        signed: word & (1 << 13) != 0,
                    }
                } else if word & !0x000100000ff000ff == 0x5c30000000070000 {
                    let src = ((word >> 20) & 255) as u8;
                    if write_barrier != 0
                        || pending.iter().any(|v| *v)
                        || !initialized[src as usize]
                    {
                        return Err(fail(
                            "FLO requires ready source and unused write barrier zero",
                        ));
                    }
                    pending[dst as usize] = true;
                    Operation::LeadingBit {
                        dst,
                        src,
                        signed: word & (1 << 48) != 0,
                    }
                } else if word & !0x000001000ff000ff == 0x5c08000000070000 {
                    let src = ((word >> 20) & 255) as u8;
                    // POPC is scoreboarded on SM50. A stall count alone does
                    // not establish readiness, including for RZ or aliases.
                    if write_barrier != 0
                        || pending.iter().any(|v| *v)
                        || !initialized[src as usize]
                    {
                        return Err(fail(
                            "POPC requires ready source and unused write barrier zero",
                        ));
                    }
                    pending[dst as usize] = true;
                    Operation::CountBits {
                        dst,
                        src,
                        invert: word & (1 << 40) != 0,
                    }
                } else if word & !0x00007f8ffff0ffff == 0x5be7000000070000 {
                    let first = ((word >> 8) & 255) as u8;
                    let second = ((word >> 20) & 255) as u8;
                    let third = ((word >> 39) & 255) as u8;
                    if write_barrier != 7
                        || [first, second, third]
                            .iter()
                            .any(|r| !initialized[*r as usize] || pending[*r as usize])
                    {
                        return Err(fail("unready LOP3 operand or unsupported schedule"));
                    }
                    Operation::TernaryLogic {
                        dst,
                        first,
                        second,
                        third,
                        lut: ((word >> 28) & 255) as u8,
                    }
                } else if word & !0x0001000ffff0ffff == 0x3800000000070000 {
                    let src = ((word >> 8) & 255) as u8;
                    let offset = ((word >> 20) & 255) as u8;
                    let width = ((word >> 28) & 255) as u8;
                    if offset >= 32 || width > 32 || u16::from(offset) + u16::from(width) > 32 {
                        return Err(fail("unsupported BFE field outside source word"));
                    }
                    if write_barrier != 7 || !initialized[src as usize] || pending[src as usize] {
                        return Err(fail("unready BFE source or unsupported schedule"));
                    }
                    Operation::BitfieldExtract {
                        dst,
                        src,
                        offset,
                        width,
                        signed: word & (1 << 48) != 0,
                    }
                } else if matches!(
                    word & !(255 | (255u64 << 8) | (255u64 << 20)),
                    0x5c58000000070000 | 0x5c68000000070000
                ) {
                    let left = ((word >> 8) & 255) as u8;
                    let right = ((word >> 20) & 255) as u8;
                    if write_barrier != 7
                        || [left, right]
                            .iter()
                            .any(|r| !initialized[*r as usize] || pending[*r as usize])
                    {
                        return Err(fail("unready floating ALU operand or unsupported schedule"));
                    }
                    if word >> 48 == 0x5c58
                        && (multiply_result[left as usize] || multiply_result[right as usize])
                    {
                        return Err(fail("FMUL-derived FADD requires unfused rounding control"));
                    }
                    Operation::Float {
                        dst,
                        left,
                        right,
                        operator: if word >> 48 == 0x5c58 {
                            FloatOperator::Add
                        } else {
                            FloatOperator::Multiply
                        },
                    }
                } else if word & !(255 | (255u64 << 8) | (255u64 << 20) | (3u64 << 40))
                    == 0x5c38000000070000
                {
                    let left = ((word >> 8) & 255) as u8;
                    let right = ((word >> 20) & 255) as u8;
                    // SM50 integer multiplication has variable latency. Six
                    // stalls never replace assigning and waiting on barrier zero.
                    if write_barrier != 0
                        || pending.iter().any(|v| *v)
                        || [left, right].iter().any(|r| !initialized[*r as usize])
                    {
                        return Err(fail(
                            "IMUL requires ready operands and unused write barrier zero",
                        ));
                    }
                    pending[dst as usize] = true;
                    // Signed/unsigned source interpretations have identical low
                    //32 product bits. HI and all other modes remain rejected.
                    Operation::Integer {
                        dst,
                        left,
                        right,
                        operator: IntegerOperator::Multiply,
                    }
                } else if matches!(
                    word & !(255 | (255u64 << 8) | (255u64 << 20)),
                    0x5c10000000070000
                        | 0x5c11000000070000
                        | 0x5c12000000070000
                        | 0x5c48000000070000
                        | 0x5c48008000070000
                        | 0x5c28000000070000
                        | 0x5c28008000070000
                        | 0x5c29000000070000
                        | 0x5c29008000070000
                        | 0x5c20038000070000
                        | 0x5c20078000070000
                        | 0x5c21038000070000
                        | 0x5c21078000070000
                ) {
                    let mut left = ((word >> 8) & 255) as u8;
                    let mut right = ((word >> 20) & 255) as u8;
                    if write_barrier != 7
                        || [left, right]
                            .iter()
                            .any(|r| !initialized[*r as usize] || pending[*r as usize])
                    {
                        return Err(fail("unready integer ALU operand or unsupported schedule"));
                    }
                    // Negating source A computes B-A, source B computes A-B.
                    // Both bits encode the distinct, unsupported IADD.PO form.
                    if word & !(255 | (255u64 << 8) | (255u64 << 20)) == 0x5c12000000070000 {
                        std::mem::swap(&mut left, &mut right);
                    }
                    Operation::Integer {
                        dst,
                        left,
                        right,
                        operator: match word & !(255 | (255u64 << 8) | (255u64 << 20)) {
                            0x5c10000000070000 => IntegerOperator::Add,
                            0x5c11000000070000 | 0x5c12000000070000 => IntegerOperator::Subtract,
                            0x5c48000000070000 => IntegerOperator::ShiftLeft { wrap: false },
                            0x5c48008000070000 => IntegerOperator::ShiftLeft { wrap: true },
                            0x5c28000000070000 => IntegerOperator::ShiftRight { wrap: false },
                            0x5c28008000070000 => IntegerOperator::ShiftRight { wrap: true },
                            0x5c29000000070000 => IntegerOperator::ShiftRightSigned { wrap: false },
                            0x5c29008000070000 => IntegerOperator::ShiftRightSigned { wrap: true },
                            0x5c20038000070000 => IntegerOperator::MinUnsigned,
                            0x5c20078000070000 => IntegerOperator::MaxUnsigned,
                            0x5c21038000070000 => IntegerOperator::MinSigned,
                            _ => IntegerOperator::MaxSigned,
                        },
                    }
                } else if !vertex && word & !ldc_mask == 0xef9400000007ff00 {
                    if write_barrier != 0 || pending.iter().any(|v| *v) {
                        return Err(fail("LDC requires an unused write barrier zero"));
                    }
                    let offset = ((word >> 20) & 0xffff) as u32;
                    if offset & 0x8003 != 0 {
                        return Err(fail("unaligned or negative LDC offset"));
                    }
                    let reference = ConstantRef {
                        slot: ((word >> 36) & 31) as u8,
                        offset,
                    };
                    let index = constant_input(&mut out.constants, reference)
                        .ok_or_else(|| fail("too many constant references"))?;
                    pending[dst as usize] = true;
                    Operation::Constant { dst, input: index }
                } else if perspective && word & !255 == 0xe003ff87cff7ff00 {
                    if write_barrier != 0 || pending.iter().any(|v| *v) {
                        return Err(fail("PositionW requires unused barrier zero"));
                    }
                    pending[dst as usize] = true;
                    Operation::PositionW { dst }
                } else if perspective && word & !(255 | (255u64 << 8)) == 0x5080000000470000 {
                    let src = ((word >> 8) & 255) as u8;
                    if write_barrier != 0 || pending.iter().any(|v| *v) || !position_w[src as usize]
                    {
                        return Err(fail(
                            "MUFU.RCP requires ready PositionW provenance and barrier zero",
                        ));
                    }
                    pending[dst as usize] = true;
                    Operation::Reciprocal { dst, src }
                } else if perspective
                    && word & !(255 | (255u64 << 20) | (0x3ffu64 << 28)) == 0xe043ff800007ff00
                {
                    let attribute = ((word >> 28) & 0x3ff) as u16;
                    let multiplier = ((word >> 20) & 255) as u8;
                    if !matches!(attribute, 0x80 | 0x84)
                        || write_barrier != 0
                        || pending.iter().any(|v| *v)
                        || !reciprocal_w[multiplier as usize]
                    {
                        return Err(fail("perspective IPA requires ready PositionW reciprocal"));
                    }
                    pending[dst as usize] = true;
                    Operation::Perspective {
                        dst,
                        component: ((attribute - 0x80) / 4) as u8,
                        multiplier,
                    }
                } else if !vertex && word & !(255 | (0x3ffu64 << 28)) == 0xe003ff800ff7ff00 {
                    let attribute = ((word >> 28) & 0x3ff) as u32;
                    if !interpolated
                        || !matches!(attribute, 0x80 | 0x84)
                        || write_barrier != 0
                        || pending.iter().any(|v| *v)
                    {
                        return Err(fail("unsupported IPA input or scoreboard"));
                    }
                    pending[dst as usize] = true;
                    Operation::Interpolate {
                        dst,
                        component: ((attribute - 0x80) / 4) as u8,
                    }
                } else if !vertex
                    && word & !(255 | (255u64 << 8) | (255u64 << 20)) == 0xdeb80027a0070000
                {
                    let coordinates = ((word >> 8) & 255) as u8;
                    let handle = ((word >> 20) & 255) as u8;
                    if dst > 251
                        || coordinates > 253
                        || write_barrier != 0
                        || pending.iter().any(|v| *v)
                        || !initialized[coordinates as usize]
                        || !initialized[coordinates as usize + 1]
                        || !initialized[handle as usize]
                    {
                        return Err(fail("unsupported TEX register group or scoreboard"));
                    }
                    let input = provenance[handle as usize].ok_or_else(|| {
                        fail("TEX handle must originate in a constant-buffer load")
                    })?;
                    if out.texture_input.is_some() {
                        return Err(fail("only one TEX operation supported"));
                    }
                    out.texture_input = Some(input);
                    for r in dst..dst + 4 {
                        initialized[r as usize] = true;
                        pending[r as usize] = true;
                        provenance[r as usize] = None;
                        multiply_result[r as usize] = false;
                        position_w[r as usize] = false;
                        reciprocal_w[r as usize] = false;
                    }
                    Operation::Sample2d {
                        dst,
                        coordinates,
                        handle,
                    }
                } else {
                    return Err(fail("unsupported instruction/modifier/predicate"));
                };
                if pending[dst as usize]
                    && !matches!(
                        op,
                        Operation::Constant { .. }
                            | Operation::CountBits { .. }
                            | Operation::LeadingBit { .. }
                            | Operation::IntToFloat { .. }
                            | Operation::IntWiden { .. }
                            | Operation::Integer {
                                operator: IntegerOperator::Multiply,
                                ..
                            }
                            | Operation::AttributeRead { .. }
                            | Operation::Interpolate { .. }
                            | Operation::PositionW { .. }
                            | Operation::Reciprocal { .. }
                            | Operation::Perspective { .. }
                            | Operation::Sample2d { .. }
                    )
                {
                    return Err(fail("write to pending register"));
                }
                multiply_result[dst as usize] = match op {
                    Operation::PermuteBytes {
                        left,
                        right,
                        selector,
                        ..
                    } => {
                        multiply_result[left as usize]
                            || multiply_result[right as usize]
                            || match selector {
                                PermutationSelector::Register(r) => multiply_result[r as usize],
                                PermutationSelector::Immediate(_) => false,
                            }
                    }
                    // Integer identities can disappear in host optimization.
                    // Retain the unfused-rounding hazard even across bit operations.
                    Operation::Select { left, right, .. } => {
                        multiply_result[left as usize]
                            || multiply_result[right as usize]
                            || predicate_multiply
                    }
                    Operation::ConditionalMove { dst, src, .. } => {
                        multiply_result[dst as usize]
                            || multiply_result[src as usize]
                            || predicate_multiply
                    }
                    Operation::Integer { left, right, .. }
                    | Operation::FloatSet { left, right, .. }
                    | Operation::IntegerSet { left, right, .. }
                    | Operation::ScaledAdd { left, right, .. } => {
                        multiply_result[left as usize] || multiply_result[right as usize]
                    }
                    Operation::TernaryLogic {
                        first,
                        second,
                        third,
                        ..
                    } => {
                        multiply_result[first as usize]
                            || multiply_result[second as usize]
                            || multiply_result[third as usize]
                    }
                    Operation::Float {
                        operator: FloatOperator::Multiply,
                        ..
                    }
                    | Operation::FloatImmediate {
                        operator: FloatOperator::Multiply,
                        ..
                    } => true,
                    Operation::Move { src, .. }
                    | Operation::CountBits { src, .. }
                    | Operation::LeadingBit { src, .. }
                    | Operation::IntToFloat { src, .. }
                    | Operation::IntWiden { src, .. }
                    | Operation::BitfieldExtract { src, .. }
                    | Operation::LogicImmediate { src, .. }
                    | Operation::AddImmediate { src, .. } => multiply_result[src as usize],
                    _ => false,
                };
                position_w[dst as usize] = match op {
                    Operation::PositionW { .. } => true,
                    Operation::Move { src, .. } => position_w[src as usize],
                    _ => false,
                };
                reciprocal_w[dst as usize] = match op {
                    Operation::Reciprocal { .. } => true,
                    Operation::Move { src, .. } => reciprocal_w[src as usize],
                    _ => false,
                };
                provenance[dst as usize] = match op {
                    Operation::Constant { input, .. } => Some(input),
                    Operation::Move { src, .. } => provenance[src as usize],
                    _ => None,
                };
                initialized[dst as usize] = true;
                out.operations.push(op);
            }
        }
        if !exited {
            return Err(invalid("fragment missing EXIT"));
        }
        Ok(out)
    }
    pub fn required_registers(&self) -> u16 {
        let mut largest = 3;
        for op in &self.operations {
            match *op {
                Operation::PermuteBytes {
                    dst,
                    left,
                    right,
                    selector,
                } => {
                    largest = largest.max(u16::from(dst));
                    let control = match selector {
                        PermutationSelector::Register(r) => r,
                        PermutationSelector::Immediate(_) => 255,
                    };
                    for r in [left, right, control] {
                        if r != 255 {
                            largest = largest.max(u16::from(r));
                        }
                    }
                }
                Operation::AttributeRead { dst, words, .. } => {
                    largest = largest.max(u16::from(dst) + u16::from(words) - 1)
                }
                Operation::AttributeWrite { src, words, .. } => {
                    if src != 255 {
                        largest = largest.max(u16::from(src) + u16::from(words) - 1)
                    }
                }
                Operation::Immediate { dst, .. }
                | Operation::Constant { dst, .. }
                | Operation::PositionW { dst }
                | Operation::Interpolate { dst, .. } => largest = largest.max(u16::from(dst)),
                Operation::Reciprocal { dst, src } => {
                    largest = largest.max(u16::from(dst)).max(u16::from(src));
                }
                Operation::Perspective {
                    dst, multiplier, ..
                } => {
                    largest = largest.max(u16::from(dst)).max(u16::from(multiplier));
                }
                Operation::PredicateCompare { left, right, .. }
                | Operation::FloatPredicateCompare { left, right, .. } => {
                    for r in [left, right] {
                        if r != 255 {
                            largest = largest.max(u16::from(r));
                        }
                    }
                }
                Operation::Integer {
                    dst, left, right, ..
                }
                | Operation::Select {
                    dst, left, right, ..
                }
                | Operation::Float {
                    dst, left, right, ..
                }
                | Operation::FloatSet {
                    dst, left, right, ..
                }
                | Operation::IntegerSet {
                    dst, left, right, ..
                }
                | Operation::ScaledAdd {
                    dst, left, right, ..
                } => {
                    largest = largest.max(u16::from(dst));
                    for r in [left, right] {
                        if r != 255 {
                            largest = largest.max(u16::from(r));
                        }
                    }
                }
                Operation::TernaryLogic {
                    dst,
                    first,
                    second,
                    third,
                    ..
                } => {
                    largest = largest.max(u16::from(dst));
                    for r in [first, second, third] {
                        if r != 255 {
                            largest = largest.max(u16::from(r));
                        }
                    }
                }
                Operation::Move { dst, src }
                | Operation::CountBits { dst, src, .. }
                | Operation::LeadingBit { dst, src, .. }
                | Operation::IntToFloat { dst, src, .. }
                | Operation::IntWiden { dst, src, .. }
                | Operation::BitfieldExtract { dst, src, .. }
                | Operation::ConditionalMove { dst, src, .. }
                | Operation::LogicImmediate { dst, src, .. }
                | Operation::AddImmediate { dst, src, .. }
                | Operation::FloatImmediate { dst, src, .. } => {
                    largest = largest.max(u16::from(dst));
                    if src != 255 {
                        largest = largest.max(u16::from(src));
                    }
                }
                Operation::Sample2d {
                    dst,
                    coordinates,
                    handle,
                } => {
                    largest = largest
                        .max(u16::from(dst) + 3)
                        .max(u16::from(coordinates) + 1)
                        .max(u16::from(handle))
                }
            }
        }
        largest + 1
    }
    pub fn texture_input(&self) -> Option<u8> {
        self.texture_input
    }
    pub fn constants(&self) -> &[ConstantRef] {
        &self.constants
    }
    pub fn operations(&self) -> &[Operation] {
        &self.operations
    }
    pub fn wgsl(&self) -> String {
        use std::fmt::Write;
        let mut s = stage_source(
            &self.operations,
            "struct Constants { words: array<vec4<u32>,16> };\n@group(0) @binding(0) var<uniform> cb: Constants;\n",
        );
        if self.texture_input.is_some() {
            s.push_str("@group(1) @binding(0) var image:texture_2d<f32>; @group(1) @binding(1) var image_sampler:sampler;\n");
        }
        s.push_str("struct Vertex { @builtin(position) position:vec4<f32>, @location(0) @interpolate(linear) uv:vec2<f32> };\n@vertex fn vs_main(@builtin(vertex_index) i:u32)->Vertex { let ps=array<vec2<f32>,3>(vec2(-1.,-1.),vec2(3.,-1.),vec2(-1.,3.));let p=ps[i];var v:Vertex;v.position=vec4(p,0.,1.);v.uv=vec2((p.x+1.)*.5,(1.-p.y)*.5);return v;}\n@fragment fn fs_main(v:Vertex)->@location(0) vec4<f32>{var r:array<u32,256>;\n");
        if self.operations.iter().any(|op| {
            matches!(
                op,
                Operation::PredicateCompare { .. }
                    | Operation::FloatPredicateCompare { .. }
                    | Operation::Select { .. }
                    | Operation::ConditionalMove { .. }
            )
        }) {
            s.push_str("var p0:bool;\n");
        }
        for op in &self.operations {
            match *op {
                Operation::AttributeRead { .. } | Operation::AttributeWrite { .. } => {
                    unreachable!("vertex operation in fragment")
                }
                Operation::Interpolate { dst, component } => {
                    writeln!(s, "r[{dst}]=bitcast<u32>(v.uv[{component}]);").unwrap()
                }
                Operation::PositionW { dst } => {
                    writeln!(s, "r[{dst}]=bitcast<u32>(v.position.w);").unwrap()
                }
                Operation::Reciprocal { dst, src } => {
                    writeln!(s, "r[{dst}]=bitcast<u32>(1.0 / bitcast<f32>(r[{src}]));").unwrap()
                }
                Operation::Perspective { dst, component, multiplier } => {
                    writeln!(s, "r[{dst}]=bitcast<u32>(v.uv[{component}] * v.position.w * bitcast<f32>(r[{multiplier}]));").unwrap()
                }
                Operation::Sample2d {
                    dst, coordinates, ..
                } => {
                    writeln!(s,"let sampled=textureSampleLevel(image,image_sampler,vec2(bitcast<f32>(r[{coordinates}]),bitcast<f32>(r[{}])),0.);",coordinates+1).unwrap();
                    for c in 0..4 {
                        writeln!(s, "r[{}]=bitcast<u32>(sampled[{c}]);", dst + c).unwrap();
                    }
                }

                Operation::PredicateCompare { left, right, comparison } => emit_comparison(&mut s, left, right, comparison),
                Operation::FloatPredicateCompare { left, right, comparison } => {
                    emit_float_comparison(&mut s, left, right, comparison);
                }
                Operation::Select { dst, left, right, invert } => {
                    let condition = if invert { "!p0" } else { "p0" };
                    writeln!(s, "r[{dst}]=select(r[{right}],r[{left}],{condition});").unwrap();
                }
                Operation::Float {
                    dst,
                    left,
                    right,
                    operator,
                } => {
                    let symbol = match operator {
                        FloatOperator::Add => "+",
                        FloatOperator::Multiply => "*",
                    };
                    writeln!(s,"r[{dst}]=bitcast<u32>(bitcast<f32>(r[{left}]) {symbol} bitcast<f32>(r[{right}]));").unwrap();
                }
                Operation::FloatImmediate { dst, src, bits, operator } => {
                    emit_float_immediate(&mut s, dst, src, bits, operator);
                }
                Operation::Integer { dst, left, right, operator } => {
                    emit_integer(&mut s, dst, left, right, operator);
                }
                Operation::LogicImmediate { dst, src, bits, operator, invert_src, invert_immediate } => {
                    emit_logic_immediate(&mut s, dst, src, bits, operator, invert_src, invert_immediate);
                }
                Operation::AddImmediate { dst, src, bits, negate_src } => {
                    emit_add_immediate(&mut s, dst, src, bits, negate_src);
                }
                Operation::BitfieldExtract { dst, src, offset, width, signed } => {
                    emit_bitfield(&mut s, dst, src, offset, width, signed);
                }
                Operation::CountBits { dst, src, invert } => {
                    emit_count_bits(&mut s, dst, src, invert);
                }
                Operation::LeadingBit { dst, src, signed } => {
                    emit_leading_bit(&mut s, dst, src, signed);
                }
                Operation::IntToFloat { dst, src, signed } => {
                    emit_int_to_float(&mut s, dst, src, signed);
                }
                Operation::IntWiden { dst, src, bits, signed } => {
                    emit_int_widen(&mut s, dst, src, bits, signed);
                }
                Operation::PermuteBytes { dst, left, right, selector } => {
                    emit_byte_permutation(&mut s, dst, left, right, selector);
                }
                Operation::FloatSet { dst, left, right, comparison, float_result } => {
                    emit_float_set(&mut s, dst, left, right, comparison, float_result);
                }
                Operation::IntegerSet { dst, left, right, comparison, float_result } => {
                    emit_integer_set(&mut s, dst, left, right, comparison, float_result);
                }
                Operation::TernaryLogic { dst, first, second, third, lut } => {
                    emit_ternary(&mut s, dst, first, second, third, lut);
                }
                Operation::ScaledAdd { dst, left, right, shift } => {
                    emit_scaled_add(&mut s, dst, left, right, shift);
                }
                Operation::Immediate { dst, bits } => writeln!(s, "r[{dst}]={bits}u;").unwrap(),
                Operation::Move { dst, src } => writeln!(s, "r[{dst}]=r[{src}];").unwrap(),
                Operation::ConditionalMove { dst, src, invert } => {
                    emit_conditional_move(&mut s, dst, src, invert);
                }
                Operation::Constant { dst, input } => {
                    writeln!(s, "r[{dst}]=cb.words[{}][{}];", input / 4, input % 4).unwrap()
                }
            }
        }
        s.push_str("return vec4(bitcast<f32>(r[0]),bitcast<f32>(r[1]),bitcast<f32>(r[2]),bitcast<f32>(r[3]));}");
        if self.perspective {
            s = s.replace("@interpolate(linear)", "@interpolate(perspective,center)");
        }
        s
    }
}

/// Strict PS interface; retained independently from vertex-header acceptance.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FragmentProgram(Program);
impl FragmentProgram {
    pub fn perspective(&self) -> bool {
        self.0.perspective
    }
    pub fn decode(input: &[u8]) -> Result<Self> {
        Ok(Self(Program::decode(input, false)?))
    }
    pub fn required_registers(&self) -> u16 {
        self.0.required_registers()
    }
    pub fn texture_input(&self) -> Option<u8> {
        self.0.texture_input()
    }
    pub fn constants(&self) -> &[ConstantRef] {
        self.0.constants()
    }
    /// B64 base references retained for full-pair binding validation before the
    /// scalar micro-ops execute. Each requires an aligned contiguous eight bytes.
    pub fn constant_pairs(&self) -> &[ConstantRef] {
        &self.0.constant_pairs
    }
    pub fn operations(&self) -> &[Operation] {
        self.0.operations()
    }
    pub fn wgsl(&self) -> String {
        self.0.wgsl()
    }
}
/// VTG vertex stage: generic-zero float4 and generic-one float2 input,
/// position xyzw and generic-zero xy output. Other interfaces fail explicitly.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VertexProgram(Program);
impl VertexProgram {
    pub fn decode(input: &[u8]) -> Result<Self> {
        Ok(Self(Program::decode(input, true)?))
    }
    pub fn required_registers(&self) -> u16 {
        self.0.required_registers()
    }
    pub fn operations(&self) -> &[Operation] {
        self.0.operations()
    }
    pub fn wgsl(&self) -> String {
        self.wgsl_for_raster(false, false)
    }
    pub(crate) fn wgsl_for_raster(&self, flip_y: bool, perspective: bool) -> String {
        use std::fmt::Write;
        let mut s = stage_source(
            self.operations(),
            "struct Vertex { @builtin(position) position:vec4<f32>, @location(0) @interpolate(linear) uv:vec2<f32> };\n@vertex fn vs_main(@location(0) a0:vec4<f32>,@location(1) a1:vec2<f32>)->Vertex {var r:array<u32,256>;var v:Vertex;\n",
        );
        if self.operations().iter().any(|op| {
            matches!(
                op,
                Operation::PredicateCompare { .. }
                    | Operation::FloatPredicateCompare { .. }
                    | Operation::Select { .. }
                    | Operation::ConditionalMove { .. }
            )
        }) {
            s.push_str("var p0:bool;\n");
        }
        for op in self.operations() {
            match *op {
                Operation::AttributeRead {
                    dst,
                    attribute,
                    words,
                } => {
                    for c in 0..words {
                        let (a, n) = if attribute < 0x90 {
                            (0, (attribute - 0x80) / 4)
                        } else {
                            (1, (attribute - 0x90) / 4)
                        };
                        writeln!(
                            s,
                            "r[{}]=bitcast<u32>(a{a}[{}]);",
                            u16::from(dst) + u16::from(c),
                            n + u16::from(c)
                        )
                        .unwrap();
                    }
                }
                Operation::AttributeWrite {
                    src,
                    attribute,
                    words,
                } => {
                    for c in 0..words {
                        let (name, n) = if attribute < 0x80 {
                            ("position", (attribute - 0x70) / 4)
                        } else {
                            ("uv", (attribute - 0x80) / 4)
                        };
                        writeln!(
                            s,
                            "v.{name}[{}]=bitcast<f32>(r[{}]);",
                            n + u16::from(c),
                            u16::from(src) + u16::from(c)
                        )
                        .unwrap();
                    }
                }
                Operation::PredicateCompare {
                    left,
                    right,
                    comparison,
                } => emit_comparison(&mut s, left, right, comparison),
                Operation::FloatPredicateCompare {
                    left,
                    right,
                    comparison,
                } => emit_float_comparison(&mut s, left, right, comparison),
                Operation::Select {
                    dst,
                    left,
                    right,
                    invert,
                } => {
                    let condition = if invert { "!p0" } else { "p0" };
                    writeln!(s, "r[{dst}]=select(r[{right}],r[{left}],{condition});").unwrap();
                }
                Operation::Float {
                    dst,
                    left,
                    right,
                    operator,
                } => {
                    let symbol = match operator {
                        FloatOperator::Add => "+",
                        FloatOperator::Multiply => "*",
                    };
                    writeln!(s,"r[{dst}]=bitcast<u32>(bitcast<f32>(r[{left}]) {symbol} bitcast<f32>(r[{right}]));").unwrap();
                }
                Operation::FloatImmediate {
                    dst,
                    src,
                    bits,
                    operator,
                } => {
                    emit_float_immediate(&mut s, dst, src, bits, operator);
                }
                Operation::Integer {
                    dst,
                    left,
                    right,
                    operator,
                } => {
                    emit_integer(&mut s, dst, left, right, operator);
                }
                Operation::LogicImmediate {
                    dst,
                    src,
                    bits,
                    operator,
                    invert_src,
                    invert_immediate,
                } => {
                    emit_logic_immediate(
                        &mut s,
                        dst,
                        src,
                        bits,
                        operator,
                        invert_src,
                        invert_immediate,
                    );
                }
                Operation::AddImmediate {
                    dst,
                    src,
                    bits,
                    negate_src,
                } => {
                    emit_add_immediate(&mut s, dst, src, bits, negate_src);
                }
                Operation::BitfieldExtract {
                    dst,
                    src,
                    offset,
                    width,
                    signed,
                } => {
                    emit_bitfield(&mut s, dst, src, offset, width, signed);
                }
                Operation::TernaryLogic {
                    dst,
                    first,
                    second,
                    third,
                    lut,
                } => {
                    emit_ternary(&mut s, dst, first, second, third, lut);
                }
                Operation::CountBits { dst, src, invert } => {
                    emit_count_bits(&mut s, dst, src, invert);
                }
                Operation::LeadingBit { dst, src, signed } => {
                    emit_leading_bit(&mut s, dst, src, signed);
                }
                Operation::IntToFloat { dst, src, signed } => {
                    emit_int_to_float(&mut s, dst, src, signed);
                }
                Operation::IntWiden {
                    dst,
                    src,
                    bits,
                    signed,
                } => {
                    emit_int_widen(&mut s, dst, src, bits, signed);
                }
                Operation::PermuteBytes {
                    dst,
                    left,
                    right,
                    selector,
                } => {
                    emit_byte_permutation(&mut s, dst, left, right, selector);
                }
                Operation::FloatSet {
                    dst,
                    left,
                    right,
                    comparison,
                    float_result,
                } => {
                    emit_float_set(&mut s, dst, left, right, comparison, float_result);
                }
                Operation::IntegerSet {
                    dst,
                    left,
                    right,
                    comparison,
                    float_result,
                } => {
                    emit_integer_set(&mut s, dst, left, right, comparison, float_result);
                }
                Operation::ScaledAdd {
                    dst,
                    left,
                    right,
                    shift,
                } => {
                    emit_scaled_add(&mut s, dst, left, right, shift);
                }
                Operation::Immediate { dst, bits } => writeln!(s, "r[{dst}]={bits}u;").unwrap(),
                Operation::Move { dst, src } => writeln!(s, "r[{dst}]=r[{src}];").unwrap(),
                Operation::ConditionalMove { dst, src, invert } => {
                    emit_conditional_move(&mut s, dst, src, invert);
                }
                _ => unreachable!("resource/fragment operation in strict vertex program"),
            }
        }
        if flip_y {
            s.push_str("v.position.y = -v.position.y;\n");
        }
        s.push_str("return v;}");
        if perspective {
            s = s.replace("@interpolate(linear)", "@interpolate(perspective,center)");
        }
        s
    }
}

fn stage_source(operations: &[Operation], header: &str) -> String {
    // Global helper must precede both entry points; vertex headers already open
    // the function body. Existing shader text stays identical without helpers.
    let helper = if operations
        .iter()
        .any(|op| matches!(op, Operation::IntToFloat { .. }))
    {
        include_str!("i2f.wgsl")
    } else {
        ""
    };
    let permutation = if operations
        .iter()
        .any(|op| matches!(op, Operation::PermuteBytes { .. }))
    {
        include_str!("prmt.wgsl")
    } else {
        ""
    };
    let mut source = String::with_capacity(helper.len() + permutation.len() + header.len());
    source.push_str(helper);
    source.push_str(permutation);
    source.push_str(header);
    source
}

fn emit_byte_permutation(
    s: &mut String,
    dst: u8,
    left: u8,
    right: u8,
    selector: PermutationSelector,
) {
    use std::fmt::Write;
    // All function arguments are values, snapshotted before any aliased write.
    match selector {
        PermutationSelector::Immediate(control) => {
            writeln!(
                s,
                "r[{dst}]=permute_bytes(r[{left}],r[{right}],{control}u);"
            )
        }
        PermutationSelector::Register(control) => {
            writeln!(
                s,
                "r[{dst}]=permute_bytes(r[{left}],r[{right}],r[{control}]);"
            )
        }
    }
    .unwrap();
}

fn emit_int_widen(s: &mut String, dst: u8, src: u8, bits: u8, signed: bool) {
    use std::fmt::Write;
    // Decode restricts bits to8/16. Shift counts are bounded16/24; a signed
    // right shift sign-extends the low source lane after discarding upper bits.
    if signed {
        let shift = 32 - bits;
        writeln!(
            s,
            "r[{dst}]=bitcast<u32>(bitcast<i32>(r[{src}] << {shift}u) >> {shift}u);"
        )
        .unwrap();
    } else {
        let mask = (1u32 << bits) - 1;
        writeln!(s, "r[{dst}]=r[{src}] & {mask}u;").unwrap();
    }
}

fn emit_scaled_add(s: &mut String, dst: u8, left: u8, right: u8, shift: u8) {
    use std::fmt::Write;
    // Both operands are read before a potentially aliased destination is stored.
    // The decoded count is 0..31 and u32 arithmetic wraps modulo 2^32.
    writeln!(s, "r[{dst}]=(r[{left}] << {shift}u)+r[{right}];").unwrap();
}

fn emit_int_to_float(s: &mut String, dst: u8, src: u8, signed: bool) {
    use std::fmt::Write;
    // WGSL numeric conversion permits either adjacent representable float.
    // Integer-only packing makes Maxwell RN-even independent of that choice.
    writeln!(s, "r[{dst}]=wl_i2f_rn_bits(r[{src}],{signed});").unwrap();
}

fn emit_leading_bit(s: &mut String, dst: u8, src: u8, signed: bool) {
    use std::fmt::Write;
    if signed {
        // Preserve the signed builtin's -1 sentinel as raw u32, including
        // zero and all-ones inputs. This is not a numeric float conversion.
        writeln!(
            s,
            "r[{dst}]=bitcast<u32>(firstLeadingBit(bitcast<i32>(r[{src}])));"
        )
        .unwrap();
    } else {
        writeln!(s, "r[{dst}]=firstLeadingBit(r[{src}]);").unwrap();
    }
}

fn emit_count_bits(s: &mut String, dst: u8, src: u8, invert: bool) {
    use std::fmt::Write;
    let modifier = if invert { "~" } else { "" };
    writeln!(s, "r[{dst}]=countOneBits({modifier}r[{src}]);").unwrap();
}

fn emit_ternary(s: &mut String, dst: u8, first: u8, second: u8, third: u8, lut: u8) {
    use std::fmt::Write;
    // Snapshot every operand before writing the destination, which may alias
    // any of them. LUT bit index is (A_bit<<2)|(B_bit<<1)|C_bit.
    writeln!(
        s,
        "{{ let a=r[{first}]; let b=r[{second}]; let c=r[{third}];"
    )
    .unwrap();
    write!(s, "r[{dst}]=").unwrap();
    let mut any = false;
    for index in 0..8 {
        if lut & (1 << index) != 0 {
            let a = if index & 4 != 0 { "a" } else { "~a" };
            let b = if index & 2 != 0 { "b" } else { "~b" };
            let c = if index & 1 != 0 { "c" } else { "~c" };
            if any {
                s.push_str(" | ");
            }
            write!(s, "({a} & {b} & {c})").unwrap();
            any = true;
        }
    }
    if !any {
        s.push_str("0u");
    }
    s.push_str("; }\n");
}

fn emit_bitfield(s: &mut String, dst: u8, src: u8, offset: u8, width: u8, signed: bool) {
    use std::fmt::Write;
    if width == 0 {
        writeln!(s, "r[{dst}]=0u;").unwrap();
    } else if width == 32 {
        writeln!(s, "r[{dst}]=r[{src}];").unwrap();
    } else if signed {
        let left = 32 - offset - width;
        let right = 32 - width;
        writeln!(
            s,
            "r[{dst}]=bitcast<u32>(bitcast<i32>(r[{src}] << {left}u) >> {right}u);"
        )
        .unwrap();
    } else {
        let mask = u32::MAX >> (32 - width);
        writeln!(s, "r[{dst}]=(r[{src}] >> {offset}u) & {mask}u;").unwrap();
    }
}

fn emit_integer(s: &mut String, dst: u8, left: u8, right: u8, operator: IntegerOperator) {
    use std::fmt::Write;
    let (symbol, wrap) = match operator {
        IntegerOperator::Add => ("+", None),
        IntegerOperator::Multiply => ("*", None),
        IntegerOperator::Subtract => ("-", None),
        IntegerOperator::And => ("&", None),
        IntegerOperator::Or => ("|", None),
        IntegerOperator::Xor => ("^", None),
        IntegerOperator::Logical {
            operator,
            invert_left,
            invert_right,
        } => {
            let left = format!("{}r[{left}]", if invert_left { "~" } else { "" });
            let right = format!("{}r[{right}]", if invert_right { "~" } else { "" });
            let symbol = match operator {
                LogicalOperator::And => "&",
                LogicalOperator::Or => "|",
                LogicalOperator::Xor => "^",
                LogicalOperator::PassB => {
                    writeln!(s, "r[{dst}]={right};").unwrap();
                    return;
                }
            };
            writeln!(s, "r[{dst}]=({left}) {symbol} ({right});").unwrap();
            return;
        }
        IntegerOperator::MinUnsigned => {
            writeln!(s, "r[{dst}]=min(r[{left}],r[{right}]);").unwrap();
            return;
        }
        IntegerOperator::MaxUnsigned => {
            writeln!(s, "r[{dst}]=max(r[{left}],r[{right}]);").unwrap();
            return;
        }
        IntegerOperator::MinSigned | IntegerOperator::MaxSigned => {
            let function = if operator == IntegerOperator::MinSigned {
                "min"
            } else {
                "max"
            };
            writeln!(s, "r[{dst}]=bitcast<u32>({function}(bitcast<i32>(r[{left}]),bitcast<i32>(r[{right}])));").unwrap();
            return;
        }
        IntegerOperator::ShiftLeft { wrap } => ("<<", Some(wrap)),
        IntegerOperator::ShiftRight { wrap } => (">>", Some(wrap)),
        IntegerOperator::ShiftRightSigned { wrap } => {
            // Counts remain unsigned. Default >=32 is sign fill, equivalent
            // to arithmetic shift by31; every host count is explicitly bounded.
            if wrap {
                writeln!(
                    s,
                    "r[{dst}]=bitcast<u32>(bitcast<i32>(r[{left}]) >> (r[{right}] & 31u));"
                )
            } else {
                writeln!(
                    s,
                    "r[{dst}]=bitcast<u32>(bitcast<i32>(r[{left}]) >> min(r[{right}],31u));"
                )
            }
            .unwrap();
            return;
        }
    };
    match wrap {
        None => writeln!(s, "r[{dst}]=r[{left}] {symbol} r[{right}];"),
        Some(true) => writeln!(s, "r[{dst}]=r[{left}] {symbol} (r[{right}] & 31u);"),
        // WGSL shifts mask their count. Mask even the unselected expression,
        // then test the complete unsigned guest count to implement clamping.
        Some(false) => writeln!(
            s,
            "r[{dst}]=select(0u,r[{left}] {symbol} (r[{right}] & 31u),r[{right}] < 32u);"
        ),
    }
    .unwrap();
}

fn emit_comparison(s: &mut String, left: u8, right: u8, comparison: IntegerComparison) {
    emit_integer_condition(s, left, right, comparison, "p0");
}

fn emit_integer_condition(
    s: &mut String,
    left: u8,
    right: u8,
    comparison: IntegerComparison,
    target: &str,
) {
    use std::fmt::Write;
    let (symbol, signed) = match comparison {
        IntegerComparison::Less => ("<", false),
        IntegerComparison::Equal => ("==", false),
        IntegerComparison::LessEqual => ("<=", false),
        IntegerComparison::Greater => (">", false),
        IntegerComparison::NotEqual => ("!=", false),
        IntegerComparison::GreaterEqual => (">=", false),
        IntegerComparison::LessSigned => ("<", true),
        IntegerComparison::EqualSigned => ("==", true),
        IntegerComparison::LessEqualSigned => ("<=", true),
        IntegerComparison::GreaterSigned => (">", true),
        IntegerComparison::NotEqualSigned => ("!=", true),
        IntegerComparison::GreaterEqualSigned => (">=", true),
    };
    if signed {
        writeln!(
            s,
            "{target}=bitcast<i32>(r[{left}]) {symbol} bitcast<i32>(r[{right}]);"
        )
        .unwrap();
    } else {
        writeln!(s, "{target}=r[{left}] {symbol} r[{right}];").unwrap();
    }
}

fn emit_integer_set(
    s: &mut String,
    dst: u8,
    left: u8,
    right: u8,
    comparison: IntegerComparison,
    float_result: bool,
) {
    use std::fmt::Write;
    // Snapshot the relation before an aliased destination changes either input.
    // ISET must neither initialize nor clobber the existing predicate P0.
    s.push_str("{ var condition:bool;\n");
    emit_integer_condition(s, left, right, comparison, "condition");
    let bits = if float_result {
        0x3f800000u32
    } else {
        u32::MAX
    };
    writeln!(s, "r[{dst}]=select(0u,{bits}u,condition); }}").unwrap();
}

fn emit_float_set(
    s: &mut String,
    dst: u8,
    left: u8,
    right: u8,
    comparison: FloatComparison,
    float_result: bool,
) {
    use std::fmt::Write;
    // Scoped condition preserves P0 and snapshots both inputs before a possibly
    // aliased destination is written. Mask true is raw FFFFFFFF, not NaN math.
    s.push_str("{ var condition:bool;\n");
    emit_float_condition(s, left, right, comparison, "condition");
    let bits = if float_result {
        0x3f800000u32
    } else {
        u32::MAX
    };
    writeln!(s, "r[{dst}]=select(0u,{bits}u,condition); }}").unwrap();
}

fn emit_float_comparison(s: &mut String, left: u8, right: u8, comparison: FloatComparison) {
    emit_float_condition(s, left, right, comparison, "p0");
}

fn emit_float_condition(
    s: &mut String,
    left: u8,
    right: u8,
    comparison: FloatComparison,
    target: &str,
) {
    use std::fmt::Write;
    if matches!(comparison, FloatComparison::False | FloatComparison::True) {
        writeln!(s, "{target}={};", comparison == FloatComparison::True).unwrap();
        return;
    }
    // Integer classification preserves every subnormal and signaling-NaN bit
    // independently of host floating comparison/flush modes. Signed zeros are
    // equal; complementing negative encodings reverses their magnitude order.
    writeln!(s, "{{ let a=r[{left}]; let b=r[{right}];").unwrap();
    s.push_str("let nan=(a & 0x7fffffffu)>0x7f800000u || (b & 0x7fffffffu)>0x7f800000u;\n");
    if matches!(comparison, FloatComparison::Numeric | FloatComparison::Nan) {
        let inversion = if comparison == FloatComparison::Numeric {
            "!"
        } else {
            ""
        };
        writeln!(s, "{target}={inversion}nan; }}").unwrap();
        return;
    }
    let (symbol, unordered) = match comparison {
        FloatComparison::Less => ("<", false),
        FloatComparison::Equal => ("==", false),
        FloatComparison::LessEqual => ("<=", false),
        FloatComparison::Greater => (">", false),
        FloatComparison::NotEqual => ("!=", false),
        FloatComparison::GreaterEqual => (">=", false),
        FloatComparison::LessUnordered => ("<", true),
        FloatComparison::EqualUnordered => ("==", true),
        FloatComparison::LessEqualUnordered => ("<=", true),
        FloatComparison::GreaterUnordered => (">", true),
        FloatComparison::NotEqualUnordered => ("!=", true),
        FloatComparison::GreaterEqualUnordered => (">=", true),
        _ => unreachable!("classification and constant conditions returned above"),
    };
    s.push_str(
        "let an=select(a,0u,(a & 0x7fffffffu)==0u); let bn=select(b,0u,(b & 0x7fffffffu)==0u);\n",
    );
    s.push_str("let ak=select(an ^ 0x80000000u,~an,(an & 0x80000000u)!=0u); let bk=select(bn ^ 0x80000000u,~bn,(bn & 0x80000000u)!=0u);\n");
    let guard = if unordered { "nan ||" } else { "!nan &&" };
    writeln!(s, "{target}={guard} (ak {symbol} bk); }}").unwrap();
}

fn constant_input(constants: &mut Vec<ConstantRef>, reference: ConstantRef) -> Option<u8> {
    if let Some(index) = constants.iter().position(|r| *r == reference) {
        return Some(index as u8);
    }
    if constants.len() == 64 {
        return None;
    }
    let index = constants.len() as u8;
    constants.push(reference);
    Some(index)
}

fn emit_logic_immediate(
    s: &mut String,
    dst: u8,
    src: u8,
    bits: u32,
    operator: LogicalOperator,
    invert_src: bool,
    invert_immediate: bool,
) {
    use std::fmt::Write;
    let symbol = match operator {
        LogicalOperator::And => "&",
        LogicalOperator::Or => "|",
        LogicalOperator::Xor => "^",
        LogicalOperator::PassB => unreachable!("unsupported instruction: LOP32I operator"),
    };
    let left = if invert_src { "~" } else { "" };
    let right = if invert_immediate { "~" } else { "" };
    writeln!(s, "r[{dst}]=({left}r[{src}]) {symbol} ({right}{bits}u);").unwrap();
}

fn emit_add_immediate(s: &mut String, dst: u8, src: u8, bits: u32, negate_src: bool) {
    use std::fmt::Write;
    if negate_src {
        writeln!(s, "r[{dst}]=(0u-r[{src}])+{bits}u;").unwrap();
    } else {
        writeln!(s, "r[{dst}]=r[{src}]+{bits}u;").unwrap();
    }
}

fn emit_float_immediate(s: &mut String, dst: u8, src: u8, bits: u32, operator: FloatOperator) {
    use std::fmt::Write;
    let symbol = match operator {
        FloatOperator::Add => "+",
        FloatOperator::Multiply => "*",
    };
    writeln!(
        s,
        "r[{dst}]=bitcast<u32>(bitcast<f32>(r[{src}]) {symbol} bitcast<f32>({bits}u));"
    )
    .unwrap();
}

fn emit_conditional_move(s: &mut String, dst: u8, src: u8, invert: bool) {
    use std::fmt::Write;
    let condition = if invert { "!p0" } else { "p0" };
    writeln!(s, "if {condition} {{ r[{dst}]=r[{src}]; }}").unwrap();
}
