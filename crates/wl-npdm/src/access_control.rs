//! Service Access Control (SAC) and Filesystem Access Control (FAC/FAH).
//!
//! Borrowed, zero-allocation structures and iterators over NPDM access descriptors.

use crate::error::NpdmError;
use serde::Serialize;

/// A single borrowed entry in a Service Access Control table.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
pub struct SacEntry<'a> {
    /// True if the process registers/hosts this service port, false if it accesses/clients it.
    pub is_server: bool,
    /// Service port name string (up to 8 ASCII characters).
    pub name: &'a str,
}

impl<'a> SacEntry<'a> {
    /// Whether this service permission specifies a wildcard match (ends in `*`).
    pub fn is_wildcard(&self) -> bool {
        self.name.ends_with('*')
    }

    /// Checks if a requested service name matches this rule.
    pub fn matches(&self, candidate: &str) -> bool {
        if self.is_wildcard() {
            let prefix = &self.name[..self.name.len() - 1];
            candidate.starts_with(prefix)
        } else {
            self.name == candidate
        }
    }
}

/// Zero-allocation borrowed iterator over a Service Access Control (SAC) buffer.
#[derive(Clone, Debug)]
pub struct SacIterator<'a> {
    data: &'a [u8],
    offset: usize,
}

impl<'a> SacIterator<'a> {
    pub(crate) fn new(data: &'a [u8]) -> Self {
        Self { data, offset: 0 }
    }
}

impl<'a> Iterator for SacIterator<'a> {
    type Item = Result<SacEntry<'a>, NpdmError>;

    fn next(&mut self) -> Option<Self::Item> {
        if self.offset >= self.data.len() {
            return None;
        }

        let ctrl = self.data[self.offset];
        self.offset += 1;

        let name_len = ((ctrl & 0x07) + 1) as usize;
        let end = match self.offset.checked_add(name_len) {
            Some(e) => e,
            None => {
                return Some(Err(NpdmError::MalformedServiceAccessControl {
                    offset: self.offset,
                }));
            }
        };

        if end > self.data.len() {
            return Some(Err(NpdmError::MalformedServiceAccessControl {
                offset: self.offset,
            }));
        }

        let name_bytes = &self.data[self.offset..end];
        self.offset = end;

        let name = match std::str::from_utf8(name_bytes) {
            Ok(s) => s,
            Err(_) => {
                return Some(Err(NpdmError::MalformedServiceAccessControl {
                    offset: self.offset - name_len,
                }));
            }
        };

        let is_server = (ctrl & 0x80) != 0;
        Some(Ok(SacEntry { is_server, name }))
    }
}

/// Filesystem access control permissions combined from ACID (FAC) and ACI0 (FAH).
///
/// Follows Switchbrew specification for Nintendo Switch filesystem permission descriptors.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
pub struct FsAccessControl<'a> {
    pub fac_version: u8,
    pub fah_version: u8,
    pub content_owner_id_count: u8,
    pub save_data_owner_id_count: u8,
    pub fac_permissions: u64,
    pub fah_permissions: u64,
    pub effective_permissions: u64,
    pub content_owner_id_min: u64,
    pub content_owner_id_max: u64,
    pub save_data_owner_id_min: u64,
    pub save_data_owner_id_max: u64,
    #[serde(skip)]
    pub content_owner_ids: &'a [u8],
    #[serde(skip)]
    pub save_data_owner_ids: &'a [u8],
    #[serde(skip)]
    pub fah_content_owner_ids: &'a [u8],
    #[serde(skip)]
    pub fah_save_data_owner_ids: &'a [u8],
}

impl<'a> FsAccessControl<'a> {
    /// Parses and validates bounded FAC and FAH subregions.
    pub fn parse(fac_bytes: &'a [u8], fah_bytes: &'a [u8]) -> Result<Option<Self>, NpdmError> {
        if fac_bytes.is_empty() && fah_bytes.is_empty() {
            return Ok(None);
        }
        if fac_bytes.is_empty() || fah_bytes.is_empty() {
            return Err(NpdmError::MalformedFsAccessControl {
                section: "FAC/FAH",
                reason: "both ACID FAC and ACI0 FAH must be present if filesystem control is declared",
            });
        }

        // ACID FAC must have minimum 0x2C header bytes
        if fac_bytes.len() < 0x2C {
            return Err(NpdmError::MalformedFsAccessControl {
                section: "ACID FAC",
                reason: "descriptor buffer is smaller than 0x2C base header",
            });
        }
        let fac_version = fac_bytes[0];
        if fac_version != 1 {
            return Err(NpdmError::MalformedFsAccessControl {
                section: "ACID FAC",
                reason: "unsupported FAC version (must be 1)",
            });
        }

        let content_owner_id_count = fac_bytes[1];
        let save_data_owner_id_count = fac_bytes[2];

        let content_ids_len = (content_owner_id_count as usize) * 8;
        let save_ids_len = (save_data_owner_id_count as usize) * 8;
        let required_fac_len = 0x2C + content_ids_len + save_ids_len;

        if fac_bytes.len() < required_fac_len {
            return Err(NpdmError::MalformedFsAccessControl {
                section: "ACID FAC",
                reason: "descriptor buffer is smaller than declared owner id arrays",
            });
        }

        let fac_permissions = u64::from_le_bytes(fac_bytes[4..12].try_into().unwrap());
        let content_owner_id_min = u64::from_le_bytes(fac_bytes[12..20].try_into().unwrap());
        let content_owner_id_max = u64::from_le_bytes(fac_bytes[20..28].try_into().unwrap());
        let save_data_owner_id_min = u64::from_le_bytes(fac_bytes[28..36].try_into().unwrap());
        let save_data_owner_id_max = u64::from_le_bytes(fac_bytes[36..44].try_into().unwrap());

        let content_ids_start = 0x2C;
        let content_ids_end = content_ids_start + content_ids_len;
        let save_ids_start = content_ids_end;
        let save_ids_end = save_ids_start + save_ids_len;

        let content_owner_ids = &fac_bytes[content_ids_start..content_ids_end];
        let save_data_owner_ids = &fac_bytes[save_ids_start..save_ids_end];

        // ACI0 FAH must have minimum 0x1C header bytes
        if fah_bytes.len() < 0x1C {
            return Err(NpdmError::MalformedFsAccessControl {
                section: "ACI0 FAH",
                reason: "descriptor buffer is smaller than 0x1C base header",
            });
        }
        let fah_version = fah_bytes[0];
        if fah_version != 1 {
            return Err(NpdmError::MalformedFsAccessControl {
                section: "ACI0 FAH",
                reason: "unsupported FAH version (must be 1)",
            });
        }
        let fah_permissions = u64::from_le_bytes(fah_bytes[4..12].try_into().unwrap());

        let content_owner_info_offset =
            u32::from_le_bytes(fah_bytes[0x0C..0x10].try_into().unwrap());
        let content_owner_info_size = u32::from_le_bytes(fah_bytes[0x10..0x14].try_into().unwrap());
        let save_data_owner_info_offset =
            u32::from_le_bytes(fah_bytes[0x14..0x18].try_into().unwrap());
        let save_data_owner_info_size =
            u32::from_le_bytes(fah_bytes[0x18..0x1C].try_into().unwrap());

        if content_owner_info_size > 0 {
            if (content_owner_info_size % 8) != 0 {
                return Err(NpdmError::MalformedFsAccessControl {
                    section: "ACI0 FAH",
                    reason: "ContentOwnerInfoSize must be divisible by 8",
                });
            }
            if (content_owner_info_offset as usize) < 0x1C {
                return Err(NpdmError::MalformedFsAccessControl {
                    section: "ACI0 FAH",
                    reason: "ContentOwnerInfoOffset overlaps FAH base header",
                });
            }
            let end = content_owner_info_offset
                .checked_add(content_owner_info_size)
                .ok_or(NpdmError::MalformedFsAccessControl {
                    section: "ACI0 FAH",
                    reason: "ContentOwnerInfo extent overflow",
                })? as usize;
            if end > fah_bytes.len() {
                return Err(NpdmError::MalformedFsAccessControl {
                    section: "ACI0 FAH",
                    reason: "ContentOwnerInfo extent exceeds FAH section buffer",
                });
            }
        } else if content_owner_info_offset != 0
            && (content_owner_info_offset as usize) > fah_bytes.len()
        {
            return Err(NpdmError::MalformedFsAccessControl {
                section: "ACI0 FAH",
                reason: "ContentOwnerInfoOffset exceeds FAH section buffer",
            });
        }

        if save_data_owner_info_size > 0 {
            if (save_data_owner_info_size % 8) != 0 {
                return Err(NpdmError::MalformedFsAccessControl {
                    section: "ACI0 FAH",
                    reason: "SaveDataOwnerInfoSize must be divisible by 8",
                });
            }
            if (save_data_owner_info_offset as usize) < 0x1C {
                return Err(NpdmError::MalformedFsAccessControl {
                    section: "ACI0 FAH",
                    reason: "SaveDataOwnerInfoOffset overlaps FAH base header",
                });
            }
            let end = save_data_owner_info_offset
                .checked_add(save_data_owner_info_size)
                .ok_or(NpdmError::MalformedFsAccessControl {
                    section: "ACI0 FAH",
                    reason: "SaveDataOwnerInfo extent overflow",
                })? as usize;
            if end > fah_bytes.len() {
                return Err(NpdmError::MalformedFsAccessControl {
                    section: "ACI0 FAH",
                    reason: "SaveDataOwnerInfo extent exceeds FAH section buffer",
                });
            }
        } else if save_data_owner_info_offset != 0
            && (save_data_owner_info_offset as usize) > fah_bytes.len()
        {
            return Err(NpdmError::MalformedFsAccessControl {
                section: "ACI0 FAH",
                reason: "SaveDataOwnerInfoOffset exceeds FAH section buffer",
            });
        }

        if content_owner_info_size > 0 && save_data_owner_info_size > 0 {
            let c_start = content_owner_info_offset as usize;
            let c_end = c_start + content_owner_info_size as usize;
            let s_start = save_data_owner_info_offset as usize;
            let s_end = s_start + save_data_owner_info_size as usize;
            if c_start < s_end && s_start < c_end {
                return Err(NpdmError::MalformedFsAccessControl {
                    section: "ACI0 FAH",
                    reason: "ContentOwnerInfo and SaveDataOwnerInfo regions overlap",
                });
            }
        }

        let fah_content_owner_ids = if content_owner_info_size > 0 {
            let start = content_owner_info_offset as usize;
            let end = start + content_owner_info_size as usize;
            &fah_bytes[start..end]
        } else {
            &[]
        };

        let fah_save_data_owner_ids = if save_data_owner_info_size > 0 {
            let start = save_data_owner_info_offset as usize;
            let end = start + save_data_owner_info_size as usize;
            &fah_bytes[start..end]
        } else {
            &[]
        };

        Ok(Some(Self {
            fac_version,
            fah_version,
            content_owner_id_count,
            save_data_owner_id_count,
            fac_permissions,
            fah_permissions,
            effective_permissions: fac_permissions & fah_permissions,
            content_owner_id_min,
            content_owner_id_max,
            save_data_owner_id_min,
            save_data_owner_id_max,
            content_owner_ids,
            save_data_owner_ids,
            fah_content_owner_ids,
            fah_save_data_owner_ids,
        }))
    }

    /// Checks if a given permission bitmask is satisfied by effective permissions.
    pub fn has_permission(&self, mask: u64) -> bool {
        (self.effective_permissions & mask) == mask
    }
}
