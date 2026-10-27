//! Read-only function candidates from the LSB 4.1 `.eh_frame_hdr` index.
//! This does not execute CFI, personality routines, or guest exception unwinding.
//! Format: https://refspecs.linuxfoundation.org/LSB_4.1.0/LSB-Core-generic/LSB-Core-generic/ehframechpt.html
use crate::Image;
use std::collections::BTreeSet;
use wl_common::{GuestAddr, Result};

pub const MAX_FUNCTIONS: usize = 1_048_576;
const MAX_CIES: usize = 4096;

#[derive(Debug, serde::Serialize)]
pub struct Function {
    pub start: GuestAddr,
    pub length: u64,
    pub fde: GuestAddr,
}
#[derive(Debug, serde::Serialize)]
pub struct FunctionIndex {
    pub header: GuestAddr,
    pub frame: GuestAddr,
    pub functions: Vec<Function>,
}
fn bytes(image: &Image, address: u64, size: u64) -> Result<&[u8]> {
    address.checked_add(size).ok_or("unwind address overflow")?;
    for m in &image.mappings {
        if !m.read || m.write {
            continue;
        }
        let Some(at) = address.checked_sub(m.base.0) else {
            continue;
        };
        let Some(end) = at.checked_add(size) else {
            continue;
        };
        if end <= m.bytes.len() as u64 {
            return Ok(&m.bytes[at as usize..end as usize]);
        }
    }
    Err("unwind range is not in one readable immutable mapping".into())
}
fn u32_at(data: &[u8], at: usize) -> u32 {
    u32::from_le_bytes(data[at..at + 4].try_into().unwrap())
}
fn relative(base: u64, value: u32) -> Result<u64> {
    base.checked_add_signed(i64::from(value as i32))
        .ok_or_else(|| "unwind relative address overflow".into())
}
fn record(image: &Image, address: u64, minimum: u32, maximum: u32) -> Result<&[u8]> {
    let size = u32_at(bytes(image, address, 4)?, 0);
    if size < minimum || size > maximum {
        return Err("unsupported or truncated unwind record length".into());
    }
    bytes(image, address, u64::from(size) + 4)
}
struct Cursor<'a> {
    data: &'a [u8],
    at: usize,
}
impl Cursor<'_> {
    fn byte(&mut self) -> Result<u8> {
        let b = *self.data.get(self.at).ok_or("truncated CIE augmentation")?;
        self.at += 1;
        Ok(b)
    }
    fn leb(&mut self, signed: bool) -> Result<u64> {
        let mut value = 0;
        for i in 0..10 {
            let b = self.byte()?;
            if i == 9 && ((!signed && b & 127 > 1) || (signed && !matches!(b & 127, 0 | 127))) {
                return Err("CIE LEB128 overflow".into());
            }
            value |= u64::from(b & 127) << (7 * i);
            if b & 128 == 0 {
                return Ok(value);
            }
        }
        Err("unterminated CIE LEB128".into())
    }
}
fn check_cie(image: &Image, address: u64) -> Result<()> {
    let data = record(image, address, 9, 4096)?;
    if u32_at(data, 4) != 0 || data[8] != 1 {
        return Err("unsupported CIE identifier/version".into());
    }
    let mut c = Cursor { data, at: 9 };
    let mut augmentation = Vec::new();
    loop {
        let b = c.byte()?;
        if b == 0 {
            break;
        }
        if augmentation.len() == 16 {
            return Err("CIE augmentation name too long".into());
        }
        augmentation.push(b);
    }
    if augmentation.first() != Some(&b'z') {
        return Err("unsupported CIE augmentation".into());
    }
    if c.leb(false)? == 0 {
        return Err("zero CIE code alignment".into());
    }
    c.leb(true)?; // Data alignment is not interpreted for function discovery.
    c.byte()?; // Version-one return-address register.
    let length = usize::try_from(c.leb(false)?).map_err(|_| "CIE augmentation length overflow")?;
    let end =
        c.at.checked_add(length)
            .filter(|end| *end <= data.len())
            .ok_or("truncated CIE augmentation data")?;
    let mut fields = 0u8;
    for field in &augmentation[1..] {
        let flag = match field {
            b'P' => 1,
            b'L' => 2,
            b'R' => 4,
            _ => return Err("unsupported CIE augmentation field".into()),
        };
        if fields & flag != 0 {
            return Err("duplicate CIE augmentation field".into());
        }
        fields |= flag;
        match field {
            b'P' => {
                let encoding = c.byte()?;
                if !matches!(encoding & 0x70, 0 | 0x10 | 0x20 | 0x30 | 0x40) {
                    return Err("unsupported personality pointer encoding".into());
                }
                let width = match encoding & 15 {
                    0 | 4 | 12 => 8,
                    2 | 10 => 2,
                    3 | 11 => 4,
                    _ => return Err("unsupported personality pointer width".into()),
                };
                // Only skip the encoded value: it is never dereferenced or executed.
                for _ in 0..width {
                    c.byte()?;
                }
            }
            b'L' => {
                c.byte()?;
            }
            b'R' => {
                if c.byte()? != 0x1b {
                    return Err("unsupported FDE pointer encoding".into());
                }
            }
            _ => unreachable!(),
        }
        if c.at > end {
            return Err("CIE augmentation exceeds declared length".into());
        }
    }
    if fields & 4 == 0 || c.at != end {
        return Err("missing FDE encoding or unmatched augmentation length".into());
    }
    Ok(())
}
/// Supported index: version 1, PC-relative signed32 frame pointer, unsigned32
/// count, data-relative signed32 table; version-one zR/zPLR CIEs with signed32
/// PC-relative FDE locations. Unsupported layouts fail before returning roots.
pub fn function_index(image: &Image, header: GuestAddr, size: u64) -> Result<FunctionIndex> {
    if size < 12 {
        return Err("truncated unwind index header".into());
    }
    let data = bytes(image, header.0, size)?;
    if data[..4] != [1, 0x1b, 3, 0x3b] {
        return Err("unsupported unwind index encoding/version".into());
    }
    let count = u32_at(data, 8) as usize;
    if count > MAX_FUNCTIONS {
        return Err("unwind function budget exceeded".into());
    }
    if size != 12 + count as u64 * 8 {
        return Err("unwind index length does not match entry count".into());
    }
    let frame = relative(
        header
            .0
            .checked_add(4)
            .ok_or("unwind header address overflow")?,
        u32_at(data, 4),
    )?;
    let mut functions = Vec::new();
    functions
        .try_reserve_exact(count)
        .map_err(|e| e.to_string())?;
    let mut cies = BTreeSet::new();
    let mut previous = None;
    for i in 0..count {
        let start = relative(header.0, u32_at(data, 12 + i * 8))?;
        if previous.is_some_and(|p| start < p) {
            return Err("unsorted unwind function index".into());
        }
        previous = Some(start);
        let fde = relative(header.0, u32_at(data, 16 + i * 8))?;
        if fde < frame {
            return Err("FDE precedes indexed frame section".into());
        }
        let entry = record(image, fde, 12, 1024 * 1024)?;
        if u32_at(entry, 4) == 0 {
            return Err("FDE has no backward CIE reference".into());
        }
        let cie = fde
            .checked_add(4)
            .and_then(|p| p.checked_sub(u64::from(u32_at(entry, 4))))
            .filter(|p| *p >= frame)
            .ok_or("invalid FDE CIE reference")?;
        if !cies.contains(&cie) {
            if cies.len() >= MAX_CIES {
                return Err("unwind CIE budget exceeded".into());
            }
            check_cie(image, cie)?;
            cies.insert(cie);
        }
        let location = relative(
            fde.checked_add(8).ok_or("FDE location address overflow")?,
            u32_at(entry, 8),
        )?;
        if location != start {
            return Err("FDE location disagrees with search index".into());
        }
        let length = u64::from(u32_at(entry, 12));
        let end = start
            .checked_add(length)
            .ok_or("unwind function range overflow")?;
        if start % 4 != 0
            || length == 0
            || length % 4 != 0
            || length > i32::MAX as u64
            || !image.mappings.iter().any(|m| {
                m.read
                    && m.execute
                    && !m.write
                    && start >= m.base.0
                    && end - m.base.0 <= m.bytes.len() as u64
            })
        {
            return Err("unwind function range is outside immutable executable memory".into());
        }
        functions.push(Function {
            start: GuestAddr(start),
            length,
            fde: GuestAddr(fde),
        });
    }
    Ok(FunctionIndex {
        header,
        frame: GuestAddr(frame),
        functions,
    })
}
