#!/usr/bin/env python3
"""Encode simd_programs.py with Keystone and verify every word with Capstone.

A word is accepted when Capstone disassembles it and Keystone re-assembles Capstone's text to
the same word (two independent implementations agree on the encoding). Output:
simd_encoding.json, mapping each assembly line to its little-endian word.
"""
import json
from pathlib import Path
import struct
import sys

import capstone
import keystone

sys.path.insert(0, str(Path(__file__).resolve().parent))
from simd_programs import PROGRAMS  # noqa: E402

OUT = Path(__file__).resolve().parent / 'simd_encoding.json'


def main():
    ks = keystone.Ks(keystone.KS_ARCH_ARM64, keystone.KS_MODE_LITTLE_ENDIAN)
    cs = capstone.Cs(capstone.CS_ARCH_ARM64, capstone.CS_MODE_ARM)
    table, problems = {}, []
    for name, lines in PROGRAMS.items():
        for line in lines:
            if line in table:
                continue
            encoded, _ = ks.asm(line)
            if not encoded or len(encoded) != 4:
                problems.append((name, line, 'keystone did not produce one word'))
                continue
            word = struct.unpack('<I', bytes(encoded))[0]
            decoded = list(cs.disasm(bytes(encoded), 0))
            if not decoded:
                problems.append((name, line, f'capstone rejects {word:#010x}'))
                continue
            text = f'{decoded[0].mnemonic} {decoded[0].op_str}'
            again, _ = ks.asm(text.replace('#-0.000000e+00', '#-0.0'))
            if again is None or bytes(again) != bytes(encoded):
                problems.append((name, line, f'{word:#010x} disassembles to {text!r}, which re-encodes differently'))
                continue
            table[line] = word
    for problem in problems:
        print('PROBLEM', *problem)
    OUT.write_text(json.dumps(table, indent=1, sort_keys=True) + '\n', encoding='utf-8')
    print(json.dumps({'encoded': len(table), 'problems': len(problems), 'out': str(OUT)}))
    return 1 if problems else 0


if __name__ == '__main__':
    sys.exit(main())
