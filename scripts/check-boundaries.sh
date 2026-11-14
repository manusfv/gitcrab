#!/usr/bin/env bash
# Checks the crate boundaries before a commit:
#
#   1. gitcrab-core has no UI, platform or I/O dependency, including build and
#      test dependencies on every target.
#   2. gitcrab-ui reaches Git and settings only through core's traits: it does
#      not depend on gitcrab-git or gitcrab-settings.
#   3. The I/O crates (gitcrab-git, gitcrab-settings) do not depend on the UI.
#   4. gitcrab-core's tests pass without a window or Git.
#
# Usage: scripts/check-boundaries.sh [--skip-tests]
# Exits non-zero on the first broken rule. CI runs it with --skip-tests
# because the test steps follow.
set -euo pipefail
cd "$(dirname "$0")/.."

skip_tests=false
for arg in "$@"; do
    case "$arg" in
        --skip-tests) skip_tests=true ;;
        *) echo "usage: $0 [--skip-tests]" >&2; exit 2 ;;
    esac
done

failed=0
UI_CRATES='gpui|gpui-component|gitcrab-ui'
IO_CRATES='notify|async-channel|windows-sys|libc|gitcrab-git|gitcrab-settings'

# Reports every dependency of `package` whose name matches `pattern`.
# A failing `cargo tree` stops the script rather than passing silently.
forbid() {
    local package=$1 pattern=$2 why=$3
    shift 3
    local tree found
    if ! tree=$(cargo tree --locked -p "$package" --target all --prefix none --format '{p}' "$@"); then
        echo "FAIL: could not read the dependency tree of $package" >&2
        exit 1
    fi
    # Package names, without the crate itself.
    found=$(echo "$tree" | awk '{print $1}' | grep -vx "$package" | sort -u | grep -Ex "$pattern" || true)
    if [ -n "$found" ]; then
        echo "FAIL: $package $why:" >&2
        echo "$found" | sed 's/^/  - /' >&2
        failed=1
    else
        echo "ok:   $package $why: none"
    fi
}

forbid gitcrab-core "($UI_CRATES|$IO_CRATES)" "UI, platform or I/O dependencies" \
    --all-features -e normal,build,dev
forbid gitcrab-ui "gitcrab-git|gitcrab-settings" "direct dependencies on I/O crates" \
    --all-features -e normal --depth 1
forbid gitcrab-git "$UI_CRATES" "UI dependencies" --all-features -e normal,build
forbid gitcrab-settings "$UI_CRATES" "UI dependencies" --all-features -e normal,build

if [ "$failed" -ne 0 ]; then
    echo "Crate boundaries broken. See the rules at the top of $0." >&2
    exit 1
fi

if [ "$skip_tests" = false ]; then
    cargo test --locked -p gitcrab-core
fi
echo "Crate boundaries hold."
