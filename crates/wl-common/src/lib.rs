//! Address-domain types shared by the loader and recompiler.
use serde::Serialize;
pub type Result<T> = std::result::Result<T, String>;
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize)]
#[serde(transparent)]
pub struct GuestAddr(pub u64);
#[derive(Clone, Copy, Debug, Serialize)]
#[serde(transparent)]
pub struct FileOffset(pub u32);

/// Relative to the beginning of a module; not a process virtual address.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize)]
#[serde(transparent)]
pub struct ModuleOffset(pub u64);
