# Arona Contribution Guidelines

## Language

Documentation, commit messages, and code comments are written in English; technical terms retain their original form or abbreviation.

Use a space between content wrapped in backticks and the surrounding text.

## Commit Messages

Write every commit message in English, in the imperative mood, with a single-line subject. Start with a capitalized imperative verb, keep it concise, and do not end with a period.

For example:

```
Add the Arona Book
Fix interrupt starvation from repeated timer rearming
Move kernel integration tests into kernel-tests
```

### Description

When further explanation is needed, add a blank line and a description after the subject; use the imperative mood like the subject.

The description should not duplicate the diff; use a description only when there is information the code cannot convey.

## Branches

Development work must be done on a non-main branch and merged into `main` as a linear history.

## Tests

When writing integration tests, keep the test file structure consistent with the corresponding source code.

Good tests should:
* Write scenarios around user-observable functionality, covering normal inputs, necessary boundaries, and representative erroneous inputs.
* Tests for erroneous functionality should assert the error code and also check the state and resources after failure.
* Complete feature changes and the corresponding tests in the same commit, prefer extending existing scenarios, and merge duplicate coverage.

Good tests **should not**:
* Test things that are obviously correct, including but not limited to well-defined data structures and other content whose correctness the compiler guarantees.
* Regression tests.
* Treat internal layout or UB as functional assertions.
* Test hardware-related functionality by faking the hardware backend or injecting according to the order of internal operations.

## Comments

### Doc Comments

Doc comments in Rust may only use `///` and `//!`, must not use `/* */`, and must follow **rustdoc RFC 1574**.

* All `unsafe` functions and traits must have a `# Safety` section
* Module comments should state the module's functionality and the capabilities it exposes

### Line Comments

Only `//` may be used; do not write them if they have no explanatory value.

Code that meets the following special cases must use comments with specific markers:

| Marker | Used for |
|---|---|
| `// TODO:` | Content to be completed |
| `// HACK:` | Code that has to be implemented in an unconventional way |
| `// FIXME:` | Code with known functional or safety issues |
| `// REFERENCE: <url>` | Behavior defined by an external specification or a non-trivial algorithm |
| `// SAFETY:` | `unsafe` is used, or `unsafe` is not used but unsafe preconditions exist |

## License

The project's open-source license is Mozilla Public License Version 2.0; every code file requires an SPDX identifier:

```rust
// SPDX-License-Identifier: MPL-2.0
```

Non-Rust files use the comment syntax corresponding to that language.

Files that cannot contain comments (`.md`, `.json`, images, etc.) are declared in `REUSE.toml`.
