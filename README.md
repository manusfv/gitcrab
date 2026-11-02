# GitCrab

A native Rust Git desktop client inspired by the reference app's Git workflows. GPUI renders the main interface without a WebView. Git operations, history topology, patch parsing and the bounded worker remain framework independent.


The native workspace polish adds consistent iconography, clearer Git context and selection, a guided staging/commit flow, and explicit diff modes.

## Run

Install stable Rust, Git and the native prerequisites:

```sh
cargo run --locked --release --bin gitcrab-gpui -- /path/to/repository
cargo test --locked --no-default-features --lib
```

On Windows, quote paths containing spaces. The executable is `target/release/gitcrab-gpui.exe`; Linux/macOS omit `.exe`. Start without a repository to use the native folder picker. This workspace's provisioned GNU installation can be used through `scripts/cargo-gpui-local.ps1`; Windows release builds also require the SDK shader compiler via `GPUI_FXC_PATH`.

## Workflows

- Navigate branches/tags, search history, select commits with pointer/arrow keys, inspect metadata and open first-parent/root diffs. Load history in bounded 500-commit increments.
- Select WIP for separate unstaged/index lists and commit editing. Stage/unstage files, all files or complete hunks without discarding worktree contents.
- Review unified or positional split diffs with original blob line numbers, lexical colors, hunk navigation, whitespace emphasis, horizontal scrolling and explicit line-range selection/copy across virtual rows.
- Review create-branch, checkout, merge, cherry-pick, revert, stash and pop operations with repository/HEAD/target context. Git errors and conflicts remain visible; state refreshes on success and failure.
- Use Cmd+O on macOS or Ctrl+O elsewhere, F5 to refresh, Cmd/Ctrl+Enter to commit staged changes, and context menus for commit operations. Resize panels and use Appearance for density, text size and keyboard width controls. Explicitly save preferences to the platform configuration directory.

No destructive discard/reset UI or remote writes are exposed. Conflict resolution/continue/abort, line staging, rebase/reset UI, hosting integrations and full the reference app parity remain future work. See the capability matrix.

## Validation and development packages

```sh
cargo fmt --all -- --check
cargo test --locked --no-default-features --lib
cargo build --locked --release --bin gitcrab-gpui
# Separate evidence executable, never the packaged production binary:
cargo build --locked --features native-evidence --bin gitcrab-gpui
pwsh -File scripts/smoke-gpui.ps1
```

The [native workflow](https://github.com/manusfv/gitcrab/actions/workflows/gpui.yml) targets Windows x64, Linux x64, macOS ARM64 and macOS Intel, with real renderer smoke runs and development packages. Check the exact run before claiming a platform passed. Packages are unsigned on Windows/Linux and ad-hoc signed on macOS, without notarization.

Read [architecture](docs/architecture.md).

GPUI is the sole desktop host. The runnable Tauri baseline is preserved in Git at `6000266`; the measured baseline and its harness are identified in the evidence ledger. Tauri/React/WebView source, configuration and dependencies have been removed after native workflow parity. Its four frontend tests/build passed before cutover. Historical WebView and egui evidence is labeled separately.
