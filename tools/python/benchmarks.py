#!/usr/bin/env python3
"""Reproducible compiler and runtime measurements.

  benchmarks.py measure -- COMMAND...   run COMMAND; print wall time, total child CPU time,
                                        peak child RSS (Linux ru_maxrss) and its JSON output
  benchmarks.py loop OUT                write the dispatch benchmark: 2,000,000 iterations of
                                        11 instructions with a call and three block transitions
  benchmarks.py memloop OUT             write the memory benchmark: 2,000,000 iterations of
                                        LDR/ADD/STR/LDP/ADD/STP through X2 (run with X2 pointing
                                        into scratch memory)
"""
import json
from pathlib import Path
import resource
import struct
import subprocess
import sys
import time

sys.path.insert(0, str(Path(__file__).resolve().parent))
from fixtures import add, branch, logical, movz, regop  # noqa: E402


def measure(command):
    start = time.time()
    run = subprocess.run(command, capture_output=True, text=True)
    usage = resource.getrusage(resource.RUSAGE_CHILDREN)
    result = {'returncode': run.returncode, 'wall_s': round(time.time() - start, 2),
              'child_cpu_s': round(usage.ru_utime + usage.ru_stime, 1),
              'peak_rss_mib': usage.ru_maxrss // 1024}
    try:
        result['tool'] = json.loads(run.stdout)
    except json.JSONDecodeError:
        result['stdout'] = run.stdout[-2000:]
    result['stderr'] = run.stderr[-2000:]
    return result


def loop_program():
    code = [logical(19, 31, 30), movz(0, 0), movz(1, 0x1e, half=1), 0xF2800000 | 0x8480 << 5 | 1,  # x1 = 2,000,000
            add(0, 0, 3), regop(2, 0, 1), logical(3, 2, 0, kind=2), regop(4, 3, 2, sub=True), add(5, 4, 7),
            0,                                        # bl leaf (patched below)
            0xF1000421,                               # subs x1, x1, #1
            0x54000001 | (((4 - 11) & 0x7FFFF) << 5),  # b.ne index 4
            logical(30, 31, 19), 0xD65F03C0]          # mov x30, x19; ret
    leaf = len(code)
    code[9] = branch(9, leaf, True)
    code += [add(6, 6, 1), regop(7, 6, 0), 0xD65F03C0]
    return struct.pack('<%dI' % len(code), *code)


def memory_loop_program():
    code = [movz(1, 0x1e, half=1), 0xF2800000 | 0x8480 << 5 | 1,  # x1 = 2,000,000
            0xF9400443, 0x91000463, 0xF9000443,   # ldr x3,[x2,#8]; add x3,x3,#1; str x3,[x2,#8]
            0xA9411444, 0x8B050084, 0xA9011444,   # ldp x4,x5,[x2,#16]; add x4,x4,x5; stp x4,x5,[x2,#16]
            0xF1000421,                           # subs x1, x1, #1
            0x54000001 | (((2 - 9) & 0x7FFFF) << 5),  # b.ne index 2
            0xD65F03C0]
    return struct.pack('<%dI' % len(code), *code)


def main():
    if len(sys.argv) >= 3 and sys.argv[1] == 'measure' and sys.argv[2] == '--':
        print(json.dumps(measure(sys.argv[3:])))
    elif len(sys.argv) == 3 and sys.argv[1] == 'loop':
        Path(sys.argv[2]).write_bytes(loop_program())
    elif len(sys.argv) == 3 and sys.argv[1] == 'memloop':
        Path(sys.argv[2]).write_bytes(memory_loop_program())
    else:
        print(__doc__)
        return 2
    return 0


if __name__ == '__main__':
    sys.exit(main())
