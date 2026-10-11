use crate::Nro;
use crate::abi::EntryConvention;
use crate::error::NroError;
use crate::header::NroFileOffset;
use crate::image::{NroImage, PAGE_SIZE};
use serde::Serialize;
use wl_common::ModuleOffset;

/// A contiguous range in the NRO file backed by file bytes that must be mapped into memory.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
pub struct MappableRange {
    pub name: &'static str,
    pub file_offset: NroFileOffset,
    pub file_size: u32,
    pub memory_offset: ModuleOffset,
    pub memory_size: u32,
    pub read: bool,
    pub write: bool,
    pub execute: bool,
}

/// A memory range that must be allocated and zero-filled in the guest address space.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
pub struct ZeroFillRange {
    pub name: &'static str,
    pub memory_offset: ModuleOffset,
    pub size: u64,
}

/// Dynamic relocation execution requirement for the NRO.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
pub enum RelocationRequirement {
    /// No dynamic metadata or relocations.
    None,
    /// The NRO contains a self-relocating crt0 (such as libnx `__nx_dynamic`).
    /// The host loader MUST NOT apply relative relocations beforehand to avoid
    /// double-relocation corruption.
    GuestSelfRelocating {
        mod0_offset: u32,
        dynamic_offset: u32,
    },
    /// Relocations must be applied by the host loader before guest execution begins.
    HostRequired { mod0_offset: u32 },
}

/// Register, stack, and environment requirements for starting an NRO under the Homebrew ABI.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
pub struct HomebrewStartupRequirements {
    /// Initial PC register offset relative to module base.
    pub entry_offset: u64,
    /// Expected X0 register: pointer to LoaderConfig table in guest memory.
    pub requires_loader_config: bool,
    /// Expected X1 register: sentinel `0xFFFF_FFFF_FFFF_FFFF` indicating Homebrew ABI.
    pub x1_sentinel: u64,
    /// Return convention: whether LR contains a returnable address back to host/menu.
    pub returnable_lr: bool,
    /// Recommended minimum main thread stack size (bytes).
    pub recommended_stack_size: u64,
    /// Whether the main thread handle must be provided in LoaderConfig (type 1).
    pub requires_main_thread_handle: bool,
}

/// Validated execution-readiness descriptor for an external loader or Horizon process bootstrap.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct HomebrewImagePlan<'a> {
    pub image: NroImage<'a>,
    pub entry_offset: u64,
    pub page_alignment: u64,
    pub image_end: u64,
    pub mappable_ranges: Vec<MappableRange>,
    pub zero_fill_ranges: Vec<ZeroFillRange>,
    pub has_mod0: bool,
    pub mod0_offset: Option<u32>,
    pub dynamic_offset: Option<u32>,
    pub relocation: RelocationRequirement,
    pub startup: HomebrewStartupRequirements,
    pub unsupported_features: Vec<&'static str>,
}

impl<'a> Nro<'a> {
    /// Constructs a validated execution-readiness plan for process bootstrapping and AOT ingestion.
    pub fn plan(&self) -> Result<HomebrewImagePlan<'a>, NroError> {
        let image = self.to_image()?;
        let entry_offset = image.entrypoint_offset.0;
        let page_alignment = PAGE_SIZE;

        let mappable_ranges = vec![
            MappableRange {
                name: "text",
                file_offset: self.header.text.file_offset,
                file_size: self.header.text.size,
                memory_offset: image.text.memory_offset,
                memory_size: image.text.memory_size,
                read: true,
                write: false,
                execute: true,
            },
            MappableRange {
                name: "rodata",
                file_offset: self.header.ro.file_offset,
                file_size: self.header.ro.size,
                memory_offset: image.rodata.memory_offset,
                memory_size: image.rodata.memory_size,
                read: true,
                write: false,
                execute: false,
            },
            MappableRange {
                name: "data",
                file_offset: self.header.data.file_offset,
                file_size: self.header.data.size,
                memory_offset: image.data.memory_offset,
                memory_size: image.data.memory_size,
                read: true,
                write: true,
                execute: false,
            },
        ];

        let mut zero_fill_ranges = Vec::new();
        if self.header.bss_size > 0 {
            zero_fill_ranges.push(ZeroFillRange {
                name: "bss",
                memory_offset: image.bss_offset,
                size: self.header.bss_size as u64,
            });
        }

        // Align image_end to page boundary
        let unaligned_end = image.total_memory_size;
        let image_end = match unaligned_end % PAGE_SIZE {
            0 => unaligned_end,
            rem => unaligned_end
                .checked_add(PAGE_SIZE - rem)
                .ok_or(NroError::ArithmeticOverflow("image_end"))?,
        };

        let mod0_opt = if self.start.mod0_offset.0 != 0 {
            Some(self.start.mod0_offset.0 as usize)
        } else {
            None
        };
        let mut dynamic_offset = None;
        let relocation = if let Some(mod0_off) = mod0_opt {
            let mod0_u32 = mod0_off as u32;
            // Check if MOD0 has dynamic offset
            if mod0_off + 8 <= self.raw_bytes.len() {
                let dyn_rel = i32::from_le_bytes(
                    self.raw_bytes[mod0_off + 4..mod0_off + 8]
                        .try_into()
                        .unwrap(),
                );
                if dyn_rel != 0 {
                    let dyn_abs = mod0_off as i64 + dyn_rel as i64;
                    if dyn_abs >= 0 && dyn_abs as usize <= self.raw_bytes.len() {
                        dynamic_offset = Some(dyn_abs as u32);
                    }
                }
            }

            if let Some(dyn_off) = dynamic_offset {
                RelocationRequirement::GuestSelfRelocating {
                    mod0_offset: mod0_u32,
                    dynamic_offset: dyn_off,
                }
            } else {
                RelocationRequirement::HostRequired {
                    mod0_offset: mod0_u32,
                }
            }
        } else {
            RelocationRequirement::None
        };

        let startup = HomebrewStartupRequirements {
            entry_offset,
            requires_loader_config: matches!(
                image.entry_convention,
                EntryConvention::HomebrewNro { .. }
            ),
            x1_sentinel: 0xFFFF_FFFF_FFFF_FFFF,
            returnable_lr: true,
            recommended_stack_size: 0x10_0000, // 1 MiB standard homebrew stack
            requires_main_thread_handle: true,
        };

        let unsupported_features = Vec::new();

        Ok(HomebrewImagePlan {
            image,
            entry_offset,
            page_alignment,
            image_end,
            mappable_ranges,
            zero_fill_ranges,
            has_mod0: mod0_opt.is_some(),
            mod0_offset: mod0_opt.map(|o| o as u32),
            dynamic_offset,
            relocation,
            startup,
            unsupported_features,
        })
    }
}
