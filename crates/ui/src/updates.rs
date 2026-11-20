//! Updates shared by every window: the installed [`Updater`], the latest
//! check, the automatic schedule, the user's answers to an available release
//! and the progress of installing it.
//!
//! The rules live in [`gitcrab_core::updates`]; this module keeps the state
//! in a gpui global. Views observe the global to redraw. The workspace
//! decides when installing is safe and restarts GitCrab afterwards.
use gitcrab_core::updates::{self as rules, UpdateDownload, Updater};
pub use gitcrab_core::updates::{AvailableUpdate, InstallSupport, UpdateChannel};
use gpui::{App, BorrowAppContext as _, Global, Task};
use std::{path::PathBuf, sync::Arc, time::Duration};

/// The running version.
pub const CURRENT_VERSION: &str = env!("CARGO_PKG_VERSION");

/// Delay before the first automatic check, so startup work comes first.
const FIRST_CHECK: Duration = Duration::from_secs(10);
/// How often the automatic schedule asks whether a check is due.
const RECHECK: Duration = Duration::from_secs(60 * 60);

/// The outcome of the latest update check.
#[derive(Clone, Debug, Default, PartialEq)]
pub enum Status {
    /// No check has finished since startup or since the channel changed.
    #[default]
    Idle,
    Checking,
    UpToDate,
    Available(AvailableUpdate),
    Failed(String),
}

/// Where installing the available release stands.
#[derive(Clone, Debug, Default, PartialEq)]
pub enum Install {
    #[default]
    Idle,
    /// Downloading, with the share received when the size is known.
    Downloading(Option<u8>),
    /// Replacing GitCrab; it restarts next.
    Installing,
    /// The last attempt failed. The user can try again.
    Failed(String),
}

impl Install {
    /// A download or install is running.
    pub fn in_progress(&self) -> bool {
        matches!(self, Self::Downloading(_) | Self::Installing)
    }
}

pub(crate) struct Updates {
    updater: Arc<dyn Updater>,
    status: Status,
    /// Whether this copy installs updates, asked when a release is found.
    support: Option<InstallSupport>,
    install: Install,
    /// The release downloaded and verified, ready to install.
    downloaded: Option<String>,
    /// Identifies the check in flight; results of older checks are dropped.
    generation: u64,
    /// The latest result came from a manual check, which shows a release
    /// even if the user skipped it.
    manual: bool,
    /// A release whose notice the user closed for this session.
    dismissed: Option<String>,
}

impl Global for Updates {}

/// Installs the updater and starts the automatic schedule. Call once at
/// startup, after the settings store is installed.
pub fn install(updater: Arc<dyn Updater>, cx: &mut App) {
    cx.set_global(Updates {
        updater,
        status: Status::Idle,
        support: None,
        install: Install::Idle,
        downloaded: None,
        generation: 0,
        manual: false,
        dismissed: None,
    });
    cx.spawn(async move |cx| {
        let mut wait = FIRST_CHECK;
        loop {
            cx.background_executor().timer(wait).await;
            wait = RECHECK;
            cx.update(check_if_due);
        }
    })
    .detach();
}

/// The latest check's outcome.
pub fn status(cx: &App) -> Status {
    cx.try_global::<Updates>()
        .map(|updates| updates.status.clone())
        .unwrap_or_default()
}

/// The release to announce in the workspace, if any: available, not closed
/// this session, and not skipped unless the user asked for this check.
pub fn notice(cx: &mut App) -> Option<AvailableUpdate> {
    let skipped = crate::preferences::current(cx).skipped_update;
    let updates = cx.try_global::<Updates>()?;
    let Status::Available(update) = &updates.status else {
        return None;
    };
    let hidden = updates.dismissed.as_deref() == Some(update.version.as_str())
        || (!updates.manual && rules::is_skipped(skipped.as_deref(), &update.version));
    (!hidden).then(|| update.clone())
}

/// Whether the user skipped `version`.
pub fn is_skipped(version: &str, cx: &mut App) -> bool {
    rules::is_skipped(
        crate::preferences::current(cx).skipped_update.as_deref(),
        version,
    )
}

/// Runs an automatic check when automatic checks are on and the last one is
/// at least a day old.
fn check_if_due(cx: &mut App) {
    let preferences = crate::preferences::current(cx);
    if rules::should_check(
        preferences.check_for_updates,
        preferences.last_update_check,
        now(),
        false,
    ) {
        let channel = UpdateChannel::effective(preferences.update_channel, CURRENT_VERSION);
        start(channel, false, cx);
    }
}

/// Checks `channel` now, whatever the schedule, and shows any release found
/// even if the user skipped it.
pub fn check_now(channel: UpdateChannel, cx: &mut App) {
    start(channel, true, cx);
}

/// Forgets the result of a check on another channel, unless that release is
/// being installed.
pub fn channel_changed(cx: &mut App) {
    if cx.has_global::<Updates>() {
        cx.update_global::<Updates, _>(|updates, _| {
            if updates.install.in_progress() {
                return;
            }
            updates.generation += 1;
            updates.status = Status::Idle;
            updates.install = Install::Idle;
        });
    }
}

/// Where installing the available release stands.
pub fn install_state(cx: &App) -> Install {
    cx.try_global::<Updates>()
        .map(|updates| updates.install.clone())
        .unwrap_or_default()
}

/// Whether this copy installs the available release itself, or only
/// announces it. `None` until a check finds a release.
pub fn install_support(cx: &App) -> Option<InstallSupport> {
    cx.try_global::<Updates>()?.support.clone()
}

/// Shows why installing stopped. The user can try again.
pub fn install_failed(message: impl Into<String>, cx: &mut App) {
    set_install(Install::Failed(message.into()), cx);
}

fn set_install(install: Install, cx: &mut App) {
    if cx.has_global::<Updates>() {
        cx.update_global::<Updates, _>(|updates, _| updates.install = install);
    }
}

/// Downloads and verifies `version` unless that is already done. Resolves
/// once the bundle is ready to install; failures are shown in the notice.
pub fn download_update(version: &str, cx: &mut App) -> Task<Result<(), String>> {
    let Some(updates) = cx.try_global::<Updates>() else {
        return Task::ready(Err("Updates are off.".into()));
    };
    if updates.install.in_progress() {
        return Task::ready(Err("The update is already being installed.".into()));
    }
    if updates.downloaded.as_deref() == Some(version) {
        return Task::ready(Ok(()));
    }
    let version = version.to_owned();
    let started = updates.updater.download(&version);
    let UpdateDownload { mut progress, done } = match started {
        Ok(download) => download,
        Err(error) => {
            install_failed(error.clone(), cx);
            return Task::ready(Err(error));
        }
    };
    cx.update_global::<Updates, _>(|updates, _| {
        updates.install = Install::Downloading(None);
        updates.downloaded = None;
    });
    cx.spawn(async move |cx| {
        while progress.changed().await.is_ok() {
            let percent = progress.borrow_and_update().percent();
            cx.update(|cx| set_install(Install::Downloading(percent), cx));
        }
        let result = done
            .await
            .unwrap_or_else(|_| Err("The download stopped.".into()));
        cx.update(|cx| {
            cx.update_global::<Updates, _>(|updates, _| match &result {
                Ok(()) => {
                    updates.downloaded = Some(version);
                    updates.install = Install::Idle;
                }
                Err(error) => updates.install = Install::Failed(error.clone()),
            })
        });
        result
    })
}

/// Installs the downloaded `version`. Resolves to the program to start in
/// place of this one; the caller saves state first and restarts after.
pub fn install_update(version: &str, cx: &mut App) -> Task<Result<PathBuf, String>> {
    let Some(updates) = cx.try_global::<Updates>() else {
        return Task::ready(Err("Updates are off.".into()));
    };
    let started = updates.updater.install(version);
    let installing = match started {
        Ok(installing) => installing,
        Err(error) => {
            install_failed(error.clone(), cx);
            return Task::ready(Err(error));
        }
    };
    cx.update_global::<Updates, _>(|updates, _| {
        updates.install = Install::Installing;
        // The verified bundle is used up.
        updates.downloaded = None;
    });
    cx.spawn(async move |cx| {
        let result = installing
            .await
            .unwrap_or_else(|_| Err("Installing stopped.".into()));
        if let Err(error) = &result {
            cx.update(|cx| install_failed(error.clone(), cx));
        }
        result
    })
}

/// Closes the notice for `version` until GitCrab restarts.
pub fn dismiss(version: &str, cx: &mut App) {
    if cx.has_global::<Updates>() {
        cx.update_global::<Updates, _>(|updates, _| updates.dismissed = Some(version.to_owned()));
    }
}

/// Stops announcing `version` and remembers the choice.
pub fn skip(version: &str, cx: &mut App) {
    let version = version.to_owned();
    let _ = crate::preferences::record_app_state(
        |preferences| preferences.skipped_update = Some(version.clone()),
        cx,
    );
    if cx.has_global::<Updates>() {
        cx.update_global::<Updates, _>(|updates, _| {
            updates.manual = false;
            updates.dismissed = Some(version);
        });
    }
}

fn start(channel: UpdateChannel, manual: bool, cx: &mut App) {
    let Some(updates) = cx.try_global::<Updates>() else {
        return;
    };
    // A manual check while an automatic one runs takes over its result.
    if updates.status == Status::Checking && !manual {
        return;
    }
    // Keep the release being installed on screen.
    if updates.install.in_progress() {
        return;
    }
    let updater = updates.updater.clone();
    let check = updater.check(channel, CURRENT_VERSION);
    let generation = cx.update_global::<Updates, _>(|updates, _| {
        updates.generation += 1;
        updates.status = Status::Checking;
        updates.manual = manual;
        if manual {
            updates.dismissed = None;
        }
        updates.generation
    });
    let check = match check {
        Ok(check) => check,
        Err(error) => return finish(generation, Err(error), cx),
    };
    cx.spawn(async move |cx| {
        let result = check
            .await
            .unwrap_or_else(|_| Err("The update check stopped.".into()));
        cx.update(|cx| finish(generation, result, cx));
    })
    .detach();
}

fn finish(generation: u64, result: Result<Option<AvailableUpdate>, String>, cx: &mut App) {
    if cx.global::<Updates>().generation != generation {
        return;
    }
    if result.is_ok() {
        let checked = now();
        // A failed save only means the next start checks again.
        let _ = crate::preferences::record_app_state(
            |preferences| preferences.last_update_check = Some(checked),
            cx,
        );
    }
    cx.update_global::<Updates, _>(|updates, _| {
        if let Ok(found) = &result {
            let version = found.as_ref().map(|update| update.version.as_str());
            if updates.downloaded.as_deref() != version {
                updates.downloaded = None;
            }
            updates.install = Install::Idle;
            if found.is_some() && updates.support.is_none() {
                updates.support = Some(updates.updater.install_support());
            }
        }
        updates.status = match result {
            Ok(Some(update)) => Status::Available(update),
            Ok(None) => Status::UpToDate,
            Err(error) => Status::Failed(error),
        };
    });
}

/// The current time in Unix seconds.
fn now() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |elapsed| elapsed.as_secs())
}
