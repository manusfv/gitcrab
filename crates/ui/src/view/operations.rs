//! One-click Git operations.
//!
//! Push, Pull, Checkout, Stash, Pop, Undo, Redo, Merge, Cherry-pick and Revert
//! run as soon as they are chosen. The worker still resolves and checks the
//! target first (the same [`ReviewedOperation`] preflight a confirmation used
//! to show), and the write rechecks HEAD, the index and the target before Git
//! runs. Progress and results appear in the status bar, failures in the error
//! banner, and Undo reverses every recorded step, so nothing waits on a dialog.
//!
//! Creating a branch needs a name, which the inline prompt bar below the
//! toolbar asks for without blocking the rest of the window.

use super::*;

type OperationRequest = (u32, u32);

/// An operation whose preflight review is in the worker. When it resolves,
/// the write starts at once.
pub(super) struct PendingRun {
    request: OperationRequest,
    label: &'static str,
    /// Switch to the new branch once a Create branch at HEAD has succeeded.
    then_switch: bool,
    /// When the review started, so the write that follows keeps one
    /// loading period instead of starting a new one.
    started: std::time::Instant,
    /// The toolbar button that spins during the review.
    command: Option<Command>,
    network: bool,
}

/// How long a local operation runs before its loader shows, so quick ones
/// do not flicker. Network operations show it at once.
pub(super) const LOADER_DELAY: std::time::Duration = std::time::Duration::from_millis(150);

/// The toolbar button that spins while `action` runs. Fetch has no button of
/// its own and lives in the Pull menu, so it spins Pull.
pub(super) fn loading_command(action: &Action) -> Option<Command> {
    match action {
        Action::Fetch => Some(Command::Pull),
        Action::Operation { reviewed } => operation_command(&reviewed.op),
        Action::RunOperation { op, .. } | Action::ReviewOperation { op, .. } => {
            operation_command(op)
        }
        _ => None,
    }
}

fn operation_command(op: &Operation) -> Option<Command> {
    Some(match op {
        Operation::CreateBranch | Operation::CreateBranchAt { .. } => Command::CreateBranch,
        Operation::Checkout => Command::Checkout,
        Operation::Merge => Command::Merge,
        Operation::CherryPick => Command::CherryPick,
        Operation::Revert => Command::Revert,
        Operation::Stash => Command::Stash,
        Operation::PopStash => Command::PopStash,
        Operation::Pull { .. } => Command::Pull,
        Operation::Push => Command::Push,
        Operation::Undo => Command::Undo,
        Operation::Redo => Command::Redo,
        Operation::Abort => Command::Abort,
        Operation::Continue { .. } => Command::Continue,
        Operation::Skip => Command::Skip,
    })
}

/// The inline bar asking for a new branch's name.
pub(super) struct BranchPrompt {
    /// Commit to start from, or `None` for HEAD.
    start: Option<String>,
    error: Option<String>,
}

impl GitCrab {
    /// Runs an operation immediately: the worker reviews the target against
    /// the loaded snapshot's HEAD and index, then the write starts.
    pub(super) fn run_operation(
        &mut self,
        op: Operation,
        target: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.start_operation(op, target, false, window, cx);
    }

    pub(super) fn start_operation(
        &mut self,
        op: Operation,
        target: String,
        then_switch: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        let Some(snapshot) = self.state.snapshot.as_ref() else {
            return false;
        };
        if self.state.busy && !self.state.blocking_write() && self.pending_run.is_none() {
            // Staging is still being written: run once it lands, reviewed
            // against the HEAD and index it leaves.
            self.after_writes = Some(AfterWrites::Operation {
                op,
                target,
                then_switch,
            });
            cx.notify();
            return true;
        }
        if self.state.busy || self.pending_run.is_some() {
            self.state.notice = "A Git operation is running. Wait for it to finish.".into();
            cx.notify();
            return false;
        }
        let Some(destination) = snapshot.reviewed.clone() else {
            self.state.notice = "Refresh the repository before running an operation.".into();
            cx.notify();
            return false;
        };
        if let Err(error) = op.validate_target(&target) {
            self.state.notice = format!("{} unavailable. {error}", op.label());
            cx.notify();
            return false;
        }
        // Every surface that starts a write ends here, including the ones
        // that skip `run_command`, such as double-clicking a branch.
        if snapshot.commit_blocked() && !op.finishes_sequence() {
            self.state.notice = format!(
                "{} unavailable. {}",
                op.label(),
                gitcrab_core::git::OPERATION_IN_PROGRESS
            );
            cx.notify();
            return false;
        }
        let label = op.label();
        if !op.is_network() && op.journal_direction().is_none() {
            // Local operations resolve and run in one worker request; the
            // worker checks HEAD and the index against `destination` first.
            self.switch_after_branch = then_switch
                .then(|| target.clone())
                .filter(|_| matches!(op, Operation::CreateBranch));
            self.state.dismiss_notice();
            let job = self.state.mutate(Action::RunOperation {
                op,
                target,
                destination,
            });
            self.dispatch_mutation(job, window, cx);
            cx.notify();
            return true;
        }
        let command = operation_command(&op);
        let network = op.is_network();
        let (sequence, job) = self.state.review_operation(op, target, destination);
        self.pending_run = Some(PendingRun {
            request: (self.state.epoch, sequence),
            label,
            then_switch,
            started: std::time::Instant::now(),
            command,
            network,
        });
        self.reveal_loader_later(cx);
        self.state.dismiss_notice();
        self.dispatch(job, window, cx);
        cx.notify();
        true
    }

    /// The work whose loader is on screen, with the toolbar button it spins
    /// if it has one. `None` while nothing runs, and while a local operation
    /// is still younger than [`LOADER_DELAY`].
    pub(super) fn loader(&self) -> Option<Option<Command>> {
        let (command, started, network) = match (&self.running, &self.pending_run) {
            (Some(running), _) => (running.command, running.started, running.network),
            (None, Some(pending)) => (pending.command, pending.started, pending.network),
            (None, None) => return None,
        };
        (network || started.elapsed() >= LOADER_DELAY).then_some(command)
    }

    /// Status bar text while a snapshot read runs. Opening a repository says
    /// so at once; a refresh of the repository on screen, such as one the
    /// file watcher starts, stays silent until it outlasts [`LOADER_DELAY`].
    pub(super) fn loading_label(&self) -> Option<&'static str> {
        if !self.state.loading() {
            return None;
        }
        if self.state.snapshot.is_none() {
            return Some("Loading repository…");
        }
        self.snapshot_sent
            .is_some_and(|sent| sent.elapsed() >= LOADER_DELAY)
            .then_some("Refreshing…")
    }

    /// Draws again once [`LOADER_DELAY`] has passed, so the loader of an
    /// operation still running by then appears without other input.
    pub(super) fn reveal_loader_later(&self, cx: &mut Context<Self>) {
        cx.spawn(async move |view, cx| {
            cx.background_executor().timer(LOADER_DELAY).await;
            view.update(cx, |_, cx| cx.notify()).ok();
        })
        .detach();
    }

    /// Status bar text while the preflight review runs.
    pub(super) fn pending_label(&self) -> Option<String> {
        self.pending_run
            .as_ref()
            .map(|pending| format!("{}…", pending.label))
    }

    /// Drops a review that can no longer start its write, such as after the
    /// tab opened another repository.
    pub(super) fn cancel_pending_run(&mut self) {
        if self.pending_run.take().is_some() {
            self.state.cancel_operation_review();
            self.service.cancel_read(ReadKind::OperationReview);
        }
    }

    /// Starts the write for a resolved review, or reports why it cannot run.
    pub(super) fn operation_resolved(
        &mut self,
        request: OperationRequest,
        result: Result<ReviewedOperation, String>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(pending) = self
            .pending_run
            .take_if(|pending| pending.request == request)
        else {
            return;
        };
        let reviewed = match result {
            Ok(reviewed) if self.state.epoch == request.0 => reviewed,
            Ok(_) => return,
            Err(error) => {
                self.error_heading = format!("{} did not run", pending.label);
                self.state.notice = error;
                self.state.error = true;
                cx.notify();
                return;
            }
        };
        self.switch_after_branch = pending
            .then_switch
            .then(|| reviewed.target.clone())
            .filter(|_| matches!(reviewed.op, Operation::CreateBranch));
        let job = self.state.mutate(Action::Operation { reviewed });
        self.loading_since = Some(pending.started);
        self.dispatch_mutation(job, window, cx);
    }

    /// Follows a successful Create branch at HEAD by switching to it, which
    /// changes no files because the branch points at HEAD.
    pub(super) fn after_mutation(&mut self, ok: bool, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(branch) = self.switch_after_branch.take().filter(|_| ok) {
            self.start_operation(Operation::Checkout, branch, false, window, cx);
        }
    }

    /// Opens the inline bar that names a new branch at `start`, or at HEAD.
    pub(super) fn prompt_branch(
        &mut self,
        start: Option<String>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.state.snapshot.is_none() {
            return;
        }
        self.branch_prompt = Some(BranchPrompt { start, error: None });
        self.target.update(cx, |s, cx| {
            s.set_placeholder("New branch name", window, cx);
            s.set_value("", window, cx);
            s.focus(window, cx);
        });
        cx.notify();
    }

    pub(super) fn close_branch_prompt(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.branch_prompt.take().is_some() {
            self.focus.focus(window, cx);
            cx.notify();
        }
    }

    pub(super) fn branch_prompt_open(&self) -> bool {
        self.branch_prompt.is_some()
    }

    /// Clears a shown name error once the user edits the name.
    pub(super) fn operation_target_changed(&mut self, cx: &mut Context<Self>) {
        if let Some(prompt) = self.branch_prompt.as_mut() {
            if prompt.error.take().is_some() {
                cx.notify();
            }
        }
    }

    /// Creates the branch named in the prompt. At HEAD it also switches to it.
    pub(super) fn submit_branch_prompt(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(prompt) = self.branch_prompt.as_mut() else {
            return;
        };
        let name = self.target.read(cx).value().trim().to_string();
        let (op, then_switch) = match &prompt.start {
            Some(start) => (
                Operation::CreateBranchAt {
                    start: start.clone(),
                },
                false,
            ),
            None => (Operation::CreateBranch, true),
        };
        if let Err(error) =
            gitcrab_core::service::validate_new_branch(&op, &name, self.state.snapshot.as_ref())
        {
            prompt.error = Some(error);
            cx.notify();
            return;
        }
        if self.start_operation(op, name, then_switch, window, cx) {
            self.close_branch_prompt(window, cx);
        }
    }

    /// The non-modal bar asking for a new branch's name, shown below the toolbar.
    pub(super) fn render_branch_prompt(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let prompt = self.branch_prompt.as_ref()?;
        let snapshot = self.state.snapshot.as_ref()?;
        let place = match &prompt.start {
            Some(start) => {
                let subject = snapshot
                    .commits
                    .iter()
                    .find(|c| &c.id == start)
                    .map(|c| format!(" ({})", c.subject))
                    .unwrap_or_default();
                format!("Creates a branch at {}{subject}", short_id(start))
            }
            None => format!(
                "Creates a branch at {} ({}) and switches to it",
                snapshot.head_label(),
                short_id(&snapshot.head_id)
            ),
        };
        let busy = self.state.busy || self.pending_run.is_some();
        Some(
            div()
                .id("branch-prompt")
                .flex_none()
                .mx_2()
                .mt_2()
                .px_3()
                .py_2()
                .rounded(px(6.))
                .bg_role(Role::SurfacePanel)
                .border_1()
                .border_role(Role::ActionPrimary)
                .flex()
                .flex_col()
                .gap_1()
                .on_key_down(cx.listener(|v, event: &KeyDownEvent, w, cx| {
                    if event.keystroke.key == "escape" {
                        v.close_branch_prompt(w, cx);
                        cx.stop_propagation();
                    }
                }))
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap_2()
                        .child(icon("branch-plus").size(scaled(GLYPH_SIZE)))
                        .child(
                            div()
                                .flex_none()
                                .max_w(px(360.))
                                .truncate()
                                .font_weight(FontWeight::MEDIUM)
                                .child(place),
                        )
                        .child(
                            div()
                                .flex_1()
                                .min_w(px(160.))
                                .max_w(px(420.))
                                .child(Input::new(&self.target).small()),
                        )
                        .child(
                            components::basic(
                                Button::new("branch-prompt-create").label("Create"),
                                components::ButtonKind::Primary,
                                ControlSize::Compact,
                            )
                            .set_disabled(busy)
                            .on_click(cx.listener(|v, _, w, cx| v.submit_branch_prompt(w, cx))),
                        )
                        .child(
                            self.button("branch-prompt-cancel", "Cancel")
                                .on_click(cx.listener(|v, _, w, cx| v.close_branch_prompt(w, cx))),
                        ),
                )
                .when_some(prompt.error.clone(), |el, error| {
                    el.child(
                        div()
                            .text_size(scaled(TEXT_SIZE_SMALL))
                            .text_role(Role::StatusErrorFg)
                            .child(error),
                    )
                })
                .into_any_element(),
        )
    }
}
