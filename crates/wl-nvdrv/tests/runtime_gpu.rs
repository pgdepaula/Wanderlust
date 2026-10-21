//! Real runtime HIPC/CMIF -> NVIDIA device adapter -> GPFIFO -> wgpu.
//! Synthetic independently assembled Maxwell programs; no original game trace.
extern crate wl_runtime as wl_guest_runtime;
#[path = "../../wl-gpu/tests/common/mod.rs"]
mod common;
#[path = "../src/runtime_adapter.rs"]
mod native;
use common::{maxwell::*, *};
use wl_gpu::maxwell::{TextureImage, TextureSampler};
use wl_guest_runtime::{Cpu, Memory, Process, horizon::*, ipc, nvidia, services::HostObject};
type Buffer = (u64, u64);
const DATA: u64 = 0x10000000;
const META: u64 = 0x11000000;
const CODE: u64 = 0x08000000;
const TRANSFER: u64 = 0x20000000;
const RT: usize = 0x8000;
const SIZE: usize = 208 * 29;
const BG: [u8; 4] = [11, 17, 23, 255];
const POS: [[f32; 2]; 3] = [[-0.8, -0.6], [0.6, -0.4], [-0.2, 0.7]];
const UV: [[f32; 2]; 3] = [[0.1, 0.2], [0.9, 0.1], [0.25, 0.85]];
fn cache_policy(h: &mut Harness, value: u64) {
    h.cpu.x[..4].copy_from_slice(&[DATA, 0x18000, 8, value]);
    assert_eq!(dispatch(&mut h.cpu, &mut h.p, 3).unwrap(), None);
    assert_eq!(h.cpu.x[0], 0);
    assert!(h.p.memory.ensure_gpu_unpinned(DATA, 0x18000).is_err());
}
fn method(w: &mut Vec<u32>, m: u32, v: u32) {
    w.extend([0x20010000 | (m / 4), v]);
}
fn entry(address: u64, words: usize) -> [u8; 8] {
    // Independent libnx start | (num_cmds<<42).
    (address | ((words as u64) << 42)).to_le_bytes()
}
fn put(data: &mut [u8], offset: usize, words: &[u32]) {
    for (b, w) in data[offset..offset + words.len() * 4]
        .chunks_exact_mut(4)
        .zip(words)
    {
        b.copy_from_slice(&w.to_le_bytes());
    }
}
fn fixture() -> (Vec<u8>, Vec<u32>, Vec<u8>) {
    let desc = TextureImage::decode(TIC_RGBA)
        .unwrap()
        .with_sampler(TextureSampler::decode(TSC_NEAREST).unwrap());
    let (surface, images) = rgba_surface(desc);
    let mut data = vec![0u8; 0x18000];
    data[..64].copy_from_slice(&descriptor_bytes(TIC_RGBA, TSC_NEAREST));
    data[0x1000..0x1000 + surface.len()].copy_from_slice(&surface);
    for (off, bin) in [
        (
            0x4000,
            include_bytes!("../../wl-gpu/tests/fixtures/maxwell/tex.bin").as_slice(),
        ),
        (
            0x4100,
            include_bytes!("../../wl-gpu/tests/fixtures/maxwell/quarters.bin").as_slice(),
        ),
        (
            0x4200,
            include_bytes!("../../wl-gpu/tests/fixtures/maxwell/vertex.bin").as_slice(),
        ),
        (
            0x4400,
            include_bytes!("../../wl-gpu/tests/fixtures/maxwell/red.bin").as_slice(),
        ),
    ] {
        data[off..off + bin.len()].copy_from_slice(bin);
    }
    for n in 0..3 {
        for (c, v) in [POS[n][0], POS[n][1], 0., 1., UV[n][0], UV[n][1]]
            .into_iter()
            .enumerate()
        {
            data[0x3000 + n * 24 + c * 4..0x3004 + n * 24 + c * 4]
                .copy_from_slice(&v.to_le_bytes());
        }
    }
    for (n, i) in [0u16, 1, 2].into_iter().enumerate() {
        data[0x6000 + n * 2..0x6002 + n * 2].copy_from_slice(&i.to_le_bytes());
    }
    for p in data[RT..RT + SIZE].chunks_exact_mut(4) {
        p.copy_from_slice(&BG);
    }
    let mut w = common::hardware::setup();
    for (m, v) in [
        (0x17c8, 0),
        (0x17cc, 0x80000),
        (0x17d0, 0),
        (0x17d4, 0x80005),
        (0x17d8, 1),
        (0x1434, 0),
        (0x1438, 0),
        (0x1644, 0),
    ] {
        method(&mut w, m, v);
    }
    w.extend([0x20010586, 4, 0x200105f7, 0, 0x200105f8, 3, 0x20010585, 0]);
    // TSC and CB are subregions of a page-aligned canonical allocation.
    // Change their actual B197 addresses, not the texture handle semantics.
    for word in &mut w {
        if *word == 0x31000 {
            *word = 0x31020;
        } else if *word == 0x40000 {
            *word = 0x40040;
        }
    }
    (data, w, images[0].clone())
}
fn edge(a: [f64; 2], b: [f64; 2], p: [f64; 2]) -> f64 {
    (b[0] - a[0]) * (p[1] - a[1]) - (b[1] - a[1]) * (p[0] - a[0])
}
fn check(pixels: &[u8], image: &[u8], solid: Option<[u8; 4]>, bg: [u8; 4]) {
    let v = POS.map(|p| p.map(f64::from));
    let area = edge(v[0], v[1], v[2]);
    let mut counts = [0, 0];
    for y in 0..29 {
        for x in 0..52 {
            let p = [
                (x as f64 + 0.5) * 2. / 47. - 1.,
                1. - (y as f64 + 0.5) * 2. / 29.,
            ];
            let b = [
                edge(v[1], v[2], p) / area,
                edge(v[2], v[0], p) / area,
                edge(v[0], v[1], p) / area,
            ];
            if x < 47 && b.iter().any(|v| v.abs() < 0.005) {
                continue;
            }
            let inside = x < 47 && b.iter().all(|v| *v >= 0.);
            let expected = if inside {
                if let Some(s) = solid {
                    s
                } else {
                    let u = (0..3).map(|n| b[n] * f64::from(UV[n][0])).sum::<f64>() * 37.;
                    let v = (0..3).map(|n| b[n] * f64::from(UV[n][1])).sum::<f64>() * 19.;
                    if (u - u.round()).abs() < 0.02 || (v - v.round()).abs() < 0.02 {
                        continue;
                    }
                    let i = (v.floor().clamp(0., 18.) as usize * 37
                        + u.floor().clamp(0., 36.) as usize)
                        * 4;
                    image[i..i + 4].try_into().unwrap()
                }
            } else {
                bg
            };
            assert_eq!(
                &pixels[(y * 52 + x) * 4..(y * 52 + x + 1) * 4],
                expected,
                "pixel{x},{y}"
            );
            counts[usize::from(inside)] += 1;
        }
    }
    assert!(counts[0] > 600 && counts[1] > 100);
}

#[derive(Default)]
struct Wire<'a> {
    command: u32,
    args: &'a [u8],
    pid: bool,
    copies: &'a [u32],
    x: &'a [Buffer],
    a: &'a [Buffer],
    b: &'a [Buffer],
    c: &'a [Buffer],
}
fn put32(out: &mut [u8], at: usize, value: u32) {
    out[at..at + 4].copy_from_slice(&value.to_le_bytes());
}
impl Wire<'_> {
    fn encode(&self) -> [u8; 0x100] {
        // This does not call or expose the production Request parser.
        let mut out = [0; 0x100];
        let special = self.pid || !self.copies.is_empty();
        let raw_size = (32 + self.args.len() + 1) & !1;
        let words = (raw_size + 2 * self.c.len()).div_ceil(4);
        let c_mode = if self.c.is_empty() {
            0
        } else {
            self.c.len() as u32 + 2
        };
        put32(
            &mut out,
            0,
            4 | ((self.x.len() as u32) << 16)
                | ((self.a.len() as u32) << 20)
                | ((self.b.len() as u32) << 24),
        );
        put32(
            &mut out,
            4,
            words as u32 | (c_mode << 10) | (u32::from(special) << 31),
        );
        let mut at = 8;
        if special {
            put32(
                &mut out,
                at,
                u32::from(self.pid) | ((self.copies.len() as u32) << 1),
            );
            at += 4;
            if self.pid {
                out[at..at + 8].copy_from_slice(&0xdead_beef_f00d_1234u64.to_le_bytes());
                at += 8;
            }
            for &handle in self.copies {
                put32(&mut out, at, handle);
                at += 4;
            }
        }
        for (index, &(address, size)) in self.x.iter().enumerate() {
            assert!(size <= 0xffff && address < (1 << 42));
            put32(
                &mut out,
                at,
                ((size as u32) << 16)
                    | (((address >> 32) as u32 & 15) << 12)
                    | (((address >> 36) as u32 & 63) << 6)
                    | index as u32,
            );
            put32(&mut out, at + 4, address as u32);
            at += 8;
        }
        for &(address, size) in self.a.iter().chain(self.b.iter()) {
            assert!(size < (1 << 36) && address < (1 << 39));
            put32(&mut out, at, size as u32);
            put32(&mut out, at + 4, address as u32);
            put32(
                &mut out,
                at + 8,
                (((address >> 32) as u32 & 15) << 28)
                    | (((address >> 36) as u32 & 7) << 2)
                    | (((size >> 32) as u32 & 15) << 24),
            );
            at += 12;
        }
        let raw = at;
        let cmif = (raw + 15) & !15;
        out[cmif..cmif + 4].copy_from_slice(b"SFCI");
        put32(&mut out, cmif + 8, self.command);
        out[cmif + 16..cmif + 16 + self.args.len()].copy_from_slice(self.args);
        for (i, &(_, size)) in self.c.iter().enumerate() {
            let start = raw + raw_size + 2 * i;
            out[start..start + 2].copy_from_slice(&(size as u16).to_le_bytes());
        }
        at = raw + 4 * words;
        for &(address, size) in self.c {
            assert!(address < (1 << 39) && size <= 0xffff);
            put32(&mut out, at, address as u32);
            put32(
                &mut out,
                at + 4,
                (address >> 32) as u32 | ((size as u32) << 16),
            );
            at += 8;
        }
        assert!(at <= out.len());
        out
    }
}
type Metrics =
    std::rc::Rc<std::cell::RefCell<(wl_gpu::Counters, wl_gpu::driver_backend::DriverCounters)>>;
#[derive(Debug)]
struct ObservedHost {
    inner: native::NativeHost,
    metrics: Metrics,
}
impl nvidia::Host for ObservedHost {
    fn initialize(&mut self, t: nvidia::TransferBuffer) -> nvidia::HostResult<()> {
        self.inner.initialize(t)
    }
    fn set_client_pid(&mut self, a: u64, p: u64) -> nvidia::HostResult<()> {
        self.inner.set_client_pid(a, p)
    }
    fn open(&mut self, path: &str) -> nvidia::HostResult<u32> {
        self.inner.open(path)
    }
    fn close(&mut self, fd: u32, m: &mut Memory) -> nvidia::HostResult<()> {
        self.inner.close(fd, m)
    }
    fn ioctl(&mut self, r: nvidia::Ioctl<'_>, m: &mut Memory) -> nvidia::HostResult<()> {
        let result = self.inner.ioctl(r, m);
        *self.metrics.borrow_mut() = (self.inner.graphics_counters(), self.inner.driver_counters());
        result
    }
}
struct Harness {
    p: Process,
    cpu: Cpu,
    session: u32,
    metrics: Metrics,
}
impl Harness {
    fn new(data: Vec<u8>) -> Self {
        let mut m = Memory::default();
        for (base, size, st, r, w, x) in [
            (CODE, 4096, state::CODE, true, false, true),
            (META, 0x10000, state::NORMAL, true, true, false),
            (TRANSFER, 0x2000, state::NORMAL, true, true, false),
        ] {
            m.map_state(base, vec![0; size], r, w, x, st).unwrap();
        }
        m.map_state(DATA, data, true, true, false, state::NORMAL)
            .unwrap();
        let (mut p, cpu) = Process::horizon(
            m,
            CODE,
            Range {
                start: CODE,
                end: CODE + 4096,
            },
            ProcessConfig {
                program_id: 0x0100000000002000,
                process_id: 0x51,
                main_thread_id: 0x72,
                main_thread_stack_size: 0x10000,
                main_thread_priority: 44,
                ideal_core: 0,
                core_mask: 1,
                priority_mask: u64::MAX,
                memory_limit: 0x20000000,
                handle_table_size: 1023,
                thread_limit: 32,
                is_application: true,
                svc_permissions: [u64::MAX; 3],
                service_access: &[(false, "*")],
            },
        )
        .unwrap();
        let k = p.kernel.as_mut().unwrap();
        let metrics = Metrics::default();
        k.nvidia
            .install(Box::new(ObservedHost {
                inner: native::NativeHost::new().unwrap(),
                metrics: metrics.clone(),
            }))
            .unwrap();
        let index = k.sessions.len();
        k.sessions.push(ipc::Session {
            endpoint: ipc::Endpoint::Host(HostObject::NvDrvServices),
            domain: None,
            closed: false,
            references: 1,
        });
        let session = k.handles.add(Object::Session(index)).unwrap();
        let transfer = k.create_transfer_memory(TRANSFER, 0x2000, 0).unwrap();
        let mut h = Self {
            p,
            cpu,
            session,
            metrics,
        };
        h.send(&Wire {
            command: 3,
            args: &0x2000u32.to_le_bytes(),
            copies: &[CURRENT_PROCESS, transfer],
            ..Wire::default()
        });
        h
    }
    fn tls(&self) -> u64 {
        let k = self.p.kernel.as_ref().unwrap();
        k.threads[k.current].tls
    }
    fn send(&mut self, wire: &Wire<'_>) {
        let tls = self.tls();
        self.p.memory.write_bytes(tls, &wire.encode()).unwrap();
        ipc::send_sync_request(
            &mut self.p.memory,
            self.p.kernel.as_mut().unwrap(),
            &mut self.cpu,
            self.session,
        )
        .unwrap();
        assert_eq!(self.cpu.x[0], 0);
        assert_eq!(self.p.memory.load(tls + 0x18, 4).unwrap(), 0);
    }
    fn open(&mut self, path: &str) -> u32 {
        self.p.memory.write_bytes(META, path.as_bytes()).unwrap();
        self.send(&Wire {
            command: 0,
            a: &[(META, path.len() as u64)],
            ..Wire::default()
        });
        assert_eq!(self.p.memory.load(self.tls() + 0x24, 4).unwrap(), 0);
        self.p.memory.load(self.tls() + 0x20, 4).unwrap() as u32
    }
    fn ioctl(&mut self, fd: u32, request: u32, input: &[u8], extra: Option<&[u8]>) -> Vec<u8> {
        let mut args = [0u8; 8];
        args[..4].copy_from_slice(&fd.to_le_bytes());
        args[4..].copy_from_slice(&request.to_le_bytes());
        self.p.memory.write_bytes(META + 0x1000, input).unwrap();
        let mut xs = vec![(
            if input.is_empty() { 0 } else { META + 0x1000 },
            input.len() as u64,
        )];
        let mut aliases = vec![(0, 0)];
        if let Some(extra) = extra {
            self.p.memory.write_bytes(META + 0x6000, extra).unwrap();
            xs.push((META + 0x6000, extra.len() as u64));
            aliases.push((0, 0));
        }
        let size = if request >> 30 & 2 != 0 {
            ((request >> 16) & 0x3fff) as u64
        } else {
            0
        };
        self.send(&Wire {
            command: if extra.is_some() { 11 } else { 1 },
            args: &args,
            x: &xs,
            a: &aliases,
            b: &[(if size == 0 { 0 } else { META + 0x3000 }, size)],
            c: &[(0, 0)],
            ..Wire::default()
        });
        assert_eq!(
            self.p.memory.load(self.tls() + 0x20, 4).unwrap(),
            0,
            "request{request:x}"
        );
        self.p
            .memory
            .read_bytes(META + 0x3000, size as usize)
            .unwrap()
            .to_vec()
    }
    fn close(&mut self, fd: u32) {
        self.send(&Wire {
            command: 2,
            args: &fd.to_le_bytes(),
            ..Wire::default()
        });
        assert_eq!(self.p.memory.load(self.tls() + 0x20, 4).unwrap(), 0);
    }
}
fn words(values: &[u32]) -> Vec<u8> {
    values.iter().flat_map(|v| v.to_le_bytes()).collect()
}
fn mapping(h: &mut Harness, space: u32, handle: u32, gpu: u64, offset: u64, size: u64) {
    let mut reserve = words(&[(size / 4096) as u32, 4096, 1, 0]);
    reserve.extend(gpu.to_le_bytes());
    h.ioctl(space, 0xc0184102, &reserve, None);
    let mut map = words(&[1, 0, handle, 0]);
    map.extend(offset.to_le_bytes());
    map.extend(size.to_le_bytes());
    map.extend(gpu.to_le_bytes());
    h.ioctl(space, 0xc0284106, &map, None);
}
const RANGES: [(u64, u64, u64); 9] = [
    (0x10000, 0x1000, 0x2000),
    (0x30000, 0, 0x1000),
    (0x31000, 0, 0x1000),
    (0x40000, 0, 0x1000),
    (0x50000, 0x4000, 0x1000),
    (0x70000, 0x3000, 0x1000),
    (0x80000, 0x6000, 0x1000),
    (0x90000, 0x8000, 0x2000),
    (0xb0000, 0x10000, 0x4000),
];
fn configured() -> (Harness, u32, u32, u32, Vec<u32>, Vec<u8>) {
    let (mut data, command, image) = fixture();
    put(&mut data, 0x10000, &command);
    let mut h = Harness::new(data);
    let nvmap = h.open("/dev/nvmap");
    let space = h.open("/dev/nvhost-as-gpu");
    let channel = h.open("/dev/nvhost-gpu");
    assert_ne!(nvmap, space);
    assert_ne!(channel, space);
    let created = h.ioctl(nvmap, 0xc0080101, &words(&[0x18000, 0]), None);
    let handle = u32::from_le_bytes(created[4..8].try_into().unwrap());
    let mut alloc = words(&[handle, 0, 0, 0x20000, 0, 0]);
    alloc.extend(DATA.to_le_bytes());
    h.ioctl(nvmap, 0xc0200104, &alloc, None);
    assert!(h.p.memory.ensure_gpu_unpinned(DATA, 4096).is_err());
    cache_policy(&mut h, 8);
    // Explicit Switch/libnx field order, including the selected 64 KiB page.
    // This fixture retains the default VA bounds for its low-address mappings.
    let mut initialization = [0; 40];
    initialization[0] = 1;
    initialization[8..12].copy_from_slice(&65536u32.to_le_bytes());
    h.ioctl(space, 0x40284109, &initialization, None);
    h.ioctl(channel, 0x40044801, &nvmap.to_le_bytes(), None);
    h.ioctl(space, 0x40044101, &channel.to_le_bytes(), None);
    // Bound channels retain their nvmap namespace even before any VA mapping.
    // The original faulty close freed this allocation and permitted stale-fd use.
    h.send(&Wire {
        command: 2,
        args: &nvmap.to_le_bytes(),
        ..Wire::default()
    });
    assert_eq!(h.p.memory.load(h.tls() + 0x20, 4).unwrap(), 8);
    assert!(h.p.memory.ensure_gpu_unpinned(DATA, 4096).is_err());

    let ranges = RANGES;
    for (gpu, offset, size) in ranges {
        mapping(&mut h, space, handle, gpu, offset, size);
    }
    h.ioctl(channel, 0x40084805, &words(&[2048, 0]), None);
    h.ioctl(channel, 0xc0104809, &words(&[0xb197, 0, 0, 0]), None);
    (h, nvmap, space, channel, command, image)
}
#[test]
#[ignore = "requires actual GPU execution, not only runtime transport"]
fn runtime_service_executes_indexed_textured_draw_and_cpu_mutation() {
    let (mut h, nvmap, space, channel, command, image) = configured();
    let ranges = RANGES;
    let entry_bytes = entry(0xb0000, command.len());
    let header = words(&[0, 0, 1, 6, 0, 0]);
    let output = h.ioctl(channel, 0xc018481b, &header, Some(&entry_bytes));
    assert_eq!(u32::from_le_bytes(output[12..16].try_into().unwrap()), 0);
    let syncpoint = u32::from_le_bytes(output[16..20].try_into().unwrap());
    assert_ne!(syncpoint, 0);
    assert_eq!(u32::from_le_bytes(output[20..24].try_into().unwrap()), 2);
    check(
        h.p.memory.read_bytes(DATA + RT as u64, SIZE).unwrap(),
        &image,
        None,
        BG,
    );
    let cold = *h.metrics.borrow();
    assert_eq!(cold.0.draw_commands, 1);
    assert_eq!(cold.1.accepted, 1);
    // CPU mutation affects the same canonical backing; no host texture selector.
    let color = [77, 137, 199, 255];
    h.p.memory
        .write_bytes(DATA + 0x1000, &color.repeat(6144 / 4))
        .unwrap();
    let draw = [0x20010586, 4, 0x200105f7, 0, 0x200105f8, 3, 0x20010585, 0];
    h.p.memory
        .write_bytes(DATA + 0x10000, &words(&draw))
        .unwrap();
    let entry_bytes = entry(0xb0000, draw.len());
    let output = h.ioctl(
        channel,
        0xc018481b,
        &words(&[0, 0, 1, 4, syncpoint, 2]),
        Some(&entry_bytes),
    );
    assert_eq!(
        u32::from_le_bytes(output[20..24].try_into().unwrap()),
        2,
        "no increment requested"
    );
    check(
        h.p.memory.read_bytes(DATA + RT as u64, SIZE).unwrap(),
        &image,
        Some(color),
        BG,
    );
    let warm = *h.metrics.borrow();
    assert_eq!(warm.0.draw_commands, 2);
    assert_eq!(warm.1.accepted, 2);
    assert_eq!(
        warm.0.shader_module_allocations,
        cold.0.shader_module_allocations
    );
    assert_eq!(warm.0.pipeline_allocations, cold.0.pipeline_allocations);
    eprintln!(
        "service draw metrics graphics={:?} driver={:?}",
        warm.0, warm.1
    );
    for (gpu, _, size) in ranges {
        h.ioctl(space, 0xc0084105, &gpu.to_le_bytes(), None);
        let mut free = gpu.to_le_bytes().to_vec();
        free.extend(words(&[(size / 4096) as u32, 4096]));
        h.ioctl(space, 0xc0104103, &free, None);
    }
    h.close(channel);
    h.close(space);
    cache_policy(&mut h, 0);
    h.close(nvmap);
    let pin = h.p.memory.pin_gpu_range(DATA, 0x18000).unwrap();
    h.p.memory.release_gpu_pin(pin).unwrap();
}

#[test]
#[ignore = "requires actual GPU partial execution and publication"]
fn runtime_service_partial_gpu_fault_publishes_prefix_and_never_replays() {
    let (mut h, _, _, channel, mut command, image) = configured();
    // Literal unsupported method at 0x3ffc AFTER the valid indexed draw.
    command.extend([0x20010fff, 0]);
    h.p.memory
        .write_bytes(DATA + 0x10000, &words(&command))
        .unwrap();
    let header = words(&[0, 0, 1, 6, 0, 0]);
    let entries = entry(0xb0000, command.len());
    h.p.memory.write_bytes(META + 0x1000, &header).unwrap();
    h.p.memory.write_bytes(META + 0x6000, &entries).unwrap();
    h.p.memory.write_bytes(META + 0x3000, &[0xcc; 24]).unwrap();
    let args = words(&[channel, 0xc018481b]);
    let wire = Wire {
        command: 11,
        args: &args,
        x: &[(META + 0x1000, 24), (META + 0x6000, 8)],
        a: &[(0, 0), (0, 0)],
        b: &[(META + 0x3000, 24)],
        c: &[(0, 0)],
        ..Wire::default()
    };
    h.p.memory.write_bytes(h.tls(), &wire.encode()).unwrap();
    let result = ipc::send_sync_request(
        &mut h.p.memory,
        h.p.kernel.as_mut().unwrap(),
        &mut h.cpu,
        h.session,
    );
    assert!(
        result
            .unwrap_err()
            .contains("accepted NVIDIA submission fault")
    );
    assert!(h.p.kernel.as_ref().unwrap().nvidia.is_faulted());
    assert_eq!(
        h.p.memory.read_bytes(META + 0x3000, 24).unwrap(),
        [0xcc; 24]
    );
    check(
        h.p.memory.read_bytes(DATA + RT as u64, SIZE).unwrap(),
        &image,
        None,
        BG,
    );
    assert!(h.p.memory.ensure_gpu_unpinned(DATA, 4096).is_err());
    let before = *h.metrics.borrow();
    assert_eq!(before.0.draw_commands, 1);
    assert_eq!(before.1.accepted, 1);
    h.p.memory.write_bytes(h.tls(), &wire.encode()).unwrap();
    let retry = ipc::send_sync_request(
        &mut h.p.memory,
        h.p.kernel.as_mut().unwrap(),
        &mut h.cpu,
        h.session,
    );
    assert!(retry.unwrap_err().contains("NVIDIA backend halted"));
    let after = *h.metrics.borrow();
    assert_eq!(after.0.draw_commands, before.0.draw_commands);
    assert_eq!(after.0.submissions, before.0.submissions);
    assert_eq!(after.1.accepted, before.1.accepted);
}

#[test]
#[ignore = "requires the actual NVIDIA runtime backend and wgpu device"]
fn runtime_service_reports_the_configured_gpu_va_regions() {
    let mut h = Harness::new(vec![0; 0x18000]);
    let fd = h.open("/dev/nvhost-as-gpu");
    let mut init = [0; 40];
    init[0] = 1;
    init[8..12].copy_from_slice(&65536u32.to_le_bytes());
    for (offset, value) in [(16, 0x4000000u64), (24, 0x2000000000), (32, 0x400000000)] {
        init[offset..offset + 8].copy_from_slice(&value.to_le_bytes());
    }
    h.ioctl(fd, 0x40284109, &init, None);
    let input = [0; 64];
    let legacy = h.ioctl(fd, 0xc0404108, &input, None);
    assert_eq!(&legacy[8..12], &48u32.to_le_bytes());
    h.p.memory.write_bytes(META + 0x1000, &input).unwrap();
    let mut args = fd.to_le_bytes().to_vec();
    args.extend(0xc0404108u32.to_le_bytes());
    h.send(&Wire {
        command: 12,
        args: &args,
        x: &[(META + 0x1000, 64)],
        a: &[(0, 0)],
        b: &[(META + 0x3000, 64), (META + 0x4000, 48)],
        c: &[(0, 0), (0, 0)],
        ..Wire::default()
    });
    assert_eq!(h.p.memory.load(h.tls() + 0x20, 4).unwrap(), 0);
    let primary = h.p.memory.read_bytes(META + 0x3000, 64).unwrap();
    let inline = h.p.memory.read_bytes(META + 0x4000, 48).unwrap();
    assert_eq!(primary, legacy);
    assert_eq!(inline, &primary[16..]);
    assert_eq!(&inline[..8], &0x4000000u64.to_le_bytes());
    assert_eq!(&inline[8..12], &4096u32.to_le_bytes());
    assert_eq!(&inline[16..24], &0x3fc000u64.to_le_bytes());
    assert_eq!(&inline[24..32], &0x400000000u64.to_le_bytes());
    assert_eq!(&inline[32..36], &65536u32.to_le_bytes());
    assert_eq!(&inline[40..48], &0x1c0000u64.to_le_bytes());
    let mut allocation = words(&[1, 4096, 0, 0]);
    allocation.extend(0u64.to_le_bytes());
    let first = h.ioctl(fd, 0xc0184102, &allocation, None);
    let second = h.ioctl(fd, 0xc0184102, &allocation, None);
    assert_eq!(&first[16..], &0x4000000u64.to_le_bytes());
    assert_eq!(&second[16..], &0x4001000u64.to_le_bytes());
    for address in [0x4000000u64, 0x4001000] {
        let mut free = address.to_le_bytes().to_vec();
        free.extend(words(&[1, 4096]));
        h.ioctl(fd, 0xc0104103, &free, None);
    }
    let nvmap = h.open("/dev/nvmap");
    let created = h.ioctl(nvmap, 0xc0080101, &words(&[4096, 0]), None);
    let handle = u32::from_le_bytes(created[4..8].try_into().unwrap());
    let mut alloc = words(&[handle, 0, 0, 0x20000, 0, 0]);
    // CPU storage is page aligned, but deliberately not 128 KiB aligned.
    alloc.extend((DATA + 4096).to_le_bytes());
    h.ioctl(nvmap, 0xc0200104, &alloc, None);
    assert!(h.p.memory.ensure_gpu_unpinned(DATA + 4096, 4096).is_err());
    for (param, expected) in [(1, 4096u32), (2, 0x20000), (4, 0x40000000), (5, 0)] {
        let bytes = h.ioctl(nvmap, 0xc00c0109, &words(&[handle, param, 0]), None);
        assert_eq!(&bytes[8..], &expected.to_le_bytes());
    }
    h.close(nvmap);
    h.p.memory.ensure_gpu_unpinned(DATA + 4096, 4096).unwrap();
    assert_eq!(h.metrics.borrow().0.submissions, 0);
    h.close(fd);
}

#[test]
#[ignore = "requires the actual NVIDIA runtime backend and wgpu device"]
fn runtime_service_ioctl3_characteristics_publishes_two_checked_outputs() {
    let mut h = Harness::new(vec![0; 0x18000]);
    let fd = h.open("/dev/nvhost-ctrl-gpu");
    let mut input = [0xa5; 176];
    input[..8].copy_from_slice(&160u64.to_le_bytes());
    input[8..16].fill(0);
    h.p.memory.write_bytes(META + 0x1000, &input).unwrap();
    let mut args = fd.to_le_bytes().to_vec();
    args.extend(0xc0b04705u32.to_le_bytes());
    // Literal libnx auto-select descriptors: one input, two outputs.
    let send = |h: &mut Harness, second: u64| {
        h.send(&Wire {
            command: 12,
            args: &args,
            x: &[(META + 0x1000, 176)],
            a: &[(0, 0)],
            b: &[(META + 0x3000, 176), (second, 160)],
            c: &[(0, 0), (0, 0)],
            ..Wire::default()
        });
    };
    send(&mut h, META + 0x4000);
    assert_eq!(h.p.memory.load(h.tls() + 0x20, 4).unwrap(), 0);
    let header = h.p.memory.read_bytes(META + 0x3000, 176).unwrap().to_vec();
    let inline = h.p.memory.read_bytes(META + 0x4000, 160).unwrap();
    assert_eq!(inline, &header[16..]);
    assert_eq!(&header[..8], &160u64.to_le_bytes());
    assert_eq!(&header[8..16], &[0; 8]);
    assert_eq!(&inline[..4], &[0x20, 1, 0, 0]);
    assert_eq!(&inline[92..96], &[0x97, 0xb1, 0, 0]);
    assert_eq!(&inline[80..88], &[0; 8]);
    assert_eq!(&inline[144..152], b"gm20b\0\0\0");
    h.p.memory.write_bytes(META + 0x3000, &[0xcc; 176]).unwrap();
    // Overlapping destinations fail before the host call or either write.
    send(&mut h, META + 0x3008);
    assert_eq!(h.p.memory.load(h.tls() + 0x20, 4).unwrap(), 4);
    assert_eq!(
        h.p.memory.read_bytes(META + 0x3000, 176).unwrap(),
        &[0xcc; 176]
    );
    send(&mut h, META + 0x4000);
    assert_eq!(h.p.memory.load(h.tls() + 0x20, 4).unwrap(), 0);
    let counters = *h.metrics.borrow();
    assert_eq!(counters.0.draw_commands, 0);
    assert_eq!(counters.0.submissions, 0);
    assert_eq!(counters.0.readback_operations, 0);
    h.close(fd);
}

#[test]
#[ignore = "requires the actual NVIDIA runtime backend and wgpu device"]
fn runtime_service_ioctl3_tpc_masks_publish_the_one_gpc_mask() {
    let mut h = Harness::new(vec![0; 0x18000]);
    let fd = h.open("/dev/nvhost-ctrl-gpu");
    let mut input = [0u8; 24];
    input[0] = 8;
    h.p.memory.write_bytes(META + 0x1000, &input).unwrap();
    let mut args = fd.to_le_bytes().to_vec();
    args.extend(0xc0184706u32.to_le_bytes());
    for extent in [4u64, 8] {
        h.send(&Wire {
            command: 12,
            args: &args,
            x: &[(META + 0x1000, 24)],
            a: &[(0, 0)],
            b: &[(META + 0x3000, 24), (META + 0x4000, extent)],
            c: &[(0, 0), (0, 0)],
            ..Wire::default()
        });
        assert_eq!(h.p.memory.load(h.tls() + 0x20, 4).unwrap(), 0);
        assert_eq!(
            h.p.memory.read_bytes(META + 0x3000, 16).unwrap(),
            &input[..16]
        );
        assert_eq!(
            h.p.memory.read_bytes(META + 0x3010, 8).unwrap(),
            &[3, 0, 0, 0, 0, 0, 0, 0]
        );
        assert_eq!(h.p.memory.load(META + 0x4000, 4).unwrap(), 3);
        if extent == 8 {
            assert_eq!(h.p.memory.load(META + 0x4004, 4).unwrap(), 0);
        }
    }
    let counters = *h.metrics.borrow();
    assert_eq!(counters.0.submissions, 0);
    assert_eq!(counters.0.draw_commands, 0);
    assert_eq!(counters.0.readback_operations, 0);
    h.close(fd);
}

#[test]
#[ignore = "executes the actual NVIDIA runtime adapter and GPU mapping controller"]
fn runtime_automatic_small_buffer_mapping_uses_checked_backing_and_releases_va() {
    let mut h = Harness::new(vec![0x5a; 4096]);
    let nvmap = h.open("/dev/nvmap");
    let space = h.open("/dev/nvhost-as-gpu");
    let mut init = words(&[1, 0, 65536, 0]);
    for value in [0x4000000u64, 0x2000000000, 0x400000000] {
        init.extend(value.to_le_bytes());
    }
    h.ioctl(space, 0x40284109, &init, None);
    let created = h.ioctl(nvmap, 0xc0080101, &words(&[4096, 0]), None);
    let handle = u32::from_le_bytes(created[4..].try_into().unwrap());
    let mut alloc = words(&[handle, 0, 0, 131072, 0, 0]);
    alloc.extend(DATA.to_le_bytes());
    h.ioctl(nvmap, 0xc0200104, &alloc, None);
    // Same literal wire shape as libnx nvAddressSpaceMap: no fixed address,
    // configured 64 KiB hint, zero size means the entire 4 KiB allocation.
    let mut input = words(&[0, 0, handle, 65536]);
    input.extend([0; 24]);
    for (offset, bytes, error) in [
        (0, 2u64.to_le_bytes().to_vec(), 2),
        (8, u32::MAX.to_le_bytes().to_vec(), 4),
        (16, 4096u64.to_le_bytes().to_vec(), 10),
        (24, 8192u64.to_le_bytes().to_vec(), 10),
    ] {
        let mut invalid = input.clone();
        invalid[offset..offset + bytes.len()].copy_from_slice(&bytes);
        h.p.memory.write_bytes(META + 0x1000, &invalid).unwrap();
        h.p.memory.write_bytes(META + 0x3000, &[0xcc; 40]).unwrap();
        let mut args = space.to_le_bytes().to_vec();
        args.extend(0xc0284106u32.to_le_bytes());
        h.send(&Wire {
            command: 1,
            args: &args,
            x: &[(META + 0x1000, 40)],
            a: &[(0, 0)],
            b: &[(META + 0x3000, 40)],
            c: &[(0, 0)],
            ..Wire::default()
        });
        assert_eq!(h.p.memory.load(h.tls() + 0x20, 4).unwrap(), error);
        assert_eq!(
            h.p.memory.read_bytes(META + 0x3000, 40).unwrap(),
            &[0xcc; 40]
        );
    }
    let mut addresses = [0; 2];
    for (i, address) in addresses.iter_mut().enumerate() {
        // Independent pitch and generic-uncompressed aliases of the same
        // allocation. Neither mapping may convert or overwrite backing bytes.
        input[4] = if i == 0 { 0 } else { 0xfe };
        let output = h.ioctl(space, 0xc0284106, &input, None);
        *address = u64::from_le_bytes(output[32..].try_into().unwrap());
        assert_eq!(*address, 0x4000000 + i as u64 * 65536);
        assert_eq!(&output[..32], &input[..32]);
    }
    // Unmap owns the automatically allocated VA; no fabricated FreeSpace is
    // required. A subsequent mapping can reuse that hole safely.
    h.ioctl(space, 0xc0084105, &addresses[0].to_le_bytes(), None);
    let output = h.ioctl(space, 0xc0284106, &input, None);
    assert_eq!(&output[32..], &addresses[0].to_le_bytes());
    for address in addresses {
        h.ioctl(space, 0xc0084105, &address.to_le_bytes(), None);
    }
    assert_eq!(h.p.memory.read_bytes(DATA, 4096).unwrap(), &[0x5a; 4096]);
    assert!(h.p.memory.ensure_gpu_unpinned(DATA, 4096).is_err());
    h.close(space);
    h.close(nvmap);
    h.p.memory.ensure_gpu_unpinned(DATA, 4096).unwrap();
    let counters = *h.metrics.borrow();
    assert_eq!(counters.0.draw_commands, 0);
    assert_eq!(counters.0.submissions, 0);
    assert_eq!(counters.0.readback_operations, 0);
}
