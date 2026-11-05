//! Bounded Switch IGraphicBufferProducer wire service. Single VI layer, preallocated
//! RGBA buffers. Queue acceptance is separate from consumer release/presentation.
use std::cell::Cell;
use std::collections::VecDeque;
use wl_guest_runtime::{
    Cpu, Memory, Process, RuntimeResult,
    audio::Reply,
    horizon::Object,
    ipc::{Endpoint, Request},
    services::HostObject,
};
pub const INDEX: usize = 1 << 27;
const SLOTS: usize = 32;
const MAX_PARCEL: usize = 16 * 1024;
const BAD_VALUE: i32 = -22;
const INVALID_OPERATION: i32 = -38;
const NO_INIT: i32 = -19;
const WOULD_BLOCK: i32 = -11;
/// Scoped compositor hooks. The acquire callback must copy from checked,
/// GPU-visible backing and prove acquire fences before returning an image.
#[derive(Clone, Copy)]
pub struct Consumer {
    pub acquire: fn(
        &mut Memory,
        &QueuedBuffer,
    ) -> RuntimeResult<Option<wl_guest_runtime::display::Framebuffer>>,
    pub present: fn(&wl_guest_runtime::display::Framebuffer) -> RuntimeResult<()>,
}
thread_local! { static CONSUMER: Cell<Option<Consumer>> = const { Cell::new(None) }; }
pub fn install_consumer(consumer: Consumer) -> RuntimeResult<()> {
    CONSUMER.with(|slot| {
        if slot.get().is_some() {
            return Err("native-window consumer already installed on this host thread".into());
        }
        slot.set(Some(consumer));
        Ok(())
    })
}
#[derive(Debug, Clone, Copy)]
pub struct NativeSurface {
    pub nvmap_id: u32,
    pub offset: u64,
    pub size: usize,
    pub pitch: u32,
    pub layout: u32,
    pub block_height_log2: u8,
}
#[derive(Debug, Clone)]
pub struct GraphicBuffer {
    pub bytes: Vec<u8>,
    pub width: u32,
    pub height: u32,
    pub stride: u32,
    pub format: u32,
    pub usage: u32,
}
impl GraphicBuffer {
    /// NvGraphicBuffer/NvSurface layout from libnx feebd026 graphic_buffer.h.
    /// Supports one uncompressed progressive RGBA8 plane, not opaque pointers.
    pub fn native_surface(&self) -> RuntimeResult<NativeSurface> {
        let b = self.bytes.get(40..).ok_or("short native buffer")?;
        let w = |n| u32_at(b, n).ok_or("short native surface");
        let q = |n| -> RuntimeResult<u64> { Ok(u64::from(w(n)?) | (u64::from(w(n + 4)?) << 32)) };
        if b.len() != 324
            || w(0)? != u32::MAX
            || w(4)? == 0
            || w(12)? != 0xdaffcaff
            || w(28)? != self.format
            || w(36)? != self.stride
            || w(44)? != 1
            || w(52)? != self.width
            || w(56)? != self.height
            || self.format != 1
            || q(60)? != 0x0100532120
            || !matches!(w(68)?, 1 | 3)
            || w(84)? != if w(68)? == 3 { 0xfe } else { 0 }
            || w(88)? > 5
            || w(92)? != 0
            || w(96)? != 0
            || !matches!(q(100)?, 0 | 0x8000000000000000)
            || b[116..316].iter().any(|v| *v != 0)
        {
            return Err("unsupported native RGBA scanout surface".into());
        }
        let size = usize::try_from(q(108)?).map_err(|_| "native surface size overflow")?;
        if size == 0 || size > 64 * 1024 * 1024 || u64::from(w(40)?) < size as u64 {
            return Err("native surface allocation size".into());
        }
        Ok(NativeSurface {
            nvmap_id: w(4)?,
            offset: u64::from(w(80)?),
            size,
            pitch: w(72)?,
            layout: w(68)?,
            block_height_log2: w(88)? as u8,
        })
    }
    fn decode(bytes: &[u8]) -> Result<Self, i32> {
        let get = |n| u32_at(bytes, n).ok_or(BAD_VALUE);
        if bytes.len() < 40
            || bytes.len() > 1024
            || bytes.len() % 4 != 0
            || get(0)? != 0x47424652
            || get(32)? != 0
            || get(36)? > 128
            || 40 + get(36)? as usize * 4 != bytes.len()
        {
            return Err(BAD_VALUE);
        }
        let (width, height, stride, format) = (get(4)?, get(8)?, get(12)?, get(16)?);
        if width == 0
            || height == 0
            || width > 8192
            || height > 8192
            || stride < width
            || !matches!(format, 1 | 5)
            || u64::from(stride) * u64::from(height) * 4 > 64 * 1024 * 1024
        {
            return Err(BAD_VALUE);
        }
        Ok(Self {
            bytes: bytes.to_vec(),
            width,
            height,
            stride,
            format,
            usage: get(20)?,
        })
    }
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Slot {
    Free,
    Dequeued,
    Queued,
}
#[derive(Debug, Clone)]
pub struct QueuedBuffer {
    pub slot: u32,
    pub sequence: u64,
    pub buffer: GraphicBuffer,
    pub input: Vec<u8>,
}
#[derive(Debug)]
pub struct Service {
    connected: bool,
    count: usize,
    width: u32,
    height: u32,
    format: u32,
    buffers: [Option<GraphicBuffer>; SLOTS],
    slots: [Slot; SLOTS],
    free_fences: [[u8; 36]; SLOTS],
    queued: VecDeque<QueuedBuffer>,
    sequence: u64,
    last_present_tick: Option<u64>,
    latch: Option<(u64, u64)>,
    timer_armed: bool,
}
impl Default for Service {
    fn default() -> Self {
        Self {
            connected: false,
            count: 2,
            width: 1280,
            height: 720,
            format: 1,
            buffers: std::array::from_fn(|_| None),
            slots: [Slot::Free; SLOTS],
            free_fences: [[0; 36]; SLOTS],
            queued: VecDeque::new(),
            sequence: 0,
            last_present_tick: None,
            latch: None,
            timer_armed: false,
        }
    }
}
#[derive(Debug)]
enum Change {
    None,
    Connect,
    Disconnect,
    Count(usize),
    Buffer(usize, GraphicBuffer),
    Slot(usize, Slot, [u8; 36]),
    Queue(QueuedBuffer),
}
fn u32_at(b: &[u8], at: usize) -> Option<u32> {
    Some(u32::from_le_bytes(
        b.get(at..at.checked_add(4)?)?.try_into().ok()?,
    ))
}
fn words(values: &[u32]) -> Vec<u8> {
    values.iter().flat_map(|v| v.to_le_bytes()).collect()
}
fn parcel(data: &[u8]) -> Vec<u8> {
    let mut b = words(&[data.len() as u32, 16, 0, 16 + data.len() as u32]);
    b.extend_from_slice(data);
    b
}
struct Reader<'a> {
    bytes: &'a [u8],
    at: usize,
}
impl<'a> Reader<'a> {
    fn word(&mut self) -> Result<u32, i32> {
        let w = u32_at(self.bytes, self.at).ok_or(BAD_VALUE)?;
        self.at += 4;
        Ok(w)
    }
    fn flat(&mut self) -> Result<&'a [u8], i32> {
        let len = self.word()? as usize;
        if len > 1024 || self.word()? != 0 {
            return Err(BAD_VALUE);
        }
        let end = self.at.checked_add(len).ok_or(BAD_VALUE)?;
        let b = self.bytes.get(self.at..end).ok_or(BAD_VALUE)?;
        self.at = end.checked_add(3).ok_or(BAD_VALUE)? & !3;
        Ok(b)
    }
    fn end(&self) -> Result<(), i32> {
        if self.at == self.bytes.len() {
            Ok(())
        } else {
            Err(BAD_VALUE)
        }
    }
}
fn input(bytes: &[u8]) -> Result<Reader<'_>, i32> {
    if bytes.len() < 16 || bytes.len() > MAX_PARCEL {
        return Err(BAD_VALUE);
    }
    let size = u32_at(bytes, 0).ok_or(BAD_VALUE)? as usize;
    let off = u32_at(bytes, 4).ok_or(BAD_VALUE)? as usize;
    let objects = u32_at(bytes, 8).ok_or(BAD_VALUE)?;
    let objects_at = u32_at(bytes, 12).ok_or(BAD_VALUE)? as usize;
    let end = off.checked_add(size).ok_or(BAD_VALUE)?;
    if off != 16 || end != bytes.len() || objects != 0 || objects_at != end {
        return Err(BAD_VALUE);
    }
    let mut r = Reader {
        bytes: &bytes[off..end],
        at: 0,
    };
    if r.word()? != 0x100 {
        return Err(BAD_VALUE);
    }
    let len = r.word()? as usize;
    let token = "android.gui.IGraphicBufferProducer";
    if len != token.len() {
        return Err(BAD_VALUE);
    }
    let n = (len + 1).checked_mul(2).ok_or(BAD_VALUE)?;
    let token_end = r.at.checked_add(n).ok_or(BAD_VALUE)?;
    let string = r.bytes.get(r.at..token_end).ok_or(BAD_VALUE)?;
    for (unit, expected) in string.chunks_exact(2).zip(token.encode_utf16().chain([0])) {
        if u16::from_le_bytes(unit.try_into().unwrap()) != expected {
            return Err(BAD_VALUE);
        }
    }
    r.at = (token_end + 3) & !3;
    Ok(r)
}
fn slot(value: u32) -> Result<usize, i32> {
    let n = value as usize;
    if n < SLOTS { Ok(n) } else { Err(BAD_VALUE) }
}
impl Service {
    fn error_reply(&self, code: u32, error: i32) -> Vec<u8> {
        match code {
            1 => words(&[0, error as u32]),
            3 => words(&[u32::MAX, 0, error as u32]),
            9 => words(&[0, error as u32]),
            7 | 10 => {
                let mut b = self.output(self.queued.len() as u32);
                b.extend_from_slice(&(error as u32).to_le_bytes());
                b
            }
            _ => words(&[error as u32]),
        }
    }
    fn output(&self, pending: u32) -> Vec<u8> {
        words(&[self.width, self.height, 0, pending])
    }
    fn plan(&self, code: u32, bytes: &[u8]) -> Result<(Vec<u8>, Change), i32> {
        let mut r = input(bytes)?;
        if code == 10 {
            if r.word()? != 0 || r.word()? != 1 || r.word()? > 1 {
                return Err(BAD_VALUE);
            }
            r.end()?;
            if self.connected {
                return Err(INVALID_OPERATION);
            }
            let mut reply = self.output(0);
            reply.extend_from_slice(&0u32.to_le_bytes());
            return Ok((reply, Change::Connect));
        }
        if !self.connected {
            return Err(NO_INIT);
        }
        match code {
            9 => {
                let value = match r.word()? {
                    0 => self.width,
                    1 => self.height,
                    2 => self.format,
                    3 => 1,
                    _ => return Err(BAD_VALUE),
                };
                r.end()?;
                Ok((words(&[value, 0]), Change::None))
            }
            14 => {
                let n = slot(r.word()?)?;
                if r.word()? != 1 {
                    return Err(INVALID_OPERATION);
                }
                let buffer = GraphicBuffer::decode(r.flat()?)?;
                r.end()?;
                if self.slots[n] != Slot::Free || !self.queued.is_empty() {
                    return Err(INVALID_OPERATION);
                }
                Ok((Vec::new(), Change::Buffer(n, buffer)))
            }
            2 => {
                let n = r.word()? as usize;
                r.end()?;
                if !(2..=SLOTS).contains(&n) {
                    return Err(BAD_VALUE);
                }
                if self.slots.iter().any(|s| *s != Slot::Free) {
                    return Err(INVALID_OPERATION);
                }
                Ok((words(&[0]), Change::Count(n)))
            }
            3 => {
                if r.word()? > 1 {
                    return Err(BAD_VALUE);
                }
                let width = r.word()?;
                let height = r.word()?;
                let format = r.word()?;
                let usage = r.word()?;
                r.end()?;
                if self.slots[..self.count]
                    .iter()
                    .filter(|state| **state == Slot::Dequeued)
                    .count()
                    >= self.count - 1
                {
                    return Err(WOULD_BLOCK);
                }
                let n = (0..self.count)
                    .find(|&n| {
                        self.slots[n] == Slot::Free
                            && self.buffers[n].as_ref().is_some_and(|b| {
                                (width == 0 || b.width == width)
                                    && (height == 0 || b.height == height)
                                    && (format == 0 || b.format == format)
                                    && usage & !b.usage == 0
                            })
                    })
                    .ok_or(WOULD_BLOCK)?;
                // Forward an actual cancel/release fence; a fresh slot has none.
                // No GPU completion or hardware syncpoint is synthesized.
                // The SDK dereferences the returned Fence object even for a
                // fresh buffer. Serialize a non-null empty NvMultiFence, rather
                // than leaving the client's sp<Fence> null. Count zero denotes
                // absence of work; it does not complete any submitted GPU fence.
                let mut reply = words(&[n as u32, 1, 36, 0]);
                reply.extend_from_slice(&self.free_fences[n]);
                reply.extend_from_slice(&0u32.to_le_bytes());
                Ok((reply, Change::Slot(n, Slot::Dequeued, [0; 36])))
            }
            1 => {
                let n = slot(r.word()?)?;
                r.end()?;
                if n >= self.count || self.slots[n] != Slot::Dequeued {
                    return Err(BAD_VALUE);
                }
                let buffer = self.buffers[n].as_ref().ok_or(BAD_VALUE)?;
                let mut reply = words(&[1, buffer.bytes.len() as u32, 0]);
                reply.extend_from_slice(&buffer.bytes);
                reply.extend_from_slice(&0u32.to_le_bytes());
                Ok((reply, Change::None))
            }
            8 => {
                let n = slot(r.word()?)?;
                let fence = r.flat()?;
                r.end()?;
                if n >= self.count || self.slots[n] != Slot::Dequeued || fence.len() != 36 {
                    return Err(BAD_VALUE);
                }
                if u32_at(fence, 0).is_none_or(|v| v > 4) {
                    return Err(BAD_VALUE);
                }
                Ok((
                    Vec::new(),
                    Change::Slot(n, Slot::Free, fence.try_into().unwrap()),
                ))
            }
            7 => {
                let n = slot(r.word()?)?;
                let b = r.flat()?;
                r.end()?;
                if n >= self.count || self.slots[n] != Slot::Dequeued || b.len() != 84 {
                    return Err(BAD_VALUE);
                }
                // BqBufferInput.swapInterval at byte 44. This virtual display
                // supports 0..4, not an asserted hardware-wide maximum.
                if u32_at(b, 8).is_none_or(|v| v > 1)
                    || u32_at(b, 8) == Some(0) && b[..8].iter().any(|v| *v != 0)
                    || u32_at(b, 44).is_none_or(|v| v > 4)
                    || u32_at(b, 48).is_none_or(|v| v > 4)
                {
                    return Err(INVALID_OPERATION);
                }
                if u32_at(b, 32).is_none_or(|v| v & !7 != 0)
                    || !matches!(u32_at(b, 36), Some(0 | 8))
                {
                    return Err(INVALID_OPERATION);
                }
                let buffer = self.buffers[n].as_ref().ok_or(BAD_VALUE)?.clone();
                let sequence = self.sequence.checked_add(1).ok_or(INVALID_OPERATION)?;
                let q = QueuedBuffer {
                    slot: n as u32,
                    sequence,
                    buffer,
                    input: b.to_vec(),
                };
                let mut reply = self.output(self.queued.len() as u32 + 1);
                reply.extend_from_slice(&0u32.to_le_bytes());
                Ok((reply, Change::Queue(q)))
            }
            11 => {
                if r.word()? != 1 {
                    return Err(BAD_VALUE);
                }
                r.end()?;
                if self.slots.iter().any(|s| *s != Slot::Free) {
                    return Err(INVALID_OPERATION);
                }
                Ok((words(&[0]), Change::Disconnect))
            }
            _ => Err(INVALID_OPERATION),
        }
    }
    fn commit(&mut self, change: Change) {
        match change {
            Change::None => {}
            Change::Connect => self.connected = true,
            Change::Disconnect => {
                self.connected = false;
                self.buffers = std::array::from_fn(|_| None);
                self.free_fences = [[0; 36]; SLOTS];
                self.last_present_tick = None;
                self.latch = None;
                self.timer_armed = false;
            }
            Change::Count(n) => self.count = n,
            Change::Slot(n, state, fence) => {
                self.slots[n] = state;
                self.free_fences[n] = fence;
            }
            Change::Buffer(n, b) => {
                self.width = b.width;
                self.height = b.height;
                self.format = b.format;
                self.buffers[n] = Some(b);
            }
            Change::Queue(q) => {
                self.sequence = q.sequence;
                self.slots[q.slot as usize] = Slot::Queued;
                self.queued.push_back(q);
            }
        }
    }
    /// Consumer observation is explicit. Queue acceptance alone cannot release
    /// a buffer, report GPU completion, or increment a VI frame counter.
    pub fn update_event(&self, k: &mut wl_guest_runtime::horizon::Kernel) {
        let Some(event) = k.host.binder_event else {
            return;
        };
        let held = self.slots[..self.count]
            .iter()
            .filter(|state| **state == Slot::Dequeued)
            .count();
        let available = held < self.count - 1
            && (0..self.count).any(|n| self.slots[n] == Slot::Free && self.buffers[n].is_some());
        if available {
            wl_guest_runtime::threads::signal_event(k, event);
        } else if let Some(event) = k.events.get_mut(event) {
            event.signaled = false;
        }
    }
    pub fn pending(&self) -> Option<&QueuedBuffer> {
        self.queued.front()
    }
    pub fn consume_ready(
        &mut self,
        memory: &mut Memory,
        k: &mut wl_guest_runtime::horizon::Kernel,
    ) -> RuntimeResult<()> {
        let Some(consumer) = CONSUMER.with(Cell::get) else {
            return Ok(());
        };
        let Some(queued) = self.queued.front() else {
            return Ok(());
        };
        let interval = u32_at(&queued.input, 44).ok_or("short swap interval")?;
        if interval != 0 {
            if self.latch.is_none() {
                let current = (u128::from(k.time_ns) * 60 / 1_000_000_000) as u64;
                let tick = self
                    .last_present_tick
                    .map_or_else(
                        || current.checked_add(u64::from(interval)),
                        |last| {
                            last.checked_add(u64::from(interval))
                                .and_then(|tick| current.checked_add(1).map(|next| tick.max(next)))
                        },
                    )
                    .ok_or("VI virtual display tick overflow")?;
                let deadline = u64::try_from((u128::from(tick) * 1_000_000_000).div_ceil(60))
                    .map_err(|_| "VI virtual display deadline overflow")?;
                self.latch = Some((tick, deadline));
                self.timer_armed = true;
            }
            if k.time_ns < self.latch.unwrap().1 {
                return Ok(());
            }
            // A due but unresolved GPU fence cannot cause a zero-time idle spin.
            // Polling retries it after actual work/completion; no fake wakeup.
            self.timer_armed = false;
        }
        let Some(frame) = (consumer.acquire)(memory, queued)? else {
            return Ok(());
        };
        let size = frame
            .width
            .checked_mul(frame.height)
            .and_then(|n| n.checked_mul(4))
            .ok_or("VI image size overflow")?;
        if frame.width != queued.buffer.width
            || frame.height != queued.buffer.height
            || frame.stride != frame.width
            || frame.format != wl_guest_runtime::display::PixelFormat::Rgba8888
            || frame.pixels.len() != size as usize
        {
            return Err("invalid acquired VI image".into());
        }
        k.host
            .display
            .frame_count
            .checked_add(1)
            .ok_or("VI frame count exhausted")?;
        let sequence = queued.sequence;
        let tick = self.latch.map_or_else(
            || (u128::from(k.time_ns) * 60).div_ceil(1_000_000_000) as u64,
            |(tick, _)| tick,
        );
        k.host
            .display
            .queue_buffer(frame.width, frame.height, frame.stride, 1, frame.pixels);
        self.release_consumed(sequence)?;
        self.last_present_tick = Some(tick);
        self.update_event(k);
        (consumer.present)(k.host.display.current_frame.as_ref().unwrap())
    }
    /// Virtual 60 Hz display deadline, not a GPU fence or native vblank.
    pub fn next_deadline_ns(&self) -> Option<u64> {
        self.latch.filter(|_| self.timer_armed).map(|(_, ns)| ns)
    }
    /// Trusted consumer acknowledgement after its acquire fences and actual
    /// consumption complete. This method does not wait or prove GPU completion;
    /// it is never reachable as a producer IPC transaction.
    pub fn release_consumed(&mut self, sequence: u64) -> RuntimeResult<()> {
        let front = self.queued.front().ok_or("no queued Binder buffer")?;
        if front.sequence != sequence {
            return Err("stale/out-of-order Binder release".into());
        }
        let q = self.queued.pop_front().unwrap();
        self.slots[q.slot as usize] = Slot::Free;
        self.latch = None;
        self.timer_armed = false;
        Ok(())
    }
    pub fn request(
        &mut self,
        memory: &mut Memory,
        command: u32,
        req: &Request,
        args: &[u8],
    ) -> RuntimeResult<Reply> {
        if !matches!(command, 0 | 3)
            || args.len() < 12
            || u32_at(args, 0) != Some(32)
            || u32_at(args, 8) != Some(0)
            || req.pid.is_some()
            || req.copy_handles != 0
            || req.move_handles != 0
            || req.exchanges != 0
        {
            return Err("unsupported Binder transaction metadata".into());
        }
        let mut sources = req
            .statics
            .iter()
            .chain(req.sends.iter())
            .copied()
            .filter(|(_, n)| *n != 0);
        let source = sources.next().ok_or("Binder transaction has no input")?;
        if sources.next().is_some() || source.1 > MAX_PARCEL as u64 {
            return Err("unsupported Binder input span".into());
        }
        let mut outputs = req.recvs.iter().copied().filter(|(_, n)| *n != 0);
        let output = outputs.next().ok_or("Binder transaction has no output")?;
        if outputs.next().is_some() {
            return Err("multiple Binder outputs".into());
        }
        let code = u32_at(args, 4).ok_or("short Binder code")?;
        if !matches!(code, 1 | 2 | 3 | 7 | 8 | 9 | 10 | 11 | 14) {
            return Err("unsupported producer transaction".into());
        }
        let bytes = memory.read_bytes(source.0, source.1 as usize)?;
        let (data, change) = match self.plan(u32_at(args, 4).ok_or("short Binder code")?, bytes) {
            Ok(p) => p,
            Err(error) => {
                if matches!(code, 8 | 14) {
                    return Err(format!("Binder transaction {code} rejected: {error}"));
                }
                (self.error_reply(code, error), Change::None)
            }
        };
        let reply = parcel(&data);
        if reply.len() as u64 > output.1 {
            return Err("Binder reply buffer too small".into());
        }
        memory.validate_write(output.0, reply.len())?;
        memory.write_bytes(output.0, &reply)?;
        self.commit(change);
        Ok(Reply::ok())
    }
}
/// Temporary typed callback routing uses the runtime's existing opaque service
/// tag. No guest-visible object/handle is fabricated or replaced permanently.
pub struct Binding {
    session: usize,
    original: Endpoint,
}
pub fn bind(cpu: &Cpu, process: &mut Process, svc: u16) -> Option<Binding> {
    if svc != 0x21 {
        return None;
    }
    let k = process.kernel.as_ref()?;
    let Object::Session(n) = k.handles.get(cpu.x[0] as u32)? else {
        return None;
    };
    let s = k.sessions.get(n)?;
    if s.closed
        || s.references == 0
        || s.domain.is_some()
        || s.endpoint != Endpoint::Host(HostObject::HosBinderDriver)
    {
        return None;
    }
    let mut b = [0u8; 256];
    process
        .memory
        .read_into(k.threads.get(k.current)?.tls, &mut b)
        .ok()?;
    let w0 = u32_at(&b, 0)?;
    let w1 = u32_at(&b, 4)?;
    if !matches!(w0 as u16, 4 | 6) {
        return None;
    }
    let mut at = 8;
    if w1 >> 31 != 0 {
        let h = u32_at(&b, 8)?;
        at = 12 + if h & 1 != 0 { 8 } else { 0 } + 4 * (((h >> 1) & 15) + ((h >> 5) & 15)) as usize;
    }
    at += 8 * ((w0 >> 16) & 15) as usize
        + 12 * (((w0 >> 20) & 15) + ((w0 >> 24) & 15) + (w0 >> 28)) as usize;
    let end = at.checked_add(4 * (w1 & 0x3ff) as usize)?;
    let raw = at.checked_add(15)? & !15;
    if end > 256
        || raw + 16 > end
        || b.get(raw..raw + 4) != Some(b"SFCI")
        || !matches!(u32_at(&b, raw + 8)?, 0 | 3)
    {
        return None;
    }
    let original = s.endpoint;
    process.kernel.as_mut()?.sessions[n].endpoint =
        Endpoint::Host(HostObject::AudioRenderer { index: INDEX });
    Some(Binding {
        session: n,
        original,
    })
}
pub fn restore(process: &mut Process, binding: Option<Binding>) {
    if let Some(b) = binding
        && let Some(k) = process.kernel.as_mut()
        && let Some(s) = k.sessions.get_mut(b.session)
    {
        s.endpoint = b.original;
    }
}
