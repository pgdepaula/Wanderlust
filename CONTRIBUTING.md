# Contributing to Wanderlust

Wanderlust develops native recompilation and hardware compatibility through small, verifiable changes. The repository is experimental; support for an individual instruction, service or GPU command must be demonstrated rather than inferred from a successful build.

## Get started

1. Read the [README](README.md), [build guide](docs/development/build-environment.md) and the documentation for the subsystem you want to change.
2. Build the workspace and run its tests with the pinned lockfile.
3. For substantial interface changes, describe the concrete problem and proposed boundary in an issue before preparing a large implementation.

```sh
cargo build --workspace --locked
cargo fmt --all --check
cargo clippy --workspace --all-targets --locked -- -D warnings
cargo test --workspace --locked
```

GPU tests require explicit execution with `--ignored` and an available backend. Optional source-built homebrew tests need devkitPro. Report an unavailable environment clearly; do not count an ignored scenario as an executed pass.

## Changes and pull requests

- Keep a change focused. Explain the trigger, resulting behavior and relevant limitations.
- Cite the hardware specification, ABI definition or pinned source revision behind a compatibility change. Check the license before reusing external code.
- Add a minimal regression for a bug. For new hardware behavior, use independently calculated results or independently assembled fixtures rather than the production implementation as its own oracle.
- Cover bounds, malformed input, address-domain separation, resource lifetime and partial-execution failures where relevant.
- Preserve checked arithmetic and byte order. Do not introduce silent fallbacks for unsupported guest behavior.
- Include the actual validation commands and backend results. Separate CPU tests, shader validation, GPU execution and guest-game evidence.
- Measure performance changes on a defined workload. Distinguish allocations, copies, uploads, submissions, completion waits and readbacks.
- Update technical documentation when an implemented contract changes.
- Sign off commits using the configured author identity: `git commit -s`.

## Fixtures and private material

Use small original programs, synthetic data or redistributable homebrew. Include assembly/source, toolchain provenance, expected results and applicable third-party notices. Do not add generated bulk output simply to increase test coverage counts.

Do not commit or upload commercial game code or assets, encryption keys, console-specific data, firmware, memory dumps, private logs or credentials. Keep locally supplied inputs under ignored `local-inputs/`; keep generated executables and diagnostics under ignored output directories.

Before submitting, inspect `git diff --check` and the exact files in the commit. Avoid unrelated formatting or generated-file changes.

## Licensing and security

Project-authored contributions use the [MIT License](LICENSE). Preserve the licenses and attribution of third-party material; the project license does not replace them.

Read [SECURITY.md](SECURITY.md) before reporting a security-sensitive issue.
