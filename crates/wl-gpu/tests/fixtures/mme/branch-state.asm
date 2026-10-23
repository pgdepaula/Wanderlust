maddr 0x680;
mov $r2 (add $r1 -1);
branz annul $r2 #full;
send (add $r0 1);
bra annul #done;
send (add $r0 0x1111);
full:
mov $r3 (add $r0 2);
loop:
mov $r3 (add $r3 -1);
branz $r3 #loop;
nop;
send (add $r0 0x1111);
done:
exit;
nop;
