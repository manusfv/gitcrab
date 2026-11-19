#!/usr/bin/env bash
# Checks the .app and .dmg, then prints their signing status as the last line
# of stdout. The status reports only what codesign, spctl and stapler verified.
#
# Usage: scripts/verify-macos-packages.sh [--expect-signed] [--expect-notarized] <GitCrab.app> <GitCrab.dmg>
set -euo pipefail

expect_signed=false
expect_notarized=false
while [[ "${1:-}" == --* ]]; do
  case "$1" in
    --expect-signed) expect_signed=true ;;
    --expect-notarized) expect_signed=true; expect_notarized=true ;;
    *) echo "error: unknown option $1" >&2; exit 2 ;;
  esac
  shift
done
app=$1
dmg=$2

# Tool output goes to stderr; stdout carries only the status.
exec 3>&1 1>&2
fail() { echo "error: $*"; exit 1; }

[[ -x "$app/Contents/MacOS/gitcrab" ]] || fail "$app/Contents/MacOS/gitcrab is missing or not executable"
plutil -lint "$app/Contents/Info.plist"
hdiutil verify "$dmg"

mnt=$(mktemp -d)
hdiutil attach "$dmg" -nobrowse -readonly -mountpoint "$mnt"
trap 'hdiutil detach "$mnt" >/dev/null 2>&1 || true' EXIT
[[ -d "$mnt/GitCrab.app" ]] || fail "the DMG does not contain GitCrab.app"
[[ -L "$mnt/Applications" ]] || fail "the DMG has no Applications link"

status="unsigned (not notarized)"
signature=$(codesign -dv --verbose=4 "$app" 2>&1 || true)
if grep -q '^Authority=Developer ID Application' <<<"$signature"; then
  codesign --verify --deep --strict --verbose=2 "$app"
  status="signed with Developer ID, not notarized"
  if xcrun stapler validate "$dmg" && xcrun stapler validate "$app" \
    && spctl --assess --type execute --verbose "$app" \
    && spctl --assess --type open --context context:primary-signature --verbose "$dmg"; then
    status="signed with Developer ID and notarized"
  fi
fi

if [[ "$expect_signed" == true && "$status" == unsigned* ]]; then
  fail "signing was configured but the app is not Developer ID signed"
fi
if [[ "$expect_notarized" == true && "$status" != *"and notarized" ]]; then
  fail "notarization was configured but could not be verified"
fi
echo "$status" >&3
