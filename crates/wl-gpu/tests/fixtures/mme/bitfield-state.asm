mov $r2 (extrinsrt $r0 $r1 0 16 0);
mov $r3 (add $r0 0);
mov $r2 (extrshl $r2 $r3 16 0);
mov $r2 (extrshl $r2 0 16 $r3);
maddr 0x680;
exit send (add $r2 0);
nop;
