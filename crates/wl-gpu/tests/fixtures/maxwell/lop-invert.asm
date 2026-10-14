sched (st 0x6 wr 0x0) (st 0x6 wt 0x1) (st 0x6);
ld b64 $r4 c0[0x10];
lop and $p7 $r0 inv $r4 $r5;
lop or $p7 $r1 $r4 inv $r5;
sched (st 0x6) (st 0x6) (st 0x6);
lop xor $p7 $r2 inv $r4 inv $r5;
lop pass_b $p7 $r3 $r255 inv $r4;
exit;
