//! Process-local, bounded prepo:u report storage. No network transmission.
//! Command signatures: PrepoService.cs at Ryujinx 6f60e102a29aa04fff5c6bcdec37213b2d8ac646.
use std::collections::VecDeque;
use wl_guest_runtime::{Memory, RuntimeResult, audio::Reply, ipc::Request};

pub const INDEX: usize = 1 << 26;
pub const NAME: [u8; 8] = *b"prepo:u\0";
const MAX_REPORT: u64 = 16 * 1024;
const MAX_REPORTS: usize = 64;

pub struct Report {
    pub process_id: u64,
    pub user_id: Option<[u8; 16]>,
    pub room: Vec<u8>,
    pub payload: Vec<u8>,
}

pub struct Service {
    reports: VecDeque<Report>,
    agreement_check: bool,
}
impl std::fmt::Debug for Service {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("LocalPrepo")
            .field("report_count", &self.reports.len())
            .field("agreement_check", &self.agreement_check)
            .finish()
    }
}
impl Default for Service {
    fn default() -> Self {
        Self {
            reports: VecDeque::new(),
            agreement_check: true,
        }
    }
}
impl Service {
    pub fn reports(&self) -> &VecDeque<Report> {
        &self.reports
    }

    pub fn request(
        &mut self,
        memory: &Memory,
        process_id: u64,
        command: u32,
        req: &Request,
        args: &[u8],
    ) -> RuntimeResult<Reply> {
        let report = matches!(command, 10100..=10105);
        if !report && args.len() > 16 {
            return Err("unsupported prepo scalar data size".into());
        }
        if !req.recvs.is_empty()
            || req.exchanges != 0
            || req.copy_handles != 0
            || req.move_handles != 0
            || !req.copies.is_empty()
            || (!report && (req.pid.is_some() || !req.statics.is_empty() || !req.sends.is_empty()))
        {
            return Err("unsupported prepo request descriptors".into());
        }
        let failure = |description: u32| Reply::failure(129 | (description << 9));
        match command {
            10100..=10105 => {
                if req.pid != Some(process_id) {
                    return Ok(failure(101));
                }
                let with_user = command & 1 != 0;
                let scalar_bytes = if with_user { 16 } else { 0 };
                // CMIF may include the ClientProcessId scalar placeholder and
                // alignment padding. Only the kernel-supplied PID is trusted.
                if args.len() < scalar_bytes
                    || args.len() > scalar_bytes + 16
                    || req.statics.len() != 1
                    || req.sends.len() != 1
                {
                    return Err("unsupported prepo report layout".into());
                }
                let user_id = with_user.then(|| <[u8; 16]>::try_from(&args[..16]).unwrap());
                if user_id == Some([0; 16]) {
                    return Ok(failure(1));
                }
                let (room_address, room_size) = req.statics[0];
                let (report_address, report_size) = req.sends[0];
                if room_size > 31 {
                    return Ok(failure(1));
                }
                if report_size == 0 || report_size > MAX_REPORT {
                    return Ok(failure(9));
                }
                let mut room = vec![0; room_size as usize];
                memory.read_into(room_address, &mut room)?;
                if room.is_empty() || room.iter().all(|b| b.is_ascii_whitespace() || *b == 0) {
                    return Ok(failure(5));
                }
                let mut payload = vec![0; report_size as usize];
                memory.read_into(report_address, &mut payload)?;
                // All checked reads precede the only state change. Reports are
                // opaque bytes, not evaluated, logged or uploaded.
                if self.reports.len() == MAX_REPORTS {
                    self.reports.pop_front();
                }
                self.reports.push_back(Report {
                    process_id,
                    user_id,
                    room,
                    payload,
                });
                Ok(Reply::ok())
            }
            10300 => Ok(Reply::data(&0i32.to_le_bytes())), // no transmission in progress
            40100 => Ok(Reply::data(&[u8::from(self.agreement_check)])),
            40101 if matches!(args.first(), Some(0 | 1)) => {
                self.agreement_check = args[0] != 0;
                Ok(Reply::ok())
            }
            // A local store cannot promise Internet delivery or a persistent
            // system session identity. Keep these explicitly unsupported.
            _ => Err(format!("unsupported prepo:u command {command}")),
        }
    }
}
