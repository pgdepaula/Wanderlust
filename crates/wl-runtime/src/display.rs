//! Display presentation: Surface and Framebuffer management for Horizon VI
//! and HosBinderDriver buffer queueing.

/// Maximum packed RGBA bytes accepted by the host presentation converter.
pub const MAX_RGBA_BYTES: usize = 64 * 1024 * 1024;

/// Pixel format enumeration matching Android / Horizon GraphicBuffer formats.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u32)]
pub enum PixelFormat {
    Rgba8888 = 1,
    Rgbx8888 = 2,
    Rgb565 = 4,
    Bgra8888 = 5,
    Unknown = 0,
}

impl From<u32> for PixelFormat {
    fn from(val: u32) -> Self {
        match val {
            1 => Self::Rgba8888,
            2 => Self::Rgbx8888,
            4 => Self::Rgb565,
            5 => Self::Bgra8888,
            _ => Self::Unknown,
        }
    }
}

/// A presented display framebuffer.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Framebuffer {
    pub width: u32,
    pub height: u32,
    pub stride: u32,
    pub format: PixelFormat,
    pub pixels: Vec<u8>,
}

impl Framebuffer {
    pub fn new(width: u32, height: u32, stride: u32, format: PixelFormat, pixels: Vec<u8>) -> Self {
        Self {
            width,
            height,
            stride,
            format,
            pixels,
        }
    }

    /// Validates the guest geometry and converts to packed host RGBA bytes.
    /// The budget is a frontend allocation limit, not a Maxwell format limit.
    pub fn to_rgba8888(&self) -> Result<Vec<u8>, &'static str> {
        if self.width == 0 || self.height == 0 || self.stride < self.width {
            return Err("invalid framebuffer geometry");
        }
        let width = usize::try_from(self.width).map_err(|_| "framebuffer width overflow")?;
        let height = usize::try_from(self.height).map_err(|_| "framebuffer height overflow")?;
        let output_len = width
            .checked_mul(height)
            .and_then(|n| n.checked_mul(4))
            .filter(|n| *n <= MAX_RGBA_BYTES)
            .ok_or("framebuffer allocation budget exceeded")?;
        let bpp = match self.format {
            PixelFormat::Rgb565 => 2,
            PixelFormat::Rgba8888 | PixelFormat::Rgbx8888 | PixelFormat::Bgra8888 => 4,
            PixelFormat::Unknown => return Err("unsupported framebuffer pixel format"),
        };
        let pitch = usize::try_from(self.stride)
            .ok()
            .and_then(|n| n.checked_mul(bpp))
            .ok_or("framebuffer pitch overflow")?;
        let row_bytes = width.checked_mul(bpp).ok_or("framebuffer row overflow")?;
        let required = (height - 1)
            .checked_mul(pitch)
            .and_then(|n| n.checked_add(row_bytes))
            .ok_or("framebuffer source range overflow")?;
        if self.pixels.len() < required {
            return Err("truncated framebuffer storage");
        }
        let mut out = vec![0; output_len];
        for y in 0..height {
            let src = &self.pixels[y * pitch..y * pitch + row_bytes];
            let dst = &mut out[y * width * 4..(y + 1) * width * 4];
            match self.format {
                PixelFormat::Rgba8888 | PixelFormat::Rgbx8888 => {
                    dst.copy_from_slice(src);
                    if self.format == PixelFormat::Rgbx8888 {
                        for pixel in dst.chunks_exact_mut(4) {
                            pixel[3] = 255;
                        }
                    }
                }
                PixelFormat::Bgra8888 => {
                    for (src, dst) in src.chunks_exact(4).zip(dst.chunks_exact_mut(4)) {
                        dst.copy_from_slice(&[src[2], src[1], src[0], src[3]]);
                    }
                }
                PixelFormat::Rgb565 => {
                    for (src, dst) in src.chunks_exact(2).zip(dst.chunks_exact_mut(4)) {
                        let value = u16::from_le_bytes([src[0], src[1]]);
                        dst.copy_from_slice(&[
                            (((value >> 11) & 31) * 255 / 31) as u8,
                            (((value >> 5) & 63) * 255 / 63) as u8,
                            ((value & 31) * 255 / 31) as u8,
                            255,
                        ]);
                    }
                }
                PixelFormat::Unknown => unreachable!("format checked before allocation"),
            }
        }
        Ok(out)
    }

    /// Writes a validated frame as a binary RGB PPM.
    pub fn to_ppm(&self) -> Result<Vec<u8>, &'static str> {
        let rgba = self.to_rgba8888()?;
        let header = format!("P6\n{} {}\n255\n", self.width, self.height);
        let mut out = header.into_bytes();
        out.reserve(rgba.len() / 4 * 3);
        for pixel in rgba.chunks_exact(4) {
            out.extend_from_slice(&pixel[..3]);
        }
        Ok(out)
    }
}

/// Host display presentation state.
#[derive(Clone, Debug)]
pub struct DisplayServer {
    pub width: u32,
    pub height: u32,
    pub current_frame: Option<Framebuffer>,
    pub frame_count: u64,
}

impl Default for DisplayServer {
    fn default() -> Self {
        Self {
            width: 1280,
            height: 720,
            current_frame: None,
            frame_count: 0,
        }
    }
}

impl DisplayServer {
    pub fn new(width: u32, height: u32) -> Self {
        Self {
            width,
            height,
            current_frame: None,
            frame_count: 0,
        }
    }

    /// Enqueues a new rendered frame buffer.
    pub fn queue_buffer(
        &mut self,
        width: u32,
        height: u32,
        stride: u32,
        format: u32,
        pixels: Vec<u8>,
    ) {
        self.current_frame = Some(Framebuffer::new(
            width,
            height,
            stride,
            PixelFormat::from(format),
            pixels,
        ));
        self.frame_count += 1;
    }
}
