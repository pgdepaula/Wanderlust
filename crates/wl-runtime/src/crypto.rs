//! System counter, CRC32/CRC32C and the cryptographic-extension instructions recognised by
//! `wl_aarch64::extensions`, following the Arm Architecture Reference Manual pseudocode
//! (`AESSubBytes`, `AESShiftRows`, `AESInvMixColumns`, `SHA1C`, `SHA256SU0`, `PolynomialMult`).
use crate::{Cpu, Process};

/// CNTFRQ_EL0 of the Switch (19.2 MHz).
pub const COUNTER_FREQUENCY: u64 = 19_200_000;

/// CNTPCT_EL0 / CNTVCT_EL0: in a Horizon process, the deterministic virtual clock that
/// svcGetSystemTick also reports; in harness mode, the retired-instruction count at one
/// instruction per nanosecond.
pub fn system_counter(c: &Cpu, m: &Process) -> u64 {
    let ns = m
        .kernel
        .as_ref()
        .map_or(c.steps, |k| crate::threads::now_ns(k, c));
    crate::threads::ticks(ns)
}

/// CRC32 (polynomial 0x04C11DB7) or CRC32C (0x1EDC6F41) of the low `bytes` bytes of
/// `value`, least significant byte first, bit-reflected, without pre- or post-inversion.
pub fn crc32(acc: u64, value: u64, bytes: u8, castagnoli: bool) -> u64 {
    let poly: u32 = if castagnoli { 0x82F6_3B78 } else { 0xEDB8_8320 };
    let mut crc = acc as u32;
    for i in 0..u32::from(bytes) {
        crc ^= (value >> (8 * i)) as u32 & 0xff;
        for _ in 0..8 {
            crc = (crc >> 1) ^ if crc & 1 != 0 { poly } else { 0 };
        }
    }
    u64::from(crc)
}

const fn xtime(b: u8) -> u8 {
    (b << 1) ^ if b & 0x80 != 0 { 0x1b } else { 0 }
}
const fn gmul(mut a: u8, mut b: u8) -> u8 {
    let mut p = 0u8;
    while b != 0 {
        if b & 1 != 0 {
            p ^= a;
        }
        a = xtime(a);
        b >>= 1;
    }
    p
}
/// The AES S-box: multiplicative inverse in GF(2^8) followed by the affine transform.
const fn sbox() -> [u8; 256] {
    let mut s = [0u8; 256];
    let mut x = 0usize;
    while x < 256 {
        // Inverse by exponentiation: x^254.
        let mut inv = 1u8;
        let mut i = 0;
        while i < 254 {
            inv = gmul(inv, x as u8);
            i += 1;
        }
        if x == 0 {
            inv = 0;
        }
        let b = inv;
        s[x] = b ^ b.rotate_left(1) ^ b.rotate_left(2) ^ b.rotate_left(3) ^ b.rotate_left(4) ^ 0x63;
        x += 1;
    }
    s
}
const fn inverse(table: [u8; 256]) -> [u8; 256] {
    let mut out = [0u8; 256];
    let mut i = 0;
    while i < 256 {
        out[table[i] as usize] = i as u8;
        i += 1;
    }
    out
}
const SBOX: [u8; 256] = sbox();
const INV_SBOX: [u8; 256] = inverse(SBOX);
/// AESShiftRows / AESInvShiftRows as byte permutations (byte i = Elem[op, i, 8]).
const SHIFT_ROWS: [usize; 16] = [0, 5, 10, 15, 4, 9, 14, 3, 8, 13, 2, 7, 12, 1, 6, 11];
const INV_SHIFT_ROWS: [usize; 16] = [0, 13, 10, 7, 4, 1, 14, 11, 8, 5, 2, 15, 12, 9, 6, 3];

fn permute_substitute(state: u128, order: &[usize; 16], table: &[u8; 256]) -> u128 {
    let b = state.to_le_bytes();
    let mut out = [0u8; 16];
    for (i, o) in out.iter_mut().enumerate() {
        *o = table[b[order[i]] as usize];
    }
    u128::from_le_bytes(out)
}
/// AESE: SubBytes(ShiftRows(Vd XOR Vn)).
pub fn aese(d: u128, n: u128) -> u128 {
    permute_substitute(d ^ n, &SHIFT_ROWS, &SBOX)
}
/// AESD: InvSubBytes(InvShiftRows(Vd XOR Vn)).
pub fn aesd(d: u128, n: u128) -> u128 {
    permute_substitute(d ^ n, &INV_SHIFT_ROWS, &INV_SBOX)
}
/// AESIMC: InvMixColumns(Vn), columns of four consecutive bytes.
pub fn aesimc(n: u128) -> u128 {
    let b = n.to_le_bytes();
    let mut out = [0u8; 16];
    for c in 0..4 {
        let col = &b[4 * c..4 * c + 4];
        for r in 0..4 {
            out[4 * c + r] = gmul(col[r], 0x0e)
                ^ gmul(col[(r + 1) % 4], 0x0b)
                ^ gmul(col[(r + 2) % 4], 0x0d)
                ^ gmul(col[(r + 3) % 4], 0x09);
        }
    }
    u128::from_le_bytes(out)
}
/// SHA1C Qd, Sn, Vm.4S: four rounds with the "choose" function.
pub fn sha1c(d: u128, n: u128, m: u128) -> u128 {
    let mut x = d;
    let mut y = n as u32;
    for e in 0..4 {
        let x1 = (x >> 32) as u32;
        let x2 = (x >> 64) as u32;
        let x3 = (x >> 96) as u32;
        let t = ((x2 ^ x3) & x1) ^ x3;
        y = y
            .wrapping_add((x as u32).rotate_left(5))
            .wrapping_add(t)
            .wrapping_add((m >> (32 * e)) as u32);
        let x1 = x1.rotate_left(30);
        x = (x & !(0xffff_ffffu128 << 32)) | (u128::from(x1) << 32);
        // <y, X> = ROL(y : X, 32)
        let top = (x >> 96) as u32;
        x = (x << 32) | u128::from(y);
        y = top;
    }
    x
}
/// SHA256SU0 Vd.4S, Vn.4S.
pub fn sha256su0(d: u128, n: u128) -> u128 {
    let t = (u128::from(n as u32) << 96) | (d >> 32);
    let mut out = 0u128;
    for e in 0..4 {
        let elt = (t >> (32 * e)) as u32;
        let s = elt.rotate_right(7) ^ elt.rotate_right(18) ^ (elt >> 3);
        out |= u128::from(s.wrapping_add((d >> (32 * e)) as u32)) << (32 * e);
    }
    out
}
/// PMULL Vd.1Q, Vn.1D, Vm.1D: carry-less 64 x 64 -> 128 multiplication.
pub fn pmull64(a: u64, b: u64) -> u128 {
    let mut out = 0u128;
    for i in 0..64 {
        if b >> i & 1 != 0 {
            out ^= u128::from(a) << i;
        }
    }
    out
}
