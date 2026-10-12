//! MAXWELL_B (B197) texture pool state and modern block-linear TIC/TSC subset.
//! Register offsets are bytes; FIFO packet addresses are separately word-based.
use crate::{Error, Result, fifo::MethodWrite, format::Format, invalid, memory::*, texture::*};

pub const CLASS: u32 = 0xb197;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MethodError {
    InvalidSubchannel(u8),
    InvalidMethod(u16),
    UnboundSubchannel(u8),
    UnsupportedClass(u32),
    UnsupportedMethod(u16),
    UnsupportedValue { method: u16, value: u32 },
    EpochOverflow,
    IncompleteConstantBuffer,
}
impl std::fmt::Display for MethodError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "Maxwell method fault: {self:?}")
    }
}
impl std::error::Error for MethodError {}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MethodEffect {
    State,
    TextureHeadersInvalidated(Option<u32>),
    SamplersInvalidated(Option<u32>),
}
#[derive(Debug, Clone, Copy, Default)]
struct Pool {
    upper: Option<u32>,
    lower: Option<u32>,
    maximum: Option<u32>,
}
impl Pool {
    fn descriptor(&self, index: u32) -> Result<GpuAddress> {
        let (Some(high), Some(low), Some(maximum)) = (self.upper, self.lower, self.maximum) else {
            return Err(invalid("incomplete Maxwell descriptor pool"));
        };
        if index > maximum {
            return Err(invalid("descriptor index exceeds inclusive pool limit"));
        }
        let base = (u64::from(high) << 32) | u64::from(low);
        // Accepted implementation subset, not an inferred hardware fault rule.
        if base % 32 != 0 {
            return Err(invalid("descriptor pool base must be 32-byte aligned"));
        }
        let address = base
            .checked_add(u64::from(index) * 32)
            .ok_or_else(|| invalid("descriptor address overflow"))?;
        if address.checked_add(32).is_none_or(|end| end > 1 << 40) {
            return Err(invalid("descriptor exceeds 40-bit pool address space"));
        }
        Ok(GpuAddress(address))
    }
}
/// A stage/slot binding snapshots the selector at BIND_GROUP_CONSTANT_BUFFER.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ConstantBuffer {
    pub address: GpuAddress,
    pub size: u32,
}
impl ConstantBuffer {
    pub fn read_word(
        &self,
        offset: u32,
        map: &AddressMap,
        memory: &impl GuestMemory,
    ) -> Result<u32> {
        if offset % 4 != 0 || offset.checked_add(4).is_none_or(|end| end > self.size) {
            return Err(invalid("unaligned/out-of-bounds constant buffer word"));
        }
        let address = self
            .address
            .0
            .checked_add(u64::from(offset))
            .ok_or_else(|| invalid("constant address overflow"))?;
        let bytes = map.read(memory, GpuAddress(address), 4)?;
        if bytes.len() != 4 {
            return Err(invalid("short constant buffer read"));
        }
        Ok(u32::from_le_bytes(
            bytes.try_into().expect("checked constant word"),
        ))
    }
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct DescriptorStamp {
    image: GuestAddress,
    sampler: GuestAddress,
    image_version: (u64, u64),
    sampler_version: (u64, u64),
    image_epoch: u64,
    sampler_epoch: u64,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FragmentSource {
    pub address: GpuAddress,
    pub registers: u8,
    pub group: u8,
}
/// One channel's single MAXWELL_B instance. Subchannels route to the same state;
/// rebinding the same class does not manufacture an independent register bank.
#[derive(Debug, Default)]
pub struct Maxwell3d {
    method_writes: u64,
    bound: [bool; 8],
    modern_headers: bool,
    images: Pool,
    program_upper: Option<u32>,
    program_lower: Option<u32>,
    fragment_enabled: bool,
    fragment_offset: Option<u32>,
    fragment_registers: Option<u8>,
    fragment_group: Option<u8>,
    cb_size: Option<u32>,
    cb_upper: Option<u32>,
    cb_lower: Option<u32>,
    constants: [[Option<ConstantBuffer>; 32]; 5],
    samplers: Pool,
    pub texture_header_epoch: u64,
    pub sampler_epoch: u64,
}
impl Maxwell3d {
    /// Allocation-free even on errors. Caller must stop the channel on a fault.
    pub fn apply(&mut self, write: MethodWrite) -> std::result::Result<MethodEffect, MethodError> {
        self.method_writes = self.method_writes.saturating_add(1);
        let MethodWrite {
            subchannel,
            method,
            value,
        } = write;
        if subchannel >= 8 {
            return Err(MethodError::InvalidSubchannel(subchannel));
        }
        if method % 4 != 0 || method > 0x3ffc {
            return Err(MethodError::InvalidMethod(method));
        }
        let bad = || MethodError::UnsupportedValue { method, value };
        if method == 0 {
            // Engine instance zero only; SET_OBJECT carries a class, not texture ID.
            if value != CLASS {
                return Err(MethodError::UnsupportedClass(value));
            }
            self.bound[subchannel as usize] = true;
            return Ok(MethodEffect::State);
        }
        if !self.bound[subchannel as usize] {
            return Err(MethodError::UnboundSubchannel(subchannel));
        }
        match method {
            0x1608 => {
                if value > 0xff {
                    return Err(bad());
                }
                self.program_upper = Some(value);
            }
            0x160c => self.program_lower = Some(value),
            0x2140 => {
                if !matches!(value, 0x50 | 0x51) {
                    return Err(bad());
                }
                self.fragment_enabled = value & 1 != 0;
            }
            0x2144 => self.fragment_offset = Some(value),
            0x214c => {
                if value == 0 || value > 255 {
                    return Err(bad());
                }
                self.fragment_registers = Some(value as u8);
            }
            0x2150 => {
                if value > 4 {
                    return Err(bad());
                }
                self.fragment_group = Some(value as u8);
            }
            0x2380 => {
                if value == 0 || value > 65536 || value % 4 != 0 {
                    return Err(bad());
                }
                self.cb_size = Some(value);
            }
            0x2384 => {
                if value > 0xff {
                    return Err(bad());
                }
                self.cb_upper = Some(value);
            }
            0x2388 => {
                if value % 4 != 0 {
                    return Err(bad());
                }
                self.cb_lower = Some(value);
            }
            m if (0x2410..=0x2490).contains(&m) && (m - 0x2410) % 32 == 0 => {
                if value & !0x1f1 != 0 {
                    return Err(bad());
                }
                let group = ((m - 0x2410) / 32) as usize;
                let slot = ((value >> 4) & 31) as usize;
                let binding = if value & 1 == 0 {
                    None
                } else {
                    let (Some(size), Some(hi), Some(lo)) =
                        (self.cb_size, self.cb_upper, self.cb_lower)
                    else {
                        return Err(MethodError::IncompleteConstantBuffer);
                    };
                    let address = (u64::from(hi) << 32) | u64::from(lo);
                    if address
                        .checked_add(u64::from(size))
                        .is_none_or(|end| end > 1 << 40)
                    {
                        return Err(bad());
                    }
                    Some(ConstantBuffer {
                        address: GpuAddress(address),
                        size,
                    })
                };
                self.constants[group][slot] = binding;
            }
            0x0100 => {} // documented NO_OPERATION
            0x0f10 => {
                if value != 1 {
                    return Err(bad());
                }
                self.modern_headers = true;
            }
            0x155c | 0x1574 => {
                if value & !0xff != 0 {
                    return Err(bad());
                }
                if method == 0x155c {
                    self.samplers.upper = Some(value);
                } else {
                    self.images.upper = Some(value);
                }
            }
            0x1560 => self.samplers.lower = Some(value),
            0x1578 => self.images.lower = Some(value),
            0x1564 => {
                if value & !0xfffff != 0 {
                    return Err(bad());
                }
                self.samplers.maximum = Some(value);
            }
            0x157c => {
                if value & !0x3fffff != 0 {
                    return Err(bad());
                }
                self.images.maximum = Some(value);
            }
            0x1424 | 0x1428 => {
                if value & !0x03fffff1 != 0 {
                    return Err(bad());
                }
                let index = (value & 1 != 0).then_some((value >> 4) & 0x3fffff);
                let epoch = if method == 0x1424 {
                    &mut self.sampler_epoch
                } else {
                    &mut self.texture_header_epoch
                };
                *epoch = epoch.checked_add(1).ok_or(MethodError::EpochOverflow)?;
                return Ok(if method == 0x1424 {
                    MethodEffect::SamplersInvalidated(index)
                } else {
                    MethodEffect::TextureHeadersInvalidated(index)
                });
            }
            _ => return Err(MethodError::UnsupportedMethod(method)),
        }
        Ok(MethodEffect::State)
    }
    /// Counts attempted method writes, including rejected writes; saturates at u64::MAX.
    pub fn method_writes(&self) -> u64 {
        self.method_writes
    }
    /// Pixel pipeline index five explicitly selects its constant-binding group.
    pub fn fragment_program(&self) -> Result<FragmentSource> {
        if !self.bound.iter().any(|b| *b) || !self.fragment_enabled {
            return Err(invalid("pixel program disabled or B197 unbound"));
        }
        let (Some(hi), Some(lo), Some(offset), Some(registers), Some(group)) = (
            self.program_upper,
            self.program_lower,
            self.fragment_offset,
            self.fragment_registers,
            self.fragment_group,
        ) else {
            return Err(invalid("incomplete pixel program state"));
        };
        let address = ((u64::from(hi) << 32) | u64::from(lo))
            .checked_add(u64::from(offset))
            .ok_or_else(|| invalid("shader program address overflow"))?;
        if address >= 1 << 40 {
            return Err(invalid("shader program exceeds 40-bit address space"));
        }
        Ok(FragmentSource {
            address: GpuAddress(address),
            registers,
            group,
        })
    }
    pub fn constant_buffer(&self, group: u8, slot: u8) -> Result<ConstantBuffer> {
        self.constants
            .get(group as usize)
            .and_then(|g| g.get(slot as usize))
            .copied()
            .flatten()
            .ok_or_else(|| invalid("unbound constant buffer group/slot"))
    }
    /// Maxwell's 32-bit combined handle: low 20 bits TIC, high 12 bits TSC.
    /// TEX.B supplies this handle from a register; SET_BINDLESS_TEXTURE instead
    /// selects a CB slot for the distinct bound-TEX instruction form.
    pub fn resolve_handle(
        &self,
        subchannel: u8,
        handle: u32,
        map: &AddressMap,
        memory: &impl GuestMemory,
    ) -> Result<TextureDesc> {
        self.resolve_texture(subchannel, handle & 0xfffff, handle >> 20, map, memory)
    }
    pub(crate) fn texture_subchannel(&self) -> Result<u8> {
        self.bound
            .iter()
            .position(|b| *b)
            .map(|n| n as u8)
            .ok_or_else(|| invalid("no bound B197 subchannel"))
    }
    pub(crate) fn descriptor_stamp(
        &self,
        subchannel: u8,
        handle: u32,
        map: &AddressMap,
        changes: &TextureChanges,
    ) -> Result<DescriptorStamp> {
        if !self
            .bound
            .get(subchannel as usize)
            .copied()
            .unwrap_or(false)
            || !self.modern_headers
        {
            return Err(invalid("texture engine not configured"));
        }
        let image = map.resolve(self.images.descriptor(handle & 0xfffff)?, 32)?;
        let sampler = map.resolve(self.samplers.descriptor(handle >> 20)?, 32)?;
        Ok(DescriptorStamp {
            image,
            sampler,
            image_version: changes.stamp(image.0..image.0 + 32),
            sampler_version: changes.stamp(sampler.0..sampler.0 + 32),
            image_epoch: self.texture_header_epoch,
            sampler_epoch: self.sampler_epoch,
        })
    }
    /// Descriptor indices are supplied by a caller, not a fabricated bind method.
    /// Original shader handle/constant-buffer decoding is not implemented.
    pub fn resolve_texture(
        &self,
        subchannel: u8,
        image_index: u32,
        sampler_index: u32,
        map: &AddressMap,
        memory: &impl GuestMemory,
    ) -> Result<TextureDesc> {
        if !self
            .bound
            .get(subchannel as usize)
            .copied()
            .unwrap_or(false)
            || !self.modern_headers
        {
            return Err(invalid(
                "unbound engine or modern texture headers not selected",
            ));
        }
        let image = TextureImage::decode(read_descriptor(
            self.images.descriptor(image_index)?,
            map,
            memory,
        )?)?;
        let sampler = TextureSampler::decode(read_descriptor(
            self.samplers.descriptor(sampler_index)?,
            map,
            memory,
        )?)?;
        let desc = image.with_sampler(sampler);
        Ok(desc)
    }
}
fn read_descriptor(
    address: GpuAddress,
    map: &AddressMap,
    memory: &impl GuestMemory,
) -> Result<[u32; 8]> {
    let guest = map.resolve(address, 32)?;
    let bytes = memory.read(guest, 32)?;
    if bytes.len() != 32 {
        return Err(invalid("short descriptor read"));
    }
    Ok(std::array::from_fn(|n| {
        u32::from_le_bytes(
            bytes[n * 4..n * 4 + 4]
                .try_into()
                .expect("checked descriptor"),
        )
    }))
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TextureImage {
    pub address: GpuAddress,
    pub width: u32,
    pub height: u32,
    pub mip_count: u32,
    pub format: Format,
    pub layout: TextureLayout,
}
impl TextureImage {
    pub fn decode(words: [u32; 8]) -> Result<Self> {
        // These masks select implemented fields, not merely reserved bits. Fields
        // such as depth, sparse modes and multisampling are rejected explicitly.
        let masks = [
            u32::MAX,
            0xfffffe00,
            0x00e0ffff,
            0xf0000038,
            0x07c0ffff,
            0x8000ffff,
            0,
            0xff,
        ];
        for (n, (&word, &mask)) in words.iter().zip(&masks).enumerate() {
            if word & !mask != 0 {
                return Err(Error(format!(
                    "unsupported TIC word {n} bits {:#x}",
                    word & !mask
                )));
            }
        }
        if words[0] & !0x7f != 0x58d24900 {
            return Err(invalid("unsupported TIC channel types or swizzle"));
        }
        if words[2] >> 21 & 7 != 3 || words[4] >> 23 & 15 != 1 || words[5] >> 31 != 1 {
            return Err(invalid("requires normalized block-linear 2D TIC"));
        }
        let srgb = words[4] & (1 << 22) != 0;
        let format = match (words[0] & 0x7f, srgb) {
            (0x08, false) => Format::Rgba8Unorm,
            (0x08, true) => Format::Rgba8Srgb,
            (0x24, false) => Format::Bc1Rgba,
            (0x24, true) => Format::Bc1RgbaSrgb,
            (0x40, false) => Format::Astc4x4,
            (0x40, true) => Format::Astc4x4Srgb,
            _ => return Err(invalid("unsupported TIC format")),
        };
        let max_mip = words[3] >> 28;
        if words[7] & 15 != 0 || words[7] >> 4 & 15 != max_mip {
            return Err(invalid("TIC mip subviews unsupported"));
        }
        let image = Self {
            address: GpuAddress(u64::from(words[1]) | (u64::from(words[2] & 0xffff) << 32)),
            width: (words[4] & 0xffff) + 1,
            height: (words[5] & 0xffff) + 1,
            mip_count: max_mip + 1,
            format,
            layout: TextureLayout::BlockLinear {
                block_height_log2: ((words[3] >> 3) & 7) as u8,
            },
        };
        image
            .with_sampler(TextureSampler {
                filter: Filter::Nearest,
            })
            .validate()?;
        Ok(image)
    }
    pub fn with_sampler(self, sampler: TextureSampler) -> TextureDesc {
        TextureDesc {
            address: self.address,
            width: self.width,
            height: self.height,
            mip_count: self.mip_count,
            format: self.format,
            layout: self.layout,
            filter: sampler.filter,
        }
    }
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TextureSampler {
    pub filter: Filter,
}
impl TextureSampler {
    pub fn decode(words: [u32; 8]) -> Result<Self> {
        if words[0] != 0x92 || words[1] & !0xf7 != 0 || words[2..].iter().any(|&v| v != 0) {
            return Err(invalid(
                "unsupported TSC wrap, compare, LOD, bias, anisotropy or border fields",
            ));
        }
        let mag = words[1] & 7;
        let min = (words[1] >> 4) & 3;
        let mip = (words[1] >> 6) & 3;
        if mag != min || !matches!(mip, 1 | 2) {
            return Err(invalid("unsupported TSC filter combination"));
        }
        let filter = match mag {
            1 => Filter::Nearest,
            2 => Filter::Linear,
            _ => return Err(invalid("unsupported TSC filter")),
        };
        Ok(Self { filter })
    }
}
