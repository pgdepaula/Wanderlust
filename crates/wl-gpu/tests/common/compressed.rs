// Google astc-codec testdata fragment; see ../fixtures/NOTICE and LICENSE-APACHE.
pub const ASTC_REFERENCE: [u8; 16] = [
    0xbd, 0x27, 0x39, 0x6e, 0x5f, 0x03, 0xcd, 0x4e, 0x18, 0x04, 0x40, 0x25, 0x92, 0xf4, 0x7b, 0x1f,
];
pub const ASTC_PIXELS: [u8; 64] = [
    187, 68, 186, 255, 190, 70, 186, 255, 192, 71, 186, 255, 195, 73, 186, 255, 186, 67, 184, 255,
    188, 69, 184, 255, 191, 70, 184, 255, 193, 72, 184, 255, 185, 67, 182, 255, 187, 68, 182, 255,
    189, 69, 182, 255, 192, 71, 182, 255, 184, 66, 180, 255, 185, 67, 180, 255, 187, 68, 180, 255,
    191, 70, 180, 255,
];
// ASTC unbounded LDR void extent, RGBA16 UNORM endpoints.
pub fn astc_constant(color: [u8; 4]) -> [u8; 16] {
    let mut bytes = [255; 16];
    bytes[0] = 0xfc;
    bytes[1] = 0xfd;
    for (n, c) in color.into_iter().enumerate() {
        bytes[8 + n * 2..10 + n * 2].copy_from_slice(&(u16::from(c) * 257).to_le_bytes());
    }
    bytes
}
pub const BC1: [u8; 8] = [0, 0xf8, 0xe0, 7, 0xe4, 0xe4, 0xe4, 0xe4];
pub const BC1_ALPHA: [u8; 8] = [0x1f, 0, 0xff, 0xff, 0xe4, 0xe4, 0xe4, 0xe4];
pub fn bc_pixels(alpha: bool) -> [u8; 64] {
    let row = if alpha {
        [
            0, 0, 255, 255, 255, 255, 255, 255, 127, 127, 255, 255, 0, 0, 0, 0,
        ]
    } else {
        [
            255, 0, 0, 255, 0, 255, 0, 255, 170, 85, 0, 255, 85, 170, 0, 255,
        ]
    };
    let mut pixels = [0; 64];
    for y in 0..4 {
        pixels[y * 16..(y + 1) * 16].copy_from_slice(&row);
    }
    pixels
}

/// Multiple distinct blocks and cropped edge pixels, without decoder-generated references.
pub fn compressed_mip(
    format: wl_gpu::format::Format,
    w: u32,
    h: u32,
    mip: u32,
) -> (Vec<u8>, Vec<u8>) {
    use wl_gpu::format::Format;
    let mut blocks = Vec::new();
    let mut pixels = vec![0; (w * h * 4) as usize];
    for by in 0..h.div_ceil(4) {
        for bx in 0..w.div_ceil(4) {
            let alternate = (bx + by + mip) % 2 == 1;
            let block_pixels = if matches!(format, Format::Astc4x4 | Format::Astc4x4Srgb) {
                if alternate {
                    let color = [13 + mip as u8, 197, 53, 91];
                    blocks.extend_from_slice(&astc_constant(color));
                    let mut p = [0; 64];
                    for pixel in p.chunks_exact_mut(4) {
                        pixel.copy_from_slice(&color);
                    }
                    p
                } else {
                    blocks.extend_from_slice(&ASTC_REFERENCE);
                    ASTC_PIXELS
                }
            } else {
                blocks.extend_from_slice(if alternate { &BC1_ALPHA } else { &BC1 });
                bc_pixels(alternate)
            };
            for y in 0..4 {
                for x in 0..4 {
                    let px = bx * 4 + x;
                    let py = by * 4 + y;
                    if px < w && py < h {
                        let offset = ((py * w + px) * 4) as usize;
                        let src = ((y * 4 + x) * 4) as usize;
                        pixels[offset..offset + 4].copy_from_slice(&block_pixels[src..src + 4]);
                    }
                }
            }
        }
    }
    (blocks, pixels)
}
pub fn surface(d: wl_gpu::texture::TextureDesc) -> (Vec<u8>, Vec<Vec<u8>>) {
    let mut source = Vec::new();
    let mut expected = Vec::new();
    for (mip, level) in d.levels().unwrap().iter().enumerate() {
        let (blocks, pixels) = compressed_mip(d.format, level.width, level.height, mip as u32);
        let bytes = if level.tiled.is_some() {
            let base = match d.layout {
                wl_gpu::texture::TextureLayout::BlockLinear { block_height_log2 } => {
                    block_height_log2
                }
                _ => 0,
            };
            let mut bh = 1u32 << base;
            while bh > 1 && level.height.div_ceil(4) <= bh / 2 * 8 {
                bh /= 2;
            }
            super::tile_mip(
                &blocks,
                level.width,
                level.height,
                d.format,
                bh.ilog2() as u8,
            )
        } else {
            blocks
        };
        source.extend_from_slice(&bytes);
        expected.push(pixels);
    }
    (source, expected)
}
