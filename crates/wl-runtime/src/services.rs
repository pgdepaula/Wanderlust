//! System services hosted by the runtime, reached through sm like services of other
//! processes, and the CMIF server protocol they speak (Atmosphère `libstratosphere/source/sf`,
//! `include/stratosphere/sf/impl/sf_impl_command_serialization.hpp`; 0e9cd964).
//!
//! A CMIF request is a HIPC message of type 4 (5 for control requests, 6 and 7 with an inline
//! context, 2 to close) whose raw data starts at the next 16-byte boundary with the `SFCI`
//! header (magic, version, command ID, token) and the arguments. The reply has `SFCO`, version
//! 0, the result and the interface ID at the same alignment; its raw data word count includes
//! 16 bytes of alignment padding, and moved handles and output objects (sessions) follow the
//! special header. A command whose result is a failure replies with the header alone.
//!
//! Hosted services:
//! - `lm` (log manager, `libstratosphere/source/lm`): ILogService 0 OpenLogger(pid) opens an
//!   ILogger; ILogger 0 Log(InAutoSelectBuffer) receives a log packet and 1
//!   SetDestination(u32) selects where packets go. Packets carry a 0x18-byte header (process
//!   ID, thread ID, head/tail flags, severity, verbosity, payload size) and chunks of ULEB128
//!   key, ULEB128 size and data; the text chunks of a message, joined from its head packet to
//!   its tail packet, become one line of the process's debug output.
//!
//! Requests the model does not cover (domains, unknown commands, a request layout that
//! differs from the command's) are explicit faults rather than replies.
use crate::horizon::{self, result, Kernel};
use crate::ipc::{Endpoint, Request};
use crate::threads;
use crate::{ipc, Memory};

/// An object behind a session to a hosted service.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum HostObject {
    /// `lm` ILogService.
    LogService,
    /// ILogger opened by `process_id`.
    Logger {
        process_id: u64,
    },
    /// `apm` IManager (ptm process).
    ApmManager,
    /// apm ISession.
    ApmSession,
    /// `appletOE` IApplicationProxyService and the interfaces of IApplicationProxy (am
    /// process).
    AppletOe,
    ApplicationProxy,
    CommonStateGetter,
    SelfController,
    WindowController,
    AudioController,
    DisplayController,
    LibraryAppletCreator,
    ApplicationFunctions,
    DebugFunctions,
    /// `aoc:u` IAddOnContentManager (ns process).
    AocManager,
    /// `ectx:aw` IWriterForApplication and IContextRegistrar (glue process).
    EctxWriter,
    EctxRegistrar,
    /// `pctl` IParentalControlServiceFactory and IParentalControlService (pctl process).
    PctlFactory,
    PctlService,
    /// `time:u` IStaticService, clocks, and timezone (time process).
    TimeStaticService,
    TimeSystemClock,
    TimeSteadyClock,
    TimeZoneService,
    /// `acc:u0` IAccountServiceForApplication, profile, and session object (account process).
    AccUser,
    AccProfile,
    AccSession,
    /// `fsp-srv` IFileSystemProxy, IFileSystem, IStorage (fssrv process).
    FileSystemProxy,
    FileSystem,
    Storage,
    /// `nvdrv` INvDrvServices (nvservices process).
    NvDrvServices,
    /// `vi` IApplicationRootService, IApplicationDisplayService (vi process).
    ViUser,
    ViDisplayService,
    HosBinderDriver,
    /// `hid` IHidServer, IAppletResource, IActiveVibrationDeviceList (hid process).
    HidServer,
    HidAppletResource,
    ActiveVibrationDeviceList,
}

/// The process serving a hosted object; domain object IDs are allocated per server.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Server {
    Lm,
    Ptm,
    Am,
    Ns,
    Glue,
    Pctl,
    Time,
    Acc,
    Fs,
    NvDrv,
    Vi,
    Hid,
}

/// Size of `lm`'s pointer buffer (`lm_ipc_server.cpp`, `PointerBufferSize`).
const LM_POINTER_BUFFER_SIZE: u16 = 0x400;

/// Size of `am`'s pointer buffer (0x1000 bytes for HIPC pointer transfer buffers).
const AM_POINTER_BUFFER_SIZE: u16 = 0x1000;

/// Minimal valid specification-conformant empty RomFS image (header, dir hash table, root dir).
const EMPTY_ROMFS: [u8; 304] = [
    0x50, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, // header_size: 0x50
    0x50, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, // dir_hash_table_offset: 0x50
    0x04, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, // dir_hash_table_size: 4
    0x58, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, // dir_table_offset: 0x58
    0x18, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, // dir_table_size: 0x18
    0x70, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, // file_hash_table_offset: 0x70
    0x04, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, // file_hash_table_size: 4
    0x78, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, // file_table_offset: 0x78
    0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, // file_table_size: 0
    0x80, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, // file_data_offset: 0x80
    // 0x50: dir hash table (4 bytes bucket + 4 bytes padding)
    0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
    // 0x58: root dir entry (parent=0, sibling=none, child=none, file=none, hash=none, name_len=0)
    0x00, 0x00, 0x00, 0x00, // parent: 0
    0xff, 0xff, 0xff, 0xff, // sibling: none
    0xff, 0xff, 0xff, 0xff, // child: none
    0xff, 0xff, 0xff, 0xff, // file: none
    0xff, 0xff, 0xff, 0xff, // next_hash: none
    0x00, 0x00, 0x00, 0x00, // name_len: 0
    // 0x70: file hash table (4 bytes bucket + 4 bytes padding)
    0xff, 0xff, 0xff, 0xff, 0x00, 0x00, 0x00, 0x00, // 0x78..0x100: empty padding
    0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
    0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
    0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
    0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
    0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
    0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
];

/// `lm::LogDestination_TargetManager`, the destination before any SetDestination.
pub const LOG_DESTINATION_TARGET_MANAGER: u32 = 1;

/// State of the hosted services.
#[derive(Clone, Debug, Default)]
pub struct HostState {
    /// Destination set by ILogger::SetDestination; `None` before the first call.
    pub log_destination: Option<u32>,
    /// Log text of messages whose tail packet has not arrived, by thread ID.
    pub partial_logs: Vec<(u64, String)>,
    /// Log packets received.
    pub log_packets: u64,
    /// CMIF domains, indexed by `Session::domain`.
    pub domains: Vec<Domain>,
    /// Domain object IDs of each server.
    pub lm_domain_ids: DomainIds,
    pub ptm_domain_ids: DomainIds,
    /// apm PerformanceConfiguration set through ISession, for Normal and Boost.
    pub performance_configuration: [Option<u32>; 2],
    pub am_domain_ids: DomainIds,
    pub ns_domain_ids: DomainIds,
    pub glue_domain_ids: DomainIds,
    pub pctl_domain_ids: DomainIds,
    pub time_domain_ids: DomainIds,
    pub acc_domain_ids: DomainIds,
    pub fs_domain_ids: DomainIds,
    pub nvdrv_domain_ids: DomainIds,
    pub vi_domain_ids: DomainIds,
    pub hid_domain_ids: DomainIds,
    /// Cached time:u shared memory index.
    pub time_shared_memory: Option<usize>,
    /// Cached hid shared memory index.
    pub hid_shared_memory: Option<usize>,
    /// Npad style set update event handle.
    pub npad_style_set_update_event: Option<usize>,
    /// The application's AppletMessage queue and the event signalled while it is not
    /// empty (created by the first ICommonStateGetter::GetEventHandle).
    pub applet_messages: Vec<u32>,
    pub applet_message_event: Option<usize>,
    /// ISelfController's accumulated-suspended-tick event: never signalled, as the
    /// application is never suspended.
    pub suspended_tick_event: Option<usize>,
    /// IApplicationFunctions' GPU-error event: never signalled, as GPU-error detection is
    /// not modelled.
    pub gpu_error_event: Option<usize>,
    pub delay_time_to_abort_on_gpu_error: Option<u64>,
    /// IApplicationFunctions' Cmd210 event: never signalled.
    pub application_event_210: Option<usize>,
    /// nvdrv QueryEvent: never signalled.
    pub nvdrv_event: Option<usize>,
    /// vi VsyncEvent: never signalled.
    pub vi_vsync_event: Option<usize>,
    /// hosbinder BinderEvent.
    pub binder_event: Option<usize>,
    /// aoc:u AddOnContentListChangedEvent: never signalled, as DLCs are static.
    pub aoc_list_changed_event: Option<usize>,
    /// Settings recorded by ISelfController and IWindowController.
    pub operation_mode_notification: Option<bool>,
    pub performance_mode_notification: Option<bool>,
    pub focus_handling_mode: Option<[bool; 3]>,
    pub restart_message_enabled: Option<bool>,
    pub out_of_focus_suspending: Option<bool>,
    pub foreground_rights: bool,
    pub request_exit_to_library_applet_at_execute_next_program: bool,
    /// The first `CALL_TRACE_LIMIT` hosted commands: interface, command ID, result.
    pub calls: Vec<(&'static str, u32, u32)>,
}

/// Hosted commands kept in `HostState::calls` for the run report.
const CALL_TRACE_LIMIT: usize = 512;

/// `am` results (module 128).
const AM_NO_MESSAGE: u32 = 128 | 3 << 9;
/// nn::oe::FocusState InFocus: the application is the foreground applet.
const FOCUS_IN_FOCUS: u8 = 1;
/// nn::am::AppletMessage notifications.
pub const APPLET_MESSAGE_CHANGE_INTO_FOREGROUND: u32 = 1;
pub const APPLET_MESSAGE_FOCUS_STATE_CHANGED: u32 = 15;

/// The console operating mode the runtime presents: docked *(deterministic choice)*. apm
/// reports it as PerformanceMode Boost (1); handheld is Normal (0).
pub const DOCKED: bool = true;

/// apm PerformanceConfiguration values documented for retail units (SwitchBrew, PTM
/// services); 0x00010002 and 0x00020002 exist only on SDEV units.
const PERFORMANCE_CONFIGURATIONS: [u32; 14] = [
    0x0001_0000,
    0x0001_0001,
    0x0002_0000,
    0x0002_0001,
    0x0002_0003,
    0x0002_0004,
    0x0002_0005,
    0x0002_0006,
    0x9222_0007,
    0x9222_0008,
    0x9222_0009,
    0x9222_000A,
    0x9222_000B,
    0x9222_000C,
];

/// The hosted service registered under `name`.
pub fn lookup(name: &[u8; 8]) -> Option<HostObject> {
    match name {
        b"lm\0\0\0\0\0\0" => Some(HostObject::LogService),
        b"apm\0\0\0\0\0" => Some(HostObject::ApmManager),
        b"appletOE" => Some(HostObject::AppletOe),
        b"aoc:u\0\0\0" => Some(HostObject::AocManager),
        b"ectx:aw\0" => Some(HostObject::EctxWriter),
        b"pctl\0\0\0\0" | b"pctl:s\0\0" | b"pctl:r\0\0" | b"pctl:a\0\0" => {
            Some(HostObject::PctlFactory)
        }
        b"time:u\0\0" => Some(HostObject::TimeStaticService),
        b"acc:u0\0\0" => Some(HostObject::AccUser),
        b"fsp-srv\0" => Some(HostObject::FileSystemProxy),
        b"nvdrv\0\0\0" | b"nvdrv:a\0" | b"nvdrv:s\0" | b"nvdrv:t\0" => {
            Some(HostObject::NvDrvServices)
        }
        b"vi:u\0\0\0\0" | b"vi:m\0\0\0\0" | b"vi:s\0\0\0\0" => Some(HostObject::ViUser),
        b"hid\0\0\0\0\0" | b"hid:sys\0" => Some(HostObject::HidServer),
        _ => None,
    }
}

impl HostObject {
    /// Name and the interface ID written in the reply's out header: Atmosphère's for the
    /// services it implements, 0 otherwise.
    fn interface(&self) -> (&'static str, u32) {
        match self {
            HostObject::LogService => ("lm ILogService", 0xE1D7_F748),
            HostObject::Logger { .. } => ("lm ILogger", 0x3E81_DAD2),
            HostObject::ApmManager => ("apm IManager", 0),
            HostObject::ApmSession => ("apm ISession", 0),
            HostObject::AppletOe => ("appletOE IApplicationProxyService", 0),
            HostObject::ApplicationProxy => ("am IApplicationProxy", 0),
            HostObject::CommonStateGetter => ("am ICommonStateGetter", 0),
            HostObject::SelfController => ("am ISelfController", 0),
            HostObject::WindowController => ("am IWindowController", 0),
            HostObject::AudioController => ("am IAudioController", 0),
            HostObject::DisplayController => ("am IDisplayController", 0),
            HostObject::LibraryAppletCreator => ("am ILibraryAppletCreator", 0),
            HostObject::ApplicationFunctions => ("am IApplicationFunctions", 0),
            HostObject::DebugFunctions => ("am IDebugFunctions", 0),
            HostObject::AocManager => ("aoc:u IAddOnContentManager", 0),
            HostObject::EctxWriter => ("ectx:aw IWriterForApplication", 0),
            HostObject::EctxRegistrar => ("ectx:aw IContextRegistrar", 0),
            HostObject::PctlFactory => ("pctl IParentalControlServiceFactory", 0),
            HostObject::PctlService => ("pctl IParentalControlService", 0),
            HostObject::TimeStaticService => ("time:u IStaticService", 0),
            HostObject::TimeSystemClock => ("time:u ISystemClock", 0),
            HostObject::TimeSteadyClock => ("time:u ISteadyClock", 0),
            HostObject::TimeZoneService => ("time:u ITimeZoneService", 0),
            HostObject::AccUser => ("acc:u0 IAccountServiceForApplication", 0),
            HostObject::AccProfile => ("acc:u0 IProfile", 0),
            HostObject::AccSession => ("acc:u0 ISessionObject", 0),
            HostObject::FileSystemProxy => ("fsp-srv IFileSystemProxy", 0),
            HostObject::FileSystem => ("fsp-srv IFileSystem", 0),
            HostObject::Storage => ("fsp-srv IStorage", 0),
            HostObject::NvDrvServices => ("nvdrv INvDrvServices", 0),
            HostObject::ViUser => ("vi:u IApplicationRootService", 0),
            HostObject::ViDisplayService => ("vi IApplicationDisplayService", 0),
            HostObject::HosBinderDriver => ("nns::hosbinder::IHOSBinderDriver", 0),
            HostObject::HidServer => ("nn::hid::IHidServer", 0),
            HostObject::HidAppletResource => ("nn::hid::IAppletResource", 0),
            HostObject::ActiveVibrationDeviceList => ("nn::hid::IActiveVibrationDeviceList", 0),
        }
    }
    fn server(&self) -> Server {
        match self {
            HostObject::LogService | HostObject::Logger { .. } => Server::Lm,
            HostObject::ApmManager | HostObject::ApmSession => Server::Ptm,
            HostObject::AocManager => Server::Ns,
            HostObject::EctxWriter | HostObject::EctxRegistrar => Server::Glue,
            HostObject::PctlFactory | HostObject::PctlService => Server::Pctl,
            HostObject::TimeStaticService
            | HostObject::TimeSystemClock
            | HostObject::TimeSteadyClock
            | HostObject::TimeZoneService => Server::Time,
            HostObject::AccUser | HostObject::AccProfile | HostObject::AccSession => Server::Acc,
            HostObject::FileSystemProxy | HostObject::FileSystem | HostObject::Storage => {
                Server::Fs
            }
            HostObject::NvDrvServices => Server::NvDrv,
            HostObject::ViUser | HostObject::ViDisplayService | HostObject::HosBinderDriver => {
                Server::Vi
            }
            HostObject::HidServer
            | HostObject::HidAppletResource
            | HostObject::ActiveVibrationDeviceList => Server::Hid,
            _ => Server::Am,
        }
    }
    /// The server's pointer-buffer size, where a source establishes it.
    fn pointer_buffer_size(&self) -> Option<u16> {
        match self.server() {
            Server::Lm => Some(LM_POINTER_BUFFER_SIZE),
            Server::Am
            | Server::Ns
            | Server::Glue
            | Server::Pctl
            | Server::Time
            | Server::Acc
            | Server::Fs
            | Server::NvDrv
            | Server::Vi
            | Server::Hid => Some(AM_POINTER_BUFFER_SIZE),
            Server::Ptm => None,
        }
    }
}

/// What a hosted service does with a request.
pub(crate) enum Outcome {
    Reply(Vec<u8>),
    /// The server closes the session; the client sees SessionClosed.
    Close,
}

/// A CMIF domain: objects addressed by ID through one session.
#[derive(Clone, Debug, Default)]
pub struct Domain {
    pub objects: Vec<(u32, HostObject)>,
}
/// `lm`'s `MaxDomainObjects`.
const LM_MAX_DOMAIN_OBJECTS: u32 = 61;

/// The domain object IDs of one server (`ServerDomainManager::EntryManager`): entries in
/// order from ID 1, and freed IDs reused most recently freed first.
#[derive(Clone, Debug, Default)]
pub struct DomainIds {
    next: u32,
    freed: Vec<u32>,
}
impl DomainIds {
    fn allocate(&mut self, limit: Option<u32>) -> Result<u32, String> {
        if let Some(id) = self.freed.pop() {
            return Ok(id);
        }
        if limit.is_some_and(|limit| self.next >= limit) {
            return Err("unsupported Horizon IPC: CMIF domain entries exhausted".into());
        }
        self.next += 1;
        Ok(self.next)
    }
}

impl Domain {
    fn get(&self, id: u32) -> Option<HostObject> {
        self.objects.iter().find(|(i, _)| *i == id).map(|&(_, o)| o)
    }
}
impl HostState {
    /// Registers `object` in domain `d` under a newly allocated ID.
    fn ids(&mut self, server: Server) -> &mut DomainIds {
        match server {
            Server::Lm => &mut self.lm_domain_ids,
            Server::Ptm => &mut self.ptm_domain_ids,
            Server::Am => &mut self.am_domain_ids,
            Server::Ns => &mut self.ns_domain_ids,
            Server::Glue => &mut self.glue_domain_ids,
            Server::Pctl => &mut self.pctl_domain_ids,
            Server::Time => &mut self.time_domain_ids,
            Server::Acc => &mut self.acc_domain_ids,
            Server::Fs => &mut self.fs_domain_ids,
            Server::NvDrv => &mut self.nvdrv_domain_ids,
            Server::Vi => &mut self.vi_domain_ids,
            Server::Hid => &mut self.hid_domain_ids,
        }
    }
    /// Registers `object` in domain `d` under a newly allocated ID of its server (`lm`
    /// allows 61 entries; the limits of ptm and am are not documented and not enforced).
    fn add(&mut self, d: usize, object: HostObject) -> Result<u32, String> {
        let server = object.server();
        let limit = (server == Server::Lm).then_some(LM_MAX_DOMAIN_OBJECTS);
        let id = self.ids(server).allocate(limit)?;
        self.domains[d].objects.push((id, object));
        Ok(id)
    }
    /// Removes object `id` from domain `d`, freeing its ID.
    fn remove(&mut self, d: usize, id: u32) {
        let objects = &mut self.domains[d].objects;
        if let Some(i) = objects.iter().position(|(o, _)| *o == id) {
            let (_, object) = objects.remove(i);
            self.ids(object.server()).freed.push(id);
        }
    }
}

const CMIF_IN_MAGIC: u32 = u32::from_le_bytes(*b"SFCI");
const CMIF_OUT_MAGIC: u32 = u32::from_le_bytes(*b"SFCO");
const DOMAIN_SEND_MESSAGE: u8 = 1;
const DOMAIN_CLOSE: u8 = 2;
/// `sf::hipc` results (module 11).
const TARGET_NOT_DOMAIN: u32 = 11 | 491 << 9;
const DOMAIN_OBJECT_NOT_FOUND: u32 = 11 | 492 << 9;

/// A CMIF reply. Outside a domain, output objects are sessions moved in the special header
/// after `handles`; in a domain (`domain_ids`), a domain out header with the object count
/// precedes `SFCO`, and the object IDs follow the output data.
fn reply(
    interface_id: u32,
    code: u32,
    data: &[u8],
    handles: &[u32],
    domain_ids: Option<&[u32]>,
) -> Vec<u8> {
    reply_with_copies(interface_id, code, data, &[], handles, domain_ids)
}

/// A CMIF reply that also copies `copies` (before the moved handles).
fn reply_with_copies(
    interface_id: u32,
    code: u32,
    data: &[u8],
    copies: &[u32],
    handles: &[u32],
    domain_ids: Option<&[u32]>,
) -> Vec<u8> {
    let domain_header = if domain_ids.is_some() { 0x10 } else { 0 };
    let ids = domain_ids.unwrap_or_default();
    let raw = domain_header + 0x10 + data.len() + 4 * ids.len();
    let words = (raw.div_ceil(4) * 4 + 0x10) / 4;
    let mut out = vec![0u8; 8];
    let special = !(handles.is_empty() && copies.is_empty());
    out[4..8].copy_from_slice(&(words as u32 | if special { 1 << 31 } else { 0 }).to_le_bytes());
    if special {
        out.extend(((copies.len() as u32) << 1 | (handles.len() as u32) << 5).to_le_bytes());
        for h in copies.iter().chain(handles) {
            out.extend(h.to_le_bytes());
        }
    }
    let mut at = out.len().next_multiple_of(16);
    out.resize(out.len() + words * 4, 0);
    let mut put = |at: &mut usize, bytes: &[u8]| {
        out[*at..*at + bytes.len()].copy_from_slice(bytes);
        *at += bytes.len();
    };
    if domain_ids.is_some() {
        put(&mut at, &(ids.len() as u32).to_le_bytes());
        at += 12;
    }
    for w in [CMIF_OUT_MAGIC, 0, code, interface_id] {
        put(&mut at, &w.to_le_bytes());
    }
    put(&mut at, data);
    for id in ids {
        put(&mut at, &id.to_le_bytes());
    }
    out
}

/// The command's expected request shape, checked as `HipcCommandProcessor::PrepareForProcess`.
struct Shape {
    pid: bool,
    statics: usize,
    sends: usize,
    copies: usize,
    in_data: usize,
}

/// A command's result: on success, output data and output objects.
struct Done {
    code: u32,
    data: Vec<u8>,
    objects: Vec<HostObject>,
    /// Handles copied to the client.
    copies: Vec<u32>,
}
impl Done {
    fn ok() -> Self {
        Done {
            code: result::SUCCESS,
            data: Vec::new(),
            objects: Vec::new(),
            copies: Vec::new(),
        }
    }
    fn data(data: &[u8]) -> Self {
        Done {
            data: data.to_vec(),
            ..Done::ok()
        }
    }
    fn object(object: HostObject) -> Self {
        Done {
            objects: vec![object],
            ..Done::ok()
        }
    }
    fn failure(code: u32) -> Self {
        Done { code, ..Done::ok() }
    }
}

/// One request on a session to a hosted service.
pub(crate) fn request(
    memory: &mut Memory,
    k: &mut Kernel,
    s: usize,
    object: HostObject,
    req: &Request,
    message: &[u8; ipc::MESSAGE_BUFFER_SIZE],
) -> Result<Outcome, String> {
    if req.kind == 2 {
        return Ok(Outcome::Close);
    }
    let control = matches!(req.kind, 5 | 7);
    if !matches!(req.kind, 4..=7) {
        // sf::hipc::ResultUnknownCommandType: the session is closed.
        return Ok(Outcome::Close);
    }
    // CMIF requires 16 bytes of padding in the raw data and 16-byte alignment.
    let raw_start = req.data.next_multiple_of(16);
    let raw_end = req.data + 4 * req.data_words;
    if 4 * req.data_words < 0x10 || raw_end - raw_start < 0x10 {
        return Ok(Outcome::Close);
    }
    let domain = k.sessions[s].domain;
    let (target, cmif) = match domain {
        Some(d) if !control => {
            // Domain in header: type, input object count, data size, object ID.
            let raw = &message[raw_start..raw_end];
            let (kind, in_objects) = (raw[0], raw[1]);
            let size = usize::from(u16::from_le_bytes([raw[2], raw[3]]));
            let id = ipc::read_u32(raw, 4);
            let body = &raw[0x10..];
            match kind {
                DOMAIN_SEND_MESSAGE => {
                    let Some(target) = k.host.domains[d].get(id) else {
                        // sf::cmif::ResultTargetNotFound: the session is closed.
                        return Ok(Outcome::Close);
                    };
                    if size + 4 * usize::from(in_objects) > body.len() {
                        return Ok(Outcome::Close);
                    }
                    if in_objects != 0 {
                        return Err(format!(
                            "unsupported Horizon IPC: input objects sent to {target:?}"
                        ));
                    }
                    (target, raw_start + 0x10..raw_start + 0x10 + size)
                }
                DOMAIN_CLOSE => {
                    k.host.remove(d, id);
                    // No reply is written: the message buffer is returned as sent.
                    return Ok(Outcome::Reply(Vec::new()));
                }
                _ => return Ok(Outcome::Close),
            }
        }
        _ => (object, raw_start..raw_end),
    };
    let (name, interface_id) = target.interface();
    if cmif.len() < 0x10 {
        return Ok(Outcome::Close);
    }
    let word = |i: usize| ipc::read_u32(message, cmif.start + 4 * i);
    if word(0) != CMIF_IN_MAGIC || word(1) > 1 {
        return Ok(Outcome::Close);
    }
    let command = word(2);
    let args = &message[cmif.start + 0x10..cmif.end];
    eprintln!("[ipc] {name} cmd {command}");
    let unsupported = |what: &str| {
        Err(format!(
            "unsupported Horizon IPC: {what} (command {command} of {name}{})",
            if control { " control" } else { "" }
        ))
    };
    let check = |shape: Shape| {
        let valid = req.pid.is_some() == shape.pid
            && req.statics.len() == shape.statics
            && req.sends.len() == shape.sends
            && req.recvs.is_empty()
            && req.exchanges == 0
            && req.copy_handles as usize == shape.copies
            && req.move_handles == 0
            && args.len() >= shape.in_data;
        if valid {
            Ok(())
        } else {
            Err(format!(
                "unsupported Horizon IPC: the request layout differs from command {command} of {name} ({req:?})"
            ))
        }
    };
    let no_arguments = Shape {
        pid: false,
        statics: 0,
        sends: 0,
        copies: 0,
        in_data: 0,
    };
    if control {
        return match command {
            // QueryPointerBufferSize: out u16.
            3 => {
                check(no_arguments)?;
                let Some(size) = object.pointer_buffer_size() else {
                    return unsupported("the pointer-buffer size of this server");
                };
                let size = size.to_le_bytes();
                Ok(Outcome::Reply(reply(0, result::SUCCESS, &size, &[], None)))
            }
            // ConvertCurrentObjectToDomain: out the object's ID in the new domain.
            0 => {
                check(no_arguments)?;
                if domain.is_some() {
                    return unsupported("converting a domain session to a domain");
                }
                k.host.domains.push(Domain::default());
                let d = k.host.domains.len() - 1;
                let id = k.host.add(d, object)?;
                k.sessions[s].domain = Some(d);
                Ok(Outcome::Reply(reply(
                    0,
                    result::SUCCESS,
                    &id.to_le_bytes(),
                    &[],
                    None,
                )))
            }
            // CopyFromCurrentDomain(object ID): a session to that object, outside the domain.
            1 => {
                check(Shape {
                    in_data: 4,
                    ..no_arguments
                })?;
                let id = ipc::read_u32(args, 0);
                let Some(d) = domain else {
                    return Ok(Outcome::Reply(reply(0, TARGET_NOT_DOMAIN, &[], &[], None)));
                };
                let Some(found) = k.host.domains[d].get(id) else {
                    return Ok(Outcome::Reply(reply(
                        0,
                        DOMAIN_OBJECT_NOT_FOUND,
                        &[],
                        &[],
                        None,
                    )));
                };
                Ok(Outcome::Reply(
                    match ipc::open_session(k, Endpoint::Host(found)) {
                        Ok(h) => reply(0, result::SUCCESS, &[], &[h], None),
                        Err(e) => reply(0, e, &[], &[], None),
                    },
                ))
            }
            // CloneCurrentObject, CloneCurrentObjectEx(u32 tag): a new session to the object.
            2 | 4 => {
                check(Shape {
                    in_data: if command == 4 { 4 } else { 0 },
                    ..no_arguments
                })?;
                let endpoint = k.sessions[s].endpoint;
                let domain = k.sessions[s].domain;
                Ok(Outcome::Reply(match ipc::open_session(k, endpoint) {
                    Ok(h) => {
                        if let Some(crate::horizon::Object::Session(new_s)) = k.handles.get(h) {
                            k.sessions[new_s].domain = domain;
                        }
                        reply(0, result::SUCCESS, &[], &[h], None)
                    }
                    Err(e) => reply(0, e, &[], &[], None),
                }))
            }
            _ => unsupported("an unknown control command"),
        };
    }
    let done = match (target, command) {
        (HostObject::LogService, 0) => {
            // OpenLogger(ClientProcessId): the process ID is a u64 placeholder in the data,
            // replaced by the one the kernel wrote into the special header.
            check(Shape {
                pid: true,
                in_data: 8,
                ..no_arguments
            })?;
            let process_id = req.pid.unwrap_or_default();
            Done {
                objects: vec![HostObject::Logger { process_id }],
                ..Done::ok()
            }
        }
        (HostObject::Logger { process_id }, 0) => {
            // Log(InAutoSelectBuffer): the mapped buffer when its address is non-zero, else
            // the pointer buffer.
            check(Shape {
                statics: 1,
                sends: 1,
                ..no_arguments
            })?;
            let (address, size) = if req.sends[0].0 != 0 {
                req.sends[0]
            } else {
                req.statics[0]
            };
            let mut packet = vec![0u8; size as usize];
            memory
                .read_into(address, &mut packet)
                .map_err(|e| format!("lm ILogger::Log buffer unreadable: {e}"))?;
            log(k, process_id, &packet);
            Done::ok()
        }
        (HostObject::Logger { .. }, 1) => {
            check(Shape {
                in_data: 4,
                ..no_arguments
            })?;
            k.host.log_destination = Some(ipc::read_u32(args, 0));
            Done::ok()
        }
        (HostObject::ApmManager, 0) => {
            check(no_arguments)?;
            Done {
                objects: vec![HostObject::ApmSession],
                ..Done::ok()
            }
        }
        (HostObject::ApmManager, 1) => {
            // GetPerformanceMode: the operating mode.
            check(no_arguments)?;
            Done {
                data: u32::from(DOCKED).to_le_bytes().to_vec(),
                ..Done::ok()
            }
        }
        (HostObject::ApmSession, 0) => {
            // SetPerformanceConfiguration(PerformanceMode, PerformanceConfiguration).
            check(Shape {
                in_data: 8,
                ..no_arguments
            })?;
            let (mode, configuration) = (ipc::read_u32(args, 0), ipc::read_u32(args, 4));
            if mode > 1 || !PERFORMANCE_CONFIGURATIONS.contains(&configuration) {
                return unsupported(&format!(
                    "the result of PerformanceMode {mode:#x} with configuration {configuration:#x}"
                ));
            }
            k.host.performance_configuration[mode as usize] = Some(configuration);
            Done::ok()
        }
        (HostObject::ApmSession, 1) => {
            // GetPerformanceConfiguration(PerformanceMode).
            check(Shape {
                in_data: 4,
                ..no_arguments
            })?;
            let mode = ipc::read_u32(args, 0);
            let Some(configuration) = k
                .host
                .performance_configuration
                .get(mode as usize)
                .copied()
                .flatten()
            else {
                return unsupported(&format!(
                    "the configuration of PerformanceMode {mode:#x} before one is set"
                ));
            };
            Done {
                data: configuration.to_le_bytes().to_vec(),
                ..Done::ok()
            }
        }
        (HostObject::AppletOe, 0) => {
            // OpenApplicationProxy(u64 reserved, ClientProcessId, copied process handle).
            check(Shape {
                pid: true,
                copies: 1,
                in_data: 8,
                ..no_arguments
            })?;
            k.host.applet_messages = vec![
                APPLET_MESSAGE_FOCUS_STATE_CHANGED,
                APPLET_MESSAGE_CHANGE_INTO_FOREGROUND,
            ];
            Done::object(HostObject::ApplicationProxy)
        }
        (HostObject::ApplicationProxy, c) => {
            check(no_arguments)?;
            Done::object(match c {
                0 => HostObject::CommonStateGetter,
                1 => HostObject::SelfController,
                2 => HostObject::WindowController,
                3 => HostObject::AudioController,
                4 => HostObject::DisplayController,
                11 => HostObject::LibraryAppletCreator,
                20 => HostObject::ApplicationFunctions,
                1000 => HostObject::DebugFunctions,
                _ => return unsupported("an unknown command"),
            })
        }
        (HostObject::CommonStateGetter, 0) => {
            // GetEventHandle: the message event, signalled while messages are queued.
            check(no_arguments)?;
            let signaled = !k.host.applet_messages.is_empty();
            let e = host_event(k, |h| &mut h.applet_message_event, signaled);
            copy_event(k, e)
        }
        (HostObject::SelfController, 90) => {
            // GetAccumulatedSuspendedTickValue: never suspended.
            check(no_arguments)?;
            Done::data(&0u64.to_le_bytes())
        }
        (HostObject::SelfController, 91) => {
            // GetAccumulatedSuspendedTickChangedEvent.
            check(no_arguments)?;
            let e = host_event(k, |h| &mut h.suspended_tick_event, false);
            copy_event(k, e)
        }
        (HostObject::CommonStateGetter, 1) => {
            // ReceiveMessage: the oldest message, or NoMessage.
            check(no_arguments)?;
            if k.host.applet_messages.is_empty() {
                Done::failure(AM_NO_MESSAGE)
            } else {
                let message = k.host.applet_messages.remove(0);
                if let (true, Some(e)) = (
                    k.host.applet_messages.is_empty(),
                    k.host.applet_message_event,
                ) {
                    k.events[e].signaled = false;
                }
                Done::data(&message.to_le_bytes())
            }
        }
        (HostObject::CommonStateGetter, 5) => {
            // GetOperationMode: Console (1) when docked, Handheld (0).
            check(no_arguments)?;
            Done::data(&[u8::from(DOCKED)])
        }
        (HostObject::CommonStateGetter, 6) => {
            check(no_arguments)?;
            Done::data(&u32::from(DOCKED).to_le_bytes())
        }
        (HostObject::CommonStateGetter, 9) => {
            check(no_arguments)?;
            Done::data(&[FOCUS_IN_FOCUS])
        }
        (HostObject::SelfController, 11 | 12 | 14 | 16) => {
            // SetOperationModeChangedNotification, SetPerformanceModeChangedNotification,
            // SetRestartMessageEnabled, SetOutOfFocusSuspendingEnabled: one bool.
            check(Shape {
                in_data: 1,
                ..no_arguments
            })?;
            let enabled = Some(args[0] & 1 != 0);
            match command {
                11 => k.host.operation_mode_notification = enabled,
                12 => k.host.performance_mode_notification = enabled,
                14 => k.host.restart_message_enabled = enabled,
                _ => k.host.out_of_focus_suspending = enabled,
            }
            Done::ok()
        }
        (HostObject::SelfController, 13) => {
            // SetFocusHandlingMode: three bools.
            check(Shape {
                in_data: 3,
                ..no_arguments
            })?;
            k.host.focus_handling_mode = Some([0, 1, 2].map(|i| args[i] & 1 != 0));
            Done::ok()
        }
        (HostObject::SelfController, 40) => {
            // CreateManagedDisplayLayer: out u64 LayerId
            Done::data(&1u64.to_le_bytes())
        }
        (HostObject::SelfController, 44) => {
            // CreateManagedDisplaySeparableLayer: out u64 LayerId, out u64 RecordingLayerId
            let mut out = [0u8; 16];
            out[..8].copy_from_slice(&1u64.to_le_bytes());
            out[8..16].copy_from_slice(&2u64.to_le_bytes());
            Done::data(&out)
        }
        (HostObject::SelfController, 45..=48) => {
            // SetManagedDisplayLayerSeparationMode, SetRecordingLayerCompositionEnabled,
            // SetHandlesRequestToDisplay, ApproveToDisplay.
            Done::ok()
        }
        (HostObject::CommonStateGetter, 2) => {
            // GetThisAppletKind: Application (3).
            Done::data(&3u32.to_le_bytes())
        }
        (HostObject::CommonStateGetter, 7) => {
            // GetCradleStatus: InCradle (1).
            Done::data(&[1u8])
        }
        (HostObject::CommonStateGetter, 8) => {
            // GetBootMode: Normal (0).
            Done::data(&[0u8])
        }
        (HostObject::CommonStateGetter, 65..=69) => {
            // SetGamePlayRecordingState, InitializeGamePlayRecording, RequestFlushGamePlayingMovieForDebug.
            Done::ok()
        }
        (HostObject::CommonStateGetter, 900) => {
            // SetRequestExitToLibraryAppletAtExecuteNextProgramEnabled.
            check(no_arguments)?;
            k.host
                .request_exit_to_library_applet_at_execute_next_program = true;
            Done::ok()
        }
        (HostObject::ApplicationFunctions, 130) => {
            // GetGpuErrorDetectedSystemEvent.
            check(no_arguments)?;
            let e = host_event(k, |h| &mut h.gpu_error_event, false);
            copy_event(k, e)
        }
        (HostObject::ApplicationFunctions, 131) => {
            // SetDelayTimeToAbortOnGpuError(u64).
            check(Shape {
                in_data: 8,
                ..no_arguments
            })?;
            k.host.delay_time_to_abort_on_gpu_error =
                Some(u64::from_le_bytes(args[..8].try_into().unwrap_or_default()));
            Done::ok()
        }
        (HostObject::ApplicationFunctions, 1) => {
            // PopLaunchParameter(u32 kind): out IStorage.
            // When no launch parameter is present in the channel, return ResultNoDataInChannel (0x480).
            Done::failure(0x480)
        }
        (HostObject::ApplicationFunctions, 2) => {
            // CreateFeatureStorage: out IStorage.
            Done::object(HostObject::Storage)
        }
        (HostObject::ApplicationFunctions, 50) => {
            // GetPseudoDeviceId: out 16 bytes UUID.
            Done::data(&[0u8; 16])
        }
        (HostObject::ApplicationFunctions, 123) => {
            // GetApplicationLaunchProperty: out ApplicationLaunchProperty (16 bytes).
            let requested_title_id = if args.len() >= 8 {
                (ipc::read_u32(args, 0) as u64) | ((ipc::read_u32(args, 4) as u64) << 32)
            } else {
                0
            };
            let title_id = if requested_title_id != 0 {
                requested_title_id
            } else if k.config.program_id != 0 {
                k.config.program_id
            } else {
                0x0100f2c0115b6000
            };
            let mut prop = [0u8; 16];
            prop[..8].copy_from_slice(&title_id.to_le_bytes());
            prop[8..12].copy_from_slice(&720896u32.to_le_bytes()); // version 0xb0000 (v1.2.0)
            prop[12] = 1; // StorageId::BuiltInUser
            prop[13] = 1; // StorageId::BuiltInUser
            prop[14] = 1; // ApplicationKind::Standard
            prop[15] = 0;
            let dst_info = req
                .recvs
                .first()
                .or(req.statics.first())
                .or(req.sends.first());
            if let Some(&(dst, buf_size)) = dst_info {
                let to_copy = prop.len().min(buf_size as usize);
                let _ = memory.write_bytes(dst, &prop[..to_copy]);
            }
            Done::data(&prop)
        }
        (HostObject::ApplicationFunctions, 210) => {
            // Cmd210: no arguments, returns copy handle to event.
            check(no_arguments)?;
            let e = host_event(k, |h| &mut h.application_event_210, true);
            copy_event(k, e)
        }
        (HostObject::ApplicationFunctions, 20) => {
            // EnsureSaveData: out u64 size.
            Done::data(&0u64.to_le_bytes())
        }
        (HostObject::ApplicationFunctions, 21) => {
            // GetDesiredLanguage: out LanguageCode (8 bytes, "en-US").
            Done::data(b"en-US\0\0\0")
        }
        (HostObject::ApplicationFunctions, 22) => {
            // SetTerminateResult(u32).
            let rc = if args.len() >= 4 {
                ipc::read_u32(args, 0)
            } else {
                0
            };
            eprintln!(
                "[am] SetTerminateResult: {rc:#x} (desc={}, mod={})",
                rc & 0x1fff,
                (rc >> 9) & 0x1ff
            );
            Done::ok()
        }
        (HostObject::ApplicationFunctions, 23) => {
            // GetDisplayVersion: out 16 bytes version string.
            let mut v = [0u8; 16];
            v[..5].copy_from_slice(b"1.0.0");
            Done::data(&v)
        }
        (HostObject::ApplicationFunctions, 40) => {
            // NotifyRunning: out bool (true).
            Done::data(&[1u8])
        }
        (HostObject::ApplicationFunctions, 65..=69) => {
            // SetGamePlayRecordingState, InitializeGamePlayRecording, RequestFlushGamePlayingMovieForDebug.
            Done::ok()
        }
        (HostObject::WindowController, 1) => {
            // GetAppletResourceUserId: the application's process ID.
            check(no_arguments)?;
            Done::data(&k.config.process_id.to_le_bytes())
        }
        (HostObject::WindowController, 10) => {
            check(no_arguments)?;
            k.host.foreground_rights = true;
            Done::ok()
        }
        (HostObject::AocManager, 0) => {
            // CountAddOnContentByApplicationId(u64 application_id): out u32 count.
            check(Shape {
                in_data: 8,
                ..no_arguments
            })?;
            Done::data(&0u32.to_le_bytes())
        }
        (HostObject::AocManager, 1) => {
            // ListAddOnContentByApplicationId: out u32 count.
            Done::data(&0u32.to_le_bytes())
        }
        (HostObject::AocManager, 2) => {
            // CountAddOnContent: out u32 count (0 DLCs).
            if req.pid.is_some() {
                check(Shape {
                    pid: true,
                    ..no_arguments
                })?;
            } else {
                check(no_arguments)?;
            }
            Done::data(&0u32.to_le_bytes())
        }
        (HostObject::AocManager, 3) => {
            // ListAddOnContent: out u32 count.
            Done::data(&0u32.to_le_bytes())
        }
        (HostObject::AocManager, 4) => {
            // GetAddOnContentBaseIdByApplicationId(u64 application_id): out u64 base_id.
            check(Shape {
                in_data: 8,
                ..no_arguments
            })?;
            Done::data(&0u64.to_le_bytes())
        }
        (HostObject::AocManager, 5) => {
            // GetAddOnContentBaseId: out u64 base_id.
            if req.pid.is_some() {
                check(Shape {
                    pid: true,
                    ..no_arguments
                })?;
            } else {
                check(no_arguments)?;
            }
            Done::data(&0u64.to_le_bytes())
        }
        (HostObject::AocManager, 7) => {
            // PrepareAddOnContent.
            Done::ok()
        }
        (HostObject::AocManager, 8) => {
            // GetAddOnContentListChangedEvent.
            check(no_arguments)?;
            let e = host_event(k, |h| &mut h.aoc_list_changed_event, false);
            copy_event(k, e)
        }
        (HostObject::EctxWriter, 0) => {
            // CreateContextRegistrar.
            check(no_arguments)?;
            Done::object(HostObject::EctxRegistrar)
        }
        (HostObject::EctxWriter, 1) => {
            // CommitContext.
            Done::ok()
        }
        (HostObject::EctxRegistrar, 0) => {
            // Complete.
            check(no_arguments)?;
            Done::ok()
        }
        (HostObject::PctlFactory, 0 | 1) => {
            // CreateService / CreateServiceWithoutInitialize.
            Done::object(HostObject::PctlService)
        }
        (HostObject::PctlService, 0 | 1) => {
            // Initialize.
            Done::ok()
        }
        (HostObject::PctlService, 1011) => {
            // IsRestrictionEnabled: out bool (false).
            Done::data(&[0u8])
        }
        (HostObject::PctlService, 1012) => {
            // GetSafetyLevel: out u32 (0).
            Done::data(&0u32.to_le_bytes())
        }
        (HostObject::PctlService, 1013 | 1017 | 1031) => {
            // IsFreeCommunicationAvailable, IsStereoVisionPermitted: out bool (true).
            Done::data(&[1u8])
        }
        (HostObject::PctlService, 1014) => {
            // IsRestrictionTemporaryUnlocked: out bool (false).
            Done::data(&[0u8])
        }
        (HostObject::PctlService, _) => {
            // Permission and state queries succeed when restrictions are disabled.
            Done::ok()
        }
        (HostObject::TimeStaticService, 0 | 1 | 4) => {
            // GetStandardUserSystemClock / GetStandardNetworkSystemClock / GetStandardLocalSystemClock.
            Done::object(HostObject::TimeSystemClock)
        }
        (HostObject::TimeStaticService, 2) => {
            // GetStandardSteadyClock.
            Done::object(HostObject::TimeSteadyClock)
        }
        (HostObject::TimeStaticService, 3) => {
            // GetTimeZoneService.
            Done::object(HostObject::TimeZoneService)
        }
        (HostObject::TimeStaticService, 20) => {
            // GetSharedMemoryNativeHandle: out copy handle of TimeSharedMemory.
            check(no_arguments)?;
            match get_or_create_time_shared_memory(k) {
                Ok(h) => Done {
                    copies: vec![h],
                    ..Done::ok()
                },
                Err(code) => Done::failure(code),
            }
        }
        (HostObject::TimeStaticService, _) => Done::ok(),
        (HostObject::TimeSystemClock, 0) => {
            // GetCurrentTime: u64 posix seconds.
            Done::data(&1700000000u64.to_le_bytes())
        }
        (HostObject::TimeSystemClock, _) => Done::ok(),
        (HostObject::TimeSteadyClock, 0) => {
            // GetCurrentTimePoint: 24 bytes (time, source uuid).
            Done::data(&[0u8; 24])
        }
        (HostObject::TimeSteadyClock, _) => Done::ok(),
        (HostObject::TimeZoneService, 0) => {
            // GetDeviceLocationName: 0x24 bytes "UTC".
            let mut name = [0u8; 0x24];
            name[..3].copy_from_slice(b"UTC");
            Done::data(&name)
        }
        (HostObject::TimeZoneService, _) => Done::ok(),
        (HostObject::AccUser, 0) => {
            // GetUserCount: out u32 (1).
            Done::data(&1u32.to_le_bytes())
        }
        (HostObject::AccUser, 1) => {
            // GetUserExistence: out bool (true).
            Done::data(&[1u8])
        }
        (HostObject::AccUser, 2) => {
            // ListAllUsers: 1 user (16 bytes UID).
            let mut uid = [0u8; 16];
            uid[0] = 1;
            let dst_info = req.recvs.first().or(req.statics.first());
            if let Some(&(dst, buf_size)) = dst_info {
                let to_copy = uid.len().min(buf_size as usize);
                let _ = memory.write_bytes(dst, &uid[..to_copy]);
            }
            Done::ok()
        }
        (HostObject::AccUser, 3) => {
            // ListOpenUsers: 1 user (16 bytes UID) into out buffer
            let mut uid = [0u8; 16];
            uid[0] = 1;
            let dst_info = req.recvs.first().or(req.statics.first());
            if let Some(&(dst, buf_size)) = dst_info {
                let to_copy = uid.len().min(buf_size as usize);
                let _ = memory.write_bytes(dst, &uid[..to_copy]);
            }
            Done::ok()
        }
        (HostObject::AccUser, 4) => {
            // GetLastOpenedUser: out 16-byte UID.
            let mut uid = [0u8; 16];
            uid[0] = 1;
            Done::data(&uid)
        }
        (HostObject::AccUser, 5) => {
            // GetProfile: in 16-byte UID, out object AccProfile.
            Done::object(HostObject::AccProfile)
        }
        (HostObject::AccUser, 100 | 160) => {
            // InitializeApplicationInfo / InitializeApplicationInfoV2
            Done::ok()
        }
        (HostObject::AccUser, 130) => {
            // ActivateOpenContextRetention: returns ISessionObject
            Done::object(HostObject::AccSession)
        }
        (HostObject::AccUser, 131) => {
            // ListOpenContextStoredUsers: write user UID to out buffer
            let mut uid = [0u8; 16];
            uid[0] = 1;
            let dst_info = req.recvs.first().or(req.statics.first());
            if let Some(&(dst, buf_size)) = dst_info {
                let to_copy = uid.len().min(buf_size as usize);
                let _ = memory.write_bytes(dst, &uid[..to_copy]);
            }
            Done::ok()
        }
        (HostObject::AccUser, 150) => {
            // IsUserAccountSwitchLocked: out bool (false)
            Done::data(&[0u8])
        }
        (HostObject::AccUser, _) => Done::ok(),
        (HostObject::AccSession, _) => Done::ok(),
        (HostObject::AccProfile, 0) => {
            // Get: out ProfileBase (0x38 bytes) and out UserData (0x80 bytes to buffer)
            let mut profile = [0u8; 0x38];
            profile[0] = 1; // uid
            profile[0x10..0x10 + 4].copy_from_slice(b"Link");
            let dst_info = req.recvs.first().or(req.statics.first());
            if let Some(&(dst, buf_size)) = dst_info {
                let user_data = [0u8; 0x80];
                let to_copy = user_data.len().min(buf_size as usize);
                let _ = memory.write_bytes(dst, &user_data[..to_copy]);
            }
            Done::data(&profile)
        }
        (HostObject::AccProfile, 1) => {
            // GetBase: out ProfileBase (0x38 bytes)
            let mut profile = [0u8; 0x38];
            profile[0] = 1; // uid
            profile[0x10..0x10 + 4].copy_from_slice(b"Link");
            Done::data(&profile)
        }
        (HostObject::AccProfile, 10) => {
            // GetImageSize: out u32 size (0)
            Done::data(&0u32.to_le_bytes())
        }
        (HostObject::AccProfile, 11) => {
            // LoadImage: out image bytes to buffer
            Done::ok()
        }
        (HostObject::AccProfile, _) => Done::ok(),
        (HostObject::HidServer, 0) => {
            // CreateAppletResource(u64 aruid): out object IAppletResource
            Done::object(HostObject::HidAppletResource)
        }
        (HostObject::HidServer, 101) => {
            // GetSupportedNpadStyleSet: out u32 (0x1f: FullKey, Handheld, JoyDual, JoyLeft, JoyRight)
            Done::data(&0x1fu32.to_le_bytes())
        }
        (HostObject::HidServer, 106) => {
            // AcquireNpadStyleSetUpdateEventHandle: out copy handle of event
            let e = host_event(k, |h| &mut h.npad_style_set_update_event, false);
            copy_event(k, e)
        }
        (HostObject::HidServer, 108 | 121) => {
            // GetNpadJoyHoldType: out u64 (0: Vertical, 1: Horizontal)
            Done::data(&0u64.to_le_bytes())
        }
        (HostObject::HidServer, 114 | 127) => {
            // GetNpadHandheldActivationMode: out u64 (0: Dual, 1: Single)
            Done::data(&0u64.to_le_bytes())
        }
        (HostObject::HidServer, 203) => {
            // CreateActiveVibrationDeviceList: out object ActiveVibrationDeviceList
            Done::object(HostObject::ActiveVibrationDeviceList)
        }
        (HostObject::HidServer, 205) => {
            // IsVibrationPermitted: out bool (true)
            Done::data(&[1u8])
        }
        (HostObject::HidServer, 511) => {
            // GetVibrationMasterVolume: out f32 (1.0)
            Done::data(&1.0f32.to_bits().to_le_bytes())
        }
        (HostObject::HidServer, _) => Done::ok(),
        (HostObject::HidAppletResource, 0) => {
            // GetSharedMemoryHandle: out copy handle of HidSharedMemory
            match get_or_create_hid_shared_memory(k) {
                Ok(h) => Done {
                    copies: vec![h],
                    ..Done::ok()
                },
                Err(code) => Done::failure(code),
            }
        }
        (HostObject::HidAppletResource, _) => Done::ok(),
        (HostObject::ActiveVibrationDeviceList, _) => Done::ok(),
        (HostObject::FileSystemProxy, 1) => {
            // Initialize(u64 reserved, ClientProcessId, copied process handle).
            Done::ok()
        }
        (HostObject::FileSystemProxy, 18 | 51 | 52 | 203) => {
            // MountSaveData / OpenSaveDataFileSystem / OpenDirectoryFileSystem / OpenRomFileSystemByCurrentProcess
            Done::object(HostObject::FileSystem)
        }
        (HostObject::FileSystemProxy, 200 | 202) => {
            // OpenDataStorageByCurrentProcess / OpenDataStorageByDataId
            Done::object(HostObject::Storage)
        }
        (HostObject::FileSystemProxy, 1005) => {
            // SetGlobalAccessLogMode(u32 mode)
            Done::ok()
        }
        (HostObject::FileSystemProxy, 1006) => {
            // GetGlobalAccessLogMode: out u32 mode
            Done::data(&0u32.to_le_bytes())
        }
        (HostObject::FileSystemProxy, _) => Done::ok(),
        (HostObject::FileSystem, _) => Done::ok(),
        (HostObject::Storage, 0) => {
            // Read(u64 offset, u64 size, OutBuffer)
            let offset =
                (ipc::read_u32(args, 0) as usize) | ((ipc::read_u32(args, 4) as usize) << 32);
            let size =
                (ipc::read_u32(args, 8) as usize) | ((ipc::read_u32(args, 12) as usize) << 32);
            let dst_info = req
                .recvs
                .first()
                .or(req.statics.first())
                .or(req.sends.first());
            if let Some(&(dst, buf_size)) = dst_info {
                let to_copy = size.min(buf_size as usize);
                if offset < EMPTY_ROMFS.len() {
                    let avail = (EMPTY_ROMFS.len() - offset).min(to_copy);
                    let _ = memory.write_bytes(dst, &EMPTY_ROMFS[offset..offset + avail]);
                    if to_copy > avail {
                        let zeroes = vec![0u8; to_copy - avail];
                        let _ = memory.write_bytes(dst + avail as u64, &zeroes);
                    }
                } else {
                    let zeroes = vec![0u8; to_copy];
                    let _ = memory.write_bytes(dst, &zeroes);
                }
            }
            Done::ok()
        }
        (HostObject::Storage, 4) => {
            // GetSize: out u64 size
            Done::data(&(EMPTY_ROMFS.len() as u64).to_le_bytes())
        }
        (HostObject::Storage, _) => Done::ok(),
        (HostObject::NvDrvServices, 3) => {
            // Initialize(u32 transfer_mem_size, copy_handle proc, copy_handle tmem): out u32 error_code (0)
            Done::data(&0u32.to_le_bytes())
        }
        (HostObject::NvDrvServices, 0) => {
            // Open(DeviceName in buffer): out u32 fd, out u32 error_code (0)
            let mut out = [0u8; 8];
            out[..4].copy_from_slice(&1u32.to_le_bytes()); // fd 1
            out[4..8].copy_from_slice(&0u32.to_le_bytes()); // error_code 0
            Done::data(&out)
        }
        (HostObject::NvDrvServices, 1 | 11 | 12) => {
            // Ioctl, Ioctl2, Ioctl3: out u32 error_code (0)
            Done::data(&0u32.to_le_bytes())
        }
        (HostObject::NvDrvServices, 2) => {
            // Close: out u32 error_code (0)
            Done::data(&0u32.to_le_bytes())
        }
        (HostObject::NvDrvServices, 4) => {
            // QueryEvent: out copy handle, out u32 error_code (0)
            let e = host_event(k, |h| &mut h.nvdrv_event, true);
            let mut done = copy_event(k, e);
            done.data = 0u32.to_le_bytes().to_vec();
            done
        }
        (HostObject::NvDrvServices, 8) => {
            // SetAruid: out u32 error_code (0)
            Done::data(&0u32.to_le_bytes())
        }
        (HostObject::NvDrvServices, _) => Done::data(&0u32.to_le_bytes()),

        (HostObject::ViUser, 0..=2) => {
            // GetDisplayService: out object IApplicationDisplayService
            Done::object(HostObject::ViDisplayService)
        }
        (HostObject::ViUser, _) => Done::ok(),

        (HostObject::ViDisplayService, 100) => {
            // GetRelayService: out object IHOSBinderDriver
            Done::object(HostObject::HosBinderDriver)
        }
        (HostObject::ViDisplayService, 101) => {
            // GetSystemDisplayService: out object IApplicationDisplayService
            Done::object(HostObject::ViDisplayService)
        }
        (HostObject::ViDisplayService, 102) => {
            // GetManagerDisplayService: out object IApplicationDisplayService
            Done::object(HostObject::ViDisplayService)
        }
        (HostObject::ViDisplayService, 103) => {
            // GetIndirectDisplayTransactionService: out object IHOSBinderDriver
            Done::object(HostObject::HosBinderDriver)
        }
        (HostObject::ViDisplayService, 1000) => {
            // ListDisplays: out u64 count (1)
            Done::data(&1u64.to_le_bytes())
        }
        (HostObject::ViDisplayService, 1010) => {
            // OpenDisplay: out u64 display_id (1)
            Done::data(&1u64.to_le_bytes())
        }
        (HostObject::ViDisplayService, 1020) => {
            // CloseDisplay
            Done::ok()
        }
        (HostObject::ViDisplayService, 1102) => {
            // GetDisplayResolution: out u64 width (1920), out u64 height (1080)
            let mut data = [0u8; 16];
            data[..8].copy_from_slice(&1920u64.to_le_bytes());
            data[8..16].copy_from_slice(&1080u64.to_le_bytes());
            Done::data(&data)
        }
        (HostObject::ViDisplayService, 210 | 5202) => {
            // GetDisplayVsyncEvent: copy handle to event
            let e = host_event(k, |h| &mut h.vi_vsync_event, true);
            copy_event(k, e)
        }
        (HostObject::ViDisplayService, 2020) => {
            // OpenLayer: writes parcel into output buffer, returns out u64 parcel_size
            let parcel = make_native_window_parcel();
            let dst_info = req
                .recvs
                .first()
                .or(req.statics.first())
                .or(req.sends.first());
            if let Some(&(dst, buf_size)) = dst_info {
                let to_copy = parcel.len().min(buf_size as usize);
                let _ = memory.write_bytes(dst, &parcel[..to_copy]);
            }
            Done::data(&(parcel.len() as u64).to_le_bytes())
        }
        (HostObject::ViDisplayService, 2021) => {
            // CloseLayer
            Done::ok()
        }
        (HostObject::ViDisplayService, 2030) => {
            // CreateStrayLayer: out u64 layer_id (1), out u64 parcel_size
            let parcel = make_native_window_parcel();
            let dst_info = req
                .recvs
                .first()
                .or(req.statics.first())
                .or(req.sends.first());
            if let Some(&(dst, buf_size)) = dst_info {
                let to_copy = parcel.len().min(buf_size as usize);
                let _ = memory.write_bytes(dst, &parcel[..to_copy]);
            }
            let mut data = [0u8; 16];
            data[..8].copy_from_slice(&1u64.to_le_bytes());
            data[8..16].copy_from_slice(&(parcel.len() as u64).to_le_bytes());
            Done::data(&data)
        }
        (HostObject::ViDisplayService, 2031 | 2101) => {
            // DestroyStrayLayer, SetLayerScalingMode
            Done::ok()
        }
        (HostObject::ViDisplayService, _) => Done::ok(),

        (HostObject::HosBinderDriver, 0 | 3) => {
            // TransactParcel / TransactParcelAuto: write reply parcel into recv buffer
            // Default reply parcel: 1280x720, transform 0, pending 0, result 0
            let reply = make_parcel(
                &[
                    0x00, 0x05, 0x00, 0x00, // 1280
                    0xd0, 0x02, 0x00, 0x00, // 720
                    0x00, 0x00, 0x00, 0x00, // 0
                    0x00, 0x00, 0x00, 0x00, // 0
                    0x00, 0x00, 0x00, 0x00, // 0 (result = SUCCESS)
                ],
                &[],
            );
            let dst_info = req
                .recvs
                .first()
                .or(req.statics.first())
                .or(req.sends.first());
            if let Some(&(dst, buf_size)) = dst_info {
                let to_copy = reply.len().min(buf_size as usize);
                let _ = memory.write_bytes(dst, &reply[..to_copy]);
            }
            Done::ok()
        }
        (HostObject::HosBinderDriver, 1) => {
            // AdjustRefcount: no output
            Done::ok()
        }
        (HostObject::HosBinderDriver, 2) => {
            // GetNativeHandle: copy handle to event
            let e = host_event(k, |h| &mut h.binder_event, true);
            copy_event(k, e)
        }
        (HostObject::HosBinderDriver, _) => Done::ok(),
        _ => return unsupported("an unknown command"),
    };
    if k.host.calls.len() < CALL_TRACE_LIMIT {
        k.host.calls.push((name, command, done.code));
    }
    if done.code != result::SUCCESS {
        for h in done.copies {
            k.handles.close(h);
        }
        let ids: &[u32] = &[];
        return Ok(Outcome::Reply(reply(
            interface_id,
            done.code,
            &[],
            &[],
            domain.map(|_| ids),
        )));
    }
    Ok(Outcome::Reply(match domain {
        Some(d) => {
            let mut ids = Vec::new();
            for o in done.objects {
                ids.push(k.host.add(d, o)?);
            }
            reply_with_copies(
                interface_id,
                done.code,
                &done.data,
                &done.copies,
                &[],
                Some(&ids),
            )
        }
        None => {
            let mut handles = Vec::new();
            for o in done.objects {
                match ipc::open_session(k, Endpoint::Host(o)) {
                    Ok(h) => handles.push(h),
                    Err(e) => return Ok(Outcome::Reply(reply(interface_id, e, &[], &[], None))),
                }
            }
            reply_with_copies(
                interface_id,
                done.code,
                &done.data,
                &done.copies,
                &handles,
                None,
            )
        }
    }))
}

/// The event a hosted service keeps in `slot`, created on first use.
fn host_event(
    k: &mut Kernel,
    slot: impl Fn(&mut HostState) -> &mut Option<usize>,
    signaled: bool,
) -> usize {
    if let Some(e) = *slot(&mut k.host) {
        return e;
    }
    k.events.push(threads::Event {
        signaled,
        ..threads::Event::default()
    });
    let e = k.events.len() - 1;
    *slot(&mut k.host) = Some(e);
    e
}

/// A reply copying a readable handle of event `e`.
fn copy_event(k: &mut Kernel, e: usize) -> Done {
    match threads::readable_event_handle(k, e) {
        Ok(h) => Done {
            copies: vec![h],
            ..Done::ok()
        },
        Err(code) => Done::failure(code),
    }
}

fn make_parcel(data: &[u8], objs: &[u8]) -> Vec<u8> {
    let mut p = Vec::with_capacity(16 + data.len() + objs.len());
    p.extend_from_slice(&(data.len() as u32).to_le_bytes());
    p.extend_from_slice(&0x10u32.to_le_bytes());
    p.extend_from_slice(&(objs.len() as u32).to_le_bytes());
    p.extend_from_slice(&((data.len() + 0x10) as u32).to_le_bytes());
    p.extend_from_slice(data);
    p.extend_from_slice(objs);
    p
}

fn make_native_window_parcel() -> Vec<u8> {
    let mut data = Vec::with_capacity(40);
    data.extend_from_slice(&2u32.to_le_bytes()); // Type: BINDER_TYPE_WEAK_BINDER
    data.extend_from_slice(&0u32.to_le_bytes()); // Flags: 0
    data.extend_from_slice(&0x20u64.to_le_bytes()); // Id: 0x20
    data.extend_from_slice(&0u64.to_le_bytes()); // Cookie: 0
    data.extend_from_slice(b"dispdrv\0"); // Name: dispdrv
    data.extend_from_slice(&0u64.to_le_bytes()); // Pad: 0
    make_parcel(&data, &[0, 0, 0, 0])
}

/// Obtains or creates the shared memory object backing time:u GetSharedMemoryNativeHandle.
fn get_or_create_time_shared_memory(k: &mut Kernel) -> Result<u32, u32> {
    let idx = match k.host.time_shared_memory {
        Some(idx) => idx,
        None => {
            let mut bytes = vec![0u8; 4096];
            let clock_source_id: [u8; 16] = [
                0x57, 0x4c, 0x54, 0x4b, 0x11, 0x22, 0x33, 0x44, 0x55, 0x66, 0x77, 0x88, 0x99, 0xaa,
                0xbb, 0xcc,
            ];
            // 1. SteadyClockContext at offset 0x00
            bytes[0..4].copy_from_slice(&2u32.to_le_bytes());
            bytes[0x08..0x10].copy_from_slice(&0u64.to_le_bytes());
            bytes[0x10..0x20].copy_from_slice(&clock_source_id);
            bytes[0x20..0x28].copy_from_slice(&0u64.to_le_bytes());
            bytes[0x28..0x38].copy_from_slice(&clock_source_id);

            // 2. LocalSystemClockContext at offset 0x38
            bytes[0x38..0x3c].copy_from_slice(&2u32.to_le_bytes());
            let posix_epoch = 1700000000i64;
            bytes[0x40..0x48].copy_from_slice(&posix_epoch.to_le_bytes());
            bytes[0x48..0x50].copy_from_slice(&0u64.to_le_bytes());
            bytes[0x50..0x60].copy_from_slice(&clock_source_id);
            bytes[0x60..0x68].copy_from_slice(&posix_epoch.to_le_bytes());
            bytes[0x68..0x70].copy_from_slice(&0u64.to_le_bytes());
            bytes[0x70..0x80].copy_from_slice(&clock_source_id);

            // 3. NetworkSystemClockContext at offset 0x80
            bytes[0x80..0x84].copy_from_slice(&2u32.to_le_bytes());
            bytes[0x88..0x90].copy_from_slice(&posix_epoch.to_le_bytes());
            bytes[0x90..0x98].copy_from_slice(&0u64.to_le_bytes());
            bytes[0x98..0xa8].copy_from_slice(&clock_source_id);
            bytes[0xa8..0xb0].copy_from_slice(&posix_epoch.to_le_bytes());
            bytes[0xb0..0xb8].copy_from_slice(&0u64.to_le_bytes());
            bytes[0xb8..0xc8].copy_from_slice(&clock_source_id);

            // 4. AutomaticCorrectionEnabled at offset 0xc8
            bytes[0xc8..0xcc].copy_from_slice(&2u32.to_le_bytes());

            k.shared_memories.push(horizon::SharedMemory {
                size: 0x1000,
                permission: 1, // Read-only for guest
                bytes,
            });
            let idx = k.shared_memories.len() - 1;
            k.host.time_shared_memory = Some(idx);
            idx
        }
    };
    k.handles.add(horizon::Object::SharedMemory(idx))
}

/// Obtains or creates the shared memory object backing hid IAppletResource GetSharedMemoryHandle.
fn get_or_create_hid_shared_memory(k: &mut Kernel) -> Result<u32, u32> {
    let idx = match k.host.hid_shared_memory {
        Some(idx) => idx,
        None => {
            let bytes = vec![0u8; 0x40000];
            k.shared_memories.push(horizon::SharedMemory {
                size: 0x40000,
                permission: 1, // Read-only for guest
                bytes,
            });
            let idx = k.shared_memories.len() - 1;
            k.host.hid_shared_memory = Some(idx);
            idx
        }
    };
    k.handles.add(horizon::Object::SharedMemory(idx))
}

const LOG_PACKET_HEADER_SIZE: usize = 0x18;
const LOG_CHUNK_TEXT: u64 = 2;

fn uleb128(bytes: &[u8], at: &mut usize) -> Option<u64> {
    let mut value = 0u64;
    for shift in (0..64).step_by(7) {
        let b = *bytes.get(*at)?;
        *at += 1;
        value |= u64::from(b & 0x7f) << shift;
        if b & 0x80 == 0 {
            return Some(value);
        }
    }
    None
}

/// ILogger::Log: a packet whose size disagrees with its header is ignored with success, as
/// in `lm`.
fn log(k: &mut Kernel, _process_id: u64, packet: &[u8]) {
    if packet.len() < LOG_PACKET_HEADER_SIZE {
        return;
    }
    let payload = ipc::read_u32(packet, 0x14) as usize;
    if LOG_PACKET_HEADER_SIZE + payload != packet.len() {
        return;
    }
    k.host.log_packets += 1;
    let thread_id = u64::from_le_bytes(packet[8..16].try_into().unwrap_or_default());
    let flags = packet[0x10];
    let (head, tail) = (flags & 1 != 0, flags & 2 != 0);
    let mut text = String::new();
    let body = &packet[LOG_PACKET_HEADER_SIZE..];
    let mut at = 0;
    while at < body.len() {
        let (Some(key), Some(size)) = (uleb128(body, &mut at), uleb128(body, &mut at)) else {
            break;
        };
        let Some(data) = body.get(at..at.saturating_add(size as usize)) else {
            break;
        };
        at += data.len();
        if key == LOG_CHUNK_TEXT {
            text.push_str(&String::from_utf8_lossy(data));
        }
    }
    let partial = &mut k.host.partial_logs;
    let slot = partial.iter().position(|(t, _)| *t == thread_id);
    let mut message = match slot {
        Some(i) if !head => partial.remove(i).1,
        Some(i) => {
            partial.remove(i);
            String::new()
        }
        None => String::new(),
    };
    message.push_str(&text);
    if tail {
        let line = message.trim_end_matches(['\n', '\0']);
        if !line.is_empty() {
            k.debug_output.push(line.to_string());
        }
    } else {
        partial.push((thread_id, message));
    }
}
