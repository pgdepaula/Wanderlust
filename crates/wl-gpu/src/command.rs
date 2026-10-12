//! Host-independent execution commands, not Maxwell method packets.
use crate::texture::TextureId;
use crate::{Result, invalid, memory::DirtyRanges};
use std::collections::HashMap;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct BufferId(pub u32);
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BufferKind {
    Vertex,
    Index,
}

/// Vertex layout is little-endian float32 x,y,r,g,b,a (24 bytes); indices are u16.
#[derive(Debug, Clone)]
pub enum Command<'a> {
    CreateBuffer {
        id: BufferId,
        size: u64,
        kind: BufferKind,
    },
    Upload {
        id: BufferId,
        offset: u64,
        data: &'a [u8],
    },
    DrawIndexed {
        vertex: BufferId,
        index: BufferId,
        index_count: u32,
    },
    DestroyBuffer {
        id: BufferId,
    },
    /// Sample one resident mip through the texture pipeline. Texture existence is
    /// checked by the backend; buffer-only preflight enforces queue-write ordering.
    /// Fullscreen fragment validation harness. Resources must be synchronized from
    /// B197 state; this is not a hardware draw-method implementation.
    DrawFragment,
    /// Translated vertex/fragment validation boundary. Hardware methods are
    /// decoded separately; callers must not describe this command as a B197 method.
    DrawVertices {
        first: u32,
        count: u32,
    },
    DrawGuestIndices {
        count: u32,
        base: i32,
    },
    DrawTexture {
        id: TextureId,
        mip: u32,
    },
}

#[derive(Debug, Clone)]
pub struct BufferState {
    pub size: u64,
    pub kind: BufferKind,
    pub initialized: DirtyRanges,
}
pub type ResourceState = HashMap<BufferId, BufferState>;

/// Preflight before any backend side effects. Pending uploads may precede a draw;
/// an upload after a draw in the same submission is rejected (queue writes run first).
pub fn validate(
    commands: &[Command<'_>],
    existing: &ResourceState,
    max_buffer_size: u64,
) -> Result<ResourceState> {
    let mut state = existing.clone();
    let mut drew = false;
    for cmd in commands {
        match cmd {
            Command::CreateBuffer { id, size, kind } => {
                if *size == 0 || size % 4 != 0 || *size > max_buffer_size || state.contains_key(id)
                {
                    return Err(invalid("invalid/duplicate buffer descriptor"));
                }
                state.insert(
                    *id,
                    BufferState {
                        size: *size,
                        kind: *kind,
                        initialized: DirtyRanges::default(),
                    },
                );
            }
            Command::Upload { id, offset, data } => {
                let resource = state
                    .get_mut(id)
                    .ok_or_else(|| invalid("unknown upload buffer"))?;
                let end = offset
                    .checked_add(data.len() as u64)
                    .ok_or_else(|| invalid("upload overflow"))?;
                if drew
                    || data.is_empty()
                    || offset % 4 != 0
                    || data.len() % 4 != 0
                    || end > resource.size
                {
                    return Err(invalid("unaligned/out-of-bounds/late upload"));
                }
                resource.initialized.insert(*offset..end)?;
            }
            Command::DrawIndexed {
                vertex,
                index,
                index_count,
            } => {
                let v = state
                    .get(vertex)
                    .ok_or_else(|| invalid("unknown vertex buffer"))?;
                let i = state
                    .get(index)
                    .ok_or_else(|| invalid("unknown index buffer"))?;
                let bytes = u64::from(*index_count) * 2;
                if *index_count == 0
                    || index_count % 3 != 0
                    || v.kind != BufferKind::Vertex
                    || i.kind != BufferKind::Index
                    || v.size % 24 != 0
                    || !v.initialized.covers(0..v.size)
                    || bytes > i.size
                    || !i.initialized.covers(0..bytes)
                {
                    return Err(invalid("invalid draw bindings or uninitialized data"));
                }
                drew = true;
            }
            Command::DestroyBuffer { id } => {
                state
                    .remove(id)
                    .ok_or_else(|| invalid("destroy unknown buffer"))?;
            }
            Command::DrawFragment
            | Command::DrawTexture { .. }
            | Command::DrawVertices { .. }
            | Command::DrawGuestIndices { .. } => {
                drew = true;
            }
        }
    }
    Ok(state)
}

/// Synthetic vertices: two full-screen rectangles with different constant colors.
pub fn test_geometry() -> (Vec<u8>, Vec<u8>) {
    let mut vertices = Vec::with_capacity(8 * 24);
    for (left, right, color) in [
        (-1.0_f32, 0.0_f32, [1.0_f32, 0.0, 0.0, 1.0]),
        (0.0, 1.0, [0.0, 1.0, 0.0, 1.0]),
    ] {
        for [x, y] in [[left, -1.0], [right, -1.0], [right, 1.0], [left, 1.0]] {
            for f in [x, y, color[0], color[1], color[2], color[3]] {
                vertices.extend_from_slice(&f.to_le_bytes());
            }
        }
    }
    let indices = [0_u16, 1, 2, 0, 2, 3, 4, 5, 6, 4, 6, 7]
        .into_iter()
        .flat_map(u16::to_le_bytes)
        .collect();
    (vertices, indices)
}
