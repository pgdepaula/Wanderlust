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
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IntegerOperator {
    Add,
    And,
    Or,
    Xor,
    MinUnsigned,
    MaxUnsigned,
    ShiftLeft { wrap: bool },
    ShiftRight { wrap: bool },
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UnsignedComparison {
    Less,
    Equal,
    LessEqual,
    Greater,
    NotEqual,
    GreaterEqual,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Operation {
    /// Compare into P0; the complementary predicate output is discarded to PT.
    PredicateCompare {
        left: u8,
        right: u8,
        comparison: UnsignedComparison,
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
    Float {
        dst: u8,
        left: u8,
        right: u8,
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
                // Predicate-producing ISETP has its own conservative fixed latency.
                // Do not generalize the new stall encoding to other instructions.
                if schedule & 31 != if word >> 52 == 0x5b6 { 13 } else { 6 } {
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
                let compare_mask = (7u64 << 49) | (255u64 << 8) | (255u64 << 20);
                if word & !compare_mask == 0x5b60038000070007 {
                    let comparison = match (word >> 49) & 7 {
                        1 => UnsignedComparison::Less,
                        2 => UnsignedComparison::Equal,
                        3 => UnsignedComparison::LessEqual,
                        4 => UnsignedComparison::Greater,
                        5 => UnsignedComparison::NotEqual,
                        6 => UnsignedComparison::GreaterEqual,
                        _ => return Err(fail("unsupported unsigned comparison mode")),
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
                } else if matches!(
                    word & !(255 | (255u64 << 8) | (255u64 << 20)),
                    0x5c10000000070000
                        | 0x5c47000000070000
                        | 0x5c47020000070000
                        | 0x5c47040000070000
                        | 0x5c48000000070000
                        | 0x5c48008000070000
                        | 0x5c28000000070000
                        | 0x5c28008000070000
                        | 0x5c20038000070000
                        | 0x5c20078000070000
                ) {
                    let left = ((word >> 8) & 255) as u8;
                    let right = ((word >> 20) & 255) as u8;
                    if write_barrier != 7
                        || [left, right]
                            .iter()
                            .any(|r| !initialized[*r as usize] || pending[*r as usize])
                    {
                        return Err(fail("unready integer ALU operand or unsupported schedule"));
                    }
                    Operation::Integer {
                        dst,
                        left,
                        right,
                        operator: match word & !(255 | (255u64 << 8) | (255u64 << 20)) {
                            0x5c10000000070000 => IntegerOperator::Add,
                            0x5c47000000070000 => IntegerOperator::And,
                            0x5c47020000070000 => IntegerOperator::Or,
                            0x5c47040000070000 => IntegerOperator::Xor,
                            0x5c48000000070000 => IntegerOperator::ShiftLeft { wrap: false },
                            0x5c48008000070000 => IntegerOperator::ShiftLeft { wrap: true },
                            0x5c28000000070000 => IntegerOperator::ShiftRight { wrap: false },
                            0x5c28008000070000 => IntegerOperator::ShiftRight { wrap: true },
                            0x5c20038000070000 => IntegerOperator::MinUnsigned,
                            _ => IntegerOperator::MaxUnsigned,
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
                    // Integer identities can disappear in host optimization.
                    // Retain the unfused-rounding hazard even across bit operations.
                    Operation::Select { left, right, .. } => {
                        multiply_result[left as usize]
                            || multiply_result[right as usize]
                            || predicate_multiply
                    }
                    Operation::Integer { left, right, .. } => {
                        multiply_result[left as usize] || multiply_result[right as usize]
                    }
                    Operation::Float {
                        operator: FloatOperator::Multiply,
                        ..
                    } => true,
                    Operation::Move { src, .. } => multiply_result[src as usize],
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
                Operation::PredicateCompare { left, right, .. } => {
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
                } => {
                    largest = largest.max(u16::from(dst));
                    for r in [left, right] {
                        if r != 255 {
                            largest = largest.max(u16::from(r));
                        }
                    }
                }
                Operation::Move { dst, src } => {
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
        let mut s = String::from(
            "struct Constants { words: array<vec4<u32>,16> };\n@group(0) @binding(0) var<uniform> cb: Constants;\n",
        );
        if self.texture_input.is_some() {
            s.push_str("@group(1) @binding(0) var image:texture_2d<f32>; @group(1) @binding(1) var image_sampler:sampler;\n");
        }
        s.push_str("struct Vertex { @builtin(position) position:vec4<f32>, @location(0) @interpolate(linear) uv:vec2<f32> };\n@vertex fn vs_main(@builtin(vertex_index) i:u32)->Vertex { let ps=array<vec2<f32>,3>(vec2(-1.,-1.),vec2(3.,-1.),vec2(-1.,3.));let p=ps[i];var v:Vertex;v.position=vec4(p,0.,1.);v.uv=vec2((p.x+1.)*.5,(1.-p.y)*.5);return v;}\n@fragment fn fs_main(v:Vertex)->@location(0) vec4<f32>{var r:array<u32,256>;\n");
        if self.operations.iter().any(|op| {
            matches!(
                op,
                Operation::PredicateCompare { .. } | Operation::Select { .. }
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
                Operation::Integer { dst, left, right, operator } => {
                    emit_integer(&mut s, dst, left, right, operator);
                }
                Operation::Immediate { dst, bits } => writeln!(s, "r[{dst}]={bits}u;").unwrap(),
                Operation::Move { dst, src } => writeln!(s, "r[{dst}]=r[{src}];").unwrap(),
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
        let mut s = String::from(
            "struct Vertex { @builtin(position) position:vec4<f32>, @location(0) @interpolate(linear) uv:vec2<f32> };\n@vertex fn vs_main(@location(0) a0:vec4<f32>,@location(1) a1:vec2<f32>)->Vertex {var r:array<u32,256>;var v:Vertex;\n",
        );
        if self.operations().iter().any(|op| {
            matches!(
                op,
                Operation::PredicateCompare { .. } | Operation::Select { .. }
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
                Operation::Integer {
                    dst,
                    left,
                    right,
                    operator,
                } => {
                    emit_integer(&mut s, dst, left, right, operator);
                }
                Operation::Immediate { dst, bits } => writeln!(s, "r[{dst}]={bits}u;").unwrap(),
                Operation::Move { dst, src } => writeln!(s, "r[{dst}]=r[{src}];").unwrap(),
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

fn emit_integer(s: &mut String, dst: u8, left: u8, right: u8, operator: IntegerOperator) {
    use std::fmt::Write;
    let (symbol, wrap) = match operator {
        IntegerOperator::Add => ("+", None),
        IntegerOperator::And => ("&", None),
        IntegerOperator::Or => ("|", None),
        IntegerOperator::Xor => ("^", None),
        IntegerOperator::MinUnsigned => {
            writeln!(s, "r[{dst}]=min(r[{left}],r[{right}]);").unwrap();
            return;
        }
        IntegerOperator::MaxUnsigned => {
            writeln!(s, "r[{dst}]=max(r[{left}],r[{right}]);").unwrap();
            return;
        }
        IntegerOperator::ShiftLeft { wrap } => ("<<", Some(wrap)),
        IntegerOperator::ShiftRight { wrap } => (">>", Some(wrap)),
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

fn emit_comparison(s: &mut String, left: u8, right: u8, comparison: UnsignedComparison) {
    use std::fmt::Write;
    let symbol = match comparison {
        UnsignedComparison::Less => "<",
        UnsignedComparison::Equal => "==",
        UnsignedComparison::LessEqual => "<=",
        UnsignedComparison::Greater => ">",
        UnsignedComparison::NotEqual => "!=",
        UnsignedComparison::GreaterEqual => ">=",
    };
    writeln!(s, "p0=r[{left}] {symbol} r[{right}];").unwrap();
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
