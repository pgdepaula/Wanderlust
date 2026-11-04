//! Bounded nvhost controller for already completed channel fences.
//! Wire ABI: libnx feebd026 nvhost-ctrl.c; event identity: Ryujinx 4930dced
//! NvHostCtrlDeviceFile. Registration is not completion or a syncpoint increment.
use crate::{NvError, Result, out32, word};

#[derive(Debug)]
pub(super) struct HostControl {
    events: [bool; 64],
}
impl Default for HostControl {
    fn default() -> Self {
        Self {
            events: [false; 64],
        }
    }
}
impl HostControl {
    pub(super) fn query(&self, event: u32) -> Result<bool> {
        let (slot, syncpoint) = if event >> 28 == 1 {
            (event & 0xffff, (event >> 16) & 0xfff)
        } else {
            (event & 0xff, event >> 4)
        };
        // Only available (unarmed) registered events are currently supported.
        // They start with fence ID zero and are never signalled by registration.
        if syncpoint != 0 || !self.events.get(slot as usize).copied().unwrap_or(false) {
            return Err(NvError::BadParameter.into());
        }
        Ok(false)
    }
    pub(super) fn ioctl(
        &mut self,
        request: u32,
        input: &[u8],
        output: &mut [u8],
        completed: impl Fn(u32) -> Result<u32>,
    ) -> Result<()> {
        match request {
            0xc004001f | 0xc0040020 | 0xc004001c => {
                let id = word(input, 0);
                let id = if request == 0xc004001c {
                    id & 0xffff
                } else {
                    id
                };
                let event = self
                    .events
                    .get_mut(id as usize)
                    .ok_or(NvError::BadParameter)?;
                match request {
                    0xc004001f => *event = true,
                    0xc0040020 => *event = false,
                    _ if !*event => return Err(NvError::BadParameter.into()),
                    _ => {} // Cancel an available event; do not signal it.
                }
                output.copy_from_slice(input);
                Ok(())
            }
            0xc0080014 => {
                let value = completed(word(input, 0))?;
                output.copy_from_slice(input);
                out32(output, 4, value);
                Ok(())
            }
            0xc00c0016 | 0xc010001d | 0xc010001e => {
                let value = completed(word(input, 0))?;
                if value < word(input, 4) {
                    // Future waits require suspended service execution and the
                    // exact nvdrv TryAgain ABI. Never fabricate an expired fence.
                    return Err(NvError::NotSupported.into());
                }
                output.copy_from_slice(input);
                if input.len() == 16 {
                    out32(output, 12, value);
                }
                Ok(())
            }
            _ => Err(NvError::NotSupported.into()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn call(c: &mut HostControl, request: u32, words: &[u32], current: u32) -> Result<Vec<u8>> {
        let input: Vec<_> = words.iter().flat_map(|w| w.to_le_bytes()).collect();
        let mut output = vec![0x55; input.len()];
        c.ioctl(request, &input, &mut output, |id| {
            if id == 1 {
                Ok(current)
            } else {
                Err(NvError::BadParameter.into())
            }
        })?;
        Ok(output)
    }
    #[test]
    fn registration_is_fd_local_unarmed_and_bounded() {
        let mut c = HostControl::default();
        let other = HostControl::default();
        for id in [0, 15, 16, 63] {
            call(&mut c, 0xc004001f, &[id], 0).unwrap();
            assert!(!c.query(0x10000000 | id).unwrap());
            assert!(other.query(0x10000000 | id).is_err());
            call(&mut c, 0xc004001c, &[0x10000000 | id], 0).unwrap();
            assert!(!c.query(0x10000000 | id).unwrap());
            call(&mut c, 0xc0040020, &[id], 0).unwrap();
            assert!(c.query(0x10000000 | id).is_err());
        }
        assert!(call(&mut c, 0xc004001f, &[64], 0).is_err());
        assert!(call(&mut c, 0xc004001f, &[u32::MAX], 0).is_err());
    }
    #[test]
    fn completed_wait_reports_actual_value_and_future_wait_does_not_write() {
        let mut c = HostControl::default();
        for request in [0xc010001d, 0xc010001e] {
            let out = call(&mut c, request, &[1, 7, u32::MAX, 63], 9).unwrap();
            assert_eq!(word(&out, 12), 9);
        }
        let input: Vec<_> = [1u32, 10, u32::MAX, 0]
            .iter()
            .flat_map(|w| w.to_le_bytes())
            .collect();
        let mut out = [0x55; 16];
        assert!(c.ioctl(0xc010001e, &input, &mut out, |_| Ok(9)).is_err());
        assert_eq!(out, [0x55; 16]);
        assert!(c.query(0x10000000).is_err());
        assert!(call(&mut c, 0xc0080014, &[2, 0], 9).is_err());
    }
}
