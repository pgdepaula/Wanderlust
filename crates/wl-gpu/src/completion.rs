//! Host queue progress deadlines. These bound polling waits, not driver calls
//! that themselves block, and do not implement hardware instruction preemption.
use crate::{Counters, Error, Result};
use std::{
    sync::{
        Arc,
        atomic::{AtomicBool, AtomicU64, Ordering},
        mpsc,
    },
    time::{Duration, Instant},
};
#[derive(Default)]
pub(crate) struct CompletionWatchdog {
    timeout: Option<Duration>,
    completed: Arc<AtomicU64>,
}
struct ProgressDeadline {
    timeout: Duration,
    last_progress: Instant,
    completed: u64,
}
impl ProgressDeadline {
    fn observe(&mut self, now: Instant, completed: u64) -> bool {
        if self.completed != completed {
            self.completed = completed;
            self.last_progress = now;
        }
        now.saturating_duration_since(self.last_progress) >= self.timeout
    }
}
impl CompletionWatchdog {
    pub(crate) fn configure(&mut self, timeout: Option<Duration>) {
        self.timeout = timeout;
    }
    pub(crate) fn track(&self, queue: &wgpu::Queue) {
        if self.timeout.is_some() {
            let completed = self.completed.clone();
            queue.on_submitted_work_done(move || {
                completed.fetch_add(1, Ordering::Release);
            });
        }
    }
    fn poll_until(
        &mut self,
        device: &wgpu::Device,
        counters: &mut Counters,
        mut ready: impl FnMut() -> Result<bool>,
    ) -> Result<()> {
        let timeout = self.timeout.expect("finite polling deadline");
        let mut deadline = ProgressDeadline {
            timeout,
            last_progress: Instant::now(),
            completed: self.completed.load(Ordering::Acquire),
        };
        loop {
            device
                .poll(wgpu::PollType::Poll)
                .map_err(|e| Error(e.to_string()))?;
            if ready()? {
                return Ok(());
            }
            if deadline.observe(Instant::now(), self.completed.load(Ordering::Acquire)) {
                counters.completion_timeouts += 1;
                return Err(Error(format!(
                    "GPU completion made no observed queue progress for {} ms",
                    timeout.as_millis()
                )));
            }
            std::thread::yield_now();
        }
    }
    /// `done` belongs to this exact queue prefix; a later submission cannot
    /// replace its identity or make an earlier token wait for later work.
    pub(crate) fn wait(
        &mut self,
        device: &wgpu::Device,
        index: wgpu::SubmissionIndex,
        done: &AtomicBool,
        counters: &mut Counters,
    ) -> Result<()> {
        if self.timeout.is_none() {
            device
                .poll(wgpu::PollType::wait_for(index))
                .map_err(|e| Error(e.to_string()))?;
            return Ok(());
        }
        self.poll_until(device, counters, || Ok(done.load(Ordering::Acquire)))
    }
    /// Completion is the actual map callback, not merely queue acceptance.
    pub(crate) fn mapping(
        &mut self,
        device: &wgpu::Device,
        index: wgpu::SubmissionIndex,
        rx: &mpsc::Receiver<std::result::Result<(), wgpu::BufferAsyncError>>,
        counters: &mut Counters,
    ) -> Result<()> {
        if self.timeout.is_none() {
            device
                .poll(wgpu::PollType::wait_for(index))
                .map_err(|e| Error(e.to_string()))?;
            return rx
                .recv_timeout(Duration::from_secs(30))
                .map_err(|e| Error(e.to_string()))?
                .map_err(|e| Error(e.to_string()));
        }
        self.poll_until(device, counters, || match rx.try_recv() {
            Ok(result) => result.map(|()| true).map_err(|e| Error(e.to_string())),
            Err(mpsc::TryRecvError::Empty) => Ok(false),
            Err(mpsc::TryRecvError::Disconnected) => {
                Err(Error("GPU mapping callback disconnected".into()))
            }
        })
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    #[ignore = "requires actual wgpu device; deterministic deadline failure and retry"]
    fn polling_expiry_does_not_submit_or_publish_and_retry_is_possible() {
        let mut renderer = crate::backend::Renderer::new().unwrap();
        renderer.completion.configure(Some(Duration::ZERO));
        let before = renderer.counters;
        let error = renderer
            .completion
            .poll_until(&renderer.device, &mut renderer.counters, || Ok(false))
            .unwrap_err();
        assert!(error.0.contains("no observed queue progress"));
        let mut expected = before;
        expected.completion_timeouts += 1;
        assert_eq!(renderer.counters, expected);
        renderer
            .completion
            .configure(Some(Duration::from_millis(614)));
        renderer
            .completion
            .poll_until(&renderer.device, &mut renderer.counters, || Ok(true))
            .unwrap();
        assert_eq!(renderer.counters, expected);
    }
    #[test]
    fn timeout_is_no_progress_not_total_duration_and_never_moves_on_polling() {
        let start = Instant::now();
        let mut deadline = ProgressDeadline {
            timeout: Duration::from_millis(614),
            last_progress: start,
            completed: 8,
        };
        assert!(!deadline.observe(start + Duration::from_millis(613), 8));
        assert!(deadline.observe(start + Duration::from_millis(614), 8));
        // Independent completion resets the window, even in a long workload.
        assert!(!deadline.observe(start + Duration::from_millis(615), 9));
        assert!(!deadline.observe(start + Duration::from_millis(1228), 9));
        assert!(deadline.observe(start + Duration::from_millis(1229), 9));
    }
    #[test]
    fn zero_deadline_and_counter_wrap_are_checked_without_sleeping() {
        let now = Instant::now();
        let mut deadline = ProgressDeadline {
            timeout: Duration::ZERO,
            last_progress: now,
            completed: u64::MAX,
        };
        assert!(deadline.observe(now, u64::MAX));
        deadline.timeout = Duration::from_millis(1);
        assert!(!deadline.observe(now, 0));
        assert!(deadline.observe(now + Duration::from_millis(1), 0));
    }
}
