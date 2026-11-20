#!/usr/bin/env bash
# Prints the update manifest for a release in the static JSON format that
# cargo-packager-updater reads: version, notes, pub_date, and one entry per
# `<os>-<arch>` key with the bundle URL, the contents of its .sig file and the
# update format. Every platform must have its bundle and signature, so a
# manifest never sends one platform to an update that does not exist.
#
# Usage: scripts/update-manifest.sh <tag> <notes-file> <dist-dir>
# PUB_DATE (RFC 3339) defaults to now; GITHUB_REPOSITORY to manusfv/gitcrab.
set -euo pipefail

if (( $# != 3 )); then
  echo "usage: $0 <tag> <notes-file> <dist-dir>" >&2
  exit 2
fi
tag=$1
notes=$2
dist=$3
pub_date=${PUB_DATE:-$(date -u +%Y-%m-%dT%H:%M:%SZ)}
base="https://github.com/${GITHUB_REPOSITORY:-manusfv/gitcrab}/releases/download/$tag"

fail() { echo "error: $*" >&2; exit 1; }
[[ "$tag" == v* ]] || fail "tag $tag does not start with v"
[[ -f "$notes" ]] || fail "notes file $notes not found"

# Updater key, release asset, update format. The keys come from the updater's
# target detection (linux|macos|windows, x86_64|aarch64), so they differ from
# the asset names for Apple silicon.
platforms=(
  "windows-x86_64 gitcrab-windows-x86_64-setup.exe nsis"
  "linux-x86_64 gitcrab-linux-x86_64.AppImage appimage"
  "macos-aarch64 gitcrab-macos-arm64.app.tar.gz app"
  "macos-x86_64 gitcrab-macos-x86_64.app.tar.gz app"
)

entries='{}'
for platform in "${platforms[@]}"; do
  read -r key asset format <<<"$platform"
  [[ -f "$dist/$asset" ]] || fail "$dist/$asset is missing"
  [[ -s "$dist/$asset.sig" ]] || fail "$dist/$asset.sig is missing or empty"
  entries=$(jq --arg key "$key" --arg url "$base/$asset" --arg format "$format" \
    --rawfile sig "$dist/$asset.sig" \
    '.[$key] = {url: $url, signature: ($sig | gsub("\\s"; "")), format: $format}' <<<"$entries")
done

jq -n --arg version "${tag#v}" --rawfile notes "$notes" --arg pub_date "$pub_date" \
  --argjson platforms "$entries" \
  '{version: $version, notes: $notes, pub_date: $pub_date, platforms: $platforms}'
