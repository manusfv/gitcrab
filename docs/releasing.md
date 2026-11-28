# Releasing

Bump `version` under `[workspace.package]` in `Cargo.toml`, merge it to `main`, then tag that commit:

```sh
git tag v1.0.0
git push origin v1.0.0
```

The [release workflow](../.github/workflows/release.yml) checks that the tag is on `main` and matches the Cargo version, runs the tests on all four targets, builds the archives and installers, and publishes them with `SHA256SUMS` as one GitHub Release, only if every build succeeded. Tags with a hyphen (`v1.0.0-rc.1`) become prereleases. Running the workflow by hand from the Actions tab builds everything without publishing. [Updates](updates.md) explains how the release publishes the in-app update manifests.

## Installers

Installers are made with [cargo-packager](https://github.com/crabnebula-dev/cargo-packager), configured under `[package.metadata.packager]` in `crates/app/Cargo.toml`. To build one locally after `cargo build --release`:

```sh
cargo install --locked cargo-packager --version 0.11.8
cargo packager -p gitcrab --release --formats nsis   # or app,dmg on macOS, appimage on Linux
```
