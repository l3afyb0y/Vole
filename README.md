# Vole

Safe, Mole-inspired cleanup for Linux with a fast TUI and a full CLI.

Vole focuses on safe cleanup first: default rules are conservative, browser caches are excluded, and the tool always asks for confirmation before deleting.

## Features

- TUI + CLI parity: run `vole` for the interface or use `vole clean` flags.
- Safe defaults: user-only cleanup by default, with optional sudo mode.
- Dry-run toggle: preview without deleting via `--dry-run` or the TUI (shown in the TUI Output panel and saved to `~/vole-dry-run.txt`, removed after a successful non-dry run).
- Distro-aware rules: Arch first-class, then Fedora, then Ubuntu/Debian.
- Downloads cleanup: detects archives with matching extracted folders in `~/Downloads` and lets you choose which side to remove.
- Optional log cleanup: user log files older than 7 days (off by default).
- Snapshot gating: snapshot option only appears if a supported provider is detected.

## Install

### Local install (recommended)

```bash
./install.sh
```

This builds a release binary and installs it to a user bin directory (prefers `~/bin` if it is on `PATH`, otherwise `~/.local/bin`).
If Rust is missing, the installer will attempt to install it using your distro package manager (pacman, dnf, apt, zypper, apk, xbps) or rustup.
The install script will attempt to update your shell config automatically (bash, zsh, fish, or `~/.profile`).
If the install directory is not already on `PATH`, the installer will start a new login shell so you can run `vole` immediately.

### Cargo install

```bash
cargo install --path .
```

## Usage

### TUI

```bash
vole
```

Keys:
- `j/k` or arrows: move
- `space`: toggle rule
- `r`: rescan
- `d`: toggle dry-run
- `s`: sudo mode (will prompt via sudo)
- `p`: snapshot (only shown when supported)
- `a` or `enter`: apply/confirm
- `q`: quit
- Mouse: click to toggle, scroll to move, click/drag in output to jump

When Downloads Cleanup is enabled, apply will prompt you to remove archives or the extracted folders.

### CLI

```bash
vole clean --dry-run
vole clean --sudo
vole clean --downloads-remove archives
vole clean --downloads-remove folders
vole clean --rule user-trash --rule thumbnails
vole clean --list-rules
```

Use `--dry-run` to preview. Dry-run writes a detailed report to `~/vole-dry-run.txt` and shows output in the TUI panel (scrollable). The report is removed after a successful non-dry run.
By default, `clean` applies deletions after confirmation.
When running with `--sudo`, Vole requires typing `DELETE` to confirm.

## Configuration

By default Vole uses the embedded config. You can override it with:

```bash
vole --config /path/to/config.json clean --dry-run
```

The expected format is JSON (see `config/default.json` for examples).

## Snapshot Support

Vole only offers snapshotting when it detects a supported provider:

- Btrfs (home subvolume)
- Timeshift (Btrfs mode only)

If no supported snapshot provider is detected, the snapshot option is hidden. Snapshotting
is only available when running with sudo/root.

Btrfs snapshots are stored outside the source subvolume under the parent directory's
`.snapshots/vole` folder.

## Safety Notes

- Browser caches are excluded by default.
- System-wide cleanup requires sudo and explicit confirmation.
- Vole only deletes paths configured in the ruleset.

## Roadmap

- Disk analyzer, uninstall, optimize, and live status dashboards.
- Wider snapshot provider support (ZFS, LVM, Timeshift rsync).
