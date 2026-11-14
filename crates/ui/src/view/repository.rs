use super::*;
use gitcrab_core::platform::RepositoryLocation;

impl GitCrab {
    /// Discovers a worktree off the UI thread before entering the trust flow.
    pub fn open_path(&mut self, path: String, window: &mut Window, cx: &mut Context<Self>) {
        if self.awaiting_trust || self.state.busy {
            return;
        }
        self.awaiting_trust = true;
        let selected = path.clone();
        let task = cx
            .background_executor()
            .spawn(async move { RepositoryLocation::discover(std::path::Path::new(&selected)) });
        cx.spawn_in(window, async move |view, cx| {
            let result = task.await;
            let _ = view.update_in(cx, |v, window, cx| {
                v.awaiting_trust = false;
                match result {
                    Ok(location) => v.open_repository(location, window, cx),
                    Err(details) => {
                        if v.last_repository.as_deref() == Some(path.as_str()) {
                            match Preferences::forget_last_repository(&path, cx) {
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
                path: location.worktree.display().to_string(),
                details: "Repository folder names must be valid Unicode on this platform.".into(),
            });
            cx.notify();
            return;
        };
        if Preferences::trusts_repository(&location) {
            self.open_trusted(location, window, cx);
            return;
        }
        self.awaiting_trust = true;
        let shown = gitcrab_core::presentation::display_path(worktree);
        let name = gitcrab_core::presentation::path_labels(std::slice::from_ref(&shown))
            .pop()
            .map(|label| label.name)
            .unwrap_or_else(|| shown.clone());
        let mut metadata = vec![(
            "Git metadata",
            display_path(&location.git_dir.display().to_string()),
        )];
        if location.common_dir != location.git_dir {
            metadata.push((
                "Shared Git metadata",
                display_path(&location.common_dir.display().to_string()),
            ));
        }
        let view = cx.weak_entity();
        window.open_dialog(cx, move |dialog, _, _| {
            let accepted = view.clone();
            let closed = view.clone();
            let location = location.clone();
            let small = scaled(12.);
            let content = div()
                .flex()
                .flex_col()
                .gap_2()
                .child(
                    div()
                        .text_size(scaled(16.))
                        .font_weight(FontWeight::SEMIBOLD)
                        .child(name.clone()),
                )
                .child(
                    div()
                        .text_role(Role::TextSecondary)
                        .child(shown.clone()),
                )
                .child(
                    "Git can run programs configured by this repository, including hooks and filters. Only continue if you trust its local Git configuration and metadata.",
                )
                .child(
                    div()
                        .mt_1()
                        .text_size(small)
                        .text_role(Role::TextMuted)
                        .font_weight(FontWeight::MEDIUM)
                        .child("Details"),
                )
                .children(metadata.iter().map(|(label, path)| {
                    div()
                        .text_size(small)
                        .flex()
                        .flex_col()
                        .child(
                            div()
                                .text_role(Role::TextMuted)
                                .child(*label),
                        )
                        .child(
                            div()
                                .font_family(CODE_FONT)
                                .text_role(Role::TextSecondary)
                                .child(path.clone()),
                        )
                }))
                .child(
                    div()
                        .mt_2()
                        .text_size(small)
                        .text_role(Role::TextSecondary)
                        .child("Trust and open remembers this approval for these paths, including after restart. Cancel opens nothing."),
                );
            dialog
                .title("Trust this local repository?")
                .confirm()
                .button_props(
                    gpui_component::dialog::DialogButtonProps::default().ok_text("Trust and open"),
                )
                .child(content)
                .on_ok(move |_, w, cx| {
                    let _ = accepted.update(cx, |v, cx| {
                        v.open_trusted(location.clone(), w, cx);
                        match Preferences::remember_trust(&location, cx) {
                            Ok(result) => v.observe_persistence(result, "repository trust", cx),
                            Err(error) => v.persistence_error("repository trust", error),
                        }
                    });
                    true
                })
                .on_close(move |_, w, cx| {
                    let _ = closed.update(cx, |v, cx| {
                        v.awaiting_trust = false;
                        v.focus.focus(w);
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
            self.state.limit = limit.clamp(1, 100_000);
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
        let task = cx
            .background_executor()
            .spawn(async move { crate::watch::start(location) });
        cx.spawn_in(window, async move |view, cx| {
            let result = task.await;
            let receiver = view.update_in(cx, |v, _, cx| {
                if v.state.epoch != epoch { return None; }
                v.watch_pending = false;
                match result {
                    Ok((watcher, receiver)) => { v.watcher = Some(watcher); Some(receiver) }
                    Err(e) => { v.watch_warning = Some(format!("Live refresh unavailable: {e}. Use F5; focus refresh remains available.")); cx.notify(); None }
                }
            }).ok().flatten();
            let Some(receiver) = receiver else { return; };
            while receiver.wake().await.is_ok() {
                cx.background_executor().timer(std::time::Duration::from_millis(150)).await;
                let Some(crate::watch::Change { error, event_time, metadata_dirty }) = receiver.take_pending() else {
                    continue;
                };
                let keep = view.update_in(cx, |v, w, cx| {
                    if v.state.epoch != epoch { return false; }
                    let interrupted = error.is_some();
                    if let Some(error) = error {
                        v.watcher = None;
                        v.watch_warning = Some(format!("Live refresh interrupted: {error}. Retrying after refresh."));
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
