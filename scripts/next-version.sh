#!/usr/bin/env bash

set -euo pipefail

if [[ $# -ne 1 ]]; then
  echo "usage: $0 <current-version>" >&2
  exit 1
fi

current_version="$1"

if [[ ! "$current_version" =~ ^([0-9]+)\.([0-9]+)\.([0-9]+)$ ]]; then
  echo "invalid semantic version: $current_version" >&2
  exit 1
fi

major="${BASH_REMATCH[1]}"
minor="${BASH_REMATCH[2]}"
patch="${BASH_REMATCH[3]}"

if (( patch < 9 )); then
  patch=$((patch + 1))
else
  patch=0
  if (( minor < 9 )); then
    minor=$((minor + 1))
  else
    minor=0
    major=$((major + 1))
  fi
fi

printf '%s.%s.%s\n' "$major" "$minor" "$patch"
