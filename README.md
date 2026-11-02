# GitCrab

An independent Rust desktop Git client inspired by the reference app Desktop's compact dark interface and commit graph. Original implementation, branding, graph algorithm, and UI assets.


## Run

The Windows executable built in this workspace is `target\debug\gitcrab.exe`:

```powershell
.\target\debug\gitcrab.exe 'C:\path\to\repository'
```

Start without an argument to enter a local repository folder. Git must be available on PATH. The current build was validated on Windows; cross-platform packaging is unfinished.

With a standard Rust installation and platform linker:

```powershell
cargo run --release -- 'C:\path\to\repository'
cargo test
```

This workspace also contains an isolated Rust GNU toolchain and binutils under the ignored `.toolchain` folder. It does not change system PATH. Use the wrapper with that installation:

```powershell
.\scripts\cargo-local.ps1 build --offline
.\scripts\cargo-local.ps1 run -- fixtures\showcase
.\scripts\cargo-local.ps1 test --offline
```

The local wrapper expects the provisioned `.toolchain` directory and is not a general installer. Rust 1.99.1 and Git 2.53.0.windows.3 were used in this session.

## Use

- Click a commit to inspect its author, parents, references, and changed files. Click a file for its first-parent diff. Merge commits show changes against their first parent.
- Click Working directory to review separate unstaged and staged lists. The `+`/`−` buttons and Stage/Unstage all update the index. Opening a file shows Stage/Unstage file and inline hunk actions.
- Use Unstaged/Staged tabs and Split view to compare the worktree, index, and committed changes. Close the diff with `×` or the left rail button to return to history.
- Enter a summary and optional description, separated by a blank line, then commit the staged changes.
- Double-click a local branch or use the toolbar/context menu for branch creation, checkout, merge, cherry-pick, and revert. Each operation shows its exact target before execution. Commit checkout intentionally detaches HEAD.
- Arrow keys select history rows. Ctrl+O opens a repository; F5 refreshes. Search dims unmatched commits while preserving graph topology. The footer loads another 500 commits when the current page is full.
- Resize the branch and details panels using their dividers. Stash/pop are available; the stash list and advanced rebase/reset UI remain unfinished.

Git errors remain visible. Conflicted files are marked and committing is disabled until they are resolved. Edit conflicts and continue/abort the operation using Git externally; GitCrab does not yet provide a conflict editor or operation sequencer. Commit hooks and signing use normal local Git configuration.

## Evidence and validation

```powershell
# Refuses an existing destination; use -OutputRoot for another location.
.\scripts\create-fixtures.ps1
.\scripts\capture-evidence.ps1
```

The supplied `fixtures` directory is disposable and ignored by Git. Capture runs use real repository state, the actual native renderer, and close each capture process automatically. `GITCRAB_SCREENSHOT`, `GITCRAB_CAPTURE_VIEW` (`history`, `staging`, `diff`, `split`, `commitdiff`), and `GITCRAB_CAPTURE_EXIT` are optional evidence controls.

Read [architecture](docs/architecture.md).

Known limits include line-level staging, complete syntax grammars, conflict resolution UI, command cancellation/timeouts, persisted preferences/recent repositories, and installer packaging. Large-repository scalability has not been established.
