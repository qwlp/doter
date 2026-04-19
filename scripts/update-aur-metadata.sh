#!/usr/bin/env bash

set -euo pipefail

version=""
sha256=""
pkgbuild_path=""
srcinfo_path=""

while [[ $# -gt 0 ]]; do
  case "$1" in
    --version)
      version="${2:-}"
      shift 2
      ;;
    --sha256)
      sha256="${2:-}"
      shift 2
      ;;
    --pkgbuild)
      pkgbuild_path="${2:-}"
      shift 2
      ;;
    --srcinfo)
      srcinfo_path="${2:-}"
      shift 2
      ;;
    *)
      echo "unknown argument: $1" >&2
      exit 1
      ;;
  esac
done

repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
pkgbuild_path="${pkgbuild_path:-$repo_root/packaging/aur/doter-bin/PKGBUILD}"
srcinfo_path="${srcinfo_path:-$repo_root/packaging/aur/doter-bin/.SRCINFO}"

if [[ ! "$version" =~ ^[0-9]+\.[0-9]+\.[0-9]+$ ]]; then
  echo "invalid semantic version: $version" >&2
  exit 1
fi

if [[ ! "$sha256" =~ ^[0-9a-f]{64}$ ]]; then
  echo "invalid sha256: $sha256" >&2
  exit 1
fi

perl -0pi -e "s/^pkgver=.*/pkgver=$version/m" \
  "$pkgbuild_path"
perl -0pi -e "s|releases/download/v[0-9]+\.[0-9]+\.[0-9]+/|releases/download/v$version/|g; s|doter-[0-9]+\.[0-9]+\.[0-9]+-x86_64-linux|doter-$version-x86_64-linux|g" \
  "$pkgbuild_path"
perl -0pi -e "s/^sha256sums=\\('.*'\\)\$/sha256sums=('${sha256}')/m" \
  "$pkgbuild_path"

cat > "$srcinfo_path" <<EOF
pkgbase = doter-bin
	pkgdesc = A simple GTK4 GUI for managing dotfiles with Git
	pkgver = $version
	pkgrel = 1
	url = https://github.com/qwlp/doter
	arch = x86_64
	license = MIT
	depends = git
	depends = gtk4
	provides = doter
	conflicts = doter
	source = doter-bin-$version.tar.gz::https://github.com/qwlp/doter/releases/download/v$version/doter-$version-x86_64-linux.tar.gz
	sha256sums = $sha256

pkgname = doter-bin
EOF
