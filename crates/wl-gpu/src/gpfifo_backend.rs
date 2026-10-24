//! Borrow command spans only until the next draw; release before mutable GPU work.
use crate::{
    backend::Renderer,
    gpfifo::*,
    invalid,
    maxwell::{Channel, MethodEffect},
    memory::*,
    texture::TextureChanges,
};
impl Renderer {
    /// Executes a bounded array of little-endian B06F GPFIFO entries. This is not
    /// an nvdrv ioctl, ring PUT/GET implementation, syncpoint or completion fence.
    /// Metadata is preflighted before any methods; fetch/execution faults preserve
    /// earlier submitted draws and latch the channel, making whole-array retry fail.
    /// Packets and engine state continue across entries and calls; use Channel::finish
    /// only when the caller declares the logical command stream ended.
    /// All payload acquisition is deferred to the current entry (also for
    /// PROCEED), honoring WAIT without prefetching later entries. Descriptor
    /// LEVEL does not introduce control flow; unsupported packet control flow
    /// still faults. No USERD GET/resume-register emulation is claimed.
    pub fn submit_gpfifo_deferred(
        &mut self,
        channel: &mut Channel,
        entries: &[u8],
        map: &AddressMap,
        memory: &mut impl GuestMemoryWrite,
        changes: &mut TextureChanges,
    ) -> std::result::Result<Progress, SubmissionFault> {
        let mut progress = Progress::default();
        let before_draws = self.counters.gpu_target_writes;
        let mut phase = FaultPhase::Preflight;
        let result = (|| {
            if channel.is_faulted() {
                return Err(invalid("GPU channel fault is latched"));
            }
            let submission =
                Submission::new_counted(entries, &mut self.counters.gpfifo_entry_decodes)?;
            channel
                .engine
                .begin_submission()
                .map_err(|e| crate::Error(e.to_string()))?;
            self.counters.gpfifo_submissions += 1;
            for entry in submission.entries() {
                self.counters.gpfifo_entry_decodes += 1;
                phase = FaultPhase::Fetch;
                // Reject adjacent separate mappings even if each word alone maps.
                let base = map.resolve(entry.address(), entry.byte_len())?;
                let mut offset = 0usize;
                while offset < entry.byte_len() {
                    let address = GuestAddress(
                        base.0
                            .checked_add(offset as u64)
                            .ok_or_else(|| invalid("command guest address overflow"))?,
                    );
                    let remaining = entry.byte_len() - offset;
                    self.synchronize_command_source(address, remaining, memory, changes)?;
                    let mut draw = None;
                    {
                        self.counters.gpfifo_read_calls += 1;
                        let bytes = memory.read(address, remaining)?;
                        if bytes.len() != remaining {
                            return Err(invalid("short guest GPFIFO command read"));
                        }
                        self.counters.gpfifo_borrowed_bytes += remaining as u64;
                        phase = FaultPhase::Execute;
                        for word in bytes.chunks_exact(4) {
                            offset += 4;
                            progress.words_consumed += 1;
                            self.counters.gpfifo_command_bytes += 4;
                            // Scalar endian decoding, no owned command-buffer copy.
                            let effect =
                                channel.push_word(u32::from_le_bytes(word.try_into().unwrap()))?;
                            if matches!(
                                effect,
                                MethodEffect::Draw { .. }
                                    | MethodEffect::DrawIndices { .. }
                                    | MethodEffect::Clear { .. }
                                    | MethodEffect::ConstantWrite { .. }
                                    | MethodEffect::WaitIdle
                            ) {
                                draw = Some(effect);
                                break;
                            }
                        }
                    } // No guest borrow survives a mutable draw or conditional barrier.
                    if let Some(effect) = draw {
                        self.channel_effect(channel, effect, map, memory, changes, false)?;
                    }
                    phase = FaultPhase::Fetch;
                }
                progress.entries_completed += 1;
            }
            if channel.engine.macro_waiting_for_parameter() {
                phase = FaultPhase::Execute;
                return Err(invalid(
                    "unsupported MME parameter continuation across submissions",
                ));
            }
            Ok(())
        })();
        progress.draws_submitted = self.counters.gpu_target_writes - before_draws;
        match result {
            Ok(()) => Ok(progress),
            Err(cause) => {
                channel.faulted = true;
                self.vertex_ready = false;
                if !self.target_pending {
                    self.guest_target = None;
                }
                Err(SubmissionFault {
                    phase,
                    progress,
                    cause,
                })
            }
        }
    }
}
