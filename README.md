# doter

A simple GTK4 GUI for managing dotfiles with Git.

![App Screenshot](assets/app.png)

## Features

- Visual management of dotfiles in `~` and `~/.config`
- Git-based versioning and sync
- Symlink management for active/inactive states

## Build

```bash
cargo build --release
```

## Run

```bash
cargo run
```

## Requirements

- GTK4 development libraries
- Git

## Release Archive

Build a distributable archive containing the binary, desktop file, and icon:

```bash
./scripts/package-release.sh
```

This writes a tarball to `dist/` that can be uploaded to a GitHub release and consumed by the AUR `doter-bin` package.

## Releases

The GitHub Actions release workflow supports two paths:

- Push an existing `vX.Y.Z` tag to build the archive, publish the GitHub release, and push updated AUR metadata.
- Run the `release` workflow manually to auto-bump the version before release. The bump rule is patch-first within a single decimal digit, so `0.0.8 -> 0.0.9`, `0.0.9 -> 0.1.0`, and `0.1.9 -> 0.2.0`.

For AUR publishing, configure an `AUR_SSH_PRIVATE_KEY` repository secret with push access to `doter-bin` on AUR. Optional repository variables `AUR_GIT_AUTHOR_NAME` and `AUR_GIT_AUTHOR_EMAIL` can override the default commit identity used for AUR updates.

## AUR

The AUR package metadata lives in the repository and is updated by the release workflow after each published version.

## License

MIT

## Linux Desktop Integration

The release archive and AUR package install:

- the `doter` binary
- a desktop launcher for application menus
- the bundled icon from `assets/logo.png`
