//! Updates: which release channel to follow, when to check, whether a
//! release is newer, whether the user skipped it, and whether this copy of
//! GitCrab can install a release in place. Fetching, verifying and
//! installing sit behind [`Updater`].
use semver::Version;
use serde::{Deserialize, Serialize};
use std::{cmp::Ordering, path::PathBuf};
use tokio::sync::{oneshot, watch};

/// Least time between two automatic checks, in seconds.
pub const CHECK_INTERVAL_SECS: u64 = 24 * 60 * 60;

/// Where release pages are published, one per tag `v<version>`.
pub const RELEASES_URL: &str = "https://github.com/manusfv/gitcrab/releases";

/// Which releases GitCrab offers as updates.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum UpdateChannel {
    /// Stable releases only.
    Stable,
    /// Prereleases and stable releases.
    Prerelease,
}

impl UpdateChannel {
    /// The channel to follow: the saved choice, or the one matching the
    /// running build. A prerelease build such as `0.1.0-alpha.3` follows
    /// prereleases; any other build, or one whose version does not parse,
    /// follows stable releases.
    pub fn effective(saved: Option<Self>, current_version: &str) -> Self {
        saved.unwrap_or_else(|| match parse(current_version) {
            Some(version) if !version.pre.is_empty() => Self::Prerelease,
            _ => Self::Stable,
        })
    }

    /// The name used in manifest file names and settings.
    pub fn name(self) -> &'static str {
        match self {
            Self::Stable => "stable",
            Self::Prerelease => "prerelease",
        }
    }

    /// The name shown to the user.
    pub fn label(self) -> &'static str {
        match self {
            Self::Stable => "Stable",
            Self::Prerelease => "Prerelease",
        }
    }
}

/// A release newer than the running build.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AvailableUpdate {
    /// The release's version, without a leading `v`.
    pub version: String,
    /// Release notes from the manifest, if any.
    pub notes: Option<String>,
    /// The release's page, with the full notes and downloads.
    pub release_url: String,
    /// The publication date (`YYYY-MM-DD`), if the manifest has one.
    pub date: Option<String>,
}

impl AvailableUpdate {
    /// An update for `version` with its release page filled in.
    pub fn new(version: &str, notes: Option<String>, date: Option<String>) -> Self {
        let version = version.trim().trim_start_matches('v').to_owned();
        Self {
            release_url: release_page(&version),
            version,
            notes,
            date,
        }
    }
}

pub fn release_page(version: &str) -> String {
    format!(
        "{RELEASES_URL}/tag/v{}",
        version.trim().trim_start_matches('v')
    )
}

fn parse(version: &str) -> Option<Version> {
    Version::parse(version.trim().trim_start_matches('v')).ok()
}

/// Whether `candidate` is a newer release than `current`, by semver
/// precedence: `0.1.0` is newer than `0.1.0-alpha.3`, and `alpha.10` newer
/// than `alpha.9`. Build metadata does not count. Versions that do not
/// parse are never newer.
pub fn is_newer(candidate: &str, current: &str) -> bool {
    match (parse(candidate), parse(current)) {
        (Some(candidate), Some(current)) => candidate.cmp_precedence(&current) == Ordering::Greater,
        _ => false,
    }
}

/// Whether the user chose to skip `version`. Spellings of one version, such
/// as `v1.0.0` and `1.0.0`, match.
pub fn is_skipped(skipped: Option<&str>, version: &str) -> bool {
    let Some(skipped) = skipped else {
        return false;
    };
    match (parse(skipped), parse(version)) {
        (Some(skipped), Some(version)) => skipped.cmp_precedence(&version) == Ordering::Equal,
        _ => skipped.trim() == version.trim(),
    }
}

/// Whether to check for updates now.
///
/// A manual check always runs. An automatic one runs when automatic checks
/// are on and the last successful check is at least [`CHECK_INTERVAL_SECS`]
/// old, missing, or in the future (the clock moved back).
pub fn should_check(automatic: bool, last_check: Option<u64>, now: u64, manual: bool) -> bool {
    if manual {
        return true;
    }
    if !automatic {
        return false;
    }
    match last_check {
        None => true,
        Some(last) if last > now => true,
        Some(last) => now - last >= CHECK_INTERVAL_SECS,
    }
}

/// Completion of an update check: the newest release if it is newer than the
/// running build, `None` when up to date, or a message on failure.
pub type UpdateCheck = oneshot::Receiver<Result<Option<AvailableUpdate>, String>>;

/// How this copy of GitCrab was installed, as far as updating it goes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Packaging {
    /// Installed on Windows by the GitCrab setup program.
    WindowsInstaller,
    /// Run on Windows from anywhere else, such as an unpacked zip.
    WindowsPortable,
    /// Run on Linux as an AppImage; `writable` when its folder can be written.
    AppImage { writable: bool },
    /// Run on Linux from anything but an AppImage.
    LinuxUnpackaged,
    /// Run on macOS from `GitCrab.app`; `writable` when the folder holding
    /// the bundle can be written.
    AppBundle { writable: bool },
    /// Run on macOS outside an app bundle.
    MacUnbundled,
    /// Any other system.
    Unknown,
}

/// Whether this copy of GitCrab can install an update in place.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum InstallSupport {
    Supported,
    /// Only announce releases; the reason is shown to the user.
    Unsupported(String),
}

impl InstallSupport {
    /// `can_verify` is whether the build has the public key update bundles
    /// are signed with: without it no bundle can be verified, so none is
    /// installed. Development builds never replace themselves.
    pub fn decide(debug_build: bool, can_verify: bool, packaging: Packaging) -> Self {
        let reason = if !can_verify {
            "This build can't verify updates."
        } else if debug_build {
            "Development builds don't install updates."
        } else {
            match packaging {
                Packaging::WindowsInstaller
                | Packaging::AppImage { writable: true }
                | Packaging::AppBundle { writable: true } => return Self::Supported,
                Packaging::WindowsPortable => "This copy wasn't installed by the GitCrab setup.",
                Packaging::AppImage { writable: false } => {
                    "The folder holding the AppImage is read-only."
                }
                Packaging::LinuxUnpackaged => "Only the AppImage installs updates itself.",
                Packaging::AppBundle { writable: false } => {
                    "GitCrab.app is in a folder GitCrab can't change. Move it to Applications."
                }
                Packaging::MacUnbundled => "GitCrab isn't running from GitCrab.app.",
                Packaging::Unknown => "Updates can't be installed on this system.",
            }
        };
        Self::Unsupported(reason.to_owned())
    }

    pub fn is_supported(&self) -> bool {
        *self == Self::Supported
    }
}

/// Bytes of an update bundle received so far.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct DownloadProgress {
    pub received: u64,
    /// The bundle's size, when the server sent it.
    pub total: Option<u64>,
}

impl DownloadProgress {
    const UNKNOWN_SIZE_STEP: u64 = 1024 * 1024;

    /// The share received, from 0 to 100, when the size is known.
    pub fn percent(self) -> Option<u8> {
        let total = self.total.filter(|&total| total > 0)?;
        let percent = self.received.min(total).saturating_mul(100) / total;
        u8::try_from(percent).ok()
    }

    /// A number that grows by one per whole percent, or per MiB when the size
    /// is unknown. Progress is worth reporting when it changes.
    pub fn step(self) -> u64 {
        match self.percent() {
            Some(percent) => u64::from(percent),
            None => self.received / Self::UNKNOWN_SIZE_STEP,
        }
    }
}

/// A download in progress. `progress` closes when the download ends, then
/// `done` says whether the bundle arrived and its signature verified.
#[derive(Debug)]
pub struct UpdateDownload {
    pub progress: watch::Receiver<DownloadProgress>,
    pub done: oneshot::Receiver<Result<(), String>>,
}

/// Completion of an install: the program to start in place of the running
/// one, or a message on failure.
pub type UpdateInstall = oneshot::Receiver<Result<PathBuf, String>>;

/// Finds, downloads and installs releases newer than the running build.
///
/// Implementations do their network and disk work off the calling thread.
pub trait Updater: Send + Sync {
    /// Starts checking `channel` for a release newer than `current_version`.
    ///
    /// # Errors
    ///
    /// Returns a message if the check cannot be started.
    fn check(&self, channel: UpdateChannel, current_version: &str) -> Result<UpdateCheck, String>;

    /// Whether this copy can install updates in place. When it cannot,
    /// releases are only announced.
    fn install_support(&self) -> InstallSupport;

    /// Starts downloading `version`, the release the last check found, and
    /// verifies its signature. A bundle that fails verification is dropped.
    ///
    /// # Errors
    ///
    /// Returns a message if the download cannot be started, for example
    /// because the last check did not find `version`.
    fn download(&self, version: &str) -> Result<UpdateDownload, String>;

    /// Starts installing `version` from its verified download.
    ///
    /// On success the new version is in place: start the returned program
    /// and quit. Some systems hand over to an installer that restarts
    /// GitCrab and end this process without replying, so save state before
    /// calling this.
    ///
    /// # Errors
    ///
    /// Returns a message if nothing verified is ready to install or this copy
    /// cannot install updates.
    fn install(&self, version: &str) -> Result<UpdateInstall, String>;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn channel_follows_the_build_until_chosen() {
        use UpdateChannel::*;
        assert_eq!(UpdateChannel::effective(None, "0.1.0-alpha.3"), Prerelease);
        assert_eq!(UpdateChannel::effective(None, "0.1.0"), Stable);
        assert_eq!(UpdateChannel::effective(None, "1.2.3+build.5"), Stable);
        assert_eq!(UpdateChannel::effective(None, "not a version"), Stable);
        assert_eq!(
            UpdateChannel::effective(Some(Stable), "0.1.0-alpha.3"),
            Stable
        );
        assert_eq!(
            UpdateChannel::effective(Some(Prerelease), "0.1.0"),
            Prerelease
        );
    }

    #[test]
    fn channel_is_saved_by_name() {
        assert_eq!(
            serde_json::to_string(&UpdateChannel::Prerelease).unwrap(),
            r#""prerelease""#
        );
        let stable: UpdateChannel = serde_json::from_str(r#""stable""#).unwrap();
        assert_eq!(stable, UpdateChannel::Stable);
        assert_eq!(stable.name(), "stable");
    }

    #[test]
    fn newer_follows_semver_precedence() {
        assert!(is_newer("0.1.0-alpha.4", "0.1.0-alpha.3"));
        assert!(is_newer("0.1.0-alpha.10", "0.1.0-alpha.9"));
        assert!(is_newer("0.1.0-beta.1", "0.1.0-alpha.9"));
        assert!(is_newer("0.1.0", "0.1.0-alpha.3"));
        assert!(is_newer("v0.2.0", "0.1.0"));
        assert!(!is_newer("0.1.0-alpha.3", "0.1.0-alpha.3"));
        assert!(!is_newer("0.1.0-alpha.3", "0.1.0"));
        assert!(!is_newer("0.1.0-alpha.2", "0.1.0-alpha.3"));
        assert!(!is_newer("0.1.0+other", "0.1.0+build"));
        assert!(!is_newer("garbage", "0.1.0"));
        assert!(!is_newer("0.2.0", "garbage"));
    }

    #[test]
    fn skipped_version_matches_any_spelling() {
        assert!(is_skipped(Some("v0.2.0"), "0.2.0"));
        assert!(is_skipped(Some("0.2.0-beta.1"), "0.2.0-beta.1"));
        assert!(!is_skipped(Some("0.2.0"), "0.2.1"));
        assert!(!is_skipped(None, "0.2.0"));
        assert!(is_skipped(Some("odd"), "odd"));
    }

    #[test]
    fn automatic_checks_run_at_most_daily() {
        let now = 1_000_000;
        assert!(should_check(true, None, now, false));
        assert!(!should_check(true, Some(now - 60), now, false));
        assert!(!should_check(
            true,
            Some(now - CHECK_INTERVAL_SECS + 1),
            now,
            false
        ));
        assert!(should_check(
            true,
            Some(now - CHECK_INTERVAL_SECS),
            now,
            false
        ));
        // The clock moved back past the last check.
        assert!(should_check(true, Some(now + 10), now, false));
        assert!(!should_check(false, None, now, false));
    }

    #[test]
    fn manual_checks_ignore_the_limit_and_the_switch() {
        assert!(should_check(false, Some(100), 100, true));
        assert!(should_check(true, Some(100), 100, true));
    }

    #[test]
    fn available_update_links_its_release_page() {
        let update = AvailableUpdate::new("v0.2.0", Some("Notes".into()), None);
        assert_eq!(update.version, "0.2.0");
        assert_eq!(
            update.release_url,
            "https://github.com/manusfv/gitcrab/releases/tag/v0.2.0"
        );
        assert_eq!(
            release_page("0.1.0-alpha.4"),
            format!("{RELEASES_URL}/tag/v0.1.0-alpha.4")
        );
    }

    #[test]
    fn installing_needs_a_key_a_release_build_and_a_writable_package() {
        use Packaging::*;
        let decide = InstallSupport::decide;
        for packaging in [
            WindowsInstaller,
            AppImage { writable: true },
            AppBundle { writable: true },
        ] {
            assert!(
                decide(false, true, packaging).is_supported(),
                "{packaging:?}"
            );
            assert_eq!(
                decide(false, false, packaging),
                InstallSupport::Unsupported("This build can't verify updates.".into())
            );
            assert_eq!(
                decide(true, true, packaging),
                InstallSupport::Unsupported("Development builds don't install updates.".into())
            );
        }
        for packaging in [
            WindowsPortable,
            AppImage { writable: false },
            LinuxUnpackaged,
            AppBundle { writable: false },
            MacUnbundled,
            Unknown,
        ] {
            assert!(
                !decide(false, true, packaging).is_supported(),
                "{packaging:?}"
            );
        }
    }

    #[test]
    fn progress_reports_whole_percents() {
        let progress = |received, total| DownloadProgress { received, total };
        assert_eq!(progress(0, Some(200)).percent(), Some(0));
        assert_eq!(progress(84, Some(200)).percent(), Some(42));
        assert_eq!(progress(200, Some(200)).percent(), Some(100));
        // More than announced still reads as complete.
        assert_eq!(progress(300, Some(200)).percent(), Some(100));
        assert_eq!(progress(10, None).percent(), None);
        assert_eq!(progress(10, Some(0)).percent(), None);
        assert_eq!(progress(1, Some(200)).step(), progress(0, Some(200)).step());
        assert_ne!(progress(2, Some(200)).step(), progress(0, Some(200)).step());
        assert_eq!(progress(1024 * 1024 - 1, None).step(), 0);
        assert_eq!(progress(3 * 1024 * 1024, None).step(), 3);
    }
}
