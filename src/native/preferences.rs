//! Desktop preferences, remembered repositories and serialized persistence.
use gitcrab_core::platform::RepositoryLocation;
use gpui::{App, Global};
use serde::{Deserialize, Serialize};
use std::{
    collections::{BTreeMap, VecDeque},
    ffi::OsString,
    io::Write,
    path::{Path, PathBuf},
};
use tokio::sync::oneshot;

/// Completion of a queued settings write or flush.
pub type PersistenceResult = oneshot::Receiver<Result<(), String>>;

enum PersistenceAction {
    File(PathBuf, FileAction),
    Barrier,
}

enum FileAction {
    Write(Vec<u8>),
    ForgetLast(String),
    RememberTrust(TrustedRepository),
}

struct PersistenceJob {
    action: PersistenceAction,
    reply: oneshot::Sender<Result<(), String>>,
}

struct PersistenceWriter(std::sync::mpsc::SyncSender<PersistenceJob>);
impl Global for PersistenceWriter {}

fn start_writer() -> std::sync::mpsc::SyncSender<PersistenceJob> {
    let (sender, receiver) = std::sync::mpsc::sync_channel::<PersistenceJob>(64);
    std::thread::Builder::new()
        .name("gitcrab-settings".into())
        .spawn(move || {
            let mut pending = BTreeMap::<PathBuf, VecDeque<FileAction>>::new();
            for job in receiver {
                let result = match job.action {
                    PersistenceAction::File(path, action) => {
                        let actions = pending.entry(path.clone()).or_default();
                        // A full replacement supersedes older actions for this path.
                        if matches!(action, FileAction::Write(_)) {
                            actions.clear();
                        }
                        actions.push_back(action);
                        let result = persist_file_actions(&path, actions);
                        if actions.is_empty() {
                            pending.remove(&path);
                        }
                        result
                    }
                    PersistenceAction::Barrier => {
                        let mut result = Ok(());
                        pending.retain(|path, actions| match persist_file_actions(path, actions) {
                            Ok(()) => false,
                            Err(error) => {
                                result = Err(error);
                                true
                            }
                        });
                        result
                    }
                };
                let _ = job.reply.send(result);
            }
        })
        .expect("start settings writer");
    sender
}

fn persist_file_actions(path: &Path, actions: &mut VecDeque<FileAction>) -> Result<(), String> {
    while let Some(action) = actions.front() {
        match action {
            FileAction::Write(bytes) => atomic_write(path, bytes).map_err(|e| e.to_string())?,
            FileAction::RememberTrust(location) => {
                write_trust(path, location).map_err(|e| e.to_string())?
            }
            FileAction::ForgetLast(expected) => match std::fs::read(path) {
                Ok(bytes) => {
                    let mut paths = parse_session(&bytes);
                    let before = paths.len();
                    let expected = gitcrab_core::presentation::display_path(expected);
                    paths.retain(|p| gitcrab_core::presentation::display_path(p) != expected);
                    if paths.len() != before {
                        if paths.is_empty() {
                            std::fs::remove_file(path).map_err(|e| e.to_string())?;
                        } else {
                            let bytes = serde_json::to_vec(&paths).map_err(|e| e.to_string())?;
                            atomic_write(path, &bytes).map_err(|e| e.to_string())?;
                        }
                    }
                }
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
                Err(error) => return Err(error.to_string()),
            },
        }
        actions.pop_front();
    }
    Ok(())
}

fn persist(action: PersistenceAction, cx: &mut App) -> Result<PersistenceResult, String> {
    if !cx.has_global::<PersistenceWriter>() {
        cx.set_global(PersistenceWriter(start_writer()));
    }
    let (reply, result) = oneshot::channel();
    cx.global::<PersistenceWriter>()
        .0
        .try_send(PersistenceJob { action, reply })
        .map_err(|_| "The settings writer is unavailable or busy.".to_owned())?;
    Ok(result)
}
/// Saved desktop appearance, layout and recent repository settings.
#[derive(Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Preferences {
    pub compact: bool,
    pub theme: gitcrab_core::appearance::ThemePreference,
    pub font_size: f32,
    pub nav_width: f32,
    pub inspector_width: f32,
    pub split: bool,
    pub code_size: f32,
    pub compact_diff: bool,
    pub always_show_file_actions: bool,
    pub collapsed: std::collections::BTreeSet<String>,
    pub recent: Vec<String>,
    pub history_columns: gitcrab_core::history_columns::HistoryColumns,
}
impl Default for Preferences {
    fn default() -> Self {
        Self {
            compact: false,
            theme: Default::default(),
            font_size: 13.,
            nav_width: 210.,
            inspector_width: 360.,
            split: false,
            code_size: 13.,
            compact_diff: false,
            always_show_file_actions: true,
            collapsed: Default::default(),
            recent: vec![],
            history_columns: Default::default(),
        }
    }
}
struct SharedPreferences(Preferences);
impl Global for SharedPreferences {}

impl Preferences {
    /// Reads shared in-memory preferences, loading them from disk on first use.
    pub fn current(cx: &mut App) -> Self {
        if let Some(shared) = cx.try_global::<SharedPreferences>() {
            shared.0.clone()
        } else {
            let preferences = Self::load();
            cx.set_global(SharedPreferences(preferences.clone()));
            preferences
        }
    }

    /// Updates the shared appearance and applies the font size immediately.
    pub fn set_app_appearance(
        font_size: f32,
        theme: gitcrab_core::appearance::ThemePreference,
        cx: &mut App,
    ) {
        let mut shared = Self::current(cx);
        shared.font_size = font_size;
        shared.theme = theme;
        cx.set_global(SharedPreferences(shared));
        gpui_component::Theme::global_mut(cx).font_size = gpui::px(font_size);
    }

    /// Promotes a repository to the top of the recent list and queues a save.
    /// Returns `Ok(None)` if it is already first.
    pub fn add_recent(path: &str, cx: &mut App) -> Result<Option<PersistenceResult>, String> {
        let path = &gitcrab_core::presentation::display_path(path);
        let mut shared = Self::current(cx);
        if shared.recent.first().is_some_and(|recent| recent == path) {
            return Ok(None);
        }
        shared.recent.retain(|recent| recent != path);
        shared.recent.insert(0, path.to_owned());
        shared.recent.truncate(10);
        cx.set_global(SharedPreferences(shared.clone()));
        shared.save(cx).map(Some)
    }

    /// Merges this tab's edits since `previous` into shared preferences and queues a save.
    /// Changes made by other tabs to fields this tab left alone are preserved.
    pub fn save_changes(&self, previous: &Self, cx: &mut App) -> Result<PersistenceResult, String> {
        let mut shared = Self::current(cx);
        shared.merge_tab_changes(previous, self);
        let result = shared.save(cx)?;
        cx.set_global(SharedPreferences(shared));
        Ok(result)
    }

    fn merge_tab_changes(&mut self, previous: &Self, edited: &Self) {
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
            always_show_file_actions
        );
        if edited.history_columns != previous.history_columns {
            self.history_columns = edited.history_columns.clone();
        }
        for key in previous.collapsed.difference(&edited.collapsed) {
            self.collapsed.remove(key);
        }
        self.collapsed
            .extend(edited.collapsed.difference(&previous.collapsed).cloned());
    }

    fn path() -> Option<PathBuf> {
        if let Some(p) = std::env::var_os("GITCRAB_SETTINGS") {
            return Some(p.into());
        }
        #[cfg(windows)]
        let base = PathBuf::from(std::env::var_os("APPDATA")?);
        #[cfg(target_os = "macos")]
        let base = PathBuf::from(std::env::var_os("HOME")?).join("Library/Application Support");
        #[cfg(target_os = "linux")]
        let base = std::env::var_os("XDG_CONFIG_HOME")
            .map(PathBuf::from)
            .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".config")))?;
        let dir = base.join("GitCrab");
        migrate_legacy_files(&dir);
        Some(dir.join("preferences.json"))
    }
    fn session_path() -> Option<PathBuf> {
        Self::sibling_path("session")
    }

    fn trust_path() -> Option<PathBuf> {
        Self::sibling_path("trust")
    }

    /// Names a companion file next to the preferences file. A `GITCRAB_SETTINGS`
    /// override keeps deriving from its own name so overrides never share files.
    fn sibling_path(name: &str) -> Option<PathBuf> {
        let path = Self::path()?;
        if std::env::var_os("GITCRAB_SETTINGS").is_some() {
            Some(path.with_extension(format!("{name}.json")))
        } else {
            Some(path.with_file_name(format!("{name}.json")))
        }
    }

    /// Checks for saved trust in this exact worktree and its metadata paths.
    pub fn trusts_repository(location: &RepositoryLocation) -> bool {
        Self::trust_path()
            .and_then(|path| read_trust(&path).ok())
            .is_some_and(|trusted| trusted.contains(&TrustedRepository::from(location)))
    }

    /// Queues persistence of the user's trust in the worktree and its metadata paths.
    pub fn remember_trust(
        location: &RepositoryLocation,
        cx: &mut App,
    ) -> Result<PersistenceResult, String> {
        let path = Self::trust_path().ok_or("No configuration directory is available")?;
        persist(
            PersistenceAction::File(path, FileAction::RememberTrust(location.into())),
            cx,
        )
    }

    /// Reads the last repository saved for session restoration, if available.
    pub fn last_repository() -> Option<String> {
        Self::session_repositories().into_iter().next()
    }

    /// Reads every repository that was open in the last session, in tab order.
    /// Accepts the older single-path format.
    pub fn session_repositories() -> Vec<String> {
        let Some(bytes) = Self::session_path().and_then(|p| std::fs::read(p).ok()) else {
            return vec![];
        };
        parse_session(&bytes)
    }

    /// Queues clearing the saved session path if it still matches `repository`.
    pub fn forget_last_repository(
        repository: &str,
        cx: &mut App,
    ) -> Result<PersistenceResult, String> {
        let path = Self::session_path().ok_or("No configuration directory is available")?;
        persist(
            PersistenceAction::File(path, FileAction::ForgetLast(repository.to_owned())),
            cx,
        )
    }

    /// Queues saving the open repositories (tab order) for session restoration.
    pub fn remember_repositories(
        repositories: &[String],
        cx: &mut App,
    ) -> Result<PersistenceResult, String> {
        let path = Self::session_path().ok_or("No configuration directory is available")?;
        // Store familiar paths; opening canonicalizes them again.
        let familiar: Vec<String> = repositories
            .iter()
            .map(|p| gitcrab_core::presentation::display_path(p))
            .collect();
        let bytes = serde_json::to_vec(&familiar).map_err(|e| e.to_string())?;
        persist(PersistenceAction::File(path, FileAction::Write(bytes)), cx)
    }

    /// Reads saved preferences and normalizes the layout, falling back to defaults.
    pub fn load() -> Self {
        let mut p = Self::path()
            .and_then(|p| std::fs::read(p).ok())
            .and_then(|b| serde_json::from_slice::<Self>(&b).ok())
            .unwrap_or_default();
        p.font_size = if p.font_size.is_finite() {
            p.font_size.clamp(11., 26.)
        } else {
            13.
        };
        p.nav_width = if p.nav_width.is_finite() {
            p.nav_width.clamp(150., 330.)
        } else {
            210.
        };
        p.inspector_width = if p.inspector_width.is_finite() {
            p.inspector_width.clamp(260., 560.)
        } else {
            360.
        };
        p.code_size = if p.code_size.is_finite() {
            p.code_size.clamp(11., 26.)
        } else {
            13.
        };
        p.recent.truncate(10);
        p.history_columns.normalize();
        p
    }
    fn save(&self, cx: &mut App) -> Result<PersistenceResult, String> {
        let path = Self::path().ok_or("No configuration directory is available")?;
        let bytes = serde_json::to_vec_pretty(self).map_err(|e| e.to_string())?;
        persist(PersistenceAction::File(path, FileAction::Write(bytes)), cx)
    }

    /// Queues a barrier after earlier jobs and retries any pending file actions.
    /// Await the result before closing a window that requires persistence.
    pub fn flush(cx: &mut App) -> Result<PersistenceResult, String> {
        persist(PersistenceAction::Barrier, cx)
    }
}

/// Renames files from the old `appearance*` names so saved preferences, open
/// tabs and repository trust survive the rename. Never overwrites a new file.
fn migrate_legacy_files(dir: &Path) {
    static ONCE: std::sync::Once = std::sync::Once::new();
    ONCE.call_once(|| {
        for (old, new) in [
            ("appearance.json", "preferences.json"),
            ("appearance.session.json", "session.json"),
            ("appearance.trust.json", "trust.json"),
            ("appearance.trust.lock", "trust.lock"),
        ] {
            let (old, new) = (dir.join(old), dir.join(new));
            if old.exists() && !new.exists() {
                let _ = std::fs::rename(old, new);
            }
        }
    });
}

fn parse_session(bytes: &[u8]) -> Vec<String> {
    let paths = serde_json::from_slice::<Vec<String>>(bytes)
        .or_else(|_| serde_json::from_slice::<String>(bytes).map(|p| vec![p]))
        .unwrap_or_default();
    paths.into_iter().filter(|p| !p.is_empty()).collect()
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
struct TrustedRepository {
    worktree: PathBuf,
    // Git metadata paths can contain non-Unicode characters.
    git_dir: OsString,
    common_dir: OsString,
}

impl From<&RepositoryLocation> for TrustedRepository {
    fn from(location: &RepositoryLocation) -> Self {
        Self {
            worktree: location.worktree.clone(),
            git_dir: location.git_dir.as_os_str().to_owned(),
            common_dir: location.common_dir.as_os_str().to_owned(),
        }
    }
}

fn read_trust(path: &Path) -> std::io::Result<Vec<TrustedRepository>> {
    match std::fs::read(path) {
        Ok(bytes) => Ok(serde_json::from_slice(&bytes).unwrap_or_default()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(Vec::new()),
        Err(error) => Err(error),
    }
}

fn write_trust(path: &Path, location: &TrustedRepository) -> std::io::Result<()> {
    if let Some(parent) = path
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
    {
        std::fs::create_dir_all(parent)?;
    }
    // Lock a separate file so replacing the trust file keeps the lock in place.
    let lock = std::fs::OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(false)
        .open(path.with_extension("lock"))?;
    lock.lock()?;
    let mut trusted = read_trust(path)?;
    trusted.retain(|entry| entry.worktree != location.worktree);
    trusted.push(location.clone());
    atomic_write(path, &serde_json::to_vec_pretty(&trusted)?)
}

fn atomic_write(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    use std::sync::atomic::{AtomicU64, Ordering::Relaxed};
    static NEXT_WRITE: AtomicU64 = AtomicU64::new(0);
    if let Some(parent) = path
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
    {
        std::fs::create_dir_all(parent)?;
    }
    let serial = NEXT_WRITE.fetch_add(1, Relaxed);
    let temporary = path.with_extension(format!("{}-{serial}.tmp", std::process::id()));
    let mut file = std::fs::File::create_new(&temporary)?;
    let result = file.write_all(bytes).and_then(|_| file.sync_all());
    drop(file);
    let result = result.and_then(|_| std::fs::rename(&temporary, path));
    if result.is_err() {
        let _ = std::fs::remove_file(&temporary);
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    #[::core::prelude::v1::test]
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

    #[::core::prelude::v1::test]
    fn queued_settings_writes_preserve_order_and_retry_failed_flushes() {
        let root = std::env::temp_dir().join(format!(
            "gitcrab-writer-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir(&root).unwrap();
        let path = root.join("appearance.json");
        let writer = start_writer();
        let send = |action| {
            let (reply, result) = oneshot::channel();
            writer.send(PersistenceJob { action, reply }).unwrap();
            result
        };
        let write = |path: &Path, bytes: &[u8]| {
            send(PersistenceAction::File(
                path.to_path_buf(),
                FileAction::Write(bytes.to_vec()),
            ))
        };
        let flush = || send(PersistenceAction::Barrier).blocking_recv().unwrap();
        let first = write(&path, b"first");
        let second = write(&path, b"second");
        first.blocking_recv().unwrap().unwrap();
        second.blocking_recv().unwrap().unwrap();
        flush().unwrap();
        assert_eq!(std::fs::read(&path).unwrap(), b"second");

        let blocked = root.join("blocked.json");
        std::fs::create_dir(&blocked).unwrap();
        assert!(write(&blocked, b"old").blocking_recv().unwrap().is_err());
        assert!(write(&blocked, b"latest").blocking_recv().unwrap().is_err());
        write(&path, b"independent")
            .blocking_recv()
            .unwrap()
            .unwrap();
        assert!(flush().is_err());
        assert!(flush().is_err());
        std::fs::remove_dir(&blocked).unwrap();
        flush().unwrap();
        assert_eq!(std::fs::read(&blocked).unwrap(), b"latest");
        assert_eq!(std::fs::read(&path).unwrap(), b"independent");

        let session = root.join("session.json");
        std::fs::create_dir(&session).unwrap();
        assert!(write(&session, br#""old-repository""#)
            .blocking_recv()
            .unwrap()
            .is_err());
        assert!(send(PersistenceAction::File(
            session.clone(),
            FileAction::ForgetLast("old-repository".to_owned()),
        ))
        .blocking_recv()
        .unwrap()
        .is_err());
        std::fs::remove_dir(&session).unwrap();
        flush().unwrap();
        assert!(!session.exists());

        std::fs::write(&session, b"invalid json").unwrap();
        send(PersistenceAction::File(
            session.clone(),
            FileAction::ForgetLast("old-repository".to_owned()),
        ))
        .blocking_recv()
        .unwrap()
        .unwrap();
        assert_eq!(std::fs::read(&session).unwrap(), b"invalid json");
        drop(writer);
        std::fs::remove_dir_all(root).unwrap();
    }
    #[::core::prelude::v1::test]
    fn appearance_replacement_is_atomic_and_cleans_failed_temporary_files() {
        let root = std::env::temp_dir().join(format!(
            "gitcrab-settings-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir(&root).unwrap();
        let path = root.join("appearance.json");
        atomic_write(&path, b"old").unwrap();
        atomic_write(&path, b"new").unwrap();
        assert_eq!(std::fs::read(&path).unwrap(), b"new");
        let blocked = root.join("directory.json");
        std::fs::create_dir(&blocked).unwrap();
        std::fs::write(blocked.join("keep"), b"preserved").unwrap();
        assert!(atomic_write(&blocked, b"cannot replace a directory").is_err());
        assert_eq!(std::fs::read(blocked.join("keep")).unwrap(), b"preserved");
        assert_eq!(std::fs::read_dir(&root).unwrap().count(), 2);
        std::fs::remove_dir_all(root).unwrap();
    }

    #[::core::prelude::v1::test]
    fn remembered_trust_requires_matching_worktree_and_metadata() {
        let root = std::env::temp_dir().join(format!(
            "gitcrab-trust-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let path = root.join("trust.json");
        let worktree = root.join("repo");
        let original = RepositoryLocation {
            git_dir: worktree.join(".git"),
            common_dir: worktree.join(".git"),
            worktree,
        };
        #[cfg(unix)]
        let metadata = {
            use std::os::unix::ffi::OsStringExt;
            OsString::from_vec(vec![b'm', 0xff])
        };
        #[cfg(windows)]
        let metadata = {
            use std::os::windows::ffi::OsStringExt;
            OsString::from_wide(&[b'm' as u16, 0xd800])
        };
        let new_git_dir = RepositoryLocation {
            git_dir: root.join(metadata),
            ..original.clone()
        };
        let new_common_dir = RepositoryLocation {
            common_dir: root.join("shared-metadata"),
            ..original.clone()
        };
        let other_worktree = RepositoryLocation {
            worktree: root.join("other"),
            ..original.clone()
        };
        let original = TrustedRepository::from(&original);
        let new_git_dir = TrustedRepository::from(&new_git_dir);
        let new_common_dir = TrustedRepository::from(&new_common_dir);
        let other_worktree = TrustedRepository::from(&other_worktree);

        assert!(read_trust(&path).unwrap().is_empty());
        write_trust(&path, &original).unwrap();
        let trusted = read_trust(&path).unwrap();
        assert_eq!(trusted, std::slice::from_ref(&original));
        assert!(!trusted.contains(&new_git_dir));
        assert!(!trusted.contains(&new_common_dir));
        assert!(!trusted.contains(&other_worktree));

        write_trust(&path, &other_worktree).unwrap();
        write_trust(&path, &new_git_dir).unwrap();
        let trusted = read_trust(&path).unwrap();
        assert_eq!(trusted, [other_worktree, new_git_dir]);
        assert!(!trusted.contains(&original));

        std::fs::write(&path, b"invalid json").unwrap();
        assert!(read_trust(&path).unwrap().is_empty());
        std::fs::remove_dir_all(root).unwrap();
    }

    #[::core::prelude::v1::test]
    fn old_preferences_keep_values_and_gain_the_default_palette() {
        let p: Preferences = serde_json::from_str(
            r#"{"compact":true,"font_size":15,"recent":["example"],"split":true}"#,
        )
        .unwrap();
        assert!(p.compact && p.split);
        assert_eq!(p.font_size, 15.);
        assert_eq!(p.recent, ["example"]);
        assert_eq!(p.history_columns, Default::default());
        assert_eq!(p.theme, gitcrab_core::appearance::ThemePreference::Dark);
        let p: Preferences = serde_json::from_str(r#"{"theme":"missing","code_size":17}"#).unwrap();
        assert_eq!(p.code_size, 17.);
        assert_eq!(p.theme.resolve(false), "dark");
    }
}
