//! Checked graphics execution, bounded Maxwell texture state and constant fragments.
#![forbid(unsafe_code)]

#[cfg(feature = "wgpu-backend")]
pub mod backend;
pub mod command;
#[cfg(feature = "wgpu-backend")]
mod completion;
pub mod compression;
#[cfg(feature = "wgpu-backend")]
mod depth_backend;
#[cfg(feature = "wgpu-backend")]
mod draw_backend;
pub mod driver;
#[cfg(feature = "wgpu-backend")]
pub mod driver_backend;
pub mod fifo;
pub mod format;
pub mod gpfifo;
#[cfg(feature = "wgpu-backend")]
mod gpfifo_backend;
pub mod index;
#[cfg(feature = "wgpu-backend")]
mod index_backend;
pub mod initial_capture;
pub mod layout;
pub mod maxwell;
pub mod memory;
pub mod mme;
pub mod raster;
pub mod shader;
#[cfg(feature = "wgpu-backend")]
mod shader_backend;
pub mod shader_program;
pub mod texture;
#[cfg(feature = "wgpu-backend")]
mod texture_backend;
#[cfg(feature = "wgpu-backend")]
mod vertex_backend;
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
    pub gpfifo_submissions: u64,
    pub gpfifo_entry_decodes: u64,
    pub gpfifo_read_calls: u64,
    /// Borrowed spans may overlap when execution resumes after a draw.
    pub gpfifo_borrowed_bytes: u64,
    /// Words actually consumed, four scalar bytes per word; no owned command array.
    pub gpfifo_command_bytes: u64,
    pub owned_output_allocations: u64,
    pub guest_to_host_bytes: u64,
    pub upload_bytes: u64,
    pub readback_bytes: u64,
    pub readback_operations: u64,
    pub coherence_barriers: u64,
    pub gpu_target_writes: u64,
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
    pub completion_timeouts: u64,
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
    pub constant_upload_bytes: u64,
    pub constant_guest_bytes: u64,
    pub bind_group_allocations: u64,
    pub bind_group_reuses: u64,
    pub shader_decodes: u64,
    pub shader_binary_bytes: u64,
    pub descriptor_reads: u64,
    pub draw_commands: u64,
    pub clear_commands: u64,
    pub index_guest_bytes: u64,
    pub index_upload_bytes: u64,
    pub index_cache_hits: u64,
    pub vertex_guest_bytes: u64,
    pub vertex_upload_bytes: u64,
    pub vertex_shader_bytes: u64,
    pub vertex_shader_decodes: u64,
    pub depth_allocations: u64,
    pub depth_upload_bytes: u64,
    pub depth_guest_bytes: u64,
    pub depth_write_bytes: u64,
    pub render_target_allocations: u64,
    pub render_target_reuses: u64,
    pub render_target_guest_bytes: u64,
    pub target_deswizzle_bytes: u64,
    pub target_swizzle_bytes: u64,
    pub target_pack_copy_bytes: u64,
    pub target_patch_bytes: u64,
    pub render_target_upload_bytes: u64,
    pub guest_framebuffer_write_bytes: u64,
    /// Inline B197 constant words committed to canonical guest memory.
    pub constant_buffer_write_bytes: u64,
    /// B197 one-row pitch inline words committed to canonical guest memory.
    pub inline_upload_write_bytes: u64,
}
