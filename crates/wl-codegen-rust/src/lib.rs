//! Instruction-specialized Rust source, compiled ahead of execution with rustc.
pub mod build;
use std::fmt::Write as _;
use wl_aarch64::{
    CounterRegister, CryptoOp, ExtensionOp, FpBinary, FpOp, FpUnary, Op, Operand, PostIndex, Reg,
    SimdOp, SystemRegister, Width,
};
use wl_analysis::{Image, Program, RelocationState};
use wl_common::Result;

fn read(reg: Reg) -> String {
    match reg {
        Reg::Gpr(n) => format!("c.x[{n}]"),
        Reg::Sp => "c.sp".into(),
        Reg::Zero => "0u64".into(),
    }
}
fn write(reg: Reg, value: &str, width: Width) -> String {
    let value = if width == Width::W32 {
        format!("({value}) & 0xffff_ffffu64")
    } else {
        value.into()
    };
    match reg {
        Reg::Gpr(n) => format!("c.x[{n}] = {value};\n"),
        Reg::Sp => format!("c.sp = {value};\n"),
        Reg::Zero => String::new(),
    }
}
fn operand(op: Operand, width: Width) -> String {
    match op {
        Operand::Imm(v) => format!("{v:#x}u64"),
        // LSL #0 is the register itself; callers mask to the operation width.
        Operand::Shifted {
            reg,
            kind: 0,
            amount: 0,
        } => read(reg),
        Operand::Shifted { reg, kind, amount } => {
            format!("shift({}, {kind}, {amount}, {})", read(reg), width.bits())
        }
        Operand::Extended { reg, extend, shift } => {
            format!("extend({}, {extend}, {shift})", read(reg))
        }
    }
}
fn stack_check(base: Reg) -> &'static str {
    if base == Reg::Sp {
        "if c.sp & 15 != 0 { return Err(\"guest SP alignment fault\".into()); }\n"
    } else {
        ""
    }
}
fn fmt_name(bits: u8) -> &'static str {
    match bits {
        16 => "fp::H",
        32 => "fp::S",
        _ => "fp::D",
    }
}
/// The low `bits` of V register `n` as a u64.
fn vscalar(n: u8, bits: u8) -> String {
    if bits == 64 {
        format!("(c.v[{n}] as u64)")
    } else {
        format!("(c.v[{n}] as u64 & {:#x})", (1u64 << bits) - 1)
    }
}
/// Writes a vector result, zeroing destination bits above `total`.
fn vwrite(d: u8, value: &str, total: u32) -> String {
    if total >= 128 {
        format!("c.v[{d}] = {value};\n")
    } else {
        format!("c.v[{d}] = ({value}) & {:#x};\n", (1u128 << total) - 1)
    }
}
fn address_of(base: Reg, offset: i64, mode: u8, index: Option<wl_aarch64::Index>) -> String {
    let index = index.map_or(String::new(), |i| {
        format!(
            ".wrapping_add(extend({}, {}, {}))",
            read(i.reg),
            i.extend,
            i.shift
        )
    });
    format!(
        "{}let base = {}; let adjusted = base.wrapping_add_signed({offset}i64){index}; let address = {};\n",
        stack_check(base),
        read(base),
        if mode == 1 { "base" } else { "adjusted" }
    )
}
fn lower_fp(op: FpOp) -> String {
    let mut out = String::new();
    match op {
        FpOp::Binary { bits, op, d, n, m } => {
            let f = fmt_name(bits);
            let call = match op {
                FpBinary::Add => format!("fp::add(c, {f}, a, b, false)"),
                FpBinary::Sub => format!("fp::add(c, {f}, a, b, true)"),
                FpBinary::Mul => format!("fp::mul(c, {f}, a, b, false)"),
                FpBinary::NegMul => format!("fp::mul(c, {f}, a, b, true)"),
                FpBinary::Div => format!("fp::div(c, {f}, a, b)"),
                FpBinary::Max => format!("fp::min_max(c, {f}, a, b, true, false)"),
                FpBinary::Min => format!("fp::min_max(c, {f}, a, b, false, false)"),
                FpBinary::MaxNum => format!("fp::min_max(c, {f}, a, b, true, true)"),
                FpBinary::MinNum => format!("fp::min_max(c, {f}, a, b, false, true)"),
            };
            writeln!(
                out,
                "let a = {}; let b = {}; let r = {call}; c.v[{d}] = u128::from(r);",
                vscalar(n, bits),
                vscalar(m, bits)
            )
            .unwrap();
        }
        FpOp::Unary { bits, op, d, n } => {
            let f = fmt_name(bits);
            let call = match op {
                FpUnary::Mov => "a".to_string(),
                FpUnary::Abs => format!("fp::abs({f}, a)"),
                FpUnary::Neg => format!("fp::neg({f}, a)"),
                FpUnary::Sqrt => format!("fp::sqrt(c, {f}, a)"),
                FpUnary::Round(mode) => format!("fp::round_int(c, {f}, a, {mode}, false)"),
                FpUnary::RoundExact => format!("fp::round_int(c, {f}, a, fp::ROUND_FPCR, true)"),
            };
            writeln!(
                out,
                "let a = {}; let r = {call}; c.v[{d}] = u128::from(r);",
                vscalar(n, bits)
            )
            .unwrap();
        }
        FpOp::Convert { from, to, d, n } => {
            writeln!(
                out,
                "let a = {}; let r = fp::convert(c, {}, {}, a)?; c.v[{d}] = u128::from(r);",
                vscalar(n, from),
                fmt_name(from),
                fmt_name(to)
            )
            .unwrap();
        }
        FpOp::MulAdd {
            bits,
            negate_addend,
            negate_product,
            d,
            n,
            m,
            a,
        } => {
            let f = fmt_name(bits);
            let neg = |x: String, yes: bool| if yes { format!("fp::neg({f}, {x})") } else { x };
            writeln!(
                out,
                "let addend = {}; let op1 = {}; let op2 = {}; let r = fp::mul_add(c, {f}, addend, op1, op2); c.v[{d}] = u128::from(r);",
                neg(vscalar(a, bits), negate_addend),
                neg(vscalar(n, bits), negate_product),
                vscalar(m, bits)
            )
            .unwrap();
        }
        FpOp::Compare { bits, n, m, signal } => {
            let b = m.map_or("0u64".to_string(), |m| vscalar(m, bits));
            writeln!(
                out,
                "let a = {}; let b = {b}; c.nzcv = fp::compare(c, {}, a, b, {signal});",
                vscalar(n, bits),
                fmt_name(bits)
            )
            .unwrap();
        }
        FpOp::CondCompare {
            bits,
            n,
            m,
            condition,
            nzcv,
            signal,
        } => {
            writeln!(
                out,
                "if c.condition({condition}) {{ let a = {}; let b = {}; c.nzcv = fp::compare(c, {}, a, b, {signal}); }} else {{ c.nzcv = {nzcv}; }}",
                vscalar(n, bits),
                vscalar(m, bits),
                fmt_name(bits)
            )
            .unwrap();
        }
        FpOp::Select {
            bits,
            d,
            n,
            m,
            condition,
        } => {
            writeln!(
                out,
                "c.v[{d}] = u128::from(if c.condition({condition}) {{ {} }} else {{ {} }});",
                vscalar(n, bits),
                vscalar(m, bits)
            )
            .unwrap();
        }
        FpOp::Immediate { bits, d, imm8 } => {
            let format = match bits {
                16 => wl_runtime::fp::H,
                32 => wl_runtime::fp::S,
                _ => wl_runtime::fp::D,
            };
            writeln!(
                out,
                "c.v[{d}] = {:#x};",
                wl_runtime::fp::expand_imm(format, imm8)
            )
            .unwrap();
        }
        FpOp::ToInt {
            bits,
            dst,
            n,
            width,
            signed,
            rounding,
            fbits,
        } => {
            let value = format!(
                "{{ let a = {}; fp::to_fixed(c, {}, a, {}, {signed}, {rounding}, {fbits}) }}",
                vscalar(n, bits),
                fmt_name(bits),
                width.bits()
            );
            out.push_str(&write(dst, &value, width));
        }
        FpOp::FromInt {
            bits,
            d,
            src,
            width,
            signed,
            fbits,
        } => {
            writeln!(out, "let x = {}; let r = fp::from_int(c, {}, x, {}, {signed}, {fbits}); c.v[{d}] = u128::from(r);", read(src), fmt_name(bits), width.bits()).unwrap();
        }
        FpOp::MoveToGeneral { dst, n, width, top } => {
            let value = if top {
                format!("(c.v[{n}] >> 64) as u64")
            } else {
                format!("c.v[{n}] as u64")
            };
            out.push_str(&write(dst, &value, width));
        }
        FpOp::MoveFromGeneral { d, src, width, top } => {
            if top {
                writeln!(
                    out,
                    "c.v[{d}] = (c.v[{d}] & u128::from(u64::MAX)) | (u128::from({}) << 64);",
                    read(src)
                )
                .unwrap();
            } else {
                let value = if width == Width::W32 {
                    format!("{} & 0xffff_ffff", read(src))
                } else {
                    read(src)
                };
                writeln!(out, "c.v[{d}] = u128::from({value});").unwrap();
            }
        }
    }
    out
}
fn lower_simd(op: SimdOp) -> String {
    let mut out = String::new();
    let operands =
        |n: u8, m: u8, d: u8| format!("let a = c.v[{n}]; let b = c.v[{m}]; let d0 = c.v[{d}];\n");
    match op {
        SimdOp::Immediate {
            d,
            total,
            value,
            kind,
        } => {
            let v = if total == 128 {
                u128::from(value) | (u128::from(value) << 64)
            } else {
                u128::from(value)
            };
            let expr = match kind {
                0 => format!("{v:#x}u128"),
                1 => format!("c.v[{d}] | {v:#x}u128"),
                _ => format!("c.v[{d}] & !{v:#x}u128"),
            };
            out.push_str(&vwrite(d, &expr, u32::from(total)));
        }
        SimdOp::DupElement { shape, d, n, index } => {
            out.push_str(&vwrite(
                d,
                &format!(
                    "simd::dup(simd::lane(c.v[{n}], {}, {index}), {}, {})",
                    shape.esize, shape.esize, shape.lanes
                ),
                128,
            ));
        }
        SimdOp::DupGeneral { shape, d, src } => {
            out.push_str(&vwrite(
                d,
                &format!("simd::dup({}, {}, {})", read(src), shape.esize, shape.lanes),
                128,
            ));
        }
        SimdOp::InsertElement {
            esize,
            d,
            index,
            n,
            source_index,
        } => {
            out.push_str(&vwrite(d, &format!("simd::with_lane(c.v[{d}], {esize}, {index}, simd::lane(c.v[{n}], {esize}, {source_index}))"), 128));
        }
        SimdOp::InsertGeneral {
            esize,
            d,
            index,
            src,
        } => {
            out.push_str(&vwrite(
                d,
                &format!("simd::with_lane(c.v[{d}], {esize}, {index}, {})", read(src)),
                128,
            ));
        }
        SimdOp::MoveToGeneral {
            esize,
            dst,
            n,
            index,
            signed,
            width,
        } => {
            let lane = format!("simd::lane(c.v[{n}], {esize}, {index})");
            let value = if signed {
                format!("sign_extend({lane}, {esize}) as u64")
            } else {
                lane
            };
            out.push_str(&write(dst, &value, width));
        }
        SimdOp::Int { shape, op, d, n, m } => {
            out.push_str(&operands(n, m, d));
            out.push_str(&vwrite(
                d,
                &format!(
                    "simd::int2(c, {op}, {}, {}, a, b, d0)",
                    shape.esize, shape.lanes
                ),
                shape.total(),
            ));
        }
        SimdOp::IntPairwise { shape, op, d, n, m } => {
            out.push_str(&operands(n, m, d));
            out.push_str(&vwrite(
                d,
                &format!(
                    "simd::pairwise({op}, {}, {}, a, b)",
                    shape.esize, shape.lanes
                ),
                shape.total(),
            ));
        }
        SimdOp::Logical { total, op, d, n, m } => {
            out.push_str(&operands(n, m, d));
            out.push_str(&vwrite(
                d,
                &format!("simd::logical({op}, a, b, d0)"),
                u32::from(total),
            ));
        }
        SimdOp::Fp { shape, op, d, n, m } => {
            out.push_str(&operands(n, m, d));
            out.push_str(&vwrite(
                d,
                &format!(
                    "simd::fp2(c, {op}, {}, {}, a, b, d0)",
                    shape.esize, shape.lanes
                ),
                shape.total(),
            ));
        }
        SimdOp::FpPairwise { shape, op, d, n, m } => {
            out.push_str(&operands(n, m, d));
            out.push_str(&vwrite(
                d,
                &format!(
                    "simd::fp_pairwise(c, {op}, {}, {}, a, b)",
                    shape.esize, shape.lanes
                ),
                shape.total(),
            ));
        }
        SimdOp::IntUnary { shape, op, d, n } => {
            writeln!(out, "let a = c.v[{n}];").unwrap();
            out.push_str(&vwrite(
                d,
                &format!("simd::int1(c, {op}, {}, {}, a)", shape.esize, shape.lanes),
                shape.total(),
            ));
        }
        SimdOp::FpUnary {
            shape,
            op,
            d,
            n,
            mode,
            fbits,
        } => {
            writeln!(out, "let a = c.v[{n}];").unwrap();
            out.push_str(&vwrite(
                d,
                &format!(
                    "simd::fp1(c, {op}, {}, {}, a, {mode}, {fbits})",
                    shape.esize, shape.lanes
                ),
                shape.total(),
            ));
        }
        SimdOp::ShiftImm {
            shape,
            op,
            d,
            n,
            amount,
        } => {
            out.push_str(&vwrite(
                d,
                &format!(
                    "simd::shift_imm({op}, {}, {}, c.v[{n}], c.v[{d}], {amount})",
                    shape.esize, shape.lanes
                ),
                shape.total(),
            ));
        }
        SimdOp::Widen {
            esize,
            lanes,
            d,
            n,
            upper,
            signed,
            amount,
        } => {
            out.push_str(&vwrite(
                d,
                &format!(
                    "simd::widen_shift({esize}, {lanes}, c.v[{n}], {upper}, {signed}, {amount})"
                ),
                128,
            ));
        }
        SimdOp::Narrow {
            esize,
            lanes,
            op,
            d,
            n,
            upper,
            amount,
        } => {
            writeln!(out, "let a = c.v[{n}]; let d0 = c.v[{d}];").unwrap();
            let total = if upper {
                128
            } else {
                u32::from(esize) * u32::from(lanes)
            };
            out.push_str(&vwrite(
                d,
                &format!("simd::narrow(c, {op}, {esize}, {lanes}, a, d0, {upper}, {amount})"),
                total,
            ));
        }
        SimdOp::FpWiden {
            esize,
            lanes,
            d,
            n,
            upper,
        } => {
            writeln!(out, "let a = c.v[{n}];").unwrap();
            out.push_str(&vwrite(
                d,
                &format!("simd::fp_widen(c, {esize}, {lanes}, a, {upper})?"),
                128,
            ));
        }
        SimdOp::FpNarrow {
            esize,
            lanes,
            d,
            n,
            upper,
        } => {
            writeln!(out, "let a = c.v[{n}]; let d0 = c.v[{d}];").unwrap();
            let total = if upper {
                128
            } else {
                u32::from(esize) * u32::from(lanes)
            };
            out.push_str(&vwrite(
                d,
                &format!("simd::fp_narrow(c, {esize}, {lanes}, a, d0, {upper})?"),
                total,
            ));
        }
        SimdOp::IntElement {
            shape,
            op,
            d,
            n,
            m,
            index,
        } => {
            out.push_str(&operands(n, m, d));
            out.push_str(&vwrite(
                d,
                &format!(
                    "simd::int2_element(c, {op}, {}, {}, a, b, {index}, d0)",
                    shape.esize, shape.lanes
                ),
                shape.total(),
            ));
        }
        SimdOp::FpElement {
            shape,
            op,
            d,
            n,
            m,
            index,
        } => {
            out.push_str(&operands(n, m, d));
            out.push_str(&vwrite(
                d,
                &format!(
                    "simd::fp2_element(c, {op}, {}, {}, a, b, {index}, d0)",
                    shape.esize, shape.lanes
                ),
                shape.total(),
            ));
        }
        SimdOp::Reduce {
            esize,
            lanes,
            op,
            d,
            n,
        } => {
            writeln!(
                out,
                "c.v[{d}] = simd::reduce({op}, {esize}, {lanes}, c.v[{n}]);"
            )
            .unwrap();
        }
        SimdOp::FpReduce {
            esize,
            lanes,
            op,
            d,
            n,
        } => {
            writeln!(
                out,
                "let a = c.v[{n}]; c.v[{d}] = simd::fp_reduce(c, {op}, {esize}, {lanes}, a);"
            )
            .unwrap();
        }
        SimdOp::Permute { shape, op, d, n, m } => {
            out.push_str(&vwrite(
                d,
                &format!(
                    "simd::permute({op}, {}, {}, c.v[{n}], c.v[{m}])",
                    shape.esize, shape.lanes
                ),
                shape.total(),
            ));
        }
        SimdOp::Ext {
            total,
            d,
            n,
            m,
            index,
        } => {
            out.push_str(&vwrite(
                d,
                &format!("simd::ext(c.v[{n}], c.v[{m}], {index}, {})", total / 8),
                u32::from(total),
            ));
        }
        SimdOp::LongElement {
            esize,
            lanes,
            op,
            d,
            n,
            m,
            index,
            upper,
            signed,
        } => {
            out.push_str(&operands(n, m, d));
            out.push_str(&vwrite(
                d,
                &format!(
                    "simd::long3_element({op}, {esize}, {lanes}, a, b, {index}, d0, {upper}, {signed})"
                ),
                128,
            ));
        }
        SimdOp::AddLongPairwise {
            esize,
            lanes,
            d,
            n,
            signed,
            accumulate,
        } => {
            writeln!(out, "let a = c.v[{n}]; let d0 = c.v[{d}];").unwrap();
            out.push_str(&vwrite(
                d,
                &format!(
                    "simd::add_long_pairwise({esize}, {lanes}, a, d0, {signed}, {accumulate})"
                ),
                2 * u32::from(esize) * u32::from(lanes),
            ));
        }
        SimdOp::Long3 {
            esize,
            lanes,
            op,
            d,
            n,
            m,
            upper,
            signed,
        } => {
            out.push_str(&operands(n, m, d));
            // High-narrowing results occupy one half; widening results the whole register.
            let total = if op >= 9 && !upper { 64 } else { 128 };
            out.push_str(&vwrite(
                d,
                &format!("simd::long3({op}, {esize}, {lanes}, a, b, d0, {upper}, {signed})"),
                total,
            ));
        }
        SimdOp::Table {
            total,
            d,
            n,
            regs,
            m,
            keep,
        } => {
            let table: Vec<String> = (0..regs)
                .map(|i| format!("c.v[{}]", (n + i) % 32))
                .collect();
            out.push_str(&vwrite(
                d,
                &format!(
                    "simd::table(&[{}], c.v[{m}], c.v[{d}], {}, {keep})",
                    table.join(", "),
                    total / 8
                ),
                u32::from(total),
            ));
        }
    }
    out
}
/// System counter reads, CRC32/CRC32C and the recognised crypto-extension forms.
fn lower_extension(e: ExtensionOp) -> String {
    match e {
        ExtensionOp::Counter(c) => {
            let value = match c.register {
                CounterRegister::CntfrqEl0 => "COUNTER_FREQUENCY".to_string(),
                CounterRegister::CntpctEl0 | CounterRegister::CntvctEl0 => {
                    "system_counter(c, m)".to_string()
                }
            };
            write(c.reg, &value, Width::X64)
        }
        ExtensionOp::Crc(c) => {
            let value = format!(
                "crc32({}, {}, {}, {})",
                read(c.acc),
                read(c.val),
                1u8 << c.sz,
                c.c
            );
            write(c.dst, &value, Width::W32)
        }
        ExtensionOp::Crypto(op) => match op {
            CryptoOp::Aese { dst, src } => format!("c.v[{dst}] = aese(c.v[{dst}], c.v[{src}]);\n"),
            CryptoOp::Aesd { dst, src } => format!("c.v[{dst}] = aesd(c.v[{dst}], c.v[{src}]);\n"),
            CryptoOp::Aesimc { dst, src } => format!("c.v[{dst}] = aesimc(c.v[{src}]);\n"),
            CryptoOp::Sha1c { dst, src1, src2 } => {
                format!("c.v[{dst}] = sha1c(c.v[{dst}], c.v[{src1}], c.v[{src2}]);\n")
            }
            CryptoOp::Sha256su0 { dst, src } => {
                format!("c.v[{dst}] = sha256su0(c.v[{dst}], c.v[{src}]);\n")
            }
            CryptoOp::Pmull { dst, lhs, rhs } => {
                format!("c.v[{dst}] = pmull64(c.v[{lhs}] as u64, c.v[{rhs}] as u64);\n")
            }
        },
    }
}
fn lower(op: Op, next: u64) -> String {
    let mut out = String::new();
    match op {
        Op::Extension(e) => out.push_str(&lower_extension(e)),
        Op::Fp(fp) => out.push_str(&lower_fp(fp)),
        Op::Simd(simd) => out.push_str(&lower_simd(simd)),
        Op::VecMemory {
            load,
            bytes,
            vt,
            base,
            offset,
            mode,
            index,
        } => {
            out.push_str(&address_of(base, offset, mode, index));
            if load {
                writeln!(out, "c.v[{vt}] = m.load_vector(address, {bytes})?;").unwrap();
            } else {
                writeln!(out, "m.store_vector(address, {bytes}, c.v[{vt}])?;").unwrap();
            }
            if mode != 0 {
                out.push_str(&write(base, "adjusted", Width::X64));
            }
        }
        Op::VecPair {
            load,
            bytes,
            first,
            second,
            base,
            offset,
            mode,
        } => {
            out.push_str(&address_of(base, offset, mode, None));
            if load {
                writeln!(out, "let p = m.load_vector(address, {bytes})?; let q = m.load_vector(address.wrapping_add({bytes}), {bytes})?; c.v[{first}] = p; c.v[{second}] = q;").unwrap();
            } else {
                writeln!(out, "m.store_vector(address, {bytes}, c.v[{first}])?; m.store_vector(address.wrapping_add({bytes}), {bytes}, c.v[{second}])?;").unwrap();
            }
            if mode != 2 {
                out.push_str(&write(base, "adjusted", Width::X64));
            }
        }
        Op::VecLiteral { vt, address, bytes } => {
            writeln!(
                out,
                "c.v[{vt}] = m.load_vector({:#x}, {bytes})?;",
                address.0
            )
            .unwrap();
        }
        Op::VecStructures(s) => {
            out.push_str(stack_check(s.base));
            writeln!(
                out,
                "let address = {}; let moved = simd::structures(c, m, {}, {}, {}, {}, {}, {}, {:?}, {}, address)?;",
                read(s.base),
                s.load,
                s.selem,
                s.repeats,
                s.esize,
                s.total,
                s.t,
                s.lane.map(u32::from),
                s.replicate
            )
            .unwrap();
            match s.post {
                Some(PostIndex::Immediate(_)) => {
                    out.push_str(&write(s.base, "address.wrapping_add(moved)", Width::X64))
                }
                Some(PostIndex::Register(r)) => out.push_str(&write(
                    s.base,
                    &format!("address.wrapping_add({})", read(r)),
                    Width::X64,
                )),
                None => {}
            }
        }
        Op::Nop => {}
        Op::Wide {
            width,
            dst,
            imm,
            shift,
            keep,
            invert,
        } => {
            let value = if keep {
                format!("({} & !({}u64)) | {imm}u64", read(dst), 0xffffu64 << shift)
            } else if invert {
                format!("!{imm}u64")
            } else {
                format!("{imm}u64")
            };
            out.push_str(&write(dst, &value, width));
        }
        Op::Arithmetic {
            width,
            dst,
            lhs,
            rhs,
            sub,
            flags,
        } => {
            let bits = width.bits();
            let op = if sub { "wrapping_sub" } else { "wrapping_add" };
            let (l, r) = (read(lhs), operand(rhs, width));
            if flags {
                writeln!(
                    out,
                    "let a = {l} & mask({bits}); let b = {r} & mask({bits}); let result = a.{op}(b) & mask({bits}); c.flags(a, b, result, {bits}, {sub});"
                )
                .unwrap();
                out.push_str(&write(dst, "result", width));
            } else if width == Width::W32 {
                out.push_str(&write(
                    dst,
                    &format!("({l} as u32).{op}({r} as u32) as u64"),
                    Width::X64,
                ));
            } else {
                out.push_str(&write(dst, &format!("{l}.{op}({r})"), Width::X64));
            }
        }
        Op::Logical {
            width,
            dst,
            lhs,
            rhs,
            kind,
            invert,
        } => {
            let rhs = operand(rhs, width);
            let rhs = if invert { format!("!{rhs}") } else { rhs };
            let symbol = match kind {
                0 | 3 => "&",
                1 => "|",
                _ => "^",
            };
            let value = format!("({} {symbol} {rhs})", read(lhs));
            if kind == 3 {
                writeln!(
                    out,
                    "let result = {value} & mask({}); c.nzcv = (((result >> {}) as u8) << 3) | (((result == 0) as u8) << 2);",
                    width.bits(),
                    width.bits() - 1
                )
                .unwrap();
                out.push_str(&write(dst, "result", width));
            } else {
                out.push_str(&write(dst, &value, width));
            }
        }
        Op::Address { dst, value } => {
            out.push_str(&write(dst, &format!("{}u64", value.0), Width::X64))
        }
        Op::Branch { target, link } => {
            if link {
                writeln!(out, "c.x[30] = {next};").unwrap();
            }
            writeln!(out, "c.pc = {};", target.0).unwrap();
        }
        Op::Conditional { target, condition } => writeln!(
            out,
            "c.pc = if c.condition({condition}) {{ {} }} else {{ {next} }};",
            target.0
        )
        .unwrap(),
        Op::CompareBranch {
            width,
            src,
            target,
            nonzero,
        } => writeln!(
            out,
            "c.pc = if (({} & mask({})) != 0) == {nonzero} {{ {} }} else {{ {next} }};",
            read(src),
            width.bits(),
            target.0
        )
        .unwrap(),
        Op::TestBranch {
            src,
            bit,
            target,
            nonzero,
        } => writeln!(
            out,
            "c.pc = if (({} & (1u64 << {bit})) != 0) == {nonzero} {{ {} }} else {{ {next} }};",
            read(src),
            target.0
        )
        .unwrap(),
        Op::Indirect { src, link } => {
            writeln!(out, "let target = {};", read(src)).unwrap();
            if link {
                writeln!(out, "c.x[30] = {next};").unwrap();
            }
            out.push_str("c.pc = target;\n");
        }
        Op::Memory {
            load,
            bytes,
            dst,
            base,
            offset,
            mode,
            signed,
            dst_width,
            index,
        } => {
            out.push_str(stack_check(base));
            if mode == 0 && index.is_none() {
                // Base plus constant offset, no writeback: one expression.
                let address = if offset == 0 {
                    read(base)
                } else {
                    format!("{}.wrapping_add({:#x})", read(base), offset as u64)
                };
                if load {
                    let value = format!("m.load({address}, {bytes})?");
                    let value = if signed {
                        format!("sign_extend({value}, {}) as u64", bytes * 8)
                    } else {
                        value
                    };
                    if dst == Reg::Zero {
                        writeln!(out, "{value};").unwrap();
                    } else {
                        out.push_str(&write(dst, &value, dst_width));
                    }
                } else {
                    writeln!(out, "m.store({address}, {bytes}, {})?;", read(dst)).unwrap();
                }
                return out;
            }
            let index = index.map_or(String::new(), |i| {
                format!(
                    ".wrapping_add(extend({}, {}, {}))",
                    read(i.reg),
                    i.extend,
                    i.shift
                )
            });
            writeln!(out,"let base = {}; let adjusted = base.wrapping_add_signed({offset}i64){index}; let address = {};",read(base),if mode==1 {"base"} else {"adjusted"}).unwrap();
            if load {
                writeln!(out, "let value = m.load(address, {bytes})?;").unwrap();
                let value = if signed {
                    format!("sign_extend(value, {}) as u64", bytes * 8)
                } else {
                    "value".into()
                };
                out.push_str(&write(dst, &value, dst_width));
            } else {
                writeln!(out, "m.store(address, {bytes}, {})?;", read(dst)).unwrap();
            }
            if mode != 0 {
                out.push_str(&write(base, "adjusted", Width::X64));
            }
        }
        Op::LoadLiteral {
            dst,
            address,
            bytes,
            signed,
        } => {
            writeln!(out, "let value = m.load({}, {bytes})?;", address.0).unwrap();
            let (value, width) = match (signed, bytes) {
                (true, _) => ("sign_extend(value, 32) as u64", Width::X64),
                (false, 8) => ("value", Width::X64),
                _ => ("value", Width::W32),
            };
            out.push_str(&write(dst, value, width));
        }
        Op::Pair {
            load,
            bytes,
            first,
            second,
            base,
            offset,
            mode,
            signed,
        } => {
            out.push_str(stack_check(base));
            writeln!(out,"let base = {}; let adjusted = base.wrapping_add_signed({offset}i64); let address = {};",read(base),if mode==1 {"base"} else {"adjusted"}).unwrap();
            if load {
                writeln!(out,"let mut data = [0u8; {}]; m.read_into(address, &mut data)?; let mut a = [0u8;8]; let mut b = [0u8;8]; a[..{bytes}].copy_from_slice(&data[..{bytes}]); b[..{bytes}].copy_from_slice(&data[{bytes}..]);",bytes*2).unwrap();
                let (a, b, width) = if signed {
                    (
                        "sign_extend(u64::from_le_bytes(a), 32) as u64",
                        "sign_extend(u64::from_le_bytes(b), 32) as u64",
                        Width::X64,
                    )
                } else {
                    (
                        "u64::from_le_bytes(a)",
                        "u64::from_le_bytes(b)",
                        if bytes == 8 { Width::X64 } else { Width::W32 },
                    )
                };
                out.push_str(&write(first, a, width));
                out.push_str(&write(second, b, width));
            } else {
                writeln!(out,"let mut data = [0u8;{}]; data[..{bytes}].copy_from_slice(&({}).to_le_bytes()[..{bytes}]); data[{bytes}..].copy_from_slice(&({}).to_le_bytes()[..{bytes}]); m.write_bytes(address, &data)?;",bytes*2,read(first),read(second)).unwrap();
            }
            if mode != 2 {
                out.push_str(&write(base, "adjusted", Width::X64));
            }
        }
        Op::Exclusive {
            load,
            exclusive,
            ordered: _,
            pair,
            bytes,
            status,
            first,
            second,
            base,
        } => {
            // Single guest thread: acquire/release ordering has no observable effect; the
            // local monitor models exclusivity (a store-exclusive succeeds only if it matches
            // the address and size tagged by the preceding load-exclusive).
            let total = u64::from(bytes) * if pair { 2 } else { 1 };
            out.push_str(stack_check(base));
            writeln!(
                out,
                "let address = {}; check_alignment(address, {total})?;",
                read(base)
            )
            .unwrap();
            let width = if bytes == 8 { Width::X64 } else { Width::W32 };
            if load {
                writeln!(out, "let value = m.load(address, {bytes})?;").unwrap();
                if pair {
                    writeln!(
                        out,
                        "let value2 = m.load(address.wrapping_add({bytes}), {bytes})?;"
                    )
                    .unwrap();
                }
                if exclusive {
                    writeln!(out, "c.monitor = Some((address, {total}));").unwrap();
                }
                out.push_str(&write(first, "value", width));
                if pair {
                    out.push_str(&write(second, "value2", width));
                }
            } else {
                let mut store = format!("m.store(address, {bytes}, {})?;", read(first));
                if pair {
                    store.push_str(&format!(
                        " m.store(address.wrapping_add({bytes}), {bytes}, {})?;",
                        read(second)
                    ));
                }
                if exclusive {
                    writeln!(out, "let success = c.monitor == Some((address, {total})); c.monitor = None; if success {{ {store} }}").unwrap();
                    out.push_str(&write(status, "(!success) as u64", Width::W32));
                } else {
                    out.push_str(&store);
                    out.push('\n');
                }
            }
        }
        Op::Barrier { .. } | Op::CacheMaintenance => {}
        Op::ClearExclusive => out.push_str("c.monitor = None;\n"),
        Op::SystemRegister {
            read: is_read,
            register,
            reg,
        } => {
            let field = match register {
                SystemRegister::TpidrEl0 => "c.tpidr_el0",
                SystemRegister::TpidrroEl0 => "c.tpidrro_el0",
                SystemRegister::Fpcr => "c.fpcr",
                SystemRegister::Fpsr => "c.fpsr",
                SystemRegister::Nzcv => "u64::from(c.nzcv) << 28",
                // Cortex-A57 identification values: 64-byte cache lines, DC ZVA block of 64 bytes.
                SystemRegister::CtrEl0 => "0x8444_c004u64",
                SystemRegister::DczidEl0 => "4u64",
            };
            if is_read {
                out.push_str(&write(reg, field, Width::X64));
            } else if register == SystemRegister::Nzcv {
                writeln!(out, "c.nzcv = (({} >> 28) & 15) as u8;", read(reg)).unwrap();
            } else {
                // Only implemented FPCR/FPSR bits are writable.
                let mask = match register {
                    SystemRegister::Fpcr => " & FPCR_WRITE_MASK",
                    SystemRegister::Fpsr => " & FPSR_WRITE_MASK",
                    _ => "",
                };
                writeln!(out, "{field} = {}{mask};", read(reg)).unwrap();
            }
        }
        Op::ZeroBlock { address } => {
            writeln!(out, "m.write_bytes({} & !63, &[0u8; 64])?;", read(address)).unwrap();
        }
        Op::Trap { kind, imm } => {
            writeln!(
                out,
                "return Err(\"guest {} #{imm:#x} exception\".into());",
                if kind == 0 { "BRK" } else { "HLT" }
            )
            .unwrap();
        }
        Op::Carry {
            width,
            dst,
            lhs,
            rhs,
            sub,
            flags,
        } => {
            let bits = width.bits();
            let y = if sub {
                format!("!{} & mask({bits})", read(rhs))
            } else {
                read(rhs)
            };
            writeln!(
                out,
                "let (result, f) = add_with_carry({}, {y}, c.nzcv & 2 != 0, {bits});",
                read(lhs)
            )
            .unwrap();
            if flags {
                out.push_str("c.nzcv = f;\n");
            } else {
                out.push_str("let _ = f;\n");
            }
            out.push_str(&write(dst, "result", width));
            if dst == Reg::Zero {
                out.push_str("let _ = result;\n");
            }
        }
        Op::Bitfield {
            width,
            dst,
            src,
            kind,
            rotate,
            sign_bit,
            wmask,
            tmask,
        } => {
            let value = format!(
                "bitfield({}, {}, {kind}, {rotate}, {sign_bit}, {wmask:#x}, {tmask:#x}, {})",
                read(dst),
                read(src),
                width.bits()
            );
            out.push_str(&write(dst, &value, width));
        }
        Op::Extract {
            width,
            dst,
            high,
            low,
            lsb,
        } => {
            let value = format!(
                "extract({}, {}, {lsb}, {})",
                read(high),
                read(low),
                width.bits()
            );
            out.push_str(&write(dst, &value, width));
        }
        Op::Select {
            width,
            dst,
            lhs,
            rhs,
            condition,
            invert,
            increment,
        } => {
            let other = match (invert, increment) {
                (false, false) => read(rhs),
                (false, true) => format!("{}.wrapping_add(1)", read(rhs)),
                (true, false) => format!("!{}", read(rhs)),
                (true, true) => format!("{}.wrapping_neg()", read(rhs)),
            };
            let value = format!(
                "(if c.condition({condition}) {{ {} }} else {{ {other} }}) & mask({})",
                read(lhs),
                width.bits()
            );
            out.push_str(&write(dst, &value, width));
        }
        Op::CondCompare {
            width,
            lhs,
            rhs,
            condition,
            nzcv,
            negate,
        } => {
            let bits = width.bits();
            writeln!(
                out,
                "if c.condition({condition}) {{ let a = {} & mask({bits}); let b = ({}) & mask({bits}); let r = a.{}(b) & mask({bits}); c.flags(a, b, r, {bits}, {}); }} else {{ c.nzcv = {nzcv}; }}",
                read(lhs),
                operand(rhs, width),
                if negate { "wrapping_add" } else { "wrapping_sub" },
                !negate
            )
            .unwrap();
        }
        Op::Divide {
            width,
            dst,
            lhs,
            rhs,
            signed,
        } => {
            let value = format!(
                "divide({}, {}, {signed}, {})",
                read(lhs),
                read(rhs),
                width.bits()
            );
            out.push_str(&write(dst, &value, width));
        }
        Op::ShiftVariable {
            width,
            dst,
            src,
            amount,
            kind,
        } => {
            let bits = width.bits();
            let value = format!(
                "shift({}, {kind}, ({} % {bits}) as u8, {bits})",
                read(src),
                read(amount)
            );
            out.push_str(&write(dst, &value, width));
        }
        Op::MultiplyAdd {
            width,
            dst,
            lhs,
            rhs,
            acc,
            sub,
        } => {
            let value = format!(
                "{}.{}({}.wrapping_mul({})) & mask({})",
                read(acc),
                if sub { "wrapping_sub" } else { "wrapping_add" },
                read(lhs),
                read(rhs),
                width.bits()
            );
            out.push_str(&write(dst, &value, width));
        }
        Op::MultiplyLong {
            dst,
            lhs,
            rhs,
            acc,
            sub,
            signed,
        } => {
            let cast = if signed {
                "as u32 as i32 as i64 as u64"
            } else {
                "as u32 as u64"
            };
            let value = format!(
                "{}.{}(({} {cast}).wrapping_mul({} {cast}))",
                read(acc),
                if sub { "wrapping_sub" } else { "wrapping_add" },
                read(lhs),
                read(rhs)
            );
            out.push_str(&write(dst, &value, Width::X64));
        }
        Op::MultiplyHigh {
            dst,
            lhs,
            rhs,
            signed,
        } => {
            let value = if signed {
                format!(
                    "((i128::from({} as i64) * i128::from({} as i64)) >> 64) as u64",
                    read(lhs),
                    read(rhs)
                )
            } else {
                format!(
                    "((u128::from({}) * u128::from({})) >> 64) as u64",
                    read(lhs),
                    read(rhs)
                )
            };
            out.push_str(&write(dst, &value, Width::X64));
        }
        Op::Unary {
            width,
            dst,
            src,
            kind,
        } => {
            let value = format!("unary({}, {kind}, {})", read(src), width.bits());
            out.push_str(&write(dst, &value, width));
        }
        Op::Svc { id } => {
            writeln!(
                out,
                "if let Some(status) = service(c, m, {id})? {{ return Ok(Some(status)); }} c.pc = {next};"
            )
            .unwrap();
        }
    }
    out
}
/// Operations that can fault record their PC first so the fault and step count are exact.
fn faults(op: Op) -> bool {
    matches!(
        op,
        Op::Memory { .. }
            | Op::Pair { .. }
            | Op::LoadLiteral { .. }
            | Op::Exclusive { .. }
            | Op::ZeroBlock { .. }
            | Op::Svc { .. }
            | Op::Trap { .. }
            | Op::VecMemory { .. }
            | Op::VecPair { .. }
            | Op::VecLiteral { .. }
            | Op::VecStructures(_)
            | Op::Fp(FpOp::Convert { .. })
            | Op::Simd(SimdOp::FpWiden { .. } | SimdOp::FpNarrow { .. })
    )
}
fn transfer(op: Op) -> bool {
    matches!(
        op,
        Op::Trap { .. }
            | Op::Branch { .. }
            | Op::Conditional { .. }
            | Op::CompareBranch { .. }
            | Op::TestBranch { .. }
            | Op::Indirect { .. }
            | Op::Svc { .. }
    )
}

/// Generated crates for one program.
pub struct Emitted {
    /// Translation units in ascending guest-address order.
    pub units: Vec<Unit>,
    /// Source of the executable crate (unit table, module map, memory setup, `main`).
    pub main: String,
    /// (file name relative to the main source, bytes); trailing zero bytes omitted.
    pub blobs: Vec<(String, Vec<u8>)>,
}
pub struct Unit {
    pub name: String,
    pub start: u64,
    pub source: String,
    pub blocks: usize,
    pub instructions: usize,
}
#[derive(Clone, Debug)]
pub struct EmitOptions {
    /// Blocks starting in the same aligned `1 << unit_shift`-byte guest window form a unit.
    pub unit_shift: u32,
    /// Launch as a Horizon process instead of the register-level test harness.
    pub horizon: Option<HorizonOptions>,
}
/// Process parameters for a Horizon launch, emitted into the executable crate.
#[derive(Clone, Debug)]
pub struct HorizonOptions {
    pub code_start: u64,
    pub code_end: u64,
    pub program_id: u64,
    pub main_thread_stack_size: u64,
    pub main_thread_priority: u8,
    pub ideal_core: u8,
    pub core_mask: u64,
    pub priority_mask: u64,
    /// 0 selects the kernel maximum (1024).
    pub handle_table_size: u16,
    pub is_application: bool,
    /// Bit `id` permits SVC `id` (0..0xC0).
    pub svc_permissions: [u64; 3],
    /// ACI0 service access control: (may host, name or prefix ending in `*`).
    pub service_access: Vec<(bool, String)>,
    pub memory_limit: u64,
    /// Origin of the values, reported by the launcher (e.g. "main.npdm").
    pub source: &'static str,
    /// Start with the homebrew (NRO) entry convention instead of the NSO one.
    pub homebrew: bool,
}
impl Default for EmitOptions {
    fn default() -> Self {
        Self {
            unit_shift: 12,
            horizon: None,
        }
    }
}
const ALLOWS: &str = "#![allow(unused_variables, unused_parens, unreachable_code, unused_mut)]\n";

fn block_source(out: &mut String, block: &wl_analysis::Block) -> Result<()> {
    writeln!(
        out,
        "pub fn b{:x}(c: &mut Cpu, m: &mut Process) -> RuntimeResult<Option<Stop>> {{",
        block.start.0
    )
    .unwrap();
    writeln!(
        out,
        "c.enter({:#x}, {})?;",
        block.start.0,
        block.instructions.len()
    )
    .unwrap();
    for instruction in &block.instructions {
        writeln!(
            out,
            "// {:#x}: {:#010x}",
            instruction.pc.0, instruction.word
        )
        .unwrap();
        if faults(instruction.op) {
            writeln!(out, "c.pc = {:#x};", instruction.pc.0).unwrap();
        }
        out.push_str(&lower(instruction.op, instruction.pc.0 + 4));
    }
    if let Some(u) = &block.unsupported {
        writeln!(
            out,
            "c.pc = {}; return Err(\"unsupported instruction reached: opcode {:#010x} ({}) at guest PC {:#x}\".into());",
            u.pc.0,
            u.word,
            wl_aarch64::encoding_class(u.word),
            u.pc.0
        )
        .unwrap();
    } else {
        let last = block.instructions.last().ok_or("empty CFG block")?;
        if !transfer(last.op) {
            writeln!(out, "c.pc = {};", last.pc.0 + 4).unwrap();
        }
    }
    out.push_str("Ok(None)\n}\n");
    Ok(())
}

/// Emits one crate per guest-address window plus the executable crate. `modules` names the
/// loaded modules as (start, end, name) for fault reports.
pub fn emit(
    image: &Image,
    program: &Program,
    blob_stem: &str,
    modules: &[(u64, u64, String)],
    options: EmitOptions,
) -> Result<Emitted> {
    if image.relocation == RelocationState::NotApplied {
        return Err("module has MOD0 dynamic relocations that were not applied; load it as a process (directory input) so wl-loader can link it".into());
    }
    if !(8..=40).contains(&options.unit_shift) {
        return Err("unit window must be 2^8..2^40 bytes".into());
    }
    // Detect module/stack conflicts at translation time, with the runtime's actual mapping rules.
    let mut check = wl_runtime::Memory::default();
    check.map(
        wl_runtime::SCRATCH_BASE,
        vec![0; wl_runtime::SCRATCH_SIZE],
        true,
        true,
        false,
    )?;
    for m in &image.mappings {
        check.map(m.base.0, m.bytes.clone(), m.read, m.write, m.execute)?;
    }
    let mut units: Vec<Unit> = Vec::new();
    for block in &program.blocks {
        let start = block.start.0 >> options.unit_shift << options.unit_shift;
        if units.last().is_none_or(|u| u.start != start) {
            units.push(Unit {
                name: format!("wlu_{start:x}"),
                start,
                source: format!(
                    "// Translated guest blocks in [{start:#x}, {:#x}).\n{ALLOWS}use wl_guest_runtime::*;\n",
                    start + (1u64 << options.unit_shift)
                ),
                blocks: 0,
                instructions: 0,
            });
        }
        let unit = units.last_mut().ok_or("unit")?;
        block_source(&mut unit.source, block)?;
        unit.blocks += 1;
        unit.instructions += block.instructions.len();
    }
    // Each unit exports its sorted block table; the executable only lists the units.
    let mut index = 0;
    for unit in &mut units {
        let blocks = &program.blocks[index..index + unit.blocks];
        index += unit.blocks;
        writeln!(
            unit.source,
            "pub static BLOCKS: [(u64, BlockFn); {}] = [",
            blocks.len()
        )
        .unwrap();
        for b in blocks {
            writeln!(unit.source, "({:#x}, b{:x}),", b.start.0, b.start.0).unwrap();
        }
        unit.source.push_str("];\n");
    }
    let mut out = format!(
        "// Generated AArch64 translation: executable crate. No runtime decoder or JIT.\n{ALLOWS}extern crate wl_guest_runtime;\nuse wl_guest_runtime::*;\n"
    );
    for unit in &units {
        writeln!(out, "extern crate {};", unit.name).unwrap();
    }
    writeln!(
        out,
        "static UNITS: [(u64, BlockTable); {}] = [",
        units.len()
    )
    .unwrap();
    for unit in &units {
        writeln!(out, "({:#x}, &{}::BLOCKS),", unit.start, unit.name).unwrap();
    }
    out.push_str("];\n");
    writeln!(
        out,
        "static MODULES: [(u64, u64, &str); {}] = [",
        modules.len()
    )
    .unwrap();
    for (start, end, name) in modules {
        writeln!(out, "({start:#x}, {end:#x}, {name:?}),").unwrap();
    }
    out.push_str("];\n");
    writeln!(
        out,
        "static PROGRAM: Program = Program {{ entry: {:#x}, units: &UNITS, modules: &MODULES }};",
        program.entry.0
    )
    .unwrap();
    out.push_str("fn setup() -> RuntimeResult<Memory> { let mut m = Memory::default();\n");
    if options.horizon.is_none() {
        out.push_str("m.map(SCRATCH_BASE, vec![0; SCRATCH_SIZE], true, true, false)?;\n");
    }
    let mut blobs = Vec::new();
    for mapping in &image.mappings {
        if mapping.bytes.is_empty() {
            continue;
        }
        let stored = mapping
            .bytes
            .iter()
            .rposition(|b| *b != 0)
            .map_or(0, |last| last + 1);
        let initial = if stored == 0 {
            "Vec::new()".to_string()
        } else {
            let name = format!("{blob_stem}.{:x}.bin", mapping.base.0);
            let text = format!("include_bytes!({name:?}).to_vec()");
            blobs.push((name, mapping.bytes[..stored].to_vec()));
            text
        };
        writeln!(
            out,
            "{{ let mut b = {initial}; b.resize({}, 0); m.map_state({}, b, {}, {}, {}, {})?; }}",
            mapping.bytes.len(),
            mapping.base.0,
            mapping.read,
            mapping.write,
            mapping.execute,
            // Module segments: text and rodata are Code, data and BSS are CodeData.
            if mapping.write {
                "horizon::state::CODE_DATA"
            } else {
                "horizon::state::CODE"
            }
        )
        .unwrap();
    }
    let launcher = match &options.horizon {
        None => "launch(&PROGRAM, m)".to_string(),
        Some(h) => {
            writeln!(
                out,
                "Ok(m) }}\nstatic IMAGE: ProcessImage = ProcessImage {{ code_start: {:#x}, code_end: {:#x}, config_source: {:?}, homebrew: {}, config: horizon::ProcessConfig {{ program_id: {:#x}, process_id: 0x51, main_thread_id: 0x72, main_thread_stack_size: {:#x}, main_thread_priority: {}, ideal_core: {}, core_mask: {:#x}, priority_mask: {:#x}, memory_limit: {:#x}, handle_table_size: {}, thread_limit: 32, is_application: {}, svc_permissions: {:#x?}, service_access: &{:?} }} }};",
                h.code_start,
                h.code_end,
                h.source,
                h.homebrew,
                h.program_id,
                h.main_thread_stack_size,
                h.main_thread_priority,
                h.ideal_core,
                h.core_mask,
                h.priority_mask,
                h.memory_limit,
                h.handle_table_size,
                h.is_application,
                h.svc_permissions,
                h.service_access
            )
            .unwrap();
            "launch_horizon(&PROGRAM, m, &IMAGE)".to_string()
        }
    };
    if options.horizon.is_none() {
        out.push_str("Ok(m) }\n");
    }
    writeln!(out, "fn main() {{ let m = match setup() {{ Ok(m) => m, Err(e) => {{ eprintln!(\"mapping: {{e}}\"); std::process::exit(2); }} }}; std::process::exit({launcher}); }}").unwrap();
    Ok(Emitted {
        units,
        main: out,
        blobs,
    })
}
