//! Bounded headless audren initialization and stopped/paused-voice updates.
//! Wire layouts: libnx feebd026, services/audren.c and services/audren.h.
//! Bounded auxiliary/reverb/limiter processing; no queued voice decoding or audible output.
use crate::host_audio_dsp::Auxiliary;
use wl_guest_runtime::audio::{AudioHost, Factory, Reply as Done};
use wl_guest_runtime::gpu_memory::GpuPin;
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

#[derive(Debug)]
pub struct Renderer {
    pub parameters: Parameters,
    pub running: bool,
    pub event: usize,
    pub next_tick_ns: Option<u64>,
    pub elapsed_frames: u64,
    pub time_limit: u32,
    auxiliary: Option<AuxiliaryState>,
    graph: Option<EffectGraph>,
}
#[derive(Debug)]
struct AuxiliaryState {
    exchange: Auxiliary,
    channels: usize,
    /// Server usage state: New=1, Enabled=2, Disabled=3 (not wire EffectState).
    usage: u8,
    pools: Vec<(u64, u64)>,
    pins: Vec<GpuPin>,
}
struct PreparedUpdate {
    bytes: Vec<u8>,
    auxiliary: Option<AuxiliaryState>,
    graph: Option<EffectGraph>,
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
            auxiliary: None,
            graph: None,
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
    pub fn advance(&mut self, memory: &mut Memory, now: u64) -> RuntimeResult<bool> {
        if let Some(next) = self.next_tick_ns.filter(|n| *n <= now) {
            if let Some(graph) = &mut self.graph {
                let frames = (now - next) / 5_000_000 + 1;
                if frames > 1024 {
                    return Err(unsupported("effect graph catch-up budget"));
                }
                let samples = self.parameters.sample_count as usize;
                graph.preflight(memory, samples)?;
                for _ in 0..frames {
                    graph.frame(memory, samples)?;
                    self.elapsed_frames = self.elapsed_frames.saturating_add(1);
                    self.next_tick_ns = self.next_tick_ns.and_then(|n| n.checked_add(5_000_000));
                }
                return Ok(true);
            }
            if let Some(auxiliary) = &mut self.auxiliary {
                let frames = (now - next) / 5_000_000 + 1;
                if frames > 1024 {
                    return Err(unsupported("auxiliary catch-up budget"));
                }
                let samples = self.parameters.sample_count as usize;
                auxiliary
                    .exchange
                    .preflight(memory, auxiliary.channels, samples)?;
                let input = vec![0.0; samples];
                for _ in 0..frames {
                    for channel in 0..auxiliary.channels {
                        let update = if channel + 1 == auxiliary.channels {
                            (samples * auxiliary.channels) as u32
                        } else {
                            0
                        };
                        // This subset has one final mix, no sample-producing
                        // voices, one auxiliary and a headless device sink.
                        // Each channel is independent; device output is discarded.
                        auxiliary.exchange.exchange_channel(
                            memory,
                            &input,
                            (samples * channel) as u32,
                            update,
                        )?;
                    }
                }
                auxiliary.usage = if auxiliary.exchange.enabled { 2 } else { 3 };
            }
        }
        Ok(self.tick(now))
    }
    /// Validate and preview a reply without installing DSP configuration. The
    /// service adapter commits configuration after checked output publication.
    pub fn update(&self, input: &[u8], memory: &Memory) -> RuntimeResult<Vec<u8>> {
        self.prepare_update(input, memory).map(|p| p.bytes)
    }
    fn prepare_update(&self, input: &[u8], memory: &Memory) -> RuntimeResult<PreparedUpdate> {
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
                // libnx feebd026 audio/mempool.c: these are guest backing
                // ranges, not bytes embedded in the IPC update. Validate mapped
                // regions without copying the pool or applying the IPC size cap.
                if address == 0 || address % 0x1000 != 0 || length == 0 || length % 0x1000 != 0 {
                    return Err(format!(
                        "{}: address={address:#x} length={length:#x} state={state}",
                        unsupported("memory-pool range")
                    ));
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
        let wave_base = if p.revision_number() >= 15 {
            0x78
        } else {
            0x60
        };
        for (slot, voice) in parts[3].chunks_exact(stride).enumerate() {
            // Ryujinx Audio 4930dced VoiceState.ShouldSkip: zero queued
            // wave buffers produce no sample-processing commands. Require the
            // pristine already-sent descriptors; actual queued audio still fails.
            let empty_started = voice[10] == 0
                && word(voice, wave_base - 0x24) == 0
                && (0..4).all(|i| {
                    let at = wave_base + i * 0x38;
                    wide(voice, at) == 0
                        && wide(voice, at + 8) == 0
                        && word(voice, at + 16) == 0
                        && word(voice, at + 20) == 0
                        && voice[at + 24] == 0
                        && voice[at + 25] == 0
                        && voice[at + 26] == 1
                        && wide(voice, at + 32) == 0
                        && wide(voice, at + 40) == 0
                });
            // libnx feebd026: Started=0, Stopped=1, Paused=2. Paused
            // voices cannot advance DSP samples, like this renderer's stopped
            // subset. Actual playback remains unsupported, with useful metadata.
            let queued_candidate = p.revision_number() == 11
                && p.submixes > 0
                && voice[9] == 1
                && voice[10] == 0
                && word(voice, wave_base - 0x24) > 0;
            if word(voice, 0) >= p.voices
                || voice[9] > 1
                || (voice[9] != 0
                    && !matches!(voice[10], 1 | 2)
                    && !empty_started
                    && !queued_candidate)
            {
                let buffers: Vec<_> = (0..4)
                    .map(|i| {
                        let at = wave_base + i * 0x38;
                        (
                            wide(voice, at),
                            wide(voice, at + 8),
                            word(voice, at + 16),
                            word(voice, at + 20),
                            voice[at + 24],
                            voice[at + 25],
                            voice[at + 26],
                        )
                    })
                    .collect();
                return Err(format!(
                    "{}: slot={slot} id={} used={} state={} format={} rate={} channels={} wave_buffers={} active_effects={} sink_types={:?} buffers={buffers:?}",
                    unsupported("active voice playback"),
                    word(voice, 0),
                    voice[9],
                    voice[10],
                    voice[11],
                    word(voice, 12),
                    word(voice, 24),
                    word(voice, wave_base - 0x24),
                    parts[5].chunks_exact(0xc0).filter(|e| e[2] != 0).count(),
                    parts[7]
                        .chunks_exact(0x140)
                        .map(|s| s[0])
                        .collect::<Vec<_>>()
                ));
            }
        }
        for sink in parts[7].chunks_exact(0x140) {
            if sink[0] > 1 || sink[1] > 1 {
                return Err(unsupported("circular or unknown sink"));
            }
        }
        let graph = if self.graph.is_some()
            || (p.submixes != 0 && parts[5].chunks_exact(0xc0).any(|e| e[0] != 0))
        {
            Some(self.prepare_graph(&parts, memory)?)
        } else {
            None
        };
        let auxiliary = if graph.is_none()
            && parts[5].len() == 0xc0
            && parts[5][0] == 2
            && (parts[5][1] != 0 || parts[5][2] != 0 || self.auxiliary.is_some())
        {
            Some(self.prepare_auxiliary(parts[5], parts[6], parts[7], parts[2], memory)?)
        } else {
            None
        };
        if graph.is_none()
            && auxiliary.is_none()
            && parts[5].chunks_exact(0xc0).any(|effect| effect[2] != 0)
        {
            let effects: Vec<_> = parts[5]
                .chunks_exact(0xc0)
                .enumerate()
                .filter(|(_, e)| e[2] != 0)
                .take(16)
                .map(|(slot, e)| {
                    (
                        slot,
                        e[0],
                        e[1],
                        e[2],
                        word(e, 4),
                        wide(e, 8),
                        wide(e, 16),
                        word(e, 24),
                        &e[32..64],
                    )
                })
                .collect();
            return Err(format!(
                "{}: first_enabled={effects:?}",
                unsupported("active DSP effect")
            ));
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
        for voice in parts[3].chunks_exact(stride) {
            if voice[9] != 0
                && let Some(state) = graph.as_ref().and_then(|g| g.voices.get(&word(voice, 0)))
            {
                output[at..at + 8].copy_from_slice(&state.queue.played.to_le_bytes());
                output[at + 8..at + 12].copy_from_slice(&state.queue.consumed.to_le_bytes());
            }
            at += 16;
        }
        for (slot, _) in parts[5].chunks_exact(0xc0).enumerate() {
            output[at] = if let Some(effect) = graph
                .as_ref()
                .and_then(|g| g.effects.get(slot))
                .and_then(Option::as_ref)
            {
                if (self.running && effect.usage != 3) || (!self.running && effect.usage == 1) {
                    3
                } else {
                    4
                }
            } else if let Some(aux) = &auxiliary {
                if (self.running && aux.usage != 3) || (!self.running && aux.usage == 1) {
                    3
                } else {
                    4
                }
            } else {
                4
            };
            if effect_stride >= 0x90 {
                if let Some(Effect {
                    processor: EffectProcessor::Limiter(_, limiter),
                    ..
                }) = graph
                    .as_ref()
                    .and_then(|g| g.effects.get(slot))
                    .and_then(Option::as_ref)
                {
                    for (i, value) in limiter
                        .statistics
                        .peak
                        .iter()
                        .chain(&limiter.statistics.minimum_gain)
                        .enumerate()
                    {
                        output[at + 16 + i * 4..at + 20 + i * 4]
                            .copy_from_slice(&value.to_le_bytes());
                    }
                }
            }
            at += effect_stride;
        }
        at += sizes[6] + 0x10 + 0xb0;
        if sizes[9] != 0 {
            output[at..at + 8].copy_from_slice(&self.elapsed_frames.to_le_bytes());
        }
        Ok(PreparedUpdate {
            bytes: output,
            auxiliary,
            graph,
        })
    }
    fn prepare_auxiliary(
        &self,
        effect: &[u8],
        mix: &[u8],
        sinks: &[u8],
        pools: &[u8],
        memory: &Memory,
    ) -> RuntimeResult<AuxiliaryState> {
        if self.parameters.submixes != 0
            || self.parameters.splitters != 0
            || self.parameters.revision_number() < 10
            || mix.len() != 0x930
        {
            return Err(unsupported(
                "auxiliary requires REV10+ single final mix without splitters",
            ));
        }
        if effect[1] > 1
            || effect[2] > 1
            || word(effect, 4) != 0
            || word(mix, 16) != 0
            || mix[12] != 1
            || word(mix, 20) != 1
        {
            return Err(unsupported("auxiliary final-mix binding"));
        }
        let mut devices = 0;
        for sink in sinks.chunks_exact(0x140).filter(|s| s[1] != 0) {
            let count = word(sink, 0x120) as usize;
            if sink[0] != 1
                || !(1..=6).contains(&count)
                || sink[0x124..0x124 + count]
                    .iter()
                    .any(|n| u32::from(*n) >= word(mix, 8))
            {
                return Err(unsupported("auxiliary headless device-sink geometry"));
            }
            devices += 1;
        }
        if devices == 0 {
            return Err(unsupported("auxiliary requires a device sink"));
        }
        let specific = &effect[32..];
        let channels = word(specific, 48) as usize;
        if !(1..=6).contains(&channels)
            || word(specific, 52) != self.parameters.sample_rate
            || word(specific, 60) < channels as u32
            || word(specific, 104) != self.parameters.sample_count
            || word(mix, 4) != self.parameters.sample_rate
            || word(mix, 8) < channels as u32
            || word(mix, 8) > 24
            || !f32::from_bits(word(mix, 0)).is_finite()
            || specific[..channels]
                .iter()
                .copied()
                .ne((0..channels).map(|c| c as u8))
            || specific[24..24 + channels]
                .iter()
                .copied()
                .ne((0..channels).map(|c| c as u8))
        {
            return Err(unsupported(
                "auxiliary channel/rate/identity-routing subset",
            ));
        }
        let exchange = Auxiliary {
            send_info: wide(specific, 64),
            return_info: wide(specific, 80),
            capacity: word(specific, 56),
            enabled: effect[2] != 0,
        };
        if exchange.send_info.checked_add(128) != Some(wide(specific, 72))
            || exchange.return_info.checked_add(128) != Some(wide(specific, 88))
        {
            return Err(unsupported("auxiliary noncontiguous storage"));
        }
        let footprint = 128 + u64::from(exchange.capacity) * 4;
        let mut attached = Vec::new();
        for base in [exchange.send_info, exchange.return_info] {
            let end = base
                .checked_add(footprint)
                .ok_or("auxiliary backing overflow")?;
            let pool = pools.chunks_exact(32).find(|pool| {
                matches!(word(pool, 16), 4 | 5)
                    && wide(pool, 0) <= base
                    && wide(pool, 0)
                        .checked_add(wide(pool, 8))
                        .is_some_and(|e| end <= e)
            });
            let Some(pool) = pool else {
                return Err(unsupported("auxiliary storage outside attached pools"));
            };
            let range = (wide(pool, 0), wide(pool, 8));
            if !attached.contains(&range) {
                attached.push(range);
            }
        }
        exchange.preflight(memory, channels, self.parameters.sample_count as usize)?;
        let usage = if effect[1] != 0 {
            1
        } else if let Some(old) = &self.auxiliary {
            if old.channels != channels
                || old.exchange.send_info != exchange.send_info
                || old.exchange.return_info != exchange.return_info
                || old.exchange.capacity != exchange.capacity
            {
                return Err(unsupported("auxiliary rebind requires a new effect"));
            }
            old.usage
        } else {
            1
        };
        Ok(AuxiliaryState {
            exchange,
            channels,
            usage,
            pools: attached,
            pins: Vec::new(),
        })
    }
}

#[derive(Debug)]
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
    released_audio_pins: Vec<GpuPin>,
}
impl Default for Backend {
    fn default() -> Self {
        Self {
            state: State::default(),
            released_audio_pins: Vec::new(),
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
    fn release_audio_pins(&mut self, memory: &mut Memory) -> RuntimeResult<()> {
        while let Some(&token) = self.released_audio_pins.last() {
            memory.release_gpu_pin(token)?;
            self.released_audio_pins.pop();
        }
        Ok(())
    }
    fn pin_auxiliary(
        &mut self,
        memory: &mut Memory,
        index: usize,
        prepared: &mut PreparedUpdate,
    ) -> RuntimeResult<()> {
        if let Some(new) = prepared.graph.as_mut() {
            let renderer = self
                .state
                .renderers
                .get_mut(&index)
                .ok_or("invalid audio renderer identity")?;
            let old_pools = renderer
                .graph
                .as_ref()
                .map_or(&[][..], |g| g.pools.as_slice());
            if renderer
                .graph
                .as_ref()
                .is_some_and(|g| g.pins.len() != g.pools.len())
            {
                return Err("audio pool ownership invariant".into());
            }
            let mut planned = vec![None; new.pools.len()];
            // Acquire only genuinely new ranges. A failure releases this prefix
            // while every old token remains attached to the old configuration.
            for (i, &(address, size)) in new.pools.iter().enumerate() {
                if old_pools.contains(&(address, size)) {
                    continue;
                }
                match memory.pin_gpu_range(address, size) {
                    Ok(pin) => planned[i] = Some(pin),
                    Err(error) => {
                        for pin in planned.into_iter().flatten().rev() {
                            memory.release_gpu_pin(pin)?;
                        }
                        return Err(format!("audio graph lifetime registration: {error}"));
                    }
                }
            }
            if let Some(old) = renderer.graph.as_mut() {
                let mut remaining_pools = Vec::new();
                let mut remaining_pins = Vec::new();
                for (pool, pin) in old.pools.drain(..).zip(old.pins.drain(..)) {
                    if let Some(i) = new.pools.iter().position(|p| *p == pool) {
                        planned[i] = Some(pin);
                    } else {
                        remaining_pools.push(pool);
                        remaining_pins.push(pin);
                    }
                }
                old.pools = remaining_pools;
                old.pins = remaining_pins;
            }
            new.pins = planned
                .into_iter()
                .map(|p| p.expect("validated audio pool token"))
                .collect();
            return Ok(());
        }
        let Some(new) = prepared.auxiliary.as_mut() else {
            return Ok(());
        };
        let renderer = self
            .state
            .renderers
            .get_mut(&index)
            .ok_or("invalid audio renderer identity")?;
        if let Some(old) = renderer.auxiliary.as_mut()
            && old.pools == new.pools
        {
            new.pins = std::mem::take(&mut old.pins);
            return Ok(());
        }
        // Reuse the runtime's existing canonical lifetime guards. No new
        // memory model or escaped guest borrow is introduced. Shared existing
        // GPU registrations and unsupported storage kinds fail explicitly.
        for &(address, size) in &new.pools {
            match memory.pin_gpu_range(address, size) {
                Ok(pin) => new.pins.push(pin),
                Err(error) => {
                    for pin in new.pins.drain(..).rev() {
                        memory.release_gpu_pin(pin)?;
                    }
                    return Err(format!(
                        "unsupported audio auxiliary lifetime registration: {error}"
                    ));
                }
            }
        }
        Ok(())
    }
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
        self.release_audio_pins(memory)?;
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
                let mut prepared = r.prepare_update(&bytes, memory)?;
                let bytes = std::mem::take(&mut prepared.bytes);
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
                self.pin_auxiliary(memory, index, &mut prepared)?;
                let mut full = bytes;
                full.resize(output.1 as usize, 0);
                memory.write_bytes(output.0, &full)?;
                if let Some((address, size)) = performance {
                    memory.write_bytes(address, &vec![0; size as usize])?;
                }
                let old = std::mem::replace(
                    &mut self
                        .state
                        .renderers
                        .get_mut(&index)
                        .ok_or("invalid audio renderer identity")?
                        .auxiliary,
                    prepared.auxiliary,
                );
                if let Some(old) = old {
                    for pin in old.pins {
                        memory.release_gpu_pin(pin)?;
                    }
                }
                let renderer = self
                    .state
                    .renderers
                    .get_mut(&index)
                    .ok_or("invalid renderer")?;
                if let Some(old) = std::mem::replace(&mut renderer.graph, prepared.graph) {
                    for pin in old.pins {
                        memory.release_gpu_pin(pin)?;
                    }
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
    fn poll(&mut self, memory: &mut Memory, k: &mut Kernel) -> RuntimeResult<()> {
        self.release_audio_pins(memory)?;
        register_codec_port(k);
        self.trace_threads(k);
        for renderer in self.state.renderers.values_mut() {
            if renderer.advance(memory, k.time_ns)? {
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
            if let Some(renderer) = self.state.renderers.remove(&index) {
                if let Some(auxiliary) = renderer.auxiliary {
                    self.released_audio_pins.extend(auxiliary.pins);
                }
                if let Some(graph) = renderer.graph {
                    self.released_audio_pins.extend(graph.pins);
                }
            }
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
    if result.is_ok()
        && matches!(id, 0x16 | 0x21)
        && let Some(kernel) = process.kernel.as_mut()
    {
        // Close callbacks lack a Memory parameter in the frozen runtime ABI.
        // Drain deferred lifetime releases at this same service boundary,
        // before the next guest instruction can unmap/reuse the allocation.
        wl_guest_runtime::audio::poll(&mut process.memory, kernel)?;
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

#[derive(Clone, Debug)]
struct Mix {
    channels: usize,
    destination: Option<usize>,
    volume: f32,
    matrix: Vec<f32>,
    effect_count: usize,
}
#[derive(Clone, Debug)]
enum EffectProcessor {
    Auxiliary(Auxiliary),
    Reverb(
        crate::host_audio_dsp::reverb3d::ReverbParams,
        Box<crate::host_audio_dsp::reverb3d::Reverb3d>,
    ),
    Limiter(
        crate::host_audio_dsp::limiter::LimiterParams,
        Box<crate::host_audio_dsp::limiter::Limiter>,
    ),
}
#[derive(Clone, Debug)]
struct Effect {
    mix: usize,
    order: u32,
    input: Vec<usize>,
    output: Vec<usize>,
    enabled: bool,
    usage: u8,
    processor: EffectProcessor,
}
#[derive(Debug)]
struct EffectGraph {
    mixes: Vec<Option<Mix>>,
    effects: Vec<Option<Effect>>,
    order: Vec<usize>,
    pools: Vec<(u64, u64)>,
    pins: Vec<GpuPin>,
    sample_rate: u32,
    voices: std::collections::BTreeMap<u32, crate::host_audio_dsp::voice::MonoVoice>,
    routing: crate::host_audio_dsp::routing::Routing,
}
impl EffectGraph {
    fn preflight(&self, memory: &Memory, samples: usize) -> RuntimeResult<()> {
        for effect in self.effects.iter().flatten() {
            if let EffectProcessor::Auxiliary(exchange) = &effect.processor {
                exchange.preflight(memory, effect.input.len(), samples)?;
            }
        }
        Ok(())
    }
    fn frame(&mut self, memory: &mut Memory, samples: usize) -> RuntimeResult<()> {
        // No sample-producing voices are accepted by prepare_update yet. Mix
        // buffers start clear each frame; real auxiliary return samples and
        // persistent delay tails are processed and routed, never discarded early.
        let mut buffers: Vec<Vec<Vec<f32>>> = self
            .mixes
            .iter()
            .map(|m| vec![vec![0.; samples]; m.as_ref().map_or(0, |m| m.channels)])
            .collect();
        let mut voices = self.voices.clone();
        let mut routing = self.routing.clone();
        let mut sequence: Vec<_> = voices.values_mut().collect();
        sequence.sort_by_key(|voice| (std::cmp::Reverse(voice.priority), voice.order, voice.id));
        for voice in sequence {
            let source = voice.render(memory, samples)?;
            routing.route_mono(
                voice.splitter as usize,
                self.sample_rate,
                &source,
                &mut buffers,
            )?;
        }
        routing.finish_frame();
        let mut processors = self.effects.clone();
        let mut writes = Vec::new();
        for &mix_id in &self.order {
            let mut effects: Vec<_> = processors
                .iter_mut()
                .flatten()
                .filter(|e| e.mix == mix_id)
                .collect();
            effects.sort_by_key(|e| e.order);
            for effect in effects {
                let input: Vec<_> = effect
                    .input
                    .iter()
                    .map(|&c| buffers[mix_id][c].clone())
                    .collect();
                let output = match &mut effect.processor {
                    EffectProcessor::Auxiliary(exchange) => {
                        let count = input.len();
                        let mut output = Vec::with_capacity(count);
                        for (c, channel) in input.iter().enumerate() {
                            let (result, plan) = exchange.plan_channel(
                                memory,
                                channel,
                                (samples * c) as u32,
                                if c + 1 == count {
                                    (samples * count) as u32
                                } else {
                                    0
                                },
                            )?;
                            writes.extend(plan);
                            buffers[mix_id][effect.input[c]] = result.quantized_input;
                            output.push(result.output);
                        }
                        output
                    }
                    EffectProcessor::Reverb(_, reverb) => {
                        if effect.enabled {
                            reverb.process(&input)?
                        } else {
                            input
                        }
                    }
                    EffectProcessor::Limiter(_, limiter) => {
                        limiter.process(&input, effect.enabled)?
                    }
                };
                for (&c, data) in effect.output.iter().zip(output) {
                    buffers[mix_id][c] = data;
                }
                effect.usage = if effect.enabled { 2 } else { 3 };
            }
            let mix = self.mixes[mix_id].as_ref().unwrap();
            if let Some(dest) = mix.destination {
                let (source, destination) = if mix_id < dest {
                    let (left, right) = buffers.split_at_mut(dest);
                    (&left[mix_id], &mut right[0])
                } else {
                    let (left, right) = buffers.split_at_mut(mix_id);
                    (&right[0], &mut left[dest])
                };
                for (c, channel) in source.iter().enumerate() {
                    for (d, target) in destination.iter_mut().enumerate() {
                        let gain = mix.matrix[c * 24 + d] * mix.volume;
                        if gain != 0. {
                            for (to, from) in target.iter_mut().zip(channel) {
                                *to += from * gain;
                            }
                        }
                    }
                }
            }
        }
        if buffers.iter().flatten().flatten().any(|v| !v.is_finite()) {
            return Err(unsupported("effect graph nonfinite output"));
        }
        crate::host_audio_dsp::commit(memory, writes)?;
        self.effects = processors;
        self.voices = voices;
        self.routing = routing;
        Ok(())
    }
}
fn float(data: &[u8], at: usize) -> f32 {
    f32::from_bits(word(data, at))
}
fn short(data: &[u8], at: usize) -> usize {
    u16::from_le_bytes(data[at..at + 2].try_into().unwrap()) as usize
}
impl Renderer {
    fn prepare_graph(&self, parts: &[&[u8]; 11], memory: &Memory) -> RuntimeResult<EffectGraph> {
        let (effects, mix_bytes, sinks, pools, voice_bytes, splitter_bytes) =
            (parts[5], parts[6], parts[7], parts[2], parts[3], parts[9]);
        use crate::host_audio_dsp::{
            limiter::{Limiter, LimiterParams},
            reverb3d::{Reverb3d, ReverbParams},
        };
        if self.parameters.effects > 64 {
            return Err(unsupported("effect graph allocation budget"));
        }
        if self.parameters.revision_number() < 10 {
            return Err(unsupported("effect graph requires REV10+"));
        }
        let count = self.parameters.submixes as usize + 1;
        let mut mixes = self
            .graph
            .as_ref()
            .map_or_else(|| vec![None; count], |g| g.mixes.clone());
        let dirty_only = mix_bytes.len() >= 32 && word(mix_bytes, 0) == 0xcafebabe;
        let records = if dirty_only {
            if (mix_bytes.len() - 32) % 0x930 != 0
                || word(mix_bytes, 4) as usize != (mix_bytes.len() - 32) / 0x930
            {
                return Err(unsupported("dirty mix geometry"));
            }
            &mix_bytes[32..]
        } else {
            if mix_bytes.len() != count * 0x930 {
                return Err(unsupported("full mix geometry"));
            }
            mix_bytes
        };
        let mut seen = vec![false; count];
        for (slot, m) in records.chunks_exact(0x930).enumerate() {
            let id = word(m, 16) as usize;
            if id >= count || seen[id] || (!dirty_only && id != slot) || m[12] > 1 || m[13] > 1 {
                return Err(unsupported("mix id/flags"));
            }
            seen[id] = true;
            if m[12] == 0 {
                mixes[id] = None;
                continue;
            }
            let channels = word(m, 8) as usize;
            if !(1..=24).contains(&channels)
                || word(m, 4) != self.parameters.sample_rate
                || !float(m, 0).is_finite()
                || float(m, 0).abs() > 64.
                || word(m, 0x928) != u32::MAX
            {
                return Err(unsupported("mix geometry, gain or splitter destination"));
            }
            let destination = match word(m, 0x924) {
                0x7fffffff => None,
                d if (d as usize) < count && d as usize != id => Some(d as usize),
                _ => return Err(unsupported("mix destination")),
            };
            if id == 0 && destination.is_some() {
                return Err(unsupported("final mix destination"));
            }
            let matrix: Vec<_> = (0..576).map(|i| float(m, 36 + i * 4)).collect();
            if matrix.iter().any(|v| !v.is_finite() || v.abs() > 64.) {
                return Err(unsupported("mix matrix coefficient"));
            }
            mixes[id] = Some(Mix {
                channels,
                destination,
                volume: float(m, 0),
                matrix,
                effect_count: word(m, 20) as usize,
            });
        }
        if mixes[0].is_none()
            || mixes.iter().flatten().map(|m| m.channels).sum::<usize>()
                > self.parameters.mix_buffers as usize
        {
            return Err(unsupported("final mix or mix-buffer budget"));
        }
        for m in mixes.iter().flatten() {
            if let Some(d) = m.destination {
                if mixes[d].is_none() {
                    return Err(unsupported("inactive mix destination"));
                }
            }
        }
        let mut pending = vec![0usize; count];
        for m in mixes.iter().flatten() {
            if let Some(d) = m.destination {
                pending[d] += 1;
            }
        }
        let mut order = Vec::new();
        let mut ready: Vec<_> = (0..count)
            .filter(|&i| mixes[i].is_some() && pending[i] == 0)
            .collect();
        while let Some(i) = ready.pop() {
            order.push(i);
            if let Some(d) = mixes[i].as_ref().unwrap().destination {
                pending[d] -= 1;
                if pending[d] == 0 {
                    ready.push(d);
                }
            }
        }
        if order.len() != mixes.iter().flatten().count() {
            return Err(unsupported("cyclic mix routing"));
        }
        for sink in sinks.chunks_exact(0x140).filter(|s| s[1] != 0) {
            let channels = word(sink, 0x120) as usize;
            if sink[0] != 1
                || !(1..=6).contains(&channels)
                || sink[0x124..0x124 + channels]
                    .iter()
                    .any(|c| *c as usize >= mixes[0].as_ref().unwrap().channels)
            {
                return Err(unsupported("graph device sink geometry"));
            }
        }
        let mut result = EffectGraph {
            mixes,
            effects: Vec::new(),
            order,
            pools: Vec::new(),
            pins: Vec::new(),
            sample_rate: self.parameters.sample_rate,
            voices: std::collections::BTreeMap::new(),
            routing: if let Some(old) = &self.graph {
                old.routing.clone()
            } else {
                crate::host_audio_dsp::routing::Routing::new(
                    self.parameters.splitters as usize,
                    self.parameters.splitter_destinations as usize,
                )?
            },
        };
        if !splitter_bytes.is_empty() {
            result.routing = result.routing.update(splitter_bytes, count)?;
        }
        let mut occupied = Vec::<(u64, u64)>::new();
        let mut attach = |base: u64, size: u64| -> RuntimeResult<()> {
            let end = base
                .checked_add(size)
                .filter(|e| *e > base)
                .ok_or("effect backing overflow/empty")?;
            if occupied.iter().any(|&(a, b)| base < b && a < end) {
                return Err(unsupported("overlapping effect storage"));
            }
            occupied.push((base, end));
            let pool = pools
                .chunks_exact(32)
                .find(|p| {
                    matches!(word(p, 16), 4 | 5)
                        && wide(p, 0) <= base
                        && wide(p, 0).checked_add(wide(p, 8)).is_some_and(|e| end <= e)
                })
                .ok_or("effect storage outside attached pools")?;
            let range = (wide(pool, 0), wide(pool, 8));
            if !result.pools.contains(&range) {
                result.pools.push(range);
            }
            Ok(())
        };
        let mut state_samples = 0usize;
        for (slot, e) in effects.chunks_exact(0xc0).enumerate() {
            if e[0] == 0 {
                if e[2] != 0 {
                    return Err(unsupported("enabled invalid effect"));
                }
                result.effects.push(None);
                continue;
            }
            if ![2, 5, 7].contains(&e[0]) || e[1] > 1 || e[2] > 1 {
                return Err(unsupported("effect graph kind or flags"));
            }
            let mix = word(e, 4) as usize;
            let Some(m) = result.mixes.get(mix).and_then(Option::as_ref) else {
                return Err(unsupported("effect inactive mix"));
            };
            let s = &e[32..];
            let enabled = e[2] != 0;
            let channels = if e[0] == 2 {
                word(s, 48) as usize
            } else {
                short(s, 14)
            };
            if channels == 0 || channels > 6 {
                return Err(unsupported("effect channel count"));
            }
            let (input, output): (Vec<_>, Vec<_>) = if e[0] == 2 {
                (
                    s[..channels].iter().map(|v| *v as usize).collect(),
                    s[24..24 + channels].iter().map(|v| *v as usize).collect(),
                )
            } else {
                (
                    s[..channels].iter().map(|v| *v as usize).collect(),
                    s[6..6 + channels].iter().map(|v| *v as usize).collect(),
                )
            };
            if input.iter().chain(&output).any(|&c| c >= m.channels)
                || output
                    .iter()
                    .enumerate()
                    .any(|(i, c)| output[..i].contains(c))
                || (input != output && input.iter().any(|c| output.contains(c)))
            {
                return Err(unsupported("effect channel mapping or partial alias"));
            }
            let old = self
                .graph
                .as_ref()
                .and_then(|g| g.effects.get(slot))
                .and_then(Option::as_ref);
            let reuse = e[1] == 0;
            let usage = if reuse { old.map_or(1, |e| e.usage) } else { 1 };
            let processor = match e[0] {
                2 => {
                    if word(s, 52) != self.parameters.sample_rate
                        || word(s, 60) < channels as u32
                        || word(s, 104) != self.parameters.sample_count
                    {
                        return Err(unsupported("auxiliary graph rate/frame"));
                    }
                    let exchange = Auxiliary {
                        send_info: wide(s, 64),
                        return_info: wide(s, 80),
                        capacity: word(s, 56),
                        enabled,
                    };
                    if exchange.send_info.checked_add(128) != Some(wide(s, 72))
                        || exchange.return_info.checked_add(128) != Some(wide(s, 88))
                    {
                        return Err(unsupported("auxiliary graph storage"));
                    }
                    for base in [exchange.send_info, exchange.return_info] {
                        attach(base, 128 + u64::from(exchange.capacity) * 4)?;
                    }
                    exchange.preflight(memory, channels, self.parameters.sample_count as usize)?;
                    if reuse && let Some(old) = old {
                        match &old.processor {
                            EffectProcessor::Auxiliary(p)
                                if p.send_info == exchange.send_info
                                    && p.return_info == exchange.return_info
                                    && p.capacity == exchange.capacity
                                    && old.input.len() == channels => {}
                            _ => {
                                return Err(unsupported(
                                    "auxiliary graph rebind requires new effect",
                                ));
                            }
                        }
                    }
                    EffectProcessor::Auxiliary(exchange)
                }
                5 => {
                    if word(s, 20) != self.parameters.sample_rate
                        || short(s, 12) < channels
                        || s[72] > 2
                    {
                        return Err(unsupported("reverb graph rate/state"));
                    }
                    let p = ReverbParams {
                        channels,
                        rate: word(s, 20),
                        room_hf: float(s, 24),
                        hf_reference: float(s, 28),
                        decay: float(s, 32),
                        hf_decay: float(s, 36),
                        room_gain: float(s, 40),
                        reflections_gain: float(s, 44),
                        reverb_gain: float(s, 48),
                        diffusion: float(s, 52),
                        reflection_delay: float(s, 56),
                        reverb_delay: float(s, 60),
                        density: float(s, 64),
                        dry_gain: float(s, 68),
                    };
                    p.validate()?;
                    state_samples += p.rate as usize * 2; // conservative bound on all delay lines
                    if state_samples > 8 * 1024 * 1024 {
                        return Err(unsupported("effect delay allocation budget"));
                    }
                    attach(wide(e, 8), wide(e, 16))?;
                    memory.validate_write(
                        wide(e, 8),
                        usize::try_from(wide(e, 16)).map_err(|_| "reverb work size")?,
                    )?;
                    let state = if reuse && s[72] != 0 {
                        if let Some(Effect {
                            processor: EffectProcessor::Reverb(old_p, old_state),
                            ..
                        }) = old
                        {
                            let mut state = (**old_state).clone();
                            if *old_p != p || s[72] == 1 {
                                state.set_params(p)?;
                            }
                            state
                        } else {
                            Reverb3d::new(p)?
                        }
                    } else {
                        Reverb3d::new(p)?
                    };
                    EffectProcessor::Reverb(p, Box::new(state))
                }
                7 => {
                    if word(s, 16) != self.parameters.sample_rate
                        || short(s, 12) < channels
                        || s[64] > 2
                        || s[65] > 1
                        || s[66] > 1
                    {
                        return Err(unsupported("limiter graph rate/state"));
                    }
                    let p = LimiterParams {
                        channels,
                        delay_min: word(s, 56) as usize,
                        delay_max: word(s, 60) as usize,
                        attack: float(s, 36),
                        release: float(s, 40),
                        threshold: float(s, 44),
                        input_gain: float(s, 48),
                        output_gain: float(s, 52),
                        statistics_enabled: s[65] != 0,
                    };
                    p.validate()?;
                    state_samples += p.channels * p.delay_max;
                    if state_samples > 8 * 1024 * 1024 {
                        return Err(unsupported("effect delay allocation budget"));
                    }
                    attach(wide(e, 8), wide(e, 16))?;
                    memory.validate_write(
                        wide(e, 8),
                        usize::try_from(wide(e, 16)).map_err(|_| "limiter work size")?,
                    )?;
                    let mut state = if reuse && s[64] != 0 {
                        if let Some(Effect {
                            processor: EffectProcessor::Limiter(old_p, old_state),
                            ..
                        }) = old
                        {
                            if old_p.channels != p.channels || old_p.delay_max != p.delay_max {
                                return Err(unsupported("limiter resize requires new effect"));
                            }
                            (**old_state).clone()
                        } else {
                            Limiter::new(p)?
                        }
                    } else {
                        Limiter::new(p)?
                    };
                    state.set_params(p, s[66] != 0)?;
                    EffectProcessor::Limiter(p, Box::new(state))
                }
                _ => unreachable!(),
            };
            result.effects.push(Some(Effect {
                mix,
                order: word(e, 24),
                input,
                output,
                enabled,
                usage,
                processor,
            }));
        }
        let mut voice_ids = std::collections::BTreeSet::new();
        let voice_stride = if self.parameters.revision_number() >= 15 {
            0x188
        } else {
            0x170
        };
        for wire in voice_bytes.chunks_exact(voice_stride) {
            let id = word(wire, 0);
            if wire[9] == 0 {
                continue;
            }
            if !voice_ids.insert(id) {
                return Err(unsupported("duplicate voice id"));
            }
            let old = self.graph.as_ref().and_then(|g| g.voices.get(&id));
            let queued = word(wire, 60) > 0;
            if wire[10] == 1 {
                if old.is_some_and(|v| v.has_queued_buffers()) {
                    return Err(unsupported("queued voice stop/depop"));
                }
                if let Some(old) = old {
                    let mut state = old.clone();
                    state.playing = false;
                    result.voices.insert(id, state);
                }
                continue;
            }
            if !queued {
                if let Some(old) = old {
                    let mut state = old.clone();
                    state.playing = false;
                    result.voices.insert(id, state);
                }
                continue;
            }
            if self.parameters.revision_number() != 11 {
                return Err(unsupported("queued voice revision"));
            }
            let voice = crate::host_audio_dsp::voice::MonoVoice::prepare(
                old,
                wire,
                self.parameters.sample_rate,
            )?;
            for (base, size) in voice.backing_ranges() {
                let range = attached_audio_pool(pools, base, size)?;
                if !result.pools.contains(&range) {
                    result.pools.push(range);
                }
            }
            // Validate all currently referenced reads and routes on a disposable
            // state. Installation retains the unadvanced queue/filter history.
            let mut preview = voice.clone();
            let source = preview.render(memory, self.parameters.sample_count as usize)?;
            let mut scratch: Vec<_> = result
                .mixes
                .iter()
                .map(|m| {
                    vec![
                        vec![0.; self.parameters.sample_count as usize];
                        m.as_ref().map_or(0, |m| m.channels)
                    ]
                })
                .collect();
            let mut routing = result.routing.clone();
            routing.route_mono(
                voice.splitter as usize,
                self.parameters.sample_rate,
                &source,
                &mut scratch,
            )?;
            result.voices.insert(id, voice);
        }
        if self.graph.as_ref().is_some_and(|g| {
            g.voices
                .iter()
                .any(|(id, v)| !voice_ids.contains(id) && v.has_queued_buffers())
        }) {
            return Err(unsupported("queued voice release/depop"));
        }
        result.pools.sort_unstable();
        for (id, m) in result
            .mixes
            .iter()
            .enumerate()
            .filter_map(|(id, m)| m.as_ref().map(|m| (id, m)))
        {
            let effects: Vec<_> = result
                .effects
                .iter()
                .flatten()
                .filter(|e| e.mix == id)
                .collect();
            if effects.len() != m.effect_count
                || effects
                    .iter()
                    .enumerate()
                    .any(|(i, e)| effects[..i].iter().any(|p| p.order == e.order))
            {
                return Err(unsupported("mix effect count/order"));
            }
        }
        result.preflight(memory, self.parameters.sample_count as usize)?;
        Ok(result)
    }
}

fn attached_audio_pool(pools: &[u8], base: u64, size: u64) -> RuntimeResult<(u64, u64)> {
    let end = base
        .checked_add(size)
        .filter(|e| *e > base)
        .ok_or("audio source range overflow/empty")?;
    let pool = pools
        .chunks_exact(32)
        .find(|p| {
            matches!(word(p, 16), 4 | 5)
                && wide(p, 0) <= base
                && wide(p, 0).checked_add(wide(p, 8)).is_some_and(|e| end <= e)
        })
        .ok_or("audio source outside attached pool")?;
    Ok((wide(pool, 0), wide(pool, 8)))
}
