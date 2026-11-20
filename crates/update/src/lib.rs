//! Update checks behind [`gitcrab_core::updates::Updater`].
//!
//! [`ManifestUpdater`] reads the release manifest of a channel with
//! `cargo-packager-updater`. Each release publishes one manifest per channel
//! in that crate's JSON format (`version`, `notes`, `pub_date` and a
//! `platforms` object keyed `<os>-<arch>`, such as `windows-x86_64`,
//! `linux-x86_64`, `macos-aarch64` or `macos-x86_64`, each with `url`,
//! `signature` and `format`). See `docs/updates.md`.
//!
//! The decisions (which channel, when to check, what counts as newer) come
//! from `gitcrab-core`; this crate only fetches and parses.
use cargo_packager_updater::{
    http::header::USER_AGENT, semver::Version, url::Url, Config, Error, UpdaterBuilder,
};
use gitcrab_core::updates::{self, AvailableUpdate, UpdateChannel, UpdateCheck, Updater};
use std::time::Duration;
use tokio::sync::oneshot;

/// The minisign public key that update bundles are signed with, as printed
/// by `cargo packager signer generate` (the contents of the `.pub` file).
///
/// The maintainer fills this in when release signing is set up; see
/// `docs/updates.md`. Checking for updates does not use it: the key only
/// verifies a downloaded bundle before it is installed, so checks work with
/// this empty placeholder and installing must refuse to run without a key.
pub const UPDATE_PUBLIC_KEY: &str = "";

/// Where each channel's manifest is published: `<base>/<channel>.json`.
const MANIFEST_BASE: &str = "https://raw.githubusercontent.com/manusfv/gitcrab/update-manifests";

/// Overrides the manifest URL, for testing against another server. A
/// `{channel}` in it is replaced by `stable` or `prerelease`.
pub const ENDPOINT_VARIABLE: &str = "GITCRAB_UPDATE_ENDPOINT";

/// Longest time one check may take.
const TIMEOUT: Duration = Duration::from_secs(30);

/// The published manifest of `channel`.
pub fn manifest_url(channel: UpdateChannel) -> String {
    format!("{MANIFEST_BASE}/{}.json", channel.name())
}

/// The manifest URL to read for `channel`, honouring a non-empty override.
///
/// # Errors
///
/// Returns a message if the URL does not parse.
fn endpoint(channel: UpdateChannel, overridden: Option<String>) -> Result<Url, String> {
    let url = overridden
        .map(|url| url.trim().replace("{channel}", channel.name()))
        .filter(|url| !url.is_empty())
        .unwrap_or_else(|| manifest_url(channel));
    Url::parse(&url).map_err(|error| format!("Invalid update address {url}: {error}"))
}

/// Checks the published release manifests on a background thread per check.
#[derive(Clone, Copy, Debug, Default)]
pub struct ManifestUpdater;

impl Updater for ManifestUpdater {
    fn check(&self, channel: UpdateChannel, current_version: &str) -> Result<UpdateCheck, String> {
        let current = Version::parse(current_version.trim().trim_start_matches('v'))
            .map_err(|error| format!("Invalid version {current_version}: {error}"))?;
        let url = endpoint(channel, std::env::var(ENDPOINT_VARIABLE).ok())?;
        let (reply, result) = oneshot::channel();
        std::thread::Builder::new()
            .name("gitcrab-update-check".into())
            .spawn(move || {
                let _ = reply.send(check_manifest(current, url));
            })
            .map_err(|error| format!("Could not start the update check: {error}"))?;
        Ok(result)
    }
}

/// Reads the manifest at `url` and returns its release if it is newer than
/// `current`. Blocks while the request runs.
fn check_manifest(current: Version, url: Url) -> Result<Option<AvailableUpdate>, String> {
    let shown = url.to_string();
    let updater = UpdaterBuilder::new(
        current,
        Config {
            endpoints: vec![url],
            pubkey: UPDATE_PUBLIC_KEY.into(),
            windows: None,
        },
    )
    .version_comparator(|current, release| {
        updates::is_newer(&release.version.to_string(), &current.to_string())
    })
    .timeout(TIMEOUT)
    .header(USER_AGENT, concat!("GitCrab/", env!("CARGO_PKG_VERSION")))
    .and_then(|builder| builder.build())
    .map_err(|error| error.to_string())?;
    match updater.check() {
        Ok(Some(update)) => Ok(Some(AvailableUpdate::new(
            &update.version,
            update.body.filter(|notes| !notes.trim().is_empty()),
            update.date.map(|date| date.date().to_string()),
        ))),
        Ok(None) => Ok(None),
        // A newer release with no bundle for this platform has nothing to
        // install here, so it is not offered.
        Err(Error::TargetNotFound(_)) => Ok(None),
        // Any unsuccessful status, such as 404 before the first manifest
        // is published.
        Err(Error::ReleaseNotFound) => Err(format!("No release information at {shown}")),
        Err(error) => Err(error.to_string()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{Read, Write};
    use std::net::TcpListener;

    #[test]
    fn channels_read_their_own_manifest() {
        assert_eq!(
            endpoint(UpdateChannel::Stable, None).unwrap().as_str(),
            "https://raw.githubusercontent.com/manusfv/gitcrab/update-manifests/stable.json"
        );
        assert_eq!(
            endpoint(UpdateChannel::Prerelease, None).unwrap().as_str(),
            "https://raw.githubusercontent.com/manusfv/gitcrab/update-manifests/prerelease.json"
        );
    }

    #[test]
    fn an_override_replaces_the_manifest_address() {
        let url = endpoint(
            UpdateChannel::Prerelease,
            Some("http://127.0.0.1:8000/{channel}.json".into()),
        )
        .unwrap();
        assert_eq!(url.as_str(), "http://127.0.0.1:8000/prerelease.json");
        assert_eq!(
            endpoint(UpdateChannel::Stable, Some(" ".into())).unwrap(),
            endpoint(UpdateChannel::Stable, None).unwrap()
        );
        assert!(endpoint(UpdateChannel::Stable, Some("not a url".into())).is_err());
    }

    /// Serves one HTTP response on a local port and returns its address.
    fn serve_once(status: &'static str, body: String) -> Url {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            let mut request = [0; 4096];
            let _ = stream.read(&mut request);
            let response = format!(
                "HTTP/1.1 {status}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                body.len()
            );
            stream.write_all(response.as_bytes()).unwrap();
        });
        Url::parse(&format!("http://{address}/prerelease.json")).unwrap()
    }

    fn manifest(version: &str, platforms: &[&str]) -> String {
        let platforms = platforms
            .iter()
            .map(|key| {
                format!(
                    r#""{key}":{{"url":"https://example.com/{key}","signature":"sig","format":"nsis"}}"#
                )
            })
            .collect::<Vec<_>>()
            .join(",");
        format!(
            r#"{{"version":"{version}","notes":"Fixes","pub_date":"2026-11-02T10:00:00Z","platforms":{{{platforms}}}}}"#
        )
    }

    const ALL_PLATFORMS: &[&str] = &[
        "windows-x86_64",
        "linux-x86_64",
        "macos-aarch64",
        "macos-x86_64",
    ];

    fn version(text: &str) -> Version {
        Version::parse(text).unwrap()
    }

    #[test]
    fn a_newer_release_is_offered_with_its_notes_and_date() {
        let url = serve_once("200 OK", manifest("v0.1.0-alpha.4", ALL_PLATFORMS));
        let update = check_manifest(version("0.1.0-alpha.3"), url)
            .unwrap()
            .unwrap();
        assert_eq!(update.version, "0.1.0-alpha.4");
        assert_eq!(update.notes.as_deref(), Some("Fixes"));
        assert_eq!(update.date.as_deref(), Some("2026-11-02"));
        assert_eq!(
            update.release_url,
            "https://github.com/manusfv/gitcrab/releases/tag/v0.1.0-alpha.4"
        );
    }

    #[test]
    fn the_running_or_an_older_release_is_not_offered() {
        let url = serve_once("200 OK", manifest("0.1.0-alpha.3", ALL_PLATFORMS));
        assert_eq!(check_manifest(version("0.1.0-alpha.3"), url), Ok(None));
        let url = serve_once("200 OK", manifest("0.1.0-alpha.2", ALL_PLATFORMS));
        assert_eq!(check_manifest(version("0.1.0-alpha.3"), url), Ok(None));
    }

    #[test]
    fn a_release_without_this_platform_is_not_offered() {
        let url = serve_once("200 OK", manifest("9.0.0", &["plan9-mips"]));
        assert_eq!(check_manifest(version("0.1.0"), url), Ok(None));
    }

    #[test]
    fn a_missing_manifest_is_an_error() {
        let url = serve_once("404 Not Found", String::new());
        let error = check_manifest(version("0.1.0"), url).unwrap_err();
        assert!(error.starts_with("No release information at http://127.0.0.1:"));
    }
}
