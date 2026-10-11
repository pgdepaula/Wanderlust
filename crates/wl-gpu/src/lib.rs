//! Checked graphics execution, bounded Maxwell texture state and constant fragments.
#![forbid(unsafe_code)]

#[cfg(feature = "wgpu-backend")]
pub mod backend;
pub mod command;
pub mod compression;
pub mod fifo;
pub mod format;
pub mod layout;
pub mod maxwell;
pub mod memory;
pub mod shader;
pub mod texture;
#[cfg(feature = "wgpu-backend")]
mod texture_backend;
#[cfg(feature = "wgpu-backend")]
pub use texture_backend::TextureUploadPolicy;

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
    pub guest_texture_bytes: u64,
    pub deswizzle_bytes: u64,
    pub decompress_bytes: u64,
    pub temporary_allocations: u64,
    pub texture_uploads: u64,
    pub texture_cache_hits: u64,
    pub texture_cache_misses: u64,
    pub dirty_mip_uploads: u64,
    pub pipeline_allocations: u64,
    pub shader_module_allocations: u64,
    pub shader_cache_hits: u64,
}
