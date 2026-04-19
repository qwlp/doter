#!/usr/bin/env bash

set -euo pipefail

repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$repo_root"

pkg_name="doter"
pkg_version="$(sed -n 's/^version = "\(.*\)"$/\1/p' Cargo.toml | head -n1)"
arch="$(uname -m)"
target_dir="target/release"
dist_root="dist"
archive_root="${pkg_name}-${pkg_version}-${arch}-linux"
staging_dir="${dist_root}/${archive_root}"
archive_path="${dist_root}/${archive_root}.tar.gz"

rm -rf "$staging_dir"
mkdir -p "$staging_dir" "$dist_root"

cargo build --release

install -Dm755 "${target_dir}/${pkg_name}" "${staging_dir}/${pkg_name}"
install -Dm644 "packaging/linux/doter.desktop" "${staging_dir}/doter.desktop"
install -Dm644 "assets/app.png" "${staging_dir}/doter.png"
install -Dm644 "README.md" "${staging_dir}/README.md"

tar -C "$dist_root" -czf "$archive_path" "$archive_root"
sha256sum "$archive_path"
