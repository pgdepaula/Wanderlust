#!/usr/bin/env python3
"""Wanderlust AArch64 ISA Gap and Root Discovery Analyzer

Analyzes Wanderlust analysis outputs, surveys, or raw instruction streams to:
1. Differentiate non-code roots (alignment padding, data constants, jump tables) from genuine instructions.
2. Classify unsupported instruction opcodes into architectural functional families:
   - System Registers (e.g., CNTPCT_EL0 counter reads)
   - CRC32 / CRC32C (Data-processing 2-source)
   - Cryptographic Extensions (AES, SHA-1, SHA-256, PMULL)
   - Advanced SIMD (Widening multiply, saturating doubling, narrowing shift)
3. Produce actionable frequency and shape reports for AOT recompilation readiness.
"""
from __future__ import annotations

import argparse
import collections
import json
import os
import struct
import sys
from typing import Any, Dict, List, Optional, Tuple

try:
    import capstone
    HAVE_CAPSTONE = True
except ImportError:
    HAVE_CAPSTONE = False


KNOWN_DATA_PATTERNS = {
    0x00000000: "zero_padding_or_uninitialized",
    0x00008002: "arm_constant_or_data_pointer",
    0x00000001: "data_word_one",
}


def is_probable_data(word: int) -> bool:
    """Check if an opcode word is likely a non-code data artifact."""
    if word in KNOWN_DATA_PATTERNS:
        return True
    # ASCII string fragment check (all 4 bytes printable ASCII)
    b = struct.pack('<I', word)
    if all(0x20 <= x <= 0x7E for x in b):
        return True
    # Small integers or common pointer alignment artifacts
    if word < 0x10 or (word & 0xFF000000 == 0x00000000 and (word & 0x3) == 0):
        # 0x00xxxxxx with 4-byte alignment often indicates internal offsets / indices
        if word < 0x10000:
            return True
    return False


def classify_opcode_builtin(word: int) -> Tuple[str, str]:
    """Lightweight bitmask classifier for key AArch64 instruction families."""
    # MRS / MSR (System instructions)
    if (word & 0xFFD00000) == 0xD5100000:
        read = (word & (1 << 21)) != 0
        reg_id = (word >> 5) & 0x7FFF
        rt = word & 0x1F
        action = "mrs" if read else "msr"
        if reg_id == 0x5F01:
            return "system_counter", f"{action} x{rt}, cntpct_el0"
        elif reg_id == 0x5F02:
            return "system_counter", f"{action} x{rt}, cntvct_el0"
        elif reg_id == 0x5F00:
            return "system_counter", f"{action} x{rt}, cntfrq_el0"
        return "system_register", f"{action} x{rt}, sys_reg_0x{reg_id:04x}"

    # CRC32 / CRC32C (Data-processing 2-source: 0x1ac00000 / 0x9ac00000)
    if (word & 0x7FE00000) == 0x1AC00000:
        op = (word >> 10) & 0x3F
        if 16 <= op <= 23:
            c = (op >> 2) & 1
            sz = op & 3
            poly = "crc32c" if c else "crc32"
            suffix = ["b", "h", "w", "x"][sz]
            rd = word & 0x1F
            rn = (word >> 5) & 0x1F
            rm = (word >> 16) & 0x1F
            reg_type = "x" if sz == 3 else "w"
            return "crc32", f"{poly}{suffix} w{rd}, w{rn}, {reg_type}{rm}"

    # Cryptographic AES / SHA
    if (word & 0xFFE07C00) == 0x4E284800:
        return "crypto_aes", "aese"
    if (word & 0xFFE07C00) == 0x4E285800:
        return "crypto_aes", "aesd"
    if (word & 0xFFE07C00) == 0x4E287800:
        return "crypto_aes", "aesimc"
    if (word & 0xFFE0FC00) == 0x5E020000:
        return "crypto_sha", "sha1c"
    if (word & 0xFFE0FC00) == 0x5E282800:
        return "crypto_sha", "sha256su0"
    if (word & 0xFF200C00) == 0x0EE00000:
        return "crypto_pmull", "pmull"

    # Advanced SIMD
    if (word & 0x7F200000) == 0x0F000000:
        return "simd_widening", "smull_or_smlal"
    if (word & 0xFF200000) == 0x4F000000:
        return "simd_saturating", "sqdmulh_or_sqrdmulh"

    if is_probable_data(word):
        return "data_or_padding", KNOWN_DATA_PATTERNS.get(word, f"data_word_0x{word:08x}")

    return "unclassified", f"word_0x{word:08x}"


def analyze_entry(word: int, pc: int, md: Optional[Any] = None) -> Dict[str, Any]:
    """Analyze a single opcode site."""
    family, detail = classify_opcode_builtin(word)
    cs_mnemonic = ""
    cs_op_str = ""

    if HAVE_CAPSTONE and md:
        try:
            raw = struct.pack('<I', word)
            dis = list(md.disasm(raw, pc))
            if dis:
                cs_mnemonic = dis[0].mnemonic
                cs_op_str = dis[0].op_str
                # Refine family if Capstone provides accurate mnemonic
                if cs_mnemonic in ("crc32b", "crc32h", "crc32w", "crc32x", "crc32cb", "crc32ch", "crc32cw", "crc32cx"):
                    family = "crc32"
                    detail = f"{cs_mnemonic} {cs_op_str}"
                elif cs_mnemonic == "mrs" and "cntpct" in cs_op_str:
                    family = "system_counter"
                    detail = f"{cs_mnemonic} {cs_op_str}"
                elif cs_mnemonic in ("aese", "aesd", "aesimc", "aesmc"):
                    family = "crypto_aes"
                    detail = f"{cs_mnemonic} {cs_op_str}"
                elif cs_mnemonic in ("sha1c", "sha1p", "sha1m", "sha1su0", "sha1su1", "sha1h", "sha256h", "sha256h2", "sha256su0", "sha256su1"):
                    family = "crypto_sha"
                    detail = f"{cs_mnemonic} {cs_op_str}"
                elif cs_mnemonic in ("pmull", "pmull2"):
                    family = "crypto_pmull"
                    detail = f"{cs_mnemonic} {cs_op_str}"
                elif cs_mnemonic in ("sqdmulh", "sqrdmulh", "smull", "smull2", "sqrshrn", "saddlp", "smlal", "frecpe"):
                    family = "simd"
                    detail = f"{cs_mnemonic} {cs_op_str}"
                elif cs_mnemonic == "udf" and is_probable_data(word):
                    family = "data_or_padding"
                    detail = KNOWN_DATA_PATTERNS.get(word, f"udf_data_0x{word:08x}")
        except Exception:
            pass

    return {
        "pc": pc,
        "word": word,
        "hex": f"0x{word:08x}",
        "family": family,
        "detail": detail,
        "capstone_mnemonic": cs_mnemonic,
        "capstone_operands": cs_op_str,
        "is_data": family == "data_or_padding" or is_probable_data(word),
    }


def analyze_json_dataset(data: Dict[str, Any]) -> Dict[str, Any]:
    """Analyze a Wanderlust analysis or survey JSON file."""
    md = capstone.Cs(capstone.CS_ARCH_ARM64, capstone.CS_MODE_ARM) if HAVE_CAPSTONE else None

    # Collect sites
    sites: List[Dict[str, Any]] = []
    if "blocks" in data:
        # Structured analysis output (like analysis-sdk.json)
        for b in data.get("blocks", []):
            if "unsupported" in b:
                u = b["unsupported"]
                sites.append(analyze_entry(u["word"], u["pc"], md))
    elif "unsupported_examples" in data:
        # Survey format (wanderlust survey output)
        for ex in data.get("unsupported_examples", []):
            count = ex.get("count", 1)
            entry = analyze_entry(ex["opcode"], 0, md)
            entry["count"] = count
            sites.append(entry)

    total_sites = sum(s.get("count", 1) for s in sites)
    data_sites = [s for s in sites if s["is_data"]]
    data_count = sum(s.get("count", 1) for s in data_sites)
    code_sites = [s for s in sites if not s["is_data"]]
    code_count = sum(s.get("count", 1) for s in code_sites)

    # Breakdown by family
    family_counts: Dict[str, int] = collections.Counter()
    for s in sites:
        family_counts[s["family"]] += s.get("count", 1)

    # Detailed opcode frequencies for code sites
    code_opcode_counts: Dict[str, int] = collections.Counter()
    for s in code_sites:
        code_opcode_counts[f"{s['hex']} ({s['detail']})"] += s.get("count", 1)

    return {
        "total_sites": total_sites,
        "data_count": data_count,
        "data_ratio": (data_count / total_sites) if total_sites else 0.0,
        "code_count": code_count,
        "code_ratio": (code_count / total_sites) if total_sites else 0.0,
        "family_breakdown": dict(family_counts),
        "code_opcodes_top": code_opcode_counts.most_common(50),
        "sites": sites,
    }


def run_self_test() -> bool:
    """Verify classification logic against authentic TOTK test opcodes."""
    test_vectors = [
        # MRS system counter
        (0xD53BE020, "system_counter", "mrs x0, cntpct_el0"),
        (0xD53BE028, "system_counter", "mrs x8, cntpct_el0"),
        (0xD53BE034, "system_counter", "mrs x20, cntpct_el0"),
        # CRC32 / CRC32C
        (0x1ACB4108, "crc32", "crc32b w8, w8, w11"),
        (0x9AC44D08, "crc32", "crc32x w8, w8, x4"),
        (0x1ACB5108, "crc32", "crc32cb w8, w8, w11"),
        (0x9AC95D08, "crc32", "crc32cx w8, w8, x9"),
        # Crypto
        (0x4E284820, "crypto_aes", "aese"),
        (0x4E285820, "crypto_aes", "aesd"),
        (0x4E287821, "crypto_aes", "aesimc"),
        (0x5E020016, "crypto_sha", "sha1c"),
        (0x5E282820, "crypto_sha", "sha256su0"),
        (0x0EE1E004, "crypto_pmull", "pmull"),
        # Data & Padding
        (0x00000000, "data_or_padding", "zero_padding_or_uninitialized"),
        (0x00008002, "data_or_padding", "arm_constant_or_data_pointer"),
    ]

    md = capstone.Cs(capstone.CS_ARCH_ARM64, capstone.CS_MODE_ARM) if HAVE_CAPSTONE else None
    passed = 0
    for word, expected_family, expected_substr in test_vectors:
        res = analyze_entry(word, 0x1000, md)
        match_family = (res["family"] == expected_family)
        match_detail = (expected_substr in res["detail"] or expected_substr in res["capstone_mnemonic"])
        if match_family and match_detail:
            passed += 1
        else:
            print(f"Self-test failure on 0x{word:08x}: got ({res['family']}, {res['detail']}), expected ({expected_family}, {expected_substr})", file=sys.stderr)

    print(f"Self-test: {passed}/{len(test_vectors)} test vectors passed.")
    return passed == len(test_vectors)


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("input", nargs="?", help="Path to Wanderlust analysis JSON (e.g. analysis-sdk.json) or survey JSON")
    parser.add_argument("--json", dest="json_out", help="Path to write JSON classification report")
    parser.add_argument("--test", action="store_true", help="Run internal classification self-test")
    parser.add_argument("--top", type=int, default=30, help="Number of top instruction opcodes to display")
    args = parser.parse_args()

    if args.test:
        return 0 if run_self_test() else 1

    if not args.input:
        parser.print_help()
        return 1

    if not os.path.isfile(args.input):
        print(f"Error: input file {args.input} not found", file=sys.stderr)
        return 1

    with open(args.input, "r", encoding="utf-8") as f:
        data = json.load(f)

    result = analyze_json_dataset(data)

    print("=================================================================")
    print("  Wanderlust AArch64 ISA Gap & Root Discovery Analysis")
    print("=================================================================")
    print(f"Total Unsupported Sites Inspected: {result['total_sites']}")
    print(f"  - Non-code Data/Padding Artifacts: {result['data_count']:5d}  ({result['data_ratio']*100:5.1f}%)")
    print(f"  - Genuine Instruction Set Gaps:   {result['code_count']:5d}  ({result['code_ratio']*100:5.1f}%)")
    print("\n--- Breakdown by Architectural Family ---")
    for fam, cnt in sorted(result["family_breakdown"].items(), key=lambda x: x[1], reverse=True):
        ratio = (cnt / result['total_sites']) * 100 if result['total_sites'] else 0
        print(f"  {fam:22s}: {cnt:5d} ({ratio:5.1f}%)")

    print(f"\n--- Top {args.top} Genuine Executable Instruction Sites ---")
    for desc, cnt in result["code_opcodes_top"][:args.top]:
        print(f"  {cnt:4d}x  {desc}")

    if args.json_out:
        with open(args.json_out, "w", encoding="utf-8") as f:
            # Avoid serializing massive raw sites if not needed, or include summary + sample
            json.dump({
                "summary": {
                    "total_sites": result["total_sites"],
                    "data_count": result["data_count"],
                    "data_ratio": result["data_ratio"],
                    "code_count": result["code_count"],
                    "code_ratio": result["code_ratio"],
                    "family_breakdown": result["family_breakdown"],
                    "top_code_opcodes": result["code_opcodes_top"],
                }
            }, f, indent=2)
        print(f"\nReport written to: {args.json_out}")

    return 0


if __name__ == "__main__":
    sys.exit(main())
