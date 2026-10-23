maddrsend (add $r0 0x44);
parmsend maddr (add $r0 0x1d00);
parm $r2;
send (add $r2 0);
parm $r3 maddr (add $r0 0x680);
exit parm $r3 send (add $r3 0);
nop;
