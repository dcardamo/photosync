#!/usr/bin/env bash
# Secret scan. `--staged` checks what is about to be committed (the
# pre-commit hook); without it, the whole history (scripts/release.sh, CI).
set -euo pipefail
cd "$(git rev-parse --show-toplevel)"
command -v gitleaks >/dev/null || exec nix shell "$PWD#gitleaks" -c "$PWD/scripts/check-secrets.sh" "$@"

never_commit='(^|/)\.data/|\.(jks|keystore|p12|pfx)$|(^|/)secrets\.json$|(^|/)status\.token$|(^|/)journals/|(^|/)last-backup$'
if [[ ${1:-} == --staged ]]; then
  paths=$(git diff --cached --name-only --diff-filter=ACMR)
  args=(--pre-commit --staged)
else
  paths=$(git log --all --format= --name-only --diff-filter=ACMR | sort -u)
  args=()
fi
if found=$(grep -E "$never_commit" <<<"$paths"); then
  printf 'check-secrets: these files must never be committed:\n%s\n' "$found" >&2
  exit 1
fi
gitleaks git "${args[@]}" --redact --no-banner --log-level warn -v .
