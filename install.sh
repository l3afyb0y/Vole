#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
BIN_NAME="vole"
PREFIX="${PREFIX:-}"
BIN_DIR="${BIN_DIR:-}"
CARGO="${CARGO:-cargo}"
MARKER="# Added by Vole installer"
ORIGINAL_PATH="${PATH}"

have_cmd() {
  command -v "$1" >/dev/null 2>&1
}

ensure_rust_toolchain() {
  if have_cmd "$CARGO"; then
    return 0
  fi

  echo "cargo not found. Installing Rust toolchain..."

  if have_cmd pacman; then
    if ! have_cmd sudo; then
      echo "sudo is required to install Rust with pacman." >&2
      exit 1
    fi
    sudo pacman -S --needed rust cargo
  elif have_cmd dnf; then
    if ! have_cmd sudo; then
      echo "sudo is required to install Rust with dnf." >&2
      exit 1
    fi
    sudo dnf install -y rust cargo
  elif have_cmd yum; then
    if ! have_cmd sudo; then
      echo "sudo is required to install Rust with yum." >&2
      exit 1
    fi
    sudo yum install -y rust cargo
  elif have_cmd apt-get; then
    if ! have_cmd sudo; then
      echo "sudo is required to install Rust with apt-get." >&2
      exit 1
    fi
    sudo apt-get update
    sudo apt-get install -y cargo
  elif have_cmd zypper; then
    if ! have_cmd sudo; then
      echo "sudo is required to install Rust with zypper." >&2
      exit 1
    fi
    sudo zypper --non-interactive install rust cargo
  elif have_cmd apk; then
    if ! have_cmd sudo; then
      echo "sudo is required to install Rust with apk." >&2
      exit 1
    fi
    sudo apk add rust cargo
  elif have_cmd xbps-install; then
    if ! have_cmd sudo; then
      echo "sudo is required to install Rust with xbps-install." >&2
      exit 1
    fi
    sudo xbps-install -Sy rust cargo
  else
    if have_cmd curl; then
      curl https://sh.rustup.rs -sSf | sh -s -- -y
    elif have_cmd wget; then
      wget -qO- https://sh.rustup.rs | sh -s -- -y
    else
      echo "Could not find a package manager or curl/wget to install Rust." >&2
      echo "Install Rust from https://rustup.rs and re-run this installer." >&2
      exit 1
    fi
  fi

  if [[ -f "$HOME/.cargo/env" ]]; then
    # shellcheck disable=SC1091
    source "$HOME/.cargo/env"
  fi

  if [[ -x "$HOME/.cargo/bin/cargo" ]]; then
    CARGO="$HOME/.cargo/bin/cargo"
  fi

  if ! have_cmd "$CARGO"; then
    echo "cargo is still unavailable after install. Please ensure Rust is installed." >&2
    exit 1
  fi
}

usage() {
  cat <<'EOF'
Usage: ./install.sh [--prefix <path>] [--bin-dir <path>]

Builds Vole in release mode and installs the binary.

Options:
  --prefix <path>   Install prefix (default: auto-select bin dir)
  --bin-dir <path>  Override binary directory (default: ~/bin or ~/.local/bin)
  -h, --help        Show this help
EOF
}

default_bin_dir() {
  local home_bin="$HOME/bin"
  local local_bin="$HOME/.local/bin"
  local in_path_dir=""

  if [[ -d "$home_bin" ]]; then
    home_bin="$(cd "$home_bin" && pwd)"
  fi
  if [[ -d "$local_bin" ]]; then
    local_bin="$(cd "$local_bin" && pwd)"
  fi

  if [[ -n "$home_bin" && ":$PATH:" == *":$home_bin:"* ]]; then
    echo "$home_bin"
    return
  fi
  if [[ -n "$local_bin" && ":$PATH:" == *":$local_bin:"* ]]; then
    echo "$local_bin"
    return
  fi
  if [[ -d "$home_bin" ]]; then
    echo "$home_bin"
    return
  fi
  if [[ -d "$local_bin" ]]; then
    echo "$local_bin"
    return
  fi

  IFS=':' read -r -a path_entries <<< "$PATH"
  for dir in "${path_entries[@]}"; do
    if [[ -n "$dir" && -d "$dir" && -w "$dir" ]]; then
      in_path_dir="$dir"
      break
    fi
  done
  if [[ -n "$in_path_dir" ]]; then
    echo "$in_path_dir"
    return
  fi

  echo "$HOME/.local/bin"
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

if [[ -z "$BIN_DIR" ]]; then
  if [[ -n "$PREFIX" ]]; then
    BIN_DIR="$PREFIX/bin"
  else
    BIN_DIR="$(default_bin_dir)"
  fi
fi

ensure_rust_toolchain

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
if not contains -- "$BIN_DIR" \$PATH
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
    ensure_path_bashlike "$HOME/.bash_profile"
    ensure_path_bashlike "$HOME/.profile"
    ;;
  zsh)
    ensure_path_bashlike "$HOME/.zshrc"
    ensure_path_bashlike "$HOME/.zprofile"
    ensure_path_bashlike "$HOME/.profile"
    ;;
  fish)
    ensure_path_fish "$HOME/.config/fish/config.fish"
    ensure_path_bashlike "$HOME/.profile"
    ;;
  *)
    ensure_path_bashlike "$HOME/.profile"
    ;;
esac

if [[ ":$PATH:" != *":$BIN_DIR:"* ]]; then
  export PATH="$BIN_DIR:$PATH"
fi

if [[ -f "$HOME/.profile" ]]; then
  source "$HOME/.profile" || true
fi

if [[ ":$ORIGINAL_PATH:" != *":$BIN_DIR:"* ]]; then
  if [[ -t 0 && -t 1 ]]; then
    echo "Starting a new login shell to load PATH..."
    exec "${SHELL:-/bin/bash}" -l
  else
    echo "Open a new terminal or run: source \"$HOME/.profile\" to use 'vole' immediately."
  fi
fi
