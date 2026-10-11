#![allow(dead_code)]
use wl_gpu::{format::Format, memory::GpuAddress, texture::*};

// Scalar GOB byte oracle independent of tegra_swizzle implementation.
pub fn gob_address(x: usize, y: usize, row: usize, bh: usize) -> usize {
    (y / (8 * bh)) * 512 * bh * row.div_ceil(64)
        + (x / 64) * 512 * bh
        + (y % (8 * bh) / 8) * 512
        + (x % 64 / 32) * 256
        + (y % 8 / 2) * 64
        + (x % 32 / 16) * 32
        + (y % 2) * 16
        + x % 16
}
pub fn desc(width: u32, height: u32, mip_count: u32) -> TextureDesc {
    TextureDesc {
        address: GpuAddress(0x10000),
        width,
        height,
        mip_count,
        format: Format::Rgba8Unorm,
        layout: TextureLayout::BlockLinear {
            block_height_log2: 1,
        },
        filter: Filter::Nearest,
    }
}
pub fn rgba_mip(width: u32, height: u32, mip: u32) -> Vec<u8> {
    let mut out = Vec::new();
    for y in 0..height {
        for x in 0..width {
            out.extend_from_slice(&[
                (x * 17 + y * 3 + mip * 37) as u8,
                (y * 29 + mip * 13 + 7) as u8,
                (x * 5 + y * 11 + mip * 19 + 41) as u8,
                (64 + (x * 3 + y * 7) % 192) as u8,
            ]);
        }
    }
    out
}
pub fn tile_mip(linear: &[u8], width: u32, height: u32, format: Format, log2: u8) -> Vec<u8> {
    let (bw, bh, bytes) = format.block();
    let row = width.div_ceil(bw) as usize * bytes as usize;
    let rows = height.div_ceil(bh) as usize;
    let gob_height = 1usize << log2;
    let mut out = vec![0x99; row.div_ceil(64) * rows.div_ceil(8 * gob_height) * 512 * gob_height];
    for y in 0..rows {
        for x in 0..row {
            out[gob_address(x, y, row, gob_height)] = linear[y * row + x];
        }
    }
    out
}
pub fn rgba_surface(d: TextureDesc) -> (Vec<u8>, Vec<Vec<u8>>) {
    let mut tiled = Vec::new();
    let mut mips = Vec::new();
    let mut bh = match d.layout {
        TextureLayout::BlockLinear { block_height_log2 } => 1 << block_height_log2,
        _ => 1,
    };
    for m in 0..d.mip_count {
        let w = (d.width >> m).max(1);
        let h = (d.height >> m).max(1);
        while bh > 1 && h <= (bh / 2) * 8 {
            bh /= 2;
        }
        let linear = rgba_mip(w, h, m);
        let data = match d.layout {
            TextureLayout::Linear => linear.clone(),
            _ => tile_mip(&linear, w, h, d.format, bh.ilog2() as u8),
        };
        tiled.extend_from_slice(&data);
        mips.push(linear);
    }
    (tiled, mips)
}
pub fn assert_pixels(actual: &[u8], expected: &[u8], tolerance: u8) {
    assert_eq!(actual.len(), expected.len());
    for (n, (a, b)) in actual.iter().zip(expected).enumerate() {
        assert!(
            a.abs_diff(*b) <= tolerance,
            "byte {n}: actual {a}, expected {b}"
        );
    }
}

pub mod compressed;
pub mod maxwell;
