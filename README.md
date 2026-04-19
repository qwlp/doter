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

## AUR

The AUR package metadata lives in `packaging/aur/doter-bin/`.

After you rename the GitHub repository to `doter` and upload the release tarball, update `pkgver` if needed and regenerate `.SRCINFO` with:

```bash
cd packaging/aur/doter-bin
makepkg --printsrcinfo > .SRCINFO
```

Before submitting to AUR, add a real project license. The current package metadata uses `custom` as a temporary placeholder because the repository does not include a license file yet.

## License

MIT
