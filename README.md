# GitCrab

An independent local Git desktop client built with Tauri 2, Rust, React and TypeScript. Original graph topology, branding and assets, with a compact dark interface informed by recorded the reference app screenshots.


## Run

Install Node 24 LTS, stable Rust, Git and the native prerequisites, then:

```sh
npm ci
npm run tauri -- dev
# Development with a startup repository:
npm run tauri -- dev -- /path/to/repository
# Embedded local assets and a release executable:
npm run tauri -- build --no-bundle -- --locked
```

The compiled executable is `target/release/gitcrab` (`gitcrab.exe` on Windows). Launch it with a repository directory as the first argument or use the native folder picker. Windows paths with spaces must be quoted. Frontend assets are embedded; Node is a build dependency, not an end-user dependency. Git and the platform WebView runtime remain required.

The repository-local Windows Rust wrapper remains available for this workspace; see platform notes for its isolated GNU setup. Standard Windows builds use MSVC.

## Workflows

- Select commits for full IDs, parents, author/date, refs and changed files. File diffs compare with the first parent; root commits work too.
- Select WIP for separate unstaged/index lists and a commit editor. Stage or unstage a file, all files or an individual complete hunk. Hunk operations affect only the index. Renames retain both paths.
- Open files in the central unified/split diff, with original blob line numbers and a retained staging/details panel. The navigation collapses to a rail. Large history and diff lists render visible rows.
- Create/switch/merge branches, checkout commits, cherry-pick, revert, stash and pop through explicit target-review dialogs. Double-clicking a local branch reviews checkout. Commit context menus expose local operations. No remote writes or destructive discard/reset actions are exposed.
- Use Cmd+O on macOS or Ctrl+O elsewhere for the native picker, F5 to refresh, and arrow keys inside history to select commits. Search dims unmatched rows while preserving topology. Drag panel dividers or focus them and use arrow keys. Text uses normal selectable DOM controls and clipboard behavior.
- History loads a bounded prefix of 500 commits at a time. Full identifiers appear in details/tooltips. Refresh is explicit; there is no idle polling loop.

Git errors appear in a selectable error banner. Failed operations refresh status, including conflict state, while preserving unsent commit text. Conflict resolution and continue/abort require external Git tools.

## Validate and package

```sh
cargo fmt --all -- --check
cargo test --locked --lib
npm test
npm run build
# Native renderer test executable; NEVER package this feature:
cargo build --locked --features desktop,custom-protocol,native-smoke
node scripts/webdriver-smoke.mjs
```

On Windows, `node scripts/native-e2e.mjs target/release/gitcrab.exe` exercises the actual production WebView with a disposable Git fixture. `scripts/capture-evidence.ps1` records the showcase through WebView2. Both require `npm ci`. Linux smoke runs under Xvfb. The optional renderer automation plugin is debug-only and excluded from release packages by a compile-time guard.

The [four-target workflow](https://github.com/manusfv/gitcrab/actions/workflows/ci.yml) builds Windows x64, Linux x64 and separate Apple Silicon/Intel Mac packages. Artifacts are unsigned development packages with SHA-256 checksums. See [architecture](docs/architecture.md).

Known limits: no command cancellation/timeouts, line staging, full conflict lifecycle, persisted settings, filesystem watcher, complete accessibility or semantic split alignment. The measured Windows fixtures do not establish broad 60 Hz performance or low memory on other hardware.
