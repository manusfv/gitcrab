<h1 align="center">
  <picture>
    <source media="(prefers-color-scheme: dark)" srcset="icons/gitcrab-lockup-dark.svg">
    <img alt="GitCrab" src="icons/gitcrab-lockup-light.svg" width="300" height="100">
  </picture>
</h1>

<p align="center"><strong>A native Git desktop client for exploring history, reviewing diffs, and staging hunks.</strong></p>

<p align="center">
  <a href="#install">Install</a> ·
  <a href="#what-you-can-do">What you can do</a> ·
  <a href="#contributing">Contributing</a>
</p>

![GitCrab showing the commit graph and the feature/diff branch preview while main stays checked out](docs/images/gitcrab-workspace.jpg)

## Install

Download GitCrab from [GitHub Releases](https://github.com/manusfv/gitcrab/releases), or [build it from source](#build-from-source). Every platform also needs [Git 2.43 or newer](https://git-scm.com/downloads) on PATH.

| System | Installer (recommended) | Standalone binary |
| --- | --- | --- |
| macOS, Apple Silicon | `gitcrab-macos-arm64.dmg` | `gitcrab-macos-arm64.tar.gz` |
| macOS, Intel | `gitcrab-macos-x86_64.dmg` | `gitcrab-macos-x86_64.tar.gz` |
| Windows 10/11, x64 | `gitcrab-windows-x86_64-setup.exe` | `gitcrab-windows-x86_64.zip` |
| Linux, x64 | `gitcrab-linux-x86_64.AppImage` | `gitcrab-linux-x86_64.tar.gz` |

Not sure which Mac you have? Apple menu → **About This Mac**: "Apple M…" is Apple Silicon, "Intel" is Intel. Windows and Linux on ARM are not built yet.

- **macOS:** open the `.dmg` and drag **GitCrab** onto **Applications**.
- **Windows:** run the setup `.exe`. It installs for the current user without an administrator prompt and adds a Start menu entry. The installer and binary are not code-signed, so SmartScreen may warn: choose **More info → Run anyway**.
- **Linux:** `chmod +x gitcrab-linux-x86_64.AppImage`, then run it. It needs a working Vulkan driver.
- **Standalone binaries:** extract the archive and run `gitcrab` (`gitcrab.exe` on Windows) from anywhere; nothing else is installed. The Windows binary needs the [Microsoft Visual C++ Redistributable](https://learn.microsoft.com/cpp/windows/latest-supported-vc-redist) (`VCRUNTIME140.dll`), which most systems already have. The Linux binary is built on Ubuntu 24.04, needs glibc 2.39 or newer and the system's X11/Wayland, xkbcommon, fontconfig, freetype and Vulkan libraries; if it doesn't start, `ldd gitcrab | grep "not found"` lists what's missing.
- **Checksums:** each release has a `SHA256SUMS` file. Check a download with `sha256sum -c SHA256SUMS --ignore-missing` (`shasum -a 256 -c SHA256SUMS --ignore-missing` on macOS).

**macOS Gatekeeper.** Each release's notes say whether its macOS builds are signed with an Apple Developer ID and notarized. Until they are, macOS blocks the first launch with "GitCrab can't be opened" or "Apple could not verify…". Open it anyway once with **System Settings → Privacy & Security → Open Anyway** (on macOS 14 and older you can also Control-click the app and choose **Open**), or remove the download quarantine:

```sh
xattr -dr com.apple.quarantine /Applications/GitCrab.app   # or the extracted gitcrab binary
```

Only do this for a download you have verified against `SHA256SUMS`.

Launch GitCrab, choose a local repository, and confirm that you trust its Git configuration and metadata. GitCrab remembers your approval across restarts and asks again if the resolved repository paths change. It offers to reopen the last repository on your next launch.

## What you can do

- **Explore history.** Follow branches and tags through the commit graph, search loaded commits, and open a commit to inspect its metadata and changed files. Open multiple repositories in tabs and drag to reorder tabs or history columns.
- **Review changes.** Switch between unified and side-by-side diffs, jump between hunks, and show whitespace when it matters. Checkout, merge, cherry-pick, revert, stash, pull and push run in one click, each checked against the repository first; everything but push can be reversed with Undo.
- **Prepare a commit.** Open **Working changes**, select a file, and use **Stage hunk** on the change you want to include, or select rows and stage, unstage or discard just those lines. Review the staged diff, write a summary and optional description, then commit.
- **Stage or unstage the whole worktree.** The bulk controls act on changes Git finds when the command runs, including files that arrived after the last refresh. A single-file action rechecks the displayed status and rename before updating the index.

GitCrab opens trusted local repositories. Trusted Git configuration can start
programs such as hooks, filters, and signing tools. Use a trusted Git installation
on PATH, or set `GITCRAB_GIT` to an absolute executable path. The runner ignores
inherited `GIT_*` variables and reads normal system, user, and repository config.
Commit drafts stay in memory in their tab. Closing a tab or quitting with
drafts asks before discarding them.

### Split diff

![Side-by-side diff of a staged file, with removed lines on the left, added lines on the right, and controls for unstaging each hunk](docs/images/gitcrab-split-diff.jpg)

### Keyboard shortcuts

| Action | Windows / Linux | macOS |
| --- | --- | --- |
| Open a repository | `Ctrl+O` | `Cmd+O` |
| Open a new repository tab | `Ctrl+T` | `Cmd+T` |
| Find in loaded history | `Ctrl+F` | `Cmd+F` |
| Select the previous or next history row | `↑` / `↓` | `↑` / `↓` |
| Refresh the repository | `F5` | `F5` |
| Commit staged changes | `Ctrl+Enter` | `Cmd+Enter` |
| Stage or unstage the selected working file | `Alt+S` | `Option+S` |
| Select all or copy diff lines | `Ctrl+A` / `Ctrl+C` | `Cmd+A` / `Cmd+C` |

## Project status

GitCrab is under active development. It fetches, pulls (fast-forward, merge or rebase) and pushes through your installed Git, using your credential helper and SSH agent; it never force-pushes. Remote management, clone, and an integrated conflict editor are future work. Staging supports whole files and hunks.

<details>
<summary>Current technical limits</summary>

- Side-by-side diffs align changes by position. Patches over 8 MiB, 50,000 lines, or 8,192 bytes on a line are handled with whole-file actions.
- Hunk discard is limited to supported UTF-8 worktree changes with LF line endings.
- Write commands, their hooks, and signing tools run to completion. Reads have deadlines and cancellation.
- Screen readers, IME and dead-key input, Wayland interaction, and multi-monitor DPI transitions need further validation.

</details>

## Contributing

GitCrab is written in Rust with a native [GPUI](https://github.com/zed-industries/zed/tree/main/crates/gpui) interface. The Git, graph, and diff code can be tested independently of the desktop renderer.

Bug reports and focused pull requests are welcome. Include your OS, the GitCrab revision, and steps to reproduce when reporting a bug. [Open an issue](https://github.com/manusfv/gitcrab/issues/new)

### Build from source

Use [Rust 1.99.1](https://www.rust-lang.org/tools/install), [Git 2.43 or newer](https://git-scm.com/downloads), and the native build tools for your platform.

<details>
<summary>Platform prerequisites</summary>

#### Windows x64

Use the Rust MSVC toolchain, Visual Studio C++ build tools, and the Windows SDK. Release builds require the SDK shader compiler. If discovery fails, set `GPUI_FXC_PATH` to the path of `fxc.exe`.

#### Linux x64

Use a desktop session with a working Vulkan driver. On Ubuntu 24.04, install the native dependencies with the following command.

```sh
sudo apt-get install build-essential pkg-config clang libclang-dev libasound2-dev \
  libudev-dev libfontconfig1-dev libfreetype-dev libxkbcommon-dev \
  libxkbcommon-x11-dev libwayland-dev libxcb1-dev libxcb-xkb-dev libx11-xcb-dev \
  libvulkan-dev mesa-vulkan-drivers
```

#### macOS · Apple Silicon and Intel

Install full Xcode for the Metal build tools. Build separately for each architecture.

</details>

```sh
git clone https://github.com/manusfv/gitcrab.git
cd gitcrab
cargo run --locked --release
```

To open a specific local repository, append `-- "/path/to/repository"` to the `cargo run` command.

### Development commands

```sh
cargo fmt --all -- --check
scripts/check-boundaries.sh   # crate boundaries + core tests; run before each commit
cargo test --locked --workspace --all-features
cargo build --locked --release --bin gitcrab
```

The clean-checkout CI matrix pins Rust 1.99.1 and runs these checks on Windows
MSVC, Linux x64, Apple Silicon, and Intel macOS. Git worker tests (`gitcrab-git`) create
isolated repositories using the Git on PATH. Set `GITCRAB_TEST_GIT` to an absolute
executable path to test a specific Git installation.

The [architecture notes](docs/architecture.md) describe which module owns
each part of the state and its effects.

CI injects the verified checkout revision into its release binary. Other builds
identify themselves as `local build` or `source archive`. Builds with
`native-evidence` add a diagnostics label.

The release executable is `target/release/gitcrab.exe` on Windows and `target/release/gitcrab` on Linux and macOS. Git is also required at runtime. The optional `native-evidence` feature enables deterministic view selection and readiness markers for renderer diagnostics.

### Releasing

Bump `version` under `[workspace.package]` in `Cargo.toml`, merge it to `main`, then tag that commit:

```sh
git tag v1.0.0
git push origin v1.0.0
```

The [release workflow](.github/workflows/release.yml) checks that the tag is on `main` and matches the Cargo version, runs the tests on all four targets, builds the archives and installers, and publishes them with `SHA256SUMS` as one GitHub Release, only if every build succeeded. Tags with a hyphen (`v1.0.0-rc.1`) become prereleases. Running the workflow by hand from the Actions tab builds everything without publishing.

Installers are made with [cargo-packager](https://github.com/crabnebula-dev/cargo-packager), configured under `[package.metadata.packager]` in `crates/app/Cargo.toml`. To build one locally after `cargo build --release`:

```sh
cargo install --locked cargo-packager --version 0.11.8
cargo packager -p gitcrab --release --formats nsis   # or app,dmg on macOS, appimage on Linux
```

macOS signing and notarization run only when these repository secrets are set; without them the macOS builds are unsigned. `APPLE_CERTIFICATE` (base64 `.p12` export of a Developer ID Application certificate and its key), `APPLE_CERTIFICATE_PASSWORD`, `APPLE_SIGNING_IDENTITY` (for example `Developer ID Application: Name (TEAMID)`), and for notarization an App Store Connect API key: `APPLE_API_KEY` (key ID), `APPLE_API_ISSUER` (issuer ID) and `APPLE_API_KEY_P8` (contents of the `.p8` file). The workflow records in the release notes what `codesign`, `spctl` and `stapler` verified.

## License and acknowledgements

GitCrab is licensed under [MIT](LICENSE). See [THIRD_PARTY.md](THIRD_PARTY.md) for third-party material.
