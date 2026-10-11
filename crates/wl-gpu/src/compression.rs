//! Bounded LDR conversion. ASTC HDR and multipartition modes fail explicitly.
use crate::{Result, format::Format, invalid};

fn astc_ldr(block: &[u8; 16]) -> Result<()> {
    let bits = u128::from_le_bytes(*block);
    if bits & 0x1ff == 0x1fc {
        // Only the all-ones (unbounded) 2D void extent is supported here.
        if bits & 0xe00 != 0xc00 || (bits >> 12) & ((1_u128 << 52) - 1) != (1_u128 << 52) - 1 {
            return Err(invalid(
                "unsupported ASTC HDR, reserved bits or bounded void extent",
            ));
        }
    } else {
        if (bits >> 11) & 3 != 0 {
            return Err(invalid("ASTC multipartition decoding not supported"));
        }
        let mode = (bits >> 13) & 15;
        if !matches!(mode, 0 | 1 | 4 | 5 | 6 | 8 | 9 | 10 | 12 | 13) {
            return Err(invalid("ASTC HDR endpoint mode not supported"));
        }
    }
    Ok(())
}

/// Tightly packed compressed blocks to RGBA bytes, cropped at edge blocks.
/// An error publishes no partial output. sRGB remains encoded for the host view.
pub fn decode_rgba8(format: Format, width: u32, height: u32, input: &[u8]) -> Result<Vec<u8>> {
    if !matches!(
        format,
        Format::Bc1Rgba | Format::Bc1RgbaSrgb | Format::Astc4x4 | Format::Astc4x4Srgb
    ) {
        return Err(invalid("unsupported compressed decoder"));
    }
    let layout = format.layout(width, height, 1)?;
    if input.len() as u64 != layout.size {
        return Err(invalid("compressed payload length mismatch"));
    }
    let output_size = Format::Rgba8Unorm.layout(width, height, 1)?.size;
    if output_size > 64 * 1024 * 1024 {
        return Err(invalid("decoded image exceeds 64 MiB budget"));
    }
    let mut output = vec![0; output_size as usize];
    let bytes = format.block().2 as usize;
    let columns = width.div_ceil(4) as usize;
    for (n, block) in input.chunks_exact(bytes).enumerate() {
        let mut pixels = [[0_u8; 4]; 16];
        if bytes == 8 {
            let mut bgra = [0_u32; 16];
            texture2ddecoder::decode_bc1a_block(block, &mut bgra);
            for (pixel, value) in pixels.iter_mut().zip(bgra) {
                let [b, g, r, a] = value.to_le_bytes();
                *pixel = [r, g, b, a];
            }
        } else {
            let block: &[u8; 16] = block.try_into().expect("checked block size");
            astc_ldr(block)?;
            // The dependency has assertions for malformed bitstreams. A panic is
            // rejected at this boundary as well; it never commits partial pixels.
            let valid = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                astc_decode::astc_decode_block(
                    block,
                    astc_decode::Footprint::new(4, 4),
                    |x, y, rgba| {
                        pixels[(y * 4 + x) as usize] = rgba;
                    },
                )
            }));
            if !matches!(valid, Ok(true)) {
                return Err(invalid("invalid ASTC block"));
            }
        }
        let bx = n % columns * 4;
        let by = n / columns * 4;
        for y in 0..4 {
            for x in 0..4 {
                if bx + x < width as usize && by + y < height as usize {
                    let dst = ((by + y) * width as usize + bx + x) * 4;
                    output[dst..dst + 4].copy_from_slice(&pixels[y * 4 + x]);
                }
            }
        }
    }
    Ok(output)
}
