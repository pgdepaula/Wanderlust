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


`predicate.asm`/`predicate.bin` load four scalar LDC words, compare unsigned
values into P0 and select either color with P0/!P0. A subsequent equality compare
rewrites P0 before another selection. Only AND PT with complementary output
discarded to PT is used; outer execution remains PT. Predicate producers use
conservative stall 13, other instructions stall six. The 208-byte PS fixture
retains the existing LDC SPH and was assembled/disassembled with unmodified pinned
envytools. SHA-256:
`757e2351c6cf63fb6eb052b18a7beab1c5cbe57e9b839b4e705b93f5707403b8`.
`vertex-predicate.asm`/`vertex-predicate.bin` use unsigned comparison of clip-X
bits with `0x80000000` and select X or RZ through P0, followed by genuine position
and UV exports. For the finite normal fixture inputs, negative X becomes zero
and other components are preserved. The 176-byte VTG fixture uses the existing
vertex SPH. SHA-256:
`83fbaa26cd8b5e71b6ba3bd3310ce9c550557c8906b0e390419d5ebc0b7b4154`.


`ldc64.asm` uses two independently assembled LDC.B64 instructions, loading
R0/R1 from c0[0x10]/c0[0x14] and R2/R3 from c0[0x18]/c0[0x1c]. One write barrier
covers each complete pair. The same PS header as ldc.bin precedes 32 code bytes.
Unmodified pinned envyas/envydis assemble and disassemble this fixture.
112-byte SHA-256: `75b133d0ab6537c16e72ebf6f96c32b484e4572c86ccd0b92230e521b0e7aeb2`.

`tex-ldc64.asm` replaces the scalar handle load with LDC.B64 R6,c0[0] and samples
using its upper word R7. Its 144-byte binary uses the existing screen-linear PS
header and documented TEX dummy-token grammar alias; unmodified envydis confirms
all operands. The runtime test places an invalid handle in the lower word, so
using the wrong lane cannot accidentally sample a valid default descriptor.
SHA-256: `452733ff473615459aad93d19cff1ef5e797ce8f63043bd35c3988b98a8a43aa`.


`signed-minmax.asm` is independently assembled with unmodified pinned envytools.
The 304-byte PS loads two runtime words through LDC.B64 and evaluates signed min/max,
including RZ. It maps each selected sign bit to exactly 0.5 or 1.0 using supported
integer shifts/OR, avoiding NaN/subnormal floating exports for integer extrema.
GPU output observes sign selection, not every bit of each selected payload.
SHA-256: `cc0dca6d65a971fcb793de5beb65fd1b65538888d1fb0fb37f811c19c91308fb`.

`vertex-signed-minmax.asm` uses signed min(X,RZ) and max(Y,RZ) on input bits, then
exports position and UV. For the finite coordinate fixture this clamps X above
zero and Y below zero; it is not a general floating min/max implementation.
The 176-byte VS uses the existing vertex SPH.
SHA-256: `a432fcaab574f0d6e95a5974c6e9b5c1ca623755ab4d3cc13ca0427899aa3001`.


The following IADD fixtures use original unmodified pinned envyas/envydis and the
existing exact PS/VS headers. No game content is included.

| Fixture | Bytes | SHA-256 |
|---|---:|---|
| iadd-bytes.bin (A−B) | 464 | `52a32d2dc8a06ac7b17dc8b8e6e4fae0d1edbd11fdbaa5fd87003c4d8ec629c1` |
| iadd-bytes-left.bin (B−A) | 464 | `8cb20ef54416cdc4b106b71566fde160a14b2d61b2083ff694ff449ac788344b` |
| iadd-bytes-po-negative.bin (reject) | 464 | `7253e0d2c4e3fe9dc925ba77ccf59f6ca97fb2c29c399f5c031d3b6652e6c1dd` |
| vertex-iadd.bin | 176 | `ed6d5383bd5bf3ff65a4df27f9219ff0d858e3d0aeba617f9760d14f5073b881` |

Each PS has 36 instructions and 13 GPRs. B64 loads R4/R5; IADD writes R9 at full
PC 0x98. The byte observer exports the low-to-high result bytes as RGBA using
unsigned shifts/masks and finite float arithmetic. Both-negation spelling in the
assembler produces the PO negative fixture; it is never executed as subtraction.
The VS has 7 GPRs and uses both legal negation forms to modify X/Y raw bits before
position/UV export. Half-coordinate expectations apply only to the explicitly
specified normal finite input geometry.


`float-immediate` and `vertex-float-immediate` use original unmodified envyas/
envydis at the pinned revision. Each contains the existing validated80-byte
PS/VS header followed by96bytes of independently assembled scheduled code.
The fragment loads c0[0x10/0x14/0x18], adds raw3e000000 to R4, multiplies R5
by3f000000, adds rawbe800000 to R6 and exports alpha1. The vertex multiplies
clip X by.5, adds.5 to clip Y and exports position/UV through the original
ALD/AST interface. All immediate tokens in assembly are raw IEEE hex.
SHA-256 including SPH:

- float-immediate.bin:04c46334c384e3673bad525872a5ba2dfc9f32600f27546e5abf059055a8321b
- vertex-float-immediate.bin:d1936c74a78df843dc8db125794f10fc5ec4d0a355013f4e290086e936b5e02e

Positive-zero opcode mutations in tests are explicit field tests, not claimed
assembler output. Exceptional payload assembly/decoding does not prove exact
portable exceptional arithmetic. Fixtures contain no commercial game content.


`float32-immediate` retains the validated PS header and160bytes of original
unmodified envyas code. Four scalar LDC loads supply two runtime operands and
two expected raw words. FADD32I/FMUL32I use literal3f800001; ISETP.EQ and SEL
export their bit-equality results as red/green, with blue0 and alpha1. Expected
words are test data, not translator specialization. The source explicitly
uses stall13 for predicate production and the existing wait/barrier contract.
`vertex-float32-immediate` has96code bytes and scales/offsets positions with
literal3f000001. Low-bit fidelity is established by the FS oracle; VS coverage
proves the transformation, not resolution of that tiny mantissa difference.
SHA-256 including80-byte SPH:

- float32-immediate.bin:88472bdac976fb58f5144a9284ebb74e027af272425ee0f6a418e050fb343676
- vertex-float32-immediate.bin:58f41ea86fd831e1c36a52ef501436384276d41899e09adea7d674f5445f90f2

Both code streams were independently reassembled with the original pinned
assembler and disassembled by unmodified envydis. No commercial content is used.

`conditional-move` retains the validated PS header and128bytes of original
unmodified envyas code. Two scalar LDC operands feed unsigned ISETP.LT into P0.
Initialized RGBA values .25/.5/.75/1 become1/.5/.75/1 when true and .25/.5/0/1
when false, by P0/!P0 guarded MOVs. Neither operation may overwrite its
destination when its predicate is false. `vertex-conditional-move` has96code
bytes; raw equality of each guest vertex X to bf4ccccd selects whether X becomes
.5. Other vertices retain their guest X. Both sources use explicit stall13 for
ISETP and the existing scheduled wait/barrier subset.
SHA-256 including80-byte SPH:

- conditional-move.bin:dfa9e0ba0f91a88505747b5bec86b72886dc5845781bed8e4df1530eb8d60298
- vertex-conditional-move.bin:2db8619e3b21e456e5cea295d098d0b91aa92e48b53ec854995ebec5a60254c3

These are independently assembled synthetic programs, without commercial data.

`float-predicate` retains the validated PS header and 352 bytes of original
envyas code. Two LDC operands feed all six ordered FSETP comparisons. SEL and
exact dyadic FADD accumulate each answer with weights 1/64 through 1/2 into
red. LT, equal, GT and NaN pairs independently produce red 84, 151, 223 and 0
after UNORM quantization; green is 64, blue 191 and alpha 255. Expected results
are not supplied to the guest shader. Signed-zero, subnormal, infinity and NaN
inputs remain raw constant-buffer data. Each predicate producer uses stall 13.
`vertex-float-predicate` has 96 code bytes: X < -.5 conditionally replaces X
with .5; changing only guest X changes the predicate and rasterized geometry.
Both code sections exactly reassemble with the unmodified pinned tools.
SHA-256 including the 80-byte SPH:

- float-predicate.bin:712221ead79ceb859e02b211332f89529aa133abdba64c6d3a79d5ef63aac5e3
- vertex-float-predicate.bin:5d7cec8cd3c0c90de3e5a8f4babef487aa248a7c3d357c988aa328c9bfa079ca

These fixtures contain no commercial shader code or assets.

`float-predicate-all` uses the same validated 80-byte PS header and 768 bytes
assembled by original pinned envyas. Two runtime LDC words feed all sixteen
GPR FSETP modes in encoding order: F, LT/EQ/LE/GT/NE/GE, NUM/NAN,
LTU/EQU/LEU/GTU/NEU/GEU, T. SEL and exact dyadic FADD pack each four answers
into one RGBA channel using weights 1/16,1/8,1/4,1/2. Predicate producers use
stall 13. Numeric less/equal/greater and either-NaN inputs have independent
truth masks AAAA/CCCC/F0F0/FF00, producing respectively
`[159,159,159,159]`, `[191,191,191,191]`, `[0,239,0,239]`, `[0,0,239,239]`.
Changing only the first F to T changes less-case red to 175. No expected result
is embedded in guest constants or generated by the production comparison code.
SHA-256 including SPH:

- float-predicate-all.bin:8e386aca1bf0c20b10a07b780a9a01aa518cb09692b7bbeed48a4160bc6767b3

The VS NAN coverage test changes the independently assembled vertex predicate
mode to verified NAN and changes only its MOV32I comparison constant. NaN never
reaches clip-space outputs: a true predicate moves all X coordinates to finite
.5, producing a degenerate primitive; numeric constants retain the original
asymmetric triangle. These remain synthetic hardware programs.

`bitfield` retains the validated PS header with 256 independently assembled code
bytes. Five LDC words provide a runtime source and four independent scalar
reference results. Unsigned/signed offset5-width11, unsigned full-width and
signed offset31-width1 BFE feed ISETP.EQ/SEL, exporting `[64,128,191,255]` when
all answers match. Deliberately changing the first reference produces red zero.
Changing the first BFE to offset31-width0 tests assembled zero-width semantics;
enabling BREV remains an explicit unsupported-form fault. Each BFE uses the
ordinary six-cycle schedule; predicate production retains thirteen cycles.

`vertex-bitfield` has 64 independently assembled code bytes. BFE clears only
the X sign bit with offset0-width31, moving the guest asymmetric triangle into
positive X. Independent analytic coverage uses the absolute value of guest X.
Changing only the first guest X changes coverage without recompiling either stage.
Both fixtures exactly reassemble with original pinned envyas and retain the
previously validated stage headers. SHA-256 including SPH:

- bitfield.bin:4b38e74391e293370da7051835f5b125dc6c4a931148ee550eb957e18147ffc5
- vertex-bitfield.bin:6b2c7c3dfc8a368141200a3766bf5946a56ba3175b1e16312ea36bd3612bb90b

These are synthetic hardware programs without proprietary shader contents.

`ternary` uses the validated PS header and 160 code bytes assembled with original
pinned envyas. Four LDC words provide A/B/C and an independent expected word.
LOP3 majority LUT E8 writes R4, deliberately aliasing A. ISETP.EQ compares raw
bits, then another LOP3 with LUT00 discards its predicate output to PT and must
preserve P0 before SEL exports red .25 on success. Green .5, blue .75 and alpha
1 are fixed. The first LOP3 is at PC0x88; ISETP is PC0x98 and the P0 preservation
instruction PC0xa0. Changing only LUT bits35:28 exercises every table with
F0F0F0F0/CCCCCCCC/AAAAAAAA operands and expected `LUT * 01010101`.

`vertex-ternary` has 96 code bytes: MOV32I supplies mask7FFFFFFF and LOP3 LUT C0
computes Xbits & mask with a destination/source alias, then ordinary AST exports
position and UV. Both programs retain validated stage headers. Their LOP3
encodings also match the separately pinned original MaxAs assembler.
SHA-256 including SPH:

- ternary.bin:6da7d78140e0c8813f4dff98b40d48a28a06caad391e87cc5c803fbe15816d1c
- vertex-ternary.bin:f49b1e55b1605acdf8fd9e025ffcfe8848b1b6d9197b08e96c6dd893e18933c4

These contain only independently assembled synthetic shader instructions.

`popcount` and `vertex-popcount` retain the validated stage headers, each with
128 code bytes assembled by original pinned envyas. The fragment program loads
a runtime source and independently expected count through LDC, performs POPC
R4,R4 at PC0x68, then compares after explicit wait1 at PC0x78. Success exports
[64,128,191,255]. Incorrect reference data makes red zero.

The vertex program counts raw guest position-X bits at PC0x78 into R6 using
barrier zero. An ISETP after wait1 compares against 19; only matching vertices
move to finite X=.5 before ordinary position/UV exports. Initial X=-.8 has
19 set bits; changing only that guest X to -.6 gives 18 and changes analytic
triangle coverage without shader recompilation. No NaN clip positions are used.
Ordinary POPC words independently match pinned MaxAs; inversion/RZ variants
also have independently assembled GM107 evidence. SHA-256 including SPH:

- popcount.bin:f5719b4ac6288e6b35480dbe79ac6b8101fc90b124f10ee12619b239bfa4dc86
- vertex-popcount.bin:bee5997189994b80fa718fc8406b2de9ac01ef6314fa36cc4a7327cb03c5b5e4

These contain independently assembled synthetic instructions only.

`leading-bit`, `leading-bit-signed` and `vertex-leading-bit` each retain a
validated stage header plus 128 original-envyas-assembled code bytes. The two
fragment variants differ only in FLO signed bit 48. Both load raw source and
expected result, alias R4 with FLO at PC0x68, explicitly wait before ISETP,
and export [64,128,191,255] on success. Sentinel FFFFFFFF is compared as raw
u32, avoiding any reliance on a host NaN color output.

The vertex program uses signed FLO at PC0x78 and compares against 29 after
wait1. Only initial guest X=+.6 matches and moves to finite +.5; negative X
values have highest non-sign bit30. Changing the second guest X to-.6 disables
that move and changes asymmetric analytic coverage. Ordinary AST exports
position/UV; no host geometry substitute is involved. SHA-256 including SPH:

- leading-bit.bin:e8897606e7d10309589d8ff4afdc56e2c4d099c26c6c4ec17b2e8e21870b8652
- leading-bit-signed.bin:383d9bacceb97265425b33547020c1bbdfde181b3ac4a68784fd39e0d325b107
- vertex-leading-bit.bin:f92917ec3b2a6db3ecbf9cd39dfdbcf90dd6805a5f13bdebb180c5ca5ff2f73a

These are independently assembled synthetic programs. SH/inversion/CC remain
unsupported, rather than inferred from the ordinary instruction forms.

`int-to-float`, `int-to-float-signed` and `vertex-int-to-float` retain the validated
stage headers, each with 128 code bytes assembled by the original pinned envyas.
Unsigned/signed fragment variants differ only in I2F signed-source bit13. They
load source and independently expected binary32 bits, alias R4 at PC0x68, wait1
before ISETP and export [64,128,191,255] on equality. Incorrect reference bits
make red zero. Conversion words also independently match pinned MaxAs.

The signed vertex conversion at PC0x78 interprets raw guest position-X bits as
an integer for a predicate test. It compares the converted encoding against
CE816666; only initial X=-.8 matches and moves to finite X=.5. Changing that
vertex's guest X to-.6 gives CE81CCCD and disables the move. Converted large
values are never exported as clip positions; normal AST exports position/UV.
SHA-256 including SPH:

- int-to-float.bin:3c912a83e78d166b4f6d3425ef765c5a0093ab697e62299f07778ce2ac1f8fb1
- int-to-float-signed.bin:0e208255696f18d2305244d1a580a7c254e6b33ec0dd5d8cc1757e54314d308c
- vertex-int-to-float.bin:e94e5ad765ffab19a6d942974143e9f69f423264b7c379b5f64a6fdaf6361d55

Only synthetic independently assembled instructions are included. Other type
widths and conversion modes are not implied by these fixtures.

`float-set`, `float-set-float` and `vertex-float-set` each have a validated
80-byte stage header plus128 original-envyas-assembled code bytes. The fragment
variants load two raw operands and expected result through LDC, execute FSET.LT
R4,R4,R5 at PC0x78, then compare raw bits with ISETP. A second FSET.F into R7
must preserve the already established P0 before MOV32I/SEL export red .25 on
success. Green .5, blue .75 and alpha1 are fixed. Both mask and BF variants are
retained; the main condition/result bits can be changed independently of the
P0 preservation instruction.

The vertex fixture uses FSET.BF.LT R6,R0,R7 at PC0x78 with R7=-.5. An integer
comparison against1.0 bits controls a finite X=.5 move. Only initial X=-.8
matches; mutating that guest X to-.4 disables the move. Changing the binary's
BF bit and the expected immediate to FFFFFFFF verifies equivalent mask-result
linkage, without exporting a NaN mask as a clip position. SHA-256 including SPH:

- float-set.bin:690f13cfd245d6e9f95b1699c4cb7785df7c0779b0c4063a13eb0eb9c736a07a
- float-set-float.bin:9a42b2fb9385f766c33be938a7833cd8a0efb8ae7d2f8a6767defca2ea0026e5
- vertex-float-set.bin:d39b7653dae14bd90f1d78275c5a204487ede2f9601dd0f493cee1b8ec1072bd

These are independently assembled synthetic instructions only. FSET modifiers,
FTZ and non-PT predicate combiners are outside this fixture's supported subset.

`scaled-add` and `vertex-scaled-add` retain the validated 80-byte stage
headers plus128 original-envyas-assembled code bytes each. Fragment ISCADD at
PC0x78 aliases R4 with `(R4 << 3) + R5`, wrapping to32 bits, then ISETP checks
an independently supplied expected word. A second ISCADD at PC0x88 uses RZ/RZ
with shift31 and must preserve P0 before SEL exports red .25. Green .5, blue
.75 and alpha1 are fixed. Both sources and expected bits come from guest LDC.

The vertex fixture shifts raw guest X bits once, adds RZ and compares against
7E99999A. Only initial X=-.8 matches and moves to finite+.5. Mutating that
vertex to-.6 disables the move. Changing binary shift1 to0 and its expected
MOV32I word to raw-.6 restores finite+.5 without changing vertex data. The raw
arithmetic is never exported as a clip position. SHA-256 including SPH:

- scaled-add.bin:324c0e3ce2dd900ab14760fb1213f8ca129dddf5edf8f85be3ad684afd25b2b5
- vertex-scaled-add.bin:5902847960d817d7423b5108f8304ba8a403df7593b770871f2a9268dafc7c3f

Only synthetic independently assembled instructions are included. The fixture
supports the verified five-bit shift field; permissive assembler grammar for
larger immediates is not evidence for reserved bits or instruction modifiers.

`integer-widen-u8`, `integer-widen-u16`, `integer-widen-s8`,
`integer-widen-s16` and `vertex-integer-widen` each contain a validated
80-byte stage header plus128 code bytes from the original pinned envyas.
The fragment variants alias R4 at PC0x68, consuming its low8/16 bits and
writing zero/sign-extended raw32 bits. An explicit wait precedes ISETP against
guest expected bits. An auxiliary I2I from RZ into R4 at PC0x80, with its own
WB0/wait1, must preserve P0 before SEL exports the comparison color.

The signed-halfword vertex fixture compares converted X bits against
FFFF999A. Only the SECOND initial vertex X=+.6 matches and moves to finite+.5;
the first/third low halfwords are CCCD. Guest second X=+.57 has unique low
EB85 and disables the move. Changing the binary to U16 plus expected0000EB85
restores second-only+.5 without another vertex-data change. The raw converted
value is never exported as a clip position. SHA-256 including SPH:

- integer-widen-u8.bin:25d263f4f7ffdc57f59a43681dbfa98b41648d7803a94f6c6817407ece4b0d47
- integer-widen-u16.bin:2c7b074a3caa1d3dbfe029490af0701caace93b8a8dc923864707a6af7b8e905
- integer-widen-s8.bin:f2e75e97ab11eed14608be3a92dd43f45c30a62e87806eb2d875b24dce74be9a
- integer-widen-s16.bin:ab5527eb7406f122c4043cd812da8b201355240ccc019539c68f7926c6456547
- vertex-integer-widen.bin:30f9ac2497d6f1e13deffa9eeef2165a3e8e70bb69b7b55ec3fc5134549044ec

All programs are synthetic and independently assembled. Narrow destinations,
cross-signed conversion, modifiers and other source lanes are not implied.

`integer-set-u-mask`, `integer-set-u-float`, `integer-set-s-mask`,
`integer-set-s-float` and `vertex-integer-set` each retain an80-byte validated
stage header plus128 original-envyas-assembled code bytes. The fragment
variants execute LT at PC0x78, with source signedness bit48 and BF result bit44.
LDC supplies both operands and independently expected raw bits. ISETP at0x80
checks the result, then an auxiliary ISET.NE RZ,RZ at0x88 must preserve P0
before the comparison color is exported.

The vertex fixture performs signed raw-bit X>0 and compares BF output against
3F800000. Only the second initial X=+.6 matches and moves to finite+.5. Changing
BF to mask and its expected MOV32I toFFFFFFFF preserves that geometry without
changing vertex bytes. Subsequent guest second X=-.6 disables the substitution.
Raw integer masks are not exported as float clip positions. SHA-256 including SPH:

- integer-set-u-mask.bin:0032c39f47859602affc06063f9e307d564ee727d249d00e8f6c4dae2c2a75f3
- integer-set-u-float.bin:86622a7bd41f14e3fd68ae6cfc905ae086285a744edf492e456a9e64a0f6bd1e
- integer-set-s-mask.bin:93afe9f17c9b7670007af58a82dbee31fc10cf783a6ea170452c42b01c7c4f3c
- integer-set-s-float.bin:9b2a27887965e5733148ecccf6bcbb68743f425a5542efb730b63a626792aa59
- vertex-integer-set.bin:c5a5aded51dc47666791d94d64ed495caf20c3b2aa58974a427c3dc937512020

These are synthetic independently assembled programs. F/T, extended carry,
CC, alternate boolean inputs and other source forms remain unsupported.
