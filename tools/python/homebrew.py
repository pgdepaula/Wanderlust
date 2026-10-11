#!/usr/bin/env python3
"""Build libnx test programs (tests/horizon/*.c) into ExeFS directories.

Uses devkitA64 and libnx exactly as libnx's switch_rules do: GCC with
-march=armv8-a+crc+crypto -mtune=cortex-a57 -mtp=soft -fPIE, link with switch.specs
(PIE, DT_RELR, --gc-sections), elf2nso, and npdmtool for main.npdm. The toolchain is
located through $DEVKITPRO, else target/toolchains/devkitpro/root/opt/devkitpro.

  homebrew.py SOURCE.c --out-dir DIR      -> DIR/main, DIR/main.npdm, DIR/elf/main.elf
"""
import argparse
import json
import os
from pathlib import Path
import subprocess
import sys

ROOT = Path(__file__).resolve().parents[2]
PROGRAM_ID = '0x0100000000002000'
NPDM = {
    'name': 'WanderlustProbe',
    'program_id': PROGRAM_ID,
    'program_id_range_min': PROGRAM_ID,
    'program_id_range_max': PROGRAM_ID,
    'main_thread_stack_size': '0x100000',
    'main_thread_priority': 44,
    'default_cpu_id': 0,
    'process_category': 0,
    'is_retail': True,
    'pool_partition': 0,
    'is_64_bit': True,
    'address_space_type': 3,
    'filesystem_access': {'permissions': '0xFFFFFFFFFFFFFFFF'},
    'service_access': ['*'],
    'service_host': ['*'],
    'kernel_capabilities': [
        {'type': 'kernel_flags', 'value': {'highest_thread_priority': 59, 'lowest_thread_priority': 28,
                                           'highest_cpu_id': 2, 'lowest_cpu_id': 0}},
        {'type': 'syscalls', 'value': {f'svc{i:02x}': f'0x{i:02x}' for i in range(1, 0x30)}},
        {'type': 'min_kernel_version', 'value': '0x0030'},
        {'type': 'handle_table_size', 'value': 1023},
    ],
}


def devkitpro():
    explicit = os.environ.get('DEVKITPRO')
    path = Path(explicit) if explicit else ROOT / 'target/toolchains/devkitpro/root/opt/devkitpro'
    if not (path / 'devkitA64/bin/aarch64-none-elf-gcc').is_file():
        raise SystemExit(f'devkitA64 not found under {path}; set DEVKITPRO (see docs/development/build-environment.md)')
    return path


def build(source, out_dir):
    dkp = devkitpro()
    gcc = dkp / 'devkitA64/bin/aarch64-none-elf-gcc'
    env = dict(os.environ, DEVKITPRO=str(dkp))
    work = out_dir / 'elf'
    work.mkdir(parents=True, exist_ok=True)
    arch = ['-march=armv8-a+crc+crypto', '-mtune=cortex-a57', '-mtp=soft', '-fPIE']
    elf = work / 'main.elf'
    subprocess.run([str(gcc), *arch, '-O2', '-g', '-Wall', '-ffunction-sections', '-D__SWITCH__',
                    '-I', str(dkp / 'libnx/include'), str(source),
                    f'-specs={dkp / "libnx/switch.specs"}', '-L', str(dkp / 'libnx/lib'), '-lnx',
                    '-o', str(elf)], check=True, env=env)
    subprocess.run([str(dkp / 'tools/bin/elf2nso'), str(elf), str(out_dir / 'main')], check=True,
                   env=env, stdout=subprocess.DEVNULL)
    (work / 'main.json').write_text(json.dumps(NPDM, indent=1), encoding='utf-8')
    subprocess.run([str(dkp / 'tools/bin/npdmtool'), str(work / 'main.json'), str(out_dir / 'main.npdm')],
                   check=True, env=env, stdout=subprocess.DEVNULL)
    return {'main': str(out_dir / 'main'), 'npdm': str(out_dir / 'main.npdm'), 'elf': str(elf)}


def main():
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    parser.add_argument('source', type=Path)
    parser.add_argument('--out-dir', type=Path, required=True)
    args = parser.parse_args()
    print(json.dumps(build(args.source, args.out_dir)))
    return 0


if __name__ == '__main__':
    sys.exit(main())
