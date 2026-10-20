//! Literal client HIPC/CMIF transport tests, not a GPU execution claim.
//! Wire equations independently follow libnx feebd026ca0f5dcc2119f46ad8e0d16ad3dd4973
//! nx/include/switch/{sf/cmif.h,sf/hipc.h}, nx/source/services/nv.c.
use std::sync::{Arc, Mutex};
use wl_runtime::{Cpu, Memory, Process, horizon::*, ipc, nvidia, services::HostObject};

const CODE: u64 = 0x0800_0000;
const DATA: u64 = 0x1000_0000;
const TRANSFER: u64 = 0x2000_0000;
const MARKER: u64 = DATA + 0x18000;
const INPUT: u64 = DATA + 0x100;
const EXTRA: u64 = DATA + 0x1000;
const OUTPUT: u64 = DATA + 0x200;
const EXTRA_OUT: u64 = DATA + 0x300;
const FD: u32 = 0x41;
const RW8: u32 = 0xc008_4801;

type Buffer = (u64, u64);
#[derive(Clone, Debug, PartialEq, Eq)]
enum Call {
    Initialize(nvidia::TransferBuffer),
    Pid(u64, u64),
    Open(String),
    Close(u32),
    Ioctl {
        fd: u32,
        request: u32,
        kind: nvidia::IoctlKind,
        input: Vec<u8>,
        extra: Vec<u8>,
        out_len: usize,
        extra_out_len: usize,
    },
}
#[derive(Debug, Default)]
struct Observed {
    calls: Vec<Call>,
    fail_ioctl: bool,
    fatal_ioctl: bool,
}
#[derive(Debug)]
struct Host(Arc<Mutex<Observed>>);
impl nvidia::Host for Host {
    fn initialize(&mut self, transfer: nvidia::TransferBuffer) -> nvidia::HostResult<()> {
        self.0
            .lock()
            .unwrap()
            .calls
            .push(Call::Initialize(transfer));
        Ok(())
    }
    fn set_client_pid(&mut self, aruid: u64, pid: u64) -> nvidia::HostResult<()> {
        self.0.lock().unwrap().calls.push(Call::Pid(aruid, pid));
        Ok(())
    }
    fn open(&mut self, path: &str) -> nvidia::HostResult<u32> {
        self.0.lock().unwrap().calls.push(Call::Open(path.into()));
        Ok(FD)
    }
    fn close(&mut self, fd: u32, memory: &mut Memory) -> nvidia::HostResult<()> {
        self.0.lock().unwrap().calls.push(Call::Close(fd));
        memory.write_bytes(MARKER, b"CLOS").unwrap();
        Ok(())
    }
    fn ioctl(&mut self, call: nvidia::Ioctl<'_>, memory: &mut Memory) -> nvidia::HostResult<()> {
        let (fail, fatal) = {
            let mut seen = self.0.lock().unwrap();
            seen.calls.push(Call::Ioctl {
                fd: call.fd,
                request: call.request,
                kind: call.kind,
                input: call.input.to_vec(),
                extra: call.extra_input.to_vec(),
                out_len: call.output.len(),
                extra_out_len: call.extra_output.len(),
            });
            (seen.fail_ioctl, seen.fatal_ioctl)
        };
        // Proves callback receives the actual process image, not a scratch clone.
        memory.write_bytes(MARKER, b"IOCT").unwrap();
        for (i, byte) in call.output.iter_mut().enumerate() {
            *byte = call.input.get(i).copied().unwrap_or(0x17) ^ 0x5a;
        }
        for (i, byte) in call.extra_output.iter_mut().enumerate() {
            *byte = 0xa0 + i as u8;
        }
        if fatal {
            Err(nvidia::HostError::Fatal(
                "accepted output publication failed".into(),
            ))
        } else if fail {
            Err(nvidia::NvError::IoctlFailed.into())
        } else {
            Ok(())
        }
    }
}

struct Harness {
    process: Process,
    cpu: Cpu,
    session: u32,
    observed: Arc<Mutex<Observed>>,
}
impl Harness {
    fn new() -> Self {
        let mut memory = Memory::default();
        memory
            .map_state(CODE, vec![0; 0x1000], true, false, true, state::CODE)
            .unwrap();
        memory
            .map_state(
                CODE + 0x1000,
                vec![0; 0x3000],
                true,
                true,
                false,
                state::CODE_DATA,
            )
            .unwrap();
        memory
            .map_state(DATA, vec![0; 0x20000], true, true, false, state::CODE_DATA)
            .unwrap();
        memory
            .map_state(
                TRANSFER,
                vec![0; 0x2000],
                true,
                true,
                false,
                state::CODE_DATA,
            )
            .unwrap();
        let (mut process, cpu) = Process::horizon(
            memory,
            CODE,
            Range {
                start: CODE,
                end: CODE + 0x4000,
            },
            ProcessConfig {
                program_id: 0x0100_0000_0000_2000,
                process_id: 0x51,
                main_thread_id: 0x72,
                main_thread_stack_size: 0x10_0000,
                main_thread_priority: 44,
                ideal_core: 0,
                core_mask: 3,
                priority_mask: u64::MAX,
                memory_limit: 0x2000_0000,
                handle_table_size: 64,
                thread_limit: 8,
                is_application: true,
                svc_permissions: [u64::MAX; 3],
                service_access: &[(false, "*")],
            },
        )
        .unwrap();
        let observed = Arc::new(Mutex::new(Observed::default()));
        let kernel = process.kernel.as_mut().unwrap();
        kernel
            .nvidia
            .install(Box::new(Host(observed.clone())))
            .unwrap();
        let index = kernel.sessions.len();
        kernel.sessions.push(ipc::Session {
            endpoint: ipc::Endpoint::Host(HostObject::NvDrvServices),
            domain: None,
            closed: false,
            references: 1,
        });
        let session = kernel.handles.add(Object::Session(index)).unwrap();
        Self {
            process,
            cpu,
            session,
            observed,
        }
    }
    fn tls(&self) -> u64 {
        let k = self.process.kernel.as_ref().unwrap();
        k.threads[k.current].tls
    }
    fn count(&self) -> usize {
        self.observed.lock().unwrap().calls.len()
    }
    fn write(&mut self, at: u64, bytes: &[u8]) {
        self.process.memory.write_bytes(at, bytes).unwrap();
    }
    fn read(&self, at: u64, size: usize) -> Vec<u8> {
        let mut bytes = vec![0; size];
        self.process.memory.read_into(at, &mut bytes).unwrap();
        bytes
    }
    fn stage(&mut self, wire: &Wire<'_>) {
        self.write(self.tls(), &wire.encode());
    }
    fn invoke(&mut self) -> Result<(), String> {
        let stop = ipc::send_sync_request(
            &mut self.process.memory,
            self.process.kernel.as_mut().unwrap(),
            &mut self.cpu,
            self.session,
        )?;
        assert!(stop.is_none());
        Ok(())
    }
    fn send(&mut self, wire: &Wire<'_>) {
        self.stage(wire);
        self.invoke().unwrap();
        assert_eq!(self.cpu.x[0], u64::from(result::SUCCESS));
        assert_eq!(&self.read(self.tls() + 0x10, 4), b"SFCO");
        assert_eq!(
            self.scalar(0x18),
            0,
            "CMIF success is distinct from NvError"
        );
    }
    fn scalar(&self, at: u64) -> u32 {
        u32::from_le_bytes(self.read(self.tls() + at, 4).try_into().unwrap())
    }
    fn initialize(&mut self) -> u32 {
        let tm = self
            .process
            .kernel
            .as_mut()
            .unwrap()
            .create_transfer_memory(TRANSFER, 0x2000, 0)
            .unwrap();
        self.send(&Wire {
            command: 3,
            args: &0x2000u32.to_le_bytes(),
            copies: &[CURRENT_PROCESS, tm],
            ..Wire::default()
        });
        assert!(
            self.process
                .kernel
                .as_ref()
                .unwrap()
                .nvidia
                .is_initialized()
        );
        tm
    }
    fn inputs(&mut self) {
        self.write(INPUT, &[0x03, 0xf1, 0x87, 0x00, 0x5a, 0xaa, 0x41, 0x7e]);
        self.write(EXTRA, &[0x81, 0x23, 0xee, 0x42]);
        self.write(OUTPUT, &[0xcc; 8]);
        self.write(EXTRA_OUT, &[0xdd; 4]);
    }
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
fn ioctl_args(request: u32) -> [u8; 8] {
    let mut bytes = [0; 8];
    bytes[..4].copy_from_slice(&FD.to_le_bytes());
    bytes[4..].copy_from_slice(&request.to_le_bytes());
    bytes
}
fn expected_output() -> [u8; 8] {
    [0x59, 0xab, 0xdd, 0x5a, 0x00, 0xf0, 0x1b, 0x24]
}

#[test]
fn initialize_open_pid_and_close_reach_installed_host() {
    let mut h = Harness::new();
    let tm = h.initialize();
    assert!(matches!(
        h.process.kernel.as_ref().unwrap().handles.get(tm),
        Some(Object::TransferMemory(_))
    ));
    h.write(INPUT, b"/dev/nvmap");
    h.send(&Wire {
        command: 0,
        a: &[(INPUT, 10)],
        ..Wire::default()
    });
    assert_eq!(h.scalar(0x20), FD);
    assert_ne!(h.scalar(0x20), 1);
    assert_eq!(h.scalar(0x24), 0);
    h.send(&Wire {
        command: 8,
        args: &0x1122_3344_5566_7788u64.to_le_bytes(),
        pid: true,
        ..Wire::default()
    });
    h.send(&Wire {
        command: 2,
        args: &FD.to_le_bytes(),
        ..Wire::default()
    });
    assert_eq!(h.scalar(0x20), 0);
    assert_eq!(h.read(MARKER, 4), b"CLOS");
    assert_eq!(
        h.observed.lock().unwrap().calls,
        vec![
            Call::Initialize(nvidia::TransferBuffer {
                address: TRANSFER,
                size: 0x2000
            }),
            Call::Open("/dev/nvmap".into()),
            Call::Pid(0x1122_3344_5566_7788, 0x51),
            Call::Close(FD),
        ]
    );
    h.stage(&Wire {
        command: 3,
        args: &0x2000u32.to_le_bytes(),
        copies: &[CURRENT_PROCESS, tm],
        ..Wire::default()
    });
    assert!(h.invoke().unwrap_err().contains("already initialized"));
    assert_eq!(h.count(), 4);
}

#[test]
fn ioctl_autoselect_pointer_and_map_alias_alternatives_preserve_raw_bytes() {
    let mut h = Harness::new();
    h.initialize();
    h.inputs();
    for pointer in [true, false] {
        let (x, a) = if pointer {
            ((INPUT, 8), (0, 0))
        } else {
            ((0, 0), (INPUT, 8))
        };
        let (b, c) = if pointer {
            ((0, 0), (OUTPUT, 8))
        } else {
            ((OUTPUT, 8), (0, 0))
        };
        h.send(&Wire {
            command: 1,
            args: &ioctl_args(RW8),
            x: &[x],
            a: &[a],
            b: &[b],
            c: &[c],
            ..Wire::default()
        });
        assert_eq!(h.scalar(0x20), 0);
        assert_eq!(h.read(OUTPUT, 8), expected_output());
        assert_eq!(h.read(MARKER, 4), b"IOCT");
        assert_eq!(
            h.observed.lock().unwrap().calls.last().unwrap(),
            &Call::Ioctl {
                fd: FD,
                request: RW8,
                kind: nvidia::IoctlKind::Ioctl,
                input: h.read(INPUT, 8),
                extra: vec![],
                out_len: 8,
                extra_out_len: 0,
            }
        );
    }
}

#[test]
fn ioctl2_uses_logical_x_a_pairs_in_both_mixed_orders() {
    let mut h = Harness::new();
    h.initialize();
    h.inputs();
    for reverse in [false, true] {
        let (x, a) = if reverse {
            ([(0, 0), (EXTRA, 4)], [(INPUT, 8), (0, 0)])
        } else {
            ([(INPUT, 8), (0, 0)], [(0, 0), (EXTRA, 4)])
        };
        h.send(&Wire {
            command: 11,
            args: &ioctl_args(RW8),
            x: &x,
            a: &a,
            b: &[(OUTPUT, 8)],
            c: &[(0, 0)],
            ..Wire::default()
        });
        assert_eq!(h.scalar(0x20), 0);
        assert_eq!(h.read(OUTPUT, 8), expected_output());
        assert_eq!(
            h.observed.lock().unwrap().calls.last().unwrap(),
            &Call::Ioctl {
                fd: FD,
                request: RW8,
                kind: nvidia::IoctlKind::Ioctl2,
                input: h.read(INPUT, 8),
                extra: h.read(EXTRA, 4),
                out_len: 8,
                extra_out_len: 0,
            }
        );
    }
}

#[test]
fn ioctl3_pairs_primary_and_extra_outputs_by_logical_index() {
    let mut h = Harness::new();
    h.initialize();
    h.inputs();
    for reverse in [false, true] {
        let (b, c) = if reverse {
            ([(OUTPUT, 8), (0, 0)], [(0, 0), (EXTRA_OUT, 4)])
        } else {
            ([(0, 0), (EXTRA_OUT, 4)], [(OUTPUT, 8), (0, 0)])
        };
        h.send(&Wire {
            command: 12,
            args: &ioctl_args(RW8),
            x: &[(0, 0)],
            a: &[(INPUT, 8)],
            b: &b,
            c: &c,
            ..Wire::default()
        });
        assert_eq!(h.scalar(0x20), 0);
        assert_eq!(h.read(OUTPUT, 8), expected_output());
        assert_eq!(h.read(EXTRA_OUT, 4), [0xa0, 0xa1, 0xa2, 0xa3]);
        assert_eq!(
            h.observed.lock().unwrap().calls.last().unwrap(),
            &Call::Ioctl {
                fd: FD,
                request: RW8,
                kind: nvidia::IoctlKind::Ioctl3,
                input: h.read(INPUT, 8),
                extra: vec![],
                out_len: 8,
                extra_out_len: 4,
            }
        );
    }
}

#[test]
fn invalid_secondary_destination_preflights_before_host_or_primary_publication() {
    let mut h = Harness::new();
    h.initialize();
    h.inputs();
    let before = h.count();
    for destination in [CODE, 0x3000_0000] {
        h.send(&Wire {
            command: 12,
            args: &ioctl_args(RW8),
            x: &[(INPUT, 8)],
            a: &[(0, 0)],
            b: &[(OUTPUT, 8), (destination, 4)],
            c: &[(0, 0), (0, 0)],
            ..Wire::default()
        });
        assert_eq!(h.scalar(0x20), nvidia::NvError::InvalidAddress as u32);
        assert_eq!(h.count(), before);
        assert_eq!(h.read(OUTPUT, 8), [0xcc; 8]);
        assert_eq!(h.read(MARKER, 4), [0; 4]);
    }
}

#[test]
fn read_only_reply_tls_stops_before_any_host_callback() {
    let mut h = Harness::new();
    h.initialize();
    h.write(INPUT, b"/dev/nvmap");
    let before = h.count();
    h.stage(&Wire {
        command: 0,
        a: &[(INPUT, 10)],
        ..Wire::default()
    });
    let tls = h.tls();
    let original = h.read(tls, 0x100);
    let index = h
        .process
        .memory
        .regions
        .iter()
        .position(|r| r.base <= tls && tls + 0x100 <= r.end())
        .unwrap();
    h.process.memory.regions[index].write = false;
    h.invoke().unwrap();
    assert_eq!(h.cpu.x[0], u64::from(result::INVALID_CURRENT_MEMORY));
    assert_eq!(h.count(), before);
    assert_eq!(h.read(tls, 0x100), original);
}

#[test]
fn host_error_does_not_publish_owned_outputs_or_fake_rollback_explicit_memory_side_effects() {
    let mut h = Harness::new();
    h.initialize();
    h.inputs();
    h.observed.lock().unwrap().fail_ioctl = true;
    h.send(&Wire {
        command: 12,
        args: &ioctl_args(RW8),
        x: &[(INPUT, 8)],
        a: &[(0, 0)],
        b: &[(OUTPUT, 8), (EXTRA_OUT, 4)],
        c: &[(0, 0), (0, 0)],
        ..Wire::default()
    });
    assert_eq!(h.scalar(0x20), nvidia::NvError::IoctlFailed as u32);
    assert_eq!(h.read(OUTPUT, 8), [0xcc; 8]);
    assert_eq!(h.read(EXTRA_OUT, 4), [0xdd; 4]);
    assert_eq!(h.read(MARKER, 4), b"IOCT");
    assert_eq!(h.count(), 2);
}

#[test]
fn uninitialized_and_unsupported_commands_do_not_invent_fds_events_or_completion() {
    let mut h = Harness::new();
    h.write(INPUT, b"/dev/nvmap");
    h.send(&Wire {
        command: 0,
        a: &[(INPUT, 10)],
        ..Wire::default()
    });
    assert_eq!(h.scalar(0x20), u32::MAX);
    assert_eq!(h.scalar(0x24), nvidia::NvError::NotInitialized as u32);
    h.inputs();
    h.send(&Wire {
        command: 1,
        args: &ioctl_args(RW8),
        x: &[(INPUT, 8)],
        a: &[(0, 0)],
        b: &[(OUTPUT, 8)],
        c: &[(0, 0)],
        ..Wire::default()
    });
    assert_eq!(h.scalar(0x20), nvidia::NvError::NotInitialized as u32);
    let events = h.process.kernel.as_ref().unwrap().events.len();
    h.send(&Wire {
        command: 4,
        args: &[0; 8],
        ..Wire::default()
    });
    assert_eq!(h.scalar(0x20), nvidia::NvError::NotSupported as u32);
    assert_eq!(
        h.scalar(4) >> 31,
        0,
        "QueryEvent must not return a copied handle"
    );
    assert_eq!(h.process.kernel.as_ref().unwrap().events.len(), events);
    h.stage(&Wire {
        command: 0xdead,
        ..Wire::default()
    });
    assert!(
        h.invoke()
            .unwrap_err()
            .contains("unsupported NVIDIA service command")
    );
    assert_eq!(h.count(), 0);
}

#[test]
fn aggregate_metadata_limit_is_checked_before_dereferencing_unmapped_extra_input() {
    let mut h = Harness::new();
    h.initialize();
    h.inputs();
    let before = h.count();
    h.send(&Wire {
        command: 11,
        args: &ioctl_args(RW8),
        x: &[(INPUT, 8), (0, 0)],
        a: &[(0, 0), (0x3000_0000, nvidia::MAX_METADATA_BYTES as u64)],
        b: &[(OUTPUT, 8)],
        c: &[(0, 0)],
        ..Wire::default()
    });
    assert_eq!(h.scalar(0x20), nvidia::NvError::InvalidSize as u32);
    assert_eq!(h.count(), before);
    assert_eq!(h.read(MARKER, 4), [0; 4]);
    assert_eq!(h.read(OUTPUT, 8), [0xcc; 8]);
    // Exact inclusive budget: 8 primary input + 65520 extra + 8 output = 65536.
    h.send(&Wire {
        command: 11,
        args: &ioctl_args(RW8),
        x: &[(INPUT, 8), (0, 0)],
        a: &[(0, 0), (EXTRA, (nvidia::MAX_METADATA_BYTES - 16) as u64)],
        b: &[(OUTPUT, 8)],
        c: &[(0, 0)],
        ..Wire::default()
    });
    assert_eq!(h.scalar(0x20), 0);
    assert_eq!(h.count(), before + 1);
}

#[test]
fn contradictory_autoselect_buffers_and_mismatched_ioctl_sizes_do_not_reach_host() {
    let mut h = Harness::new();
    h.initialize();
    h.inputs();
    let before = h.count();
    h.send(&Wire {
        command: 1,
        args: &ioctl_args(RW8),
        x: &[(INPUT, 8)],
        a: &[(INPUT, 8)],
        b: &[(OUTPUT, 8)],
        c: &[(0, 0)],
        ..Wire::default()
    });
    assert_eq!(h.scalar(0x20), nvidia::NvError::CountMismatch as u32);
    assert_eq!(h.count(), before);
    h.send(&Wire {
        command: 1,
        args: &ioctl_args(RW8),
        x: &[(INPUT, 7)],
        a: &[(0, 0)],
        b: &[(OUTPUT, 8)],
        c: &[(0, 0)],
        ..Wire::default()
    });
    assert_eq!(h.scalar(0x20), nvidia::NvError::InvalidSize as u32);
    assert_eq!(h.count(), before);
    assert_eq!(h.read(OUTPUT, 8), [0xcc; 8]);
}

#[test]
fn invalid_copied_transfer_handle_never_reaches_initialize() {
    let mut h = Harness::new();
    h.stage(&Wire {
        command: 3,
        args: &0x1000u32.to_le_bytes(),
        copies: &[CURRENT_PROCESS, 0x1234],
        ..Wire::default()
    });
    h.invoke().unwrap();
    assert_eq!(h.cpu.x[0], u64::from(result::INVALID_HANDLE));
    assert_eq!(h.count(), 0);
    assert!(!h.process.kernel.as_ref().unwrap().nvidia.is_initialized());
}

#[test]
fn output_overlap_with_other_output_or_tls_is_rejected_before_callback() {
    let mut h = Harness::new();
    h.initialize();
    h.inputs();
    let before = h.count();
    let tls = h.tls();
    for outputs in [
        [(OUTPUT, 8), (OUTPUT + 4, 4)],
        [(tls + 0x80, 8), (EXTRA_OUT, 4)],
        [(OUTPUT, 8), (tls + 0x90, 4)],
    ] {
        h.send(&Wire {
            command: 12,
            args: &ioctl_args(RW8),
            x: &[(INPUT, 8)],
            a: &[(0, 0)],
            b: &outputs,
            c: &[(0, 0), (0, 0)],
            ..Wire::default()
        });
        assert_eq!(h.scalar(0x20), nvidia::NvError::BadParameter as u32);
        assert_eq!(h.count(), before);
        assert_eq!(h.read(MARKER, 4), [0; 4]);
        assert_eq!(h.read(OUTPUT, 8), [0xcc; 8]);
        assert_eq!(h.read(EXTRA_OUT, 4), [0xdd; 4]);
    }
}

#[test]
fn simple_commands_reject_nonzero_receive_static_mode_before_host() {
    for command in [0, 2, 3, 8] {
        let mut h = Harness::new();
        let tm = if command == 3 {
            h.process
                .kernel
                .as_mut()
                .unwrap()
                .create_transfer_memory(TRANSFER, 0x2000, 0)
                .unwrap()
        } else {
            h.initialize()
        };
        h.write(INPUT, b"/dev/nvmap");
        let before = h.count();
        let initialize_size = 0x2000u32.to_le_bytes();
        let fd = FD.to_le_bytes();
        let aruid = 0x1234u64.to_le_bytes();
        let handles = [CURRENT_PROCESS, tm];
        let wire = match command {
            0 => Wire {
                command,
                a: &[(INPUT, 10)],
                ..Wire::default()
            },
            2 => Wire {
                command,
                args: &fd,
                ..Wire::default()
            },
            3 => Wire {
                command,
                args: &initialize_size,
                copies: &handles,
                ..Wire::default()
            },
            8 => Wire {
                command,
                args: &aruid,
                pid: true,
                ..Wire::default()
            },
            _ => unreachable!(),
        };
        let mut message = wire.encode();
        // Mode 1 is a genuine HIPC receive-list mode, but not a supported
        // descriptor-free form of these four bounded NVIDIA commands.
        let header = u32::from_le_bytes(message[4..8].try_into().unwrap());
        put32(&mut message, 4, header | (1 << 10));
        h.write(h.tls(), &message);
        if command == 0 || command == 2 {
            h.invoke().unwrap();
            assert_eq!(
                h.scalar(if command == 0 { 0x24 } else { 0x20 }),
                nvidia::NvError::BadParameter as u32
            );
        } else {
            assert!(h.invoke().is_err());
        }
        assert_eq!(h.count(), before);
    }
}

#[test]
fn initialize_rejects_unmapped_or_inaccessible_transfer_spans() {
    for case in 0..6 {
        let mut h = Harness::new();
        let address = if case == 0 { 0x3000_0000 } else { TRANSFER };
        if case == 1 || case == 2 {
            let region = h
                .process
                .memory
                .regions
                .iter_mut()
                .find(|r| r.base == TRANSFER)
                .unwrap();
            if case == 1 {
                region.read = false;
            } else {
                region.write = false;
            }
        }
        if case >= 3 {
            assert!(h.process.memory.split_at(TRANSFER + 0x1000));
            let regions = &mut h.process.memory.regions;
            let second = regions
                .iter()
                .position(|r| r.base == TRANSFER + 0x1000)
                .unwrap();
            match case {
                3 => regions[second].read = false,
                4 => regions[second].write = false,
                5 => {
                    regions.remove(second);
                }
                _ => unreachable!(),
            }
        }
        let tm = h
            .process
            .kernel
            .as_mut()
            .unwrap()
            .create_transfer_memory(address, 0x2000, 0)
            .unwrap();
        h.stage(&Wire {
            command: 3,
            args: &0x2000u32.to_le_bytes(),
            copies: &[CURRENT_PROCESS, tm],
            ..Wire::default()
        });
        assert!(h.invoke().is_err());
        assert_eq!(h.count(), 0);
        assert!(!h.process.kernel.as_ref().unwrap().nvidia.is_initialized());
    }
}

#[test]
fn initialize_accepts_adjacent_regions_without_changing_transfer_contents() {
    let mut h = Harness::new();
    let contents: Vec<u8> = (0..0x2000).map(|i| (i * 17 + 3) as u8).collect();
    h.write(TRANSFER, &contents);
    assert!(h.process.memory.split_at(TRANSFER + 0x1000));
    h.initialize();
    assert_eq!(h.count(), 1);
    assert_eq!(
        h.observed.lock().unwrap().calls,
        [Call::Initialize(nvidia::TransferBuffer {
            address: TRANSFER,
            size: 0x2000
        })]
    );
    assert_eq!(h.read(TRANSFER, 0x2000), contents);
}

#[test]
fn missing_host_is_explicit_unsupported_instead_of_successful_initialize() {
    let mut h = Harness::new();
    h.process.kernel.as_mut().unwrap().nvidia = nvidia::State::default();
    let tm = h
        .process
        .kernel
        .as_mut()
        .unwrap()
        .create_transfer_memory(TRANSFER, 0x2000, 0)
        .unwrap();
    h.stage(&Wire {
        command: 3,
        args: &0x2000u32.to_le_bytes(),
        copies: &[CURRENT_PROCESS, tm],
        ..Wire::default()
    });
    assert!(
        h.invoke()
            .unwrap_err()
            .contains("no host backend installed")
    );
    assert_eq!(h.count(), 0);
    assert!(!h.process.kernel.as_ref().unwrap().nvidia.is_initialized());
}

#[test]
fn metadata_can_span_adjacent_accessible_regions_without_native_scalar_reinterpretation() {
    let mut h = Harness::new();
    h.initialize();
    h.inputs();
    assert!(h.process.memory.split_at(INPUT + 4));
    assert!(h.process.memory.split_at(OUTPUT + 4));
    h.send(&Wire {
        command: 1,
        args: &ioctl_args(RW8),
        x: &[(0, 0)],
        a: &[(INPUT, 8)],
        b: &[(OUTPUT, 8)],
        c: &[(0, 0)],
        ..Wire::default()
    });
    assert_eq!(h.scalar(0x20), 0);
    assert_eq!(h.read(OUTPUT, 8), expected_output());
    assert_eq!(h.read(MARKER, 4), b"IOCT");
    assert_eq!(h.count(), 2);
}

#[test]
fn ioctl_direction_bits_select_empty_or_sized_metadata_exactly_like_libnx() {
    let mut h = Harness::new();
    h.initialize();
    for direction in 0..4u32 {
        h.inputs();
        let request = (direction << 30) | 0x0008_4801;
        let input = if direction & 1 != 0 {
            (INPUT, 8)
        } else {
            (0, 0)
        };
        let output = if direction & 2 != 0 {
            (OUTPUT, 8)
        } else {
            (0, 0)
        };
        h.send(&Wire {
            command: 1,
            args: &ioctl_args(request),
            x: &[input],
            a: &[(0, 0)],
            b: &[output],
            c: &[(0, 0)],
            ..Wire::default()
        });
        assert_eq!(h.scalar(0x20), 0);
        let expected_input = if direction & 1 != 0 {
            h.read(INPUT, 8)
        } else {
            vec![]
        };
        assert_eq!(
            h.observed.lock().unwrap().calls.last().unwrap(),
            &Call::Ioctl {
                fd: FD,
                request,
                kind: nvidia::IoctlKind::Ioctl,
                input: expected_input,
                extra: vec![],
                out_len: if direction & 2 != 0 { 8 } else { 0 },
                extra_out_len: 0,
            }
        );
        let expected = match direction {
            2 => vec![0x4d; 8],
            3 => expected_output().to_vec(),
            _ => vec![0xcc; 8],
        };
        assert_eq!(h.read(OUTPUT, 8), expected);
    }
}

#[test]
fn in_place_primary_metadata_is_snapshotted_before_host_and_output_publication() {
    let mut h = Harness::new();
    h.initialize();
    h.inputs();
    let original = h.read(INPUT, 8);
    let extra = h.read(EXTRA, 4);
    h.send(&Wire {
        command: 11,
        args: &ioctl_args(RW8),
        x: &[(INPUT, 8), (0, 0)],
        a: &[(0, 0), (EXTRA, 4)],
        b: &[(INPUT, 8)],
        c: &[(0, 0)],
        ..Wire::default()
    });
    assert_eq!(h.scalar(0x20), 0);
    assert_eq!(h.read(INPUT, 8), expected_output());
    assert_eq!(
        h.observed.lock().unwrap().calls.last().unwrap(),
        &Call::Ioctl {
            fd: FD,
            request: RW8,
            kind: nvidia::IoctlKind::Ioctl2,
            input: original,
            extra,
            out_len: 8,
            extra_out_len: 0,
        }
    );
}

#[test]
fn fatal_host_failure_stops_ipc_before_reply_and_latches_against_replay() {
    let mut h = Harness::new();
    h.initialize();
    h.inputs();
    h.observed.lock().unwrap().fatal_ioctl = true;
    h.stage(&Wire {
        command: 1,
        args: &ioctl_args(RW8),
        x: &[(INPUT, 8)],
        a: &[(0, 0)],
        b: &[(OUTPUT, 8)],
        c: &[(0, 0)],
        ..Wire::default()
    });
    let message = h.read(h.tls(), 0x100);
    let before = h.count();
    assert!(
        h.invoke()
            .unwrap_err()
            .contains("accepted output publication failed")
    );
    assert_eq!(h.count(), before + 1);
    assert_eq!(
        h.read(h.tls(), 0x100),
        message,
        "no success reply after fatal work"
    );
    assert_eq!(
        h.read(MARKER, 4),
        b"IOCT",
        "accepted side effect is not rolled back"
    );
    assert_eq!(h.read(OUTPUT, 8), [0xcc; 8]);
    assert!(h.process.kernel.as_ref().unwrap().nvidia.is_faulted());
    // Clearing backend injection cannot reset the process-level fault or replay.
    h.observed.lock().unwrap().fatal_ioctl = false;
    for command in [0, 1, 2, 3, 4, 8] {
        h.stage(&Wire {
            command,
            ..Wire::default()
        });
        assert!(h.invoke().unwrap_err().contains("NVIDIA backend halted"));
        assert_eq!(h.count(), before + 1);
    }
}

#[test]
fn ordinary_host_driver_error_is_wire_error_without_latching_a_fatal_fault() {
    let mut h = Harness::new();
    h.initialize();
    h.inputs();
    h.observed.lock().unwrap().fail_ioctl = true;
    let args = ioctl_args(RW8);
    let wire = Wire {
        command: 1,
        args: &args,
        x: &[(INPUT, 8)],
        a: &[(0, 0)],
        b: &[(OUTPUT, 8)],
        c: &[(0, 0)],
        ..Wire::default()
    };
    h.send(&wire);
    assert_eq!(h.scalar(0x20), nvidia::NvError::IoctlFailed as u32);
    assert!(!h.process.kernel.as_ref().unwrap().nvidia.is_faulted());
    let before = h.count();
    h.observed.lock().unwrap().fail_ioctl = false;
    h.send(&wire);
    assert_eq!(h.scalar(0x20), 0);
    assert_eq!(h.count(), before + 1);
    assert_eq!(h.read(OUTPUT, 8), expected_output());
}

#[test]
fn firmware_margin_request_honors_disabled_system_policy() {
    let mut h = Harness::new();
    h.initialize();
    let before = h.count();
    assert_eq!(
        h.process
            .kernel
            .as_ref()
            .unwrap()
            .nvidia
            .firmware_memory_margin(),
        None
    );
    for requested in [1u64, 0, 1] {
        h.send(&Wire {
            command: 13,
            args: &requested.to_le_bytes(),
            ..Wire::default()
        });
        let policy = h
            .process
            .kernel
            .as_ref()
            .unwrap()
            .nvidia
            .firmware_memory_margin()
            .unwrap();
        assert_eq!(policy.requested, requested == 1);
        assert!(
            !policy.enabled,
            "the documented system setting defaults to disabled"
        );
        assert_eq!(h.count(), before, "this policy does not submit GPU work");
    }
}

#[test]
fn firmware_margin_rejects_invalid_layout_and_preserves_prior_state() {
    for case in 0..8 {
        let mut h = Harness::new();
        h.initialize();
        h.send(&Wire {
            command: 13,
            args: &1u64.to_le_bytes(),
            ..Wire::default()
        });
        let before = h
            .process
            .kernel
            .as_ref()
            .unwrap()
            .nvidia
            .firmware_memory_margin();
        let value = if case == 0 {
            2u64
        } else if case == 1 {
            0x1_0000_0001
        } else {
            0
        };
        let copy = [CURRENT_PROCESS];
        let buffer = [(INPUT, 8)];
        let mut message = Wire {
            command: 13,
            args: &value.to_le_bytes(),
            pid: case == 2,
            copies: if case == 3 { &copy } else { &[] },
            a: if case == 4 { &buffer } else { &[] },
            b: if case == 5 { &buffer } else { &[] },
            ..Wire::default()
        }
        .encode();
        if case == 6 {
            put32(&mut message, 4, 7);
        } // Only four CMIF argument bytes.
        if case == 7 {
            let words = u32::from_le_bytes(message[4..8].try_into().unwrap());
            put32(&mut message, 4, words | (1 << 10));
        }
        h.write(h.tls(), &message);
        assert!(h.invoke().is_err(), "case {case}");
        assert_eq!(
            h.process
                .kernel
                .as_ref()
                .unwrap()
                .nvidia
                .firmware_memory_margin(),
            before
        );
        assert_eq!(h.count(), 1);
    }
    let mut h = Harness::new();
    h.stage(&Wire {
        command: 13,
        args: &1u64.to_le_bytes(),
        ..Wire::default()
    });
    assert!(h.invoke().is_err());
    assert_eq!(h.count(), 0);
    assert_eq!(
        h.process
            .kernel
            .as_ref()
            .unwrap()
            .nvidia
            .firmware_memory_margin(),
        None
    );
}
