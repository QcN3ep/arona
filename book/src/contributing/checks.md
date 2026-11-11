# Code Checks

Code checks consist of Rust tooling and host integration tests.
They check types and lints, and actual functional behavior.
Source restrictions are project conventions for module boundaries.
Code review continues along the creation, use, and release of resources, verifying responsibilities, permissions, and the state after failures.

## Rust Checks

```sh
cargo fmt --all -- --check
cargo clippy --locked --workspace --all-targets -- \
  -D warnings --force-warn clippy::too_many_lines
cargo test --locked --workspace --tests
```

Workspace lints set `unsafe_op_in_unsafe_fn` and Clippy correctness to deny, enabling general Clippy checks and unsafe code comment checks.
`check-private-items = true` in `clippy.toml` applies the relevant checks to private items as well, and `too-many-lines-threshold = 80` provides a function size hint.


## Source Restrictions

The project follows these rules by directory:

| Directory | unsafe | cfg, cfg_attr, and cfg! |
|---|---|---|
| `kernel/` implementation | Forbidden | Forbidden |
| `kernel/tests/` | Allowed | Forbidden |
| `kernel-tests/`, `abi/` | Forbidden | Forbidden |
| `components/runtime/` | Allowed | Allowed |
| Other `components/` | Forbidden | Forbidden |

Architecture implementations live in `arch/` and may use `unsafe` and conditional compilation corresponding to hardware operations.


## Documentation Checks


rustdoc builds interface documentation for the kernel and each target with warnings denied.
The handbook build checks the table of contents, includes, plugins, and outputs via `cargo xtask book`.

## Review Responsibilities and Interfaces

A review can start from a single complete operation.
For example, mapping user memory goes from the system call arguments into the address space, through page frame references and page table edits, and on to unmapping and reclamation.
Along the way, verify who holds state, which records the public entry points update, and what resources failure paths leave behind.
The responsibilities of each layer provide a reference for this path: arch performs hardware operations, kernel manages resources, services interpret device and file semantics, and runtime connects the program runtime environment with the call interface.

Interface review focuses on:

- Whether the name accurately expresses the object, ownership, or address identity.
- Whether state has a single clear holder, and whether derived information can be obtained from existing state.
- Whether public entry points uniformly maintain permissions, mapping records, and resource lifetimes.
- Whether input checks are performed in code, and whether environment requirements are documented in the necessary `# Safety`.
- When a failure occurs, which operations have committed, and who holds handles, mappings, queues, and page frames respectively.
- Whether locks, interrupt-disabled regions, and wait points are consistent with the calling environment.

Comments explain the necessary conditions and reasons that the code cannot express directly.
Established terminology, clear types, and shorter call paths carry the main readability work.

## Verification by Feature

Tests are organized around normal inputs, necessary boundaries, and representative errors.
Error scenarios observe both the return value and the resource state after failure.
Hardware scenarios are exercised through real architecture implementations and QEMU devices, and related tests are maintained together with feature changes.

