#!/usr/bin/env python3

from __future__ import annotations

import argparse
import hashlib
import os
import shutil
import sys
import tomllib
from dataclasses import dataclass
from pathlib import Path


@dataclass(frozen=True)
class Candidate:
    origin: str
    key: str
    relative_path: Path
    profiles: tuple[str, ...]
    source_path: Path
    shared_path: Path


def hash_file(path: Path) -> str:
    digest = hashlib.sha256()
    with path.open("rb") as handle:
        for chunk in iter(lambda: handle.read(65536), b""):
            digest.update(chunk)
    return digest.hexdigest()


def hash_tree(path: Path) -> str:
    rows: list[str] = []
    for root, dirnames, filenames in os.walk(path):
        dirnames.sort()
        filenames.sort()
        root_path = Path(root)
        rel_root = root_path.relative_to(path)
        rows.append(f"dir:{rel_root.as_posix()}")
        for filename in filenames:
            file_path = root_path / filename
            rel_file = file_path.relative_to(path)
            rows.append(f"file:{rel_file.as_posix()}:{hash_file(file_path)}")
    digest = hashlib.sha256()
    digest.update("\n".join(rows).encode())
    return digest.hexdigest()


def fingerprint(path: Path) -> tuple[str, str]:
    if path.is_dir():
        return ("dir", hash_tree(path))
    return ("file", hash_file(path))


def origin_label(scope: str) -> str:
    return {
        "home": "Home",
        "config": "XdgConfig",
        "custom": "Custom",
    }[scope]


def shared_root(repo_root: Path, scope: str) -> Path:
    return {
        "home": repo_root / "shared" / "home",
        "config": repo_root / "shared" / "config",
        "custom": repo_root / "shared" / "custom",
    }[scope]


def key_for(scope: str, relative_path: Path) -> str:
    return relative_path.as_posix()


def copy_path(source: Path, destination: Path) -> None:
    if destination.exists() or destination.is_symlink():
        if destination.is_dir() and not destination.is_symlink():
            shutil.rmtree(destination)
        else:
            destination.unlink()
    destination.parent.mkdir(parents=True, exist_ok=True)
    if source.is_dir():
        shutil.copytree(source, destination, symlinks=True)
    else:
        shutil.copy2(source, destination, follow_symlinks=False)


def load_links(path: Path) -> list[dict[str, object]]:
    if not path.exists():
        return []
    with path.open("rb") as handle:
        parsed = tomllib.load(handle)
    return list(parsed.get("entries", []))


def write_links(path: Path, entries: list[dict[str, object]]) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    lines: list[str] = []
    for entry in sorted(
        entries,
        key=lambda item: (str(item["origin"]), str(item["key"])),
    ):
        profiles = ", ".join(f'"{profile}"' for profile in entry["profiles"])
        lines.extend(
            [
                "[[entries]]",
                f'origin = "{entry["origin"]}"',
                f'key = "{entry["key"]}"',
                f"profiles = [{profiles}]",
                "",
            ]
        )
    content = "\n".join(lines).rstrip() + ("\n" if lines else "")
    path.write_text(content)


def discover_candidates(repo_root: Path) -> list[Candidate]:
    profiles_root = repo_root / "profiles"
    profiles = sorted(
        profile.name for profile in profiles_root.iterdir() if profile.is_dir()
    )
    grouped: dict[tuple[str, str], list[tuple[str, tuple[str, str], Path]]] = {}
    for profile in profiles:
        profile_root = profiles_root / profile
        for scope in ("home", "config"):
            scope_root = profile_root / scope
            if not scope_root.is_dir():
                continue
            for child in sorted(scope_root.iterdir(), key=lambda item: item.name):
                relative_path = Path(child.name)
                grouped.setdefault((scope, relative_path.as_posix()), []).append(
                    (profile, fingerprint(child), child)
                )

    candidates: list[Candidate] = []
    for (scope, relative_key), variants in sorted(grouped.items()):
        if len(variants) < 2:
            continue
        signatures = {signature for _, signature, _ in variants}
        if len(signatures) != 1:
            continue
        profiles_for_entry = tuple(sorted(profile for profile, _, _ in variants))
        relative_path = Path(relative_key)
        candidates.append(
            Candidate(
                origin=origin_label(scope),
                key=key_for(scope, relative_path),
                relative_path=relative_path,
                profiles=profiles_for_entry,
                source_path=variants[0][2],
                shared_path=shared_root(repo_root, scope) / relative_path,
            )
        )
    return candidates


def relink_active_path(
    candidate: Candidate,
    repo_root: Path,
    active_profile: str,
    home_root: Path,
    xdg_root: Path,
) -> str | None:
    live_path = (
        home_root / candidate.relative_path
        if candidate.origin == "Home"
        else xdg_root / candidate.relative_path
    )
    if not live_path.is_symlink():
        return None
    target = live_path.readlink()
    profile_target = (
        repo_root
        / "profiles"
        / active_profile
        / ("home" if candidate.origin == "Home" else "config")
        / candidate.relative_path
    )
    if target != profile_target:
        return None
    live_path.unlink()
    live_path.symlink_to(candidate.shared_path)
    return f"Relinked {live_path} -> {candidate.shared_path}"


def merge_links(
    existing: list[dict[str, object]], candidates: list[Candidate]
) -> list[dict[str, object]]:
    merged: dict[tuple[str, str], dict[str, object]] = {
        (str(entry["origin"]), str(entry["key"])): {
            "origin": str(entry["origin"]),
            "key": str(entry["key"]),
            "profiles": sorted({str(profile) for profile in entry.get("profiles", [])}),
        }
        for entry in existing
    }
    for candidate in candidates:
        merged[(candidate.origin, candidate.key)] = {
            "origin": candidate.origin,
            "key": candidate.key,
            "profiles": list(candidate.profiles),
        }
    return list(merged.values())


def run(args: argparse.Namespace) -> int:
    repo_root = Path(args.repo).expanduser().resolve()
    home_root = Path(args.home).expanduser().resolve()
    xdg_root = Path(args.xdg_config).expanduser().resolve()
    links_path = repo_root / "shared" / "links.toml"

    candidates = discover_candidates(repo_root)
    print(f"Found {len(candidates)} identical cross-profile entries to migrate.")
    for candidate in candidates:
        print(
            f"- {candidate.origin} {candidate.key} -> {candidate.shared_path} "
            f"[{', '.join(candidate.profiles)}]"
        )

    if not args.execute:
        return 0

    existing_links = load_links(links_path)
    for candidate in candidates:
        if candidate.shared_path.exists():
            if fingerprint(candidate.shared_path) != fingerprint(candidate.source_path):
                raise SystemExit(
                    f"Shared destination already exists with different contents: "
                    f"{candidate.shared_path}"
                )
        else:
            copy_path(candidate.source_path, candidate.shared_path)
            print(f"Copied {candidate.source_path} -> {candidate.shared_path}")

    merged_links = merge_links(existing_links, candidates)
    write_links(links_path, merged_links)
    print(f"Updated {links_path}")

    for candidate in candidates:
        if args.active_profile not in candidate.profiles:
            continue
        relink_message = relink_active_path(
            candidate, repo_root, args.active_profile, home_root, xdg_root
        )
        if relink_message:
            print(relink_message)

    return 0


def parse_args() -> argparse.Namespace:
    parser = argparse.ArgumentParser(
        description="Migrate identical profile entries into Doter's shared layer."
    )
    parser.add_argument("--repo", required=True, help="Path to the dotfiles repo")
    parser.add_argument(
        "--active-profile", required=True, help="Active profile on this machine"
    )
    parser.add_argument("--home", default=str(Path.home()), help="Home directory root")
    parser.add_argument(
        "--xdg-config",
        default=os.environ.get("XDG_CONFIG_HOME", str(Path.home() / ".config")),
        help="XDG config root",
    )
    parser.add_argument(
        "--execute", action="store_true", help="Apply the migration instead of previewing it"
    )
    return parser.parse_args()


if __name__ == "__main__":
    sys.exit(run(parse_args()))
