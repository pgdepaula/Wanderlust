//! Dependency-free guest runtime linked into AOT executables as an rlib.
//! No guest address is converted into a host pointer; no unsafe code is used.
pub mod crypto;
pub mod display;
pub mod fp;
pub mod gpu_memory;
pub mod horizon;
pub mod input;
pub mod ipc;
pub mod nvidia;
pub mod sdl;
pub mod services;
pub mod simd;
pub mod storage;
pub mod threads;
use std::io::Write;

pub const SCRATCH_BASE: u64 = 0x1000_0000;
pub const SCRATCH_SIZE: usize = 0x10000;
pub const RETURN_PC: u64 = 0xffff_ffff_ffff_fffc;
pub type RuntimeResult<T> = std::result::Result<T, String>;
/// A translated guest basic block: runs to its end, then returns `Some(status)` to stop.
pub type BlockFn = fn(&mut Cpu, &mut horizon::Process) -> RuntimeResult<Option<Stop>>;
/// Why translated code returned control to its caller instead of continuing at `Cpu::pc`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Stop {
    /// The harness return token was reached (harness mode).
    Returned,
    /// svcExitProcess.
    ExitProcess,
    /// The last thread of the process exited.
    ExitThread,
    /// The current thread must be rescheduled: it blocked, yielded, exited while other
    /// threads remain, or its time slice ended. `Cpu::pc` is where it resumes.
    Reschedule,
}
impl Stop {
    pub fn as_str(self) -> &'static str {
        match self {
            Stop::Returned => "returned",
            Stop::ExitProcess => "exit_process",
            Stop::ExitThread => "exit_thread",
            Stop::Reschedule => "reschedule",
        }
    }
}
pub use crypto::{
    COUNTER_FREQUENCY, aesd, aese, aesimc, crc32, pmull64, sha1c, sha256su0, system_counter,
};
pub use horizon::Process;
/// One translation unit's blocks, sorted by guest address.
pub type BlockTable = &'static [(u64, BlockFn)];
/// FPCR bits writable on the modelled CPU (AHP, DN, FZ, RMode, Stride, Len; no trap enables).
pub const FPCR_WRITE_MASK: u64 = 0x07f7_0000;
/// FPSR bits writable on the modelled CPU (N, Z, C, V, QC, IDC, IXC, UFC, OFC, DZC, IOC).
pub const FPSR_WRITE_MASK: u64 = 0xf800_009f;
#[derive(Debug)]
pub struct Region {
    pub base: u64,
    pub bytes: Vec<u8>,
    pub read: bool,
    pub write: bool,
    pub execute: bool,
    /// `svc::MemoryState` reported by memory queries.
    pub state: u32,
    /// `svc::MemoryAttribute` bits (Uncached, PermissionLocked) set through
    /// `svcSetMemoryAttribute`.
    pub attribute: u32,
}
impl Region {
    pub fn end(&self) -> u64 {
        self.base + self.bytes.len() as u64
    }
}
#[derive(Debug, Default)]
pub struct Memory {
    /// Disjoint regions sorted by base address (maintained by `map_state` and `split_at`).
    pub regions: Vec<Region>,
    gpu_pins: Vec<gpu_memory::PinRange>,
}
impl Memory {
    pub fn map(
        &mut self,
        base: u64,
        bytes: Vec<u8>,
        read: bool,
        write: bool,
        execute: bool,
    ) -> RuntimeResult<()> {
        self.map_state(base, bytes, read, write, execute, horizon::state::STATIC)
    }
    /// Maps a region with an explicit Horizon memory state; overlap is an error.
    pub fn map_state(
        &mut self,
        base: u64,
        bytes: Vec<u8>,
        read: bool,
        write: bool,
        execute: bool,
        state: u32,
    ) -> RuntimeResult<()> {
        let end = base
            .checked_add(bytes.len() as u64)
            .ok_or("mapping address overflow")?;
        if bytes.is_empty() {
            return Ok(());
        }
        if self
            .regions
            .iter()
            .any(|r| base < r.base + r.bytes.len() as u64 && r.base < end)
        {
            return Err(format!("overlapping guest mapping at {base:#x}"));
        }
        // Regions stay sorted by base so lookups can binary-search.
        let at = self.regions.partition_point(|r| r.base < base);
        self.regions.insert(
            at,
            Region {
                base,
                bytes,
                read,
                write,
                execute,
                state,
                attribute: 0,
            },
        );
        Ok(())
    }
    /// Splits the region that strictly contains `address` in two at `address`; both halves
    /// keep its permissions, state and attribute. Returns whether a split happened.
    pub fn split_at(&mut self, address: u64) -> bool {
        if self.gpu_split_blocked(address) {
            return false;
        }
        let Some(i) = self
            .regions
            .iter()
            .position(|r| r.base < address && address < r.end())
        else {
            return false;
        };
        let r = &mut self.regions[i];
        let tail = Region {
            base: address,
            bytes: r.bytes.split_off((address - r.base) as usize),
            read: r.read,
            write: r.write,
            execute: r.execute,
            state: r.state,
            attribute: r.attribute,
        };
        self.regions.insert(i + 1, tail);
        true
    }
    /// Fast path: the access lies inside one region that grants it.
    #[inline]
    fn locate(&self, address: u64, size: usize, write: bool) -> Option<(usize, usize)> {
        let end = address.checked_add(size as u64)?;
        let i = self.region_index(address)?;
        let r = &self.regions[i];
        if end > r.base + r.bytes.len() as u64 || (write && !r.write) || (!write && !r.read) {
            return None;
        }
        Some((i, (address - r.base) as usize))
    }
    /// Index of the region containing `address` (binary search over sorted regions).
    #[inline]
    pub fn region_index(&self, address: u64) -> Option<usize> {
        let i = self
            .regions
            .partition_point(|r| r.base <= address)
            .checked_sub(1)?;
        (address < self.regions[i].end()).then_some(i)
    }
    /// Slow path: an access spanning adjacent regions (as on hardware, where neighbouring
    /// pages with the same permission are contiguous) is split into per-region pieces
    /// `(region, offset, length)`; every byte must be mapped and grant the access, otherwise
    /// the access faults as a whole before any byte is transferred.
    #[cold]
    fn pieces(
        &self,
        address: u64,
        size: usize,
        write: bool,
    ) -> RuntimeResult<Vec<(usize, usize, usize)>> {
        let access = if write { "write" } else { "read" };
        address
            .checked_add(size as u64)
            .ok_or_else(|| format!("guest memory address overflow at {address:#x}"))?;
        let mut pieces = Vec::new();
        let (mut cursor, mut left) = (address, size);
        while left > 0 {
            let Some(i) = self
                .regions
                .iter()
                .position(|r| r.base <= cursor && cursor < r.end())
            else {
                return Err(format!(
                    "unmapped guest memory: {access} {size} bytes at {address:#x}"
                ));
            };
            let r = &self.regions[i];
            if (write && !r.write) || (!write && !r.read) {
                return Err(format!(
                    "guest memory permission fault: {access} {size} bytes at {address:#x}"
                ));
            }
            let n = left.min((r.end() - cursor) as usize);
            pieces.push((i, (cursor - r.base) as usize, n));
            cursor += n as u64;
            left -= n;
        }
        Ok(pieces)
    }
    /// Borrows `size` bytes that lie in one region; spanning reads must use [`Self::read_into`].
    #[inline]
    pub fn read_bytes(&self, address: u64, size: usize) -> RuntimeResult<&[u8]> {
        if size == 0 {
            return Ok(&[]);
        }
        if let Some((i, at)) = self.locate(address, size, false) {
            return Ok(&self.regions[i].bytes[at..at + size]);
        }
        let pieces = self.pieces(address, size, false)?;
        let (i, at, n) = pieces[0];
        if n == size {
            return Ok(&self.regions[i].bytes[at..at + size]);
        }
        Err(format!(
            "guest read of {size} bytes at {address:#x} spans regions"
        ))
    }
    #[inline]
    pub fn read_into(&self, address: u64, out: &mut [u8]) -> RuntimeResult<()> {
        if let Some((i, at)) = self.locate(address, out.len(), false) {
            out.copy_from_slice(&self.regions[i].bytes[at..at + out.len()]);
            return Ok(());
        }
        self.read_spanning(address, out)
    }
    #[cold]
    fn read_spanning(&self, address: u64, out: &mut [u8]) -> RuntimeResult<()> {
        let mut done = 0;
        for (i, at, n) in self.pieces(address, out.len(), false)? {
            out[done..done + n].copy_from_slice(&self.regions[i].bytes[at..at + n]);
            done += n;
        }
        Ok(())
    }
    /// Checks the complete destination range without allocating guest pages or writing bytes.
    /// A later write is atomic with respect to mapping/permission failures while the caller
    /// retains exclusive access to this Memory.
    pub fn validate_write(&self, address: u64, size: usize) -> RuntimeResult<()> {
        if size == 0 || self.locate(address, size, true).is_some() {
            return Ok(());
        }
        self.pieces(address, size, true).map(|_| ())
    }
    #[inline]
    pub fn write_bytes(&mut self, address: u64, bytes: &[u8]) -> RuntimeResult<()> {
        if bytes.is_empty() {
            return Ok(());
        }
        if let Some((i, at)) = self.locate(address, bytes.len(), true) {
            self.regions[i].bytes[at..at + bytes.len()].copy_from_slice(bytes);
            return Ok(());
        }
        self.write_spanning(address, bytes)
    }
    #[cold]
    fn write_spanning(&mut self, address: u64, bytes: &[u8]) -> RuntimeResult<()> {
        let mut done = 0;
        for (i, at, n) in self.pieces(address, bytes.len(), true)? {
            self.regions[i].bytes[at..at + n].copy_from_slice(&bytes[done..done + n]);
            done += n;
        }
        Ok(())
    }
    #[inline]
    pub fn load(&self, address: u64, bytes: u8) -> RuntimeResult<u64> {
        if ![1, 2, 4, 8].contains(&bytes) {
            return Err("invalid guest access width".into());
        }
        let mut value = [0u8; 8];
        self.read_into(address, &mut value[..bytes as usize])?;
        Ok(u64::from_le_bytes(value))
    }
    #[inline]
    pub fn load128(&self, address: u64) -> RuntimeResult<u128> {
        let mut value = [0u8; 16];
        self.read_into(address, &mut value)?;
        Ok(u128::from_le_bytes(value))
    }
    /// Little-endian load of 1..=16 bytes, zero-extended.
    #[inline]
    pub fn load_vector(&self, address: u64, bytes: u8) -> RuntimeResult<u128> {
        if !(1..=16).contains(&bytes) {
            return Err("invalid guest vector access width".into());
        }
        let mut value = [0u8; 16];
        self.read_into(address, &mut value[..bytes as usize])?;
        Ok(u128::from_le_bytes(value))
    }
    /// Little-endian store of the low 1..=16 bytes of `value`.
    #[inline]
    pub fn store_vector(&mut self, address: u64, bytes: u8, value: u128) -> RuntimeResult<()> {
        if !(1..=16).contains(&bytes) {
            return Err("invalid guest vector access width".into());
        }
        self.write_bytes(address, &value.to_le_bytes()[..bytes as usize])
    }
    #[inline]
    pub fn store(&mut self, address: u64, bytes: u8, value: u64) -> RuntimeResult<()> {
        if ![1, 2, 4, 8].contains(&bytes) {
            return Err("invalid guest access width".into());
        }
        self.write_bytes(address, &value.to_le_bytes()[..bytes as usize])
    }
    #[inline]
    pub fn check_execute(&self, pc: u64) -> RuntimeResult<()> {
        if pc % 4 != 0 {
            return Err(format!("unaligned branch target {pc:#x}"));
        }
        if self
            .region_index(pc)
            .map(|i| &self.regions[i])
            .is_some_and(|r| r.execute && pc + 4 <= r.end())
        {
            Ok(())
        } else {
            Err(format!("guest execute permission fault at {pc:#x}"))
        }
    }
}
#[derive(Clone, Debug)]
pub struct Cpu {
    pub x: [u64; 31],
    /// SIMD&FP registers V0-V31. B/H/S/D/Q are the low 8/16/32/64/128 bits of the same
    /// storage; lane i of an e-bit arrangement occupies bits [i*e, (i+1)*e).
    pub v: [u128; 32],
    pub sp: u64,
    pub pc: u64,
    pub nzcv: u8,
    pub steps: u64,
    pub limit: u64,
    pub tpidr_el0: u64,
    /// Read-only at EL0; the process environment sets it (Horizon: thread-local region).
    pub tpidrro_el0: u64,
    pub fpcr: u64,
    pub fpsr: u64,
    /// Local exclusive monitor: (address, size) tagged by the last load-exclusive.
    pub monitor: Option<(u64, u64)>,
    /// Start address and instruction count of the block being executed.
    pub block: (u64, u64),
}
impl Cpu {
    pub fn new(entry: u64) -> Self {
        let mut x = [0; 31];
        x[30] = RETURN_PC;
        Self {
            x,
            v: [0; 32],
            sp: SCRATCH_BASE + SCRATCH_SIZE as u64,
            pc: entry,
            nzcv: 0,
            steps: 0,
            limit: 1_000_000,
            tpidr_el0: 0,
            tpidrro_el0: 0,
            fpcr: 0,
            fpsr: 0,
            monitor: None,
            block: (0, 0),
        }
    }
    /// Block entry: accounts for all `count` instructions of the block at once. A block that
    /// would exceed the budget is not started. `execute` corrects the count after a fault.
    #[inline]
    pub fn enter(&mut self, start: u64, count: u64) -> RuntimeResult<()> {
        if self.steps + count > self.limit {
            self.pc = start;
            self.block = (start, 0);
            return Err(format!(
                "instruction budget exhausted at guest PC {start:#x}"
            ));
        }
        self.steps += count;
        self.block = (start, count);
        Ok(())
    }
    #[inline]
    pub fn tick(&mut self, pc: u64) -> RuntimeResult<()> {
        self.pc = pc;
        if self.steps >= self.limit {
            return Err(format!("instruction budget exhausted at guest PC {pc:#x}"));
        }
        self.steps += 1;
        Ok(())
    }
    #[inline]
    pub fn flags(&mut self, a: u64, b: u64, result: u64, bits: u8, sub: bool) {
        let mask = mask(bits);
        let a = a & mask;
        let b = b & mask;
        let r = result & mask;
        let sign = 1u64 << (bits - 1);
        let carry = if sub {
            a >= b
        } else {
            u128::from(a) + u128::from(b) > u128::from(mask)
        };
        let overflow = if sub {
            (a ^ b) & (a ^ r) & sign != 0
        } else {
            (!(a ^ b)) & (a ^ r) & sign != 0
        };
        self.nzcv = ((r & sign != 0) as u8) << 3
            | ((r == 0) as u8) << 2
            | (carry as u8) << 1
            | overflow as u8;
    }
    #[inline]
    pub fn condition(&self, cond: u8) -> bool {
        let n = self.nzcv & 8 != 0;
        let z = self.nzcv & 4 != 0;
        let c = self.nzcv & 2 != 0;
        let v = self.nzcv & 1 != 0;
        let value = match cond >> 1 {
            0 => z,
            1 => c,
            2 => n,
            3 => v,
            4 => c && !z,
            5 => n == v,
            6 => !z && n == v,
            _ => true,
        };
        if cond & 1 != 0 && cond != 15 {
            !value
        } else {
            value
        }
    }
    pub fn report(&self, status: &str) -> String {
        let registers = self
            .x
            .iter()
            .map(u64::to_string)
            .collect::<Vec<_>>()
            .join(",");
        let vectors = self
            .v
            .iter()
            .map(|v| format!("\"{v:#x}\""))
            .collect::<Vec<_>>()
            .join(",");
        format!(
            "{{\"status\":\"{status}\",\"x\":[{registers}],\"v\":[{vectors}],\"sp\":{},\"pc\":{},\"nzcv\":{},\"fpcr\":{},\"fpsr\":{},\"steps\":{}}}",
            self.sp, self.pc, self.nzcv, self.fpcr, self.fpsr, self.steps
        )
    }
}
#[inline]
pub fn mask(bits: u8) -> u64 {
    if bits == 32 {
        u32::MAX as u64
    } else {
        u64::MAX
    }
}
#[inline]
pub fn shift(value: u64, kind: u8, amount: u8, bits: u8) -> u64 {
    let v = value & mask(bits);
    match kind {
        0 => v.wrapping_shl(amount as u32) & mask(bits),
        1 => v >> amount,
        2 => {
            if bits == 32 {
                ((v as u32 as i32) >> amount) as u32 as u64
            } else {
                ((v as i64) >> amount) as u64
            }
        }
        _ => {
            if bits == 32 {
                (v as u32).rotate_right(amount as u32) as u64
            } else {
                v.rotate_right(amount as u32)
            }
        }
    }
}
/// Arm ARM `AddWithCarry`: returns the width-masked result and NZCV.
#[inline]
pub fn add_with_carry(x: u64, y: u64, carry: bool, bits: u8) -> (u64, u8) {
    let m = mask(bits);
    let (x, y) = (x & m, y & m);
    let unsigned_sum = u128::from(x) + u128::from(y) + u128::from(carry);
    let signed_sum =
        i128::from(sign_extend(x, bits)) + i128::from(sign_extend(y, bits)) + i128::from(carry);
    let result = unsigned_sum as u64 & m;
    let n = (result >> (bits - 1)) & 1 != 0;
    let c = u128::from(result) != unsigned_sum;
    let v = i128::from(sign_extend(result, bits)) != signed_sum;
    (
        result,
        (n as u8) << 3 | ((result == 0) as u8) << 2 | (c as u8) << 1 | v as u8,
    )
}
#[inline]
pub fn sign_extend(value: u64, bits: u8) -> i64 {
    if bits >= 64 {
        value as i64
    } else {
        ((value << (64 - bits)) as i64) >> (64 - bits)
    }
}
/// Arm ARM `ExtendReg`: option 0..7 = UXTB, UXTH, UXTW, UXTX, SXTB, SXTH, SXTW, SXTX.
#[inline]
pub fn extend(value: u64, option: u8, shift: u8) -> u64 {
    let bits = 8u8 << (option & 3);
    let v = if option & 4 != 0 {
        sign_extend(value, bits) as u64
    } else if bits == 64 {
        value
    } else {
        value & ((1u64 << bits) - 1)
    };
    v << shift
}
/// RBIT (0), REV16 (1), REV32 (2), REV (3), CLZ (4), CLS (5) at the given width.
#[inline]
pub fn unary(value: u64, kind: u8, bits: u8) -> u64 {
    let v = value & mask(bits);
    let narrow = bits == 32;
    match kind {
        0 if narrow => u64::from((v as u32).reverse_bits()),
        0 => v.reverse_bits(),
        1 => {
            let m = 0x00ff_00ff_00ff_00ff & mask(bits);
            ((v & m) << 8) | ((v >> 8) & m)
        }
        2 => u64::from((v as u32).swap_bytes()) | u64::from(((v >> 32) as u32).swap_bytes()) << 32,
        3 if narrow => u64::from((v as u32).swap_bytes()),
        3 => v.swap_bytes(),
        4 if narrow => u64::from((v as u32).leading_zeros()),
        4 => u64::from(v.leading_zeros()),
        // CountLeadingSignBits(x) = CountLeadingZeroBits(x<N-1:1> EOR x<N-2:0>).
        _ => {
            let y = ((v >> 1) ^ v) & (mask(bits) >> 1);
            u64::from(y.leading_zeros()) - (64 - u64::from(bits)) - 1
        }
    }
}
/// SBFM (0), BFM (1), UBFM (2) with DecodeBitMasks results `wmask`/`tmask`.
#[allow(clippy::too_many_arguments)]
#[inline]
pub fn bitfield(
    dst: u64,
    src: u64,
    kind: u8,
    rotate: u8,
    sign_bit: u8,
    wmask: u64,
    tmask: u64,
    bits: u8,
) -> u64 {
    let m = mask(bits);
    let rotated = shift(src, 3, rotate, bits);
    let result = match kind {
        0 => {
            let top = if (src >> sign_bit) & 1 != 0 { m } else { 0 };
            (top & !tmask) | (rotated & wmask & tmask)
        }
        1 => {
            let bottom = (dst & !wmask) | (rotated & wmask);
            (dst & !tmask) | (bottom & tmask)
        }
        _ => rotated & wmask & tmask,
    };
    result & m
}
/// EXTR: bits [lsb, lsb + width) of high:low.
#[inline]
pub fn extract(high: u64, low: u64, lsb: u8, bits: u8) -> u64 {
    let m = mask(bits);
    let (high, low) = (u128::from(high & m), u128::from(low & m));
    (((high << bits) | low) >> lsb) as u64 & m
}
/// UDIV/SDIV: division by zero yields zero; the signed overflow case wraps.
#[inline]
pub fn divide(a: u64, b: u64, signed: bool, bits: u8) -> u64 {
    let (a, b) = (a & mask(bits), b & mask(bits));
    if b == 0 {
        return 0;
    }
    let result = if !signed {
        a / b
    } else {
        sign_extend(a, bits).wrapping_div(sign_extend(b, bits)) as u64
    };
    result & mask(bits)
}
/// Exclusive and ordered accesses require natural alignment of the whole access.
#[inline]
pub fn check_alignment(address: u64, size: u64) -> RuntimeResult<()> {
    if address % size != 0 {
        return Err(format!(
            "guest alignment fault: {size}-byte exclusive/ordered access at {address:#x}"
        ));
    }
    Ok(())
}
/// SVC entry from translated code. In a Horizon process the kernel dispatches the call; in
/// harness mode only ExitProcess (0x07) is defined. Returns the stop status, if any.
pub fn service(cpu: &mut Cpu, process: &mut Process, id: u16) -> RuntimeResult<Option<Stop>> {
    if process.kernel.is_some() {
        return horizon::dispatch(cpu, process, id);
    }
    if id == 7 {
        return Ok(Some(Stop::ExitProcess));
    }
    Err(format!(
        "unsupported Horizon SVC {id:#x}; guest_pc={:#x} x0={:#x} x1={:#x} x2={:#x} x3={:#x}",
        cpu.pc, cpu.x[0], cpu.x[1], cpu.x[2], cpu.x[3]
    ))
}
fn number(value: &str) -> RuntimeResult<u64> {
    u64::try_from(wide(value)?).map_err(|_| format!("{value:?} exceeds 64 bits"))
}
fn wide(value: &str) -> RuntimeResult<u128> {
    if let Some(v) = value.strip_prefix("0x") {
        u128::from_str_radix(v, 16)
    } else {
        value.parse()
    }
    .map_err(|e| format!("invalid integer {value:?}: {e}"))
}
/// Finds the block for `pc`: units are sorted by start address and disjoint, so the unit is
/// the last one starting at or below `pc`, followed by a search of its sorted block table.
#[inline]
pub fn find_block(units: &[(u64, BlockTable)], pc: u64) -> Option<BlockFn> {
    let unit = units.partition_point(|u| u.0 <= pc).checked_sub(1)?;
    let blocks = units[unit].1;
    blocks
        .binary_search_by_key(&pc, |b| b.0)
        .ok()
        .map(|i| blocks[i].1)
}
/// Dispatch loop: every guest control transfer returns here and is resolved through the
/// translated block tables. Unknown targets fault; nothing is interpreted.
pub fn execute(c: &mut Cpu, m: &mut Process, units: &[(u64, BlockTable)]) -> RuntimeResult<Stop> {
    execute_until(c, m, units, u64::MAX)
}
/// `execute` that also returns `Stop::Reschedule` at the first block boundary at which the
/// thread has retired at least `slice_end` instructions (its `Cpu::steps`). Block boundaries
/// are the only preemption points: every architectural state is in `Cpu` there, so the
/// thread resumes exactly at `Cpu::pc`.
pub fn execute_until(
    c: &mut Cpu,
    m: &mut Process,
    units: &[(u64, BlockTable)],
    slice_end: u64,
) -> RuntimeResult<Stop> {
    loop {
        if c.steps >= slice_end {
            return Ok(Stop::Reschedule);
        }
        if c.pc == RETURN_PC {
            return Ok(Stop::Returned);
        }
        m.check_execute(c.pc)?;
        let Some(block) = find_block(units, c.pc) else {
            return Err(format!(
                "unresolved native branch target {:#x}; add an analysis root with --seed",
                c.pc
            ));
        };
        match block(c, m) {
            Ok(Some(status)) => return Ok(status),
            Ok(None) => {}
            Err(e) => {
                // Faulting instructions record their PC; recount the instructions retired up
                // to and including it (a fault after the block's last instruction, at an
                // unsupported site, does not count that site).
                let (start, count) = c.block;
                let index = c.pc.wrapping_sub(start) / 4;
                c.steps = c.steps - count + index.min(count) + u64::from(index < count);
                return Err(e);
            }
        }
    }
}
/// Static description of a translated program, emitted by the code generator.
pub struct Program {
    pub entry: u64,
    pub units: &'static [(u64, BlockTable)],
    /// (start, end, name) of each loaded module, for fault context.
    pub modules: &'static [(u64, u64, &'static str)],
}
/// Coarse fault category derived from the runtime's error vocabulary.
pub fn fault_category(message: &str) -> &'static str {
    const CATEGORIES: [(&str, &str); 12] = [
        ("unsupported instruction", "unsupported-instruction"),
        ("unresolved native branch", "unresolved-control-flow"),
        ("unsupported Horizon SVC", "unsupported-service"),
        ("instruction budget", "budget"),
        ("alignment fault", "alignment"),
        ("permission fault", "memory-permission"),
        ("unmapped guest memory", "memory-unmapped"),
        ("guest BRK", "trap"),
        ("unsupported FPCR", "unsupported-fp-configuration"),
        ("guest svcBreak", "guest-break"),
        (
            "not permitted by the process capabilities",
            "svc-not-permitted",
        ),
        ("guest deadlock", "deadlock"),
    ];
    CATEGORIES
        .iter()
        .find(|(prefix, _)| message.contains(prefix))
        .map_or("fault", |c| c.1)
}
/// Shared launch protocol for independent reference testing; dumps scratch even after a guest fault.
pub fn launch(program: &Program, memory: Memory) -> i32 {
    let mut cpu = Cpu::new(program.entry);
    let mut memory = Process {
        memory,
        kernel: None,
    };
    let args: Vec<String> = std::env::args().skip(1).collect();
    let mut dump = None;
    let configure = (|| -> RuntimeResult<()> {
        let mut i = 0;
        while i < args.len() {
            if args[i] == "--help" {
                println!(
                    "AOT guest executable: [--reg N=VALUE] [--vreg N=VALUE] [--sp VALUE] [--nzcv 0..15] [--fpcr VALUE] [--fpsr VALUE] [--steps COUNT] [--memory SCRATCH_FILE] [--dump SCRATCH_FILE]"
                );
                return Ok(());
            }
            let value = args
                .get(i + 1)
                .ok_or_else(|| format!("missing value for {}", args[i]))?;
            match args[i].as_str() {
                "--reg" => {
                    let (n, v) = value.split_once('=').ok_or("register must be N=VALUE")?;
                    let n = number(n)?;
                    if n > 30 {
                        return Err("register index must be 0..30".into());
                    }
                    cpu.x[n as usize] = number(v)?;
                }
                "--vreg" => {
                    let (n, v) = value.split_once('=').ok_or("register must be N=VALUE")?;
                    let n = number(n)?;
                    if n > 31 {
                        return Err("vector register index must be 0..31".into());
                    }
                    cpu.v[n as usize] = wide(v)?;
                }
                "--fpcr" => cpu.fpcr = number(value)? & FPCR_WRITE_MASK,
                "--fpsr" => cpu.fpsr = number(value)? & FPSR_WRITE_MASK,
                "--sp" => cpu.sp = number(value)?,
                "--nzcv" => {
                    let flags = number(value)?;
                    if flags > 15 {
                        return Err("NZCV must be 0..15".into());
                    }
                    cpu.nzcv = flags as u8;
                }
                "--steps" => cpu.limit = number(value)?,
                "--memory" => {
                    let f = std::fs::File::open(value).map_err(|e| e.to_string())?;
                    let mut data = Vec::new();
                    use std::io::Read;
                    f.take(SCRATCH_SIZE as u64 + 1)
                        .read_to_end(&mut data)
                        .map_err(|e| e.to_string())?;
                    if data.len() > SCRATCH_SIZE {
                        return Err("scratch input exceeds 64 KiB".into());
                    }
                    memory.write_bytes(SCRATCH_BASE, &data)?;
                }
                "--dump" => dump = Some(value.clone()),
                option => return Err(format!("unknown runtime option {option}")),
            }
            i += 2;
        }
        Ok(())
    })();
    if let Err(e) = configure {
        eprintln!("runtime configuration: {e}");
        return 2;
    }
    if args.iter().any(|a| a == "--help") {
        return 0;
    }
    let result = execute(&mut cpu, &mut memory, program.units);
    if let Some(path) = dump {
        let write = (|| -> RuntimeResult<()> {
            let data = memory.read_bytes(SCRATCH_BASE, SCRATCH_SIZE)?;
            let mut file = std::fs::File::create(path).map_err(|e| e.to_string())?;
            file.write_all(data).map_err(|e| e.to_string())
        })();
        if let Err(e) = write {
            eprintln!("scratch dump: {e}");
            return 2;
        }
    }
    match result {
        Ok(status) => {
            println!("{}", cpu.report(status.as_str()));
            0
        }
        Err(e) => {
            println!("{}", cpu.report("fault"));
            let module = program
                .modules
                .iter()
                .find(|m| (m.0..m.1).contains(&cpu.pc))
                .map_or("(none)".to_string(), |m| {
                    format!("{}+{:#x}", m.2, cpu.pc - m.0)
                });
            let word = memory
                .read_bytes(cpu.pc, 4)
                .ok()
                .map_or("(unreadable)".to_string(), |b| {
                    format!("{:#010x}", u32::from_le_bytes([b[0], b[1], b[2], b[3]]))
                });
            eprintln!(
                "guest fault: {e}; category={} module={module} thread=0 guest_pc={:#x} word={word} steps={}",
                fault_category(&e),
                cpu.pc,
                cpu.steps
            );
            1
        }
    }
}

/// Description of a translated program launched as a Horizon process.
pub struct ProcessImage {
    /// Guest range covered by the loaded modules (the process code).
    pub code_start: u64,
    pub code_end: u64,
    pub config: horizon::ProcessConfig,
    /// Where each configuration value came from (for the report), e.g. "main.npdm".
    pub config_source: &'static str,
    /// Homebrew (NRO) entry convention: X0 = loader configuration, X1 = -1, X30 = return to
    /// the loader. Otherwise the NSO convention.
    pub homebrew: bool,
}
fn json_string(text: &str) -> String {
    let mut out = String::from("\"");
    for ch in text.chars() {
        match ch {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            c if (c as u32) < 0x20 => out.push_str(&format!("\\u{:04x}", c as u32)),
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

/// Starts the program as a Horizon process (`horizon::Process::horizon`) and runs its
/// threads under the deterministic scheduler (`threads::run`) until the process exits,
/// every thread exits, or execution faults. Prints one JSON report on stdout.
/// Options: `--steps N` (total guest instructions), `--quantum N` (instructions per slice).
pub fn launch_horizon(program: &Program, memory: Memory, image: &ProcessImage) -> i32 {
    launch_horizon_with_nvidia(program, memory, image, None)
}

/// Composes an explicitly installed process-local NVIDIA backend before guest execution.
/// The backend must implement the scoped memory and completion contract in `nvidia`.
pub fn launch_horizon_with_nvidia(
    program: &Program,
    memory: Memory,
    image: &ProcessImage,
    host: Option<Box<dyn nvidia::Host>>,
) -> i32 {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let mut limit = 10_000_000_000u64;
    let mut quantum = threads::DEFAULT_QUANTUM;
    let mut romfs = None;
    let mut patch_romfs = None;
    let mut i = 0;
    while i < args.len() {
        let parsed = match (args[i].as_str(), args.get(i + 1)) {
            ("--romfs", Some(v)) => storage::RomFs::open(v).map(|image| romfs = Some(image)),
            ("--patch-romfs", Some(v)) => {
                storage::RomFs::open(v).map(|image| patch_romfs = Some(image))
            }
            ("--steps", Some(v)) => number(v).map(|n| limit = n),
            ("--quantum", Some(v)) => number(v).and_then(|n| {
                if n == 0 {
                    Err("--quantum must be positive".into())
                } else {
                    quantum = n;
                    Ok(())
                }
            }),
            ("--help", _) => {
                println!(
                    "Horizon process: [--steps COUNT] [--quantum INSTRUCTIONS] [--romfs IMAGE] [--patch-romfs IMAGE]"
                );
                return 0;
            }
            (option, _) => Err(format!("unknown option {option}")),
        };
        if let Err(e) = parsed {
            eprintln!("runtime configuration: {e}");
            return 2;
        }
        i += 2;
    }
    let code = horizon::Range {
        start: image.code_start,
        end: image.code_end,
    };
    let (mut process, cpu) = match Process::horizon(memory, program.entry, code, image.config) {
        Ok(p) => p,
        Err(e) => {
            eprintln!("process creation: {e}");
            return 2;
        }
    };
    process.kernel.as_mut().unwrap().host.romfs = romfs;
    process.kernel.as_mut().unwrap().host.patch_romfs = patch_romfs;
    if let Some(host) = host {
        if let Err(error) = process.kernel.as_mut().unwrap().nvidia.install(host) {
            eprintln!("NVIDIA backend installation: {error:?}");
            return 3;
        }
    }
    let mut cpu = cpu;
    if image.homebrew {
        if let Err(e) = process.homebrew_entry(&mut cpu) {
            eprintln!("process creation: {e}");
            return 2;
        }
    }
    if let Some(k) = process.kernel.as_mut() {
        k.threads[0].context = Some(Box::new(cpu));
    }
    let started = std::time::Instant::now();
    let mut presentation = if std::env::var_os("WL_NO_SDL").is_some()
        || std::env::var("WL_GPU_DEBUG_WINDOW").as_deref() == Ok("1")
    {
        None
    } else {
        sdl::SdlHost::open("Wanderlust — guest display", 1280, 720)
    };
    let run = threads::run_with_host(
        &mut process,
        program.units,
        limit,
        quantum,
        presentation.as_mut(),
    );
    let host_seconds = started.elapsed().as_secs_f64();
    let k = process.kernel.as_ref().expect("Horizon process");
    let current = k.threads[run.thread].context.as_deref();
    let (pc, x0, steps) = current.map_or((0, 0, 0), |c| (c.pc, c.x[0], c.steps));
    let main = &k.threads[0];
    let svcs = k
        .svc_counts
        .iter()
        .map(|(id, n)| format!("\"{id:#04x}\":{n}"))
        .collect::<Vec<_>>()
        .join(",");
    let last_svc = k.last_svc.map_or("null".to_string(), |(pc, id, r)| {
        format!(
            "{{\"pc\":{pc},\"id\":{id},\"result\":{}}}",
            r.map_or("null".to_string(), |r| r.to_string())
        )
    });
    let output = k
        .debug_output
        .iter()
        .map(|s| json_string(s))
        .collect::<Vec<_>>()
        .join(",");
    let thread_list = k
        .threads
        .iter()
        .map(|t| {
            let (tpc, tsteps) = t.context.as_deref().map_or((0, 0), |c| (c.pc, c.steps));
            format!(
                "{{\"id\":{},\"handle\":{},\"tls\":{},\"state\":\"{:?}\",\"priority\":{},\"core\":{},\"pc\":{tpc},\"steps\":{tsteps},\"destroyed\":{}}}",
                t.id, t.handle, t.tls, t.state, t.priority, t.core, t.destroyed
            )
        })
        .collect::<Vec<_>>()
        .join(",");
    let st = &k.stats;
    let scheduler = format!(
        "{{\"quantum\":{quantum},\"slices\":{},\"context_switches\":{},\"blocks\":{},\"wakeups\":{},\"timeouts\":{},\"yields\":{},\"rotations\":{},\"max_runnable\":{},\"threads_created\":{},\"virtual_ns\":{},\"idle_ns\":{},\"host_seconds\":{host_seconds:.6}}}",
        st.slices,
        st.context_switches,
        st.blocks,
        st.wakeups,
        st.timeouts,
        st.yields,
        st.rotations,
        st.max_runnable,
        st.threads_created,
        k.time_ns,
        st.idle_ns
    );
    let ipc = format!(
        "{{\"requests\":{},\"replies\":{},\"sessions\":{},\"sessions_closed_by_server\":{},\"deferred\":{},\"services\":[{}],\"hosted_calls\":[{}]}}",
        k.ipc.requests,
        k.ipc.replies,
        k.sessions.len(),
        k.ipc.sessions_closed_by_server,
        k.ipc.deferred,
        k.services
            .iter()
            .map(|s| json_string(String::from_utf8_lossy(&s.name).trim_end_matches('\0')))
            .collect::<Vec<_>>()
            .join(","),
        k.host
            .calls
            .iter()
            .map(|(name, command, code)| {
                format!(
                    "{{\"interface\":{},\"command\":{command},\"result\":{code}}}",
                    json_string(name)
                )
            })
            .collect::<Vec<_>>()
            .join(",")
    );
    let (status, fault) = match &run.result {
        Ok(status) => (status.as_str().to_string(), "null".to_string()),
        Err(e) => (
            "fault".to_string(),
            format!(
                "{{\"category\":\"{}\",\"thread\":{},\"message\":{}}}",
                fault_category(e),
                k.threads[run.thread].id,
                json_string(e)
            ),
        ),
    };
    println!(
        "{{\"mode\":\"horizon\",\"status\":\"{status}\",\"entry\":{},\"pc\":{pc},\"steps\":{},\"x0\":{x0},\"thread\":{{\"id\":{},\"handle\":{},\"tls\":{},\"stack\":[{},{}]}},\"threads\":[{thread_list}],\"scheduler\":{scheduler},\"ipc\":{ipc},\"heap\":[{},{}],\"process_local_region\":{},\"config_source\":\"{}\",\"svc_counts\":{{{svcs}}},\"last_svc\":{last_svc},\"debug_output\":[{output}],\"fault\":{fault}}}",
        program.entry,
        k.total_steps,
        main.id,
        main.handle,
        main.tls,
        main.stack.start,
        main.stack.end,
        k.layout.heap.start,
        k.heap_size,
        k.process_local_region,
        image.config_source,
    );
    if let Err(e) = run.result {
        eprintln!(
            "guest fault: {e}; category={} thread={:#x} guest_pc={pc:#x} steps={steps}",
            fault_category(&e),
            k.threads[run.thread].id,
        );
        eprintln!("Regions at fault around 0x208e911008:");
        for (i, r) in process.memory.regions.iter().enumerate() {
            if r.base <= 0x208e911008u64 + 0x20000
                && r.end() >= 0x208e911008u64.saturating_sub(0x20000)
            {
                eprintln!(
                    "  region[{i}]: {:#x}..{:#x} read={} write={} exec={} state={:#x} attr={:#x}",
                    r.base,
                    r.end(),
                    r.read,
                    r.write,
                    r.execute,
                    r.state,
                    r.attribute
                );
            }
        }
        eprintln!("Kernel aliases at fault:");
        for (dst, src, size) in &k.aliases {
            eprintln!("  alias: dst={dst:#x} src={src:#x} size={size:#x}");
        }
        eprintln!("Allocators state at fault:");
        for &addr in &[
            0xb963a80,
            0x3accc40,
            0xbaccc10,
            0xbaccc18,
            0xbaccc40,
            0xba2be38,
            0xba2be40,
            0xba2be48,
            0xbad0a80,
            0xbad0da0,
            0xbad2b70,
            0xbad2b78,
            0xbad2c20,
            0xbad2c28,
            0xbad2c40,
            0xbad2da0,
            0xbad2de8,
            0x208d900320,
            0x208e011320,
            0x208e011420,
            0x208e0114d0,
            0x208e0114d8,
            0x208e0114e0,
            0x208e011520,
            0x208e011548,
            0x208e060000,
            0x208e060018,
            0x208e010ca8,
            0x208e010d98,
            0xb906d18,
            0xb906d98,
        ] {
            eprintln!(
                "*({addr:#x}) = {:#x}",
                process.memory.load(addr, 8).unwrap_or(0xdeadbeef)
            );
        }
        eprintln!("Other heap objects stored in 0x208d900320:");
        for &addr in &[
            0x208e9053b8,
            0x208e905b88,
            0x208e907e38,
            0x208e908468,
            0x208e913368,
            0x208d901548,
        ] {
            eprintln!(
                "  *({addr:#x}) = {:#x}, *(+0x90) = {:#x}",
                process.memory.load(addr, 8).unwrap_or(0xdeadbeef),
                process.memory.load(addr + 0x90, 8).unwrap_or(0xdeadbeef)
            );
        }
        if let Some(c) = k.threads[run.thread].context.as_deref() {
            eprintln!("Registers at fault:");
            for (i, val) in c.x.iter().enumerate() {
                eprint!("x{i}={val:#x} ");
                if i % 4 == 3 {
                    eprintln!();
                }
            }
            eprintln!("\nsp={:#x} pc={:#x}", c.sp, c.pc);
            for offset in (0..64).step_by(8) {
                if let Ok(val) = process.memory.load(c.sp.wrapping_add(offset), 8) {
                    eprintln!("*(sp+{offset:#x}) = {val:#x}");
                }
            }
        }
        return 1;
    }
    0
}
