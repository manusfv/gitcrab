# Updates

GitCrab checks for updates with
[`cargo-packager-updater`](https://docs.rs/cargo-packager-updater/0.2.3).
The app downloads a JSON manifest for its channel, compares the version with
its own, and installs the bundle for its platform after verifying the bundle's
minisign signature against the public key built into the app.

## How a release publishes an update

`.github/workflows/release.yml` does it on every `v*` tag:

1. **Build.** When the `UPDATER_PRIVATE_KEY` and
   `UPDATER_PRIVATE_KEY_PASSWORD` secrets are set, `cargo packager` signs each
   package and writes a `.sig` file next to it. For the macOS `.app` it first
   packs `GitCrab.app.tar.gz` and signs that. Without the secrets the build
   warns and the packages have no signatures; with only one of them it fails.
2. **Release.** The update bundles and their signatures are release assets
   next to the installers people download:

   | Updater key      | Bundle                                  | Format     |
   | ---------------- | --------------------------------------- | ---------- |
   | `windows-x86_64` | `gitcrab-windows-x86_64-setup.exe`      | `nsis`     |
   | `linux-x86_64`   | `gitcrab-linux-x86_64.AppImage`         | `appimage` |
   | `macos-aarch64`  | `gitcrab-macos-arm64.app.tar.gz`        | `app`      |
   | `macos-x86_64`   | `gitcrab-macos-x86_64.app.tar.gz`       | `app`      |

   Each bundle has a `<bundle>.sig` beside it. The `.app.tar.gz` files exist
   only for the updater; people install from the `.dmg`.
3. **Manifests.** After `gh release create` succeeds,
   `scripts/update-manifest.sh` writes the manifest and the workflow commits it
   to the `update-manifests` branch, an orphan branch that holds only the
   manifests. If signing is not configured this step is skipped with a
   warning, because the updater rejects unsigned bundles.

## Channels

| Channel    | URL                                                                              |
| ---------- | -------------------------------------------------------------------------------- |
| stable     | `https://raw.githubusercontent.com/manusfv/gitcrab/update-manifests/stable.json`     |
| prerelease | `https://raw.githubusercontent.com/manusfv/gitcrab/update-manifests/prerelease.json` |

A tag with a hyphen (`v0.2.0-rc.1`) is a prerelease and updates only
`prerelease.json`. Any other tag updates both, so prerelease users also get
stable releases. A channel never moves to a lower version, so re-running an
old tag's release leaves it alone. Re-running the current tag's release
rewrites the same manifest and commits nothing.

raw.githubusercontent.com caches files for a few minutes, so a new manifest
can take that long to reach the app.

## Manifest format

This is the updater's static format. The keys of `platforms` are
`<os>-<arch>` as the updater detects them at run time: `os` is `windows`,
`linux` or `macos`, and `arch` is `x86_64` or `aarch64`. `signature` is the
contents of the bundle's `.sig` file. `pub_date` is RFC 3339.

```json
{
  "version": "0.2.0",
  "notes": "Release notes in Markdown",
  "pub_date": "2026-11-20T10:00:00Z",
  "platforms": {
    "windows-x86_64": {
      "url": "https://github.com/manusfv/gitcrab/releases/download/v0.2.0/gitcrab-windows-x86_64-setup.exe",
      "signature": "<contents of gitcrab-windows-x86_64-setup.exe.sig>",
      "format": "nsis"
    },
    "linux-x86_64": {
      "url": "https://github.com/manusfv/gitcrab/releases/download/v0.2.0/gitcrab-linux-x86_64.AppImage",
      "signature": "<contents of gitcrab-linux-x86_64.AppImage.sig>",
      "format": "appimage"
    },
    "macos-aarch64": {
      "url": "https://github.com/manusfv/gitcrab/releases/download/v0.2.0/gitcrab-macos-arm64.app.tar.gz",
      "signature": "<contents of gitcrab-macos-arm64.app.tar.gz.sig>",
      "format": "app"
    },
    "macos-x86_64": {
      "url": "https://github.com/manusfv/gitcrab/releases/download/v0.2.0/gitcrab-macos-x86_64.app.tar.gz",
      "signature": "<contents of gitcrab-macos-x86_64.app.tar.gz.sig>",
      "format": "app"
    }
  }
}
```

The `notes` are the published release's notes, and `pub_date` is its publish
time. To build a manifest by hand from downloaded assets:

```sh
scripts/update-manifest.sh v0.2.0 notes.md dist/ > stable.json
```

## Maintainer setup

Do this once, before the first release that should update itself.

1. Install the packager the workflow uses and generate a key pair with a
   password. Keep the private key and password out of the repository, for
   example in a password manager:

   ```sh
   cargo install --locked cargo-packager --version 0.11.8
   cargo packager signer generate --path ~/.gitcrab-updater.key
   ```

   This writes the private key to `~/.gitcrab-updater.key` and the public key
   to `~/.gitcrab-updater.key.pub`.

2. Add the private key and its password as repository secrets:

   ```sh
   gh secret set UPDATER_PRIVATE_KEY < ~/.gitcrab-updater.key
   gh secret set UPDATER_PRIVATE_KEY_PASSWORD
   ```

3. Put the contents of `~/.gitcrab-updater.key.pub` in the gitcrab-update
   crate as the updater's public key. Builds made before that change cannot
   verify, and so cannot install, signed updates.

If the private key is lost or leaked, generate a new pair, replace both
secrets and the public key. Installed apps keep trusting only the old key, so
their users must install the next version by hand.
