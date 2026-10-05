#!/bin/bash
# Runs INSIDE the macOS VM on every build. build.ps1 copies the source tree to
# ~/clatok, then invokes this over SSH. Produces a universal .dmg.
set -euo pipefail

# Pick up cargo / brew (non-login SSH shells skip .zprofile otherwise).
source "$HOME/.cargo/env" 2>/dev/null || true
[ -x /opt/homebrew/bin/brew ] && eval "$(/opt/homebrew/bin/brew shellenv)"
[ -x /usr/local/bin/brew  ] && eval "$(/usr/local/bin/brew shellenv)"

cd "$HOME/clatok"

echo "==> Building Tauri .dmg (universal: x86_64 + aarch64)..."
cargo tauri build --target universal-apple-darwin --bundles dmg,app

echo "==> Build finished. Artifacts:"
find "src-tauri/target/universal-apple-darwin/release/bundle/dmg" -name '*.dmg' 2>/dev/null || true
