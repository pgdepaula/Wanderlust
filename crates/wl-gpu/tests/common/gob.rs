//! Independent scalar byte oracle: Tegra X1 TRM v1.3 pp.1217–1218,
//! corroborated by NVIDIA B197 BLOCK_HEIGHT and Mesa framebuffer setup, pinned
//! in docs/graphics/sources.md. Production uses tegra_swizzle, not this oracle.
#![allow(dead_code)]
pub fn address(byte_x: usize, y: usize, width: usize, log2: u8) -> usize {
    address_bytes(byte_x, y, width * 4, log2)
}
pub fn address_bytes(byte_x: usize, y: usize, row_bytes: usize, log2: u8) -> usize {
    let bh = 1usize << log2;
    let columns = row_bytes.div_ceil(64);
    let block = (y / (8 * bh)) * columns + byte_x / 64;
    let gob = (y % (8 * bh)) / 8;
    // GOB interleave: x[5],y[2:1],x[4],y[0],x[3:0].
    let inside = ((byte_x & 32) << 3)
        | ((y & 6) << 5)
        | ((byte_x & 16) << 1)
        | ((y & 1) << 4)
        | (byte_x & 15);
    block * 512 * bh + gob * 512 + inside
}
pub fn size(width: usize, height: usize, log2: u8) -> usize {
    (width * 4).div_ceil(64) * height.div_ceil(8usize << log2) * (512usize << log2)
}
pub fn encode(linear: &[u8], width: usize, height: usize, log2: u8, original: &[u8]) -> Vec<u8> {
    assert_eq!(original.len(), size(width, height, log2));
    assert_eq!(linear.len(), width * height * 4);
    let mut out = original.to_vec();
    for y in 0..height {
        for x in 0..width * 4 {
            out[address(x, y, width, log2)] = linear[y * width * 4 + x];
        }
    }
    out
}
pub fn decode(tiled: &[u8], width: usize, height: usize, log2: u8) -> Vec<u8> {
    let mut out = vec![0; width * height * 4];
    for y in 0..height {
        for x in 0..width * 4 {
            out[y * width * 4 + x] = tiled[address(x, y, width, log2)];
        }
    }
    out
}
