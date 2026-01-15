#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
BIN_NAME="vole"
PREFIX="${PREFIX:-$HOME/.local}"
BIN_DIR="${BIN_DIR:-$PREFIX/bin}"
CARGO="${CARGO:-cargo}"
MARKER="# Added by Vole installer"

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

ensure_path_bashlike() {
  local rc="$1"
  if [[ ! -f "$rc" ]]; then
    touch "$rc"
  fi
  if ! grep -Fq "$MARKER" "$rc"; then
    cat >> "$rc" <<EOF
$MARKER
if [[ ":\$PATH:" != *":$BIN_DIR:"* ]]; then
  export PATH="$BIN_DIR:\$PATH"
fi
EOF
  fi
}

ensure_path_fish() {
  local rc="$1"
  local rc_dir
  rc_dir="$(dirname "$rc")"
  if [[ ! -d "$rc_dir" ]]; then
    mkdir -p "$rc_dir"
  fi
  if [[ ! -f "$rc" ]]; then
    touch "$rc"
  fi
  if ! grep -Fq "$MARKER" "$rc"; then
    cat >> "$rc" <<EOF
$MARKER
if not contains "$BIN_DIR" \$PATH
  set -gx PATH "$BIN_DIR" \$PATH
end
EOF
  fi
}

echo "Building Vole (release)..."
cd "$ROOT_DIR"
"$CARGO" build --release

mkdir -p "$BIN_DIR"
install -m 0755 "target/release/$BIN_NAME" "$BIN_DIR/$BIN_NAME"

echo "Installed $BIN_NAME to $BIN_DIR/$BIN_NAME"

shell_name="$(basename "${SHELL:-}")"
case "$shell_name" in
  bash)
    ensure_path_bashlike "$HOME/.bashrc"
    ;;
  zsh)
    ensure_path_bashlike "$HOME/.zshrc"
    ;;
  fish)
    ensure_path_fish "$HOME/.config/fish/config.fish"
    ;;
  *)
    ensure_path_bashlike "$HOME/.profile"
    ;;
esac

if [[ ":\$PATH:" != *":$BIN_DIR:"* ]]; then
  export PATH="$BIN_DIR:$PATH"
fi
