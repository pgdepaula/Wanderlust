maddr 0x8e3;
send (add $r0 16);
maddr 0x18e4;
send (add $r1 0);
parmsend maddr (add $r0 0x18e5);
parmsend maddr (add $r0 0x18e6);
parmsend maddr (add $r0 0x18e7);
exit;
nop;
