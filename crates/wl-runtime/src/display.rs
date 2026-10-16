//! Display presentation: Surface and Framebuffer management for Horizon VI
//! and HosBinderDriver buffer queueing.

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

    /// Converts the pixel buffer to 32-bit RGBA (standard host presentation format).
    pub fn to_rgba8888(&self) -> Vec<u8> {
        let count = (self.width * self.height) as usize;
        let mut out = vec![0u8; count * 4];

        match self.format {
            PixelFormat::Rgba8888 | PixelFormat::Rgbx8888 => {
                for y in 0..self.height as usize {
                    let src_row = y * self.stride as usize * 4;
                    let dst_row = y * self.width as usize * 4;
                    let row_bytes = (self.width as usize * 4).min(self.pixels.len().saturating_sub(src_row));
                    if src_row + row_bytes <= self.pixels.len() && dst_row + row_bytes <= out.len() {
                        out[dst_row..dst_row + row_bytes].copy_from_slice(&self.pixels[src_row..src_row + row_bytes]);
                    }
                }
            }
            PixelFormat::Bgra8888 => {
                for y in 0..self.height as usize {
                    for x in 0..self.width as usize {
                        let src_idx = (y * self.stride as usize + x) * 4;
                        let dst_idx = (y * self.width as usize + x) * 4;
                        if src_idx + 4 <= self.pixels.len() && dst_idx + 4 <= out.len() {
                            let b = self.pixels[src_idx];
                            let g = self.pixels[src_idx + 1];
                            let r = self.pixels[src_idx + 2];
                            let a = self.pixels[src_idx + 3];
                            out[dst_idx] = r;
                            out[dst_idx + 1] = g;
                            out[dst_idx + 2] = b;
                            out[dst_idx + 3] = a;
                        }
                    }
                }
            }
            PixelFormat::Rgb565 => {
                for y in 0..self.height as usize {
                    for x in 0..self.width as usize {
                        let src_idx = (y * self.stride as usize + x) * 2;
                        let dst_idx = (y * self.width as usize + x) * 4;
                        if src_idx + 2 <= self.pixels.len() && dst_idx + 4 <= out.len() {
                            let val = u16::from_le_bytes([self.pixels[src_idx], self.pixels[src_idx + 1]]);
                            let r = (((val >> 11) & 0x1f) * 255 / 31) as u8;
                            let g = (((val >> 5) & 0x3f) * 255 / 63) as u8;
                            let b = ((val & 0x1f) * 255 / 31) as u8;
                            out[dst_idx] = r;
                            out[dst_idx + 1] = g;
                            out[dst_idx + 2] = b;
                            out[dst_idx + 3] = 255;
                        }
                    }
                }
            }
            PixelFormat::Unknown => {
                let to_copy = self.pixels.len().min(out.len());
                out[..to_copy].copy_from_slice(&self.pixels[..to_copy]);
            }
        }
        out
    }

    /// Writes the current frame in PPM (Portable Pixmap) format.
    pub fn to_ppm(&self) -> Vec<u8> {
        let rgba = self.to_rgba8888();
        let header = format!("P6\n{} {}\n255\n", self.width, self.height);
        let mut out = header.into_bytes();
        out.reserve((self.width * self.height * 3) as usize);
        for chunk in rgba.chunks_exact(4) {
            out.push(chunk[0]); // R
            out.push(chunk[1]); // G
            out.push(chunk[2]); // B
        }
        out
    }
}

/// Host display presentation state.
#[derive(Debug)]
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
