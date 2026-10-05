#!/usr/bin/env bash
# Runs inside the Linux builder container.
#   /src  : project root, mounted read-only (slow Windows 9p mount)
#   /work : fast named volume - we compile here for speed + incremental caching
#   /out  : host dist/ folder, mounted read-write - final artifacts land here
set -euo pipefail

SRC=/src
WORK=/work
OUT=/out

echo "==> Syncing source into build volume..."
rsync -a --delete \
    --exclude '.git' \
    --exclude 'node_modules' \
    --exclude 'dist' \
    --exclude 'target' \
    --exclude 'src-tauri/target' \
    "$SRC"/ "$WORK"/

cd "$WORK"

# Keep Linux build artifacts off the host's Windows/MSVC target dir.
export CARGO_TARGET_DIR="$WORK/src-tauri/target"

echo "==> Building Tauri bundles (deb, appimage)..."
# --bundles overrides the nsis/msi targets pinned in tauri.conf.json (Windows-only).
cargo tauri build --bundles deb,appimage

echo "==> Collecting artifacts into dist/..."
mkdir -p "$OUT"
BUNDLE="$CARGO_TARGET_DIR/release/bundle"
found=0
while IFS= read -r -d '' f; do
    cp -v "$f" "$OUT"/
    found=1
done < <(find "$BUNDLE" -type f \( -name '*.deb' -o -name '*.AppImage' \) -print0)

if [ "$found" -eq 0 ]; then
    echo "!! No .deb / .AppImage produced - check the build log above." >&2
    exit 1
fi

echo "==> Linux build complete."
