//! Bounded Fermi MME uploads and resumable straight-line state emissions.
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
    UnsupportedRead(u32),
    UninitializedScratch(u16),
    ActiveInvocation,
    MissingParameter,
    UnexpectedParameter,
}

#[derive(Debug)]
struct PendingParameter {
    destination: usize,
    send: bool,
    exit: bool,
    delay_slot: bool,
}

/// Resumable straight-line state. No guest borrow or GPU resource is retained.
#[derive(Debug)]
pub(crate) struct Invocation {
    pc: usize,
    steps: usize,
    registers: [u32; 8],
    initialized: [bool; 8],
    address: Option<u16>,
    increment: u16,
    pending_parameter: Option<PendingParameter>,
    parameter: Option<u32>,
    delay_slot: bool,
    finished: bool,
}
impl Invocation {
    pub(crate) fn waiting_for_parameter(&self) -> bool {
        self.pending_parameter.is_some() && self.parameter.is_none()
    }
    pub(crate) fn provide_parameter(&mut self, value: u32) -> Result<(), MacroError> {
        if !self.waiting_for_parameter() {
            return Err(MacroError::UnexpectedParameter);
        }
        self.parameter = Some(value);
        Ok(())
    }
    fn send(&mut self, value: u32) -> Result<(u16, u32), MacroError> {
        let method = self.address.ok_or(MacroError::MissingMethodAddress)?;
        let next = u32::from(method) + u32::from(self.increment);
        if next > 0x3ffc {
            return Err(MacroError::UnsupportedIncrement);
        }
        self.address = Some(next as u16);
        Ok((method, value))
    }
    fn finish_instruction(&mut self, exit: bool, delay_slot: bool) {
        self.finished = delay_slot;
        self.delay_slot = exit;
        self.pc += 1;
        self.steps += 1;
    }
}

#[derive(Debug)]
pub struct MacroEngine {
    code: Vec<Option<u32>>,
    entries: [Option<usize>; ENTRIES],
    code_pointer: Option<usize>,
    entry_pointer: Option<usize>,
    scratch: [Option<u32>; 256],
}
impl Default for MacroEngine {
    fn default() -> Self {
        Self {
            code: Vec::new(),
            entries: [None; ENTRIES],
            code_pointer: None,
            entry_pointer: None,
            scratch: [None; 256],
        }
    }
}
impl MacroEngine {
    pub fn upload(&mut self, method: u16, value: u32) -> Result<bool, MacroError> {
        match method {
            0x3400..=0x37fc if method % 4 == 0 => {
                self.scratch[usize::from((method - 0x3400) / 4)] = Some(value);
            }
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
    /// Reads are limited to explicitly written scratch registers. No branch,
    /// parameter wait, ADC, or draw emission is inferred.
    pub fn execute(&self, slot: u16, parameter: u32) -> Result<(u16, u32), MacroError> {
        let mut invocation = self.begin(slot, parameter)?;
        let mut output = None;
        while let Some(emission) = self.next_output(&mut invocation)? {
            if output.is_some() {
                return Err(MacroError::UnsupportedEmissionCount);
            }
            output = Some(emission);
        }
        if invocation.waiting_for_parameter() {
            return Err(MacroError::MissingParameter);
        }
        output.ok_or(MacroError::UnsupportedEmissionCount)
    }
    pub(crate) fn begin(&self, slot: u16, parameter: u32) -> Result<Invocation, MacroError> {
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
        Ok(Invocation {
            pc: start,
            steps: 0,
            registers,
            initialized,
            address: None,
            increment: 0,
            pending_parameter: None,
            parameter: None,
            delay_slot: false,
            finished: false,
        })
    }
    /// Yield after each emission. The caller must execute that method before
    /// resuming, so scratch reads and later state cannot precede earlier effects.
    pub(crate) fn next_output(
        &self,
        invocation: &mut Invocation,
    ) -> Result<Option<(u16, u32)>, MacroError> {
        while !invocation.finished {
            if let Some(pending) = invocation.pending_parameter.take() {
                let Some(parameter) = invocation.parameter.take() else {
                    invocation.pending_parameter = Some(pending);
                    return Ok(None);
                };
                if !pending.send && pending.destination != 0 {
                    invocation.registers[pending.destination] = parameter;
                    invocation.initialized[pending.destination] = true;
                }
                let output = if pending.send {
                    Some(invocation.send(parameter)?)
                } else {
                    None
                };
                invocation.finish_instruction(pending.exit, pending.delay_slot);
                if output.is_some() {
                    return Ok(output);
                }
                continue;
            }
            if invocation.steps == MAX_STEPS {
                return Err(MacroError::MissingExit);
            }
            let pc = invocation.pc;
            let delay_slot = invocation.delay_slot;
            let registers = &mut invocation.registers;
            let initialized = &mut invocation.initialized;
            let mut output = None;
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
                0 if word & 0xffc0_0000 == 0 => {
                    let other = ((word >> 14) & 7) as usize;
                    if !initialized[other] {
                        return Err(MacroError::UninitializedRegister(other as u8));
                    }
                    let left = registers[source];
                    let right = registers[other];
                    // Only register ALU functions with no incoming carry.
                    // ADC/SBB remain explicit errors; no carry is fabricated.
                    match (word >> 17) & 31 {
                        0 => left.wrapping_add(right),
                        2 => left.wrapping_sub(right),
                        8 => left ^ right,
                        9 => left | right,
                        10 => left & right,
                        11 => left & !right,
                        12 => !(left & right),
                        _ => return Err(MacroError::UnsupportedInstruction(word)),
                    }
                }
                1 => registers[source].wrapping_add(((word as i32) >> 14) as u32),
                5 => {
                    // READ addresses are method WORD offsets, unlike apply's
                    // byte offsets. Reject every other register/read behavior.
                    let method = registers[source].wrapping_add(((word as i32) >> 14) as u32);
                    if !(0xd00..0xe00).contains(&method) {
                        return Err(MacroError::UnsupportedRead(method));
                    }
                    self.scratch[(method - 0xd00) as usize]
                        .ok_or(MacroError::UninitializedScratch((method * 4) as u16))?
                }
                _ => return Err(MacroError::UnsupportedInstruction(word)),
            };
            let assignment = (word >> 4) & 7;
            let destination = ((word >> 8) & 7) as usize;
            match assignment {
                3 => {
                    // Primary ISA documentation orders SEND before parameter
                    // fetch. Keep the old result until its emission is applied;
                    // only the following DATA word initializes the destination.
                    let emission = invocation.send(result)?;
                    invocation.pending_parameter = Some(PendingParameter {
                        destination,
                        send: false,
                        exit,
                        delay_slot,
                    });
                    return Ok(Some(emission));
                }
                0 | 5 | 6 => {
                    if assignment != 0 {
                        invocation.address = Some(((result & 0xfff) * 4) as u16);
                        invocation.increment = (((result >> 12) & 63) * 4) as u16;
                    }
                    if assignment == 6 && destination != 0 {
                        registers[destination] = result;
                        initialized[destination] = true;
                    }
                    invocation.pending_parameter = Some(PendingParameter {
                        destination,
                        send: assignment == 6,
                        exit,
                        delay_slot,
                    });
                    return Ok(None);
                }
                1 => {} // MOV, including the independently assembled NOP.
                2 | 7 => {
                    let method = ((result & 0xfff) * 4) as u16;
                    invocation.address = Some(method);
                    invocation.increment = (((result >> 12) & 63) * 4) as u16;
                    if (word >> 4) & 7 == 7 {
                        output = Some((result >> 12) & 63);
                    }
                }
                4 => {
                    output = Some(result);
                }
                _ => return Err(MacroError::UnsupportedInstruction(word)),
            }
            if destination != 0 {
                registers[destination] = result;
                initialized[destination] = true;
            }
            let output = output.map(|value| invocation.send(value)).transpose()?;
            invocation.finish_instruction(exit, delay_slot);
            if output.is_some() {
                return Ok(output);
            }
        }
        Ok(None)
    }
}

#[cfg(test)]
mod streaming_tests {
    use super::*;
    fn loaded(words: &[u32]) -> MacroEngine {
        let mut engine = MacroEngine::default();
        engine.upload(0x11c, 0).unwrap();
        engine.upload(0x120, 0).unwrap();
        engine.upload(0x114, 0).unwrap();
        for &word in words {
            engine.upload(0x118, word).unwrap();
        }
        engine
    }
    #[test]
    fn independent_maddrsend_fixture_yields_each_output_and_finishes_delay_slot() {
        let words: Vec<u32> = include_str!("../tests/fixtures/mme/streaming-state.words")
            .split_whitespace()
            .map(|w| u32::from_str_radix(w, 16).unwrap())
            .collect();
        assert_eq!(
            words,
            [
                0x01a00021, 0x04444041, 0x00110071, 0x01a00021, 0x000008c0, 0x00000011
            ]
        );
        let engine = loaded(&words);
        let mut run = engine.begin(0, 1).unwrap();
        assert_eq!(engine.next_output(&mut run), Ok(Some((0x1a00, 0x1111))));
        assert_eq!(engine.next_output(&mut run), Ok(Some((0x110, 0))));
        assert_eq!(engine.next_output(&mut run), Ok(Some((0x1a00, 1))));
        assert!(!run.finished); // EXIT's NOP still must execute.
        assert_eq!(engine.next_output(&mut run), Ok(None));
        assert!(run.finished);
        assert_eq!(engine.next_output(&mut run), Ok(None));
        assert_eq!(
            engine.execute(0, 1),
            Err(MacroError::UnsupportedEmissionCount)
        );
    }
    #[test]
    fn resumption_does_not_reset_the_instruction_budget() {
        let mut words = vec![0x01a00021];
        words.extend([0x840; 63]);
        let engine = loaded(&words);
        let mut run = engine.begin(0, 7).unwrap();
        for _ in 0..63 {
            assert_eq!(engine.next_output(&mut run), Ok(Some((0x1a00, 7))));
        }
        assert_eq!(engine.next_output(&mut run), Err(MacroError::MissingExit));
    }
    #[test]
    fn later_scratch_read_occurs_only_after_the_earlier_emission_is_returned() {
        let mut engine = loaded(&[0x01a00021, 0x840, 0x03468215, 0x10c0, 0x11]);
        let mut run = engine.begin(0, 1).unwrap();
        assert_eq!(engine.next_output(&mut run), Ok(Some((0x1a00, 1))));
        engine.upload(0x3468, 0x1111).unwrap();
        assert_eq!(engine.next_output(&mut run), Ok(Some((0x1a00, 0x1111))));
        assert_eq!(engine.next_output(&mut run), Ok(None));
    }
    #[test]
    fn maddrsend_emits_high_bits_and_increments_exact_method_words() {
        // envyas: maddrsend (add $r1 0); exit send (add $r1 0); nop;
        let engine = loaded(&[0x871, 0x8c0, 0x11]);
        let mut run = engine.begin(0, 0x1d00).unwrap();
        assert_eq!(engine.next_output(&mut run), Ok(Some((0x3400, 1))));
        assert_eq!(engine.next_output(&mut run), Ok(Some((0x3404, 0x1d00))));
        assert_eq!(engine.next_output(&mut run), Ok(None));
        assert_eq!(run.address, Some(0x3408));
        let mut overflow = engine.begin(0, 0x3ffff).unwrap();
        assert_eq!(
            engine.next_output(&mut overflow),
            Err(MacroError::UnsupportedIncrement)
        );
    }
    #[test]
    fn independent_argument_modes_wait_consume_exact_words_and_keep_registers() {
        let words: Vec<u32> = include_str!("../tests/fixtures/mme/parameter-state.words")
            .split_whitespace()
            .map(|w| u32::from_str_radix(w, 16).unwrap())
            .collect();
        assert_eq!(
            words,
            [
                0x00110071, 0x07400061, 0x00000201, 0x00001040, 0x01a00351, 0x00001bb0, 0x00000011
            ]
        );
        let engine = loaded(&words);
        let mut run = engine.begin(0, 0x55).unwrap();
        assert_eq!(engine.next_output(&mut run), Ok(Some((0x110, 0))));
        assert_eq!(engine.next_output(&mut run), Ok(None));
        assert!(run.waiting_for_parameter());
        let steps = run.steps;
        assert_eq!(engine.next_output(&mut run), Ok(None));
        assert_eq!(run.steps, steps);
        run.provide_parameter(0x89abcdef).unwrap();
        assert_eq!(
            run.provide_parameter(0),
            Err(MacroError::UnexpectedParameter)
        );
        assert_eq!(engine.next_output(&mut run), Ok(Some((0x3400, 0x89abcdef))));
        assert_eq!(engine.next_output(&mut run), Ok(None));
        run.provide_parameter(0x01234567).unwrap();
        assert_eq!(engine.next_output(&mut run), Ok(Some((0x3404, 0x01234567))));
        assert_eq!(engine.next_output(&mut run), Ok(None));
        run.provide_parameter(1).unwrap();
        assert_eq!(engine.next_output(&mut run), Ok(Some((0x1a00, 1))));
        assert_eq!(engine.next_output(&mut run), Ok(None));
        assert!(run.waiting_for_parameter());
        assert!(!run.finished);
        run.provide_parameter(0xfedcba98).unwrap();
        assert_eq!(engine.next_output(&mut run), Ok(None));
        assert!(run.finished);
        assert_eq!(run.registers[0], 0);
        assert_eq!(run.registers[1], 0x55);
        assert_eq!(run.registers[2], 0x01234567);
        assert_eq!(run.registers[3], 0xfedcba98);
        assert_eq!(run.steps, 7);
        assert_eq!(
            run.provide_parameter(9),
            Err(MacroError::UnexpectedParameter)
        );
        assert_eq!(engine.execute(0, 0x55), Err(MacroError::MissingParameter));
    }
    #[test]
    fn parameter_completion_honors_exit_delay_and_mode_six_result_destination() {
        // envyas: exit parm $r2; nop;
        let engine = loaded(&[0x281, 0x11]);
        let mut run = engine.begin(0, 0).unwrap();
        assert_eq!(engine.next_output(&mut run), Ok(None));
        assert!(!run.finished);
        run.provide_parameter(0xabcdef01).unwrap();
        assert_eq!(engine.next_output(&mut run), Ok(None));
        assert!(run.finished);
        assert_eq!(run.registers[2], 0xabcdef01);
        assert_eq!(run.steps, 2);
        // envyas: parmsend maddr $r4 0x1d00; exit; nop;
        let engine = loaded(&[0x07400461, 0x91, 0x11]);
        let mut run = engine.begin(0, 0).unwrap();
        assert_eq!(engine.next_output(&mut run), Ok(None));
        assert_eq!(run.registers[4], 0x1d00);
        run.provide_parameter(u32::MAX).unwrap();
        assert_eq!(engine.next_output(&mut run), Ok(Some((0x3400, u32::MAX))));
        assert_eq!(run.registers[4], 0x1d00);
        assert_eq!(engine.next_output(&mut run), Ok(None));
    }
    #[test]
    fn parameter_pauses_do_not_reset_the_total_instruction_budget() {
        let engine = loaded(&[0x201; 64]);
        let mut run = engine.begin(0, 0).unwrap();
        assert_eq!(
            run.provide_parameter(1),
            Err(MacroError::UnexpectedParameter)
        );
        assert_eq!(engine.next_output(&mut run), Ok(None));
        for n in 0..63 {
            run.provide_parameter(n).unwrap();
            assert_eq!(engine.next_output(&mut run), Ok(None));
            assert!(run.waiting_for_parameter());
            assert_eq!(run.steps, n as usize + 1);
        }
        run.provide_parameter(0).unwrap();
        assert_eq!(engine.next_output(&mut run), Err(MacroError::MissingExit));
        assert_eq!(run.steps, 64);
        assert_eq!(run.registers[2], 0);
        assert!(run.initialized[2]);
    }
    #[test]
    fn fetch_and_send_emits_old_result_before_fetching_new_destination() {
        // envyas: maddr 0x680; parm $r2 send (add $r1 0); exit; nop;
        let engine = loaded(&[0x01a00021, 0x00000a30, 0x91, 0x11]);
        let mut run = engine.begin(0, 0x1111).unwrap();
        assert_eq!(engine.next_output(&mut run), Ok(Some((0x1a00, 0x1111))));
        assert!(!run.initialized[2]);
        assert!(run.waiting_for_parameter());
        assert_eq!(engine.next_output(&mut run), Ok(None));
        run.provide_parameter(0xdeadbeef).unwrap();
        assert_eq!(engine.next_output(&mut run), Ok(None));
        assert_eq!(run.registers[2], 0xdeadbeef);
    }
    #[test]
    fn data_cannot_acknowledge_an_outstanding_backend_effect() {
        use crate::{
            fifo::MethodWrite,
            maxwell::{CLASS, Maxwell3d, MethodEffect, MethodError},
        };
        let mut engine = Maxwell3d::default();
        let set = |engine: &mut Maxwell3d, method, value| {
            engine.apply(MethodWrite {
                subchannel: 0,
                method,
                value,
            })
        };
        for (m, v) in [(0, CLASS), (0x11c, 0), (0x120, 0), (0x114, 0)] {
            set(&mut engine, m, v).unwrap();
        }
        // envyas: maddr 0x44; parm $r2 send (add $r0 0); exit; nop;
        for word in [0x00110021, 0x230, 0x91, 0x11] {
            set(&mut engine, 0x118, word).unwrap();
        }
        assert_eq!(set(&mut engine, 0x3800, 0), Ok(MethodEffect::WaitIdle));
        assert_eq!(
            set(&mut engine, 0x3804, 9),
            Err(MethodError::Macro(MacroError::ActiveInvocation))
        );
        // Pure state-machine acknowledgement only; actual GPU waits are covered
        // by the GPFIFO integration tests, not fabricated in this unit test.
        assert_eq!(engine.resume_macro().unwrap(), None);
        assert_eq!(set(&mut engine, 0x3804, 9), Ok(MethodEffect::State));
    }
}
