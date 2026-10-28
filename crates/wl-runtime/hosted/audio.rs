//! Bounded headless audren initialization and stopped-voice update protocol.
//! Wire layouts: libnx feebd026, services/audren.c and services/audren.h.
//! No active voice decoding, DSP effects or audible output is implemented.
use wl_guest_runtime::audio::{AudioHost, Factory, Reply as Done};
use wl_guest_runtime::{Memory, RuntimeResult};
use wl_guest_runtime::{
    horizon::{self, Kernel},
    ipc::Request,
    services::HostObject,
    threads,
};

pub const WORK_BUFFER_BYTES: u64 = 0x10000;
pub const MAX_UPDATE_BYTES: usize = 16 * 1024 * 1024;

fn word(data: &[u8], at: usize) -> u32 {
    u32::from_le_bytes(data[at..at + 4].try_into().unwrap())
}
fn wide(data: &[u8], at: usize) -> u64 {
    u64::from_le_bytes(data[at..at + 8].try_into().unwrap())
}
fn unsupported(what: &str) -> String {
    format!("unsupported Horizon audio: {what}")
}

#[derive(Clone, Debug)]
pub struct Parameters {
    pub sample_rate: u32,
    pub sample_count: u32,
    pub mix_buffers: u32,
    pub submixes: u32,
    pub voices: u32,
    pub sinks: u32,
    pub effects: u32,
    pub splitters: u32,
    pub splitter_destinations: u32,
    pub revision: u32,
}
impl Parameters {
    pub fn parse(data: &[u8]) -> RuntimeResult<Self> {
        if data.len() < 0x34 {
            return Err(unsupported("truncated renderer parameters"));
        }
        let p = Self {
            sample_rate: word(data, 0),
            sample_count: word(data, 4),
            mix_buffers: word(data, 8),
            submixes: word(data, 12),
            voices: word(data, 16),
            sinks: word(data, 20),
            effects: word(data, 24),
            splitters: word(data, 36),
            splitter_destinations: word(data, 40),
            revision: word(data, 48),
        };
        if !matches!((p.sample_rate, p.sample_count), (48000, 240) | (32000, 160))
            || !(1..=512).contains(&p.mix_buffers)
            || p.submixes > 255
            || p.voices > 1024
            || !(1..=256).contains(&p.sinks)
            || p.effects > 4096
            || p.revision & 0xffffff != 0x564552
            || !(b'1'..=b'0' + 15).contains(&((p.revision >> 24) as u8))
        {
            return Err(format!(
                "{}: {p:?}",
                unsupported("renderer geometry, counts or revision")
            ));
        }
        if data[32] > 1 || data[33..36] != [0; 3] || word(data, 44) != 0 {
            return Err(unsupported(
                "renderer execution/device flags or external context",
            ));
        }
        if p.splitters > 1024
            || p.splitter_destinations > 8192
            || ((p.splitters != 0 || p.splitter_destinations != 0)
                && (!(5..=11).contains(&p.revision_number())
                    || p.splitters == 0
                    || p.splitter_destinations == 0))
        {
            return Err(format!(
                "{}: {p:?}",
                unsupported("splitter configuration or implementation limits")
            ));
        }
        Ok(p)
    }
    pub fn revision_number(&self) -> u32 {
        (self.revision >> 24) - u32::from(b'0')
    }
    /// REV5–REV11 routing metadata is observable only through silent updates:
    /// active voices and DSP remain rejected before a result is returned.
    fn validate_splitters(&self, section: &[u8]) -> RuntimeResult<()> {
        if self.splitters == 0 {
            return if section.is_empty() {
                Ok(())
            } else {
                Err(unsupported("unexpected splitter section"))
            };
        }
        if section.len() < 32 || section.len() % 16 != 0 || word(section, 0) != 0x48444e53 {
            return Err(unsupported("splitter SNDH header or alignment"));
        }
        let info_count = word(section, 4);
        let destination_count = word(section, 8);
        if info_count > self.splitters || destination_count > self.splitter_destinations {
            return Err(unsupported("splitter update counts"));
        }
        let mut at = 32usize;
        let mut seen = vec![false; self.splitter_destinations as usize];
        for _ in 0..info_count {
            let info = take_section(section, &mut at, 16)?;
            let count = word(info, 12) as usize;
            if word(info, 0) != 0x49444e53
                || word(info, 4) >= self.splitters
                || count > self.splitter_destinations as usize
            {
                return Err(unsupported("splitter SNDI id or destination count"));
            }
            seen.fill(false);
            let ids = take_section(section, &mut at, count * 4)?;
            for value in ids.chunks_exact(4) {
                let id = word(value, 0) as usize;
                if id >= seen.len() || seen[id] {
                    return Err(unsupported("splitter destination id or cyclic list"));
                }
                seen[id] = true;
            }
            // The wire's unused twelve bytes follow the variable IDs array.
            take_section(section, &mut at, 12)?;
        }
        for _ in 0..destination_count {
            let destination = take_section(section, &mut at, 0x70)?;
            if word(destination, 0) != 0x44444e53
                || word(destination, 4) >= self.splitter_destinations
                || destination[0x6c] > 1
            {
                return Err(unsupported("splitter SNDD id or use flag"));
            }
            if destination[0x6c] != 0 {
                let mix = word(destination, 0x68);
                if mix != 0x7fffffff && mix > self.submixes {
                    return Err(unsupported("splitter destination mix"));
                }
                if destination[8..0x68]
                    .chunks_exact(4)
                    .any(|v| !f32::from_bits(word(v, 0)).is_finite())
                {
                    return Err(unsupported("nonfinite splitter volume"));
                }
            }
        }
        if at.div_ceil(16) * 16 != section.len() {
            return Err(unsupported("splitter section trailing records"));
        }
        Ok(())
    }
}

fn take_section<'a>(bytes: &'a [u8], at: &mut usize, size: usize) -> RuntimeResult<&'a [u8]> {
    let end = at
        .checked_add(size)
        .filter(|end| *end <= bytes.len())
        .ok_or_else(|| unsupported("truncated splitter record"))?;
    let result = &bytes[*at..end];
    *at = end;
    Ok(result)
}

#[derive(Clone, Debug)]
pub struct Renderer {
    pub parameters: Parameters,
    pub running: bool,
    pub event: usize,
    pub next_tick_ns: Option<u64>,
    pub elapsed_frames: u64,
    pub time_limit: u32,
}
impl Renderer {
    pub fn new(parameters: Parameters, event: usize) -> Self {
        Self {
            parameters,
            running: false,
            event,
            next_tick_ns: None,
            elapsed_frames: 0,
            time_limit: 100,
        }
    }
    pub fn start(&mut self, now: u64) -> RuntimeResult<()> {
        if !self.running {
            self.next_tick_ns = Some(
                now.checked_add(5_000_000)
                    .ok_or_else(|| unsupported("render clock overflow"))?,
            );
            self.running = true;
        }
        Ok(())
    }
    pub fn stop(&mut self) {
        self.running = false;
        self.next_tick_ns = None;
    }
    pub fn tick(&mut self, now: u64) -> bool {
        let Some(next) = self.next_tick_ns.filter(|n| *n <= now) else {
            return false;
        };
        let count = (now - next) / 5_000_000 + 1;
        self.elapsed_frames = self.elapsed_frames.saturating_add(count);
        self.next_tick_ns = count
            .checked_mul(5_000_000)
            .and_then(|n| next.checked_add(n));
        true
    }
    /// Produce a reply only for silent initialization: active playback and
    /// circular sinks fail before any guest output is changed.
    pub fn update(&self, input: &[u8], memory: &Memory) -> RuntimeResult<Vec<u8>> {
        if input.len() < 0x40 || input.len() > MAX_UPDATE_BYTES {
            return Err(unsupported("update input size"));
        }
        let h: Vec<_> = (0..16).map(|i| word(input, i * 4) as usize).collect();
        if h[0] as u32 != self.parameters.revision
            || h[11..15] != [0; 4]
            || h[15] != 0x40 + h[1..11].iter().sum::<usize>()
            || h[15] > input.len()
        {
            return Err(unsupported("update header or section sum"));
        }
        let p = &self.parameters;
        let pools = p.effects as usize + 4 * p.voices as usize;
        let stride = if p.revision_number() >= 15 {
            0x188
        } else {
            0x170
        };
        if h[1] != 0x10
            || h[2] != pools * 0x20
            || h[3] != p.voices as usize * stride
            || h[4] % 0x70 != 0
            || h[4] / 0x70 > p.voices as usize * 6
            || h[5] != p.effects as usize * 0xc0
            || h[7] != p.sinks as usize * 0x140
            || !matches!(h[8], 0 | 0x10)
            || !matches!(h[10], 0 | 0x10)
        {
            return Err(format!("{}: {h:?}", unsupported("update section geometry")));
        }
        // Physical ordering differs from the header: channel resources precede voices;
        // splitters precede mixes. Every range is validated before slicing.
        let mut at: usize = 0x40;
        let mut parts: [&[u8]; 11] = [&[]; 11];
        for i in [1, 2, 4, 3, 5, 9, 6, 7, 8, 10] {
            let end = at
                .checked_add(h[i])
                .filter(|n| *n <= h[15])
                .ok_or_else(|| unsupported("truncated update section"))?;
            parts[i] = &input[at..end];
            at = end;
        }
        if word(parts[1], 0) != p.revision || wide(parts[1], 8) != 0 {
            return Err(unsupported("behavior revision or flags"));
        }
        p.validate_splitters(parts[9])?;
        for pool in parts[2].chunks_exact(0x20) {
            let state = word(pool, 16);
            if state > 6 {
                return Err(unsupported("memory-pool state"));
            }
            if matches!(state, 4 | 5) {
                let address = wide(pool, 0);
                let length = wide(pool, 8);
                if address % 0x1000 != 0
                    || length == 0
                    || length % 0x1000 != 0
                    || length > MAX_UPDATE_BYTES as u64
                {
                    return Err(unsupported("memory-pool range"));
                }
                let end = address
                    .checked_add(length)
                    .ok_or_else(|| unsupported("memory-pool overflow"))?;
                let mut at = address;
                while at < end {
                    let index = memory
                        .region_index(at)
                        .ok_or_else(|| unsupported("unmapped memory pool"))?;
                    let region = &memory.regions[index];
                    if !region.read {
                        return Err(unsupported("unreadable memory pool"));
                    }
                    at = region.end().min(end);
                }
            }
        }
        for voice in parts[3].chunks_exact(stride) {
            if word(voice, 0) >= p.voices || voice[9] > 1 || (voice[9] != 0 && voice[10] != 1) {
                return Err(unsupported("active voice playback"));
            }
        }
        for sink in parts[7].chunks_exact(0x140) {
            if sink[0] > 1 || sink[1] > 1 {
                return Err(unsupported("circular or unknown sink"));
            }
        }
        if parts[5].chunks_exact(0xc0).any(|effect| effect[2] != 0) {
            return Err(unsupported("active DSP effect"));
        }
        let mixes = parts[6];
        if mixes.len() != (p.submixes as usize + 1) * 0x930
            && (mixes.len() < 0x20
                || word(mixes, 0) != 0xcafebabe
                || (mixes.len() - 0x20) % 0x930 != 0
                || word(mixes, 4) as usize != (mixes.len() - 0x20) / 0x930)
        {
            return Err(unsupported("mix input geometry"));
        }
        let effect_stride = if p.revision_number() >= 9 { 0x90 } else { 0x10 };
        let sizes = [
            0xb0,
            pools * 0x10,
            p.voices as usize * 0x10,
            0,
            p.effects as usize * effect_stride,
            0,
            p.sinks as usize * 0x20,
            0x10,
            0,
            if p.revision_number() >= 5 { 0x10 } else { 0 },
        ];
        let total = 0x40 + sizes.iter().sum::<usize>();
        let mut output = vec![0u8; total];
        output[..4].copy_from_slice(&p.revision.to_le_bytes());
        for (i, n) in sizes.iter().enumerate() {
            output[4 + i * 4..8 + i * 4].copy_from_slice(&(*n as u32).to_le_bytes());
        }
        output[60..64].copy_from_slice(&(total as u32).to_le_bytes());
        let mut at = 0x40;
        for pool in parts[2].chunks_exact(0x20) {
            let state = match word(pool, 16) {
                4 => 5,
                2 => 3,
                v => v,
            };
            output[at..at + 4].copy_from_slice(&state.to_le_bytes());
            at += 0x10;
        }
        at += sizes[2];
        for effect in parts[5].chunks_exact(0xc0) {
            output[at] = if effect[2] != 0 { 3 } else { 4 };
            at += effect_stride;
        }
        at += sizes[6] + 0x10 + 0xb0;
        if sizes[9] != 0 {
            output[at..at + 8].copy_from_slice(&self.elapsed_frames.to_le_bytes());
        }
        Ok(output)
    }
}

#[derive(Clone, Debug)]
pub struct State {
    pub renderers: std::collections::BTreeMap<usize, Renderer>,
    next_id: usize,
    pub device_event: Option<usize>,
    pub device_volume: f32,
    pub expected_main_volume: f32,
    pub expected_library_volume: f32,
}
impl Default for State {
    fn default() -> Self {
        Self {
            renderers: std::collections::BTreeMap::new(),
            next_id: 0,
            device_event: None,
            device_volume: 1.0,
            expected_main_volume: 1.0,
            expected_library_volume: 1.0,
        }
    }
}

#[derive(Debug)]
struct Backend {
    state: State,
    next_thread_trace: Option<std::time::Instant>,
    opus: crate::host_opus::Service,
    fs: crate::host_fs::Service,
}
impl Default for Backend {
    fn default() -> Self {
        Self {
            state: State::default(),
            opus: crate::host_opus::Service::default(),
            fs: if std::env::var("WL_VOLATILE_SAVES").as_deref() == Ok("1") {
                crate::host_fs::Service::with_volatile_saves()
            } else {
                crate::host_fs::Service::default()
            },
            next_thread_trace: (std::env::var_os("WL_THREAD_TRACE").as_deref()
                == Some(std::ffi::OsStr::new("1")))
            .then(std::time::Instant::now),
        }
    }
}
impl Backend {
    /// Observe saved scheduler state at the callback boundary; never modify it.
    fn trace_threads(&mut self, k: &Kernel) {
        let Some(deadline) = self.next_thread_trace else {
            return;
        };
        let now = std::time::Instant::now();
        if now < deadline {
            return;
        }
        self.next_thread_trace = now.checked_add(std::time::Duration::from_secs(5));
        eprintln!(
            "[thread-state] steps={} virtual_ns={} current={} threads={}",
            k.total_steps,
            k.time_ns,
            k.current,
            k.threads.len()
        );
        for thread in &k.threads {
            let pc = thread.context.as_ref().map(|c| c.pc);
            let steps = thread.context.as_ref().map(|c| c.steps);
            eprintln!(
                "[thread-state] id={} priority={} state={:?} pc={pc:?} steps={steps:?} svc={:?} wait={}",
                thread.id,
                thread.priority,
                thread.state,
                thread.pending_svc,
                thread
                    .wait
                    .as_ref()
                    .map(ToString::to_string)
                    .unwrap_or_default()
            );
        }
    }
}
#[derive(Default)]
struct Shape {
    pid: bool,
    statics: usize,
    sends: usize,
    copies: usize,
    in_data: usize,
}
impl AudioHost for Backend {
    fn request(
        &mut self,
        memory: &mut Memory,
        k: &mut Kernel,
        target: HostObject,
        command: u32,
        req: &Request,
        args: &[u8],
    ) -> RuntimeResult<Done> {
        if let HostObject::AudioRenderer { index } = target
            && crate::host_fs::owns(index)
        {
            return self.fs.request(memory, k, index, command, req, args);
        }
        if let HostObject::AudioRenderer { index } = target
            && index >= crate::host_opus::MANAGER
        {
            return self.opus.request(memory, k, index, command, req, args);
        }
        let unsupported = |what: &str| -> RuntimeResult<Done> {
            Err(format!(
                "unsupported Horizon audio: {what} (command {command} of {target:?})"
            ))
        };
        let check = |shape: Shape| {
            if req.pid.is_some() == shape.pid
                && req.statics.len() == shape.statics
                && req.sends.len() == shape.sends
                && req.recvs.is_empty()
                && req.exchanges == 0
                && req.copy_handles as usize == shape.copies
                && req.move_handles == 0
                && args.len() >= shape.in_data
            {
                Ok(())
            } else {
                Err(format!(
                    "unsupported Horizon audio request layout for command {command}"
                ))
            }
        };
        let no_arguments = Shape::default();
        let reply = match (target, command) {
            // Applet audio balance is process-owned control state, distinct
            // from an output device's gain and from renderer voice volumes.
            (HostObject::AudioRenderer { index }, 0..=2)
                if index == crate::host_fs::APPLET_AUDIO_CONTROLLER =>
            {
                check(Shape {
                    in_data: if command == 0 { 8 } else { 0 },
                    ..no_arguments
                })?;
                if command == 0 {
                    let main = f32::from_bits(word(args, 0));
                    let library = f32::from_bits(word(args, 4));
                    if !main.is_finite()
                        || !library.is_finite()
                        || !(0.0..=1.0).contains(&main)
                        || !(0.0..=1.0).contains(&library)
                    {
                        return unsupported("applet expected volume outside finite 0..1 subset");
                    }
                    self.state.expected_main_volume = main;
                    self.state.expected_library_volume = library;
                    Done::ok()
                } else {
                    let volume = if command == 1 {
                        self.state.expected_main_volume
                    } else {
                        self.state.expected_library_volume
                    };
                    Done::data(&volume.to_le_bytes())
                }
            }
            (HostObject::AudioRendererManager, 1) => {
                check(Shape {
                    in_data: 0x34,
                    ..no_arguments
                })?;
                let parameters = Parameters::parse(args)?;
                if self.next_thread_trace.is_some() {
                    eprintln!("[audio] work-buffer parameters: {parameters:?}");
                }
                Done::data(&WORK_BUFFER_BYTES.to_le_bytes())
            }
            (HostObject::AudioRendererManager, 0) => {
                check(Shape {
                    in_data: 0x48,
                    copies: 2,
                    pid: true,
                    ..no_arguments
                })?;
                let parameters = Parameters::parse(args)?;
                let Some(horizon::Object::TransferMemory(t)) = k.handles.get(req.copies[0]) else {
                    return unsupported("audio renderer requires transfer memory");
                };
                let size = u64::from_le_bytes(args[56..64].try_into().unwrap());
                if req.copies[1] != horizon::CURRENT_PROCESS
                    || size < WORK_BUFFER_BYTES
                    || size > k.transfer_memories[t].size
                    || self.state.renderers.len() >= 4
                    || self.state.next_id >= 4096
                {
                    return unsupported("audio renderer work memory, owner or capacity");
                }
                let event = k.events.len();
                k.events.push(threads::Event::default());
                let index = self.state.next_id;
                self.state.next_id += 1;
                self.state
                    .renderers
                    .insert(index, Renderer::new(parameters, event));
                Done::object(HostObject::AudioRenderer { index })
            }
            (HostObject::AudioRendererManager, 2 | 4) => {
                check(Shape {
                    in_data: 8,
                    ..no_arguments
                })?;
                Done::object(HostObject::AudioDevice)
            }
            (HostObject::AudioRenderer { index }, 0..=3) => {
                check(no_arguments)?;
                let r = self
                    .state
                    .renderers
                    .get(&index)
                    .ok_or("invalid audio renderer identity")?;
                let value = match command {
                    0 => r.parameters.sample_rate,
                    1 => r.parameters.sample_count,
                    2 => r.parameters.mix_buffers,
                    _ => u32::from(!r.running),
                };
                Done::data(&value.to_le_bytes())
            }
            (HostObject::AudioRenderer { index }, 5 | 6) => {
                check(no_arguments)?;
                let r = self
                    .state
                    .renderers
                    .get_mut(&index)
                    .ok_or("invalid audio renderer identity")?;
                if command == 5 {
                    r.start(k.time_ns)?;
                } else {
                    r.stop();
                }
                Done::ok()
            }
            (HostObject::AudioRenderer { index }, 7) => {
                check(no_arguments)?;
                let event = self
                    .state
                    .renderers
                    .get(&index)
                    .ok_or("invalid audio renderer identity")?
                    .event;
                copy_event(k, event)
            }
            (HostObject::AudioRenderer { index }, 8 | 9) => {
                check(Shape {
                    in_data: if command == 8 { 4 } else { 0 },
                    ..no_arguments
                })?;
                let r = self
                    .state
                    .renderers
                    .get_mut(&index)
                    .ok_or("invalid audio renderer identity")?;
                if command == 8 {
                    let limit = word(args, 0);
                    if !(1..=100).contains(&limit) {
                        return unsupported("audio rendering time limit");
                    }
                    r.time_limit = limit;
                    Done::ok()
                } else {
                    Done::data(&r.time_limit.to_le_bytes())
                }
            }
            (HostObject::AudioRenderer { index }, 4 | 10) => {
                if req.pid.is_some()
                    || req.copy_handles != 0
                    || req.move_handles != 0
                    || req.exchanges != 0
                    || req.sends.len() > 1
                    || req.statics.len() > 1
                {
                    return unsupported("audio update request layout");
                }
                let input = audio_input(req)?;
                if input.1 > MAX_UPDATE_BYTES as u64 {
                    return unsupported("audio update input budget");
                }
                let mut bytes = vec![0; input.1 as usize];
                memory.read_into(input.0, &mut bytes)?;
                let output = audio_output(req, 0)?.ok_or("audio update has no output buffer")?;
                let performance = audio_output(req, 1)?;
                let r = self
                    .state
                    .renderers
                    .get(&index)
                    .ok_or("invalid audio renderer identity")?;
                let bytes = r.update(&bytes, memory)?;
                if bytes.len() as u64 > output.1 || output.1 > MAX_UPDATE_BYTES as u64 {
                    return unsupported("audio update output capacity");
                }
                memory.validate_write(output.0, output.1 as usize)?;
                if let Some((address, size)) = performance {
                    if size > MAX_UPDATE_BYTES as u64 {
                        return unsupported("audio performance output budget");
                    }
                    memory.validate_write(address, size as usize)?;
                    let end = output
                        .0
                        .checked_add(output.1)
                        .ok_or("audio output overflow")?;
                    let perf_end = address
                        .checked_add(size)
                        .ok_or("audio performance range overflow")?;
                    if output.0 < perf_end && address < end {
                        return unsupported("overlapping audio outputs");
                    }
                }
                let mut full = bytes;
                full.resize(output.1 as usize, 0);
                memory.write_bytes(output.0, &full)?;
                if let Some((address, size)) = performance {
                    memory.write_bytes(address, &vec![0; size as usize])?;
                }
                Done::ok()
            }
            (HostObject::AudioDevice, 0 | 3 | 6 | 10 | 13 | 14) => {
                let output =
                    audio_output(req, 0)?.ok_or("audio device name has no output buffer")?;
                if output.1 < 0x100 || output.1 > 0x10000 {
                    return unsupported("audio device name buffer size");
                }
                let mut bytes = [0u8; 0x100];
                bytes[..13].copy_from_slice(b"AudioTvOutput");
                memory.write_bytes(output.0, &bytes)?;
                if matches!(command, 0 | 6 | 14) {
                    Done::data(&1u32.to_le_bytes())
                } else {
                    Done::ok()
                }
            }
            (HostObject::AudioDevice, 1 | 7) => {
                if args.len() < 4 {
                    return unsupported("audio device volume input");
                }
                let volume = f32::from_bits(word(args, 0));
                if !volume.is_finite() {
                    return unsupported(&format!(
                        "nonfinite audio device volume bits {:#010x}",
                        volume.to_bits()
                    ));
                }
                let known = check_device_name(req, memory)?;
                if std::env::var_os("WL_AUDIO_TRACE").is_some() {
                    eprintln!(
                        "[audio-device] known={known} volume={volume} bits={:#010x}",
                        volume.to_bits()
                    );
                }
                if known {
                    self.state.device_volume = volume;
                }
                Done::ok()
            }
            (HostObject::AudioDevice, 2 | 8) => {
                let volume = if check_device_name(req, memory)? {
                    self.state.device_volume
                } else {
                    0.0
                };
                Done::data(&volume.to_le_bytes())
            }
            (HostObject::AudioDevice, 5) => {
                check(no_arguments)?;
                Done::data(&2u32.to_le_bytes())
            }
            (HostObject::AudioDevice, 4 | 11 | 12) => {
                check(no_arguments)?;
                let e = match self.state.device_event {
                    Some(e) => e,
                    None => {
                        let e = k.events.len();
                        k.events.push(threads::Event::default());
                        self.state.device_event = Some(e);
                        e
                    }
                };
                copy_event(k, e)
            }
            _ => return unsupported("unknown command"),
        };
        Ok(reply)
    }
    fn poll(&mut self, _memory: &mut Memory, k: &mut Kernel) -> RuntimeResult<()> {
        register_codec_port(k);
        self.trace_threads(k);
        for renderer in self.state.renderers.values_mut() {
            if renderer.tick(k.time_ns) {
                threads::signal_event(k, renderer.event);
            }
        }
        Ok(())
    }
    fn close(&mut self, _kernel: &mut Kernel, object: HostObject) {
        if let HostObject::AudioRenderer { index } = object {
            if crate::host_fs::owns(index) {
                self.fs.close(index);
                return;
            }
            if index >= crate::host_opus::MANAGER {
                self.opus.close(index);
                return;
            }
            self.state.renderers.remove(&index);
        }
    }
    fn next_deadline_ns(&self) -> Option<u64> {
        self.state
            .renderers
            .values()
            .filter_map(|r| r.next_tick_ns)
            .min()
    }
}

const CODEC_SERVICE: [u8; 8] = *b"hwopus\0\0";
/// Trusted native provider registration. Guest service-access checks still run
/// in the existing SM implementation. No guest server handle is fabricated.
fn register_codec_port(k: &mut Kernel) {
    if k.services.iter().any(|s| s.name == CODEC_SERVICE) {
        return;
    }
    let port = k.ports.len();
    k.ports.push(wl_guest_runtime::ipc::Port {
        name: CODEC_SERVICE,
        max_sessions: 64,
        sessions: 0,
        is_light: false,
        server_closed: false,
        references: 1,
    });
    k.services.push(wl_guest_runtime::ipc::Service {
        name: CODEC_SERVICE,
        owner: 0,
        port,
    });
}

/// Bind only the native-owned codec port to the existing hosted audio-server
/// transport while one SVC executes. The opaque audio object index identifies
/// its interface; no original request bytes, CPU instructions or results change.
pub fn service_hook(
    cpu: &mut wl_guest_runtime::Cpu,
    process: &mut horizon::Process,
    id: u16,
) -> RuntimeResult<Option<wl_guest_runtime::Stop>> {
    use wl_guest_runtime::{horizon::Object, ipc::Endpoint};
    let fs_binding = crate::host_fs::bind(cpu, process, id);
    let binding = if let Some(k) = process.kernel.as_mut() {
        register_codec_port(k);
        if id == 0x21 {
            if let Some(Object::Session(s)) = k.handles.get(cpu.x[0] as u32) {
                if let Some(session) = k.sessions.get(s)
                    && let Endpoint::Port(port) = session.endpoint
                    && k.services.iter().any(|service| {
                        service.port == port && service.name == CODEC_SERVICE && service.owner == 0
                    })
                {
                    let original = session.endpoint;
                    k.sessions[s].endpoint = Endpoint::Host(HostObject::AudioRenderer {
                        index: crate::host_opus::MANAGER,
                    });
                    Some((s, original))
                } else {
                    None
                }
            } else {
                None
            }
        } else {
            None
        }
    } else {
        None
    };
    let result = wl_guest_runtime::service(cpu, process, id);
    crate::host_fs::restore(process, fs_binding);
    if let Some((session, original)) = binding
        && let Some(k) = process.kernel.as_mut()
        && let Some(s) = k.sessions.get_mut(session)
    {
        s.endpoint = original;
    }
    result
}
fn copy_event(k: &mut Kernel, event: usize) -> Done {
    match threads::readable_event_handle(k, event) {
        Ok(handle) => Done {
            copies: vec![handle],
            ..Done::ok()
        },
        Err(code) => Done::failure(code),
    }
}
fn audio_input(req: &Request) -> RuntimeResult<(u64, u64)> {
    if req.sends.len() > 1 || req.statics.len() > 1 {
        return Err(unsupported("audio input descriptor count"));
    }
    let a = req.sends.first().copied().filter(|b| b.1 != 0);
    let x = req.statics.first().copied().filter(|b| b.1 != 0);
    if a.is_some() && x.is_some() {
        return Err(unsupported("two active audio input descriptors"));
    }
    a.or(x)
        .ok_or_else(|| unsupported("missing audio input descriptor"))
}
fn check_device_name(req: &Request, memory: &Memory) -> RuntimeResult<bool> {
    if req.pid.is_some()
        || req.copy_handles != 0
        || req.move_handles != 0
        || req.exchanges != 0
        || !req.recvs.is_empty()
    {
        return Err(unsupported("audio device name request layout"));
    }
    let (address, size) = audio_input(req)?;
    if size != 0x100 {
        return Err(unsupported("audio device name size"));
    }
    let mut name = [0; 0x100];
    memory.read_into(address, &mut name)?;
    let end = name
        .iter()
        .position(|b| *b == 0)
        .ok_or_else(|| unsupported("unterminated audio device name"))?;
    Ok(&name[..end] == b"AudioTvOutput")
}
fn audio_output(req: &Request, slot: usize) -> Result<Option<(u64, u64)>, String> {
    let c_count = match req.recv_static_mode {
        0 | 1 => 0,
        2 => 1,
        n => (n - 2) as usize,
    };
    let b_count = req
        .recvs
        .len()
        .checked_sub(c_count)
        .ok_or("truncated audio C descriptors")?;
    if b_count > 2 || c_count > 2 {
        return Err("unsupported audio output descriptor count".into());
    }
    let b = (slot < b_count)
        .then(|| req.recvs[slot])
        .filter(|b| b.1 != 0);
    let c = (slot < c_count)
        .then(|| req.recvs[b_count + slot])
        .filter(|b| b.1 != 0);
    if b.is_some() && c.is_some() {
        return Err("audio output has two active descriptors for one slot".into());
    }
    Ok(b.or(c))
}

static FACTORY: Factory = Factory {
    create: || Box::new(Backend::default()),
};
pub fn install() -> RuntimeResult<()> {
    wl_guest_runtime::audio::install(&FACTORY)
}
