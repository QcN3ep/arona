---
name: arona-code-review
description: Use when asked to review Arona's committed Git changes or specified files for defects, including an explicitly requested self-review after implementation. Write a Markdown review report using Arona's contribution guidelines.
---

# arona-code-review

Review code against Arona's contribution guidelines and write one Markdown review file.
Each activated persona reviews independently, with only its own remit and guidelines.
Leave the reviewed files unchanged; findings and suggested remedies are the deliverable.

## Interface

Resolve the user's request into one raw argument string:

```text
diff   <base>             <output> [--overwrite]
files  <path[:lines] ...> <output> [--overwrite]
```

- `diff <base>` reviews the committed series `merge-base(<base>, HEAD)..HEAD`,
  oldest first, including each commit's message and diff.
  Uncommitted edits are excluded. Do not commit or switch checkouts to prepare a review.
- `files` reviews the current working-tree contents of the named files.
  Targets may have inclusive, 1-based ranges: `path:10-20,30-40`.
  Repeat a path to add ranges; double-quote paths containing spaces or colons
  (`"a: b.rs":10-20`).
- `<output>` is the last positional argument. Use a user-specified path;
  otherwise default to `review.md` at the repository root.
  Existing reports are preserved unless the user requests replacement (`--overwrite`).

If the request leaves the target ambiguous, clarify the scope.
A request for an uncommitted diff requires a different scope: these modes provide
committed changes or whole-file/range review, respectively.

Pass the entire raw argument string as **one argument** to `resolve_target.sh`;
do not split it yourself. Run scripts from the repository root using absolute paths.
For each shell command, set the skill path again:

```sh
SKILL="$(git rev-parse --show-toplevel)/.agents/skills/arona-code-review"
"$SKILL/scripts/resolve_target.sh" 'diff main review.md'
```

## Pipeline

1. **Resolve the target.** Save the canonical input from
   `resolve_target.sh '<raw args>'` to a temporary file.
   Save metadata from `resolve_target.sh --meta '<raw args>'` separately;
   append `date=YYYY-MM-DD` and optionally `title=...`.
   Stop on a resolution error, including an empty commit range.
2. **Activate personas.** Select them from the review input using the table below.
3. **Review independently.** For each activated persona, build its prompt with
   `build_pass_prompt.sh <input-file> <persona>` and give that exact text to a
   separate subagent with a clean context. Each subagent receives one persona.
   Use the host's native subagent tool; on Codex, use `spawn_agent` with
   `fork_turns="none"`. If isolated subagents are unavailable, report that limitation
   rather than combining the personas into one pass.
4. **Collect findings.** Save each returned JSON array as
   `<fragdir>/<persona>.json`. An empty array means no findings from that pass.
   Wait for every activated pass; report failures as incomplete coverage.
   Do not silently treat a missing or invalid fragment as a clean review.
5. **Assemble the report.** Run
   `assemble_review.sh [--overwrite] <meta-file> <fragdir> <output>`.
   Supply `--overwrite` only when requested by the user.
   The script groups findings by persona, sorts by location, and removes only
   exact duplicates within each persona.
6. **Summarize.** Replace `<!-- SUMMARY -->` with the principal findings ranked
   by severity and any material coverage limits. State when no issues were found.
   Preserve the findings produced by the passes; there is no separate verification
   or fix-consolidation stage.

## Activation

| Persona | Activate when |
|---|---|
| `maintainability` | Every review, including commit messages in `diff` mode. |
| `development` | Source code, scripts, build files, or executable configuration is reviewed. Covers correctness, including hardware and ABI behavior. |
| `security` | Source code, scripts, build files, or executable configuration is reviewed. Covers soundness and trust boundaries. |
| `documentation` | Documentation, code comments, public interfaces, commands, or user-visible behavior is reviewed. |

Activate a persona when the input falls within its remit; skip it when that remit
is absent. Hardware and ABI concerns belong to `development`.

## Pass contract and guidelines

[`scripts/pass_contract.md`](scripts/pass_contract.md) defines the shared review
method and JSON schema. The persona files define the individual responsibilities
and list their required Arona documents under `## Guidelines`.
`build_pass_prompt.sh` includes the contract, selected persona, and those documents
before the review input. Missing required documents are an error.

A pass may inspect surrounding code and consult specifications to establish a
finding. It must not reload this skill, spawn reviewers, modify reviewed files,
or assemble the final report.

## Output

The report records the date, mode, base or files, HEAD, and branch in frontmatter.
After `# Summary`, findings appear under **Maintainability**, **Correctness**,
**Security**, and **Documentation**, omitting empty sections.
Each finding includes its location, relevant excerpt, grounding, severity,
problem, and suggested remedy. Keep the script's report structure.
