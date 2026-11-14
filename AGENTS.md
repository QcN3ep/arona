# Repository Guidelines

Read and follow [CONTRIBUTING.md](CONTRIBUTING.md) before making changes.

## Project Structure & Module Organization

Arona is a Rust operating system targeting RISC-V and LoongArch.
`abi/` defines shared types and capability interfaces; `kernel/` implements resource mechanisms; `arch/` implements hardware interfaces and produces the kernel ELF.
`components/` contains the user-space runtime and system services.
`xtask/` provides host build, packaging, QEMU, and documentation tooling.
The handbook lives in `book/src/`; use its [contents](book/src/SUMMARY.md) to find subsystem documentation.
See [Code Organization](book/src/contributing/code-organization.md) for crate responsibilities.

## Build, Test, and Development Commands

Use the toolchain pinned in `rust-toolchain.toml` and run commands from the repository root:

```sh
cargo fmt --all -- --check                         # Check formatting
cargo clippy --locked --workspace --all-targets -- -D warnings --force-warn clippy::too_many_lines
cargo test --locked --workspace --tests            # Run host integration tests
cargo xtask test riscv64 --release                 # Run QEMU integration tests
cargo xtask build riscv64 --release                # Build services and kernel
cargo xtask run loongarch64 --release              # Build and boot in QEMU
cargo xtask book                                   # Build the handbook
```

Clippy checks workspace lints.
The build and run commands accept either `riscv64` or `loongarch64`; omit `--release` for debug builds.
See [xtask Command Reference](book/src/tools/xtask.md) for options and [the CI workflow](.github/workflows/ci.yml) for current CI coverage.

## Coding Style & Naming Conventions

Use Rust 2024, rustfmt, `snake_case` for functions and modules, and `UpperCamelCase` for types.
Write documentation, comments, and commit messages in English.
Follow the directory-specific `unsafe` and conditional-compilation restrictions in [Code Checks](book/src/contributing/checks.md), and the contracts in [Kernel Safety Boundaries](book/src/kernel/safety.md).
Apply CONTRIBUTING's comment and SPDX requirements.

## Testing Guidelines

Put host integration tests in the corresponding crate's `tests/` directory, matching its source structure.
Use `testkit/` for generic kernel scenarios and architecture images or component programs for hardware-dependent scenarios.
Cover observable behavior, necessary boundaries, and representative errors; check resource state after failures.
Do not add regression tests or fake hardware backends.
Choose validation using [Testing and Debugging](book/src/tools/testing.md), and report the checks actually run and any limits.

## Commit & Pull Request Guidelines

Develop on a non-main branch and integrate into `main` with a linear history.
Use a concise English imperative subject, capitalized and without a final period, such as `Add the Arona code review skill`.
Add a body only for context the diff cannot convey.

## Agent-Specific Instructions

For requested code reviews or explicit self-reviews, use [arona-code-review](.agents/skills/arona-code-review/SKILL.md).
