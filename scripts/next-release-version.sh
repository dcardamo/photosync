#!/usr/bin/env bash
# CI tags releases without committing version bumps back to main.
set -euo pipefail
current=$(jq -r .version app/src-tauri/tauri.conf.json)
latest=$(git tag --list 'v*' | grep -E '^v[0-9]+\.[0-9]+\.[0-9]+$' | sort -V | tail -1 || true)
if [[ -z $latest ]]; then
  echo "$current"
elif [[ $current != "${latest#v}" && $(printf '%s\n' "$current" "${latest#v}" | sort -V | tail -1) == "$current" ]]; then
  echo "$current"
else
  IFS=. read -r major minor patch <<<"${latest#v}"
  echo "$major.$minor.$((patch + 1))"
fi
