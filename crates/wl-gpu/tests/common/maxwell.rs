// Literal NVIDIA B197 fields (byte methods divided by four only in FIFO headers).
// No Wanderlust texture encoder generates these descriptor or transport words.
pub const TIC_RGBA: [u32; 8] = [
    0x58d24908, 0x00010000, 0x00600000, 0x00000010, 0x00800024, 0x80000012, 0, 0,
];
pub const TIC_BC1: [u32; 8] = [
    0x58d24924, 0x00010000, 0x00600000, 0, 0x00800024, 0x80000012, 0, 0,
];
pub const TIC_ASTC: [u32; 8] = [
    0x58d24940, 0x00010000, 0x00600000, 0, 0x00800024, 0x80000012, 0, 0,
];
pub const TSC_NEAREST: [u32; 8] = [0x92, 0x51, 0, 0, 0, 0, 0, 0];
pub const POOL_PACKETS: [u32; 12] = [
    0x20010000, 0xb197, // bind class to subchannel zero
    0x200103c4, 1, // modern texture header selection, method 0x0f10
    0x20030557, 0, 0x31000, 0, // TSC A/B/C, inclusive maximum index zero
    0x2003055d, 0, 0x30000, 0, // TIC A/B/C
];
pub fn descriptor_bytes(image: [u32; 8], sampler: [u32; 8]) -> Vec<u8> {
    image
        .into_iter()
        .chain(sampler)
        .flat_map(u32::to_le_bytes)
        .collect()
}
pub fn configured() -> wl_gpu::maxwell::Maxwell3d {
    let mut fifo = wl_gpu::fifo::Decoder::default();
    let mut engine = wl_gpu::maxwell::Maxwell3d::default();
    for word in POOL_PACKETS {
        if let Some(write) = fifo.push_word(word).unwrap() {
            engine.apply(write).unwrap();
        }
    }
    fifo.finish().unwrap();
    engine
}
