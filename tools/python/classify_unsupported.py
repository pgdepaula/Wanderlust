#!/usr/bin/env python3
"""Rank words rejected by the Wanderlust decoder by Capstone mnemonic and operand shape.

Input is the JSON of `wanderlust survey INPUT --examples N` (all unsupported opcodes with
counts). Capstone is an independent disassembler; this is a linear-sweep frequency ranking,
not reachability."""
import argparse
import collections
import json
import re
import struct

import capstone


def shape(op_str):
    """Operand shape: register classes and vector arrangements, immediates collapsed."""
    s = re.sub(r'#-?0x[0-9a-f]+|#-?[0-9.e+-]+', '#imm', op_str)
    s = re.sub(r'\b([xwbhsdqv])([0-9]+|zr)\b', r'\1', s)
    return s


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('survey', type=argparse.FileType())
    parser.add_argument('--top', type=int, default=60)
    args = parser.parse_args()
    survey = json.load(args.survey)
    md = capstone.Cs(capstone.CS_ARCH_ARM64, capstone.CS_MODE_ARM)
    mnemonics, shapes = collections.Counter(), collections.Counter()
    for entry in survey['unsupported_examples']:
        decoded = list(md.disasm(struct.pack('<I', entry['opcode']), 0))
        mnemonic = decoded[0].mnemonic if decoded else '(invalid)'
        mnemonics[mnemonic] += entry['count']
        shapes[f"{mnemonic} {shape(decoded[0].op_str) if decoded else ''}"] += entry['count']
    total = survey['words']
    listed = sum(e['count'] for e in survey['unsupported_examples'])
    print(json.dumps({'words': total, 'unsupported_words': survey['unsupported_words'], 'listed': listed}))
    for name, counter in (('mnemonic', mnemonics), ('shape', shapes)):
        print(f'--- by {name}')
        for key, count in counter.most_common(args.top):
            print(f'{count:7d} {100 * count / total:6.2f}%  {key}')


if __name__ == '__main__':
    main()
