// Synthetic GM107: discard U<0.5, export red on the surviving half.
// envytools f102b82381f3f11cee113d16374c87091db039d9.
sched (st 0x6 wr 0x0) (st 0x6 wt 0x1) (st 0xd);
ipa pass $r4 a[0x80] $r255 $r255 $p7;
mov32i $r5 0x3f000000 0xf;
fsetp lt and $p0 $p7 $r4 $r5 $p7;
sched (st 0x6) (st 0x6) (st 0x6);
$p0 kil;
mov32i $r0 0x3f800000 0xf;
mov32i $r1 0x00000000 0xf;
sched (st 0x6) (st 0x6) (st 0x6);
mov32i $r2 0x00000000 0xf;
mov32i $r3 0x3f800000 0xf;
exit;
