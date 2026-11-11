# Arona Code Review

Review committed changes or selected files against Arona's contribution guidelines and produce a Markdown report with findings and suggested remedies.
Each activated reviewer works in a separate context on maintainability, correctness (including hardware and ABI), security, or documentation.

## Usage

Invoke the skill in a coding agent that supports isolated subagents:

```text
$arona-code-review diff main review.md
$arona-code-review files kernel/src/trap.rs book/src/kernel/safety.md review.md
$arona-code-review files kernel/src/trap.rs:1-80 review.md
```

| Mode | Scope |
|---|---|
| `diff <base>` | The committed series `merge-base(<base>, HEAD)..HEAD`, including each commit's message and diff. Uncommitted edits are excluded. |
| `files <path[:lines] ...>` | Current working-tree file contents, optionally restricted to inclusive, 1-based line ranges. |

The output path is the last positional argument.
Existing reports are preserved unless `--overwrite` is supplied.
The review leaves the reviewed files unchanged.
See [SKILL.md](SKILL.md) for the complete interface and workflow.

## Layout

| Path | Purpose |
|---|---|
| [SKILL.md](SKILL.md) | Agent entry point and review workflow. |
| [personas/](personas/) | Individual reviewer responsibilities and required Arona documents. |
| [scripts/](scripts/) | Target resolution, prompt assembly, report rendering, and the shared findings contract. |
| [tests/](tests/) | Functional tests for the three helper scripts. |

The review uses the repository's [contribution guidelines](../../../CONTRIBUTING.md) and the documents listed by each persona.

## Script Tests

Run from the repository root:

```sh
make -C .agents/skills/arona-code-review test
```

These tests check the helper scripts; they do not measure the reviewers' ability to find defects.

Adapted from Asterinas's [aster-code-review](https://github.com/asterinas/asterinas/tree/main/.agents/skills/aster-code-review).
