#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
BIN_NAME="vole"
PREFIX="${PREFIX:-$HOME/.local}"
BIN_DIR="${BIN_DIR:-$PREFIX/bin}"
CARGO="${CARGO:-cargo}"

usage() {
  cat <<'EOF'
Usage: ./install.sh [--prefix <path>] [--bin-dir <path>]

Builds Vole in release mode and installs the binary.

Options:
  --prefix <path>   Install prefix (default: ~/.local)
  --bin-dir <path>  Override binary directory (default: <prefix>/bin)
  -h, --help        Show this help
EOF
}

while [[ $# -gt 0 ]]; do
  case "$1" in
    --prefix)
      PREFIX="${2:-}"
      if [[ -z "$PREFIX" ]]; then
        echo "Missing value for --prefix" >&2
        exit 1
      fi
      BIN_DIR="$PREFIX/bin"
      shift 2
      ;;
    --bin-dir)
      BIN_DIR="${2:-}"
      if [[ -z "$BIN_DIR" ]]; then
        echo "Missing value for --bin-dir" >&2
        exit 1
      fi
      shift 2
      ;;
    -h|--help)
      usage
      exit 0
      ;;
    *)
      echo "Unknown option: $1" >&2
      usage
      exit 1
      ;;
  esac
done

if ! command -v "$CARGO" >/dev/null 2>&1; then
  echo "cargo is required. Install Rust first: https://rustup.rs" >&2
  exit 1
fi

echo "Building Vole (release)..."
cd "$ROOT_DIR"
"$CARGO" build --release

mkdir -p "$BIN_DIR"
install -m 0755 "target/release/$BIN_NAME" "$BIN_DIR/$BIN_NAME"

echo "Installed $BIN_NAME to $BIN_DIR/$BIN_NAME"
if ! command -v "$BIN_NAME" >/dev/null 2>&1; then
  echo "Add $BIN_DIR to your PATH to use '$BIN_NAME' directly."
fi
