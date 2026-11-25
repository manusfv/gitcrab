//! Settings files behind [`gitcrab_core::preferences::SettingsStore`].
//!
//! [`FileStore`] keeps `preferences.json`, `session.json`, `trust.json` and
//! `trusted-folders.json` in
//! the platform configuration directory, or next to the file named by
//! `GITCRAB_SETTINGS`. One background thread applies writes in order; the
//! rules for what to write come from `gitcrab-core`.
use gitcrab_core::{
    platform::RepositoryLocation,
    preferences::{self, PersistenceResult, Preferences, SettingsStore},
};
use serde::{Deserialize, Serialize};
use std::{
    collections::{BTreeMap, VecDeque},
    ffi::OsString,
    io::Write,
    path::{Path, PathBuf},
    sync::mpsc::SyncSender,
};
use tokio::sync::oneshot;

const NO_DIRECTORY: &str = "No configuration directory is available";

enum PersistenceAction {
    File(PathBuf, FileAction),
    Barrier,
}

enum FileAction {
    Write(Vec<u8>),
    ForgetLast(String),
    RememberTrust(TrustedRepository),
    RememberTrustedFolder(PathBuf),
}

struct PersistenceJob {
    action: PersistenceAction,
    reply: oneshot::Sender<Result<(), String>>,
}

/// The settings files and the thread that writes them.
pub struct FileStore {
    paths: Option<Paths>,
    writer: SyncSender<PersistenceJob>,
}

struct Paths {
    preferences: PathBuf,
    session: PathBuf,
    trust: PathBuf,
    trusted_folders: PathBuf,
}

impl Paths {
    /// Finds the settings files, honouring a `GITCRAB_SETTINGS` override.
    fn locate() -> Option<Self> {
        if let Some(path) = std::env::var_os("GITCRAB_SETTINGS") {
            // An override keeps deriving from its own name so overrides never
            // share files.
            let preferences = PathBuf::from(path);
            return Some(Self {
                session: preferences.with_extension("session.json"),
                trust: preferences.with_extension("trust.json"),
                trusted_folders: preferences.with_extension("trusted-folders.json"),
                preferences,
            });
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
        Some(Self {
            preferences: dir.join("preferences.json"),
            session: dir.join("session.json"),
            trust: dir.join("trust.json"),
            trusted_folders: dir.join("trusted-folders.json"),
        })
    }
}

impl FileStore {
    /// Locates the settings files and starts the writer thread.
    ///
    /// # Panics
    ///
    /// Panics if the writer thread cannot be started.
    pub fn new() -> Self {
        Self {
            paths: Paths::locate(),
            writer: start_writer(),
        }
    }

    fn paths(&self) -> Result<&Paths, String> {
        self.paths.as_ref().ok_or_else(|| NO_DIRECTORY.to_owned())
    }

    fn persist(&self, action: PersistenceAction) -> Result<PersistenceResult, String> {
        let (reply, result) = oneshot::channel();
        self.writer
            .try_send(PersistenceJob { action, reply })
            .map_err(|_| "The settings writer is unavailable or busy.".to_owned())?;
        Ok(result)
    }
}

impl Default for FileStore {
    fn default() -> Self {
        Self::new()
    }
}

impl SettingsStore for FileStore {
    fn load_preferences(&self) -> Option<Preferences> {
        let bytes = std::fs::read(&self.paths.as_ref()?.preferences).ok()?;
        serde_json::from_slice(&bytes).ok()
    }

    fn save_preferences(&self, preferences: &Preferences) -> Result<PersistenceResult, String> {
        let path = self.paths()?.preferences.clone();
        let bytes = serde_json::to_vec_pretty(preferences).map_err(|e| e.to_string())?;
        self.persist(PersistenceAction::File(path, FileAction::Write(bytes)))
    }

    fn session(&self) -> Vec<String> {
        self.paths
            .as_ref()
            .and_then(|paths| std::fs::read(&paths.session).ok())
            .map_or_else(Vec::new, |bytes| parse_session(&bytes))
    }

    fn save_session(&self, entries: Vec<String>) -> Result<PersistenceResult, String> {
        let path = self.paths()?.session.clone();
        let bytes = serde_json::to_vec(&entries).map_err(|e| e.to_string())?;
        self.persist(PersistenceAction::File(path, FileAction::Write(bytes)))
    }

    fn forget_session_entry(&self, repository: &str) -> Result<PersistenceResult, String> {
        let path = self.paths()?.session.clone();
        self.persist(PersistenceAction::File(
            path,
            FileAction::ForgetLast(repository.to_owned()),
        ))
    }

    fn trusted(&self) -> Vec<RepositoryLocation> {
        self.paths
            .as_ref()
            .and_then(|paths| read_trust(&paths.trust).ok())
            .unwrap_or_default()
            .into_iter()
            .map(RepositoryLocation::from)
            .collect()
    }

    fn remember_trust(&self, location: &RepositoryLocation) -> Result<PersistenceResult, String> {
        let path = self.paths()?.trust.clone();
        self.persist(PersistenceAction::File(
            path,
            FileAction::RememberTrust(location.into()),
        ))
    }

    fn trusted_folders(&self) -> Vec<PathBuf> {
        self.paths
            .as_ref()
            .and_then(|paths| read_trusted_folders(&paths.trusted_folders).ok())
            .unwrap_or_default()
    }

    fn remember_trusted_folder(&self, folder: &Path) -> Result<PersistenceResult, String> {
        let path = self.paths()?.trusted_folders.clone();
        self.persist(PersistenceAction::File(
            path,
            FileAction::RememberTrustedFolder(folder.to_owned()),
        ))
    }

    fn flush(&self) -> Result<PersistenceResult, String> {
        self.persist(PersistenceAction::Barrier)
    }
}

fn start_writer() -> SyncSender<PersistenceJob> {
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
            FileAction::RememberTrustedFolder(folder) => {
                write_trusted_folder(path, folder).map_err(|e| e.to_string())?
            }
            FileAction::ForgetLast(expected) => match std::fs::read(path) {
                Ok(bytes) => {
                    let mut paths = parse_session(&bytes);
                    if preferences::forget_session_entry(&mut paths, expected) {
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

/// Reads a saved session. Accepts the older single-path format.
fn parse_session(bytes: &[u8]) -> Vec<String> {
    let paths = serde_json::from_slice::<Vec<String>>(bytes)
        .or_else(|_| serde_json::from_slice::<String>(bytes).map(|p| vec![p]))
        .unwrap_or_default();
    paths.into_iter().filter(|p| !p.is_empty()).collect()
}

/// A trusted location as stored in `trust.json`.
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

impl From<TrustedRepository> for RepositoryLocation {
    fn from(trusted: TrustedRepository) -> Self {
        Self {
            worktree: trusted.worktree,
            git_dir: trusted.git_dir.into(),
            common_dir: trusted.common_dir.into(),
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

/// Locks a file beside `path` until the returned handle drops, so concurrent
/// GitCrab processes apply their read-modify-write of `path` in turn.
fn lock_beside(path: &Path) -> std::io::Result<std::fs::File> {
    if let Some(parent) = path
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
    {
        std::fs::create_dir_all(parent)?;
    }
    // Lock a separate file so replacing `path` keeps the lock in place.
    let lock = std::fs::OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(false)
        .open(path.with_extension("lock"))?;
    lock.lock()?;
    Ok(lock)
}

fn write_trust(path: &Path, location: &TrustedRepository) -> std::io::Result<()> {
    let _lock = lock_beside(path)?;
    let mut trusted: Vec<RepositoryLocation> = read_trust(path)?
        .into_iter()
        .map(RepositoryLocation::from)
        .collect();
    preferences::remember_trust(&mut trusted, &location.clone().into());
    let stored: Vec<TrustedRepository> = trusted.iter().map(TrustedRepository::from).collect();
    atomic_write(path, &serde_json::to_vec_pretty(&stored)?)
}

fn read_trusted_folders(path: &Path) -> std::io::Result<Vec<PathBuf>> {
    match std::fs::read(path) {
        Ok(bytes) => Ok(serde_json::from_slice(&bytes).unwrap_or_default()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(Vec::new()),
        Err(error) => Err(error),
    }
}

fn write_trusted_folder(path: &Path, folder: &Path) -> std::io::Result<()> {
    let _lock = lock_beside(path)?;
    let mut folders = read_trusted_folders(path)?;
    preferences::remember_trusted_folder(&mut folders, folder);
    atomic_write(path, &serde_json::to_vec_pretty(&folders)?)
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

    fn scratch(name: &str) -> PathBuf {
        std::env::temp_dir().join(format!(
            "gitcrab-{name}-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ))
    }

    #[test]
    fn queued_settings_writes_preserve_order_and_retry_failed_flushes() {
        let root = scratch("writer");
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

    #[test]
    fn appearance_replacement_is_atomic_and_cleans_failed_temporary_files() {
        let root = scratch("settings");
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

    #[test]
    fn trust_file_round_trips_non_unicode_metadata_and_replaces_by_worktree() {
        let root = scratch("trust");
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
        let other_worktree = RepositoryLocation {
            worktree: root.join("other"),
            ..original.clone()
        };
        let read = || -> Vec<RepositoryLocation> {
            read_trust(&path)
                .unwrap()
                .into_iter()
                .map(RepositoryLocation::from)
                .collect()
        };

        assert!(read().is_empty());
        write_trust(&path, &(&original).into()).unwrap();
        assert_eq!(read(), std::slice::from_ref(&original));
        write_trust(&path, &(&other_worktree).into()).unwrap();
        write_trust(&path, &(&new_git_dir).into()).unwrap();
        assert_eq!(read(), [other_worktree, new_git_dir]);

        std::fs::write(&path, b"invalid json").unwrap();
        assert!(read().is_empty());
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn trusted_folders_file_keeps_the_widest_folders() {
        let root = scratch("trusted-folders");
        let path = root.join("trusted-folders.json");
        let projects = root.join("projects");

        assert!(read_trusted_folders(&path).unwrap().is_empty());
        write_trusted_folder(&path, &projects.join("group")).unwrap();
        write_trusted_folder(&path, &projects).unwrap();
        write_trusted_folder(&path, &projects.join("other")).unwrap();
        assert_eq!(read_trusted_folders(&path).unwrap(), [projects]);

        std::fs::write(&path, b"invalid json").unwrap();
        assert!(read_trusted_folders(&path).unwrap().is_empty());
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn older_single_path_sessions_are_still_read() {
        assert_eq!(parse_session(br#""C:\\repo""#), [r"C:\repo"]);
        assert_eq!(parse_session(br#"["a","","b"]"#), ["a", "b"]);
        assert!(parse_session(b"not json").is_empty());
    }
}
