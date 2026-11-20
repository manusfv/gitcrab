//! Preferences shared by every window, saved through the installed
//! [`SettingsStore`].
//!
//! The rules live in [`gitcrab_core::preferences`]; this module keeps the
//! in-memory copy in a gpui global and applies the font size to the theme.
pub use gitcrab_core::preferences::{PersistenceResult, Preferences, WindowSize};
use gitcrab_core::{
    appearance::ThemePreference,
    platform::RepositoryLocation,
    preferences::{self as rules, SettingsStore},
};
use gpui::{App, Global};
use std::sync::Arc;

struct Store(Arc<dyn SettingsStore>);
impl Global for Store {}

struct SharedPreferences(Preferences);
impl Global for SharedPreferences {}

/// Installs the settings store. Call once at startup, before opening a window.
pub fn install(store: Arc<dyn SettingsStore>, cx: &mut App) {
    cx.set_global(Store(store));
}

/// The installed store.
///
/// # Panics
///
/// Panics if [`install`] has not run.
fn store(cx: &App) -> Arc<dyn SettingsStore> {
    cx.global::<Store>().0.clone()
}

/// Reads shared in-memory preferences, loading them on first use.
pub fn current(cx: &mut App) -> Preferences {
    if let Some(shared) = cx.try_global::<SharedPreferences>() {
        return shared.0.clone();
    }
    let mut preferences = store(cx).load_preferences().unwrap_or_default();
    preferences.normalize();
    cx.set_global(SharedPreferences(preferences.clone()));
    preferences
}

/// Updates the shared appearance and applies the font size immediately.
pub fn set_app_appearance(font_size: f32, theme: ThemePreference, cx: &mut App) {
    let mut shared = current(cx);
    shared.font_size = font_size;
    shared.theme = theme;
    cx.set_global(SharedPreferences(shared));
    gpui::component::Theme::update(cx, |theme| theme.font_size = gpui::px(font_size));
    super::theme::shape_scrollbars(cx);
}

/// Promotes a repository to the top of the recent list and queues a save.
/// Returns `Ok(None)` if it is already first.
pub fn add_recent(path: &str, cx: &mut App) -> Result<Option<PersistenceResult>, String> {
    let mut shared = current(cx);
    if !shared.promote_recent(path) {
        return Ok(None);
    }
    cx.set_global(SharedPreferences(shared.clone()));
    store(cx).save_preferences(&shared).map(Some)
}

/// Records the main window size in shared preferences and queues a save.
pub fn remember_window(size: WindowSize, cx: &mut App) -> Result<PersistenceResult, String> {
    let mut shared = current(cx);
    shared.window = size.normalized();
    cx.set_global(SharedPreferences(shared.clone()));
    store(cx).save_preferences(&shared)
}

/// Changes state the app records for itself, such as the last update check,
/// in shared preferences and queues a save. Tabs never merge these fields.
pub fn record_app_state(
    edit: impl FnOnce(&mut Preferences),
    cx: &mut App,
) -> Result<PersistenceResult, String> {
    let mut shared = current(cx);
    edit(&mut shared);
    cx.set_global(SharedPreferences(shared.clone()));
    store(cx).save_preferences(&shared)
}

/// Merges one tab's edits since `previous` into shared preferences and
/// queues a save. Changes made by other tabs to fields this tab left alone
/// are preserved.
pub fn save_changes(
    edited: &Preferences,
    previous: &Preferences,
    cx: &mut App,
) -> Result<PersistenceResult, String> {
    let mut shared = current(cx);
    shared.merge_tab_changes(previous, edited);
    let result = store(cx).save_preferences(&shared)?;
    cx.set_global(SharedPreferences(shared));
    Ok(result)
}

/// Checks for saved trust in this exact worktree and its metadata paths.
pub fn trusts_repository(location: &RepositoryLocation, cx: &App) -> bool {
    rules::is_trusted(&store(cx).trusted(), location)
}

/// Queues persistence of the user's trust in the worktree and its metadata paths.
pub fn remember_trust(
    location: &RepositoryLocation,
    cx: &mut App,
) -> Result<PersistenceResult, String> {
    store(cx).remember_trust(location)
}

/// Reads the last repository saved for session restoration, if available.
pub fn last_repository(cx: &App) -> Option<String> {
    session_repositories(cx).into_iter().next()
}

/// Reads every repository that was open in the last session, in tab order.
pub fn session_repositories(cx: &App) -> Vec<String> {
    store(cx).session()
}

/// Queues clearing the saved session path if it still matches `repository`.
pub fn forget_last_repository(repository: &str, cx: &mut App) -> Result<PersistenceResult, String> {
    store(cx).forget_session_entry(repository)
}

/// Queues saving the open repositories (tab order) for session restoration.
pub fn remember_repositories(
    repositories: &[String],
    cx: &mut App,
) -> Result<PersistenceResult, String> {
    store(cx).save_session(rules::session_entries(repositories))
}

/// Queues a barrier after earlier jobs and retries any pending file actions.
/// Await the result before closing a window that requires persistence.
pub fn flush(cx: &mut App) -> Result<PersistenceResult, String> {
    store(cx).flush()
}
