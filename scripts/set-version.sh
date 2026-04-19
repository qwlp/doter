#!/usr/bin/env bash

set -euo pipefail

if [[ $# -ne 1 ]]; then
  echo "usage: $0 <version>" >&2
  exit 1
fi

version="$1"
repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"

if [[ ! "$version" =~ ^[0-9]+\.[0-9]+\.[0-9]+$ ]]; then
  echo "invalid semantic version: $version" >&2
  exit 1
fi

perl -0pi -e "s/^version = \".*\"\$/version = \"$version\"/m" \
  "$repo_root/Cargo.toml"

perl -0pi -e "s/^pkgver=.*/pkgver=$version/m" \
  "$repo_root/packaging/aur/doter-bin/PKGBUILD"

perl -0pi -e "s|releases/download/v[0-9]+\.[0-9]+\.[0-9]+/|releases/download/v$version/|g; s|doter-[0-9]+\.[0-9]+\.[0-9]+-x86_64-linux|doter-$version-x86_64-linux|g" \
  "$repo_root/packaging/aur/doter-bin/PKGBUILD"
