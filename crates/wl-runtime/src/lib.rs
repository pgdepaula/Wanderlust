//! Dependency-free guest runtime linked into AOT executables as an rlib.
//! No guest address is converted into a host pointer; no unsafe code is used.
pub mod fp;
pub mod simd;
use std::io::Write;

pub const SCRATCH_BASE: u64 = 0x1000_0000;
pub const SCRATCH_SIZE: usize = 0x10000;
pub const RETURN_PC: u64 = 0xffff_ffff_ffff_fffc;
pub type RuntimeResult<T> = std::result::Result<T, String>;
/// A translated guest basic block: runs to its end, then returns `Some(status)` to stop.
pub type BlockFn = fn(&mut Cpu, &mut Memory) -> RuntimeResult<Option<&'static str>>;
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
}
#[derive(Debug, Default)]
pub struct Memory {
    pub regions: Vec<Region>,
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
        self.regions.push(Region {
            base,
            bytes,
            read,
            write,
            execute,
        });
        Ok(())
    }
    #[inline]
    fn locate(&self, address: u64, size: usize, write: bool) -> RuntimeResult<(usize, usize)> {
        let end = address
            .checked_add(size as u64)
            .ok_or_else(|| format!("guest memory address overflow at {address:#x}"))?;
        for (i, r) in self.regions.iter().enumerate() {
            if address >= r.base && end <= r.base + r.bytes.len() as u64 {
                if (write && !r.write) || (!write && !r.read) {
                    return Err(format!(
                        "guest memory permission fault: {} {size} bytes at {address:#x}",
                        if write { "write" } else { "read" }
                    ));
                }
                return Ok((i, (address - r.base) as usize));
            }
        }
        Err(format!(
            "unmapped guest memory: {} {size} bytes at {address:#x}",
            if write { "write" } else { "read" }
        ))
    }
    #[inline]
    pub fn read_bytes(&self, address: u64, size: usize) -> RuntimeResult<&[u8]> {
        let (i, at) = self.locate(address, size, false)?;
        Ok(&self.regions[i].bytes[at..at + size])
    }
    #[inline]
    pub fn write_bytes(&mut self, address: u64, bytes: &[u8]) -> RuntimeResult<()> {
        let (i, at) = self.locate(address, bytes.len(), true)?;
        self.regions[i].bytes[at..at + bytes.len()].copy_from_slice(bytes);
        Ok(())
    }
    #[inline]
    pub fn load(&self, address: u64, bytes: u8) -> RuntimeResult<u64> {
        if ![1, 2, 4, 8].contains(&bytes) {
            return Err("invalid guest access width".into());
        }
        let mut value = [0u8; 8];
        value[..bytes as usize].copy_from_slice(self.read_bytes(address, bytes as usize)?);
        Ok(u64::from_le_bytes(value))
    }
    #[inline]
    pub fn load128(&self, address: u64) -> RuntimeResult<u128> {
        let b = self.read_bytes(address, 16)?;
        Ok(u128::from_le_bytes(
            b.try_into().map_err(|_| "128-bit load")?,
        ))
    }
    /// Little-endian load of 1..=16 bytes, zero-extended.
    #[inline]
    pub fn load_vector(&self, address: u64, bytes: u8) -> RuntimeResult<u128> {
        let mut value = [0u8; 16];
        value[..bytes as usize].copy_from_slice(self.read_bytes(address, bytes as usize)?);
        Ok(u128::from_le_bytes(value))
    }
    /// Little-endian store of the low 1..=16 bytes of `value`.
    #[inline]
    pub fn store_vector(&mut self, address: u64, bytes: u8, value: u128) -> RuntimeResult<()> {
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
        if self.regions.iter().any(|r| {
            r.execute
                && pc >= r.base
                && pc
                    .checked_add(4)
                    .is_some_and(|end| end <= r.base + r.bytes.len() as u64)
        }) {
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
/// Only ExitProcess is implemented. Other Horizon calls fail without mutating guest state.
pub fn service(cpu: &mut Cpu, id: u16) -> RuntimeResult<bool> {
    if id == 7 {
        return Ok(true);
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
pub fn execute(
    c: &mut Cpu,
    m: &mut Memory,
    units: &[(u64, BlockTable)],
) -> RuntimeResult<&'static str> {
    loop {
        if c.pc == RETURN_PC {
            return Ok("returned");
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
    const CATEGORIES: [(&str, &str); 9] = [
        ("unsupported instruction", "unsupported-instruction"),
        ("unresolved native branch", "unresolved-control-flow"),
        ("unsupported Horizon SVC", "unsupported-service"),
        ("instruction budget", "budget"),
        ("alignment fault", "alignment"),
        ("permission fault", "memory-permission"),
        ("unmapped guest memory", "memory-unmapped"),
        ("guest BRK", "trap"),
        ("unsupported FPCR", "unsupported-fp-configuration"),
    ];
    CATEGORIES
        .iter()
        .find(|(prefix, _)| message.contains(prefix))
        .map_or("fault", |c| c.1)
}
/// Shared launch protocol for independent reference testing; dumps scratch even after a guest fault.
pub fn launch(program: &Program, mut memory: Memory) -> i32 {
    let mut cpu = Cpu::new(program.entry);
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
            println!("{}", cpu.report(status));
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
