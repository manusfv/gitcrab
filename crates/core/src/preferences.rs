//! Desktop preferences, the remembered session and repository trust, with the
//! rules that keep them consistent. Storage sits behind [`SettingsStore`].
use crate::{
    appearance::ThemePreference,
    history_columns::HistoryColumns,
    platform::RepositoryLocation,
    presentation::{display_path, same_path, unique_paths},
    updates::UpdateChannel,
};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};
use tokio::sync::oneshot;

/// Most repositories kept in the recent list.
pub const MAX_RECENT: usize = 10;

/// Saved desktop appearance, layout and recent repository settings.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Preferences {
    pub compact: bool,
    pub theme: ThemePreference,
    pub font_size: f32,
    pub nav_width: f32,
    pub inspector_width: f32,
    pub split: bool,
    pub code_size: f32,
    pub compact_diff: bool,
    pub always_show_file_actions: bool,
    pub collapsed: BTreeSet<String>,
    pub recent: Vec<String>,
    /// Column layout for repositories without their own saved layout.
    /// Older versions saved one layout for every repository here.
    pub history_columns: HistoryColumns,
    /// Column layouts saved per repository, keyed by its familiar path.
    pub repository_columns: BTreeMap<String, HistoryColumns>,
    /// Main window size when GitCrab last closed.
    pub window: Option<WindowSize>,
    /// Whether GitCrab checks for updates on its own, at most once a day.
    pub check_for_updates: bool,
    /// The release channel the user chose. `None` follows the running
    /// build; see [`UpdateChannel::effective`].
    pub update_channel: Option<UpdateChannel>,
    /// A release version the user chose not to be told about again.
    pub skipped_update: Option<String>,
    /// When the last successful update check finished, in Unix seconds.
    pub last_update_check: Option<u64>,
}

/// Saved main window size in logical pixels.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct WindowSize {
    pub width: f32,
    pub height: f32,
    #[serde(default)]
    pub maximized: bool,
}

impl WindowSize {
    pub const MIN_WIDTH: f32 = 960.;
    pub const MIN_HEIGHT: f32 = 600.;
    /// Largest width or height kept, which drops sizes saved from corrupt state.
    pub const MAX_DIMENSION: f32 = 16384.;

    /// Drops sizes that are not finite and keeps the rest within supported bounds.
    pub fn normalized(self) -> Option<Self> {
        (self.width.is_finite() && self.height.is_finite()).then(|| Self {
            width: self.width.clamp(Self::MIN_WIDTH, Self::MAX_DIMENSION),
            height: self.height.clamp(Self::MIN_HEIGHT, Self::MAX_DIMENSION),
            maximized: self.maximized,
        })
    }
}

impl Default for Preferences {
    fn default() -> Self {
        Self {
            compact: false,
            theme: Default::default(),
            font_size: Self::DEFAULT_FONT_SIZE,
            nav_width: Self::DEFAULT_NAV_WIDTH,
            inspector_width: Self::DEFAULT_INSPECTOR_WIDTH,
            split: false,
            code_size: Self::DEFAULT_FONT_SIZE,
            compact_diff: false,
            always_show_file_actions: true,
            collapsed: Default::default(),
            recent: vec![],
            history_columns: Default::default(),
            repository_columns: Default::default(),
            window: None,
            check_for_updates: true,
            update_channel: None,
            skipped_update: None,
            last_update_check: None,
        }
    }
}

/// `value` within `min..=max`, or `default` when it is not finite.
fn clamped(value: f32, min: f32, max: f32, default: f32) -> f32 {
    if value.is_finite() {
        value.clamp(min, max)
    } else {
        default
    }
}

impl Preferences {
    /// Interface and code text size, in pixels.
    pub const DEFAULT_FONT_SIZE: f32 = 13.;
    pub const MIN_FONT_SIZE: f32 = 11.;
    pub const MAX_FONT_SIZE: f32 = 26.;
    /// Width of the navigation sidebar, in pixels.
    pub const DEFAULT_NAV_WIDTH: f32 = 210.;
    pub const MIN_NAV_WIDTH: f32 = 150.;
    pub const MAX_NAV_WIDTH: f32 = 330.;
    /// Width of the inspector, in pixels.
    pub const DEFAULT_INSPECTOR_WIDTH: f32 = 360.;
    pub const MIN_INSPECTOR_WIDTH: f32 = 260.;
    pub const MAX_INSPECTOR_WIDTH: f32 = 560.;

    /// Keeps loaded values within supported bounds and tidies lists saved by
    /// older versions.
    pub fn normalize(&mut self) {
        self.font_size = clamped(
            self.font_size,
            Self::MIN_FONT_SIZE,
            Self::MAX_FONT_SIZE,
            Self::DEFAULT_FONT_SIZE,
        );
        self.nav_width = clamped(
            self.nav_width,
            Self::MIN_NAV_WIDTH,
            Self::MAX_NAV_WIDTH,
            Self::DEFAULT_NAV_WIDTH,
        );
        self.inspector_width = clamped(
            self.inspector_width,
            Self::MIN_INSPECTOR_WIDTH,
            Self::MAX_INSPECTOR_WIDTH,
            Self::DEFAULT_INSPECTOR_WIDTH,
        );
        self.code_size = clamped(
            self.code_size,
            Self::MIN_FONT_SIZE,
            Self::MAX_FONT_SIZE,
            Self::DEFAULT_FONT_SIZE,
        );
        // Lists saved by older versions may hold several spellings of one folder.
        self.recent = unique_paths(std::mem::take(&mut self.recent));
        self.recent.truncate(MAX_RECENT);
        self.history_columns.normalize();
        self.repository_columns
            .values_mut()
            .for_each(HistoryColumns::normalize);
        self.window = self.window.and_then(WindowSize::normalized);
        self.skipped_update = self
            .skipped_update
            .take()
            .map(|version| version.trim().to_owned())
            .filter(|version| !version.is_empty());
    }

    /// Moves `path` to the top of the recent list, dropping other spellings of
    /// the same folder. Returns `false` when it was already first.
    pub fn promote_recent(&mut self, path: &str) -> bool {
        let path = display_path(path);
        if self.recent.first() == Some(&path) {
            return false;
        }
        self.recent.retain(|recent| !same_path(recent, &path));
        self.recent.insert(0, path);
        self.recent.truncate(MAX_RECENT);
        true
    }

    /// Returns the key under which a repository's own settings are saved.
    pub fn repository_key(repository: &str) -> String {
        display_path(repository)
    }

    /// Returns the history column layout saved for `repository`, or the shared fallback.
    pub fn columns_for(&self, repository: &str) -> HistoryColumns {
        self.repository_columns
            .get(&Self::repository_key(repository))
            .unwrap_or(&self.history_columns)
            .clone()
    }

    /// Applies one tab's edits since `previous` to these shared preferences.
    ///
    /// Only fields the tab changed are copied, so changes other tabs made to
    /// fields this tab left alone survive. App-wide appearance (theme and
    /// font size) and the update state the app records (the skipped version
    /// and the last check) are never taken from a tab.
    pub fn merge_tab_changes(&mut self, previous: &Self, edited: &Self) {
        macro_rules! merge {
            ($($field:ident),+ $(,)?) => {$(
                if edited.$field != previous.$field {
                    self.$field = edited.$field;
                }
            )+};
        }
        merge!(
            compact,
            nav_width,
            inspector_width,
            split,
            code_size,
            compact_diff,
            always_show_file_actions,
            check_for_updates,
            update_channel
        );
        for (repository, columns) in &edited.repository_columns {
            if previous.repository_columns.get(repository) != Some(columns) {
                self.repository_columns
                    .insert(repository.clone(), columns.clone());
            }
        }
        for key in previous.collapsed.difference(&edited.collapsed) {
            self.collapsed.remove(key);
        }
        self.collapsed
            .extend(edited.collapsed.difference(&previous.collapsed).cloned());
    }
}

/// The open repositories as saved for session restoration: familiar paths in
/// tab order. Opening canonicalizes them again.
pub fn session_entries(repositories: &[String]) -> Vec<String> {
    repositories.iter().map(|p| display_path(p)).collect()
}

/// Removes every spelling of `repository` from a saved session. Returns
/// whether anything was removed.
pub fn forget_session_entry(session: &mut Vec<String>, repository: &str) -> bool {
    let before = session.len();
    let expected = display_path(repository);
    session.retain(|p| display_path(p) != expected);
    session.len() != before
}

/// Whether the user trusted exactly this worktree and metadata paths.
pub fn is_trusted(trusted: &[RepositoryLocation], location: &RepositoryLocation) -> bool {
    trusted.contains(location)
}

/// Records trust in `location`, replacing earlier trust in the same worktree.
pub fn remember_trust(trusted: &mut Vec<RepositoryLocation>, location: &RepositoryLocation) {
    trusted.retain(|entry| entry.worktree != location.worktree);
    trusted.push(location.clone());
}

/// Completion of a queued settings write or flush.
pub type PersistenceResult = oneshot::Receiver<Result<(), String>>;

/// Where preferences, the session and repository trust are kept.
///
/// Writes are queued and applied in order; each returns a receiver for its
/// outcome. A failed write stays pending and is retried by [`Self::flush`].
pub trait SettingsStore: Send + Sync {
    /// Reads saved preferences as stored, before [`Preferences::normalize`].
    /// `None` when nothing usable is saved.
    fn load_preferences(&self) -> Option<Preferences>;

    /// Queues replacing the saved preferences.
    ///
    /// # Errors
    ///
    /// Returns a message if the write cannot be queued.
    fn save_preferences(&self, preferences: &Preferences) -> Result<PersistenceResult, String>;

    /// Reads the repositories open in the last session, in tab order.
    fn session(&self) -> Vec<String>;

    /// Queues replacing the saved session with these entries.
    ///
    /// # Errors
    ///
    /// Returns a message if the write cannot be queued.
    fn save_session(&self, entries: Vec<String>) -> Result<PersistenceResult, String>;

    /// Queues removing `repository` from the saved session, applying
    /// [`forget_session_entry`] to whatever is saved when the write runs.
    ///
    /// # Errors
    ///
    /// Returns a message if the write cannot be queued.
    fn forget_session_entry(&self, repository: &str) -> Result<PersistenceResult, String>;

    /// Reads every trusted repository location.
    fn trusted(&self) -> Vec<RepositoryLocation>;

    /// Queues recording trust, applying [`remember_trust`] to whatever is
    /// saved when the write runs.
    ///
    /// # Errors
    ///
    /// Returns a message if the write cannot be queued.
    fn remember_trust(&self, location: &RepositoryLocation) -> Result<PersistenceResult, String>;

    /// Queues a barrier after earlier writes and retries any that failed.
    ///
    /// # Errors
    ///
    /// Returns a message if the barrier cannot be queued.
    fn flush(&self) -> Result<PersistenceResult, String>;
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::history_columns::HistoryColumn;
    use std::path::PathBuf;

    #[test]
    fn old_tab_settings_do_not_replace_app_wide_appearance() {
        let original = Preferences::default();
        let mut shared = original.clone();
        shared.font_size = 21.;
        let mut edited = original.clone();
        edited.font_size = 12.;
        edited.nav_width = 300.;
        shared.merge_tab_changes(&original, &edited);
        assert_eq!(shared.font_size, 21.);
        assert_eq!(shared.nav_width, 300.);
    }

    #[test]
    fn collapsed_sections_merge_both_ways() {
        let previous = Preferences {
            collapsed: ["a".to_owned(), "b".to_owned()].into(),
            ..Default::default()
        };
        let mut edited = previous.clone();
        edited.collapsed.remove("a");
        edited.collapsed.insert("c".into());
        let mut shared = previous.clone();
        shared.collapsed.insert("other-tab".into());
        shared.merge_tab_changes(&previous, &edited);
        let expected: BTreeSet<String> = ["b", "c", "other-tab"].map(String::from).into();
        assert_eq!(shared.collapsed, expected);
    }

    #[test]
    fn old_preferences_keep_values_and_gain_the_default_palette() {
        let p: Preferences = serde_json::from_str(
            r#"{"compact":true,"font_size":15,"recent":["example"],"split":true}"#,
        )
        .unwrap();
        assert!(p.compact && p.split);
        assert_eq!(p.font_size, 15.);
        assert_eq!(p.recent, ["example"]);
        assert_eq!(p.history_columns, Default::default());
        assert_eq!(p.theme, ThemePreference::Dark);
        let p: Preferences = serde_json::from_str(r#"{"theme":"missing","code_size":17}"#).unwrap();
        assert_eq!(p.code_size, 17.);
        assert_eq!(p.theme.resolve(false), "dark");
    }

    #[test]
    fn old_preferences_check_for_updates_on_the_build_channel() {
        let p: Preferences = serde_json::from_str(r#"{"compact":true}"#).unwrap();
        assert!(p.check_for_updates);
        assert_eq!(p.update_channel, None);
        assert_eq!(p.skipped_update, None);
        assert_eq!(p.last_update_check, None);
        let p: Preferences = serde_json::from_str(
            r#"{"check_for_updates":false,"update_channel":"stable","skipped_update":"0.2.0","last_update_check":42}"#,
        )
        .unwrap();
        assert!(!p.check_for_updates);
        assert_eq!(p.update_channel, Some(UpdateChannel::Stable));
        assert_eq!(p.skipped_update.as_deref(), Some("0.2.0"));
        assert_eq!(p.last_update_check, Some(42));
    }

    #[test]
    fn update_choices_merge_but_recorded_update_state_stays_shared() {
        let original = Preferences::default();
        let mut shared = original.clone();
        shared.skipped_update = Some("0.2.0".into());
        shared.last_update_check = Some(100);
        let mut edited = original.clone();
        edited.check_for_updates = false;
        edited.update_channel = Some(UpdateChannel::Stable);
        edited.last_update_check = Some(1);
        shared.merge_tab_changes(&original, &edited);
        assert!(!shared.check_for_updates);
        assert_eq!(shared.update_channel, Some(UpdateChannel::Stable));
        assert_eq!(shared.skipped_update.as_deref(), Some("0.2.0"));
        assert_eq!(shared.last_update_check, Some(100));
    }

    #[test]
    fn blank_skipped_versions_are_dropped() {
        let mut p = Preferences {
            skipped_update: Some("  ".into()),
            ..Default::default()
        };
        p.normalize();
        assert_eq!(p.skipped_update, None);
        p.skipped_update = Some(" 0.2.0 ".into());
        p.normalize();
        assert_eq!(p.skipped_update.as_deref(), Some("0.2.0"));
    }

    #[test]
    fn column_layouts_are_kept_per_repository() {
        let mut legacy = HistoryColumns::default();
        legacy.toggle(HistoryColumn::Sha);
        let previous = Preferences {
            history_columns: legacy.clone(),
            ..Default::default()
        };
        // Repositories without their own layout use the one older versions saved.
        assert_eq!(previous.columns_for("one"), legacy);

        let mut one = HistoryColumns::default();
        one.toggle(HistoryColumn::Author);
        let mut edited = previous.clone();
        edited.repository_columns.insert("one".into(), one.clone());

        // Another tab saved a layout for a different repository meanwhile.
        let mut shared = previous.clone();
        let mut two = HistoryColumns::default();
        two.toggle(HistoryColumn::Graph);
        shared.repository_columns.insert("two".into(), two.clone());
        shared.merge_tab_changes(&previous, &edited);

        assert_eq!(shared.columns_for("one"), one);
        assert_eq!(shared.columns_for("two"), two);
        assert_eq!(shared.columns_for("three"), legacy);
        assert_eq!(shared.history_columns, legacy);
    }

    #[test]
    fn window_size_is_normalized() {
        let p: Preferences =
            serde_json::from_str(r#"{"window":{"width":100,"height":5000}}"#).unwrap();
        let size = p.window.and_then(WindowSize::normalized).unwrap();
        assert_eq!((size.width, size.height), (WindowSize::MIN_WIDTH, 5000.));
        assert!(!size.maximized);
        let nan = WindowSize {
            width: f32::NAN,
            height: 800.,
            maximized: true,
        };
        assert_eq!(nan.normalized(), None);
    }

    #[test]
    fn loaded_values_are_clamped_and_unreadable_ones_reset() {
        let mut p = Preferences {
            font_size: 40.,
            nav_width: f32::NAN,
            inspector_width: 10.,
            code_size: f32::INFINITY,
            recent: (0..15).map(|i| format!("repo-{i}")).collect(),
            ..Default::default()
        };
        p.normalize();
        assert_eq!(p.font_size, 26.);
        assert_eq!(p.nav_width, 210.);
        assert_eq!(p.inspector_width, 260.);
        assert_eq!(p.code_size, 13.);
        assert_eq!(p.recent.len(), MAX_RECENT);
    }

    #[test]
    fn promoting_a_recent_repository_moves_it_first_once() {
        let mut p = Preferences {
            recent: vec!["a".into(), "b".into(), "c".into()],
            ..Default::default()
        };
        assert!(p.promote_recent("c"));
        assert_eq!(p.recent, ["c", "a", "b"]);
        assert!(!p.promote_recent("c"));
        p.recent = (0..MAX_RECENT).map(|i| i.to_string()).collect();
        assert!(p.promote_recent("new"));
        assert_eq!(p.recent.len(), MAX_RECENT);
        assert_eq!(p.recent[0], "new");
    }

    #[test]
    fn forgetting_a_session_entry_removes_it_only_when_present() {
        let mut session = vec!["a".to_owned(), "b".to_owned()];
        assert!(!forget_session_entry(&mut session, "missing"));
        assert!(forget_session_entry(&mut session, "a"));
        assert_eq!(session, ["b"]);
    }

    #[test]
    fn remembered_trust_requires_matching_worktree_and_metadata() {
        let worktree = PathBuf::from("repo");
        let original = RepositoryLocation {
            git_dir: worktree.join(".git"),
            common_dir: worktree.join(".git"),
            worktree,
        };
        let new_git_dir = RepositoryLocation {
            git_dir: "moved-metadata".into(),
            ..original.clone()
        };
        let new_common_dir = RepositoryLocation {
            common_dir: "shared-metadata".into(),
            ..original.clone()
        };
        let other_worktree = RepositoryLocation {
            worktree: "other".into(),
            ..original.clone()
        };

        let mut trusted = vec![];
        assert!(!is_trusted(&trusted, &original));
        remember_trust(&mut trusted, &original);
        assert!(is_trusted(&trusted, &original));
        assert!(!is_trusted(&trusted, &new_git_dir));
        assert!(!is_trusted(&trusted, &new_common_dir));
        assert!(!is_trusted(&trusted, &other_worktree));

        // Trusting new metadata for the same worktree replaces the old entry.
        remember_trust(&mut trusted, &other_worktree);
        remember_trust(&mut trusted, &new_git_dir);
        assert_eq!(trusted, [other_worktree, new_git_dir]);
        assert!(!is_trusted(&trusted, &original));
    }
}
