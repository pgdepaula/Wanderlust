#!/usr/bin/env python3
"""Build original two-module MOD0 test programs with an independent ELF linker.

Original hand-encoded A64 code is written into ELF64 relocatable objects. LLD links them
as position-independent shared objects, producing the PLT, GOT, `.dynamic`, symbol hash
table and dynamic relocations. The linked ELF files are then repackaged as NSO0 modules
with a MOD0 header, as Switch toolchains do. No game content is involved.

The program's result is fixed by its source semantics, not by the relocation algorithm:
main returns sdk_double(inc(triple(sdk_add_bias(6, 7)))) + (++sdk_counter) = 1681.
"""
import argparse
import hashlib
import json
import os
from pathlib import Path
import shutil
import struct
import subprocess
import sys

sys.path.insert(0, str(Path(__file__).resolve().parent))
from fixtures import add, literal_lz4, logical, movz, regop  # noqa: E402

RET = 0xD65F03C0
EXPECTED_X0 = 1681
# AArch64 ELF relocation codes used in the objects (static relocations).
ABS64, PREL32, ADR_PREL_PG_HI21, ADD_ABS_LO12_NC = 257, 261, 275, 277
JUMP26, CALL26, LDST64_ABS_LO12_NC = 282, 283, 286
ADR_GOT_PAGE, LD64_GOT_LO12_NC = 311, 312
STB_LOCAL, STB_GLOBAL = 0, 1
STT_NOTYPE, STT_OBJECT, STT_FUNC = 0, 1, 2
STV_HIDDEN = 2

LINKER_SCRIPT = """
PHDRS { text PT_LOAD FLAGS(5); rodata PT_LOAD FLAGS(4); data PT_LOAD FLAGS(6); dyn PT_DYNAMIC FLAGS(6); }
SECTIONS {
  . = 0;
  .text : { KEEP(*(.text.crt0)) *(.text .text.*) } :text
  .plt : { *(.plt) } :text
  . = ALIGN(0x1000);
  .rodata : { *(.rodata .rodata.*) KEEP(*(.mod0)) } :rodata
  .note.gnu.build-id : { *(.note.gnu.build-id) } :rodata
  .hash : { *(.hash) } :rodata
  .dynsym : { *(.dynsym) } :rodata
  .dynstr : { *(.dynstr) } :rodata
  .rela.dyn : { *(.rela.dyn) *(.rel.dyn) } :rodata
  .rela.plt : { *(.rela.plt) *(.rel.plt) } :rodata
  . = ALIGN(0x1000);
  .dynamic : { *(.dynamic) } :data :dyn
  .got : { *(.got) } :data
  .got.plt : { *(.got.plt) } :data
  .data : { *(.data .data.* .data.rel.ro .data.rel.ro.*) } :data
  __bss_start__ = .;
  .bss : { *(.bss .bss.*) *(COMMON) . = ALIGN(8); } :data
  __bss_end__ = .;
  /DISCARD/ : { *(.comment) *(.eh_frame*) *(.note.GNU-stack) }
}
"""


def adrp(rd):
    return 0x90000000 | rd


def ldr64(rt, rn, imm=0):
    return 0xF9400000 | (imm // 8) << 10 | rn << 5 | rt


def str64(rt, rn, imm=0):
    return 0xF9000000 | (imm // 8) << 10 | rn << 5 | rt


def blr(rn):
    return 0xD63F0000 | rn << 5


class Object:
    """Minimal ELF64 little-endian AArch64 relocatable object writer."""

    def __init__(self):
        self.sections = []  # (name, type, flags, data, align)
        self.symbols = []   # (name, bind, type, section_name or None, value, size, other)
        self.relocations = {}  # section name -> [(offset, symbol name, type, addend)]

    def section(self, name, data, flags, kind=1, align=8):
        self.sections.append((name, kind, flags, bytes(data), align))

    def symbol(self, name, section=None, value=0, bind=STB_LOCAL, kind=STT_NOTYPE, size=0, other=0):
        self.symbols.append((name, bind, kind, section, value, size, other))

    def reloc(self, section, offset, symbol, kind, addend=0):
        self.relocations.setdefault(section, []).append((offset, symbol, kind, addend))

    def build(self):
        names = [s[0] for s in self.sections]
        # ELF requires local symbols before globals; index 0 is the null symbol.
        ordered = sorted(self.symbols, key=lambda s: s[1] != STB_LOCAL)
        first_global = 1 + sum(1 for s in ordered if s[1] == STB_LOCAL)
        strtab = bytearray(b'\0')
        symtab = bytearray(24)
        index = {}
        for i, (name, bind, kind, section, value, size, other) in enumerate(ordered, 1):
            index[name] = i
            offset = len(strtab)
            strtab += name.encode() + b'\0'
            shndx = 0 if section is None else 1 + names.index(section)
            symtab += struct.pack('<IBBHQQ', offset, bind << 4 | kind, other, shndx, value, size)
        all_sections = list(self.sections)
        for target, entries in self.relocations.items():
            data = b''.join(struct.pack('<QQq', off, index[sym] << 32 | kind, addend) for off, sym, kind, addend in entries)
            all_sections.append(('.rela' + target, 4, 0x40, data, 8))
        symtab_index = 1 + len(all_sections)
        all_sections += [('.symtab', 2, 0, bytes(symtab), 8), ('.strtab', 3, 0, bytes(strtab), 1)]
        shstrtab = bytearray(b'\0')
        name_offsets = []
        for s in all_sections + [('.shstrtab',)]:
            name_offsets.append(len(shstrtab))
            shstrtab += s[0].encode() + b'\0'
        all_sections.append(('.shstrtab', 3, 0, bytes(shstrtab), 1))
        body = bytearray(64)
        headers = [bytes(64)]
        for i, (name, kind, flags, data, align) in enumerate(all_sections):
            while len(body) % align:
                body.append(0)
            link = info = entsize = 0
            if kind == 4:
                link, info, entsize = symtab_index, 1 + names.index(name[5:]), 24
            elif kind == 2:
                link, info, entsize = symtab_index + 1, first_global, 24
            size = len(data)
            offset = len(body)
            if kind != 8:
                body += data
            headers.append(struct.pack('<IIQQQQIIQQ', name_offsets[i], kind, flags, 0, offset, size, link, info, align, entsize))
        while len(body) % 8:
            body.append(0)
        shoff = len(body)
        ident = b'\x7fELF\x02\x01\x01' + bytes(9)
        body[:64] = ident + struct.pack('<HHIQQQIHHHHHH', 1, 183, 1, 0, 0, shoff, 0, 64, 0, 0, 64, len(headers), len(headers) - 1)
        return bytes(body) + b''.join(headers)


def words(code):
    return struct.pack('<' + 'I' * len(code), *code)


def crt0_and_mod0(o, first_word):
    """Text word 0 and the MOD0 offset word, followed by a 0x1C-byte MOD0 header in rodata."""
    o.reloc('.text.crt0', 4, '__nx_mod0', PREL32, 4)
    o.section('.text.crt0', words([first_word, 0]), 6, align=4)
    o.section('.mod0', b'MOD0' + bytes(24), 2, align=4)
    o.symbol('__nx_mod0', '.mod0')
    for field, (symbol, addend) in enumerate([('_DYNAMIC', 0), ('__bss_start__', 0), ('__bss_end__', 0), ('__nx_mod0', 0), ('__nx_mod0', 0), ('__nx_module_runtime', 0)], 1):
        # Each field is (target - MOD0); PREL32 at MOD0+4n computes S + A - P, so A = 4n.
        o.reloc('.mod0', field * 4, symbol, PREL32, addend + field * 4)
    for name in ('_DYNAMIC', '__bss_start__', '__bss_end__'):
        o.symbol(name, None, bind=STB_GLOBAL, other=STV_HIDDEN)
    o.section('.bss', bytes(0xD0), 3, kind=8)
    o.symbol('__nx_module_runtime', '.bss', size=0xD0)


def sdk_object():
    o = Object()
    crt0_and_mod0(o, RET)
    text = [adrp(8), ldr64(8, 8), ldr64(8, 8), regop(0, 0, 1), regop(0, 0, 8), RET,
            regop(0, 0, 0), RET]
    o.section('.text', words(text), 6, align=4)
    o.reloc('.text', 0, 'sdk_bias_ptr', ADR_PREL_PG_HI21)
    o.reloc('.text', 4, 'sdk_bias_ptr', LDST64_ABS_LO12_NC)
    o.symbol('sdk_add_bias', '.text', 0, STB_GLOBAL, STT_FUNC, 24)
    o.symbol('sdk_double', '.text', 24, STB_GLOBAL, STT_FUNC, 8)
    o.section('.data', struct.pack('<QQQ', 0, 100, 1000), 3)
    o.reloc('.data', 0, 'sdk_bias', ABS64)  # local target: LLD emits R_AARCH64_RELATIVE
    o.symbol('sdk_bias_ptr', '.data', 0, size=8)
    o.symbol('sdk_bias', '.data', 8, size=8, kind=STT_OBJECT)
    o.symbol('sdk_counter', '.data', 16, STB_GLOBAL, STT_OBJECT, 8)
    return o


def main_object():
    o = Object()
    crt0_and_mod0(o, 0x14000000)
    o.reloc('.text.crt0', 0, 'entry', JUMP26)
    text = [
        0xA9BE7BFD, add(29, 31, 0), 0xA90153F3,           # stp x29,x30,[sp,#-32]!; mov x29,sp; stp x19,x20,[sp,#16]
        movz(0, 6), movz(1, 7), 0x94000000,                # sdk_add_bias(6, 7) through the PLT
        adrp(20), add(20, 20, 0),                          # x20 = &fn_table
        ldr64(16, 20, 0), blr(16), ldr64(16, 20, 8), blr(16),
        adrp(16), ldr64(16, 16), blr(16),                  # (*imported_fn_ptr)(x0) = sdk_double
        logical(19, 31, 0),                                # mov x19, x0
        adrp(8), ldr64(8, 8), ldr64(9, 8), add(9, 9, 1), str64(9, 8),  # ++*GOT(sdk_counter)
        regop(0, 19, 9),
        0xA94153F3, 0xA8C27BFD, RET,                       # ldp x19,x20; ldp x29,x30,[sp],#32
        regop(1, 0, 0), regop(0, 1, 0), RET,               # local_triple
        add(0, 0, 1), RET,                                 # local_inc
    ]
    o.section('.text', words(text), 6, align=4)
    for at, symbol, kind in [(20, 'sdk_add_bias', CALL26), (24, 'fn_table', ADR_PREL_PG_HI21), (28, 'fn_table', ADD_ABS_LO12_NC),
                             (48, 'imported_fn_ptr', ADR_PREL_PG_HI21), (52, 'imported_fn_ptr', LDST64_ABS_LO12_NC),
                             (64, 'sdk_counter', ADR_GOT_PAGE), (68, 'sdk_counter', LD64_GOT_LO12_NC)]:
        o.reloc('.text', at, symbol, kind)
    o.symbol('entry', '.text', 0, kind=STT_FUNC)
    o.symbol('local_triple', '.text', 25 * 4, kind=STT_FUNC)
    o.symbol('local_inc', '.text', 28 * 4, kind=STT_FUNC)
    o.section('.data', bytes(24), 3)
    o.reloc('.data', 0, 'local_triple', ABS64)
    o.reloc('.data', 8, 'local_inc', ABS64)
    o.reloc('.data', 16, 'sdk_double', ABS64)  # preemptible import: dynamic R_AARCH64_ABS64
    o.symbol('fn_table', '.data', 0, size=16)
    o.symbol('imported_fn_ptr', '.data', 16, size=8)
    for name in ('sdk_add_bias', 'sdk_double', 'sdk_counter'):
        o.symbol(name, None, bind=STB_GLOBAL)
    return o


def find_lld():
    explicit = os.environ.get('WL_LLD')
    if explicit:
        return [shutil.which(explicit) or explicit]
    located = shutil.which('ld.lld')
    if located:
        # Windows CreateProcess does not append .exe to a name already ending
        # in .lld. Preserve the exact executable that which actually found.
        return [located]
    sysroot = subprocess.run(['rustc', '--print', 'sysroot'], check=True, capture_output=True, text=True).stdout.strip()
    version = subprocess.run(['rustc', '-vV'], check=True, capture_output=True, text=True).stdout
    host = next((line.removeprefix('host: ').strip() for line in version.splitlines() if line.startswith('host: ')), None)
    if host:
        path = Path(sysroot, 'lib', 'rustlib', host, 'bin', 'rust-lld.exe' if sys.platform == 'win32' else 'rust-lld')
        if path.is_file():
            return [str(path), '-flavor', 'gnu']
    raise SystemExit('no LLD found: install lld or a rustup toolchain with rust-lld, or set WL_LLD')


def elf_sections(elf):
    shoff, = struct.unpack_from('<Q', elf, 0x28)
    shnum, shstrndx = struct.unpack_from('<HH', elf, 0x3C)
    headers = [struct.unpack_from('<IIQQQQIIQQ', elf, shoff + 64 * i) for i in range(shnum)]
    names = headers[shstrndx][4]
    result = {}
    for h in headers:
        name = elf[names + h[0]:elf.index(b'\0', names + h[0])].decode()
        result[name] = h
    return result


def elf_symbols(elf):
    sections = elf_sections(elf)
    symtab, strtab = sections['.symtab'], sections['.strtab']
    result = {}
    for i in range(symtab[5] // 24):
        name, info, other, shndx, value, size = struct.unpack_from('<IBBHQQ', elf, symtab[4] + 24 * i)
        if name:
            result[elf[strtab[4] + name:elf.index(b'\0', strtab[4] + name)].decode()] = value
    return result


def elf_to_nso(elf):
    """Repackage LLD's three PT_LOAD segments as an NSO0 with compressed, hashed segments."""
    phoff, = struct.unpack_from('<Q', elf, 0x20)
    phnum, = struct.unpack_from('<H', elf, 0x38)
    loads = []
    for i in range(phnum):
        kind, flags, offset, vaddr, _, filesz, memsz, _ = struct.unpack_from('<IIQQQQQQ', elf, phoff + 56 * i)
        if kind == 1:
            loads.append((flags, offset, vaddr, filesz, memsz))
    assert [l[0] for l in loads] == [5, 4, 6], 'expected text, rodata and data PT_LOAD segments'
    assert loads[0][2] == 0, 'text must be linked at address zero'
    sections = elf_sections(elf)
    note = sections['.note.gnu.build-id']
    build_id = elf[note[4] + 16:note[4] + note[5]]
    header = bytearray(0x100)
    header[:4] = b'NSO0'
    struct.pack_into('<I', header, 0xC, 0x3F)
    rodata_vaddr = loads[1][2]
    for at, name in ((0x90, '.dynstr'), (0x98, '.dynsym')):
        struct.pack_into('<II', header, at, sections[name][3] - rodata_vaddr, sections[name][5])
    header[0x40:0x40 + len(build_id)] = build_id
    payload = bytearray()
    for i, (_, offset, vaddr, filesz, memsz) in enumerate(loads):
        segment = elf[offset:offset + filesz]
        stored = literal_lz4(segment)
        struct.pack_into('<III', header, 0x10 + 16 * i, 0x100 + len(payload), vaddr, len(segment))
        struct.pack_into('<I', header, 0x60 + 4 * i, len(stored))
        header[0xA0 + 32 * i:0xC0 + 32 * i] = hashlib.sha256(segment).digest()
        payload += stored
    struct.pack_into('<I', header, 0x3C, loads[2][4] - loads[2][3])
    return bytes(header + payload), build_id.hex()


def build(directory, relocation_format='rela'):
    directory.mkdir(parents=True, exist_ok=True)
    work = directory / 'elf'
    work.mkdir(exist_ok=True)
    (work / 'module.ld').write_text(LINKER_SCRIPT, encoding='utf-8')
    lld = find_lld()
    common = ['-m', 'aarch64linux', '-shared', '--hash-style=sysv', '--build-id=sha1', '-z', 'norelro',
              '-z', 'max-page-size=0x1000', '-z', 'nopack-relative-relocs', '-z', relocation_format,
              '-T', str(work / 'module.ld')]
    manifest = {'expected_x0': EXPECTED_X0, 'relocation_format': relocation_format, 'modules': {}}
    for name, obj, extra in (('sdk', sdk_object(), ['-soname', 'sdk']), ('main', main_object(), [str(work / 'sdk.elf')])):
        (work / f'{name}.o').write_bytes(obj.build())
        subprocess.run(lld + common + [str(work / f'{name}.o'), '-o', str(work / f'{name}.elf')] + extra, check=True)
        elf = (work / f'{name}.elf').read_bytes()
        nso, build_id = elf_to_nso(elf)
        (directory / name).write_bytes(nso)
        symbols = elf_symbols(elf)
        manifest['modules'][name] = {
            'sha256': hashlib.sha256(nso).hexdigest(), 'build_id': build_id,
            'symbols': {k: v for k, v in sorted(symbols.items()) if not k.startswith('$')},
        }
    (directory / 'manifest.json').write_text(json.dumps(manifest, indent=2) + '\n', encoding='utf-8')
    return manifest


RUST_TARGET = 'aarch64-unknown-none'


def build_rust(source, directory):
    """Compile a no_std Rust guest exporting `wl_entry` for AArch64 (Cortex-A57 tuning) with
    LLVM, and link it as a single MOD0 module `main` whose text begins with `b wl_entry`."""
    directory.mkdir(parents=True, exist_ok=True)
    work = directory / 'elf'
    work.mkdir(exist_ok=True)
    (work / 'module.ld').write_text(LINKER_SCRIPT, encoding='utf-8')
    library = work / 'libguest.a'
    subprocess.run(['rustc', '--edition', '2024', '--target', RUST_TARGET, '--crate-type', 'staticlib',
                    '-C', 'opt-level=3', '-C', 'panic=abort', '-C', 'relocation-model=pic',
                    '-C', 'target-cpu=cortex-a57', str(source), '-o', str(library)], check=True)
    crt0 = Object()
    crt0_and_mod0(crt0, 0x14000000)
    crt0.reloc('.text.crt0', 0, 'wl_entry', JUMP26)
    crt0.symbol('wl_entry', None, bind=STB_GLOBAL, other=STV_HIDDEN)
    (work / 'crt0.o').write_bytes(crt0.build())
    subprocess.run(find_lld() + ['-m', 'aarch64linux', '-shared', '--hash-style=sysv', '--build-id=sha1',
                                 '-z', 'norelro', '-z', 'max-page-size=0x1000', '-z', 'nopack-relative-relocs',
                                 '--gc-sections', '-T', str(work / 'module.ld'), str(work / 'crt0.o'),
                                 str(library), '-o', str(work / 'main.elf')], check=True)
    nso, build_id = elf_to_nso((work / 'main.elf').read_bytes())
    (directory / 'main').write_bytes(nso)
    manifest = {'source': Path(source).name, 'target': RUST_TARGET, 'build_id': build_id,
                'sha256': hashlib.sha256(nso).hexdigest()}
    (directory / 'manifest.json').write_text(json.dumps(manifest, indent=2) + '\n', encoding='utf-8')
    return manifest


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--out-dir', type=Path, default=Path('generated/fixtures/modules'))
    parser.add_argument('--rust', type=Path, help='build this no_std Rust guest as <out-dir>/main instead')
    args = parser.parse_args()
    if args.rust:
        print(json.dumps(build_rust(args.rust, args.out_dir)))
        return
    for variant in ('rela', 'rel'):
        build(args.out_dir / variant, variant)
    print(json.dumps({'directory': str(args.out_dir), 'variants': ['rela', 'rel']}))


if __name__ == '__main__':
    main()
