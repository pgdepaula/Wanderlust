use serde::Serialize;
use std::fmt;

/// Errors arising from NRO0 parsing, validation, or image construction.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub enum NroError {
    /// Buffer is smaller than the required structure size.
    BufferTooSmall { required: usize, actual: usize },

    /// The header magic does not match `"NRO0"`.
    InvalidMagic { expected: [u8; 4], found: [u8; 4] },

    /// The declared size in the header is invalid or larger than the file.
    InvalidDeclaredSize { declared: u32, file_size: usize },

    /// A section extent exceeds its parent container boundaries.
    SectionOutOfBounds {
        section: &'static str,
        offset: u32,
        size: u32,
        container_size: usize,
    },

    /// Two executable sections overlap within the file or memory space.
    SectionOverlap {
        first: &'static str,
        second: &'static str,
    },

    /// Arithmetic overflow occurred while validating offsets or sizes.
    ArithmeticOverflow(&'static str),

    /// An offset or size violates required alignment.
    AlignmentViolation {
        field: &'static str,
        offset: u32,
        required: u32,
    },

    /// An invalid MOD0 dynamic header pointer was specified.
    InvalidMod0Offset { offset: u32, file_size: usize },

    /// The MOD0 magic was missing at the declared offset.
    MissingMod0Magic { offset: u32, found: [u8; 4] },

    /// The asset trailer magic does not match `"ASET"`.
    InvalidAssetMagic { expected: [u8; 4], found: [u8; 4] },

    /// An asset subsection extends beyond the asset section boundaries.
    AssetOutOfBounds {
        asset: &'static str,
        offset: u64,
        size: u64,
        container_size: u64,
    },

    /// Two asset sections overlap within the asset container.
    AssetOverlap {
        first: &'static str,
        second: &'static str,
    },

    /// Unsupported NRO format version.
    UnsupportedVersion(u32),
}

impl fmt::Display for NroError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::BufferTooSmall { required, actual } => {
                write!(
                    f,
                    "buffer too small: requires {required} bytes, got {actual}"
                )
            }
            Self::InvalidMagic { expected, found } => {
                write!(
                    f,
                    "invalid NRO magic: expected {:?}, found {:?}",
                    expected, found
                )
            }
            Self::InvalidDeclaredSize {
                declared,
                file_size,
            } => {
                write!(
                    f,
                    "invalid declared size: declared {declared} bytes, but file is {file_size} bytes"
                )
            }
            Self::SectionOutOfBounds {
                section,
                offset,
                size,
                container_size,
            } => {
                write!(
                    f,
                    "section '{section}' out of bounds: offset {offset:#x} + size {size:#x} exceeds container {container_size:#x}"
                )
            }
            Self::SectionOverlap { first, second } => {
                write!(f, "section overlap: '{first}' overlaps with '{second}'")
            }
            Self::ArithmeticOverflow(msg) => {
                write!(f, "arithmetic overflow in {msg}")
            }
            Self::AlignmentViolation {
                field,
                offset,
                required,
            } => {
                write!(
                    f,
                    "alignment violation: {field} at {offset:#x} is not aligned to {required} bytes"
                )
            }
            Self::InvalidMod0Offset { offset, file_size } => {
                write!(
                    f,
                    "invalid MOD0 offset: {offset:#x} exceeds file size {file_size:#x}"
                )
            }
            Self::MissingMod0Magic { offset, found } => {
                write!(
                    f,
                    "missing MOD0 magic at offset {offset:#x}: found {:?}",
                    found
                )
            }
            Self::InvalidAssetMagic { expected, found } => {
                write!(
                    f,
                    "invalid ASET magic: expected {:?}, found {:?}",
                    expected, found
                )
            }
            Self::AssetOutOfBounds {
                asset,
                offset,
                size,
                container_size,
            } => {
                write!(
                    f,
                    "asset '{asset}' out of bounds: offset {offset:#x} + size {size:#x} exceeds container {container_size:#x}"
                )
            }
            Self::AssetOverlap { first, second } => {
                write!(f, "asset overlap: '{first}' overlaps with '{second}'")
            }
            Self::UnsupportedVersion(v) => {
                write!(f, "unsupported NRO version: {v}")
            }
        }
    }
}

impl std::error::Error for NroError {}
