#!/usr/bin/env python3
"""Concatenate executable sections of AArch64 ELF objects (or ar archives such as .rlib/.a)
into raw little-endian words for `wanderlust survey --raw`. Relocations are not applied, so
branch and address immediates are those of the unlinked object; encoding classes are unaffected."""
import argparse
import json
from pathlib import Path
import struct


def members(data):
    if not data.startswith(b'!<arch>\n'):
        yield 'object', data
        return
    at = 8
    while at + 60 <= len(data):
        name = data[at:at + 16].decode(errors='replace').strip()
        size = int(data[at + 48:at + 58].decode().strip())
        yield name, data[at + 60:at + 60 + size]
        at += 60 + size + (size & 1)


def symbols(obj):
    """(section index, value) of STT_FUNC symbols defined in an object."""
    shoff, = struct.unpack_from('<Q', obj, 0x28)
    shnum, = struct.unpack_from('<H', obj, 0x3C)
    for i in range(shnum):
        _, kind, _, _, offset, size, _, _, _, entsize = struct.unpack_from('<IIQQQQIIQQ', obj, shoff + 64 * i)
        if kind == 2:
            for at in range(offset, offset + size, entsize or 24):
                _, info, _, shndx, value, _ = struct.unpack_from('<IBBHQQ', obj, at)
                if info & 15 == 2 and 0 < shndx < 0xFF00:
                    yield shndx, value


def text_sections(obj):
    if obj[:4] != b'\x7fELF' or obj[4] != 2 or obj[5] != 1:
        return
    machine, = struct.unpack_from('<H', obj, 18)
    if machine != 183:
        return
    shoff, = struct.unpack_from('<Q', obj, 0x28)
    shnum, = struct.unpack_from('<H', obj, 0x3C)
    for i in range(shnum):
        _, kind, flags, _, offset, size = struct.unpack_from('<IIQQQQ', obj, shoff + 64 * i)
        if kind == 1 and flags & 4 and size:
            yield i, obj[offset:offset + size - size % 4]


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('inputs', nargs='+', type=Path)
    parser.add_argument('--out', type=Path, required=True)
    parser.add_argument('--roots', type=Path, help='write STT_FUNC addresses, one per line')
    parser.add_argument('--base', type=lambda v: int(v, 0), default=0x10000)
    args = parser.parse_args()
    words = bytearray()
    sections = 0
    roots = []
    for path in args.inputs:
        for _, member in members(path.read_bytes()):
            if member[:4] != b'\x7fELF':
                continue
            placed = {}
            for index, text in text_sections(member):
                placed[index] = len(words)
                words += text
                sections += 1
            roots += [args.base + placed[s] + v for s, v in symbols(member) if s in placed]
    args.out.write_bytes(words)
    if args.roots:
        args.roots.write_text(''.join(f'{r:#x}\n' for r in sorted(set(roots))), encoding='utf-8')
    print(json.dumps({'sections': sections, 'bytes': len(words), 'function_symbols': len(set(roots)), 'out': str(args.out)}))


if __name__ == '__main__':
    main()
