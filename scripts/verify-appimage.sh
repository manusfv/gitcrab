#!/usr/bin/env bash
# Extracts the AppImage (no FUSE needed) and checks its binary, desktop entry
# and icon.
#
# Usage: scripts/verify-appimage.sh <GitCrab.AppImage>
set -euo pipefail

appimage=$(realpath "$1")
root=$(cd "$(dirname "$0")/.." && pwd)
work=$(mktemp -d)
trap 'rm -rf "$work"' EXIT

chmod +x "$appimage"
(cd "$work" && "$appimage" --appimage-extract >/dev/null)
app="$work/squashfs-root"

fail() { echo "error: $*" >&2; exit 1; }

[[ -x "$app/usr/bin/gitcrab" ]] || fail "usr/bin/gitcrab is missing or not executable"
[[ -x "$app/AppRun" ]] || fail "AppRun is missing"

# Desktops match the running window to this file by name, so its basename
# must equal the app_id set in crates/app/src/main.rs.
desktop="$app/usr/share/applications/gitcrab.desktop"
[[ -f "$desktop" ]] || fail "usr/share/applications/gitcrab.desktop is missing"
grep -q 'app_id: Some("gitcrab"' "$root/crates/app/src/main.rs" \
  || fail "app_id in crates/app/src/main.rs no longer matches gitcrab.desktop"
if command -v desktop-file-validate >/dev/null; then
  desktop-file-validate "$desktop"
fi
grep -qx 'Name=GitCrab' "$desktop" || fail "desktop entry Name is not GitCrab"
grep -qE '^Exec=gitcrab( |$)' "$desktop" || fail "desktop entry Exec is not gitcrab"
grep -qx 'Icon=gitcrab' "$desktop" || fail "desktop entry Icon is not gitcrab"
[[ -f "$app/usr/share/icons/hicolor/256x256/apps/gitcrab.png" ]] || fail "256x256 icon is missing"

echo "AppImage OK: $(basename "$appimage")"
