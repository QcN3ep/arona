#!/usr/bin/env bash

# SPDX-License-Identifier: MPL-2.0

# Build one persona's prompt: contract, persona, required guidelines, then input.
# Keeping the review input last preserves a reusable prompt prefix.
# Run from the repository being reviewed.
# Usage: build_pass_prompt.sh <input-file> <persona>
set -euo pipefail
HERE="$(cd "$(dirname "$0")" && pwd)"
SKILLDIR="$(cd "$HERE/.." && pwd)"

if [[ $# -ne 2 || ! -f "$1" ]]; then
    echo "usage: build_pass_prompt.sh <input-file> <persona>" >&2
    exit 2
fi
input="$1"
persona="$2"
case "$persona" in
    maintainability|development|security|documentation) ;;
    *) echo "build_pass_prompt.sh: no such persona: $persona" >&2; exit 2 ;;
esac
pf="$SKILLDIR/personas/$persona.md"
REPO="$(git rev-parse --show-toplevel)"

# Resolve and validate the persona's explicit document list before emitting a prompt.
guidelines=()
while IFS= read -r gp; do
    [[ -n "$gp" ]] || continue
    if [[ ! -f "$REPO/$gp" ]]; then
        echo "build_pass_prompt.sh: missing required guideline: $gp" >&2
        exit 2
    fi
    guidelines+=("$gp")
done < <(sed -n '/^## Guidelines$/,/^## /{/^- `/s/^- `\([^`]*\)`.*/\1/p;}' "$pf")
if [[ ${#guidelines[@]} -eq 0 ]]; then
    echo "build_pass_prompt.sh: no guidelines listed for $persona" >&2
    exit 2
fi

cat "$HERE/pass_contract.md"
printf '\n===== PERSONA: %s =====\n\n' "$persona"
cat "$pf"
for gp in "${guidelines[@]}"; do
    printf '\n--- guideline document (%s) ---\n\n' "$gp"
    cat "$REPO/$gp"
done
printf '\n===== REVIEW INPUT =====\n\n'
cat "$input"
