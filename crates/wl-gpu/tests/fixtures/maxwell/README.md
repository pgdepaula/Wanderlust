# Synthetic Maxwell fragment binaries

These redistributable test programs contain no game code. Each `.bin` is an
80-byte shader program header followed by 64 bytes independently assembled and
disassembled using MIT-licensed envytools revision
`f102b82381f3f11cee113d16374c87091db039d9` (`envyas`/`envydis`, architecture gm107).
The matching `.asm` files specify every instruction and scheduling word.

```sh
envyas -m gm107 -i -o red.code.bin < red.asm
envydis -m gm107 -i red.code.bin
```

Here `envyas -i` selects binary output, while `envydis -i` selects binary input.
Repeat for `quarters.asm`. Prepend the SPH separately, in little-endian words:

```python
import struct
from pathlib import Path
header = [0] * 20
header[0] = 0x61462  # PS type 2, version 3, shader type 5, SASS version 3
header[4] = 0xff000 # unused store-request range
header[18] = 0xf   # target zero exports RGBA from R0..R3
for name in ("red", "quarters"):
    Path(name + ".bin").write_bytes(
        struct.pack("<20I", *header) + Path(name + ".code.bin").read_bytes())
```

Each scheduler slot encodes stall six, no read/write dependency barrier, no wait
mask and no reuse. The four independent MOV32I writes precede unconditional EXIT;
canonical NOP pads the final bundle. The fixtures were not executed on Maxwell
hardware; their translated fragment output is executed through wgpu.

| Fixture | Expected floating-point RGBA | SHA-256 of 144-byte binary |
|---|---|---|
| red | (1, 0, 0, 1) | `98b1a826d052952c5a4ca78d962b2d314adb43d68a330124eee180449db58894` |
| quarters | (0.25, 0.5, 0.75, 1) | `eb6a8a2d9c4956c5e2e0917c6d1f9f67da8e60cc7693500f2ef04d8181a9b999` |

Encoding references: NVIDIA's public Shader Program Header specification;
[envytools gm107 ISA table](https://github.com/envytools/envytools/blob/f102b82381f3f11cee113d16374c87091db039d9/envydis/gm107.c)
(MIT); devkitPro UAM revision `5a5afc2bae8b55409ab36ba45be63fcb73f68993`,
`source/nv_shader_header.h` and `source/compiler_iface.cpp` (zlib), and its Mesa
`nv50_ir_emit_gm107.cpp` instruction emitter (MIT). No compiler source is bundled.


`ldc.asm` uses four scalar B32 constant reads at c0 byte offsets 0x10..0x1c,
followed by MOV R3,R4 and EXIT. Use the same envyas command and 80-byte header
above. Each LDC assigns write barrier zero; the next load or consumer explicitly
waits for it. Values are supplied by guest memory at runtime, not in the binary.

LDC fixture SHA-256: `6ce81713fa666f853432524c02a2c524e2ef0cfdf67b99b3cc9bbd6ab79b9e48`.
