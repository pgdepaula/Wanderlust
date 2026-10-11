//! Checked graphics execution and Maxwell method packet decoding. No shader decoder.
#![forbid(unsafe_code)]

#[cfg(feature = "wgpu-backend")]
pub mod backend;
pub mod command;
pub mod fifo;
pub mod format;
pub mod layout;
pub mod memory;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Error(pub String);
impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        self.0.fmt(f)
    }
}
impl std::error::Error for Error {}
pub type Result<T> = std::result::Result<T, Error>;
pub(crate) fn invalid(message: &str) -> Error {
    Error(message.into())
}

/// Explicit operations observed by this crate; excludes driver/internal allocations.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct Counters {
    pub owned_output_allocations: u64,
    pub guest_to_host_bytes: u64,
    pub upload_bytes: u64,
    pub readback_bytes: u64,
    pub readback_host_copy_bytes: u64,
    pub buffer_allocations: u64,
    pub texture_allocations: u64,
    pub staging_allocations: u64,
    pub upload_calls: u64,
    pub buffer_reuploads: u64,
    pub texture_conversions: u64,
    pub cache_hits: u64,
    pub cache_misses: u64,
    pub submissions: u64,
    pub synchronization_points: u64,
}
