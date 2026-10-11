#!/usr/bin/env python3
"""Compare compiled A64 fixtures with Unicorn: X0-X30, V0-V31, SP, NZCV, FPCR, FPSR, PC,
instruction count and scratch memory, all bit-exact."""
import argparse
import hashlib
import json
import os
from pathlib import Path
import random
import subprocess
import sys
import time

ROOT = Path(__file__).resolve().parents[2]
sys.path.insert(0, str(ROOT / 'tools/python'))
from fixtures import BASE, SCRATCH, generate  # noqa: E402

RETURN_PC = 0xFFFFFFFFFFFFFFFC
SIZE = 0x10000


# Floating-point lane values that exercise architectural corner cases.
F64 = [0x0, 0x8000000000000000, 0x3FF0000000000000, 0xBFF0000000000000, 0x7FF0000000000000,
       0xFFF0000000000000, 0x7FF8000000000001, 0x7FF0000000000001, 0xFFF800000000DEAD,
       0x1, 0x000FFFFFFFFFFFFF, 0x0010000000000000, 0x7FEFFFFFFFFFFFFF, 0x3FD5555555555555,
       0x4340000000000001, 0x3FE0000000000001, 0x3FF8000000000000, 0x4004000000000000,
       0xC004000000000000, 0x41DFFFFFFFC00000, 0x43E0000000000000, 0xC3E0000000000000,
       0x000F000000000001, 0x3CB0000000000000, 0x400921FB54442D18]
F32 = [0x00000000, 0x80000000, 0x3F800000, 0xBF800000, 0x7F800000, 0xFF800000, 0x7FC00001,
       0x7F800001, 0xFFC0BEEF, 0x00000001, 0x007FFFFF, 0x00800000, 0x7F7FFFFF, 0x3EAAAAAB,
       0x4B000001, 0x3F000001, 0x40200000, 0xC0200000, 0x4F000000, 0xCF000000, 0x5F000000,
       0x00400001, 0x33800000, 0x40490FDB, 0x477FE000]
F16 = [0x0000, 0x8000, 0x3C00, 0xBC00, 0x7C00, 0xFC00, 0x7E01, 0x7C01, 0x0001, 0x03FF, 0x0400,
       0x7BFF, 0x3555, 0xC000]
# FPCR per sample: rounding modes (RMode bits 23:22), FZ (24), DN (25);
# AHP (26) selects alternative half precision for conversions.
FPCRS = [0, 0, 0, 0, 1 << 22, 2 << 22, 3 << 22, 1 << 24, 1 << 25, 3 << 24, 0, 0,
         (3 << 22) | (1 << 24), (1 << 22) | (1 << 25), 1 << 26, 0]


def vector_state(rng, sample):
    """Initial V registers: FP corner values in every arrangement, plus random bit patterns."""
    registers = []
    for i in range(32):
        kind = (i + sample) % 5
        if kind == 0:
            value = rng.choice(F64) | rng.choice(F64) << 64
        elif kind == 1:
            value = sum(rng.choice(F32) << (32 * j) for j in range(4))
        elif kind == 2:
            value = sum(rng.choice(F16) << (16 * j) for j in range(8))
        elif kind == 3:
            value = rng.choice(F32) | rng.choice(F64) << 64 | rng.getrandbits(32) << 32
        else:
            value = rng.getrandbits(128)
        registers.append(value)
    return registers


def reference(code, registers, flags, memory, vectors=None, fpcr=0, fpsr=0):
    from unicorn import Uc, UC_ARCH_ARM64, UC_MODE_ARM, UC_HOOK_CODE
    import unicorn.arm64_const as arm
    uc = Uc(UC_ARCH_ARM64, UC_MODE_ARM)
    uc.mem_map(BASE, (len(code)+4095)//4096*4096)
    uc.mem_write(BASE, code)
    uc.mem_map(SCRATCH, SIZE)
    uc.mem_write(SCRATCH, memory)
    reg_ids = [getattr(arm, f'UC_ARM64_REG_X{i}') for i in range(31)]
    for reg, value in zip(reg_ids, registers):
        uc.reg_write(reg, value)
    uc.reg_write(arm.UC_ARM64_REG_SP, SCRATCH+SIZE)
    uc.reg_write(arm.UC_ARM64_REG_NZCV, flags << 28)
    vector_ids = [getattr(arm, f'UC_ARM64_REG_Q{i}') for i in range(32)]
    for reg, value in zip(vector_ids, vectors or [0] * 32):
        uc.reg_write(reg, value)
    uc.reg_write(arm.UC_ARM64_REG_FPCR, fpcr)
    uc.reg_write(arm.UC_ARM64_REG_FPSR, fpsr)
    steps = [0]
    uc.hook_add(UC_HOOK_CODE, lambda *_: steps.__setitem__(0, steps[0]+1))
    uc.emu_start(BASE, RETURN_PC, count=10000)
    state = {'x': [uc.reg_read(r) for r in reg_ids], 'sp': uc.reg_read(arm.UC_ARM64_REG_SP),
             'pc': uc.reg_read(arm.UC_ARM64_REG_PC), 'nzcv': uc.reg_read(arm.UC_ARM64_REG_NZCV) >> 28,
             'steps': steps[0], 'status': 'returned', 'v': [uc.reg_read(r) for r in vector_ids],
             'fpcr': uc.reg_read(arm.UC_ARM64_REG_FPCR), 'fpsr': uc.reg_read(arm.UC_ARM64_REG_FPSR)}
    if state['pc'] != RETURN_PC:
        raise RuntimeError(f"reference did not return: pc={state['pc']:#x}")
    return state, bytes(uc.mem_read(SCRATCH, SIZE))


def native_path(path, windows):
    if windows and sys.platform != 'win32':
        return subprocess.check_output(['wslpath', '-w', str(path.resolve())], text=True).strip()
    return str(path.resolve())


def run(args):
    import capstone
    import unicorn
    directory = args.out_dir.resolve()
    manifest = generate(directory / 'fixtures')
    cli = args.cli.resolve()
    windows = args.target is not None and 'windows' in args.target
    cases = []
    for item in manifest:
        name = item['name']
        if args.case and name != args.case:
            continue
        binary = directory / (name + ('.exe' if windows or os.name == 'nt' else ''))
        raw = directory / 'fixtures' / (name+'.a64')
        command = [str(cli), 'recompile', str(raw), '--raw', '--entry', hex(BASE), '--out', str(binary)]
        if args.target:
            command += ['--target', args.target]
        for seed in item['seeds']:
            command += ['--seed', hex(seed)]
        start = time.perf_counter()
        compiled = subprocess.run(command, capture_output=True, text=True, timeout=90)
        if compiled.returncode:
            raise RuntimeError(f'{name}: native compilation failed\n{compiled.stderr}')
        compilation_seconds = time.perf_counter()-start
        code = raw.read_bytes()
        for sample in range(args.samples):
            rng = random.Random(0x574C0000 + sample)
            registers = [rng.getrandbits(64) for _ in range(31)]
            edges = [0, 1, 0x7FFFFFFFFFFFFFFF, 0x8000000000000000, 0xFFFFFFFFFFFFFFFF,
                     0xFFFFFFFF, 0x80000000, 0x7FFFFFFF]
            registers[0] = edges[sample % len(edges)]
            registers[1] = edges[(sample//len(edges)+sample+1) % len(edges)]
            registers[2] = SCRATCH+16
            registers[30] = RETURN_PC
            flags = sample % 16
            memory = rng.randbytes(SIZE)
            vectors = vector_state(rng, sample)
            fpcr = FPCRS[sample % len(FPCRS)]
            fpsr = 0x10 if sample % 3 == 1 else (0x9F if sample % 5 == 2 else 0)
            initial = directory / 'initial.memory'
            dump = directory / 'native.memory'
            initial.write_bytes(memory)
            command = [str(binary), '--nzcv', str(flags), '--memory', native_path(initial, windows), '--dump', native_path(dump, windows)]
            for reg, value in enumerate(registers):
                command += ['--reg', f'{reg}={value}']
            for reg, value in enumerate(vectors):
                command += ['--vreg', f'{reg}={value:#x}']
            command += ['--fpcr', str(fpcr), '--fpsr', str(fpsr)]
            native = subprocess.run(command, capture_output=True, text=True, timeout=15)
            if native.returncode:
                raise RuntimeError(f'{name} sample {sample}: native execution failed ({native.returncode})\n{native.stderr}\n{native.stdout}')
            actual = json.loads(native.stdout)
            actual['v'] = [int(v, 16) for v in actual['v']]
            expected, expected_memory = reference(code, registers, flags, memory, vectors, fpcr, fpsr)
            changes = {k: {'expected': expected[k], 'actual': actual.get(k)} for k in expected if actual.get(k) != expected[k] and k != 'v'}
            vector_changes = {f'v{i}': {'initial': hex(vectors[i]), 'expected': hex(e), 'actual': hex(a)}
                              for i, (e, a) in enumerate(zip(expected['v'], actual['v'])) if e != a}
            if vector_changes:
                changes['v'] = vector_changes
            for k in ('fpcr', 'fpsr'):
                if k in changes:
                    changes[k] = {key: hex(value) for key, value in changes[k].items()}
            native_memory = dump.read_bytes()
            if native_memory != expected_memory:
                differences = [i for i, (a, b) in enumerate(zip(native_memory, expected_memory)) if a != b]
                changes['memory'] = {'first_addresses': [hex(SCRATCH+i) for i in differences[:16]],
                                     'expected_sha256': hashlib.sha256(expected_memory).hexdigest(), 'actual_sha256': hashlib.sha256(native_memory).hexdigest()}
            if changes:
                decoder = capstone.Cs(capstone.CS_ARCH_ARM64, capstone.CS_MODE_ARM)
                report = {'case': name, 'sample': sample, 'initial_registers': registers, 'initial_nzcv': flags,
                          'initial_vectors': [hex(v) for v in vectors], 'initial_fpcr': hex(fpcr), 'initial_fpsr': hex(fpsr),
                          'changes': changes, 'disassembly': [{'pc': hex(i.address), 'instruction': i.mnemonic+' '+i.op_str} for i in decoder.disasm(code, BASE)]}
                (directory / 'mismatch.json').write_text(json.dumps(report, indent=2)+'\n', encoding='utf-8')
                print(json.dumps(report))
                return 1
        result = {'case': name, 'samples': args.samples, 'compile_seconds': round(compilation_seconds, 4), 'executable_bytes': binary.stat().st_size, **json.loads(compiled.stdout)}
        cases.append(result)
        print(json.dumps(result), flush=True)
    if not cases:
        raise RuntimeError('no matching differential cases')
    report = {'status': 'passed', 'programs': len(cases), 'comparisons': len(cases)*args.samples,
              'unicorn': unicorn.__version__, 'capstone': capstone.__version__, 'target': args.target or 'host',
              'compared': ['x0..x30', 'v0..v31', 'sp', 'pc', 'nzcv', 'fpcr', 'fpsr', 'instruction_count', '64KiB scratch'], 'cases': cases}
    (directory / 'report.json').write_text(json.dumps(report, indent=2)+'\n', encoding='utf-8')
    print(json.dumps({k: v for k, v in report.items() if k != 'cases'}))
    return 0


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--cli', type=Path, default=ROOT / 'target/debug/wanderlust')
    parser.add_argument('--out-dir', type=Path, default=ROOT / 'generated/differential')
    parser.add_argument('--samples', type=int, choices=range(1, 257), default=16)
    parser.add_argument('--case')
    parser.add_argument('--target')
    args = parser.parse_args()
    try:
        return run(args)
    except (OSError, ValueError, RuntimeError, subprocess.SubprocessError) as error:
        print(json.dumps({'status': 'error', 'error': str(error)}), file=sys.stderr)
        return 2


if __name__ == '__main__':
    sys.exit(main())
