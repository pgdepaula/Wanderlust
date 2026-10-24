maddr 0x8e3;
send (add $r0 0);
maddr 0x8e4;
loop:
send (add $r0 0);
mov $r1 (add $r1 -1);
branz $r1 #loop;
nop;
exit;
nop;
