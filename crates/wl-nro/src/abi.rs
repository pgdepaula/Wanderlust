use crate::error::NroError;
use serde::Serialize;
use wl_common::GuestAddr;

/// Homebrew loader sentinel passed in X1 at NRO0 entry point.
pub const HOMEBREW_LOADER_SENTINEL: u64 = 0xFFFF_FFFF_FFFF_FFFF;

/// Typed representation of execution entrypoint arguments and ABI conventions.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
pub enum EntryConvention {
    /// Standard Horizon retail application startup (NSO0 / rtld).
    /// - X0 receives NULL or initial process context pointer.
    /// - X1 receives the main thread handle.
    /// - Exit is strictly performed via `svcExitProcess`.
    RetailNso {
        x0_context: u64,
        x1_main_thread_handle: u64,
    },

    /// Nintendo Switch Homebrew ABI application startup (NRO0).
    /// - X0 receives pointer to the loader configuration table (or NULL if standalone).
    /// - X1 receives the loader sentinel `0xFFFFFFFFFFFFFFFF`.
    /// - Original LR carries a return address to the homebrew loader/menu.
    HomebrewNro {
        x0_loader_config: GuestAddr,
        x1_sentinel: u64,
        lr_returnable: bool,
    },
}

impl EntryConvention {
    pub fn default_homebrew(loader_config_addr: GuestAddr) -> Self {
        Self::HomebrewNro {
            x0_loader_config: loader_config_addr,
            x1_sentinel: HOMEBREW_LOADER_SENTINEL,
            lr_returnable: true,
        }
    }

    pub fn default_retail(main_thread_handle: u64) -> Self {
        Self::RetailNso {
            x0_context: 0,
            x1_main_thread_handle: main_thread_handle,
        }
    }
}

/// Known Homebrew ABI loader configuration keys (Switchbrew Homebrew ABI).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[repr(u32)]
pub enum LoaderConfigKey {
    EndOfList = 0,
    MainThreadHandle = 1,
    NextLoadPath = 2,
    OverrideHeap = 3,
    OverrideService = 4,
    Argv = 5,
    SyscallAvailableHint = 6,
    AppletType = 7,
    AppletWorkaround = 8,
    Reserved9 = 9,
    ProcessHandle = 10,
    LastLoadResult = 11,
    AllocPages = 12,
    LockRegion = 13,
    RandomSeed = 14,
    UserIdStorage = 15,
    HosVersion = 16,
    SyscallAvailableHint2 = 17,
    Unknown(u32),
}

impl From<u32> for LoaderConfigKey {
    fn from(raw: u32) -> Self {
        match raw {
            0 => Self::EndOfList,
            1 => Self::MainThreadHandle,
            2 => Self::NextLoadPath,
            3 => Self::OverrideHeap,
            4 => Self::OverrideService,
            5 => Self::Argv,
            6 => Self::SyscallAvailableHint,
            7 => Self::AppletType,
            8 => Self::AppletWorkaround,
            9 => Self::Reserved9,
            10 => Self::ProcessHandle,
            11 => Self::LastLoadResult,
            12 => Self::AllocPages,
            13 => Self::LockRegion,
            14 => Self::RandomSeed,
            15 => Self::UserIdStorage,
            16 => Self::HosVersion,
            17 => Self::SyscallAvailableHint2,
            other => Self::Unknown(other),
        }
    }
}

impl From<LoaderConfigKey> for u32 {
    fn from(key: LoaderConfigKey) -> Self {
        match key {
            LoaderConfigKey::EndOfList => 0,
            LoaderConfigKey::MainThreadHandle => 1,
            LoaderConfigKey::NextLoadPath => 2,
            LoaderConfigKey::OverrideHeap => 3,
            LoaderConfigKey::OverrideService => 4,
            LoaderConfigKey::Argv => 5,
            LoaderConfigKey::SyscallAvailableHint => 6,
            LoaderConfigKey::AppletType => 7,
            LoaderConfigKey::AppletWorkaround => 8,
            LoaderConfigKey::Reserved9 => 9,
            LoaderConfigKey::ProcessHandle => 10,
            LoaderConfigKey::LastLoadResult => 11,
            LoaderConfigKey::AllocPages => 12,
            LoaderConfigKey::LockRegion => 13,
            LoaderConfigKey::RandomSeed => 14,
            LoaderConfigKey::UserIdStorage => 15,
            LoaderConfigKey::HosVersion => 16,
            LoaderConfigKey::SyscallAvailableHint2 => 17,
            LoaderConfigKey::Unknown(other) => other,
        }
    }
}

/// Single tuple entry within the Homebrew ABI loader configuration table (24 bytes).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
pub struct LoaderConfigEntry {
    pub key: LoaderConfigKey,
    pub flags: u32,
    pub value: [u64; 2],
}

impl LoaderConfigEntry {
    pub const SIZE: usize = 24;

    pub fn new(key: LoaderConfigKey, flags: u32, value0: u64, value1: u64) -> Self {
        Self {
            key,
            flags,
            value: [value0, value1],
        }
    }

    pub fn is_mandatory(&self) -> bool {
        (self.flags & 1) != 0
    }
}

/// In-memory representation of a Homebrew ABI Loader Configuration table.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize)]
pub struct LoaderConfig {
    pub entries: Vec<LoaderConfigEntry>,
}

impl LoaderConfig {
    /// Parses a sequence of 24-byte LoaderConfigEntry structures up to `EndOfList`.
    pub fn parse(bytes: &[u8]) -> Result<Self, NroError> {
        let mut entries = Vec::new();
        let mut offset = 0;

        while offset + LoaderConfigEntry::SIZE <= bytes.len() {
            let key_raw = u32::from_le_bytes(bytes[offset..offset + 4].try_into().unwrap());
            let flags = u32::from_le_bytes(bytes[offset + 4..offset + 8].try_into().unwrap());
            let val0 = u64::from_le_bytes(bytes[offset + 8..offset + 16].try_into().unwrap());
            let val1 = u64::from_le_bytes(bytes[offset + 16..offset + 24].try_into().unwrap());

            let key = LoaderConfigKey::from(key_raw);
            entries.push(LoaderConfigEntry {
                key,
                flags,
                value: [val0, val1],
            });

            offset += LoaderConfigEntry::SIZE;

            if key == LoaderConfigKey::EndOfList {
                break;
            }
        }

        Ok(Self { entries })
    }

    /// Serializes the loader config entries into a contiguous byte vector.
    pub fn to_bytes(&self) -> Vec<u8> {
        let mut bytes = Vec::with_capacity(self.entries.len() * LoaderConfigEntry::SIZE);
        for entry in &self.entries {
            let key_raw: u32 = entry.key.into();
            bytes.extend_from_slice(&key_raw.to_le_bytes());
            bytes.extend_from_slice(&entry.flags.to_le_bytes());
            bytes.extend_from_slice(&entry.value[0].to_le_bytes());
            bytes.extend_from_slice(&entry.value[1].to_le_bytes());
        }
        bytes
    }
}
