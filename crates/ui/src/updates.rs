//! Update checks shared by every window: the installed [`Updater`], the
//! latest result, the automatic schedule and the user's answers to an
//! available release.
//!
//! The rules live in [`gitcrab_core::updates`]; this module keeps the state
//! in a gpui global. Views observe the global to redraw.
use gitcrab_core::updates::{self as rules, Updater};
pub use gitcrab_core::updates::{AvailableUpdate, UpdateChannel};
use gpui::{App, BorrowAppContext as _, Global};
use std::{sync::Arc, time::Duration};

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

pub(crate) struct Updates {
    updater: Arc<dyn Updater>,
    status: Status,
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

pub fn is_skipped(version: &str, cx: &mut App) -> bool {
    rules::is_skipped(
        crate::preferences::current(cx).skipped_update.as_deref(),
        version,
    )
}

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

/// Forgets the result of a check on another channel.
pub fn channel_changed(cx: &mut App) {
    if cx.has_global::<Updates>() {
        cx.update_global::<Updates, _>(|updates, _| {
            updates.generation += 1;
            updates.status = Status::Idle;
        });
    }
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
        updates.status = match result {
            Ok(Some(update)) => Status::Available(update),
            Ok(None) => Status::UpToDate,
            Err(error) => Status::Failed(error),
        };
    });
}

fn now() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |elapsed| elapsed.as_secs())
}
