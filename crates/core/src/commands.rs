//! One availability model for repository commands.
//!
//! The toolbar, sidebar, inspector, context menus and keyboard shortcuts ask the
//! same [`Command::availability`] so a command is enabled, or explained, the
//! same way wherever it appears.

/// A user-facing repository command.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Command {
    Refresh,
    Undo,
    Redo,
    CreateBranch,
    Checkout,
    Merge,
    CherryPick,
    Revert,
    Stash,
    PopStash,
    Fetch,
    Pull,
    Push,
}

/// The repository state that decides whether commands can run.
#[derive(Clone, Copy, Debug, Default)]
pub struct CommandContext {
    /// A repository snapshot is loaded.
    pub repository: bool,
    /// A Git write is running in this tab.
    pub busy: bool,
    /// A dialog or the Appearance panel owns input.
    pub modal: bool,
    /// The snapshot captured HEAD and index identity for write review.
    pub reviewable: bool,
    /// HEAD names a branch with no commits yet.
    pub unborn: bool,
    pub stash_count: usize,
    /// A tracked file differs from HEAD in the index or working tree.
    pub tracked_changes: bool,
    /// The top Undo step, as checked with the latest snapshot.
    pub undo: crate::journal::StepState,
    /// The top Redo step, as checked with the latest snapshot.
    pub redo: crate::journal::StepState,
    /// HEAD is detached rather than on a local branch.
    pub detached: bool,
    /// Number of configured remotes.
    pub remotes: usize,
    /// The checked-out branch has an upstream on a remote.
    pub upstream: bool,
    /// Push knows where to go: the upstream's remote, the only remote, or `origin`.
    pub push_remote: bool,
    /// Commits the upstream lacks, as last fetched.
    pub ahead: usize,
    /// Upstream commits the branch lacks, as last fetched.
    pub behind: usize,
}

/// Whether a command can run, with the reason when it cannot.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Availability {
    Enabled,
    Disabled(&'static str),
}

impl Availability {
    pub fn enabled(self) -> bool {
        self == Self::Enabled
    }
    pub fn reason(self) -> Option<&'static str> {
        match self {
            Self::Enabled => None,
            Self::Disabled(reason) => Some(reason),
        }
    }
}

/// Explains why Fetch, Pull and Push are unavailable without a remote.
pub const NO_REMOTES: &str =
    "This repository has no remotes. Add one from a terminal with `git remote add`.";
/// Explains why Pull and Push are unavailable with a detached HEAD.
pub const NETWORK_DETACHED: &str = "HEAD is detached. Check out a branch first.";
/// Explains why Pull is unavailable without an upstream.
pub const NO_UPSTREAM: &str = "The current branch has no upstream on a remote. Push it to set one.";
/// Explains why Push cannot choose a remote by itself.
pub const AMBIGUOUS_PUSH_REMOTE: &str =
    "The current branch has no upstream and there are several remotes, none named origin. Push it once from a terminal with `git push -u <remote> <branch>`.";

impl Command {
    /// Commands in the toolbar action group, in order. Every one is always
    /// shown; one that cannot run is disabled with its reason.
    pub const PRIMARY: [Self; 7] = [
        Self::Undo,
        Self::Redo,
        Self::Pull,
        Self::Push,
        Self::CreateBranch,
        Self::Stash,
        Self::PopStash,
    ];

    /// Short toolbar label.
    pub fn label(self) -> &'static str {
        match self {
            Self::Refresh => "Refresh",
            Self::Undo => "Undo",
            Self::Redo => "Redo",
            Self::CreateBranch => "Branch",
            Self::Checkout => "Checkout",
            Self::Merge => "Merge",
            Self::CherryPick => "Cherry-pick",
            Self::Revert => "Revert",
            Self::Stash => "Stash",
            Self::PopStash => "Pop",
            Self::Fetch => "Fetch",
            Self::Pull => "Pull",
            Self::Push => "Push",
        }
    }

    /// Menu label, with an ellipsis for commands that ask for input first.
    pub fn menu_label(self) -> &'static str {
        match self {
            Self::Refresh => "Refresh repository",
            Self::Undo => "Undo",
            Self::Redo => "Redo",
            Self::CreateBranch => "Create branch…",
            Self::Checkout => "Checkout",
            Self::Merge => "Merge",
            Self::CherryPick => "Cherry-pick",
            Self::Revert => "Revert",
            Self::Stash => "Stash",
            Self::PopStash => "Pop latest stash",
            Self::Fetch => "Fetch",
            Self::Pull => "Pull",
            Self::Push => "Push",
        }
    }

    /// Whether the command opens a write review.
    pub fn writes(self) -> bool {
        !matches!(self, Self::Refresh | Self::Fetch)
    }

    /// Decides availability from the shared repository state.
    pub fn availability(self, cx: &CommandContext) -> Availability {
        use Availability::Disabled;
        if !cx.repository {
            return Disabled("Open a repository first.");
        }
        if cx.modal {
            return Disabled("Close the open dialog first.");
        }
        if self == Self::Fetch {
            // Fetch changes only remote-tracking refs, so it needs no review.
            if cx.busy {
                return Disabled("A Git operation is running. Wait for it to finish.");
            }
            if cx.remotes == 0 {
                return Disabled(NO_REMOTES);
            }
            return Availability::Enabled;
        }
        if !self.writes() {
            return Availability::Enabled;
        }
        if cx.busy {
            return Disabled("A Git operation is running. Wait for it to finish.");
        }
        if !cx.reviewable {
            return Disabled("Refresh the repository before reviewing an operation.");
        }
        if self == Self::PopStash && cx.stash_count == 0 {
            return Disabled("There are no stashes to apply.");
        }
        if self == Self::Stash && !cx.tracked_changes && !cx.unborn {
            return Disabled(crate::git::NOTHING_TO_STASH);
        }
        // Undo and Redo depend only on their journal step's own preconditions.
        if let Some((state, empty)) = match self {
            Self::Undo => Some((cx.undo, crate::journal::NOTHING_TO_UNDO)),
            Self::Redo => Some((cx.redo, crate::journal::NOTHING_TO_REDO)),
            _ => None,
        } {
            return match state {
                crate::journal::StepState::Empty => Disabled(empty),
                crate::journal::StepState::Blocked(blocker) => Disabled(blocker.reason()),
                crate::journal::StepState::Ready => Availability::Enabled,
            };
        }
        if cx.unborn && self != Self::PopStash {
            return Disabled("The current branch has no commits yet. Make the first commit.");
        }
        if matches!(self, Self::Pull | Self::Push) {
            if cx.detached {
                return Disabled(NETWORK_DETACHED);
            }
            if cx.remotes == 0 && !cx.upstream {
                return Disabled(NO_REMOTES);
            }
            if self == Self::Pull && !cx.upstream {
                return Disabled(NO_UPSTREAM);
            }
            if self == Self::Push && !cx.push_remote {
                return Disabled(AMBIGUOUS_PUSH_REMOTE);
            }
        }
        Availability::Enabled
    }

    /// The toolbar button's tooltip: the action's name, such as "Push" or
    /// "Pull (fast-forward if possible)" for Pull's default mode, or for Undo
    /// and Redo the step they act on, such as `Undo commit “Fix parser”`. A
    /// disabled command gives its reason instead; Undo and Redo give theirs
    /// in one line, leaving the full explanation to the status bar.
    pub fn toolbar_tooltip(self, cx: &CommandContext, summary: Option<&str>) -> String {
        use crate::journal::{NOTHING_TO_REDO, NOTHING_TO_UNDO};
        if let Availability::Disabled(reason) = self.availability(cx) {
            return match self {
                Self::Undo if reason == NOTHING_TO_UNDO => "No recent action to undo".to_owned(),
                Self::Redo if reason == NOTHING_TO_REDO => "No recent undo to redo".to_owned(),
                Self::Undo | Self::Redo => reason.to_owned(),
                _ => self.describe(cx),
            };
        }
        match self {
            Self::Undo | Self::Redo => self.describe_with_summary(cx, summary),
            Self::Pull => crate::git::PullMode::ALL[0].label().to_owned(),
            Self::PopStash => "Pop Stash".to_owned(),
            _ => self.label().to_owned(),
        }
    }

    /// Decides availability for a command aimed at one commit.
    ///
    /// Cherry-pick and Revert need a mainline parent for a merge commit, which
    /// GitCrab cannot choose yet, so they are disabled with that reason.
    pub fn availability_for_commit(self, cx: &CommandContext, parents: usize) -> Availability {
        match self.availability(cx) {
            Availability::Enabled
                if parents > 1 && matches!(self, Self::CherryPick | Self::Revert) =>
            {
                Availability::Disabled(crate::service::MERGE_COMMIT_UNSUPPORTED)
            }
            availability => availability,
        }
    }

    /// Tooltip text: the label, followed by the reason when it is disabled.
    pub fn describe(self, cx: &CommandContext) -> String {
        self.describe_availability(self.availability(cx))
    }

    /// Tooltip text naming what the command acts on, such as
    /// `Undo commit “Fix parser”`.
    pub fn describe_with_summary(self, cx: &CommandContext, summary: Option<&str>) -> String {
        let text = self.describe(cx);
        match summary {
            Some(summary) => {
                let label = self.menu_label().trim_end_matches('…');
                text.replacen(label, &format!("{label} {summary}"), 1)
            }
            None => text,
        }
    }

    /// Tooltip text for a command aimed at one commit.
    pub fn describe_for_commit(self, cx: &CommandContext, parents: usize) -> String {
        self.describe_availability(self.availability_for_commit(cx, parents))
    }

    fn describe_availability(self, availability: Availability) -> String {
        let label = self.menu_label().trim_end_matches('…');
        match availability {
            Availability::Enabled => label.to_owned(),
            Availability::Disabled(reason) => format!("{label} unavailable. {reason}"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ready() -> CommandContext {
        CommandContext {
            repository: true,
            reviewable: true,
            stash_count: 1,
            tracked_changes: true,
            ..Default::default()
        }
    }

    #[test]
    fn pop_stash_is_disabled_everywhere_without_a_stash() {
        let mut cx = ready();
        assert!(Command::PopStash.availability(&cx).enabled());
        cx.stash_count = 0;
        assert_eq!(
            Command::PopStash.availability(&cx).reason(),
            Some("There are no stashes to apply.")
        );
        assert!(Command::PopStash
            .describe(&cx)
            .starts_with("Pop latest stash unavailable."));
    }

    #[test]
    fn stash_is_disabled_without_tracked_changes() {
        let mut cx = ready();
        assert!(Command::Stash.availability(&cx).enabled());
        cx.tracked_changes = false;
        assert_eq!(
            Command::Stash.availability(&cx).reason(),
            Some(crate::git::NOTHING_TO_STASH)
        );
    }

    #[test]
    fn busy_and_modal_guards_cover_every_write_but_refresh_reads() {
        let mut cx = ready();
        cx.busy = true;
        for command in [
            Command::CreateBranch,
            Command::Checkout,
            Command::Merge,
            Command::CherryPick,
            Command::Revert,
            Command::Stash,
            Command::PopStash,
        ] {
            assert!(!command.availability(&cx).enabled(), "{command:?}");
        }
        assert!(Command::Refresh.availability(&cx).enabled());
        cx.busy = false;
        cx.modal = true;
        assert_eq!(
            Command::Refresh.availability(&cx).reason(),
            Some("Close the open dialog first.")
        );
    }

    #[test]
    fn merge_commits_disable_cherry_pick_and_revert_with_a_reason() {
        let cx = ready();
        for command in [Command::CherryPick, Command::Revert] {
            assert!(command.availability_for_commit(&cx, 1).enabled());
            assert_eq!(
                command.availability_for_commit(&cx, 2).reason(),
                Some(crate::service::MERGE_COMMIT_UNSUPPORTED)
            );
            assert!(command
                .describe_for_commit(&cx, 2)
                .contains("unavailable. This is a merge commit."));
        }
        assert!(Command::Checkout.availability_for_commit(&cx, 2).enabled());
        // Repository-wide reasons still take precedence.
        let busy = CommandContext { busy: true, ..cx };
        assert_eq!(
            Command::Revert.availability_for_commit(&busy, 2).reason(),
            Some("A Git operation is running. Wait for it to finish.")
        );
    }

    #[test]
    fn toolbar_actions_keep_their_order_and_unimplemented_ones_explain_why() {
        assert_eq!(
            Command::PRIMARY.map(Command::label),
            ["Undo", "Redo", "Pull", "Push", "Branch", "Stash", "Pop"]
        );
        let cx = ready();
        for command in [Command::Undo, Command::Redo] {
            assert!(command.writes());
        }
        assert_eq!(
            Command::Undo.availability(&cx).reason(),
            Some(crate::journal::NOTHING_TO_UNDO)
        );
        assert_eq!(
            Command::Redo.availability(&cx).reason(),
            Some(crate::journal::NOTHING_TO_REDO)
        );
        // Pop stays visible without a stash, disabled with its reason.
        let empty = CommandContext {
            stash_count: 0,
            ..cx
        };
        assert_eq!(
            Command::PopStash.availability(&empty).reason(),
            Some("There are no stashes to apply.")
        );
    }

    #[test]
    fn undo_and_redo_follow_their_journal_step() {
        use crate::journal::{Blocker, StepState};
        let mut cx = CommandContext {
            undo: StepState::Ready,
            redo: StepState::Blocked(Blocker::HeadMoved),
            unborn: true,
            ..ready()
        };
        // An unborn HEAD does not hide a step whose own checks pass.
        assert!(Command::Undo.availability(&cx).enabled());
        assert_eq!(
            Command::Undo.describe_with_summary(&cx, Some("commit “Fix parser”")),
            "Undo commit “Fix parser”"
        );
        assert_eq!(
            Command::Redo.availability(&cx).reason(),
            Some(Blocker::HeadMoved.reason())
        );
        assert!(Command::Redo
            .describe_with_summary(&cx, Some("checkout of main"))
            .starts_with("Redo checkout of main unavailable."));
        assert!(Command::Redo
            .describe_with_summary(&cx, Some("checkout of main"))
            .contains(" unavailable. HEAD is no longer where this action left it"));
        // Repository-wide guards still come first.
        cx.busy = true;
        assert_eq!(
            Command::Undo.availability(&cx).reason(),
            Some("A Git operation is running. Wait for it to finish.")
        );
        cx.busy = false;
        cx.modal = true;
        assert_eq!(
            Command::Undo.availability(&cx).reason(),
            Some("Close the open dialog first.")
        );
    }

    fn tracking() -> CommandContext {
        CommandContext {
            remotes: 1,
            upstream: true,
            push_remote: true,
            ahead: 2,
            behind: 3,
            ..ready()
        }
    }

    #[test]
    fn network_commands_are_enabled_with_a_tracked_branch() {
        let cx = tracking();
        for command in [Command::Fetch, Command::Pull, Command::Push] {
            assert!(command.availability(&cx).enabled(), "{command:?}");
        }
        assert!(!Command::Fetch.writes());
        assert!(Command::Pull.writes() && Command::Push.writes());
    }

    #[test]
    fn toolbar_tooltips_name_the_action() {
        let cx = tracking();
        let tooltips = Command::PRIMARY.map(|command| command.toolbar_tooltip(&cx, None));
        assert_eq!(
            &tooltips[2..],
            [
                "Pull (fast-forward if possible)",
                "Push",
                "Branch",
                "Stash",
                "Pop Stash"
            ]
        );
        assert_eq!(
            tooltips[..2],
            ["No recent action to undo", "No recent undo to redo"]
        );

        use crate::journal::{Blocker, StepState};
        let blocked = CommandContext {
            undo: StepState::Blocked(Blocker::BranchMissing),
            ..cx
        };
        assert_eq!(
            Command::Undo.toolbar_tooltip(&blocked, None),
            "The branch no longer exists."
        );
        let busy = CommandContext { busy: true, ..cx };
        assert_eq!(
            Command::Undo.toolbar_tooltip(&busy, None),
            "A Git operation is running. Wait for it to finish."
        );
        let empty = CommandContext {
            stash_count: 0,
            ..cx
        };
        assert_eq!(
            Command::PopStash.toolbar_tooltip(&empty, None),
            Command::PopStash.describe(&empty)
        );
    }

    #[test]
    fn network_commands_explain_why_they_cannot_run() {
        let local = ready();
        for command in [Command::Fetch, Command::Pull, Command::Push] {
            assert_eq!(command.availability(&local).reason(), Some(NO_REMOTES));
        }
        let detached = CommandContext {
            detached: true,
            ..tracking()
        };
        assert_eq!(
            Command::Pull.availability(&detached).reason(),
            Some(NETWORK_DETACHED)
        );
        assert_eq!(
            Command::Push.availability(&detached).reason(),
            Some(NETWORK_DETACHED)
        );
        // Fetch still works without a checked-out branch.
        assert!(Command::Fetch.availability(&detached).enabled());
        let untracked = CommandContext {
            upstream: false,
            ..tracking()
        };
        assert_eq!(
            Command::Pull.availability(&untracked).reason(),
            Some(NO_UPSTREAM)
        );
        assert!(Command::Push.availability(&untracked).enabled());
        let ambiguous = CommandContext {
            remotes: 2,
            push_remote: false,
            ..untracked
        };
        assert_eq!(
            Command::Push.availability(&ambiguous).reason(),
            Some(AMBIGUOUS_PUSH_REMOTE)
        );
        let unborn = CommandContext {
            unborn: true,
            ..tracking()
        };
        assert!(!Command::Push.availability(&unborn).enabled());
        let busy = CommandContext {
            busy: true,
            ..tracking()
        };
        for command in [Command::Fetch, Command::Pull, Command::Push] {
            assert_eq!(
                command.availability(&busy).reason(),
                Some("A Git operation is running. Wait for it to finish."),
            );
        }
        let modal = CommandContext {
            modal: true,
            ..tracking()
        };
        assert_eq!(
            Command::Fetch.availability(&modal).reason(),
            Some("Close the open dialog first.")
        );
    }

    #[test]
    fn unborn_branches_and_missing_snapshots_have_reasons() {
        let mut cx = ready();
        cx.unborn = true;
        assert!(!Command::Merge.availability(&cx).enabled());
        assert!(Command::Refresh.availability(&cx).enabled());
        assert_eq!(
            Command::Refresh
                .availability(&CommandContext::default())
                .reason(),
            Some("Open a repository first.")
        );
        cx.unborn = false;
        cx.reviewable = false;
        assert!(!Command::Stash.availability(&cx).enabled());
    }
}
