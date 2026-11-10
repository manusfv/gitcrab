<h1>
  <picture>
    <source media="(prefers-color-scheme: dark)" srcset="icons/gitcrab-lockup-dark.svg">
    <img alt="GitCrab" src="icons/gitcrab-lockup-light.svg" width="300" height="100">
  </picture>
</h1>

**A native Git desktop client for exploring history, reviewing diffs, and preparing commits.**

Built in Rust with [GPUI](https://github.com/zed-industries/zed/tree/main/crates/gpui). GitCrab brings a lane-based commit graph, working changes, and commit details into one workspace.

[Get started](#get-started) · [Features](#features) · [Keyboard shortcuts](#keyboard-shortcuts) · [Development](#development)

## Features

- **Follow the history.** Browse branches and tags, inspect commit metadata and changed files, and search loaded history. Load more commits in 500-commit increments.
- **Customize history columns.** Drag headers to reorder and dividers to resize. Right-click a header to show or hide columns. GitCrab saves the layout automatically, and Reset columns restores the defaults.
- **Prepare a focused commit.** Review staged and unstaged changes separately. Stage or unstage files, all files, or complete hunks, then write a summary and description.
- **Read changes your way.** Switch between unified and positional split diffs, inspect original line numbers, highlight whitespace, navigate hunks, and select or copy lines across virtualized rows.
- **Review local Git operations.** Create branches, check out revisions, merge, cherry-pick, revert, stash, and pop with repository and target context before confirming.
- **Make room for your work.** Resize panels and adjust interface density, text size, and diff appearance. Save preferences for your next session.

The interface renders through GPUI without a WebView. Git operations run on a dedicated worker, and the Git, graph, and diff code can be tested independently of the desktop renderer.

## Get started

### Build and run

You need [stable Rust](https://www.rust-lang.org/tools/install), [Git](https://git-scm.com/downloads), and the native prerequisites below.

```sh
git clone https://github.com/manusfv/gitcrab.git
cd gitcrab
cargo run --locked --release --bin gitcrab -- /path/to/repository
```

Replace `/path/to/repository` with a local Git repository. Quote paths containing spaces. Omit the path to reopen the saved repository, or use the native folder picker when no saved repository is available.

To build without launching:

```sh
cargo build --locked --release --bin gitcrab
```

The executable is `target/release/gitcrab` on Linux and macOS, or `target/release/gitcrab.exe` on Windows. Git must also be installed on the machine running the executable. Windows release builds launch without a console window. Debug builds keep the console for diagnostics.

### Platform requirements

| Platform | Native requirements |
| --- | --- |
| Windows x64 | Rust/MSVC, Visual Studio C++ tools, and the Windows SDK. Release builds need the SDK shader compiler; use `GPUI_FXC_PATH` if discovery needs an explicit path. |
| Linux x64 | The development libraries below, a supported Vulkan driver, and a working desktop session. |
| macOS Apple Silicon / Intel | Stable Rust and full Xcode/Metal tools. Each architecture has a separate build. |

On Ubuntu 24.04, install the native build dependencies:

```sh
sudo apt-get install build-essential pkg-config clang libclang-dev libasound2-dev \
  libudev-dev libfontconfig1-dev libfreetype-dev libxkbcommon-dev \
  libxkbcommon-x11-dev libwayland-dev libxcb1-dev libxcb-xkb-dev libx11-xcb-dev \
  libvulkan-dev mesa-vulkan-drivers
```

### Your first commit

1. Open a local repository and select **Working changes** to inspect the index and worktree.
2. Select a file to review its diff. Stage the intended files or hunks.
3. Enter a commit summary and optional description, then commit the staged changes.
4. Select a commit in the graph to inspect its metadata, changed files, and diff.

Use GitCrab alongside your existing Git tools. Remote operations and advanced conflict workflows still require another client or the command line.

## Keyboard shortcuts

| Action | Windows / Linux | macOS |
| --- | --- | --- |
| Open a repository | `Ctrl+O` | `Cmd+O` |
| Find in loaded history | `Ctrl+F` | `Cmd+F` |
| Select previous / next history row | `↑` / `↓` | `↑` / `↓` |
| Refresh repository state | `F5` | `F5` |
| Commit staged changes | `Ctrl+Enter` | `Cmd+Enter` |
| Select all / copy diff lines | `Ctrl+A` / `Ctrl+C` | `Cmd+A` / `Cmd+C` |

Shortcuts act in the relevant view or editor. Commit submission requires a valid draft and staged changes. Commit context menus expose additional local operations.

## Project status

GitCrab is under active development. The current focus is local repository workflows.

- **Still to come:** fetch/pull/push, remote management, an integrated conflict editor with continue/abort, line staging, amend/rebase/reset UI, and hosting integrations.
- **Diff limits:** split alignment is positional and highlighting is lexical. Large patches remain resident in memory even though their displayed rows are virtualized. Reviewed hunk discard supports ordinary LF UTF-8 worktree modifications and rejects unsupported transformations.
- **Execution limits:** Git subprocesses and hooks do not yet have cancellation or timeouts.
- **Native acceptance:** screen-reader support, IME/dead-key input, interactive Wayland, multiple monitor/DPI transitions, and oldest supported OS/driver combinations need further validation.

## Development

Run these from the repository root:

```sh
# Check generated design tokens and Rust formatting.
node scripts/generate-tokens.mjs --check
cargo fmt --all -- --check

# Exercise the framework-independent Git, graph, diff, and controller code.
cargo test --locked --no-default-features --lib

# Build the production desktop application.
cargo build --locked --release --bin gitcrab
```

Node.js is needed for the design-token check, not to run the desktop application.

For renderer validation, build with the separate `native-evidence` feature. It enables deterministic view selection and readiness markers. The smoke harness requires PowerShell and a working desktop session. On Linux it uses X11 and `scrot`.

```sh
# Use an isolated output directory to preserve the production build.
cargo build --locked --release --features native-evidence --bin gitcrab --target-dir target/native-evidence
pwsh -File scripts/smoke-gpui.ps1 -Executable "$PWD/target/native-evidence/release/gitcrab"
```

On Windows, append `.exe` to the smoke executable path. The harness refuses an existing output destination. Use its `-OutputRoot` parameter with a new path for subsequent runs.

After building the production binary, run `pwsh -File scripts/package-gpui.ps1 -Name <platform>` to create a local development archive and checksum under `dist/gpui-packages`. Use a platform name such as `windows-x64`. The package includes dependency license notices.

### Contributing

Bug reports, focused fixes, tests, and documentation improvements are welcome. [Open an issue](https://github.com/manusfv/gitcrab/issues) with your OS, revision, reproduction steps, and relevant Git errors. Use a disposable repository when reporting a mutation issue, and remove private paths and repository content from logs and screenshots.

Before a pull request, run the checks above and describe which behaviors and platforms you verified. Keep renderer evidence separate from production packages.

## Acknowledgements

## License

[MIT](LICENSE). Third-party components retain their respective licenses. See the vendored [gpui-component](vendor/gpui-component/GITCRAB-PATCH.md) and [xattr](vendor/xattr/GITCRAB-PATCH.md) patch notes for provenance.
