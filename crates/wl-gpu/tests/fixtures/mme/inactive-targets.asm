// Independent synthetic Fermi MME fixture; envytools f102b82381f3f11cee113d16374c87091db039d9.
// Entry 0 (word 0): seven inactive SET_BLEND ports, method increment one.
inactive_blend:
maddr 0x14d9;
send (add $r1 0);
send (add $r1 0);
send (add $r1 0);
send (add $r1 0);
send (add $r1 0);
send (add $r1 0);
exit send (add $r1 0);
nop;
// Entry 1 (word 9): seven inactive SET_CT_WRITE ports.
inactive_masks:
maddr 0x1681;
send (add $r1 0);
send (add $r1 0);
send (add $r1 0);
send (add $r1 0);
send (add $r1 0);
send (add $r1 0);
exit send (add $r1 0);
nop;
