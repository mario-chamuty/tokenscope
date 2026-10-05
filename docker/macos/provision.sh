#!/bin/bash
# ONE-TIME provisioning, run INSIDE the macOS VM (over SSH or in the GUI Terminal).
# Installs the full toolchain needed to build TokenScope's .dmg.
# After this finishes, shut the VM down and export its disk image (see SETUP.md).
set -euo pipefail

echo "==> [1/5] Xcode Command Line Tools (clang, ld, headers)..."
if ! xcode-select -p >/dev/null 2>&1; then
    # Headless CLT install via softwareupdate.
    touch /tmp/.com.apple.dt.CommandLineTools.installondemand.in-progress
    LABEL=$(softwareupdate -l 2>/dev/null \
        | grep -E 'Command Line Tools' \
        | awk -F'*' '{print $2}' | sed 's/^ *Label: //' | tail -n1)
    if [ -n "${LABEL:-}" ]; then
        softwareupdate -i "$LABEL" --verbose
    else
        echo "   softwareupdate found nothing; falling back to GUI prompt."
        xcode-select --install || true
        echo "   If a dialog appeared in the VM display, click Install and re-run this script."
    fi
    rm -f /tmp/.com.apple.dt.CommandLineTools.installondemand.in-progress
fi

echo "==> [2/5] Homebrew..."
if ! command -v brew >/dev/null 2>&1; then
    NONINTERACTIVE=1 /bin/bash -c \
        "$(curl -fsSL https://raw.githubusercontent.com/Homebrew/install/HEAD/install.sh)"
fi
# Make brew available for the rest of this script (Apple Silicon vs Intel paths).
if [ -x /opt/homebrew/bin/brew ]; then eval "$(/opt/homebrew/bin/brew shellenv)"; fi
if [ -x /usr/local/bin/brew  ]; then eval "$(/usr/local/bin/brew shellenv)";  fi

echo "==> [3/5] Node, create-dmg..."
brew install node create-dmg || true

echo "==> [4/5] Rust toolchain..."
if ! command -v cargo >/dev/null 2>&1; then
    curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh -s -- -y
fi
source "$HOME/.cargo/env"

# Build universal binaries (Intel + Apple Silicon) regardless of VM arch.
rustup target add x86_64-apple-darwin aarch64-apple-darwin || true

echo "==> [5/5] Tauri CLI v2..."
cargo install tauri-cli --version "^2.0" --locked || true

# Persist PATH for non-login SSH sessions used by build-remote.sh.
{
    echo 'source "$HOME/.cargo/env" 2>/dev/null || true'
    [ -x /opt/homebrew/bin/brew ] && echo 'eval "$(/opt/homebrew/bin/brew shellenv)"'
    [ -x /usr/local/bin/brew  ] && echo 'eval "$(/usr/local/bin/brew shellenv)"'
} >> "$HOME/.zprofile"

echo
echo "==> Provisioning complete. Versions:"
cargo --version || true
cargo tauri --version || true
node --version || true
echo "Now shut down the VM and export mac_hdd_ng.img (see SETUP.md)."
