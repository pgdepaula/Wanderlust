#!/usr/bin/env python3
"""Generate original A64/NSO test inputs. Standard library only; no game content."""
import argparse
import hashlib
import json
from pathlib import Path
import struct

BASE = 0x10000
SCRATCH = 0x10000000
RET = 0xD65F03C0
NOP = 0xD503201F


def words(code):
    return struct.pack('<' + 'I' * len(code), *code)


def movz(rd, imm, half=0, bits=64):
    return (0xD2800000 if bits == 64 else 0x52800000) | half << 21 | imm << 5 | rd


def add(rd, rn, imm, bits=64, sub=False, flags=False):
    return 0x11000000 | (bits == 64) << 31 | sub << 30 | flags << 29 | imm << 10 | rn << 5 | rd


def regop(rd, rn, rm, bits=64, sub=False, flags=False, kind=0, amount=0):
    return 0x0B000000 | (bits == 64) << 31 | sub << 30 | flags << 29 | kind << 22 | rm << 16 | amount << 10 | rn << 5 | rd


def logical(rd, rn, rm, kind=1, shift=0, amount=0, invert=False, bits=64):
    return 0x0A000000 | (bits == 64) << 31 | kind << 29 | shift << 22 | invert << 21 | rm << 16 | amount << 10 | rn << 5 | rd


def mem(rd, rn, offset=0, load=False, size=8):
    log2 = {1: 0, 2: 1, 4: 2, 8: 3}[size]
    assert offset % size == 0
    return 0x39000000 | log2 << 30 | load << 22 | offset // size << 10 | rn << 5 | rd


def branch(source, target, link=False):
    return (0x94000000 if link else 0x14000000) | ((target-source) & 0x3FFFFFF)


def programs():
    p = {}
    # Three loop iterations, a genuine BL/RET pair, memory, W-register zero extension.
    code = [logical(19, 31, 30), movz(0, 7), movz(1, 3), movz(2, 0x1000, 1),
            regop(3, 3, 0, flags=True), add(1, 1, 1, bits=32, sub=True, flags=True),
            0x54000001 | ((4-6) & 0x7FFFF) << 5,
            mem(3, 2), mem(4, 2, load=True, size=4), branch(9, 13, link=True),
            mem(0, 2, 8), logical(30, 31, 19), RET, add(0, 4, 5), RET]
    p['mixed'] = (code, [])
    for bits in (32, 64):
        for sub in (False, True):
            p[f'{"subs" if sub else "adds"}{bits}'] = ([regop(5, 0, 1, bits=bits, sub=sub, flags=True), RET], [])
        p[f'shifts{bits}'] = ([regop(5 + kind, 0, 1, bits=bits, kind=kind, amount=bits-1) for kind in range(3)] + [RET], [])
        for kind in range(4):
            p[f'logical{bits}_{kind}'] = ([logical(5 + shift, 0, 1, kind=kind, shift=shift, amount=bits-1, bits=bits, invert=bool(shift % 2)) for shift in range(4)] + [RET], [])
        code = [movz(5, 0xFFFF, bits=bits), (0xF2800000 if bits == 64 else 0x72800000) | 1 << 21 | 0xABCD << 5 | 5,
                (0x92800000 if bits == 64 else 0x12800000) | 0x12 << 5 | 6]
        if bits == 64:
            code += [0xF2E00005, movz(7, 0xBEEF, 3)]  # zero MOVK immediate at halfword 3
        p[f'wide{bits}'] = (code + [RET], [])
    # ADR to the entry and ADRP to the page preceding it; reads of code are permitted.
    p['addresses'] = ([0x10000005, 0xF0FFFFE6, mem(0, 2, 0), mem(5, 2, 8), mem(6, 2, 16), RET], [])
    code = []
    for i, size in enumerate((1, 2, 4, 8)):
        code += [mem(0, 2, i*8, size=size), mem(5+i, 2, i*8, load=True, size=size)]
    # STUR X0,[X2,#-1]; LDUR X10,[X2,#-1]; STR X1,[X2],#8; LDR X11,[X2,#-8]!
    code += [0xF81FF040, 0xF85FF04A, 0xF8008441, 0xF85F8C4B, RET]
    p['memory'] = (code, [])
    # Native call with a guest stack frame, followed by a nested BL.
    code = [logical(19,31,30), branch(1,4,True), logical(30,31,19), RET,
            0xA9BF7BFD, add(29,31,0), branch(6,10,True), 0xA8C17BFD, add(0,0,1), RET,
            add(0,0,2), RET]
    p['stack_calls'] = (code, [])
    p['sp_zero'] = ([add(31,31,32,sub=True), add(5,31,0), add(31,31,32), add(31,31,0,flags=True), logical(6,31,31), RET], [])
    # A BLR target cannot be inferred from the immediate ADR; supply a known analysis seed.
    p['indirect'] = ([logical(19,31,30), 0x100000B0, 0xD63F0200, logical(30,31,19), RET, NOP, add(0,0,1), RET], [BASE+24])
    # Every condition evaluated under each deterministic initial NZCV state.
    code = []
    for cond in range(16):
        code += [0x54000000 | 2 << 5 | cond, add(5,5,1), NOP]
    p['conditions'] = (code + [RET], [])
    # CBZ W0, +8; TBZ X1,#63,+8; CBNZ X0,+8; TBNZ W1,#0,+8.
    p['compare_test_branches'] = ([0x34000040, add(5,5,1), 0xB6F80041, add(6,6,1), 0xB5000040, add(7,7,1), 0x37000041, add(8,8,1), RET], [])
    # Pair W loads/stores, offset mode, and signed pre/post addressing.
    p['pairs32'] = ([0x29000440, 0x29401C46, RET], [])
    p.update(integer_programs())
    p.update(simd_programs())
    return p


def bitmask(n, imms, immr):
    """Arm ARM DecodeBitMasks(immediate=TRUE) wmask for 64 bits, or None if reserved."""
    combined = (n << 6) | (~imms & 0x3F)
    if combined == 0:
        return None
    length = combined.bit_length() - 1
    if length < 1:
        return None
    levels = (1 << length) - 1
    if imms & levels == levels:
        return None
    s, r, esize = imms & levels, immr & levels, 1 << length
    welem = (1 << (s + 1)) - 1
    rotated = ((welem >> r) | (welem << (esize - r))) & ((1 << esize) - 1) if r else welem
    value = 0
    for i in range(0, 64, esize):
        value |= rotated << i
    return value


def logical_imm(rd, rn, value, kind=0, bits=64):
    """AND (0), ORR (1), EOR (2), ANDS (3) with an encodable bitmask immediate."""
    full = value | (value << 32) if bits == 32 else value
    for n in ((0, 1) if bits == 64 else (0,)):
        for imms in range(64):
            for immr in range(64):
                if bitmask(n, imms, immr) == full:
                    return (bits == 64) << 31 | kind << 29 | 0x12000000 | n << 22 | immr << 16 | imms << 10 | rn << 5 | rd
    raise ValueError(f'{value:#x} is not a bitmask immediate')


def bitfield(rd, rn, immr, imms, kind, bits=64):
    """SBFM (0), BFM (1), UBFM (2)."""
    return (bits == 64) << 31 | kind << 29 | 0x13000000 | (bits == 64) << 22 | immr << 16 | imms << 10 | rn << 5 | rd


def dp2(rd, rn, rm, opcode, bits=64):
    return (bits == 64) << 31 | 0x1AC00000 | rm << 16 | opcode << 10 | rn << 5 | rd


def dp1(rd, rn, opcode, bits=64):
    return (bits == 64) << 31 | 0x5AC00000 | opcode << 10 | rn << 5 | rd


def dp3(rd, rn, rm, ra, op31, sub=False, bits=64):
    return (bits == 64) << 31 | 0x1B000000 | op31 << 21 | rm << 16 | sub << 15 | ra << 10 | rn << 5 | rd


def csel(rd, rn, rm, cond, op=0, o2=0, bits=64):
    return (bits == 64) << 31 | op << 30 | 0x1A800000 | rm << 16 | cond << 12 | o2 << 10 | rn << 5 | rd


def ccmp(rn, rm_or_imm, cond, nzcv, negate=False, immediate=False, bits=64):
    return (bits == 64) << 31 | (not negate) << 30 | 0x3A400000 | rm_or_imm << 16 | cond << 12 | immediate << 11 | rn << 5 | nzcv


def carry(rd, rn, rm, sub=False, flags=False, bits=64):
    return (bits == 64) << 31 | sub << 30 | flags << 29 | 0x1A000000 | rm << 16 | rn << 5 | rd


def extended(rd, rn, rm, option, shift, sub=False, flags=False, bits=64):
    return (bits == 64) << 31 | sub << 30 | flags << 29 | 0x0B200000 | rm << 16 | option << 13 | shift << 10 | rn << 5 | rd


def extr(rd, rn, rm, lsb, bits=64):
    return (bits == 64) << 31 | 0x13800000 | (bits == 64) << 22 | rm << 16 | lsb << 10 | rn << 5 | rd


def mrs(rt, key):
    return 0xD5300000 | key << 5 | rt


def msr(rt, key):
    return 0xD5100000 | key << 5 | rt


def exclusive(rt, rn, size, load, o0=0, pair=False, rs=31, rt2=31, o2=0):
    return size << 30 | 0x08000000 | o2 << 23 | load << 22 | pair << 21 | rs << 16 | o0 << 15 | rt2 << 10 | rn << 5 | rt


def mem_index(rt, rn, rm, size, opc, option=3, scaled=False):
    return size << 30 | 0x38200800 | opc << 22 | rm << 16 | option << 13 | scaled << 12 | rn << 5 | rt


def mem_unscaled(rt, rn, imm, size, opc, mode=0):
    return size << 30 | 0x38000000 | opc << 22 | (imm & 0x1FF) << 12 | mode << 10 | rn << 5 | rt


def literal(rt, source, target, opc):
    return opc << 30 | 0x18000000 | ((target - source) & 0x7FFFF) << 5 | rt


TPIDR_EL0, TPIDRRO_EL0, NZCV, CTR_EL0, DCZID_EL0 = 0x5E82, 0x5E83, 0x5A10, 0x5801, 0x5807


def integer_programs():
    """ARMv8.0 integer families beyond the original subset; registers 3-29 are outputs."""
    p = {}
    for bits in (32, 64):
        top = bits - 1
        cases = [(0, 0), (0, top), (top, top), (1, 0), (top, 0), (bits // 2, bits // 2 - 1), (3, 7), (7, 3), (bits - 8, bits - 9), (1, top)]
        code = []
        for i, (immr, imms) in enumerate(cases):
            kind = i % 3
            code.append(bitfield(3 + i, i % 2, immr, imms, kind, bits))
        code += [bitfield(14, 0, 0, 7, 0, bits), bitfield(15, 1, 0, 15, 2, bits), bitfield(16, 0, 1, 0, 1, bits)]
        p[f'bitfield{bits}'] = (code + [RET], [])
        values = [0x5555555555555555, 0x00FF00FF00FF00FF, 0xFFFF0000FFFF0000, 0x0F0F0F0F0F0F0F0F, 1, 0x7FFFFFFFFFFFFFFF, 0xFFFFFFFFFFFFFFFE, 0x0000FFFFFFFFFFF0]
        code = []
        for i, v in enumerate(values):
            v = v & 0xFFFFFFFF if bits == 32 else v
            if bits == 32 and v in (0, 0xFFFFFFFF):
                continue
            code.append(logical_imm(3 + i, i % 2, v, i % 4, bits))
        p[f'logical_imm{bits}'] = (code + [RET], [])
        code = []
        for cond in range(16):
            code.append(csel(3 + cond % 26, 0, 1, cond, op=cond % 2, o2=(cond // 2) % 2, bits=bits))
        p[f'select{bits}'] = (code + [RET], [])
        code = []
        for i in range(8):
            code += [ccmp(i % 2, (i * 5) % 32 if i % 3 == 0 else 1 - i % 2, (i * 3) % 16, i * 2 % 16, negate=i % 2 == 1, immediate=i % 3 == 0, bits=bits),
                     mrs(3 + i, NZCV)]
        p[f'ccmp{bits}'] = (code + [RET], [])
        code = [dp2(3, 0, 1, 2, bits), dp2(4, 0, 1, 3, bits), dp2(5, 1, 0, 2, bits), dp2(6, 1, 0, 3, bits),
                dp2(7, 0, 1, 8, bits), dp2(8, 0, 1, 9, bits), dp2(9, 0, 1, 10, bits), dp2(10, 0, 1, 11, bits),
                dp2(11, 1, 0, 8, bits), dp2(12, 1, 0, 10, bits), dp2(13, 0, 31, 3, bits)]
        p[f'divide_shift{bits}'] = (code + [RET], [])
        code = [dp3(3, 0, 1, 4, 0, False, bits), dp3(5, 0, 1, 6, 0, True, bits), dp3(7, 1, 1, 31, 0, False, bits)]
        if bits == 64:
            code += [dp3(8, 0, 1, 9, 1), dp3(10, 0, 1, 11, 1, True), dp3(12, 0, 1, 13, 5), dp3(14, 0, 1, 15, 5, True),
                     dp3(16, 0, 1, 31, 2), dp3(17, 0, 1, 31, 6), dp3(18, 1, 1, 31, 2), dp3(19, 3, 4, 31, 6)]
        p[f'multiply{bits}'] = (code + [RET], [])
        code = [carry(3, 0, 1, False, True, bits), mrs(4, NZCV), carry(5, 0, 1, True, True, bits), mrs(6, NZCV),
                carry(7, 1, 0, False, False, bits), carry(8, 1, 0, True, False, bits), carry(9, 0, 0, False, True, bits),
                mrs(10, NZCV), carry(11, 31, 1, True, True, bits), mrs(12, NZCV)]
        p[f'carry{bits}'] = (code + [RET], [])
        code = [dp1(3, 0, 0, bits), dp1(4, 1, 1, bits), dp1(5, 0, 2, bits), dp1(6, 1, 4, bits), dp1(7, 0, 5, bits),
                dp1(8, 1, 5, bits), dp1(9, 31, 4, bits), dp1(10, 31, 5, bits)]
        if bits == 64:
            code += [dp1(11, 0, 3), dp1(12, 3, 4)]
        p[f'unary{bits}'] = (code + [RET], [])
        code = [extended(3 + o, o % 2, 1 - o % 2, o, o % 5, sub=o % 2 == 1, flags=o % 3 == 0, bits=bits) for o in range(8)]
        code += [mrs(11, NZCV), extended(12, 31, 0, 2, 2, bits=bits), extended(13, 31, 1, 6, 4, sub=True, flags=True, bits=bits), mrs(14, NZCV)]
        p[f'extended{bits}'] = (code + [RET], [])
        code = [extr(3, 0, 1, 0, bits), extr(4, 0, 1, 1, bits), extr(5, 1, 0, top, bits), extr(6, 0, 0, bits // 2, bits), extr(7, 1, 1, 3, bits)]
        p[f'extract{bits}'] = (code + [RET], [])
    p['sp_extended'] = ([extended(31, 31, 0, 3, 0, sub=True), logical_imm(31, 31, 0xFFFFFFFFFFFFFFF0, 0), add(5, 31, 0), RET], [])
    # Register-offset, sign-extending, literal, LDPSW, non-temporal and prefetch forms.
    code = [logical_imm(3, 0, 0x1F8), movz(4, 7, bits=32),  # and x3,x0,#0x1f8; mov w4,#7
            mem_index(5, 2, 3, 3, 1), mem_index(6, 2, 3, 2, 2), mem_index(7, 2, 3, 0, 2),
            0x12800003 | 0 << 5 | 0x7 << 5,  # movn w3, #7 -> w3 = 0xfffffff8
            mem_index(8, 2, 3, 3, 1, option=6), mem_index(9, 2, 3, 1, 2, option=6), mem_index(10, 2, 3, 2, 2, option=6),
            mem_index(0, 2, 4, 3, 0, option=2, scaled=True), mem_index(1, 2, 4, 1, 0, option=7),
            mem_unscaled(11, 2, -3, 0, 3), mem_unscaled(12, 2, 5, 1, 2), mem_unscaled(13, 2, -7, 2, 2),
            0x39800000 | 9 << 10 | 2 << 5 | 14,      # ldrsb x14, [x2, #9]
            0x79C00000 | 3 << 10 | 2 << 5 | 15,      # ldrsh w15, [x2, #6]
            0xB9800000 | 2 << 10 | 2 << 5 | 16,      # ldrsw x16, [x2, #8]
            0x69400000 | (2 & 0x7F) << 15 | 18 << 10 | 2 << 5 | 17,  # ldpsw x17, x18, [x2, #8]
            0xA8000000 | (4 & 0x7F) << 15 | 1 << 10 | 2 << 5 | 0,    # stnp x0, x1, [x2, #32]
            0xA8400000 | (4 & 0x7F) << 15 | 20 << 10 | 2 << 5 | 19,  # ldnp x19, x20, [x2, #32]
            0xF9800040,                              # prfm pldl1keep, [x2]
            0xF8A26840 | 0x10,                       # prfm (register) [x2, x2]
            ]
    lit = len(code) + 5  # four loads/prefetch, RET, then two data words
    code += [literal(21, len(code), lit, 1), literal(22, len(code) + 1, lit + 1, 0), literal(23, len(code) + 2, lit + 1, 2),
             literal(0, len(code) + 3, lit, 3), RET, 0x89ABCDEF, 0x80000001]
    p['memory_extended'] = (code, [])
    code = [exclusive(5, 2, 3, True), add(5, 5, 1), exclusive(5, 2, 3, False, rs=6), exclusive(5, 2, 3, False, rs=7),
            exclusive(8, 2, 2, True, o0=1), exclusive(0, 2, 2, False, o0=1, rs=9), 0xD503305F,  # clrex
            exclusive(10, 2, 3, True), 0xD503305F, exclusive(1, 2, 3, False, rs=11),
            exclusive(12, 2, 3, True, o0=1, o2=1), exclusive(0, 2, 3, False, o0=1, o2=1),
            exclusive(13, 2, 3, True, pair=True, rt2=14), exclusive(0, 2, 3, False, pair=True, rs=15, rt2=1),
            exclusive(16, 2, 2, True, o0=1, pair=True, rt2=17), exclusive(1, 2, 2, False, o0=1, pair=True, rs=18, rt2=0),
            exclusive(19, 2, 0, True), exclusive(19, 2, 0, False, rs=20), exclusive(21, 2, 1, True, o0=1, o2=1),
            0xD5033BBF, 0xD5033F9F, 0xD5033FDF, 0xD503203F, NOP]  # dmb ish, dsb sy, isb, yield
    p['exclusive'] = (code + [RET], [])
    code = [msr(0, TPIDR_EL0), mrs(5, TPIDR_EL0), mrs(6, TPIDRRO_EL0), msr(1, NZCV), mrs(7, NZCV),
            mrs(8, CTR_EL0), mrs(9, DCZID_EL0), 0xD50B7422, 0xD50B7E22, 0xD50B7522]  # dc zva/civac, ic ivau (x2)
    p['system'] = (code + [RET], [])
    return p


def simd_programs():
    """SIMD and FP programs from simd_programs.py, using pre-verified encodings."""
    from simd_programs import PROGRAMS
    table = json.loads((Path(__file__).resolve().parent / 'simd_encoding.json').read_text(encoding='utf-8'))
    return {name: ([table[line] for line in lines] + [RET], []) for name, lines in PROGRAMS.items()}


def literal_lz4(data):
    """Valid LZ4 block with literals only, independent of the Rust encoder."""
    n = len(data)
    result = bytearray([min(n, 15) << 4])
    if n >= 15:
        n -= 15
        while n >= 255:
            result.append(255)
            n -= 255
        result.append(n)
    result.extend(data)
    return result


def nso(code, compressed=False):
    segments = [code, b'Wanderlust original fixture\0', bytes(16)]
    header = bytearray(0x100)
    header[:4] = b'NSO0'
    struct.pack_into('<I', header, 0xC, 0x38 | (7 if compressed else 0))
    struct.pack_into('<I', header, 0x3C, 32)
    header[0x40:0x60] = hashlib.sha256(b'Wanderlust synthetic fixture').digest()
    output = bytearray()
    for i, segment in enumerate(segments):
        stored = literal_lz4(segment) if compressed else segment
        struct.pack_into('<III', header, 0x10+i*16, 0x100+len(output), i*0x1000, len(segment))
        struct.pack_into('<I', header, 0x60+i*4, len(stored))
        header[0xA0+i*32:0xC0+i*32] = hashlib.sha256(segment).digest()
        output.extend(stored)
    return header+output


def generate(directory):
    directory.mkdir(parents=True, exist_ok=True)
    manifest = []
    for name, (code, seeds) in programs().items():
        data = words(code)
        (directory / (name+'.a64')).write_bytes(data)
        manifest.append({'name': name, 'sha256': hashlib.sha256(data).hexdigest(), 'instructions_in_file': len(code), 'seeds': seeds})
    mixed = words(programs()['mixed'][0])
    for compressed in (False, True):
        (directory / ('mixed-compressed.nso' if compressed else 'mixed.nso')).write_bytes(nso(mixed, compressed))
    (directory / 'manifest.json').write_text(json.dumps(manifest, indent=2)+'\n', encoding='utf-8')
    return manifest


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--out-dir', type=Path, default=Path('generated/fixtures'))
    args = parser.parse_args()
    print(json.dumps({'fixtures': len(generate(args.out_dir)), 'directory': str(args.out_dir)}))


if __name__ == '__main__':
    main()
