//! Bounded Fermi MME uploads and single-write, straight-line state macros.
//! ISA reference: envytools f102b82381f3f11cee113d16374c87091db039d9.
//! These are implementation budgets, not claims about every Maxwell RAM size.
pub const CODE_WORDS: usize = 2048;
pub const ENTRIES: usize = 128;
const MAX_STEPS: usize = 64;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MacroError {
    AllocationFailed,
    UnsupportedPointer(u32),
    MissingUploadPointer,
    UnboundEntry(u16),
    MissingInstruction(usize),
    UnsupportedInstruction(u32),
    UninitializedRegister(u8),
    UnsupportedIncrement,
    MissingMethodAddress,
    UnsupportedEmissionCount,
    MissingExit,
    UnsupportedOutput(u16),
}

#[derive(Debug)]
pub struct MacroEngine {
    code: Vec<Option<u32>>,
    entries: [Option<usize>; ENTRIES],
    code_pointer: Option<usize>,
    entry_pointer: Option<usize>,
}
impl Default for MacroEngine {
    fn default() -> Self {
        Self {
            code: Vec::new(),
            entries: [None; ENTRIES],
            code_pointer: None,
            entry_pointer: None,
        }
    }
}
impl MacroEngine {
    pub fn upload(&mut self, method: u16, value: u32) -> Result<bool, MacroError> {
        match method {
            0x114 => {
                if value as usize >= CODE_WORDS {
                    return Err(MacroError::UnsupportedPointer(value));
                }
                self.code_pointer = Some(value as usize);
            }
            0x118 => {
                let pointer = self.code_pointer.ok_or(MacroError::MissingUploadPointer)?;
                if pointer >= CODE_WORDS {
                    return Err(MacroError::UnsupportedPointer(pointer as u32));
                }
                if self.code.is_empty() {
                    self.code
                        .try_reserve_exact(CODE_WORDS)
                        .map_err(|_| MacroError::AllocationFailed)?;
                    self.code.resize(CODE_WORDS, None);
                }
                self.code[pointer] = Some(value);
                self.code_pointer = Some(pointer + 1);
            }
            0x11c => {
                if value as usize >= ENTRIES {
                    return Err(MacroError::UnsupportedPointer(value));
                }
                self.entry_pointer = Some(value as usize);
            }
            0x120 => {
                let pointer = self.entry_pointer.ok_or(MacroError::MissingUploadPointer)?;
                if pointer >= ENTRIES {
                    return Err(MacroError::UnsupportedPointer(pointer as u32));
                }
                if value as usize >= CODE_WORDS {
                    return Err(MacroError::UnsupportedPointer(value));
                }
                self.entries[pointer] = Some(value as usize);
                self.entry_pointer = Some(pointer + 1);
            }
            _ => return Ok(false),
        }
        Ok(true)
    }
    /// Preflight a complete supported program before exposing its one write.
    /// No branch, method read, parameter wait, ADC, or draw emission is inferred.
    pub fn execute(&self, slot: u16, parameter: u32) -> Result<(u16, u32), MacroError> {
        let start = self
            .entries
            .get(slot as usize)
            .copied()
            .flatten()
            .ok_or(MacroError::UnboundEntry(slot))?;
        let mut registers = [0u32; 8];
        let mut initialized = [false; 8];
        initialized[0] = true;
        initialized[1] = true;
        registers[1] = parameter;
        let mut address = None;
        let mut output = None;
        let mut delay_slot = false;
        for pc in start..start + MAX_STEPS {
            let word = self
                .code
                .get(pc)
                .copied()
                .flatten()
                .ok_or(MacroError::MissingInstruction(pc))?;
            let exit = word & 0x80 != 0;
            if word & 8 != 0 || (delay_slot && exit) {
                return Err(MacroError::UnsupportedInstruction(word));
            }
            let source = ((word >> 11) & 7) as usize;
            if !initialized[source] {
                return Err(MacroError::UninitializedRegister(source as u8));
            }
            let result = match word & 7 {
                0 if word & 0xfffe_0000 == 0 => {
                    let other = ((word >> 14) & 7) as usize;
                    if !initialized[other] {
                        return Err(MacroError::UninitializedRegister(other as u8));
                    }
                    registers[source].wrapping_add(registers[other])
                }
                1 => registers[source].wrapping_add(((word as i32) >> 14) as u32),
                _ => return Err(MacroError::UnsupportedInstruction(word)),
            };
            match (word >> 4) & 7 {
                1 => {} // MOV, including the independently assembled NOP.
                2 => {
                    if result & 0x3f000 != 0 {
                        return Err(MacroError::UnsupportedIncrement);
                    }
                    address = Some(((result & 0xfff) * 4) as u16);
                }
                4 => {
                    if output.is_some() {
                        return Err(MacroError::UnsupportedEmissionCount);
                    }
                    output = Some((address.ok_or(MacroError::MissingMethodAddress)?, result));
                }
                _ => return Err(MacroError::UnsupportedInstruction(word)),
            }
            let destination = ((word >> 8) & 7) as usize;
            if destination != 0 {
                registers[destination] = result;
                initialized[destination] = true;
            }
            if delay_slot {
                return output.ok_or(MacroError::UnsupportedEmissionCount);
            }
            delay_slot = exit;
        }
        Err(MacroError::MissingExit)
    }
}
