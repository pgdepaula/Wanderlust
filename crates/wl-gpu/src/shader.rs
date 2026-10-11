//! Experimental straight-line Maxwell fragment translation, not a general compiler.
//! Input is an 80-byte SPH followed by complete 32-byte scheduled code bundles.
use crate::{Result, invalid};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ConstantFragment {
    output_bits: [u32; 4],
    instructions: u32,
}
impl ConstantFragment {
    /// Only the independently assembled, resource-free PS header variant is accepted.
    /// Full-mask, unpredicated MOV32I R0..R3 and unconditional EXIT are supported.
    /// Scheduling and padding are checked, never treated as arbitrary instructions.
    pub fn decode(input: &[u8]) -> Result<Self> {
        if input.len() < 0x50 + 32 || input.len() > 0x50 + 4096 || (input.len() - 0x50) % 32 != 0 {
            return Err(invalid("invalid Maxwell SPH/code length"));
        }
        for (n, bytes) in input[..0x50].chunks_exact(4).enumerate() {
            let value = u32::from_le_bytes(bytes.try_into().expect("header word"));
            let expected = match n {
                0 => 0x61462,
                4 => 0xff000,
                18 => 0xf,
                _ => 0,
            };
            if value != expected {
                return Err(invalid("unsupported Maxwell fragment header fields"));
            }
        }
        let mut registers = [None; 4];
        let mut exited = false;
        let mut instructions = 0;
        for bundle in input[0x50..].chunks_exact(32) {
            let control = u64::from_le_bytes(bundle[..8].try_into().expect("control word"));
            // Three stall-six slots, no dependency barriers, wait masks or reuse.
            if control != 0x001f9800fcc007e6 {
                return Err(invalid("unsupported Maxwell scheduling control"));
            }
            for bytes in bundle[8..].chunks_exact(8) {
                let word = u64::from_le_bytes(bytes.try_into().expect("instruction word"));
                if exited {
                    if word != 0x50b0000000070f00 {
                        return Err(invalid("noncanonical padding after Maxwell EXIT"));
                    }
                } else if word == 0xe30000000007000f {
                    if registers.iter().any(Option::is_none) {
                        return Err(invalid("uninitialized fragment export register"));
                    }
                    exited = true;
                    instructions += 1;
                } else {
                    let variables = (u64::from(u32::MAX) << 20) | 0xff;
                    if word & !variables != 0x010000000007f000 {
                        return Err(invalid(
                            "unsupported Maxwell instruction, predicate or lane mask",
                        ));
                    }
                    let dst = (word & 0xff) as usize;
                    let value = ((word >> 20) & u64::from(u32::MAX)) as u32;
                    if dst >= 4 || !f32::from_bits(value).is_finite() {
                        return Err(invalid(
                            "unsupported fragment register or nonfinite immediate",
                        ));
                    }
                    registers[dst] = Some(value);
                    instructions += 1;
                }
            }
        }
        if !exited {
            return Err(invalid("Maxwell fragment has no EXIT"));
        }
        Ok(Self {
            output_bits: registers.map(|v| v.expect("initialized at EXIT")),
            instructions,
        })
    }
    pub fn output_bits(&self) -> [u32; 4] {
        self.output_bits
    }
    pub fn instruction_count(&self) -> u32 {
        self.instructions
    }
    /// Raw register contents export as floating-point bits, never integer casts.
    pub fn wgsl(&self) -> String {
        let [r, g, b, a] = self.output_bits;
        format!(
            "@fragment fn fs_main() -> @location(0) vec4<f32> {{ return vec4<f32>(bitcast<f32>({r}u), bitcast<f32>({g}u), bitcast<f32>({b}u), bitcast<f32>({a}u)); }}"
        )
    }
}
