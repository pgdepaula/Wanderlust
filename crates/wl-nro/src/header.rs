use crate::error::NroError;
use serde::Serialize;

pub const MAGIC_NRO: [u8; 4] = *b"NRO0";
pub const MAGIC_MOD0: [u8; 4] = *b"MOD0";
pub const NRO_START_SIZE: usize = 0x10;
pub const NRO_HEADER_SIZE: usize = 0x70;
pub const NRO_BASE_HEADER_SIZE: usize = 0x80;

/// Byte offset within an NRO file container.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize)]
#[serde(transparent)]
pub struct NroFileOffset(pub u32);

impl From<u32> for NroFileOffset {
    fn from(v: u32) -> Self {
        Self(v)
    }
}

impl From<wl_common::FileOffset> for NroFileOffset {
    fn from(v: wl_common::FileOffset) -> Self {
        Self(v.0)
    }
}

impl From<NroFileOffset> for wl_common::FileOffset {
    fn from(v: NroFileOffset) -> Self {
        Self(v.0)
    }
}

/// NRO startup structure (`0x00..0x10`).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
pub struct NroStart {
    /// AArch64 branch instruction jumping past the NRO header (typically `b 0x80`).
    pub branch_instruction: u32,
    /// File offset to the MOD0 dynamic header.
    pub mod0_offset: NroFileOffset,
    /// Startup padding bytes.
    pub padding: [u8; 8],
}

impl NroStart {
    pub fn parse(bytes: &[u8]) -> Result<Self, NroError> {
        if bytes.len() < NRO_START_SIZE {
            return Err(NroError::BufferTooSmall {
                required: NRO_START_SIZE,
                actual: bytes.len(),
            });
        }
        let branch_instruction = u32::from_le_bytes(bytes[0x00..0x04].try_into().unwrap());
        let mod0_offset = u32::from_le_bytes(bytes[0x04..0x08].try_into().unwrap());
        let mut padding = [0u8; 8];
        padding.copy_from_slice(&bytes[0x08..0x10]);

        Ok(Self {
            branch_instruction,
            mod0_offset: NroFileOffset(mod0_offset),
            padding,
        })
    }
}

/// Declared segment offset and byte length in an NRO header.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
pub struct NroSegmentHeader {
    pub file_offset: NroFileOffset,
    pub size: u32,
}

impl NroSegmentHeader {
    pub fn new(file_offset: u32, size: u32) -> Self {
        Self {
            file_offset: NroFileOffset(file_offset),
            size,
        }
    }
}

/// Offset and length of embedded RoData section (such as DynStr, DynSym, EmbeddedInfo).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
pub struct EmbeddedInfo {
    pub offset: u32,
    pub size: u32,
}

/// Parsed NRO0 executable header (`0x10..0x80`).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
pub struct NroHeader {
    pub magic: [u8; 4],
    pub version: u32,
    pub size: u32,
    pub flags: u32,
    pub text: NroSegmentHeader,
    pub ro: NroSegmentHeader,
    pub data: NroSegmentHeader,
    pub bss_size: u32,
    pub reserved_3c: u32,
    pub module_id: [u8; 32],
    pub dso_handle_offset: u32,
    pub reserved_64: u32,
    pub embedded_info: EmbeddedInfo,
    pub dyn_str: EmbeddedInfo,
    pub dyn_sym: EmbeddedInfo,
}

impl NroHeader {
    pub fn parse(bytes: &[u8]) -> Result<Self, NroError> {
        if bytes.len() < NRO_BASE_HEADER_SIZE {
            return Err(NroError::BufferTooSmall {
                required: NRO_BASE_HEADER_SIZE,
                actual: bytes.len(),
            });
        }

        let magic: [u8; 4] = bytes[0x10..0x14].try_into().unwrap();
        if magic != MAGIC_NRO {
            return Err(NroError::InvalidMagic {
                expected: MAGIC_NRO,
                found: magic,
            });
        }

        let version = u32::from_le_bytes(bytes[0x14..0x18].try_into().unwrap());
        let size = u32::from_le_bytes(bytes[0x18..0x1C].try_into().unwrap());
        let flags = u32::from_le_bytes(bytes[0x1C..0x20].try_into().unwrap());

        let text_offset = u32::from_le_bytes(bytes[0x20..0x24].try_into().unwrap());
        let text_size = u32::from_le_bytes(bytes[0x24..0x28].try_into().unwrap());

        let ro_offset = u32::from_le_bytes(bytes[0x28..0x2C].try_into().unwrap());
        let ro_size = u32::from_le_bytes(bytes[0x2C..0x30].try_into().unwrap());

        let data_offset = u32::from_le_bytes(bytes[0x30..0x34].try_into().unwrap());
        let data_size = u32::from_le_bytes(bytes[0x34..0x38].try_into().unwrap());

        let bss_size = u32::from_le_bytes(bytes[0x38..0x3C].try_into().unwrap());
        let reserved_3c = u32::from_le_bytes(bytes[0x3C..0x40].try_into().unwrap());

        let mut module_id = [0u8; 32];
        module_id.copy_from_slice(&bytes[0x40..0x60]);

        let dso_handle_offset = u32::from_le_bytes(bytes[0x60..0x64].try_into().unwrap());
        let reserved_64 = u32::from_le_bytes(bytes[0x64..0x68].try_into().unwrap());

        let embedded_offset = u32::from_le_bytes(bytes[0x68..0x6C].try_into().unwrap());
        let embedded_size = u32::from_le_bytes(bytes[0x6C..0x70].try_into().unwrap());

        let dyn_str_offset = u32::from_le_bytes(bytes[0x70..0x74].try_into().unwrap());
        let dyn_str_size = u32::from_le_bytes(bytes[0x74..0x78].try_into().unwrap());

        let dyn_sym_offset = u32::from_le_bytes(bytes[0x78..0x7C].try_into().unwrap());
        let dyn_sym_size = u32::from_le_bytes(bytes[0x7C..0x80].try_into().unwrap());

        Ok(Self {
            magic,
            version,
            size,
            flags,
            text: NroSegmentHeader::new(text_offset, text_size),
            ro: NroSegmentHeader::new(ro_offset, ro_size),
            data: NroSegmentHeader::new(data_offset, data_size),
            bss_size,
            reserved_3c,
            module_id,
            dso_handle_offset,
            reserved_64,
            embedded_info: EmbeddedInfo {
                offset: embedded_offset,
                size: embedded_size,
            },
            dyn_str: EmbeddedInfo {
                offset: dyn_str_offset,
                size: dyn_str_size,
            },
            dyn_sym: EmbeddedInfo {
                offset: dyn_sym_offset,
                size: dyn_sym_size,
            },
        })
    }
}
