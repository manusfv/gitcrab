//! Filesystem notifications for worktree and Git metadata changes.
use gitcrab_core::platform::RepositoryLocation;
use notify::{Event, EventKind, RecommendedWatcher, RecursiveMode, Watcher};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Instant;

/// Pending watcher event or group of coalesced events.
pub struct Change {
    pub error: Option<String>,
    /// Time of the most recent event in this group.
    pub event_time: Instant,
    /// Whether any event requires rechecking repository metadata and history.
    pub metadata_dirty: bool,
}

/// Coalesces incoming events behind a single wake signal.
pub struct Changes {
    events: async_channel::Receiver<()>,
    pending: Arc<Mutex<Option<Change>>>,
}

impl Changes {
    /// Waits for a wake signal. Use [`Self::take_pending`] to retrieve the change.
    pub async fn wake(&self) -> Result<(), async_channel::RecvError> {
        self.events.recv().await
    }

    /// Takes the accumulated change and clears any pending wake signal.
    pub fn take_pending(&self) -> Option<Change> {
        let _ = self.events.try_recv();
        self.pending.lock().unwrap().take()
    }
}

fn classify(
    result: notify::Result<Event>,
    root: &Path,
    git: &Path,
    common: &Path,
) -> Option<Change> {
    let (error, metadata_dirty) = match result {
        Err(error) => (Some(error.to_string()), true),
        Ok(event) if event.need_rescan() => (None, true),
        Ok(event) if !matches!(event.kind, EventKind::Access(_)) => {
            let mut paths = event
                .paths
                .iter()
                .filter(|path| relevant(path, root, git, common));
            let first = paths.next()?;
            let is_metadata = |path: &PathBuf| path.starts_with(git) || path.starts_with(common);
            (None, is_metadata(first) || paths.any(is_metadata))
        }
        Ok(_) => return None,
    };
    Some(Change {
        error,
        event_time: Instant::now(),
        metadata_dirty,
    })
}

fn queue_change(pending: &Mutex<Option<Change>>, wake: &async_channel::Sender<()>, change: Change) {
    {
        let mut pending = pending.lock().unwrap();
        if let Some(previous) = pending.as_mut() {
            previous.metadata_dirty |= change.metadata_dirty;
            previous.event_time = change.event_time;
            if change.error.is_some() {
                previous.error = change.error;
            }
        } else {
            *pending = Some(change);
        }
    }
    let _ = wake.try_send(());
}

fn relevant(path: &Path, root: &Path, git: &Path, common: &Path) -> bool {
    let (relative, local) = if let Ok(relative) = path.strip_prefix(git) {
        (relative, true)
    } else if let Ok(relative) = path.strip_prefix(common) {
        (relative, false)
    } else {
        return path.starts_with(root);
    };
    if relative
        .extension()
        .is_some_and(|extension| extension == "lock")
    {
        return false;
    }
    if local
        && gitcrab_core::git::OPERATION_MARKERS
            .iter()
            .any(|(marker, _)| relative.starts_with(marker))
    {
        return true;
    }
    relative.as_os_str().is_empty()
        || matches!(relative.to_str(), Some("HEAD" | "index" | "config.worktree") if local)
        || matches!(
            relative.to_str(),
            Some("packed-refs" | "config" | "shallow")
        )
        || relative.starts_with("refs")
        || relative == Path::new("info/grafts")
}

/// Watches the worktree and both Git directories, including linked-worktree metadata.
///
/// Keep the returned watcher alive while consuming changes. Start the watcher
/// off the UI thread because registration can block.
///
/// # Errors
///
/// Returns an error if the platform watcher cannot be created or a path cannot be watched.
pub fn start(location: RepositoryLocation) -> Result<(RecommendedWatcher, Changes), String> {
    start_with_dirs(location.worktree, location.git_dir, location.common_dir)
}

fn start_with_dirs(
    root: PathBuf,
    git: PathBuf,
    common: PathBuf,
) -> Result<(RecommendedWatcher, Changes), String> {
    let (watch_root, watch_git, watch_common) = (root.clone(), git.clone(), common.clone());
    let (tx, rx) = async_channel::bounded(1);
    let pending = Arc::new(Mutex::new(None));
    let changes = pending.clone();
    let mut watcher = notify::recommended_watcher(move |result| {
        if let Some(change) = classify(result, &watch_root, &watch_git, &watch_common) {
            queue_change(&changes, &tx, change);
        }
    })
    .map_err(|e| e.to_string())?;
    watcher
        .watch(&root, RecursiveMode::Recursive)
        .map_err(|e| e.to_string())?;
    if !common.starts_with(&root) {
        watcher
            .watch(&common, RecursiveMode::Recursive)
            .map_err(|e| e.to_string())?;
    }
    if !git.starts_with(&root) && !git.starts_with(&common) {
        watcher
            .watch(&git, RecursiveMode::Recursive)
            .map_err(|e| e.to_string())?;
    }
    Ok((
        watcher,
        Changes {
            events: rx,
            pending,
        },
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn linked_watch_scope_tracks_markers_and_only_ignores_metadata_locks() {
        let root = Path::new("linked");
        let common = Path::new("main/.git");
        let git = common.join("worktrees/linked");
        for path in [
            root.join("notes.lock"),
            git.join("MERGE_HEAD"),
            git.join("rebase-merge/head-name"),
            git.join("HEAD"),
            git.join("index"),
            common.join("refs/heads/main"),
            common.join("packed-refs"),
        ] {
            assert!(relevant(&path, root, &git, common), "{}", path.display());
        }
        for path in [
            git.join("index.lock"),
            common.join("refs/heads/main.lock"),
            common.join("objects/pack/object"),
            common.join("MERGE_HEAD"),
            common.join("worktrees/other/CHERRY_PICK_HEAD"),
        ] {
            assert!(!relevant(&path, root, &git, common), "{}", path.display());
        }
    }

    #[test]
    fn watcher_retains_interruption_and_pathless_rescan_behind_pending_wake() {
        use notify::event::Flag;

        let root = Path::new("worktree");
        let git = root.join(".git");
        let (wake, events) = async_channel::bounded(1);
        let changes = Changes {
            events,
            pending: Arc::new(Mutex::new(None)),
        };
        let normal = || {
            classify(
                Ok(Event::new(EventKind::Any).add_path(root.join("file"))),
                root,
                &git,
                &git,
            )
            .unwrap()
        };
        queue_change(&changes.pending, &wake, normal());
        assert!(wake.is_full());
        let rescan = Event::new(EventKind::Other).set_flag(Flag::Rescan);
        assert!(rescan.paths.is_empty());
        let rescan = classify(Ok(rescan), root, &git, &git).unwrap();
        queue_change(&changes.pending, &wake, rescan);
        let change = changes.take_pending().unwrap();
        assert!(change.metadata_dirty);
        assert!(change.error.is_none());

        queue_change(&changes.pending, &wake, normal());
        assert!(wake.is_full());
        let error = classify(
            Err(notify::Error::generic("controlled interruption")),
            root,
            &git,
            &git,
        )
        .unwrap();
        queue_change(&changes.pending, &wake, error);
        queue_change(&changes.pending, &wake, normal());
        let change = changes.take_pending().unwrap();
        assert!(change.metadata_dirty);
        assert_eq!(change.error.as_deref(), Some("controlled interruption"));
        assert!(changes.take_pending().is_none());
    }

    #[test]
    fn live_watcher_recovers_from_setup_failure_and_observes_worktree_lock_files() {
        let root = std::env::temp_dir().join(format!(
            "gitcrab-watch-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let git = root.join(".git");
        assert!(start_with_dirs(root.clone(), git.clone(), git.clone()).is_err());
        std::fs::create_dir_all(&git).unwrap();
        let location = RepositoryLocation::test_path(root.canonicalize().unwrap());
        let (watcher, events) = start(location).unwrap();
        std::fs::write(root.join("normal.lock"), b"worktree event").unwrap();
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
        let observed = loop {
            if let Some(change) = events.take_pending() {
                assert!(change.error.is_none(), "{:?}", change.error);
                break true;
            }
            if std::time::Instant::now() >= deadline {
                break false;
            }
            std::thread::sleep(std::time::Duration::from_millis(20));
        };
        drop(watcher);
        std::fs::remove_dir_all(&root).unwrap();
        assert!(
            observed,
            "No worktree notification received on {}",
            std::env::consts::OS
        );
    }
}
