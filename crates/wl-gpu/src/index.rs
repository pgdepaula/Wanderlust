//! B197 index descriptors and BEGIN/FIRST/COUNT/END triangle-list subset.
use crate::{
    Result, invalid,
    maxwell::{MethodEffect, MethodError},
    memory::GpuAddress,
};
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct IndexBuffer {
    pub address: GpuAddress,
    pub bytes: usize,
    pub width: u8,
    pub base: i32,
}
#[derive(Debug, Default)]
pub(crate) struct IndexRegisters {
    words: [Option<u32>; 5],
    first: Option<u32>,
    base: Option<i32>,
    instance: Option<u32>,
    restart: Option<u32>,
    pub active: bool,
}
impl IndexRegisters {
    pub fn apply(
        &mut self,
        method: u16,
        value: u32,
    ) -> std::result::Result<Option<MethodEffect>, MethodError> {
        let bad = || MethodError::UnsupportedValue { method, value };
        match method {
            0x17c8..=0x17d8 => {
                let n = ((method - 0x17c8) / 4) as usize;
                if (n == 0 || n == 2) && value > 255 || n == 4 && !matches!(value, 1 | 2) {
                    return Err(bad());
                }
                self.words[n] = Some(value);
            }
            0x17dc => self.first = Some(value),
            0x1434 => self.base = Some(value as i32),
            0x1438 => {
                if value != 0 {
                    return Err(bad());
                }
                self.instance = Some(value);
            }
            0x1644 => {
                if value != 0 {
                    return Err(bad());
                }
                self.restart = Some(value);
            }
            0x1618 => {
                if value != 4 || self.active {
                    return Err(bad());
                }
                self.active = true;
            }
            0x1614 => {
                if value != 0 || !self.active {
                    return Err(bad());
                }
                self.active = false;
            }
            0x17e0 => {
                if !self.active || value == 0 || value % 3 != 0 {
                    return Err(bad());
                }
                return Ok(Some(MethodEffect::DrawIndices {
                    first: self.first.ok_or_else(bad)?,
                    count: value,
                }));
            }
            _ => return Ok(None),
        }
        Ok(Some(MethodEffect::State))
    }
    pub fn resolve(&self, first: u32, count: u32) -> Result<IndexBuffer> {
        let mut w = [0; 5];
        for (a, b) in w.iter_mut().zip(self.words) {
            *a = b.ok_or_else(|| invalid("incomplete index descriptor"))?;
        }
        if self.restart != Some(0) || self.instance != Some(0) {
            return Err(invalid("missing explicit index restart/instance state"));
        }
        let base = self.base.ok_or_else(|| invalid("missing base vertex"))?;
        let width = if w[4] == 1 { 2 } else { 4 };
        let start = (u64::from(w[0]) << 32) | u64::from(w[1]);
        let limit = (u64::from(w[2]) << 32) | u64::from(w[3]);
        let bytes = u64::from(count)
            .checked_mul(width)
            .ok_or_else(|| invalid("index size overflow"))?;
        if count > 1 << 20 {
            return Err(invalid(
                "unsupported index scan budget exceeds one million elements",
            ));
        }
        let address = start
            .checked_add(u64::from(first) * width)
            .ok_or_else(|| invalid("index offset overflow"))?;
        if count == 0
            || count % 3 != 0
            || start % width != 0
            || limit < start
            || address
                .checked_add(bytes)
                .is_none_or(|e| e > 1 << 40 || e - 1 > limit)
        {
            return Err(invalid("invalid index alignment or inclusive range"));
        }
        Ok(IndexBuffer {
            address: GpuAddress(address),
            bytes: usize::try_from(bytes).map_err(|_| invalid("index span exceeds host"))?,
            width: width as u8,
            base,
        })
    }
}
