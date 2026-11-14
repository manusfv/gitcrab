//! Filesystem notifications for worktree and Git metadata changes.
//!
//! Which changes matter, and how they coalesce, is decided in
//! [`gitcrab_core::watch`]; this module only connects the platform watcher.
pub use gitcrab_core::watch::Change;
use gitcrab_core::{platform::RepositoryLocation, watch::WatchScope};
use notify::{Event, EventKind, RecommendedWatcher, RecursiveMode, Watcher};
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

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

/// Translates a platform event into the change it causes, if any.
fn classify(result: notify::Result<Event>, scope: &WatchScope) -> Option<Change> {
    match result {
        Err(error) => Some(Change::interrupted(Some(error.to_string()))),
        Ok(event) if event.need_rescan() => Some(Change::interrupted(None)),
        Ok(event) if !matches!(event.kind, EventKind::Access(_)) => scope.change_for(&event.paths),
        Ok(_) => None,
    }
}

fn queue_change(pending: &Mutex<Option<Change>>, wake: &async_channel::Sender<()>, change: Change) {
    {
        let mut pending = pending.lock().unwrap();
        match pending.as_mut() {
            Some(previous) => previous.absorb(change),
            None => *pending = Some(change),
        }
    }
    let _ = wake.try_send(());
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
    let scope = WatchScope {
        root: root.clone(),
        git: git.clone(),
        common: common.clone(),
    };
    let (tx, rx) = async_channel::bounded(1);
    let pending = Arc::new(Mutex::new(None));
    let changes = pending.clone();
    let mut watcher = notify::recommended_watcher(move |result| {
        if let Some(change) = classify(result, &scope) {
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
    fn watcher_retains_interruption_and_pathless_rescan_behind_pending_wake() {
        use notify::event::Flag;

        let root = std::path::Path::new("worktree");
        let scope = WatchScope {
            root: root.into(),
            git: root.join(".git"),
            common: root.join(".git"),
        };
        let (wake, events) = async_channel::bounded(1);
        let changes = Changes {
            events,
            pending: Arc::new(Mutex::new(None)),
        };
        let normal = || {
            classify(
                Ok(Event::new(EventKind::Any).add_path(root.join("file"))),
                &scope,
            )
            .unwrap()
        };
        queue_change(&changes.pending, &wake, normal());
        assert!(wake.is_full());
        let rescan = Event::new(EventKind::Other).set_flag(Flag::Rescan);
        assert!(rescan.paths.is_empty());
        let rescan = classify(Ok(rescan), &scope).unwrap();
        queue_change(&changes.pending, &wake, rescan);
        let change = changes.take_pending().unwrap();
        assert!(change.metadata_dirty);
        assert!(change.error.is_none());

        queue_change(&changes.pending, &wake, normal());
        assert!(wake.is_full());
        let error = classify(
            Err(notify::Error::generic("controlled interruption")),
            &scope,
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
