maddrsend (add $r0 0x44);
maddr 0x1d00;
send (add $r0 0);
parm $r2;
parm $r3;
send (add $r2 0);
send (add $r3 0);
maddr 0x8c4;
send (add $r1 0);
poll:
read $r2 0xd00;
mov $r2 (add $r2 -1);
branz $r2 #poll;
maddrsend (add $r0 0x40);
parm $r4;
maddr 0x680;
exit send (add $r4 0);
nop;
