# Synthetic Maxwell fragment binaries

These redistributable test programs contain no game code. Each `.bin` is an
80-byte shader program header followed by scheduled binary code independently assembled and
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

`tex.asm`/`tex.bin` load the combined handle from c0[0], interpolate generic-zero
x/y through IPA pass and sample TEX.B.LZ 2D RGBA. All three variable-latency
instruction families assign write barrier zero and consumers/reusers wait-one.
Prepend the same header with word six additionally set to 0xf (screen-linear xy).
The test harness supplies the varying; no game code is included.

Assembly uses the development grammar alias described in `docs/graphics/sources.md`:
rename the TEX.B dummy `N("0x0")` to `N("zero")` in the pinned gm107 table.
The original envydis confirms the resulting TEX.B opcode/operands. SHA-256 of the
144-byte fixture: `7297c6d6a48aaa36e12372e3a228e3fdbe9c6141caedab5e1d4f30bb08cf2f70`.


The vertex fixtures use the same unmodified pinned envyas/envydis. `vertex.asm`
loads generic-zero xyzw (0x80) and generic-one xy (0x90) through ALD B128/B64,
exports position xyzw (0x70) and generic-zero xy (0x80) through AST, and exits.
`vertex-swap.asm` adds three register MOVs that exchange clip-space X/Y. Both
assign barrier zero to ALD and wait before consuming/reusing it. AST uses no write
barrier. The VTG SPH is constructed from an otherwise zero 80-byte array:
word zero 0x60461, word four 0xff000, byte 24 0x3f, little-endian uint32 at byte
50 0xf0000000, byte 54 0x03. OmapSystemValuesAB is deliberately not word aligned;
NVIDIA's VTG field widths and UAM's packed layout independently agree. No output
attribute is read back by the shader, so the store-request range stays unused.

vertex.bin: 144 bytes, SHA-256 `ccfba2e1190befe39a174186870752588e828ce981abd73cca6219c9ffd9a776`.
vertex-swap.bin: 176 bytes, SHA-256 `5852b1f783f4a6d76d7cfbb7d3b74767c2225832fc9a1ea63d8c0233e0c0e3b0`.

`alu.asm` and `vertex-alu.asm` each contain three scheduled bundles (96 code
bytes, 176 bytes including SPH). They use the PS header from ldc.bin and VS
header from vertex.bin respectively. Both were independently assembled with
pinned envyas and disassembled using the original envydis. `alu` loads c0[.10],
c0[.14], c0[.18], adds the first two, multiplies the latter two, adds RZ and
squares the third. `vertex-alu` loads real attributes, halves X and adds .5 to Y,
then exports position and UV. These fixtures use exact binary fractions; they
do not establish general IEEE/Maxwell arithmetic equivalence.

`perspective.asm` has three scheduled bundles and a 176-byte SPH+code fixture.
The header adds PositionW (word five 0x80000000) and generic-zero xy Perspective
(word six 0x0a) to the PS header. The program loads its texture handle, reads
PositionW, computes its reciprocal, applies perspective IPA xy and executes TEX.B.LZ.
It was independently assembled with pinned envytools plus the already documented
TEX dummy-token grammar alias, and decoded with unmodified envydis. SHA-256:
`b80062812d98f5b98152e154b5921704d5f7910405f7262504635069c1d5ac52`.
This is synthetic original-format shader evidence, not a commercial game shader.

`integer.asm`/`integer.bin` contain five scalar LDC loads, GPR IADD and LOP
AND/OR/XOR, then EXIT and canonical padding. The 208-byte PS fixture was assembled
and disassembled with unmodified pinned envytools. Constants
`ffffffff 3f000001 00400000 3e800000 3fc00000` at c0[0x10..0x20] yield raw exports
`3f000000 3e800000 3f400000 3f800000`, including real 32-bit addition overflow.
SHA-256 `c5afab1532b72057320de0bbcb87266da1b1150da6949032d5a505ad50ae2f12`.
`vertex-integer` reads attributes, adds `0x00800000` to X bits and XORs Y with
`0x80000000`, then exports position/UV. Its chosen finite normal inputs produce
X*=2 and Y=-Y; these are integer bit operations, not a claimed general float
transform identity. Its 208-byte file includes one trailing canonical NOP bundle;
the selected-program fetch stops at the earlier EXIT bundle. SHA-256
`8f580ea90431a7e879aeee774cb219219a28b6b4202316a5f00e74140cfd47de`.


`shift.asm`/`shift.bin` use two scalar LDC inputs and unsigned GPR SHL, SHL.W,
SHR.U32 and SHR.U32.W. They were assembled and disassembled with the same
unmodified pinned envytools. The 176-byte PS fixture prepends the existing LDC
80-byte SPH. Its runtime count distinguishes clamp-to-zero for counts >=32 from
`.W` low-five-bit wrapping. Explicit `u32` is required: envyas plain `shr` is
signed and is not accepted by this subset. SHA-256:
`06def1ebc5b3d222a824c84be68321f89dc805e1ff2a44b49f44fb4fb5a09c82`.

`vertex-shift.asm`/`vertex-shift.bin` use the existing 80-byte VTG SPH and four
scheduled bundles (208 bytes total). SHL by one followed by unsigned SHR by one
removes the clip-X sign bit; SHL.W and SHR.U32.W by 32 preserve clip-Y. For the
fixture's finite normal positions this yields X=abs(X), Y unchanged; this is a
raw-bit transformation, not a general floating arithmetic identity. Position/UV
are exported by genuine AST. SHA-256:
`66952bdeb0ee615ffa71e91f44c1282bc5b31ab35f6beb52afa4c5d6a7b48183`.


`minmax.asm`/`minmax.bin` load two scalar LDC words and perform unsigned GPR
IMNMX with static selection predicates PT (minimum) and !PT (maximum), in both
operand orders. The outer execution predicate is PT in every instruction.
Unmodified pinned envyas/envydis produced and decoded the 176-byte PS fixture,
using the existing LDC SPH. SHA-256:
`51c0881af54a2135ab4d8addf34ee68c7775a15d704c862418e14db0634d53ea`.
`vertex-minmax.asm`/`vertex-minmax.bin` prepend the existing VTG SPH and use
unsigned MIN of X bits with `0x80000000`, plus unsigned MAX of Y bits with RZ.
For the chosen finite normal inputs this clips negative X to negative zero and
preserves Y, followed by genuine position/UV exports. This raw-bit transformation
is not a general floating MIN/MAX operation. The fixture is 176 bytes; SHA-256:
`64982273037c4b5196f9ade14bf277f1b670cd13f16aa6140efffffe82939baf`.
