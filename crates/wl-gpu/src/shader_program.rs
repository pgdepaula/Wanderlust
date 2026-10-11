//! Bounded straight-line Maxwell fragment dataflow. Registers contain raw bits.
//! Resource data is runtime input, never a specialization of shader code.
use crate::{Error, Result, invalid};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ConstantRef {
    pub slot: u8,
    pub offset: u32,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Operation {
    Immediate { dst: u8, bits: u32 },
    Move { dst: u8, src: u8 },
    Constant { dst: u8, input: u8 },
}
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FragmentProgram {
    operations: Vec<Operation>,
    constants: Vec<ConstantRef>,
}
impl FragmentProgram {
    pub fn decode(input: &[u8]) -> Result<Self> {
        if input.len() < 112 || input.len() > 0x50 + 4096 || (input.len() - 0x50) % 32 != 0 {
            return Err(invalid("invalid Maxwell fragment length"));
        }
        for (n, b) in input[..0x50].chunks_exact(4).enumerate() {
            let expected = match n {
                0 => 0x61462,
                4 => 0xff000,
                18 => 15,
                _ => 0,
            };
            if u32::from_le_bytes(b.try_into().unwrap()) != expected {
                return Err(invalid("unsupported resource fragment SPH"));
            }
        }
        let mut out = Self {
            operations: Vec::new(),
            constants: Vec::new(),
        };
        let mut initialized = [false; 256];
        initialized[255] = true; // RZ
        let mut pending = [false; 256];
        let mut exited = false;
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
                if schedule & 31 != 6
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
                if exited {
                    if word != 0x50b0000000070f00 || schedule != 0x7e6 {
                        return Err(fail("invalid EXIT padding"));
                    }
                    continue;
                }
                if word == 0xe30000000007000f {
                    if write_barrier != 7 || (0..4).any(|r| !initialized[r] || pending[r]) {
                        return Err(fail("unready fragment exports"));
                    }
                    exited = true;
                    continue;
                }
                let dst = (word & 255) as u8;
                if dst == 255 {
                    return Err(fail("RZ destination unsupported"));
                }
                let immediate_mask = (u64::from(u32::MAX) << 20) | 255;
                let move_mask = (255u64 << 20) | 255;
                let ldc_mask = (31u64 << 36) | (0xffffu64 << 20) | 255;
                let op = if word & !immediate_mask == 0x010000000007f000 {
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
                } else if word & !ldc_mask == 0xef9400000007ff00 {
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
                    let index = match out.constants.iter().position(|r| *r == reference) {
                        Some(n) => n,
                        None => {
                            if out.constants.len() == 64 {
                                return Err(fail("too many constant references"));
                            }
                            out.constants.push(reference);
                            out.constants.len() - 1
                        }
                    };
                    pending[dst as usize] = true;
                    Operation::Constant {
                        dst,
                        input: index as u8,
                    }
                } else {
                    return Err(fail("unsupported instruction/modifier/predicate"));
                };
                if pending[dst as usize] && !matches!(op, Operation::Constant { .. }) {
                    return Err(fail("write to pending register"));
                }
                initialized[dst as usize] = true;
                out.operations.push(op);
            }
        }
        if !exited {
            return Err(invalid("fragment missing EXIT"));
        }
        Ok(out)
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
            "struct Constants { words: array<vec4<u32>,16> };\n@group(0) @binding(0) var<uniform> cb: Constants;\n@vertex fn vs_main(@builtin(vertex_index) i:u32)->@builtin(position) vec4<f32>{ let p=array<vec2<f32>,3>(vec2(-1.,-1.),vec2(3.,-1.),vec2(-1.,3.));return vec4(p[i],0.,1.);}\n@fragment fn fs_main()->@location(0) vec4<f32>{var r:array<u32,256>;\n",
        );
        for op in &self.operations {
            match *op {
                Operation::Immediate { dst, bits } => writeln!(s, "r[{dst}]={bits}u;").unwrap(),
                Operation::Move { dst, src } => writeln!(s, "r[{dst}]=r[{src}];").unwrap(),
                Operation::Constant { dst, input } => {
                    writeln!(s, "r[{dst}]=cb.words[{}][{}];", input / 4, input % 4).unwrap()
                }
            }
        }
        s.push_str("return vec4(bitcast<f32>(r[0]),bitcast<f32>(r[1]),bitcast<f32>(r[2]),bitcast<f32>(r[3]));}");
        s
    }
}
