use super::*;
use gitcrab_core::platform::RepositoryLocation;
use gitcrab_core::presentation::display_location;

/// How long a burst of file changes may settle before it is read.
const WATCH_SETTLE: std::time::Duration = std::time::Duration::from_millis(150);

impl GitCrab {
    /// Discovers a worktree off the UI thread before entering the trust flow.
    pub fn open_path(&mut self, path: String, window: &mut Window, cx: &mut Context<Self>) {
        if self.awaiting_trust || self.state.busy {
            return;
        }
        self.awaiting_trust = true;
        let selected = path.clone();
        let backend = self.backend.clone();
        let task = cx
            .background_executor()
            .spawn(async move { backend.discover(std::path::Path::new(&selected)) });
        cx.spawn_in(window, async move |view, cx| {
            let result = task.await;
            let _ = view.update_in(cx, |v, window, cx| {
                v.awaiting_trust = false;
                match result {
                    Ok(location) => v.open_repository(location, window, cx),
                    Err(details) => {
                        if v.last_repository.as_deref() == Some(path.as_str()) {
                            match crate::preferences::forget_last_repository(&path, cx) {
                                Ok(result) => {
                                    v.last_repository = None;
                                    v.observe_persistence(result, "last repository", cx);
                                }
                                Err(error) => v.persistence_error("last repository", error),
                            }
                        }
                        v.open_failure = Some(OpenFailure { path, details });
                    }
                }
                cx.notify();
            });
        })
        .detach();
        cx.notify();
    }

    /// Opens a discovered worktree using saved trust or a new user confirmation.
    pub(crate) fn open_repository(
        &mut self,
        location: RepositoryLocation,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.awaiting_trust || self.state.busy {
            return;
        }
        let Some(worktree) = location.worktree.to_str() else {
            self.open_failure = Some(OpenFailure {
                path: display_location(&location.worktree),
                details: "Repository folder names must be valid Unicode on this platform.".into(),
            });
            cx.notify();
            return;
        };
        if crate::preferences::trusts_repository(&location, cx) {
            self.open_trusted(location, window, cx);
            return;
        }
        self.awaiting_trust = true;
        self.trust_folder = false;
        let shown = gitcrab_core::presentation::display_path(worktree);
        let name = gitcrab_core::presentation::path_labels(std::slice::from_ref(&shown))
            .pop()
            .map(|label| label.name)
            .unwrap_or_else(|| shown.clone());
        let folder = gitcrab_core::preferences::trust_folder_for(&location).map(|folder| {
            let name = folder.file_name().map_or_else(
                || display_location(folder),
                |name| name.to_string_lossy().into_owned(),
            );
            (folder.to_owned(), name)
        });
        let view = cx.weak_entity();
        let scroll = ScrollHandle::new();
        window.open_alert(cx, move |dialog, _, cx| {
            let accepted = view.clone();
            let closed = view.clone();
            let toggled = view.clone();
            let location = location.clone();
            let folder = folder.clone();
            let whole_folder = folder.is_some()
                && view.upgrade().is_some_and(|v| v.read(cx).trust_folder);
            let remembered = match &folder {
                Some((_, folder_name)) if whole_folder => {
                    format!("every repository in “{folder_name}”")
                }
                _ => "this repository".to_owned(),
            };
            let content = modal_body()
                .child(modal::subject(name.clone(), Some(shown.clone().into())))
                .child(modal::paragraph(
                    "Only continue if you trust its local Git configuration and metadata.",
                ))
                .when_some(folder.as_ref(), |body, (_, folder_name)| {
                    body.child(
                        Checkbox::new("trust-folder")
                            .label(format!("Also trust every repository in “{folder_name}”"))
                            .checked(whole_folder)
                            .tab_stop(true)
                            .on_click(move |checked, _, cx| {
                                let _ = toggled.update(cx, |v, cx| {
                                    v.trust_folder = *checked;
                                    cx.notify();
                                });
                            }),
                    )
                })
                .child(modal::fine_print(format!(
                    "Trust and open remembers this approval for {remembered}, including after restart. Cancel opens nothing."
                )));
            dialog
                .frame(
                    ModalHeader::new("Trust this local repository?")
                        .description(
                            "Git can run programs this repository configures, including hooks and filters.",
                        )
                        .tone_glyph(Tone::Warning, "shield-alert"),
                    ModalWidth::Medium,
                    cx,
                )
                .footer(
                    ModalFooter::new()
                        .action(components::dialog_cancel(cx))
                        .action(components::dialog_ok(
                            "Trust and open",
                            components::ButtonKind::Primary,
                            cx,
                        )),
                )
                .body(&scroll, content)
                .on_ok(move |_, w, cx| {
                    let _ = accepted.update(cx, |v, cx| {
                        v.open_trusted(location.clone(), w, cx);
                        let remembered = match &folder {
                            Some((folder, _)) if v.trust_folder => {
                                crate::preferences::remember_trusted_folder(folder, cx)
                            }
                            _ => crate::preferences::remember_trust(&location, cx),
                        };
                        match remembered {
                            Ok(result) => v.observe_persistence(result, "repository trust", cx),
                            Err(error) => v.persistence_error("repository trust", error),
                        }
                    });
                    true
                })
                .on_close(move |_, w, cx| {
                    let _ = closed.update(cx, |v, cx| {
                        v.awaiting_trust = false;
                        v.focus.focus(w, cx);
                        cx.notify();
                    });
                })
        });
        #[cfg(feature = "native-evidence")]
        if let Some(marker) = std::env::var_os("GITCRAB_TRUST_READY") {
            window.on_next_frame(move |_, _| {
                let _ = std::fs::write(marker, b"trust dialog mounted");
            });
            cx.notify();
        }
    }

    /// Opens a repository GitCrab just created or cloned. Its configuration
    /// and metadata come from the user's own Git, not from anyone else, so it
    /// is trusted without asking.
    pub(crate) fn open_created(
        &mut self,
        location: RepositoryLocation,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.awaiting_trust || self.state.busy {
            return;
        }
        match crate::preferences::remember_trust(&location, cx) {
            Ok(result) => self.observe_persistence(result, "repository trust", cx),
            Err(error) => self.persistence_error("repository trust", error),
        }
        self.open_trusted(location, window, cx);
    }

    fn open_trusted(
        &mut self,
        location: RepositoryLocation,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.state.busy {
            return;
        }
        self.open_failure = None;
        // A commit, fetch or operation chosen during a write in the previous
        // repository must not run in the one being opened.
        self.after_writes = None;
        self.pending_change = None;
        self.cancel_pending_run();
        self.branch_prompt = None;
        self.switch_after_branch = None;
        self.switching_to_new_branch = None;
        for kind in [
            ReadKind::Snapshot,
            ReadKind::Details,
            ReadKind::Diff,
            ReadKind::OperationReview,
        ] {
            self.service.cancel_read(kind);
        }
        let job = self.state.open_trusted(location.clone());
        self.repository_location = Some(location);
        #[cfg(feature = "native-evidence")]
        let mut job = job;
        #[cfg(feature = "native-evidence")]
        if let Ok(limit) = std::env::var("GITCRAB_CAPTURE_LIMIT")
            .unwrap_or_default()
            .parse::<usize>()
        {
            self.state.limit = limit.clamp(1, HISTORY_MAX);
            if let Some((
                _,
                Request {
                    action: Action::Open { limit, .. },
                    ..
                },
            )) = &mut job
            {
                *limit = self.state.limit;
            }
        }
        self.dispatch(job, window, cx);
        cx.notify();
    }
    /// Starts a watcher for the current epoch and drops notifications from older epochs.
    pub(super) fn install_watch(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.watch_epoch == self.state.epoch && (self.watch_pending || self.watcher.is_some()) {
            return;
        }
        self.watch_epoch = self.state.epoch;
        self.watch_pending = true;
        self.watcher = None;
        self.watch_warning = None;
        let epoch = self.state.epoch;
        let Some(location) = self.repository_location.clone() else {
            self.watch_pending = false;
            return;
        };
        let backend = self.backend.clone();
        let task = cx.background_executor().spawn(async move {
            let ignore = backend.ignore_check(&location);
            (crate::watch::start(location), ignore)
        });
        cx.spawn_in(window, async move |view, cx| {
            let (result, mut ignore) = task.await;
            let receiver = view.update_in(cx, |v, _, cx| {
                if v.state.epoch != epoch { return None; }
                v.watch_pending = false;
                match result {
                    Ok((watcher, receiver)) => { v.watcher = Some(watcher); Some(receiver) }
                    Err(e) => { v.watch_warning = Some(format!("Live refresh unavailable: {e}. Press F5 to refresh. GitCrab still refreshes when its window gets focus.")); cx.notify(); None }
                }
            }).ok().flatten();
            let Some(receiver) = receiver else { return; };
            while receiver.wake().await.is_ok() {
                cx.background_executor().timer(WATCH_SETTLE).await;
                let Some(change) = receiver.take_pending() else {
                    continue;
                };
                // Build output and other ignored files cannot change what
                // GitCrab shows, so a burst touching only them refreshes nothing.
                if let (Some(paths), Some(check)) = (change.ignorable_paths().cloned(), ignore.take()) {
                    let (check, ignored) = cx
                        .background_executor()
                        .spawn(async move {
                            let ignored = check.ignored(&paths).is_ok_and(|ignored| ignored.len() == paths.len());
                            (check, ignored)
                        })
                        .await;
                    ignore = Some(check);
                    if ignored {
                        continue;
                    }
                }
                let crate::watch::Change { error, event_time, metadata_dirty, .. } = change;
                let keep = view.update_in(cx, |v, w, cx| {
                    if v.state.epoch != epoch { return false; }
                    let interrupted = error.is_some();
                    if let Some(error) = error {
                        v.watcher = None;
                        v.watch_warning = Some(format!("Live refresh stopped: {error}. It restarts with the next refresh."));
                    }
                    // A full refresh sent after the change, or the one every
                    // write ends with, reads it already. Skipping it avoids a
                    // second full refresh after each of GitCrab's own writes.
                    let covered = !interrupted
                        && v.active
                        && w.is_window_active()
                        && (v.state.busy
                            || v.full_refresh_sent.is_some_and(|sent| sent > event_time));
                    if covered {
                        return true;
                    }
                    if v.active && !v.state.busy && w.is_window_active() {
                        #[cfg(feature="native-evidence")]
                        {v.watch_start=Some(event_time);}
                        #[cfg(not(feature="native-evidence"))]
                        let _=event_time;
                        let plan = v.state.watcher_refresh(metadata_dirty);
                        v.apply_refresh(plan, w, cx);
                    }
                    else { v.external_dirty = true; }
                    !interrupted
                }).unwrap_or(false);
                if !keep { break; }
            }
        }).detach();
    }
}
