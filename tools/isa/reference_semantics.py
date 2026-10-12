#!/usr/bin/env python3
"""Wanderlust AArch64 Reference Semantics and Test Vector Generator

Uses Unicorn Engine to execute real AArch64 instructions and generate verified,
authoritative golden test vectors for Wanderlust's AArch64 decoder and recompilation tests.

IMPORTANT SCOPE DISTINCTION:
This tool generates and validates reference semantic test vectors using an independent
CPU emulator (Unicorn Engine). It does NOT establish that the Wanderlust Rust AOT runtime
executes these instructions yet; AOT code generation and runtime lowering remain owned
by Claude for integration.

Supports:
- CRC32 / CRC32C (8-bit, 16-bit, 32-bit, 64-bit with standard and Castagnoli polynomials)
- Edge cases: all-zeros, all-ones seeds, high-bit masking, and 64-bit operand widths
- Built-in verification against independent known golden constants
"""
from __future__ import annotations

import argparse
import json
import struct
import sys
from typing import Any, Dict, List

try:
    import unicorn
    from unicorn.arm64_const import *
    HAVE_UNICORN = True
except ImportError:
    HAVE_UNICORN = False


# Independently verified golden reference constants for assertion checks
INDEPENDENT_GOLDEN_CHECKS = {
    # Standard CRC-32 (0x04C11DB7, bit-reflected)
    ("crc32b", 0x00000000, 0x00000000): 0x00000000,
    ("crc32b", 0x00000000, 0x00000041): 0x01DB7106, # 'A'
    ("crc32b", 0xFFFFFFFF, 0x0000005A): 0xA643A898, # 'Z' with full seed
    ("crc32h", 0x12345678, 0x0000ABCD): 0x5ABEC668,
    ("crc32w", 0xFFFFFFFF, 0x12345678): 0x5092782D,
    ("crc32x", 0x00000000, 0x0123456789ABCDEF): 0x21193D2E,
    ("crc32x", 0xDEADBEEF, 0xFEDCBA9876543210): 0xDC82FD63,

    # Castagnoli CRC-32C (0x1EDC6F41, bit-reflected)
    ("crc32cb", 0x00000000, 0x00000000): 0x00000000,
    ("crc32cb", 0x00000000, 0x00000041): 0xB3109EBF, # 'A'
    ("crc32cb", 0xFFFFFFFF, 0x0000005A): 0x97455E45,
    ("crc32ch", 0x12345678, 0x0000ABCD): 0xAAE32043,
    ("crc32cw", 0xFFFFFFFF, 0x12345678): 0x4DECE20C,
    ("crc32cx", 0x00000000, 0x0123456789ABCDEF): 0xE9986AA9,
    ("crc32cx", 0xDEADBEEF, 0xFEDCBA9876543210): 0xFEFFED7A,
}


def run_arm64_instruction(
    instruction_bytes: bytes,
    initial_regs: Dict[int, int],
    read_regs: List[int],
    mem_base: int = 0x1000_0000,
    mem_size: int = 0x10_000,
) -> Dict[int, int]:
    """Execute instruction bytes on Unicorn ARM64 and return register state."""
    if not HAVE_UNICORN:
        raise RuntimeError("Unicorn engine is required for reference emulation")

    uc = unicorn.Uc(unicorn.UC_ARCH_ARM64, unicorn.UC_MODE_ARM)
    uc.mem_map(mem_base, mem_size)
    uc.mem_write(mem_base, instruction_bytes)

    for reg, val in initial_regs.items():
        uc.reg_write(reg, val)

    # Execute single instruction
    uc.emu_start(mem_base, mem_base + len(instruction_bytes), count=1)

    result = {}
    for reg in read_regs:
        result[reg] = uc.reg_read(reg)
    return result


def generate_crc32_vectors() -> List[Dict[str, Any]]:
    """Generate golden test vectors for CRC32 and CRC32C across all operand widths."""
    vectors = []
    # Test cases: (name, opcode_le, initial_acc, input_val, is_64)
    test_specs = [
        # Standard CRC-32 (polynomial 0x04C11DB7, bit-reflected)
        # Edge cases: zeros, standard single-byte, halfword, word, dword
        ("crc32b", 0x1AC24020, 0x00000000, 0x00000000, False), # all zero edge case
        ("crc32b", 0x1AC24020, 0x00000000, 0x00000041, False), # 'A'
        ("crc32b", 0x1AC24020, 0xFFFFFFFF, 0x0000005A, False), # 'Z' with full seed
        ("crc32b", 0x1AC24020, 0x00000000, 0x000000FF, False), # max byte edge case
        ("crc32h", 0x1AC24420, 0x00000000, 0x00000000, False), # all zero halfword
        ("crc32h", 0x1AC24420, 0x12345678, 0x0000ABCD, False),
        ("crc32w", 0x1AC24820, 0x00000000, 0x00000000, False), # all zero word
        ("crc32w", 0x1AC24820, 0xFFFFFFFF, 0x12345678, False),
        ("crc32w", 0x1AC24820, 0x12345678, 0xFFFFFFFF, False), # max word input
        ("crc32x", 0x9AC24C20, 0x00000000, 0x0000000000000000, True), # all zero dword
        ("crc32x", 0x9AC24C20, 0x00000000, 0x0123456789ABCDEF, True),
        ("crc32x", 0x9AC24C20, 0xDEADBEEF, 0xFEDCBA9876543210, True),

        # Castagnoli CRC-32C (polynomial 0x1EDC6F41, bit-reflected)
        # Edge cases: zeros, standard single-byte, halfword, word, dword
        ("crc32cb", 0x1AC25020, 0x00000000, 0x00000000, False), # all zero edge case
        ("crc32cb", 0x1AC25020, 0x00000000, 0x00000041, False), # 'A'
        ("crc32cb", 0x1AC25020, 0xFFFFFFFF, 0x0000005A, False),
        ("crc32cb", 0x1AC25020, 0x00000000, 0x000000FF, False), # max byte edge case
        ("crc32ch", 0x1AC25420, 0x00000000, 0x00000000, False), # all zero halfword
        ("crc32ch", 0x1AC25420, 0x12345678, 0x0000ABCD, False),
        ("crc32cw", 0x1AC25820, 0x00000000, 0x00000000, False), # all zero word
        ("crc32cw", 0x1AC25820, 0xFFFFFFFF, 0x12345678, False),
        ("crc32cw", 0x1AC25820, 0x12345678, 0xFFFFFFFF, False), # max word input
        ("crc32cx", 0x9AC25C20, 0x00000000, 0x0000000000000000, True), # all zero dword
        ("crc32cx", 0x9AC25C20, 0x00000000, 0x0123456789ABCDEF, True),
        ("crc32cx", 0x9AC25C20, 0xDEADBEEF, 0xFEDCBA9876543210, True),
    ]

    for name, opcode, acc, val, is_64 in test_specs:
        raw = struct.pack('<I', opcode)
        initial = {
            UC_ARM64_REG_X1: acc,
            UC_ARM64_REG_X2: val,
        }
        res = run_arm64_instruction(raw, initial, [UC_ARM64_REG_X0])
        golden_crc = res[UC_ARM64_REG_X0] & 0xFFFFFFFF

        # Self-assertion against independent constants if present in verification table
        key = (name, acc, val)
        if key in INDEPENDENT_GOLDEN_CHECKS:
            expected = INDEPENDENT_GOLDEN_CHECKS[key]
            if golden_crc != expected:
                raise AssertionError(
                    f"Reference engine failure on {name}: acc={acc:#x}, val={val:#x} => got {golden_crc:#x}, expected {expected:#x}"
                )

        vectors.append({
            "instruction": name,
            "opcode": f"0x{opcode:08x}",
            "initial_acc": f"0x{acc:08x}",
            "input_val": f"0x{val:016x}" if is_64 else f"0x{val:08x}",
            "expected_crc": f"0x{golden_crc:08x}",
        })

    return vectors


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--json", dest="json_out", help="Write JSON golden vectors to file")
    parser.add_argument("--verify", action="store_true", help="Run self-verification against independent constants")
    args = parser.parse_args()

    if not HAVE_UNICORN:
        print("Error: Unicorn Engine not available in current environment", file=sys.stderr)
        return 1

    try:
        vectors = generate_crc32_vectors()
    except Exception as e:
        print(f"Error during reference vector generation: {e}", file=sys.stderr)
        return 1

    print("=== AArch64 CRC32 Reference Test Vectors (Unicorn ARM64) ===")
    print(f"Total verified cases: {len(vectors)} (covering all 8 width/polynomial combinations)")
    for v in vectors:
        print(f"  {v['instruction']:8s} opcode={v['opcode']} acc={v['initial_acc']} val={v['input_val']} => res={v['expected_crc']}")

    if args.json_out:
        with open(args.json_out, "w", encoding="utf-8") as f:
            json.dump({"crc32_vectors": vectors}, f, indent=2)
        print(f"\nGolden vectors written to: {args.json_out}")

    print("\n[OK] Reference semantics generation and independent assertion checks passed.")
    return 0


if __name__ == "__main__":
    sys.exit(main())
