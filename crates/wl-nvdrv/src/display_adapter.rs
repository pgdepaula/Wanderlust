//! Compiled in the generated executable beside native_gpu and host_binder.
//! One host-thread-confined VI producer consumes the SAME NVIDIA driver.
use crate::{
    host_binder::{self, QueuedBuffer},
    native_gpu::{NativeHost, SharedHost},
};
use std::{
    cell::RefCell,
    rc::{Rc, Weak},
};
use wl_guest_runtime::{
    Memory, RuntimeResult,
    display::{Framebuffer, PixelFormat},
};
thread_local! {
    static OWNER:RefCell<Weak<RefCell<NativeHost>>>=const {RefCell::new(Weak::new())};
}
pub fn new_host() -> wl_gpu::Result<SharedHost> {
    if OWNER.with(|p| p.borrow().upgrade().is_some()) {
        return Err(wl_gpu::Error("VI GPU owner already exists".into()));
    }
    let host = Rc::new(RefCell::new(NativeHost::new()?));
    host_binder::install_consumer(host_binder::Consumer { acquire, present })
        .map_err(wl_gpu::Error)?;
    OWNER.with(|p| *p.borrow_mut() = Rc::downgrade(&host));
    Ok(SharedHost(host))
}
fn owner() -> RuntimeResult<Rc<RefCell<NativeHost>>> {
    OWNER
        .with(|p| p.borrow().upgrade())
        .ok_or_else(|| "VI GPU owner closed".into())
}
fn word(b: &[u8], n: usize) -> RuntimeResult<u32> {
    Ok(u32::from_le_bytes(
        b.get(n..n + 4)
            .ok_or("short VI queue input")?
            .try_into()
            .unwrap(),
    ))
}
fn acquire(memory: &mut Memory, q: &QueuedBuffer) -> RuntimeResult<Option<Framebuffer>> {
    let b = &q.buffer;
    let input = &q.input;
    let crop = [
        word(input, 12)?,
        word(input, 16)?,
        word(input, 20)?,
        word(input, 24)?,
    ];
    if input.len() != 84
        || word(input, 8)? > 1
        || word(input, 28)? != 0
        || word(input, 32)? != 0
        || !matches!(word(input, 36)?, 0 | 8)
        || !matches!(crop, [0, 0, 0, 0]) && crop != [0, 0, b.width, b.height]
    {
        return Err("unsupported VI crop/scaling/transform".into());
    }
    // Sticky inverse-display transformation is identity for this adapter's
    // explicitly unrotated display (transform hint zero).
    let owner = owner()?;
    let host = owner
        .try_borrow()
        .map_err(|_| "reentrant VI GPU acquisition")?;
    let count = word(input, 48)?;
    if count > 4 {
        return Err("VI acquire fence count".into());
    }
    for i in 0..count as usize {
        if !host
            .display_fence_complete(word(input, 52 + i * 8)?, word(input, 56 + i * 8)?)
            .map_err(|e| format!("VI acquire fence: {e:?}"))?
        {
            return Ok(None);
        }
    }
    let surface = b.native_surface()?;
    let pitch = b.stride.checked_mul(4).ok_or("VI pitch overflow")?;
    if surface.pitch != pitch {
        return Err("VI native pitch/stride mismatch".into());
    }
    let address = host
        .visible_display_address(surface.nvmap_id, surface.offset, surface.size)
        .map_err(|e| format!("VI canonical backing: {e:?}"))?;
    let source = memory.read_bytes(address.0, surface.size)?;
    let linear = match surface.layout {
        3 => {
            let layout = wl_gpu::layout::BlockLinear2d::new(
                b.stride,
                b.height,
                wl_gpu::format::Format::Rgba8Unorm,
                surface.block_height_log2,
            )
            .map_err(|e| e.to_string())?;
            layout
                .to_linear(source, &mut wl_gpu::Counters::default())
                .map_err(|e| e.to_string())?
        }
        1 if surface.block_height_log2 == 0 => {
            let size = (pitch as usize)
                .checked_mul(b.height as usize)
                .ok_or("VI pitch footprint overflow")?;
            source
                .get(..size)
                .ok_or("truncated VI pitch image")?
                .to_vec()
        }
        _ => return Err("unsupported VI native layout".into()),
    };
    let rgba = if b.stride == b.width {
        linear
    } else {
        let mut packed = Vec::with_capacity(b.width as usize * b.height as usize * 4);
        for row in linear.chunks_exact(pitch as usize).take(b.height as usize) {
            packed.extend_from_slice(&row[..b.width as usize * 4]);
        }
        packed
    };
    Ok(Some(Framebuffer::new(
        b.width,
        b.height,
        b.width,
        PixelFormat::Rgba8888,
        rgba,
    )))
}
fn present(frame: &Framebuffer) -> RuntimeResult<()> {
    let owner = owner()?;
    let mut host = owner
        .try_borrow_mut()
        .map_err(|_| "reentrant VI GPU presentation")?;
    host.present_vi_buffer(frame)
        .map_err(|e| format!("VI presentation: {e:?}"))
}
