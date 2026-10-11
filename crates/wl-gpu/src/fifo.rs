//! Maxwell B06F pushbuffer method packets, separate from host execution commands.
//! Field definitions: NVIDIA open-gpu-doc classes/host/clb06f.h; see graphics sources.
//! No engine registers, GPFIFO entry fetching or semaphore effects are executed here.

/// A decoded method write. `method` is a byte offset, not the packet's word address.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MethodWrite {
    pub subchannel: u8,
    pub method: u16,
    pub value: u32,
}

/// Errors are compact values; decoding does not allocate even on malformed packets.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DecodeError {
    UnsupportedOpcode(u8),
    MethodOverflow,
    Truncated { remaining: u16 },
}
impl std::fmt::Display for DecodeError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::UnsupportedOpcode(opcode) => write!(f, "unsupported FIFO opcode {opcode}"),
            Self::MethodOverflow => f.write_str("FIFO method range exceeds 12-bit address"),
            Self::Truncated { remaining } => write!(f, "FIFO packet missing {remaining} words"),
        }
    }
}
impl std::error::Error for DecodeError {}

#[derive(Debug, Clone, Copy)]
enum Step {
    Every,
    Never,
    Once,
}

#[derive(Debug, Clone, Copy)]
struct Pending {
    method: u16,
    subchannel: u8,
    remaining: u16,
    step: Step,
}

/// One channel's packet state. Keep it across pushbuffer chunks in submission order.
/// `push_word` consumes one little-endian-decoded word, without retaining its source.
/// A rejected header leaves the decoder idle; callers must treat the error as a
/// channel fault rather than silently resuming at an arbitrary following word.
#[derive(Debug, Default)]
pub struct Decoder {
    pending: Option<Pending>,
}
impl Decoder {
    pub fn push_word(&mut self, word: u32) -> Result<Option<MethodWrite>, DecodeError> {
        if let Some(mut pending) = self.pending.take() {
            let output = MethodWrite {
                subchannel: pending.subchannel,
                method: pending.method * 4,
                value: word,
            };
            pending.remaining -= 1;
            if pending.remaining != 0 {
                match pending.step {
                    Step::Every => pending.method += 1,
                    Step::Never => {}
                    Step::Once => {
                        pending.method += 1;
                        pending.step = Step::Never;
                    }
                }
                self.pending = Some(pending);
            }
            return Ok(Some(output));
        }

        // NVIDIA defines exactly zero as the DMA no-operation word. Other group-0
        // packets use legacy/tertiary formats and are deliberately unsupported.
        if word == 0 {
            return Ok(None);
        }
        let opcode = (word >> 29) as u8;
        let method = (word & 0xfff) as u16;
        let subchannel = ((word >> 13) & 7) as u8;
        let count = ((word >> 16) & 0x1fff) as u16;
        if opcode == 4 {
            return Ok(Some(MethodWrite {
                subchannel,
                method: method * 4,
                value: u32::from(count),
            }));
        }
        let step = match opcode {
            1 => Step::Every,
            3 => Step::Never,
            5 => Step::Once,
            other => return Err(DecodeError::UnsupportedOpcode(other)),
        };
        if count == 0 {
            return Ok(None);
        }
        let last = match step {
            Step::Every => u32::from(method) + u32::from(count) - 1,
            Step::Never => u32::from(method),
            Step::Once => u32::from(method) + u32::from(count > 1),
        };
        if last > 0xfff {
            return Err(DecodeError::MethodOverflow);
        }
        self.pending = Some(Pending {
            method,
            subchannel,
            remaining: count,
            step,
        });
        Ok(None)
    }

    /// Call only at a logical stream end, not at every GPFIFO buffer boundary.
    /// An incomplete packet remains available for continuation after this check.
    pub fn finish(&self) -> Result<(), DecodeError> {
        match self.pending {
            Some(pending) => Err(DecodeError::Truncated {
                remaining: pending.remaining,
            }),
            None => Ok(()),
        }
    }
}
