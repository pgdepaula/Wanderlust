# ADR-0001: Compile instruction-specialized Rust source ahead of time

Status: accepted for the supported integer execution experiment.

## Evidence

[N64Recomp](https://github.com/N64Recomp/N64Recomp/blob/ffb39cdad1da5de07eaaa48bd1db4a89a7986771/src/recompilation.cpp) supplies a concrete precedent for literal instruction translation plus a native runtime, but its MIPS frontend is unsuitable for A64. [Remill](https://github.com/lifting-bits/remill/blob/56918a8c2554088e93389e97d292f4035286506c/README.md) provides actual AArch64 semantic lifting; its C++/LLVM dependencies and binary/runtime integration remain substantial. [Cranelift](https://github.com/bytecodealliance/wasmtime/blob/e4657a0ceb5cf020fb68a553f6f839c5e6f99d69/cranelift/README.md) provides code generation, not guest decoding or Horizon behavior.

The available Rust compiler can build host executables directly. Emitted Rust with explicit wrapping arithmetic and checked memory avoids C undefined-behavior pitfalls and an extra C frontend/toolchain dependency. This does not eliminate LLVM: the tested rustc uses its normal LLVM backend. It avoids embedding or configuring LLVM as a project library.

## Decision

Use `wl-codegen-rust` for the first AOT backend, with a small instruction-semantic enum in `wl-aarch64`, recursive CFG discovery in `wl-analysis`, and an embedded dependency-free runtime. Use a block-address dispatcher for calls and branches. Require complete ahead-of-time translation of recovered code; unsupported instructions and unknown targets fail explicitly. Do not add a `wl-codegen-llvm` crate without an LLVM implementation.

## Consequences

The output contains compiled guest operations and has no continuous guest decoder/interpreter/JIT. Rust bounds checking and explicit arithmetic make the implemented semantic boundary auditable. Linux and Windows x86-64 execution of the original mixed NSO fixture has been observed; independent differential evidence currently covers the Linux-generated executables.

A single generated translation unit, embedded byte arrays, mutable CPU-state accesses, per-instruction budget accounting and dispatch at block boundaries may scale poorly. No whole-game performance result exists. A future backend decision must compare compile time, generated size, correctness coverage and execution throughput on a larger real supported input; it cannot be justified by architecture aesthetics alone. See the [assessment](../research/technology-assessment.md) and [validation method](../testing/validation-methodology.md).
