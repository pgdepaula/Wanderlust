use crate::abi::EntryConvention;
use crate::header::NroFileOffset;
use serde::Serialize;
use wl_common::{GuestAddr, ModuleOffset};

pub const PAGE_SIZE: u64 = 0x1000;

/// Kind of memory segment in an NRO executable.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
pub enum SegmentKind {
    Text,
    RoData,
    Data,
    Bss,
}

/// A borrowed view of a single memory segment in an NRO.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
pub struct NroSegment<'a> {
    pub name: &'static str,
    pub kind: SegmentKind,
    pub file_offset: NroFileOffset,
    pub memory_offset: ModuleOffset,
    pub file_size: u32,
    pub memory_size: u32,
    pub readable: bool,
    pub writable: bool,
    pub executable: bool,
    #[serde(skip)]
    pub bytes: &'a [u8],
}

/// Mapped guest segment layout, suitable for process image construction or AOT analysis.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct NroGuestMapping {
    pub base: GuestAddr,
    pub size: u64,
    pub read: bool,
    pub write: bool,
    pub execute: bool,
    #[serde(skip)]
    pub bytes: Vec<u8>,
}

/// Authoritative guest image description for an ingested NRO0 executable.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct NroImage<'a> {
    pub text: NroSegment<'a>,
    pub rodata: NroSegment<'a>,
    pub data: NroSegment<'a>,
    pub bss_offset: ModuleOffset,
    pub bss_size: u64,
    pub total_memory_size: u64,
    pub module_id: [u8; 32],
    pub module_id_hex: String,
    pub entrypoint_offset: ModuleOffset,
    pub entry_convention: EntryConvention,
}

impl<'a> NroImage<'a> {
    /// Constructs mapped guest memory regions at the given guest base address.
    ///
    /// - Text segment is mapped at `base + text.memory_offset` as `R-X`.
    /// - RoData segment is mapped at `base + rodata.memory_offset` as `R--`.
    /// - Data segment is mapped at `base + data.memory_offset` as `RW-`, with BSS appended
    ///   and zero-initialized to page boundary.
    pub fn create_guest_mappings(&self, base: GuestAddr) -> Vec<NroGuestMapping> {
        let mut mappings = Vec::with_capacity(3);

        // 1. Text (R-X)
        let text_addr = GuestAddr(base.0 + self.text.memory_offset.0);
        let mut text_bytes = self.text.bytes.to_vec();
        pad_to_alignment(&mut text_bytes, PAGE_SIZE as usize);
        mappings.push(NroGuestMapping {
            base: text_addr,
            size: text_bytes.len() as u64,
            read: true,
            write: false,
            execute: true,
            bytes: text_bytes,
        });

        // 2. RoData (R--)
        let ro_addr = GuestAddr(base.0 + self.rodata.memory_offset.0);
        let mut ro_bytes = self.rodata.bytes.to_vec();
        pad_to_alignment(&mut ro_bytes, PAGE_SIZE as usize);
        mappings.push(NroGuestMapping {
            base: ro_addr,
            size: ro_bytes.len() as u64,
            read: true,
            write: false,
            execute: false,
            bytes: ro_bytes,
        });

        // 3. Data + BSS (RW-)
        let data_addr = GuestAddr(base.0 + self.data.memory_offset.0);
        let total_data_bss = self.data.bytes.len() + self.bss_size as usize;
        let mut data_bytes = Vec::with_capacity(total_data_bss);
        data_bytes.extend_from_slice(self.data.bytes);
        data_bytes.resize(total_data_bss, 0); // Zero-initialize BSS
        pad_to_alignment(&mut data_bytes, PAGE_SIZE as usize);
        mappings.push(NroGuestMapping {
            base: data_addr,
            size: data_bytes.len() as u64,
            read: true,
            write: true,
            execute: false,
            bytes: data_bytes,
        });

        mappings
    }
}

fn pad_to_alignment(vec: &mut Vec<u8>, align: usize) {
    let remainder = vec.len() % align;
    if remainder != 0 {
        let padding = align - remainder;
        vec.resize(vec.len() + padding, 0);
    }
}
