// Independently assembled GM107 vertex input + mutable stage-zero constants.
// c0[0x10/0x14] translates position; c0[0x18] scales varying U.
sched (st 0x6 wr 0x0) (st 0x6 wr 0x0 wt 0x1) (st 0x6 wt 0x1);
ld b128 $r0 a[0x80] $r255;
ld b64 $r4 a[0x90] $r255;
fadd $r0 $r0 c0[0x10];
sched (st 0x6) (st 0x6) (st 0x6);
fadd $r1 $r1 c0[0x14];
fmul $r4 $r4 c0[0x18];
st b128 a[0x70] $r0 $r255;
sched (st 0x6) (st 0x6) (st 0x6);
st b64 a[0x80] $r4 $r255;
exit;
nop 0;
