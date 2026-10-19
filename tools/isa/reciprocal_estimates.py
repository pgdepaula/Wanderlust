#!/usr/bin/env python3
"""Compare Armv8 FRECPE/FRSQRTE helper bits and flags with Unicorn Cortex-A57.

Uses all normal table bins, edge inputs, 512 deterministic random inputs per
format and all rounding/FZ/DN combinations. This validates the scalar arithmetic
helper; the CLI pipeline test separately verifies decoding and native lowering.
Requires rustc and the optional Unicorn Python package. No game data is used.
"""

import subprocess, random, pathlib, tempfile, json

ROOT = pathlib.Path(__file__).resolve().parents[2]

from unicorn import Uc, UC_ARCH_ARM64, UC_MODE_ARM
from unicorn.arm64_const import (
    UC_CPU_ARM64_A57,
    UC_ARM64_REG_Q0,
    UC_ARM64_REG_Q1,
    UC_ARM64_REG_FPCR,
    UC_ARM64_REG_FPSR,
    UC_ARM64_REG_CPACR_EL1,
)

r = random.Random(0xFECFE)
cases = []
expected = []
for bits in [32, 64]:
    eb, mb = (8, 23) if bits == 32 else (11, 52)
    bias = (1 << (eb - 1)) - 1
    inf = ((1 << eb) - 1) << mb
    sign = 1 << (bits - 1)
    vals = [
        0,
        sign,
        1,
        sign | 1,
        (1 << mb) - 1,
        1 << mb,
        (1 << mb) + 1,
        inf,
        inf | sign,
        inf | 1,
        inf | (1 << (mb - 1)),
        inf | sign | 1,
        inf | (1 << (mb - 1)) | 0x91,
        inf - 1,
        sign | (inf - 1),
    ]
    vals += [(bias << mb) | (q << (mb - 8)) for q in range(256)]
    vals += [((bias + 1) << mb) | (q << (mb - 8)) for q in range(256)]
    vals += [r.getrandbits(bits) for _ in range(512)]
    for sq in [False, True]:
        uc = Uc(UC_ARCH_ARM64, UC_MODE_ARM)
        uc.ctl_set_cpu_model(UC_CPU_ARM64_A57)
        uc.mem_map(0x1000, 0x1000)
        uc.reg_write(UC_ARM64_REG_CPACR_EL1, 3 << 20)
        w = 0x4EA1D820 | (0x20000000 if sq else 0) | (0x400000 if bits == 64 else 0)
        uc.mem_write(0x1000, w.to_bytes(4, "little"))
        for mode in range(16):
            fpcr = (
                ((mode & 3) << 22) | ((mode >> 2 & 1) << 24) | ((mode >> 3 & 1) << 25)
            )
            for value in vals:
                uc.reg_write(
                    UC_ARM64_REG_Q1,
                    sum(value << (bits * i) for i in range(128 // bits)),
                )
                uc.reg_write(UC_ARM64_REG_FPCR, fpcr)
                uc.reg_write(UC_ARM64_REG_FPSR, 0)
                uc.emu_start(0x1000, 0x1004, count=1)
                expected.append(
                    (
                        uc.reg_read(UC_ARM64_REG_Q0) & ((1 << bits) - 1),
                        uc.reg_read(UC_ARM64_REG_FPSR),
                    )
                )
                cases.append((bits, fpcr, value, int(sq)))
input = "".join(f"{b} {f:x} {v:x} {s}\n" for b, f, v, s in cases)
with tempfile.TemporaryDirectory(prefix="wanderlust-recip-") as directory:
    directory = pathlib.Path(directory)
    module = json.dumps(str(ROOT / "crates/wl-runtime/src/fp.rs"), ensure_ascii=False)
    source = directory / "main.rs"
    source.write_text(
        "#![allow(dead_code)]\nuse std::io::{self,BufRead};\npub struct Cpu{fpcr:u64,fpsr:u64}\ntype RuntimeResult<T>=Result<T,String>;\n#[path="
        + module
        + '] mod fp;\nfn main(){for l in io::stdin().lock().lines(){let l=l.unwrap();let s:Vec<_>=l.split_whitespace().collect();let f=fp::format(s[0].parse().unwrap());let mut c=Cpu{fpcr:u64::from_str_radix(s[1],16).unwrap(),fpsr:0};let a=u64::from_str_radix(s[2],16).unwrap();let r=fp::reciprocal_estimate(&mut c,f,a,s[3]=="1");println!("{r:x} {:x}",c.fpsr);}}',
        encoding="utf-8",
    )
    binary = directory / "reference-helper"
    subprocess.run(
        ["rustc", "--edition=2024", "-O", str(source), "-o", str(binary)],
        check=True,
        timeout=120,
    )
    p = subprocess.run(
        [str(binary)],
        input=input,
        text=True,
        capture_output=True,
        check=True,
        timeout=120,
    )
actual = [tuple(int(x, 16) for x in l.split()) for l in p.stdout.splitlines()]
errors = []
for case, a, e in zip(cases, actual, expected):
    if a != e:
        errors.append((case, a, e))
        if len(errors) < 20:
            print("MISMATCH", case, tuple(hex(v) for v in a), tuple(hex(v) for v in e))
print("cases", len(cases), "actual", len(actual), "errors", len(errors))
raise SystemExit(bool(errors) or len(actual) != len(cases))
