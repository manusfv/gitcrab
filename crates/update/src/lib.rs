//! Updates behind [`gitcrab_core::updates::Updater`].
//!
//! [`ManifestUpdater`] reads the release manifest of a channel with
//! `cargo-packager-updater`. Each release publishes one manifest per channel
//! in that crate's JSON format (`version`, `notes`, `pub_date` and a
//! `platforms` object keyed `<os>-<arch>`, such as `windows-x86_64`,
//! `linux-x86_64`, `macos-aarch64` or `macos-x86_64`, each with `url`,
//! `signature` and `format`). See `docs/updates.md`.
//!
//! The manifest entry the last check found is kept, so a download fetches
//! exactly that bundle and verifies it against that entry's minisign
//! signature and [`UPDATE_PUBLIC_KEY`]. Only a verified bundle is kept for
//! installing, and installing hands it to `cargo-packager-updater`:
//!
//! - Windows: the bundle is the per-user NSIS setup. It is saved to a
//!   temporary file and started with [`WINDOWS_INSTALLER_ARGS`] through a
//!   hidden PowerShell, and this process exits at once. The setup shows no
//!   window and starts GitCrab again when it finishes.
//! - Linux: the bundle is the AppImage. It replaces the file in `$APPIMAGE`,
//!   keeping its permissions; the old one is restored if writing fails.
//! - macOS: the bundle is `GitCrab.app.tar.gz`. It is unpacked to a
//!   temporary folder that then replaces the running `GitCrab.app`.
//!
//! The decisions (which channel, when to check, what counts as newer, which
//! copies can install) come from `gitcrab-core`; this crate fetches, checks
//! signatures, inspects how GitCrab was installed and runs the install.
use cargo_packager_updater::{
    http::header::USER_AGENT, semver::Version, url::Url, Config, Error, Update, UpdaterBuilder,
    WindowsConfig, WindowsUpdateInstallMode,
};
use gitcrab_core::updates::{
    self, AvailableUpdate, DownloadProgress, InstallSupport, Packaging, UpdateChannel, UpdateCheck,
    UpdateDownload, UpdateInstall, Updater,
};
use std::{
    cell::Cell,
    path::{Path, PathBuf},
    sync::{Arc, Mutex, MutexGuard, PoisonError},
    time::Duration,
};
use tokio::sync::{oneshot, watch};

/// The minisign public key update bundles are signed with (the contents of
/// the `.pub` file from `cargo packager signer generate`; see
/// `docs/updates.md`). Checks work while it is empty, but without it no bundle
/// can be verified, so [`ManifestUpdater`] reports installing as unsupported.
pub const UPDATE_PUBLIC_KEY: &str = "";

/// Where each channel's manifest is published: `<base>/<channel>.json`.
const MANIFEST_BASE: &str = "https://raw.githubusercontent.com/manusfv/gitcrab/update-manifests";

/// Overrides the manifest URL, for testing against another server. A
/// `{channel}` in it is replaced by `stable` or `prerelease`.
pub const ENDPOINT_VARIABLE: &str = "GITCRAB_UPDATE_ENDPOINT";

const TIMEOUT: Duration = Duration::from_secs(30);

/// Longest time a bundle download may take, start to finish.
const DOWNLOAD_TIMEOUT: Duration = Duration::from_secs(15 * 60);

/// What the NSIS setup is started with: `/S` runs it with no window, `/R`
/// makes it start GitCrab once installed (the template honours `/R` only in
/// silent or passive mode), and `/NS` keeps it from recreating shortcuts the
/// user removed. The setup installs per user, so it never asks to elevate.
pub const WINDOWS_INSTALLER_ARGS: [&str; 3] = ["/S", "/R", "/NS"];

fn windows_config() -> WindowsConfig {
    WindowsConfig {
        install_mode: Some(WindowsUpdateInstallMode::Quiet),
        installer_args: Some(vec!["/NS".to_owned()]),
    }
}

pub fn manifest_url(channel: UpdateChannel) -> String {
    format!("{MANIFEST_BASE}/{}.json", channel.name())
}

/// The manifest URL to read for `channel`, honouring a non-empty override.
fn endpoint(channel: UpdateChannel, overridden: Option<String>) -> Result<Url, String> {
    let url = overridden
        .map(|url| url.trim().replace("{channel}", channel.name()))
        .filter(|url| !url.is_empty())
        .unwrap_or_else(|| manifest_url(channel));
    Url::parse(&url).map_err(|error| format!("Invalid update address {url}: {error}"))
}

/// Checks the published release manifests, and downloads and installs the
/// release they offer. Network and disk work run on a background thread per
/// request.
pub struct ManifestUpdater {
    public_key: String,
    /// Replaces [`ENDPOINT_VARIABLE`], for tests.
    endpoint: Option<String>,
    state: Arc<Mutex<State>>,
}

#[derive(Default)]
struct State {
    /// The release the last check found, with its manifest entry.
    offered: Option<Update>,
    /// A bundle whose signature verified, ready to install.
    verified: Option<Verified>,
    downloading: bool,
}

struct Verified {
    update: Update,
    bytes: Vec<u8>,
}

impl Default for ManifestUpdater {
    fn default() -> Self {
        Self::with_key(UPDATE_PUBLIC_KEY)
    }
}

impl ManifestUpdater {
    /// An updater that verifies bundles with `public_key`, in the format of
    /// [`UPDATE_PUBLIC_KEY`].
    fn with_key(public_key: &str) -> Self {
        Self {
            public_key: public_key.trim().to_owned(),
            endpoint: None,
            state: Arc::default(),
        }
    }

    fn can_verify(&self) -> bool {
        !self.public_key.is_empty()
    }

    fn state(&self) -> MutexGuard<'_, State> {
        lock(&self.state)
    }
}

fn lock(state: &Mutex<State>) -> MutexGuard<'_, State> {
    state.lock().unwrap_or_else(PoisonError::into_inner)
}

/// Whether two spellings name one version.
fn same_version(a: &str, b: &str) -> bool {
    updates::is_skipped(Some(a), b)
}

impl Updater for ManifestUpdater {
    fn check(&self, channel: UpdateChannel, current_version: &str) -> Result<UpdateCheck, String> {
        let current = Version::parse(current_version.trim().trim_start_matches('v'))
            .map_err(|error| format!("Invalid version {current_version}: {error}"))?;
        let overridden = self
            .endpoint
            .clone()
            .or_else(|| std::env::var(ENDPOINT_VARIABLE).ok());
        let url = endpoint(channel, overridden)?;
        let key = self.public_key.clone();
        let state = self.state.clone();
        let (reply, result) = oneshot::channel();
        std::thread::Builder::new()
            .name("gitcrab-update-check".into())
            .spawn(move || {
                let found = check_manifest(current, url, key);
                if let Ok(update) = &found {
                    lock(&state).offered = update.clone();
                }
                let _ = reply.send(found.map(|update| update.as_ref().map(available)));
            })
            .map_err(|error| format!("Could not start the update check: {error}"))?;
        Ok(result)
    }

    fn install_support(&self) -> InstallSupport {
        InstallSupport::decide(cfg!(debug_assertions), self.can_verify(), packaging())
    }

    fn download(&self, version: &str) -> Result<UpdateDownload, String> {
        if !self.can_verify() {
            return Err("This build can't verify updates.".into());
        }
        let mut state = self.state();
        if state.downloading {
            return Err("The update is already downloading.".into());
        }
        let mut update = state
            .offered
            .clone()
            .filter(|update| same_version(&update.version, version))
            .ok_or("Check for updates again, then install.")?;
        // The bundle is far larger than a manifest.
        update.timeout = Some(DOWNLOAD_TIMEOUT);
        state.downloading = true;
        state.verified = None;
        drop(state);

        let (progress, progress_receiver) = watch::channel(DownloadProgress::default());
        let (reply, done) = oneshot::channel();
        let shared = self.state.clone();
        let started = std::thread::Builder::new()
            .name("gitcrab-update-download".into())
            .spawn(move || {
                let result = download_verified(&update, &progress);
                let mut state = lock(&shared);
                state.downloading = false;
                let result = result.map(|bytes| state.verified = Some(Verified { update, bytes }));
                drop(state);
                // Closing the progress channel tells the caller to read `done`.
                drop(progress);
                let _ = reply.send(result);
            });
        if let Err(error) = started {
            self.state().downloading = false;
            return Err(format!("Could not start the download: {error}"));
        }
        Ok(UpdateDownload {
            progress: progress_receiver,
            done,
        })
    }

    fn install(&self, version: &str) -> Result<UpdateInstall, String> {
        if let InstallSupport::Unsupported(reason) = self.install_support() {
            return Err(reason);
        }
        // Installing replaces the program running the tests.
        if cfg!(test) {
            return Err("Tests never install updates.".into());
        }
        let verified = {
            let mut state = self.state();
            match &state.verified {
                Some(verified) if same_version(&verified.update.version, version) => {
                    state.verified.take()
                }
                _ => None,
            }
        }
        .ok_or("Download the update before installing it.")?;
        let (reply, result) = oneshot::channel();
        std::thread::Builder::new()
            .name("gitcrab-update-install".into())
            .spawn(move || {
                let restart = restart_target(&verified.update);
                let installed = verified
                    .update
                    .install(verified.bytes)
                    .map_err(|error| format!("Could not install the update: {error}"));
                let _ = reply.send(installed.and(restart));
            })
            .map_err(|error| format!("Could not start installing: {error}"))?;
        Ok(result)
    }
}

/// What the user sees of a manifest's release.
fn available(update: &Update) -> AvailableUpdate {
    AvailableUpdate::new(
        &update.version,
        update.body.clone().filter(|notes| !notes.trim().is_empty()),
        update.date.map(|date| date.date().to_string()),
    )
}

/// Reads the manifest at `url` and returns its release if it is newer than
/// `current` and has a bundle for this platform. Blocks while the request
/// runs. `key` is kept with the release to verify its bundle later.
fn check_manifest(current: Version, url: Url, key: String) -> Result<Option<Update>, String> {
    let shown = url.to_string();
    let updater = UpdaterBuilder::new(
        current,
        Config {
            endpoints: vec![url],
            pubkey: key,
            windows: Some(windows_config()),
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
        Ok(update) => Ok(update),
        // A newer release with no bundle for this platform has nothing to
        // install here, so it is not offered.
        Err(Error::TargetNotFound(_)) => Ok(None),
        // Any unsuccessful status, such as 404 before the first manifest
        // is published.
        Err(Error::ReleaseNotFound) => Err(format!("No release information at {shown}")),
        Err(error) => Err(error.to_string()),
    }
}

/// Downloads the bundle of `update` and verifies its signature, reporting
/// progress. Blocks while the download runs.
fn download_verified(
    update: &Update,
    progress: &watch::Sender<DownloadProgress>,
) -> Result<Vec<u8>, String> {
    let received = Cell::new(0_u64);
    update
        .download_extended(
            |chunk, total| {
                received.set(received.get().saturating_add(chunk as u64));
                let next = DownloadProgress {
                    received: received.get(),
                    total,
                };
                progress.send_if_modified(|shown| {
                    let changed = shown.step() != next.step();
                    *shown = next;
                    changed
                });
            },
            || {},
        )
        .map_err(|error| match error {
            Error::Minisign(_) | Error::Base64(_) | Error::SignatureUtf8(_) => {
                "The download's signature didn't verify, so it was not installed.".to_owned()
            }
            error => format!("Could not download the update: {error}"),
        })
}

/// The program to start once `update` is installed: the AppImage or app
/// bundle that was replaced. On Windows the installer starts GitCrab itself.
fn restart_target(update: &Update) -> Result<PathBuf, String> {
    if cfg!(windows) {
        std::env::current_exe().map_err(|error| format!("Could not find GitCrab: {error}"))
    } else {
        Ok(update.extract_path.clone())
    }
}

fn packaging() -> Packaging {
    let Ok(exe) = std::env::current_exe() else {
        return Packaging::Unknown;
    };
    if cfg!(windows) {
        windows_packaging(&exe)
    } else if cfg!(target_os = "macos") {
        mac_packaging(&exe)
    } else if cfg!(target_os = "linux") {
        linux_packaging(std::env::var_os("APPIMAGE").map(PathBuf::from))
    } else {
        Packaging::Unknown
    }
}

/// The GitCrab setup writes `uninstall.exe` next to `gitcrab.exe`; a zip
/// has none.
fn windows_packaging(exe: &Path) -> Packaging {
    match exe.parent() {
        Some(folder) if folder.join("uninstall.exe").is_file() => Packaging::WindowsInstaller,
        _ => Packaging::WindowsPortable,
    }
}

/// An AppImage runtime sets `APPIMAGE` to the image's path. Replacing the
/// image needs its folder to be writable.
fn linux_packaging(appimage: Option<PathBuf>) -> Packaging {
    match appimage {
        Some(image) if image.is_file() => Packaging::AppImage {
            writable: image.parent().is_some_and(is_writable),
        },
        _ => Packaging::LinuxUnpackaged,
    }
}

/// The executable of a bundle is `<name>.app/Contents/MacOS/<binary>`.
/// Replacing the bundle needs the folder holding it to be writable, which a
/// mounted disk image or a quarantined copy is not.
fn mac_packaging(exe: &Path) -> Packaging {
    let bundle = exe
        .parent()
        .filter(|folder| folder.ends_with("Contents/MacOS"))
        .and_then(Path::parent)
        .and_then(Path::parent)
        .filter(|bundle| {
            bundle
                .extension()
                .is_some_and(|extension| extension == "app")
        });
    match bundle {
        Some(bundle) => Packaging::AppBundle {
            writable: bundle.parent().is_some_and(is_writable),
        },
        None => Packaging::MacUnbundled,
    }
}

/// Whether a file can be created in `folder`.
fn is_writable(folder: &Path) -> bool {
    let probe = folder.join(format!(".gitcrab-update-probe-{}", std::process::id()));
    match std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&probe)
    {
        Ok(file) => {
            drop(file);
            let _ = std::fs::remove_file(&probe);
            true
        }
        Err(_) => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use base64::{engine::general_purpose::STANDARD, Engine as _};
    use std::collections::HashMap;
    use std::io::{BufRead, BufReader, Write};
    use std::net::TcpListener;

    #[test]
    fn the_windows_setup_runs_silently_and_restarts_gitcrab() {
        let config = windows_config();
        let args: Vec<&str> = config
            .install_mode
            .unwrap_or_default()
            .nsis_args()
            .iter()
            .copied()
            .chain(config.installer_args.iter().flatten().map(String::as_str))
            .collect();
        assert_eq!(args, WINDOWS_INSTALLER_ARGS);
    }

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

    /// Serves `routes` on a local port for the rest of the test run.
    fn serve(routes: Vec<(&'static str, &'static str, Vec<u8>)>) -> String {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        let routes: HashMap<_, _> = routes
            .into_iter()
            .map(|(path, status, body)| (path, (status, body)))
            .collect();
        std::thread::spawn(move || {
            for stream in listener.incoming() {
                let Ok(mut stream) = stream else { continue };
                let mut reader = BufReader::new(stream.try_clone().unwrap());
                let mut request_line = String::new();
                let _ = reader.read_line(&mut request_line);
                // Read the headers so the client sees a complete exchange.
                let mut header = String::new();
                while reader.read_line(&mut header).is_ok_and(|n| n > 2) {
                    header.clear();
                }
                let path = request_line.split_whitespace().nth(1).unwrap_or("");
                let (status, body) = routes
                    .get(path)
                    .cloned()
                    .unwrap_or(("404 Not Found", Vec::new()));
                let head = format!(
                    "HTTP/1.1 {status}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                    body.len()
                );
                let _ = stream.write_all(head.as_bytes());
                let _ = stream.write_all(&body);
            }
        });
        format!("http://{address}")
    }

    fn serve_once(status: &'static str, body: String) -> Url {
        let base = serve(vec![("/prerelease.json", status, body.into_bytes())]);
        Url::parse(&format!("{base}/prerelease.json")).unwrap()
    }

    fn manifest(version: &str, platforms: &[&str]) -> String {
        manifest_for(version, platforms, "https://example.com/bundle", "sig")
    }

    fn manifest_for(version: &str, platforms: &[&str], url: &str, signature: &str) -> String {
        let platforms = platforms
            .iter()
            .map(|key| {
                format!(r#""{key}":{{"url":"{url}","signature":"{signature}","format":"nsis"}}"#)
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

    fn check(current: &str, url: Url) -> Result<Option<AvailableUpdate>, String> {
        check_manifest(version(current), url, String::new())
            .map(|update| update.as_ref().map(available))
    }

    #[test]
    fn a_newer_release_is_offered_with_its_notes_and_date() {
        let url = serve_once("200 OK", manifest("v0.1.0-alpha.4", ALL_PLATFORMS));
        let update = check("0.1.0-alpha.3", url).unwrap().unwrap();
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
        assert_eq!(check("0.1.0-alpha.3", url), Ok(None));
        let url = serve_once("200 OK", manifest("0.1.0-alpha.2", ALL_PLATFORMS));
        assert_eq!(check("0.1.0-alpha.3", url), Ok(None));
    }

    #[test]
    fn a_release_without_this_platform_is_not_offered() {
        let url = serve_once("200 OK", manifest("9.0.0", &["plan9-mips"]));
        assert_eq!(check("0.1.0", url), Ok(None));
    }

    #[test]
    fn a_missing_manifest_is_an_error() {
        let url = serve_once("404 Not Found", String::new());
        let error = check("0.1.0", url).unwrap_err();
        assert!(error.starts_with("No release information at http://127.0.0.1:"));
    }

    #[test]
    fn copies_without_an_installer_or_bundle_only_announce() {
        let folder = std::env::temp_dir().join(format!("gitcrab-packaging-{}", std::process::id()));
        std::fs::create_dir_all(&folder).unwrap();
        let exe = folder.join("gitcrab.exe");
        assert_eq!(windows_packaging(&exe), Packaging::WindowsPortable);
        std::fs::write(folder.join("uninstall.exe"), b"").unwrap();
        assert_eq!(windows_packaging(&exe), Packaging::WindowsInstaller);

        assert_eq!(linux_packaging(None), Packaging::LinuxUnpackaged);
        assert_eq!(
            linux_packaging(Some(folder.join("missing.AppImage"))),
            Packaging::LinuxUnpackaged
        );
        let image = folder.join("GitCrab.AppImage");
        std::fs::write(&image, b"").unwrap();
        assert_eq!(
            linux_packaging(Some(image)),
            Packaging::AppImage { writable: true }
        );

        let bundle = folder.join("Apps").join("GitCrab.app");
        assert_eq!(
            mac_packaging(&bundle.join("Contents").join("MacOS").join("gitcrab")),
            Packaging::AppBundle { writable: false },
            "the folder holding the bundle does not exist"
        );
        std::fs::create_dir_all(folder.join("Apps")).unwrap();
        assert_eq!(
            mac_packaging(&bundle.join("Contents").join("MacOS").join("gitcrab")),
            Packaging::AppBundle { writable: true }
        );
        assert_eq!(
            mac_packaging(&folder.join("target").join("release").join("gitcrab")),
            Packaging::MacUnbundled
        );
        assert!(is_writable(&folder));
        assert!(!is_writable(&folder.join("missing")));
        std::fs::remove_dir_all(&folder).unwrap();
    }

    /// A minisign key pair made for this test run only, encoded the way
    /// `cargo packager signer` writes keys and signatures: base64 of the
    /// minisign text files. Never used to sign anything real.
    struct TestKey(minisign::KeyPair);

    impl TestKey {
        fn new() -> Self {
            Self(minisign::KeyPair::generate_unencrypted_keypair().unwrap())
        }

        fn public(&self) -> String {
            STANDARD.encode(self.0.pk.to_box().unwrap().to_string())
        }

        fn sign(&self, bytes: &[u8]) -> String {
            let signature =
                minisign::sign(Some(&self.0.pk), &self.0.sk, bytes, None, None).unwrap();
            STANDARD.encode(signature.to_string())
        }
    }

    const BUNDLE: &[u8] = b"GitCrab 9.0.0 update bundle (test data)";

    /// Serves a manifest offering 9.0.0 for this platform with `signature`,
    /// and `bundle` as its download, then checks it with `key`.
    fn offered(key: &str, signature: &str, bundle: &[u8]) -> ManifestUpdater {
        let platform = cargo_packager_updater::target().unwrap();
        let downloads = serve(vec![("/bundle", "200 OK", bundle.to_vec())]);
        let manifest = manifest_for(
            "9.0.0",
            &[&platform],
            &format!("{downloads}/bundle"),
            signature,
        );
        let base = serve(vec![("/stable.json", "200 OK", manifest.into_bytes())]);
        let mut updater = ManifestUpdater::with_key(key);
        updater.endpoint = Some(format!("{base}/{{channel}}.json"));
        let found = updater
            .check(UpdateChannel::Stable, "0.1.0")
            .unwrap()
            .blocking_recv()
            .unwrap()
            .unwrap()
            .unwrap();
        assert_eq!(found.version, "9.0.0");
        updater
    }

    fn download(updater: &ManifestUpdater) -> (Result<(), String>, DownloadProgress) {
        let UpdateDownload { progress, done } = updater.download("9.0.0").unwrap();
        let result = done.blocking_recv().unwrap();
        // The progress channel closed before the reply.
        assert!(progress.has_changed().is_err());
        let last = *progress.borrow();
        (result, last)
    }

    fn has_verified(updater: &ManifestUpdater) -> bool {
        updater.state().verified.is_some()
    }

    #[test]
    fn a_correctly_signed_bundle_verifies() {
        let key = TestKey::new();
        let updater = offered(&key.public(), &key.sign(BUNDLE), BUNDLE);
        let (result, progress) = download(&updater);
        assert_eq!(result, Ok(()));
        assert_eq!(progress.percent(), Some(100));
        let state = updater.state();
        let verified = state.verified.as_ref().unwrap();
        assert_eq!(verified.bytes, BUNDLE);
        assert_eq!(verified.update.version, "9.0.0");
    }

    #[test]
    fn a_tampered_bundle_is_rejected_before_installing() {
        let key = TestKey::new();
        let mut tampered = BUNDLE.to_vec();
        tampered[0] ^= 1;
        let updater = offered(&key.public(), &key.sign(BUNDLE), &tampered);
        let (result, _) = download(&updater);
        assert_eq!(
            result,
            Err("The download's signature didn't verify, so it was not installed.".into())
        );
        assert!(!has_verified(&updater));
        assert!(updater.install("9.0.0").is_err());
    }

    #[test]
    fn unsigned_or_foreign_bundles_are_rejected() {
        let key = TestKey::new();
        for signature in [String::new(), "sig".into(), TestKey::new().sign(BUNDLE)] {
            let updater = offered(&key.public(), &signature, BUNDLE);
            let (result, _) = download(&updater);
            assert!(result.is_err(), "signature {signature:?} was accepted");
            assert!(!has_verified(&updater));
        }
    }

    #[test]
    fn without_a_public_key_nothing_downloads_or_installs() {
        let key = TestKey::new();
        let updater = offered("", &key.sign(BUNDLE), BUNDLE);
        assert_eq!(
            updater.install_support(),
            InstallSupport::Unsupported("This build can't verify updates.".into())
        );
        assert_eq!(
            updater.download("9.0.0").unwrap_err(),
            "This build can't verify updates."
        );
        assert!(updater.install("9.0.0").is_err());
    }

    #[test]
    fn only_the_checked_release_downloads() {
        let key = TestKey::new();
        let updater = offered(&key.public(), &key.sign(BUNDLE), BUNDLE);
        assert_eq!(
            updater.download("9.0.1").unwrap_err(),
            "Check for updates again, then install."
        );
        assert!(ManifestUpdater::with_key(&key.public())
            .download("9.0.0")
            .is_err());
    }

    #[test]
    fn test_builds_never_install() {
        let key = TestKey::new();
        let updater = offered(&key.public(), &key.sign(BUNDLE), BUNDLE);
        assert_eq!(download(&updater).0, Ok(()));
        assert!(updater.install("9.0.0").is_err());
        assert!(has_verified(&updater), "the verified bundle is kept");
    }
}
