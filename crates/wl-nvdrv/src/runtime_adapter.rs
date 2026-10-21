//! This module is compiled in the generated executable, not in wl-nvdrv.
use wl_gpu::memory::{GuestAddress, GuestMemory, GuestMemoryWrite};
use wl_guest_runtime::{Memory, gpu_memory::GpuPin, nvidia as service};
use wl_nvdrv as driver;
struct Capture {
    directory: std::path::PathBuf,
    remaining: u32,
    index: u32,
    bytes: u64,
}
impl Capture {
    fn create(root: &std::path::Path, limit: u32) -> std::io::Result<Self> {
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_err(std::io::Error::other)?
            .as_nanos();
        Self::create_at(root, limit, now)
    }
    fn create_at(root: &std::path::Path, limit: u32, now: u128) -> std::io::Result<Self> {
        if !(1..=64).contains(&limit) {
            return Err(std::io::Error::other("capture limit must be 1..64"));
        }
        std::fs::create_dir_all(root)?;
        // Clock resolution is not a uniqueness guarantee, including on Windows.
        static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        let sequence = NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let directory = root.join(format!("run-{}-{now}-{sequence}", std::process::id()));
        std::fs::create_dir(&directory)?;
        Ok(Self {
            directory,
            remaining: limit,
            index: 0,
            bytes: 0,
        })
    }
    fn write(&mut self, frame: &wl_gpu::backend::Frame, draws: u64) -> std::io::Result<bool> {
        use std::io::Write;
        if self.remaining == 0 {
            return Ok(false);
        }
        let size = u64::from(frame.width)
            .checked_mul(u64::from(frame.height))
            .and_then(|n| n.checked_mul(4))
            .filter(|n| *n != 0 && *n == frame.rgba.len() as u64)
            .ok_or_else(|| std::io::Error::other("invalid published color image extent"))?;
        if size > (1 << 30) - self.bytes {
            self.remaining = 0;
            return Ok(false);
        }
        let index = self.index + 1;
        let base = format!("color-{index:04}");
        let temporary = self.directory.join(format!("{base}.rgba.part"));
        let mut output = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&temporary)?;
        output.write_all(&frame.rgba)?;
        drop(output);
        std::fs::rename(temporary, self.directory.join(format!("{base}.rgba")))?;
        // The metadata is the commit marker. A partial raw image is not a capture.
        let temporary = self.directory.join(format!("{base}.json.part"));
        let mut output = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&temporary)?;
        write!(
            output,
            "{{\"kind\":\"published-color-target\",\"index\":{index},\"width\":{},\"height\":{},\"rgba_bytes\":{size},\"draw_commands\":{draws}}}",
            frame.width, frame.height
        )?;
        drop(output);
        std::fs::rename(temporary, self.directory.join(format!("{base}.json")))?;
        self.index = index;
        self.bytes += size;
        self.remaining -= 1;
        eprintln!(
            "NVIDIA captured published color target {index} ({}x{}), not a VI presentation event",
            frame.width, frame.height
        );
        Ok(true)
    }
}
struct MemoryView<'a>(&'a mut Memory);
impl GuestMemory for MemoryView<'_> {
    fn read(&self, address: GuestAddress, length: usize) -> wl_gpu::Result<&[u8]> {
        self.0.read_bytes(address.0, length).map_err(wl_gpu::Error)
    }
}
impl GuestMemoryWrite for MemoryView<'_> {
    fn validate_write(&self, address: GuestAddress, length: usize) -> wl_gpu::Result<()> {
        self.0
            .validate_write(address.0, length)
            .map_err(wl_gpu::Error)
    }
    fn write(&mut self, address: GuestAddress, bytes: &[u8]) -> wl_gpu::Result<()> {
        self.0.write_bytes(address.0, bytes).map_err(wl_gpu::Error)
    }
}
impl driver::Memory for MemoryView<'_> {
    type Pin = GpuPin;
    fn pin(&mut self, base: GuestAddress, size: u64) -> wl_gpu::Result<GpuPin> {
        self.0.pin_gpu_range(base.0, size).map_err(wl_gpu::Error)
    }
    fn unpin(&mut self, pin: GpuPin) -> wl_gpu::Result<()> {
        self.0.release_gpu_pin(pin).map_err(wl_gpu::Error)
    }
}
fn error(e: driver::Error) -> service::HostError {
    match e {
        driver::Error::Fatal(message) => service::HostError::Fatal(message),
        driver::Error::Driver(e) => service::HostError::Driver(match e {
            driver::NvError::NotSupported => service::NvError::NotSupported,
            driver::NvError::BadParameter => service::NvError::BadParameter,
            driver::NvError::InvalidState => service::NvError::InvalidState,
            driver::NvError::InvalidAddress => service::NvError::InvalidAddress,
            driver::NvError::InvalidSize => service::NvError::InvalidSize,
            driver::NvError::AlreadyAllocated => service::NvError::AlreadyAllocated,
            driver::NvError::ResourceError => service::NvError::ResourceError,
            driver::NvError::IoctlFailed => service::NvError::IoctlFailed,
        }),
    }
}
pub struct NativeHost {
    driver: driver::Driver<GpuPin>,
    debug_window: Option<wl_guest_runtime::sdl::SdlHost>,
    last_frame: Option<wl_gpu::backend::Frame>,
    hold_window: bool,
    capture: Option<Capture>,
    trace_remaining: u32,
    published_targets: u64,
    status_path: Option<std::path::PathBuf>,
}
impl std::fmt::Debug for NativeHost {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("NativeHost")
            .field("driver", &self.driver)
            .field("debug_window", &self.debug_window.is_some())
            .finish_non_exhaustive()
    }
}
impl NativeHost {
    pub fn new() -> wl_gpu::Result<Self> {
        let driver = driver::Driver::new()?;
        eprintln!("NVIDIA adapter {:?}", driver.capabilities().adapter);
        let debug_window = if std::env::var("WL_GPU_DEBUG_WINDOW").as_deref() == Ok("1") {
            Some(
                wl_guest_runtime::sdl::SdlHost::open(
                    "TOTK - Wanderlust | SDL2 | initializing GPU | original frames: 0",
                    940,
                    580,
                )
                .ok_or_else(|| {
                    wl_gpu::Error(
                        "requested GPU diagnostic window could not open; SDL2 is required".into(),
                    )
                })?,
            )
        } else {
            None
        };
        let capture = std::env::var_os("WL_GPU_CAPTURE_DIR")
            .map(|root| {
                let limit = match std::env::var("WL_GPU_CAPTURE_LIMIT") {
                    Ok(value) => value
                        .parse::<u32>()
                        .map_err(|_| wl_gpu::Error("capture limit must be 1..64".into()))?,
                    Err(std::env::VarError::NotPresent) => 8,
                    Err(_) => return Err(wl_gpu::Error("invalid capture limit encoding".into())),
                };
                Capture::create(std::path::Path::new(&root), limit)
                    .map_err(|e| wl_gpu::Error(format!("GPU capture setup: {e}")))
            })
            .transpose()?;
        Ok(Self {
            driver,
            debug_window,
            last_frame: None,
            hold_window: std::env::var("WL_GPU_DEBUG_HOLD").as_deref() == Ok("1"),
            capture,
            published_targets: 0,
            status_path: std::env::var_os("WL_GPU_STATUS_FILE").map(std::path::PathBuf::from),
            trace_remaining: if std::env::var("WL_NVDRV_TRACE").as_deref() == Ok("1") {
                1024
            } else {
                0
            },
        })
    }
    pub fn graphics_counters(&self) -> wl_gpu::Counters {
        self.driver.graphics_counters()
    }
    pub fn driver_counters(&self) -> wl_gpu::driver_backend::DriverCounters {
        self.driver.driver_counters()
    }
    fn present_debug_output(&mut self) -> service::HostResult<()> {
        if let Some(frame) = self.driver.take_frame() {
            self.published_targets += 1;
            if let Some(capture) = &mut self.capture {
                capture
                    .write(&frame, self.driver.graphics_counters().draw_commands)
                    .map_err(|e| {
                        service::HostError::Fatal(format!(
                            "GPU capture after publication failed: {e}"
                        ))
                    })?;
            }
            if let Some(window) = &mut self.debug_window {
                window.poll_input();
                if !window.present_raw_rgba(&frame.rgba, frame.width, frame.height) {
                    return Err(service::HostError::Fatal(
                        "GPU diagnostic frame presentation failed".into(),
                    ));
                }
                self.last_frame = Some(frame);
            }
        }
        Ok(())
    }
}
impl Drop for NativeHost {
    fn drop(&mut self) {
        // Explicit interactive inspection after the guest exits. This loop
        // does not count as guest execution, new GPU draws or GPU completion.
        if !self.hold_window {
            return;
        }
        if let (Some(window), Some(frame)) = (&mut self.debug_window, &self.last_frame) {
            while !window.should_close {
                window.poll_input();
                if !window.present_raw_rgba(&frame.rgba, frame.width, frame.height) {
                    break;
                }
                std::thread::sleep(std::time::Duration::from_millis(8));
            }
        }
    }
}
impl service::Host for NativeHost {
    fn initialize(&mut self, _transfer: service::TransferBuffer) -> service::HostResult<()> {
        Ok(())
    }
    fn set_client_pid(&mut self, _aruid: u64, _pid: u64) -> service::HostResult<()> {
        Ok(())
    }
    fn open(&mut self, path: &str) -> service::HostResult<u32> {
        let result = self.driver.open(path).map_err(error);
        if self.trace_remaining != 0 {
            self.trace_remaining -= 1;
            let node = match path {
                "/dev/nvmap"
                | "/dev/nvhost-gpu"
                | "/dev/nvhost-as-gpu"
                | "/dev/nvhost-ctrl-gpu"
                | "/dev/nvhost-ctrl" => path,
                _ => "<unrecognized-device>",
            };
            eprintln!("NVIDIA open node={node} result={result:?}");
        }
        result
    }
    fn update_progress(&mut self, instructions: u64, elapsed: f64) {
        if self.debug_window.is_none() && self.status_path.is_none() {
            return;
        }
        let c = self.driver.graphics_counters();
        let adapter = &self.driver.capabilities().adapter;
        let mips = if elapsed > 0.0 {
            instructions as f64 / elapsed / 1_000_000.0
        } else {
            0.0
        };
        let lines = vec![
            "TOTK - Wanderlust".into(),
            "Original AArch64 execution - RUNNING".into(),
            format!(
                "SDL2 monitor | GPU execution backend: {:?}",
                adapter.backend
            ),
            format!("GPU: {}", adapter.name.replace(['\n', '\r'], " ")),
            format!("Retired A64 instructions: {instructions}"),
            format!("CPU throughput: {mips:.2} million instructions/s"),
            format!("Host elapsed: {elapsed:.1} s"),
            format!(
                "Maxwell command words consumed: {}",
                c.gpfifo_command_bytes / 4
            ),
            format!(
                "Maxwell draws: {} | host queue submissions: {}",
                c.draw_commands, c.submissions
            ),
            format!(
                "Translated shaders: {}",
                c.shader_decodes + c.vertex_shader_decodes
            ),
            format!("Published GPU color targets: {}", self.published_targets),
            "Original VI frames: not verified".into(),
            if self.published_targets == 0 {
                "Waiting for a published guest GPU image.".into()
            } else {
                "Published color targets are not verified VI frames.".into()
            },
            "This status canvas is not a game frame.".into(),
        ];
        if let Some(path) = &self.status_path {
            let temporary = path.with_extension(format!("{}.part", std::process::id()));
            if let Err(e) = std::fs::write(&temporary, lines.join("\n"))
                .and_then(|()| std::fs::rename(&temporary, path))
            {
                eprintln!("GPU diagnostic status disabled after write failure: {e}");
                self.status_path = None;
            }
        }
        if let Some(window) = &mut self.debug_window {
            window.set_title(&format!("TOTK - Wanderlust | SDL2 | {:?} | {mips:.2} MIPS | Maxwell draws: {} | targets: {}", adapter.backend, c.draw_commands, self.published_targets));
            window.poll_input();
            if let Some(frame) = &self.last_frame {
                window.present_raw_rgba(&frame.rgba, frame.width, frame.height);
            } else {
                window.present_frame(&wl_guest_runtime::display::DisplayServer::default());
            }
        }
    }
    fn query_event(&mut self, fd: u32, event: u32) -> service::HostResult<bool> {
        self.driver.query_event(fd, event).map_err(error)
    }
    fn close(&mut self, fd: u32, memory: &mut Memory) -> service::HostResult<()> {
        self.driver
            .close(fd, &mut MemoryView(memory))
            .map_err(error)
    }
    fn ioctl(&mut self, r: service::Ioctl<'_>, memory: &mut Memory) -> service::HostResult<()> {
        let request = r.request;
        let fd = r.fd;
        let kind = match r.kind {
            service::IoctlKind::Ioctl => driver::IoctlKind::Ioctl,
            service::IoctlKind::Ioctl2 => driver::IoctlKind::Ioctl2,
            service::IoctlKind::Ioctl3 => driver::IoctlKind::Ioctl3,
        };
        let result = self
            .driver
            .ioctl(
                driver::Ioctl {
                    fd: r.fd,
                    request: r.request,
                    kind,
                    input: r.input,
                    extra_input: r.extra_input,
                    output: r.output,
                    extra_output: r.extra_output,
                },
                &mut MemoryView(memory),
            )
            .map_err(error);
        if self.trace_remaining != 0 {
            self.trace_remaining -= 1;
            eprintln!(
                "NVIDIA ioctl fd={fd} request={request:#010x} kind={kind:?} result={result:?}"
            );
        }
        result?;
        self.present_debug_output()
    }
}

#[cfg(test)]
mod capture_tests {
    use super::*;
    fn frame() -> wl_gpu::backend::Frame {
        wl_gpu::backend::Frame {
            width: 2,
            height: 1,
            rgba: vec![1, 2, 3, 255, 7, 8, 9, 128],
            encode_time: std::time::Duration::ZERO,
            total_time: std::time::Duration::ZERO,
            gpu_nanoseconds: None,
        }
    }
    #[test]
    fn concurrent_capture_sessions_with_identical_clock_ticks_are_isolated() {
        let parent = Capture::create(&std::env::temp_dir(), 1).unwrap();
        let directory = &parent.directory;
        let barrier = std::sync::Barrier::new(8);
        let captures = std::thread::scope(|scope| {
            let workers: Vec<_> = (0..8)
                .map(|_| {
                    let barrier = &barrier;
                    scope.spawn(move || {
                        barrier.wait();
                        Capture::create_at(directory, 1, 0).unwrap()
                    })
                })
                .collect();
            workers
                .into_iter()
                .map(|w| w.join().unwrap())
                .collect::<Vec<_>>()
        });
        let names: std::collections::BTreeSet<_> = captures.iter().map(|c| &c.directory).collect();
        assert_eq!(names.len(), 8);
        for mut capture in captures {
            assert!(capture.write(&frame(), 1).unwrap());
            assert_eq!(
                std::fs::read(capture.directory.join("color-0001.rgba")).unwrap(),
                frame().rgba
            );
            assert!(capture.directory.join("color-0001.json").is_file());
        }
        std::fs::remove_dir_all(directory).unwrap();
    }
    #[test]
    fn capture_exact_bytes_metadata_and_limit() {
        let mut capture = Capture::create(&std::env::temp_dir(), 1).unwrap();
        let directory = capture.directory.clone();
        assert!(capture.write(&frame(), 42).unwrap());
        assert_eq!(
            std::fs::read(directory.join("color-0001.rgba")).unwrap(),
            frame().rgba
        );
        let metadata = std::fs::read_to_string(directory.join("color-0001.json")).unwrap();
        assert!(metadata.contains("\"kind\":\"published-color-target\""));
        assert!(metadata.contains("\"draw_commands\":42"));
        assert!(metadata.contains("\"rgba_bytes\":8"));
        assert!(!capture.write(&frame(), 43).unwrap());
        assert_eq!(std::fs::read_dir(&directory).unwrap().count(), 2);
        std::fs::remove_dir_all(directory).unwrap();
    }
    #[test]
    fn capture_rejects_malformed_frames_before_writing() {
        let mut capture = Capture::create(&std::env::temp_dir(), 2).unwrap();
        let directory = capture.directory.clone();
        let mut image = frame();
        image.rgba.pop();
        assert!(capture.write(&image, 0).is_err());
        image.width = 0;
        assert!(capture.write(&image, 0).is_err());
        image.width = u32::MAX;
        image.height = u32::MAX;
        assert!(capture.write(&image, 0).is_err());
        assert_eq!(std::fs::read_dir(&directory).unwrap().count(), 0);
        assert_eq!(capture.remaining, 2);
        std::fs::remove_dir_all(directory).unwrap();
    }
    #[test]
    fn capture_setup_and_failed_write_do_not_commit_metadata() {
        assert!(Capture::create(&std::env::temp_dir(), 0).is_err());
        assert!(Capture::create(&std::env::temp_dir(), 65).is_err());
        let mut capture = Capture::create(&std::env::temp_dir(), 1).unwrap();
        let directory = capture.directory.clone();
        assert!(Capture::create(&directory.join("missing").join("file"), 0).is_err());
        std::fs::remove_dir(&directory).unwrap();
        assert!(capture.write(&frame(), 0).is_err());
        assert_eq!(capture.remaining, 1);
        assert_eq!(capture.index, 0);
        assert!(!directory.exists());
    }
}
