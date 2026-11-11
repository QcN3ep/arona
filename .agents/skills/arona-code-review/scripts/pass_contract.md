# Pass contract

This is one persona review pass of an already-running arona-code-review workflow.
The prompt contains your persona, required Arona guidelines, and canonical
**REVIEW INPUT** at the end. Do not reload the skill or its SKILL.md, spawn other
reviewers, assemble the report, or modify the reviewed files.

## Review method

Review only the supplied persona's remit. Do not run a general bug sweep from
every persona or repeat an investigation clearly owned by another persona.
Correctness owns functional behavior, including hardware and ABI contracts;
Security owns unsafe soundness and trust-boundary failures; Maintainability owns
structure and contribution conventions; Documentation owns accuracy and currency
of documentation and API comments.

Read surrounding code as needed to establish a finding. In `diff` mode, report
issues introduced by the reviewed commit series and distinguish an earlier
commit's code from the current checkout. In `files` mode, review the supplied
current contents and ranges.

For each finding, identify the concrete trigger, code evidence, and consequence.
Check the relevant callers, invariants, or authoritative specification when the
claim depends on them. Report supported defects and actual guideline violations;
do not turn an unsupported possibility or personal preference into a finding.
A real in-scope defect does not need a named guideline to be reportable.

The inlined Arona documents define project conventions. Cite the actual document
and section when reporting a violation; do not invent guideline names or import
another project's rules. Consult other repository documents or external
specifications when the specific code requires them.

## Output

Return **only** a JSON array of comment objects, with no prose or Markdown fences
around the array. Return `[]` when there are no findings.

```json
[{"file":"kernel/src/example.rs","line":42,"persona":"development","grounding":"Incorrect cleanup","severity":"major","problem":"When the second allocation fails, `create()` returns without releasing the first allocation.","fix":"Retain ownership of the first allocation until both allocations succeed, so failure releases it.","diff":"the relevant source lines or diff hunk"}]
```

Every finding has these fields:

- `file`: repository-relative path. For a commit-message finding, use
  `commit <sha> message` instead.
- `line`: line number in the post-change file of the relevant commit (`diff`),
  or the working-tree file (`files`). Omit it only for commit-message findings.
  Identify the relevant commit in `problem` if needed to locate an earlier
  commit's finding unambiguously.
- `persona`: the one supplied persona: `maintainability`, `development`,
  `security`, or `documentation`.
- `grounding`: an actual guideline citation, such as
  "`CONTRIBUTING.md` — Tests", or a short plain-language defect description,
  such as "Incorrect cleanup". Do not use the bare word "bug".
- `severity`: `critical` (must fix), `major` (should fix), `minor` (worth fixing),
  or `nit` (optional or stylistic).
- `problem`: the trigger, evidence, and consequence, written for the author.
- `fix`: a suggested remedy or required invariant. Describe it without editing
  code or generating a patch.
- `diff`: a short relevant diff hunk or source excerpt.

Use English and GitHub-flavored Markdown inside text fields. Wrap code
identifiers, paths, and literals in backticks, and fence multiline examples.
