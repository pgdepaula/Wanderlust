//! Bounded Fermi MME uploads and resumable state/control-flow execution.
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
    InvalidBranchTarget(i64),
    MissingFirmwareIdle,
    InvalidFirmwareRequest(u32),
    UnsupportedPrivilegedWrite { address: u32, value: u32, mask: u32 },
}

/// Only known bits are represented; unknown reset values are never invented.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
struct PrivilegedBits {
    value: u32,
    known_mask: u32,
}
const PRIVILEGED_ADDRESSES: [u32; 5] = [0x418800, 0x419a08, 0x419f78, 0x404468, 0x419a04];
// Exact baseline initialization writes from deko3d 373bfd43, gpu_3d_base.cpp.
// This is not a general MMIO interface or support for arbitrary raster modes.
const PRIVILEGED_INITIALIZATION: [(u32, u32, u32); 7] = [
    (0x418800, 1, 1),
    (0x419a08, 0, 0x10),
    (0x419f78, 0, 8),
    (0x404468, 0x07ffffff, 0x3fffffff),
    (0x419a04, 1, 1),
    (0x419a04, 2, 2),
    (0x418800, 0, 0x01800000),
];

#[derive(Debug)]
struct PendingParameter {
    destination: usize,
    send: bool,
    exit: bool,
    delay_slot: bool,
}

/// Resumable state. No guest borrow or GPU resource is retained.
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
    branch_target: Option<usize>,
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
        self.pc = self.branch_target.take().unwrap_or(self.pc + 1);
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
    privileged: [PrivilegedBits; 5],
}
impl Default for MacroEngine {
    fn default() -> Self {
        Self {
            code: Vec::new(),
            entries: [None; ENTRIES],
            code_pointer: None,
            entry_pointer: None,
            scratch: [None; 256],
            privileged: [PrivilegedBits::default(); 5],
        }
    }
}
impl MacroEngine {
    /// Bounded firmware function 4 HLE: an actual masked context write, followed
    /// by acknowledgement. The owning channel must first complete its WFI.
    /// No firmware machine code, physical MMIO, or unknown mode is emulated.
    pub(crate) fn initialize_privileged_register(
        &mut self,
        address: u32,
        idle_completed: bool,
    ) -> Result<(), MacroError> {
        if !idle_completed {
            return Err(MacroError::MissingFirmwareIdle);
        }
        let [request, value, mask] = [0, 1, 2].map(|i| {
            self.scratch[i].ok_or(MacroError::UninitializedScratch(0x3400 + i as u16 * 4))
        });
        let request = request?;
        let value = value?;
        let mask = mask?;
        if request != 0 {
            return Err(MacroError::InvalidFirmwareRequest(request));
        }
        if !PRIVILEGED_INITIALIZATION.contains(&(address, value, mask)) {
            return Err(MacroError::UnsupportedPrivilegedWrite {
                address,
                value,
                mask,
            });
        }
        let index = PRIVILEGED_ADDRESSES
            .iter()
            .position(|&a| a == address)
            .ok_or(MacroError::UnsupportedPrivilegedWrite {
                address,
                value,
                mask,
            })?;
        let register = &mut self.privileged[index];
        register.value = (register.value & !mask) | (value & mask);
        register.known_mask |= mask;
        // Publish success only after every validation and the context update.
        self.scratch[0] = Some(1);
        Ok(())
    }
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
    /// Reads are limited to explicitly written scratch registers. Missing
    /// parameters, ADC, firmware replies, and draw emission are not inferred.
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
            branch_target: None,
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
            let branch_delay_slot = invocation.branch_target.is_some();
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
            if word & 8 != 0 || ((delay_slot || branch_delay_slot) && exit) {
                return Err(MacroError::UnsupportedInstruction(word));
            }
            let source = ((word >> 11) & 7) as usize;
            if !initialized[source] {
                return Err(MacroError::UninitializedRegister(source as u8));
            }
            if word & 7 == 7 {
                // BRAZ/BRANZ: signed 18-bit displacement in instruction words,
                // relative to this instruction, not the following one. Taken
                // ANNUL branches skip the slot; non-taken ANNUL still falls
                // through. EXIT applies only when the branch is not taken.
                if word & 0x748 != 0 || delay_slot || branch_delay_slot {
                    return Err(MacroError::UnsupportedInstruction(word));
                }
                let taken = (registers[source] != 0) == (word & 0x10 != 0);
                if taken {
                    let target = pc as i64 + i64::from((word as i32) >> 14);
                    if !(0..CODE_WORDS as i64).contains(&target) {
                        return Err(MacroError::InvalidBranchTarget(target));
                    }
                    invocation.finish_instruction(false, false);
                    if word & 0x20 != 0 {
                        invocation.pc = target as usize;
                    } else {
                        invocation.branch_target = Some(target as usize);
                    }
                } else {
                    invocation.finish_instruction(exit, false);
                }
                continue;
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
    fn request(engine: &mut MacroEngine, value: u32, mask: u32) {
        for (method, data) in [(0x3400, 0), (0x3404, value), (0x3408, mask)] {
            engine.upload(method, data).unwrap();
        }
    }
    #[test]
    fn privileged_initialization_updates_only_known_masked_bits_before_acknowledgement() {
        let mut engine = MacroEngine::default();
        for (address, value, mask) in PRIVILEGED_INITIALIZATION {
            request(&mut engine, value, mask);
            assert_eq!(engine.scratch[0], Some(0));
            engine
                .initialize_privileged_register(address, true)
                .unwrap();
            assert_eq!(engine.scratch[0], Some(1));
            let i = PRIVILEGED_ADDRESSES
                .iter()
                .position(|&a| a == address)
                .unwrap();
            assert_eq!(engine.privileged[i].value & mask, value);
            assert_eq!(engine.privileged[i].known_mask & mask, mask);
        }
        assert_eq!(
            engine.privileged[0],
            PrivilegedBits {
                value: 1,
                known_mask: 0x01800001
            }
        );
        assert_eq!(
            engine.privileged[4],
            PrivilegedBits {
                value: 3,
                known_mask: 3
            }
        );
        request(&mut engine, 1, 1);
        engine
            .initialize_privileged_register(0x419a04, true)
            .unwrap();
        assert_eq!(
            engine.privileged[4],
            PrivilegedBits {
                value: 3,
                known_mask: 3
            }
        );
        // Independent channel/context reset values are unknown, not copied.
        assert_eq!(
            MacroEngine::default().privileged,
            [PrivilegedBits::default(); 5]
        );
    }
    #[test]
    fn privileged_requests_reject_missing_idle_inputs_and_unknown_modes_without_ack() {
        let mut engine = MacroEngine::default();
        assert_eq!(
            engine.initialize_privileged_register(0x418800, false),
            Err(MacroError::MissingFirmwareIdle)
        );
        for index in 0..3 {
            assert_eq!(
                engine.initialize_privileged_register(0x418800, true),
                Err(MacroError::UninitializedScratch(0x3400 + index * 4))
            );
            engine
                .upload(0x3400 + index * 4, if index == 0 { 0 } else { 1 })
                .unwrap();
        }
        for (address, value, mask) in [
            (0x418804, 1, 1),
            (0x418800, 0, 1),
            (0x418800, 1, 0),
            (0x418800, 3, 1),
            (0x418800, 1, 0x01800001),
            (0x419a04, 3, 3),
            (0x418800, 0x01800000, 0x01800000),
        ] {
            request(&mut engine, value, mask);
            let before = engine.privileged;
            assert_eq!(
                engine.initialize_privileged_register(address, true),
                Err(MacroError::UnsupportedPrivilegedWrite {
                    address,
                    value,
                    mask
                })
            );
            assert_eq!(engine.privileged, before);
            assert_eq!(engine.scratch[0], Some(0));
        }
        request(&mut engine, 1, 1);
        engine.upload(0x3400, 7).unwrap();
        assert_eq!(
            engine.initialize_privileged_register(0x418800, true),
            Err(MacroError::InvalidFirmwareRequest(7))
        );
        assert_eq!(engine.scratch[0], Some(7));
        assert_eq!(engine.privileged, [PrivilegedBits::default(); 5]);
    }
    #[test]
    fn independently_assembled_branches_select_masks_and_execute_backward_slots() {
        let words: Vec<u32> = include_str!("../tests/fixtures/mme/branch-state.words")
            .split_whitespace()
            .map(|w| u32::from_str_radix(w, 16).unwrap())
            .collect();
        assert_eq!(
            words,
            [
                0x01a00021, 0xffffca11, 0x00011037, 0x00004041, 0x0001c027, 0x04444041, 0x00008311,
                0xffffdb11, 0xffffd817, 0x11, 0x04444041, 0x91, 0x11,
            ]
        );
        let engine = loaded(&words);
        for (parameter, expected, steps) in [(1, 1, 7), (0x1111, 0x1111, 13)] {
            assert_eq!(engine.execute(0, parameter), Ok((0x1a00, expected)));
            let mut run = engine.begin(0, parameter).unwrap();
            assert_eq!(engine.next_output(&mut run), Ok(Some((0x1a00, expected))));
            assert_eq!(engine.next_output(&mut run), Ok(None));
            assert!(run.finished);
            assert_eq!(run.steps, steps);
            assert_eq!(run.registers[0], 0);
        }
    }
    #[test]
    fn branch_exit_is_conditional_and_annul_only_skips_taken_slots() {
        // envyas: maddr 0x680; exit braz [annul] $r1 #again;
        // send (add $r0 1); send (add $r0 0x1111);
        // again: exit send (add $r0 0x1111); nop;
        for branch in [0x0000c887, 0x0000c8a7] {
            let engine = loaded(&[0x01a00021, branch, 0x4041, 0x04444041, 0x044440c1, 0x11]);
            // Not taken: even ANNUL executes the delay-slot SEND and exits.
            assert_eq!(engine.execute(0, 1), Ok((0x1a00, 1)));
            let mut run = engine.begin(0, 0).unwrap();
            if branch & 0x20 == 0 {
                assert_eq!(engine.next_output(&mut run), Ok(Some((0x1a00, 1))));
            }
            // Taken: branch's EXIT is suppressed, then target EXIT executes.
            assert_eq!(engine.next_output(&mut run), Ok(Some((0x1a00, 0x1111))));
            assert_eq!(engine.next_output(&mut run), Ok(None));
            assert!(run.finished);
        }
    }
    #[test]
    fn delayed_branch_retains_target_across_parameter_and_backend_yields() {
        // envyas: maddr 0x680; bra #target; parm $r2;
        // send (add $r0 999); target: exit send (add $r2 0); nop;
        let engine = loaded(&[0x01a00021, 0xc007, 0x201, 0x00f9c041, 0x10c0, 0x11]);
        let mut run = engine.begin(0, 0).unwrap();
        assert_eq!(engine.next_output(&mut run), Ok(None));
        assert!(run.waiting_for_parameter());
        assert_eq!(run.branch_target, Some(4));
        assert_eq!(run.steps, 2);
        run.provide_parameter(0x89abcdef).unwrap();
        assert_eq!(engine.next_output(&mut run), Ok(Some((0x1a00, 0x89abcdef))));
        assert_eq!(engine.next_output(&mut run), Ok(None));
        assert_eq!(run.steps, 5);
        // Same branch, but a SEND in its slot. Resumption must start at the
        // target, never at the skipped emission of 999.
        let engine = loaded(&[0x01a00021, 0xc007, 0x840, 0x00f9c041, 0x044440c1, 0x11]);
        let mut run = engine.begin(0, 7).unwrap();
        assert_eq!(engine.next_output(&mut run), Ok(Some((0x1a00, 7))));
        assert_eq!(run.pc, 4);
        assert_eq!(engine.next_output(&mut run), Ok(Some((0x1a00, 0x1111))));
        assert_eq!(engine.next_output(&mut run), Ok(None));
    }
    #[test]
    fn branches_reject_invalid_targets_reserved_bits_and_delay_slot_control_flow() {
        for (word, target) in [
            (0xffffc027, -1),
            (0x7fffc027, 131071),
            (0x80000027, -131072),
        ] {
            assert_eq!(
                loaded(&[word]).execute(0, 0),
                Err(MacroError::InvalidBranchTarget(target))
            );
        }
        // An untaken branch must not fetch an invalid speculative target.
        assert_eq!(
            loaded(&[0xffffc817, 0x91, 0x11]).execute(0, 0),
            Err(MacroError::UnsupportedEmissionCount)
        );
        assert_eq!(
            loaded(&[0x1027]).execute(0, 0),
            Err(MacroError::UninitializedRegister(2))
        );
        for bit in [3, 6, 8, 9, 10] {
            let word = 0x27 | (1 << bit);
            assert_eq!(
                loaded(&[word]).execute(0, 0),
                Err(MacroError::UnsupportedInstruction(word))
            );
        }
        // A branch in either another branch's or EXIT's delay slot is invalid.
        for words in [[0x8007, 0x27], [0x91, 0x27]] {
            assert_eq!(
                loaded(&words).execute(0, 0),
                Err(MacroError::UnsupportedInstruction(0x27))
            );
        }
        // EXIT in a branch delay slot is conservatively unsupported.
        assert_eq!(
            loaded(&[0x8007, 0x91]).execute(0, 0),
            Err(MacroError::UnsupportedInstruction(0x91))
        );
        // Taken target is in RAM but not uploaded: no fabricated NOP/EXIT.
        assert_eq!(
            loaded(&[0x8027]).execute(0, 0),
            Err(MacroError::MissingInstruction(2))
        );
    }
    #[test]
    fn backward_branches_cannot_reset_the_total_execution_budget() {
        for words in [&[0x27][..], &[0x7, 0x11][..]] {
            let engine = loaded(words);
            let mut run = engine.begin(0, 0).unwrap();
            assert_eq!(engine.next_output(&mut run), Err(MacroError::MissingExit));
            assert_eq!(run.steps, MAX_STEPS);
        }
        // SEND in a backward branch's delay slot yields on each iteration.
        let engine = loaded(&[0x01a00021, 0x7, 0x840]);
        let mut run = engine.begin(0, 1).unwrap();
        for _ in 0..31 {
            assert_eq!(engine.next_output(&mut run), Ok(Some((0x1a00, 1))));
        }
        assert_eq!(engine.next_output(&mut run), Err(MacroError::MissingExit));
        assert_eq!(run.steps, MAX_STEPS);
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
