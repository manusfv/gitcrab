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

Prebuilt executables will be published on [GitHub Releases](https://github.com/manusfv/gitcrab/releases). Until then, you can [build GitCrab from source](#build-from-source).

1. Install [Git](https://git-scm.com/downloads) so GitCrab can work with your repositories.
2. When release builds are available, download the asset that matches your operating system and processor architecture.
3. Launch GitCrab and choose a local repository. GitCrab reopens that repository on your next launch.

## What you can do

- **Explore history.** Follow branches and tags through the commit graph, search loaded commits, and open a commit to inspect its metadata and changed files. Open multiple repositories in tabs and drag to reorder tabs or history columns.
- **Review changes.** Switch between unified and side-by-side diffs, jump between hunks, and show whitespace when it matters. Review operations such as checkout, merge, cherry-pick, revert, and stash before applying them.
- **Prepare a commit.** Open **Working changes**, select a file, and use **Stage hunk** on the change you want to include. Review the staged diff, write a summary and optional description, then commit.

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
| Select all or copy diff lines | `Ctrl+A` / `Ctrl+C` | `Cmd+A` / `Cmd+C` |

## Project status

GitCrab is under active development and focuses on local repositories. Fetch, pull, push, remote management, and an integrated conflict editor are future work. Staging supports whole files and hunks.

<details>
<summary>Current technical limits</summary>

- Side-by-side diffs align changes by position. Large patches stay in memory while the interface renders only visible rows.
- Hunk discard is limited to supported UTF-8 worktree changes with LF line endings.
- Git commands and hooks run to completion without cancellation or timeouts.
- Screen readers, IME and dead-key input, Wayland interaction, and multi-monitor DPI transitions need further validation.

</details>

## Contributing

GitCrab is written in Rust with a native [GPUI](https://github.com/zed-industries/zed/tree/main/crates/gpui) interface. The Git, graph, and diff code can be tested independently of the desktop renderer.

Bug reports and focused pull requests are welcome. Include your OS, the GitCrab revision, and steps to reproduce when reporting a bug. [Open an issue](https://github.com/manusfv/gitcrab/issues/new)

### Build from source

You’ll need [stable Rust](https://www.rust-lang.org/tools/install), [Git](https://git-scm.com/downloads), and the native build tools for your platform.

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
cargo test --locked --no-default-features --lib
cargo build --locked --release --bin gitcrab
```

The release executable is `target/release/gitcrab.exe` on Windows and `target/release/gitcrab` on Linux and macOS. Git is also required at runtime. The optional `native-evidence` feature enables deterministic view selection and readiness markers for renderer diagnostics.

## License and acknowledgements

GitCrab is licensed under [MIT](LICENSE). See the [gpui-component](vendor/gpui-component/GITCRAB-PATCH.md) and [xattr](vendor/xattr/GITCRAB-PATCH.md) patch notes for third-party provenance.
