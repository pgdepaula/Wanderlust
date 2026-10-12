//! Extended AArch64 instruction decoding for high-frequency Switch / TOTK extensions.
//!
//! Provides strict semantic decoders and structured representations for instruction
//! families discovered during authentic Tears of the Kingdom execution analysis:
//! 1. System counter registers (`CNTPCT_EL0`, `CNTVCT_EL0`, `CNTFRQ_EL0`)
//! 2. CRC32 and CRC32C (Data-processing 2-source, 8-bit to 64-bit)
//! 3. Cryptographic extensions (`AESE`, `AESD`, `AESIMC`, `SHA1C`, `SHA256SU0`, `PMULL`)

use serde::Serialize;
use wl_common::{GuestAddr, Result};

use crate::{Reg, reg, unsupported};

/// System counter registers accessible at EL0.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
pub enum CounterRegister {
    /// CNTPCT_EL0: Physical counter (op0=3, op1=3, CRn=14, CRm=0, op2=1).
    CntpctEl0,
    /// CNTVCT_EL0: Virtual counter (op0=3, op1=3, CRn=14, CRm=0, op2=2).
    CntvctEl0,
    /// CNTFRQ_EL0: Counter frequency (op0=3, op1=3, CRn=14, CRm=0, op2=0).
    CntfrqEl0,
}

/// A decoded system counter instruction (e.g. `mrs Xt, cntpct_el0`).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
pub struct SystemCounterOp {
    pub read: bool,
    pub register: CounterRegister,
    pub reg: Reg,
}

/// A decoded CRC32 or CRC32C instruction.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
pub struct Crc32Op {
    /// Destination register (always W register).
    pub dst: Reg,
    /// Initial CRC accumulator (always W register).
    pub acc: Reg,
    /// Input value (W register for sz 0..=2, X register for sz 3).
    pub val: Reg,
    /// Operand size: 0 = byte, 1 = halfword, 2 = word, 3 = doubleword.
    pub sz: u8,
    /// Polynomial: false = CRC-32 (0x04C11DB7), true = CRC-32C Castagnoli (0x1EDC6F41).
    pub c: bool,
}

/// Cryptographic extension operations.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
pub enum CryptoOp {
    /// AES single round encryption (`AESE Vd.16B, Vn.16B`).
    Aese { dst: u8, src: u8 },
    /// AES single round decryption (`AESD Vd.16B, Vn.16B`).
    Aesd { dst: u8, src: u8 },
    /// AES inverse mix columns (`AESIMC Vd.16B, Vn.16B`).
    Aesimc { dst: u8, src: u8 },
    /// SHA-1 hash update (`SHA1C Qd, Sn, Vm.4S`).
    Sha1c { dst: u8, src1: u8, src2: u8 },
    /// SHA-256 schedule update 0 (`SHA256SU0 Vd.4S, Vn.4S`).
    Sha256su0 { dst: u8, src: u8 },
    /// Polynomial multiply long (`PMULL Vd.1Q, Vn.1D, Vm.1D`).
    Pmull { dst: u8, lhs: u8, rhs: u8 },
}

/// An extended decoded instruction.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
pub enum ExtensionOp {
    Counter(SystemCounterOp),
    Crc(Crc32Op),
    Crypto(CryptoOp),
}

/// Decode system counter read/write operations (e.g. `mrs Xt, cntpct_el0`).
pub fn decode_system_counter(word: u32, pc: GuestAddr) -> Result<Option<SystemCounterOp>> {
    if word & 0xffd00000 == 0xd5100000 {
        let read = word & (1 << 21) != 0;
        let register = match (word >> 5) & 0x7fff {
            0x5f01 => CounterRegister::CntpctEl0,
            0x5f02 => CounterRegister::CntvctEl0,
            0x5f00 => CounterRegister::CntfrqEl0,
            _ => return Ok(None),
        };
        // EL0 may not write timer counter registers.
        if !read {
            return unsupported(word, pc);
        }
        let rd = (word & 0x1f) as u8;
        return Ok(Some(SystemCounterOp {
            read,
            register,
            reg: reg(rd, false),
        }));
    }
    Ok(None)
}

/// Decode CRC32 and CRC32C instructions (`crc32b/h/w/x` and `crc32cb/ch/cw/cx`).
pub fn decode_crc32(word: u32, pc: GuestAddr) -> Result<Option<Crc32Op>> {
    if word & 0x7fe00000 == 0x1ac00000 {
        let sf = word & (1 << 31) != 0;
        let opcode = (word >> 10) & 0x3f;
        if (16..=23).contains(&opcode) {
            let sz = (opcode & 3) as u8;
            let c = (opcode & 4) != 0;
            // In ARMv8.0: sz == 3 (64-bit) requires sf == 1; sz < 3 requires sf == 0.
            // Other combinations are reserved and must be rejected.
            if (sz == 3 && !sf) || (sz < 3 && sf) {
                return unsupported(word, pc);
            }
            let rd = (word & 0x1f) as u8;
            let rn = ((word >> 5) & 0x1f) as u8;
            let rm = ((word >> 16) & 0x1f) as u8;
            return Ok(Some(Crc32Op {
                dst: reg(rd, false),
                acc: reg(rn, false),
                val: reg(rm, false),
                sz,
                c,
            }));
        }
    }
    Ok(None)
}

/// Decode cryptographic extension instructions (`AES`, `SHA`, `PMULL`).
pub fn decode_crypto(word: u32, _pc: GuestAddr) -> Result<Option<CryptoOp>> {
    // AES single round encryption: AESE Vd.16B, Vn.16B (4e284800, mask 0xfffffc00)
    if word & 0xfffffc00 == 0x4e284800 {
        return Ok(Some(CryptoOp::Aese {
            dst: (word & 0x1f) as u8,
            src: ((word >> 5) & 0x1f) as u8,
        }));
    }
    // AES single round decryption: AESD Vd.16B, Vn.16B (4e285800, mask 0xfffffc00)
    if word & 0xfffffc00 == 0x4e285800 {
        return Ok(Some(CryptoOp::Aesd {
            dst: (word & 0x1f) as u8,
            src: ((word >> 5) & 0x1f) as u8,
        }));
    }
    // AES inverse mix columns: AESIMC Vd.16B, Vn.16B (4e287800, mask 0xfffffc00)
    if word & 0xfffffc00 == 0x4e287800 {
        return Ok(Some(CryptoOp::Aesimc {
            dst: (word & 0x1f) as u8,
            src: ((word >> 5) & 0x1f) as u8,
        }));
    }
    // SHA-1 hash update: SHA1C Qd, Sn, Vm.4S (5e000000, mask 0xffe0fc00)
    if word & 0xffe0fc00 == 0x5e000000 {
        return Ok(Some(CryptoOp::Sha1c {
            dst: (word & 0x1f) as u8,
            src1: ((word >> 5) & 0x1f) as u8,
            src2: ((word >> 16) & 0x1f) as u8,
        }));
    }
    // SHA-256 schedule update 0: SHA256SU0 Vd.4S, Vn.4S (5e282800, mask 0xfffffc00)
    if word & 0xfffffc00 == 0x5e282800 {
        return Ok(Some(CryptoOp::Sha256su0 {
            dst: (word & 0x1f) as u8,
            src: ((word >> 5) & 0x1f) as u8,
        }));
    }
    // Polynomial multiply long: PMULL Vd.1Q, Vn.1D, Vm.1D (0ee0e000, mask 0xffe0fc00)
    if word & 0xffe0fc00 == 0x0ee0e000 {
        return Ok(Some(CryptoOp::Pmull {
            dst: (word & 0x1f) as u8,
            lhs: ((word >> 5) & 0x1f) as u8,
            rhs: ((word >> 16) & 0x1f) as u8,
        }));
    }
    Ok(None)
}

/// Decode any supported extension instruction.
pub fn decode_extension(word: u32, pc: GuestAddr) -> Result<Option<ExtensionOp>> {
    if let Some(counter) = decode_system_counter(word, pc)? {
        return Ok(Some(ExtensionOp::Counter(counter)));
    }
    if let Some(crc) = decode_crc32(word, pc)? {
        return Ok(Some(ExtensionOp::Crc(crc)));
    }
    if let Some(crypto) = decode_crypto(word, pc)? {
        return Ok(Some(ExtensionOp::Crypto(crypto)));
    }
    Ok(None)
}
