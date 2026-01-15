# Vole

Safe, Mole-inspired cleanup for Linux with a fast TUI and a full CLI.

Vole focuses on safe cleanup first: default rules are conservative, browser caches are excluded, and the tool always asks for confirmation before deleting.

## Features

- TUI + CLI parity: run `vole` for the interface or use `vole clean` flags.
- Safe defaults: user-only cleanup by default, with optional sudo mode.
- Dry-run toggle: preview without deleting via `--dry-run` or the TUI.
- Distro-aware rules: Arch first-class, then Fedora, then Ubuntu/Debian.
- Snapshot gating: snapshot option only appears if a supported provider is detected.

## Install

### Local install (recommended)

```bash
./install.sh
```

This builds a release binary and installs it to `~/.local/bin/vole`. Make sure `~/.local/bin` is on your `PATH`.

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
- `a`: apply
- `q`: quit
- Mouse: click to toggle, scroll to move, use action buttons

### CLI

```bash
vole clean --dry-run
vole clean --sudo
vole clean --rule user-trash --rule thumbnails
vole clean --list-rules
```

Use `--dry-run` to preview. By default, `clean` applies deletions after confirmation.

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

If no supported snapshot provider is detected, the snapshot option is hidden.

## Safety Notes

- Browser caches are excluded by default.
- System-wide cleanup requires sudo and explicit confirmation.
- Vole only deletes paths configured in the ruleset.

## Roadmap

- Disk analyzer, uninstall, optimize, and live status dashboards.
- Wider snapshot provider support (ZFS, LVM, Timeshift rsync).
