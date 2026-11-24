//! The undo and redo stacks and the Git work that checks and runs them.
use crate::git::{Git, WriteError, WriteIdentity};
use crate::service::{Operation, RemoteCheckout, RemoteCheckoutKind, ReviewedOperation};
pub use gitcrab_core::journal::*;
/// State read before a write that recording it needs.
#[derive(Clone, Debug, Default)]
pub struct Before {
    stash_top: Option<String>,
    stash_subject: String,
    tracked_clean: bool,
}

impl Before {
    /// Reads what recording `reviewed` will need. Only stash writes read anything.
    pub fn capture(git: &Git, reviewed: &ReviewedOperation) -> Result<Self, String> {
        match reviewed.op {
            Operation::Stash => Ok(Self {
                stash_top: git.ref_commit("refs/stash")?,
                ..Self::default()
            }),
            Operation::PopStash => {
                let id = reviewed.target_id.as_deref().unwrap_or_default();
                let stash_subject = git
                    .stash_entries()?
                    .into_iter()
                    .find(|(entry, _)| entry == id)
                    .map(|(_, subject)| subject)
                    .unwrap_or_default();
                Ok(Self {
                    stash_subject,
                    tracked_clean: !git.has_tracked_changes()?,
                    ..Self::default()
                })
            }
            _ => Ok(Self::default()),
        }
    }
}

/// One tab's undo and redo stacks.
#[derive(Clone, Debug, Default)]
pub struct Journal {
    undo: Vec<Entry>,
    redo: Vec<Entry>,
}

impl Journal {
    pub fn undo_entries(&self) -> &[Entry] {
        &self.undo
    }

    pub fn redo_entries(&self) -> &[Entry] {
        &self.redo
    }

    fn stack(&self, direction: Direction) -> &Vec<Entry> {
        match direction {
            Direction::Undo => &self.undo,
            Direction::Redo => &self.redo,
        }
    }

    /// The entry the next Undo or Redo acts on.
    pub fn top(&self, direction: Direction) -> Option<&Entry> {
        self.stack(direction).last()
    }

    /// Forgets every entry, for example when the tab opens another repository.
    pub fn clear(&mut self) {
        self.undo.clear();
        self.redo.clear();
    }

    /// Forgets the redo stack after a new action that is not recorded.
    pub fn clear_redo(&mut self) {
        self.redo.clear();
    }

    /// Records a successful action. A new action clears the redo stack.
    pub fn record(&mut self, entry: Entry) {
        self.redo.clear();
        self.undo.push(entry);
        if self.undo.len() > MAX_ENTRIES {
            self.undo.remove(0);
        }
    }

    /// Records a verified commit made against the `reviewed` HEAD.
    pub fn record_commit(&mut self, reviewed: &WriteIdentity, commit: &str, message: &str) {
        self.record(Entry {
            summary: format!("commit {}", quoted(message)),
            change: Change::Commit {
                head_ref: reviewed.symbolic_head.clone(),
                parent: reviewed.head.clone(),
                commit: commit.to_owned(),
            },
        });
    }

    /// Records that a write moved HEAD's commit without switching branches,
    /// for example a merge, or a pull that updated the checked-out branch.
    ///
    /// `action` is the lowercase action name used in review text, and
    /// `summary` the tooltip summary (`pull into main`). `before` is the
    /// destination identity reviewed before the write. Nothing is recorded
    /// when HEAD did not move. Returns a note for the result message when the
    /// journal could not record the write and was cleared instead.
    pub fn record_head_move(
        &mut self,
        git: &Git,
        action: &str,
        summary: String,
        before: &WriteIdentity,
    ) -> Option<&'static str> {
        let entry = git.head_state().and_then(|(head_ref, after)| {
            if head_ref != before.symbolic_head {
                return Err("HEAD switched branches during the write.".to_owned());
            }
            Ok((after != before.head).then(|| Entry {
                summary,
                change: Change::BranchMoved {
                    action: action.to_owned(),
                    head_ref,
                    before: before.head.clone(),
                    after,
                },
            }))
        });
        self.keep(entry)
    }

    /// Records that `head_ref` (full ref, or empty for detached HEAD) moved
    /// from `before` to `after`, from IDs the caller already verified, for
    /// example a pull's [`crate::git::PullOutcome`]. Does nothing when the
    /// IDs are equal.
    pub fn record_branch_move(
        &mut self,
        action: &str,
        summary: String,
        head_ref: String,
        before: String,
        after: String,
    ) {
        if before != after {
            self.record(Entry {
                summary,
                change: Change::BranchMoved {
                    action: action.to_owned(),
                    head_ref,
                    before,
                    after,
                },
            });
        }
    }

    fn keep(&mut self, entry: Result<Option<Entry>, String>) -> Option<&'static str> {
        match entry {
            Ok(Some(entry)) => {
                self.record(entry);
                None
            }
            Ok(None) => None,
            Err(_) => {
                self.clear();
                Some(JOURNAL_CLEARED)
            }
        }
    }

    /// Records a successful reviewed operation. `before` comes from
    /// [`Before::capture`] before the write ran.
    pub fn record_operation(
        &mut self,
        git: &Git,
        reviewed: &ReviewedOperation,
        before: Result<Before, String>,
    ) -> Option<&'static str> {
        let destination = &reviewed.destination;
        let target_id = reviewed.target_id.clone().unwrap_or_default();
        let label = if reviewed.target.is_empty() || reviewed.target.starts_with(short(&target_id))
        {
            short(&target_id).to_owned()
        } else {
            reviewed.target.clone()
        };
        let here = place(&destination.symbolic_head, &destination.head);
        let moved = |action: &str, summary: String, journal: &mut Self| {
            journal.record_head_move(git, action, summary, destination)
        };
        let created = reviewed
            .remote_branch
            .as_ref()
            .filter(|remote| remote.kind == RemoteCheckoutKind::CreateTracking);
        let entry = match reviewed.op {
            Operation::Merge => {
                return moved("merge", format!("merge of {label} into {here}"), self)
            }
            Operation::CherryPick => {
                return moved("cherry-pick", format!("cherry-pick of {label}"), self)
            }
            Operation::Revert => return moved("revert", format!("revert of {label}"), self),
            Operation::CreateBranch | Operation::CreateBranchAt { .. } => git
                .ref_commit(&format!("refs/heads/{}", reviewed.target))
                .and_then(|tip| match tip {
                    Some(tip) if tip == target_id => Ok(Some(Entry {
                        summary: format!("create branch {}", reviewed.target),
                        change: Change::CreateBranch {
                            name: reviewed.target.clone(),
                            id: target_id.clone(),
                        },
                    })),
                    _ => Err("The new branch could not be verified.".into()),
                }),
            Operation::Checkout => match created {
                Some(remote) => new_branch_entry(git, destination, remote, &target_id),
                None => git.head_state().map(|(to_ref, to_id)| {
                    (to_ref != destination.symbolic_head || to_id != destination.head).then(|| {
                        Entry {
                            summary: format!("checkout of {}", place(&to_ref, &to_id)),
                            change: Change::Checkout {
                                from_ref: destination.symbolic_head.clone(),
                                from_id: destination.head.clone(),
                                to_ref,
                                to_id,
                            },
                        }
                    })
                }),
            },
            Operation::Stash => before.and_then(|before| {
                let top = git.ref_commit("refs/stash")?;
                Ok(top
                    .filter(|top| Some(top) != before.stash_top.as_ref())
                    .map(|stash| Entry {
                        summary: if reviewed.target.trim().is_empty() {
                            "stash".into()
                        } else {
                            format!("stash {}", quoted(&reviewed.target))
                        },
                        change: Change::Stash {
                            head: destination.head.clone(),
                            stash,
                            message: reviewed.target.clone(),
                        },
                    }))
            }),
            Operation::PopStash => before.and_then(|before| {
                let (_, head) = git.head_state()?;
                let parents = git
                    .commit_parents(&target_id)?
                    .ok_or("The popped stash commit is missing.")?;
                let result = git.capture_tracked_state().map_err(|e| e.message)?;
                Ok(Some(Entry {
                    summary: format!("pop of stash {}", quoted(&before.stash_subject)),
                    change: Change::PopStash {
                        head,
                        stash: target_id.clone(),
                        subject: before.stash_subject,
                        result,
                        restorable: before.tracked_clean && parents.len() <= 2,
                    },
                }))
            }),
            // Undo and Redo move entries between stacks themselves. Writes
            // without an entry are left to the live precondition checks.
            _ => Ok(None),
        };
        self.keep(entry)
    }

    /// Checks the top entry of each stack against the repository.
    pub fn status(&self, git: &Git) -> JournalStatus {
        let preview = |direction| {
            self.top(direction).map(|entry| {
                let (blocked, published) = both(
                    || check(git, &entry.change, direction),
                    || published(git, &entry.change, direction),
                );
                let (blocked, published) = match (blocked, published) {
                    (Ok(None), Ok(published)) => (None, published),
                    (Ok(Some(blocker)), _) => (Some(blocker), None),
                    (Err(_), _) | (Ok(None), Err(_)) => (Some(Blocker::Unchecked), None),
                };
                StepPreview {
                    summary: entry.summary.clone(),
                    blocked,
                    published,
                }
            })
        };
        // Each check is a few Git calls of its own, so the two run side by side.
        std::thread::scope(|scope| {
            let redo = scope.spawn(|| preview(Direction::Redo));
            let undo = preview(Direction::Undo);
            JournalStatus {
                undo,
                redo: redo.join().unwrap_or(None),
            }
        })
    }

    /// Resolves the top step for the review dialog, after checking that the
    /// repository still matches the reviewed `destination` and the entry.
    pub fn review(
        &self,
        git: &Git,
        direction: Direction,
        destination: WriteIdentity,
    ) -> Result<ReviewedOperation, String> {
        if git.write_identity()? != destination {
            return Err(
                "HEAD or index changed since the repository was last read. Wait for the refresh, then try again.".into(),
            );
        }
        let entry = self.top(direction).ok_or(match direction {
            Direction::Undo => NOTHING_TO_UNDO,
            Direction::Redo => NOTHING_TO_REDO,
        })?;
        let (blocked, published) = both(
            || check(git, &entry.change, direction),
            || published(git, &entry.change, direction),
        );
        if let Some(blocker) = blocked? {
            return Err(blocker.reason().into());
        }
        let published = published?;
        Ok(ReviewedOperation {
            op: direction.operation(),
            target: String::new(),
            destination,
            target_id: None,
            checkout_ref: None,
            remote_branch: None,
            merge_ref: None,
            target_summary: None,
            network: None,
            journal: Some(Step {
                direction,
                entry: entry.clone(),
                published,
            }),
        })
    }

    /// Runs a reviewed step after checking the destination, the entry and its
    /// preconditions again, then moves the entry to the other stack.
    pub fn execute(
        &mut self,
        git: &Git,
        reviewed: &ReviewedOperation,
    ) -> Result<String, WriteError> {
        let step = reviewed
            .journal
            .as_ref()
            .ok_or("Review Undo or Redo again.")?;
        if git.write_identity()? != reviewed.destination {
            return Err(
                "HEAD or index changed since the repository was last read. Wait for the refresh, then try again.".into(),
            );
        }
        if self.top(step.direction) != Some(&step.entry) {
            return Err("Undo history changed. Wait for the refresh, then try again.".into());
        }
        if let Some(blocker) = check(git, &step.entry.change, step.direction)? {
            return Err(blocker.reason().into());
        }
        let (change, staging_dropped) = perform(git, &step.entry.change, step.direction)?;
        let entry = Entry {
            summary: step.entry.summary.clone(),
            change,
        };
        match step.direction {
            Direction::Undo => {
                self.undo.pop();
                self.redo.push(entry);
            }
            Direction::Redo => {
                self.redo.pop();
                self.undo.push(entry);
            }
        }
        let done = format!("{} {}.", step.direction.verb(), step.entry.summary);
        Ok(if staging_dropped {
            format!("{done} {}", crate::git::POP_STAGING_DROPPED)
        } else {
            done
        })
    }
}

/// The entry for a Checkout that created `remote.local` tracking
/// `remote.tracking` at `id` and switched to it from `destination`, after
/// checking that HEAD and the upstream are as that left them.
fn new_branch_entry(
    git: &Git,
    destination: &WriteIdentity,
    remote: &RemoteCheckout,
    id: &str,
) -> Result<Option<Entry>, String> {
    let full = format!("refs/heads/{}", remote.local);
    let (head, upstream) = both(|| git.head_state(), || git.upstream_of(&full));
    if head? != (full.clone(), id.to_owned())
        || upstream?.as_deref() != Some(remote.tracking.as_str())
    {
        return Err("The new tracking branch could not be verified.".into());
    }
    Ok(Some(Entry {
        summary: format!(
            "checkout of {} as new branch {}",
            remote.remote_name(),
            remote.local
        ),
        change: Change::CheckoutNewBranch {
            from_ref: destination.symbolic_head.clone(),
            from_id: destination.head.clone(),
            name: remote.local.clone(),
            upstream: remote.tracking.clone(),
            id: id.to_owned(),
        },
    }))
}

/// Runs `a` here and `b` on a scoped thread at the same time.
fn both<A: Send, B: Send>(a: impl FnOnce() -> A + Send, b: impl FnOnce() -> B + Send) -> (A, B) {
    std::thread::scope(|scope| {
        let b = scope.spawn(b);
        let a = a();
        (
            a,
            b.join()
                .unwrap_or_else(|panic| std::panic::resume_unwind(panic)),
        )
    })
}

/// Runs three reads at the same time.
fn all3<A: Send, B: Send, C: Send>(
    a: impl FnOnce() -> A + Send,
    b: impl FnOnce() -> B + Send,
    c: impl FnOnce() -> C + Send,
) -> (A, B, C) {
    let (a, (b, c)) = both(a, || both(b, c));
    (a, b, c)
}

/// Decides whether a step can run now.
///
/// Every read is a Git process, so the reads a decision may need start
/// together. Their results are still consulted in the original order, so a
/// read that the decision never reaches cannot change the outcome, not even
/// by failing.
fn check(git: &Git, change: &Change, direction: Direction) -> Result<Option<Blocker>, String> {
    let undo = direction == Direction::Undo;
    let ((markers, unmerged), blocked) = both(
        || both(|| git.operation_markers(), || git.has_unmerged_entries()),
        || check_change(git, change, undo),
    );
    if !markers?.is_empty() || unmerged? {
        return Ok(Some(Blocker::OperationInProgress));
    }
    blocked
}

/// The remote-tracking branch, as a short name such as `origin/main`, that
/// already has the branch tip a step moves away from, when the step takes
/// commits off a local branch.
///
/// Every commit the step takes off is an ancestor of that tip, and one
/// recorded action produced it, so the tip being on a remote means the
/// action's result was shared. Checking only the tip keeps the commits an
/// action merely brought in, such as a merged remote branch's, from counting.
/// The upstream is named when it has the tip, otherwise any remote-tracking
/// branch that does.
fn published(git: &Git, change: &Change, direction: Direction) -> Result<Option<String>, String> {
    let undo = direction == Direction::Undo;
    let (head_ref, from, to) = match change {
        Change::Commit {
            head_ref,
            parent,
            commit,
        } if undo => (head_ref, commit, parent),
        Change::BranchMoved {
            head_ref,
            before,
            after,
            ..
        } if undo => (head_ref, after, before),
        Change::BranchMoved {
            head_ref,
            before,
            after,
            ..
        } => (head_ref, before, after),
        _ => return Ok(None),
    };
    if !head_ref.starts_with("refs/heads/") || from.is_empty() || to.is_empty() {
        return Ok(None);
    }
    let (kept, remotes, upstream) = all3(
        || git.is_ancestor(from, to),
        || git.remote_refs_containing(from),
        || git.upstream_of(head_ref),
    );
    if kept? {
        return Ok(None);
    }
    let remotes = remotes?;
    let upstream = upstream?.filter(|upstream| remotes.contains(upstream));
    Ok(upstream
        .or_else(|| remotes.into_iter().next())
        .map(|full| gitcrab_core::presentation::short_ref(&full).to_owned()))
}

fn check_change(git: &Git, change: &Change, undo: bool) -> Result<Option<Blocker>, String> {
    let head = || git.head_state();
    let blocked = match change {
        Change::Commit {
            head_ref,
            parent,
            commit,
        } => {
            let (expected, other) = if undo {
                (commit, parent)
            } else {
                (parent, commit)
            };
            if undo && parent.is_empty() {
                Some(Blocker::RootCommit)
            } else {
                let (here, other) = both(head, || git.commit_parents(other));
                if here? != (head_ref.clone(), expected.clone()) {
                    Some(Blocker::HeadMoved)
                } else if other?.is_none() {
                    Some(Blocker::CommitMissing)
                } else {
                    None
                }
            }
        }
        Change::Checkout {
            from_ref,
            from_id,
            to_ref,
            to_id,
        } => {
            let ((here_ref, here_id), (there_ref, there_id)) = if undo {
                ((to_ref, to_id), (from_ref, from_id))
            } else {
                ((from_ref, from_id), (to_ref, to_id))
            };
            if there_id.is_empty() {
                Some(Blocker::NoPreviousCommit)
            } else if there_ref.is_empty() {
                let (here, there) = both(head, || git.commit_parents(there_id));
                if here? != (here_ref.clone(), here_id.clone()) {
                    Some(Blocker::HeadMoved)
                } else {
                    there?.is_none().then_some(Blocker::CommitMissing)
                }
            } else {
                let (here, there) = both(head, || git.ref_commit(there_ref));
                if here? != (here_ref.clone(), here_id.clone()) {
                    Some(Blocker::HeadMoved)
                } else {
                    match there? {
                        None => Some(Blocker::BranchMissing),
                        Some(tip) if tip != *there_id => Some(Blocker::BranchMoved),
                        Some(_) => None,
                    }
                }
            }
        }
        Change::CheckoutNewBranch {
            from_ref,
            from_id,
            name,
            upstream,
            id,
        } => {
            let full = format!("refs/heads/{name}");
            if undo {
                if from_id.is_empty() {
                    return Ok(Some(Blocker::NoPreviousCommit));
                }
                let (here, there) = both(head, || {
                    if from_ref.is_empty() {
                        git.commit_parents(from_id)
                            .map(|commit| commit.map(|_| from_id.clone()))
                    } else {
                        git.ref_commit(from_ref)
                    }
                });
                if here? != (full, id.clone()) {
                    Some(Blocker::HeadMoved)
                } else {
                    match there? {
                        None if from_ref.is_empty() => Some(Blocker::CommitMissing),
                        None => Some(Blocker::BranchMissing),
                        Some(tip) if tip != *from_id => Some(Blocker::BranchMoved),
                        Some(_) => None,
                    }
                }
            } else {
                let ((here, tip, commit), tracking) = both(
                    || all3(head, || git.ref_commit(&full), || git.commit_parents(id)),
                    || git.ref_commit(upstream),
                );
                if here? != (from_ref.clone(), from_id.clone()) {
                    Some(Blocker::HeadMoved)
                } else if tip?.is_some() {
                    Some(Blocker::BranchExists)
                } else if commit?.is_none() {
                    Some(Blocker::CommitMissing)
                } else if tracking?.is_none() {
                    Some(Blocker::UpstreamMissing)
                } else {
                    None
                }
            }
        }
        Change::CreateBranch { name, id } => {
            let full = format!("refs/heads/{name}");
            if undo {
                let (tip, here) = both(|| git.ref_commit(&full), head);
                match tip? {
                    None => Some(Blocker::BranchMissing),
                    Some(tip) if tip != *id => Some(Blocker::BranchMoved),
                    Some(_) if here?.0 == full => Some(Blocker::BranchCheckedOut),
                    Some(_) => None,
                }
            } else {
                let (tip, commit) = both(|| git.ref_commit(&full), || git.commit_parents(id));
                if tip?.is_some() {
                    Some(Blocker::BranchExists)
                } else {
                    commit?.is_none().then_some(Blocker::CommitMissing)
                }
            }
        }
        Change::Stash {
            head: at, stash, ..
        } => {
            if undo {
                let (here, entries, changes) =
                    all3(head, || git.stash_entries(), || git.has_tracked_changes());
                if here?.1 != *at {
                    Some(Blocker::HeadMoved)
                } else if !entries?.iter().any(|(id, _)| id == stash) {
                    Some(Blocker::StashMissing)
                } else if changes? {
                    Some(Blocker::LocalChanges)
                } else {
                    None
                }
            } else {
                let (here, commit, matches) = all3(
                    head,
                    || git.commit_parents(stash),
                    || git.tracked_state_matches(stash),
                );
                if here?.1 != *at {
                    Some(Blocker::HeadMoved)
                } else if commit?.is_none() {
                    Some(Blocker::CommitMissing)
                } else if !matches? {
                    Some(Blocker::TreeChanged)
                } else {
                    None
                }
            }
        }
        Change::PopStash {
            head: at,
            stash,
            result,
            restorable,
            ..
        } => {
            if undo && !*restorable {
                // Reading the stash list first keeps its failure reported.
                git.stash_entries()?;
                Some(Blocker::PopNotRestorable)
            } else if undo {
                let ((entries, here), (unchanged, commit)) = both(
                    || both(|| git.stash_entries(), head),
                    || {
                        both(
                            || match result {
                                Some(state) => git.tracked_state_matches(state),
                                None => git.has_tracked_changes().map(|changed| !changed),
                            },
                            || git.commit_parents(stash),
                        )
                    },
                );
                let listed = entries?.iter().any(|(id, _)| id == stash);
                if here?.1 != *at {
                    Some(Blocker::HeadMoved)
                } else {
                    // The tree check ran before the listing check, as before.
                    let unchanged = unchanged?;
                    if listed {
                        Some(Blocker::StashPresent)
                    } else if commit?.is_none() {
                        Some(Blocker::CommitMissing)
                    } else if !unchanged {
                        Some(Blocker::TreeChanged)
                    } else {
                        None
                    }
                }
            } else {
                let (entries, here, changes) =
                    all3(|| git.stash_entries(), head, || git.has_tracked_changes());
                let listed = entries?.iter().any(|(id, _)| id == stash);
                if here?.1 != *at {
                    Some(Blocker::HeadMoved)
                } else if !listed {
                    Some(Blocker::StashMissing)
                } else if changes? {
                    Some(Blocker::LocalChanges)
                } else {
                    None
                }
            }
        }
        Change::BranchMoved {
            head_ref,
            before,
            after,
            ..
        } => {
            let (here, there) = if undo {
                (after, before)
            } else {
                (before, after)
            };
            if there.is_empty() {
                Some(Blocker::NoPreviousCommit)
            } else {
                let (now, commit, changes) = all3(
                    head,
                    || git.commit_parents(there),
                    || git.has_tracked_changes(),
                );
                if now? != (head_ref.clone(), here.clone()) {
                    Some(Blocker::HeadMoved)
                } else if commit?.is_none() {
                    Some(Blocker::CommitMissing)
                } else if changes? {
                    Some(Blocker::LocalChanges)
                } else {
                    None
                }
            }
        }
    };
    Ok(blocked)
}

fn verify_head(git: &Git, head_ref: &str, id: &str) -> Result<(), WriteError> {
    let actual = git.head_state().map_err(WriteError::inspect)?;
    if actual != (head_ref.to_owned(), id.to_owned()) {
        return Err(WriteError::inspect(
            "Outcome uncertain. HEAD is not where this step should have left it. Inspect the repository before continuing.",
        ));
    }
    Ok(())
}

/// Runs a checked step and returns the change to file on the other stack,
/// and whether a redone pop had to apply its staged changes unstaged.
fn perform(git: &Git, change: &Change, direction: Direction) -> Result<(Change, bool), WriteError> {
    let undo = direction == Direction::Undo;
    match change {
        Change::Commit {
            head_ref,
            parent,
            commit,
        } => {
            let target = if undo { parent } else { commit };
            git.reset(target, true)?;
            verify_head(git, head_ref, target)?;
        }
        Change::Checkout {
            from_ref,
            from_id,
            to_ref,
            to_id,
        } => {
            let (there_ref, there_id) = if undo {
                (from_ref, from_id)
            } else {
                (to_ref, to_id)
            };
            git.switch_to(Some(there_ref), there_id)?;
            verify_head(git, there_ref, there_id)?;
        }
        Change::CheckoutNewBranch {
            from_ref,
            from_id,
            name,
            upstream,
            id,
        } => {
            let full = format!("refs/heads/{name}");
            if !undo {
                git.switch_create_tracking(name, upstream, id)?;
                verify_head(git, &full, id)?;
                return Ok((change.clone(), false));
            }
            git.switch_to(Some(from_ref), from_id)?;
            verify_head(git, from_ref, from_id)?;
            let uncertain = |what: &str| {
                WriteError::inspect(format!(
                    "Switched back to {}, but {what}",
                    place(from_ref, from_id)
                ))
            };
            if git.ref_commit(&full).map_err(WriteError::inspect)? != Some(id.clone()) {
                return Err(uncertain(&format!(
                    "branch {name} moved meanwhile, so it was kept."
                )));
            }
            git.delete_branch(name).map_err(|error| {
                uncertain(&format!(
                    "Git could not delete branch {name}. {}",
                    error.message
                ))
            })?;
            if git
                .ref_commit(&full)
                .map_err(WriteError::inspect)?
                .is_some()
            {
                return Err(uncertain(&format!("branch {name} still exists.")));
            }
        }
        Change::CreateBranch { name, id } => {
            let full = format!("refs/heads/{name}");
            if undo {
                git.delete_branch(name)?;
            } else {
                git.create_branch_at(name, id)?;
            }
            let tip = git.ref_commit(&full).map_err(WriteError::inspect)?;
            if tip.is_some() == undo || tip.is_some_and(|tip| tip != *id) {
                return Err(WriteError::inspect(
                    "Outcome uncertain. The branch is not as this step should have left it.",
                ));
            }
        }
        Change::Stash {
            head,
            stash,
            message,
        } => {
            if undo {
                git.stash_apply(stash, true)?;
                git.drop_stash_object(stash)?;
            } else {
                let before = git.ref_commit("refs/stash").map_err(WriteError::inspect)?;
                git.stash_with_message(message)?;
                let after = git.ref_commit("refs/stash").map_err(WriteError::inspect)?;
                match after {
                    Some(stash) if Some(&stash) != before.as_ref() => {
                        return Ok((
                            Change::Stash {
                                head: head.clone(),
                                stash,
                                message: message.clone(),
                            },
                            false,
                        ))
                    }
                    _ => {
                        return Err(WriteError::inspect(
                            "Outcome uncertain. Git did not create a new stash entry.",
                        ))
                    }
                }
            }
        }
        Change::PopStash {
            head,
            stash,
            subject,
            result,
            ..
        } => {
            if undo {
                // Save the changes first, so a failure below never loses them.
                git.stash_store(subject, stash)?;
                let unchanged = match result {
                    Some(state) => git.tracked_state_matches(state),
                    None => git.has_tracked_changes().map(|changed| !changed),
                }
                .map_err(WriteError::inspect)?;
                if !unchanged {
                    return Err(WriteError::inspect(
                        "The stash entry is back in the stash list, but tracked files changed during Undo, so they were left as they are.",
                    ));
                }
                if result.is_some() {
                    git.reset_tracked_to_head()?;
                }
            } else {
                let restored = git.stash_apply_with_index(stash)?;
                git.drop_stash_object(stash)?;
                let result = git.capture_tracked_state()?;
                return Ok((
                    Change::PopStash {
                        head: head.clone(),
                        stash: stash.clone(),
                        subject: subject.clone(),
                        result,
                        restorable: true,
                    },
                    !restored,
                ));
            }
        }
        Change::BranchMoved {
            head_ref,
            before,
            after,
            ..
        } => {
            let target = if undo { before } else { after };
            git.reset_keep(target)?;
            verify_head(git, head_ref, target)?;
        }
    }
    Ok((change.clone(), false))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_repo::Fixture;

    fn base(fixture: &Fixture) -> String {
        fixture.write("tracked.txt", "base\n");
        fixture.git.stage("tracked.txt").unwrap();
        fixture.git.commit("base").unwrap();
        fixture.git.resolve_commit("HEAD").unwrap()
    }

    fn commit(fixture: &Fixture, name: &str, text: &str, message: &str) -> String {
        fixture.write(name, text);
        fixture.git.stage(name).unwrap();
        fixture.git.commit(message).unwrap();
        fixture.git.resolve_commit("HEAD").unwrap()
    }

    fn read(fixture: &Fixture, name: &str) -> String {
        std::fs::read_to_string(fixture.git.path.join(name)).unwrap_or_default()
    }

    fn reviewed(
        op: Operation,
        target: &str,
        destination: WriteIdentity,
        id: Option<String>,
    ) -> ReviewedOperation {
        ReviewedOperation {
            op,
            target: target.into(),
            destination,
            target_id: id,
            checkout_ref: None,
            remote_branch: None,
            merge_ref: None,
            target_summary: None,
            network: None,
            journal: None,
        }
    }

    fn state(journal: &Journal, git: &Git, direction: Direction) -> StepState {
        journal.status(git).state(direction)
    }

    fn step(journal: &mut Journal, git: &Git, direction: Direction) -> Result<String, String> {
        let reviewed = journal.review(git, direction, git.write_identity().unwrap())?;
        journal
            .execute(git, &reviewed)
            .map_err(|error| error.message)
    }

    fn head(git: &Git) -> (String, String) {
        git.head_state().unwrap()
    }

    #[test]
    fn commit_round_trips_and_a_new_action_clears_redo() {
        let fixture = Fixture::new();
        let git = &fixture.git;
        let parent = base(&fixture);
        let mut journal = Journal::default();
        assert_eq!(state(&journal, git, Direction::Undo), StepState::Empty);
        fixture.write("tracked.txt", "second\n");
        git.stage("tracked.txt").unwrap();
        let identity = git.write_identity().unwrap();
        let sha = git.commit_reviewed("second\n\nbody", &identity).unwrap();
        journal.record_commit(&identity, &sha, "second\n\nbody");
        let status = journal.status(git);
        assert_eq!(status.summary(Direction::Undo), Some("commit “second”"));
        assert_eq!(status.state(Direction::Undo), StepState::Ready);

        let reviewed = journal
            .review(git, Direction::Undo, git.write_identity().unwrap())
            .unwrap();
        assert_eq!(reviewed.title(), "Undo");
        assert!(reviewed
            .describe("main")
            .contains("move main back to its parent"));
        assert_eq!(
            journal.execute(git, &reviewed).unwrap(),
            "Undid commit “second”."
        );
        assert_eq!(head(git), ("refs/heads/main".into(), parent.clone()));
        // The commit's changes stay staged; the working tree is untouched.
        assert_eq!(
            git.run(&["diff", "--cached", "--name-only"]).unwrap(),
            "tracked.txt\n"
        );
        assert_eq!(read(&fixture, "tracked.txt"), "second\n");
        assert_eq!(state(&journal, git, Direction::Undo), StepState::Empty);
        assert_eq!(state(&journal, git, Direction::Redo), StepState::Ready);

        step(&mut journal, git, Direction::Redo).unwrap();
        assert_eq!(head(git), ("refs/heads/main".into(), sha.clone()));
        assert!(!git.has_tracked_changes().unwrap());

        // Undo again, then record a new action: Redo is gone.
        step(&mut journal, git, Direction::Undo).unwrap();
        let identity = git.write_identity().unwrap();
        let other = git.commit_reviewed("other", &identity).unwrap();
        journal.record_commit(&identity, &other, "other");
        assert_eq!(state(&journal, git, Direction::Redo), StepState::Empty);
        assert_eq!(journal.undo_entries().len(), 1);
        assert_eq!(
            journal
                .review(git, Direction::Redo, git.write_identity().unwrap())
                .unwrap_err(),
            NOTHING_TO_REDO
        );
    }

    #[test]
    fn external_changes_block_undo_at_availability_and_execution() {
        let fixture = Fixture::new();
        let git = &fixture.git;
        // A root commit is recorded but not undone.
        fixture.write("tracked.txt", "base\n");
        git.stage("tracked.txt").unwrap();
        let unborn = git.write_identity().unwrap();
        let root = git.commit_reviewed("root", &unborn).unwrap();
        let mut journal = Journal::default();
        journal.record_commit(&unborn, &root, "root");
        assert_eq!(
            state(&journal, git, Direction::Undo),
            StepState::Blocked(Blocker::RootCommit)
        );

        fixture.write("tracked.txt", "second\n");
        git.stage("tracked.txt").unwrap();
        let identity = git.write_identity().unwrap();
        let sha = git.commit_reviewed("second", &identity).unwrap();
        journal.record_commit(&identity, &sha, "second");
        assert_eq!(state(&journal, git, Direction::Undo), StepState::Ready);
        // Another client moves the branch: Undo is disabled with the reason.
        let other = Git::open(&git.path).unwrap();
        other
            .write(&["commit", "--allow-empty", "-m", "external"])
            .unwrap();
        assert_eq!(
            state(&journal, git, Direction::Undo),
            StepState::Blocked(Blocker::HeadMoved)
        );
        assert_eq!(
            journal
                .review(git, Direction::Undo, git.write_identity().unwrap())
                .unwrap_err(),
            Blocker::HeadMoved.reason()
        );
        let moved = git.resolve_commit("HEAD").unwrap();
        assert_ne!(moved, sha);

        // A branch moved between review and execution is caught again.
        let mut journal = Journal::default();
        git.create_branch("topic").unwrap();
        journal.record(Entry {
            summary: "create branch topic".into(),
            change: Change::CreateBranch {
                name: "topic".into(),
                id: moved.clone(),
            },
        });
        let reviewed = journal
            .review(git, Direction::Undo, git.write_identity().unwrap())
            .unwrap();
        other.write(&["branch", "-f", "topic", &sha]).unwrap();
        assert_eq!(
            journal.execute(git, &reviewed).unwrap_err().message,
            Blocker::BranchMoved.reason()
        );
        assert_eq!(git.ref_commit("refs/heads/topic").unwrap(), Some(sha));
        assert_eq!(journal.undo_entries().len(), 1);
        // A HEAD or index change since review needs a fresh review.
        other.write(&["branch", "-f", "topic", &moved]).unwrap();
        fixture.write("tracked.txt", "staged\n");
        git.stage("tracked.txt").unwrap();
        assert!(journal
            .execute(git, &reviewed)
            .unwrap_err()
            .message
            .contains("HEAD or index changed"));
    }

    #[test]
    fn checkout_round_trips_and_needs_the_previous_branch_unchanged() {
        let fixture = Fixture::new();
        let git = &fixture.git;
        let start = base(&fixture);
        git.create_branch("feature").unwrap();
        let tip = commit(&fixture, "tracked.txt", "main\n", "main");
        let before = git.write_identity().unwrap();
        git.checkout("feature").unwrap();
        let mut journal = Journal::default();
        let mut op = reviewed(Operation::Checkout, "feature", before, Some(start.clone()));
        op.checkout_ref = Some("refs/heads/feature".into());
        assert_eq!(
            journal.record_operation(git, &op, Before::capture(git, &op)),
            None
        );
        assert_eq!(
            journal.status(git).summary(Direction::Undo),
            Some("checkout of feature")
        );

        step(&mut journal, git, Direction::Undo).unwrap();
        assert_eq!(head(git), ("refs/heads/main".into(), tip.clone()));
        assert_eq!(read(&fixture, "tracked.txt"), "main\n");
        step(&mut journal, git, Direction::Redo).unwrap();
        assert_eq!(head(git), ("refs/heads/feature".into(), start.clone()));
        assert_eq!(read(&fixture, "tracked.txt"), "base\n");

        // The branch to return to moved: Undo is disabled, not redirected.
        git.write(&["branch", "-f", "main", &start]).unwrap();
        assert_eq!(
            state(&journal, git, Direction::Undo),
            StepState::Blocked(Blocker::BranchMoved)
        );
        git.write(&["branch", "-D", "main"]).unwrap();
        assert_eq!(
            state(&journal, git, Direction::Undo),
            StepState::Blocked(Blocker::BranchMissing)
        );
    }

    #[test]
    fn created_branch_round_trips_unless_checked_out() {
        let fixture = Fixture::new();
        let git = &fixture.git;
        let id = base(&fixture);
        let identity = git.write_identity().unwrap();
        git.create_branch_at("topic", &id).unwrap();
        let mut journal = Journal::default();
        let op = reviewed(Operation::CreateBranch, "topic", identity, Some(id.clone()));
        journal.record_operation(git, &op, Before::capture(git, &op));
        assert_eq!(
            journal.status(git).summary(Direction::Undo),
            Some("create branch topic")
        );
        step(&mut journal, git, Direction::Undo).unwrap();
        assert_eq!(git.ref_commit("refs/heads/topic").unwrap(), None);
        // Another client recreates it: Redo is disabled.
        git.write(&["branch", "topic"]).unwrap();
        assert_eq!(
            state(&journal, git, Direction::Redo),
            StepState::Blocked(Blocker::BranchExists)
        );
        git.write(&["branch", "-D", "topic"]).unwrap();
        step(&mut journal, git, Direction::Redo).unwrap();
        assert_eq!(git.ref_commit("refs/heads/topic").unwrap(), Some(id));
        git.checkout("topic").unwrap();
        assert_eq!(
            state(&journal, git, Direction::Undo),
            StepState::Blocked(Blocker::BranchCheckedOut)
        );
    }

    #[test]
    fn stash_round_trips_with_its_index_state() {
        let fixture = Fixture::new();
        let git = &fixture.git;
        let start = base(&fixture);
        fixture.write("staged.txt", "staged\n");
        git.stage("staged.txt").unwrap();
        fixture.write("tracked.txt", "unstaged\n");
        let identity = git.write_identity().unwrap();
        let op = reviewed(Operation::Stash, "wip", identity, None);
        let before = Before::capture(git, &op);
        git.stash_with_message("wip").unwrap();
        let mut journal = Journal::default();
        journal.record_operation(git, &op, before);
        assert_eq!(
            journal.status(git).summary(Direction::Undo),
            Some("stash “wip”")
        );

        step(&mut journal, git, Direction::Undo).unwrap();
        assert!(git.stash_entries().unwrap().is_empty());
        assert_eq!(read(&fixture, "tracked.txt"), "unstaged\n");
        assert_eq!(
            git.run(&["diff", "--cached", "--name-only"]).unwrap(),
            "staged.txt\n"
        );
        assert_eq!(head(git).1, start);

        step(&mut journal, git, Direction::Redo).unwrap();
        let entries = git.stash_entries().unwrap();
        assert_eq!(entries.len(), 1);
        assert!(entries[0].1.ends_with("wip"), "{entries:?}");
        assert!(!git.has_tracked_changes().unwrap());
        // The redone entry tracks the new stash object.
        let Change::Stash { stash, .. } = &journal.top(Direction::Undo).unwrap().change else {
            panic!("stash entry expected");
        };
        assert_eq!(*stash, entries[0].0);

        fixture.write("tracked.txt", "later\n");
        assert_eq!(
            state(&journal, git, Direction::Undo),
            StepState::Blocked(Blocker::LocalChanges)
        );
    }

    #[test]
    fn popped_stash_returns_only_when_the_tree_holds_exactly_its_changes() {
        let fixture = Fixture::new();
        let git = &fixture.git;
        let start = base(&fixture);
        fixture.write("tracked.txt", "stashed\n");
        git.stash_with_message("saved").unwrap();
        let pop = |journal: &mut Journal| {
            let id = git.resolve_commit("refs/stash").unwrap();
            let op = reviewed(
                Operation::PopStash,
                "",
                git.write_identity().unwrap(),
                Some(id.clone()),
            );
            let before = Before::capture(git, &op);
            assert!(git.stash_apply_with_index(&id).unwrap());
            git.drop_stash_object(&id).unwrap();
            journal.record_operation(git, &op, before);
        };
        let mut journal = Journal::default();
        pop(&mut journal);
        assert_eq!(
            journal.status(git).summary(Direction::Undo),
            Some("pop of stash “On main: saved”")
        );
        step(&mut journal, git, Direction::Undo).unwrap();
        let entries = git.stash_entries().unwrap();
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].1, "On main: saved");
        assert_eq!(read(&fixture, "tracked.txt"), "base\n");
        assert!(!git.has_tracked_changes().unwrap());
        assert_eq!(head(git).1, start);

        step(&mut journal, git, Direction::Redo).unwrap();
        assert!(git.stash_entries().unwrap().is_empty());
        assert_eq!(read(&fixture, "tracked.txt"), "stashed\n");

        // Editing after the pop means the changes can no longer be told apart.
        fixture.write("tracked.txt", "stashed and edited\n");
        assert_eq!(
            state(&journal, git, Direction::Undo),
            StepState::Blocked(Blocker::TreeChanged)
        );

        // A pop onto other tracked changes is recorded but cannot be undone.
        fixture.write("tracked.txt", "base\n");
        fixture.write("other.txt", "other\n");
        git.stage("other.txt").unwrap();
        git.commit("other").unwrap();
        fixture.write("tracked.txt", "second stash\n");
        git.stash_with_message("second").unwrap();
        fixture.write("other.txt", "local\n");
        pop(&mut journal);
        assert_eq!(
            state(&journal, git, Direction::Undo),
            StepState::Blocked(Blocker::PopNotRestorable)
        );
    }

    #[test]
    fn undo_and_redo_of_a_pop_keep_the_staged_and_unstaged_split() {
        let fixture = Fixture::new();
        let git = &fixture.git;
        base(&fixture);
        fixture.write("tracked.txt", "staged\n");
        git.stage("tracked.txt").unwrap();
        fixture.write("tracked.txt", "staged and edited\n");
        git.stash_with_message("split").unwrap();
        let split = |git: &Git| {
            let status = git.snapshot(1).unwrap().status;
            assert_eq!(status.len(), 1);
            assert_eq!((status[0].index, status[0].worktree), ('M', 'M'));
            assert_eq!(git.run(&["show", ":tracked.txt"]).unwrap(), "staged\n");
            assert_eq!(read(&fixture, "tracked.txt"), "staged and edited\n");
        };
        let id = git.resolve_commit("refs/stash").unwrap();
        let op = reviewed(
            Operation::PopStash,
            "",
            git.write_identity().unwrap(),
            Some(id.clone()),
        );
        let before = Before::capture(git, &op);
        assert!(git.stash_apply_with_index(&id).unwrap());
        git.drop_stash_object(&id).unwrap();
        let mut journal = Journal::default();
        journal.record_operation(git, &op, before);
        split(git);

        step(&mut journal, git, Direction::Undo).unwrap();
        assert!(!git.has_tracked_changes().unwrap());
        assert_eq!(git.stash_entries().unwrap()[0].0, id);

        step(&mut journal, git, Direction::Redo).unwrap();
        assert!(git.stash_entries().unwrap().is_empty());
        split(git);
    }

    #[test]
    fn merge_and_revert_reset_back_only_on_a_clean_tree() {
        let fixture = Fixture::new();
        let git = &fixture.git;
        base(&fixture);
        git.create_branch("feature").unwrap();
        git.checkout("feature").unwrap();
        let feature = commit(&fixture, "feature.txt", "feature\n", "feature");
        git.checkout("main").unwrap();
        commit(&fixture, "main.txt", "main\n", "main");
        let before = git.write_identity().unwrap();
        git.merge(&feature).unwrap();
        let merged = git.resolve_commit("HEAD").unwrap();
        let mut journal = Journal::default();
        let op = reviewed(
            Operation::Merge,
            "feature",
            before.clone(),
            Some(feature.clone()),
        );
        journal.record_operation(git, &op, Before::capture(git, &op));
        assert_eq!(
            journal.status(git).summary(Direction::Undo),
            Some("merge of feature into main")
        );
        let review = journal
            .review(git, Direction::Undo, git.write_identity().unwrap())
            .unwrap();
        assert!(review.describe("main").contains("none are discarded"));

        fixture.write("main.txt", "uncommitted\n");
        assert_eq!(
            state(&journal, git, Direction::Undo),
            StepState::Blocked(Blocker::LocalChanges)
        );
        fixture.write("main.txt", "main\n");
        step(&mut journal, git, Direction::Undo).unwrap();
        assert_eq!(head(git), ("refs/heads/main".into(), before.head.clone()));
        assert!(!git.path.join("feature.txt").exists());
        step(&mut journal, git, Direction::Redo).unwrap();
        assert_eq!(head(git).1, merged);
        assert_eq!(read(&fixture, "feature.txt"), "feature\n");

        // Revert records the same kind of entry.
        let before = git.write_identity().unwrap();
        git.revert(&feature).unwrap();
        let op = reviewed(
            Operation::Revert,
            &feature,
            before.clone(),
            Some(feature.clone()),
        );
        journal.record_operation(git, &op, Before::capture(git, &op));
        assert_eq!(
            journal.status(git).summary(Direction::Undo),
            Some(format!("revert of {}", &feature[..8]).as_str())
        );
        step(&mut journal, git, Direction::Undo).unwrap();
        assert_eq!(head(git).1, before.head);
        assert_eq!(read(&fixture, "feature.txt"), "feature\n");
    }

    #[test]
    fn a_verified_branch_move_such_as_a_pull_can_be_recorded_and_undone() {
        let fixture = Fixture::new();
        let git = &fixture.git;
        let before = base(&fixture);
        let after = commit(&fixture, "pulled.txt", "pulled\n", "upstream");
        let mut journal = Journal::default();
        journal.record_branch_move(
            "pull",
            "pull into main".into(),
            "refs/heads/main".into(),
            before.clone(),
            after.clone(),
        );
        assert_eq!(
            journal.status(git).summary(Direction::Undo),
            Some("pull into main")
        );
        step(&mut journal, git, Direction::Undo).unwrap();
        assert_eq!(head(git).1, before);
        assert!(!git.path.join("pulled.txt").exists());
        step(&mut journal, git, Direction::Redo).unwrap();
        assert_eq!(head(git).1, after);
        // An unchanged branch records nothing.
        journal.record_branch_move(
            "pull",
            "pull".into(),
            "refs/heads/main".into(),
            after.clone(),
            after,
        );
        assert_eq!(journal.undo_entries().len(), 1);
    }

    mod published {
        use super::*;
        use crate::git::network_tests::{commit, connected, publish};

        fn published(journal: &Journal, git: &Git, direction: Direction) -> Option<String> {
            journal.status(git).published(direction).map(str::to_owned)
        }

        fn record_commit(journal: &mut Journal, fixture: &Fixture, name: &str) -> String {
            fixture.write(name, &format!("{name}\n"));
            fixture.git.stage(name).unwrap();
            let identity = fixture.git.write_identity().unwrap();
            let sha = fixture.git.commit_reviewed(name, &identity).unwrap();
            journal.record_commit(&identity, &sha, name);
            sha
        }

        #[test]
        fn undoing_a_pushed_commit_names_the_remote_that_keeps_it() {
            let (local, _other) = connected();
            let git = &local.git;
            git.run(&["switch", "--quiet", "-c", "local-only"]).unwrap();
            let mut journal = Journal::default();
            let pushed = record_commit(&mut journal, &local, "Pushed commit");
            assert_eq!(published(&journal, git, Direction::Undo), None);
            git.run(&["push", "--quiet", "-u", "origin", "local-only"])
                .unwrap();
            let status = journal.status(git);
            assert_eq!(status.state(Direction::Undo), StepState::Ready);
            assert_eq!(status.published(Direction::Undo), Some("origin/local-only"));
            assert_eq!(
                published_note("origin/local-only"),
                "It is already on origin/local-only; only your local branch moves."
            );

            record_commit(&mut journal, &local, "Local commit");
            assert_eq!(published(&journal, git, Direction::Undo), None);
            let reviewed = journal
                .review(git, Direction::Undo, git.write_identity().unwrap())
                .unwrap();
            assert_eq!(reviewed.journal.unwrap().published, None);
            step(&mut journal, git, Direction::Undo).unwrap();
            git.run(&["reset", "--quiet", "--hard"]).unwrap();

            let reviewed = journal
                .review(git, Direction::Redo, git.write_identity().unwrap())
                .unwrap();
            assert_eq!(reviewed.journal.unwrap().published, None);

            let reviewed = journal
                .review(git, Direction::Undo, git.write_identity().unwrap())
                .unwrap();
            assert_eq!(
                reviewed.journal.as_ref().unwrap().published.as_deref(),
                Some("origin/local-only")
            );
            journal.execute(git, &reviewed).unwrap();
            assert_ne!(head(git).1, pushed);
            assert_eq!(published(&journal, git, Direction::Redo), None);
        }

        #[test]
        fn any_remote_tracking_branch_counts_but_the_upstream_is_named_first() {
            let (local, _other) = connected();
            let git = &local.git;
            let mut journal = Journal::default();
            record_commit(&mut journal, &local, "Backed up");
            git.run(&["push", "--quiet", "origin", "HEAD:backup"])
                .unwrap();
            assert_eq!(
                published(&journal, git, Direction::Undo).as_deref(),
                Some("origin/backup")
            );
            git.run(&["push", "--quiet"]).unwrap();
            assert_eq!(
                published(&journal, git, Direction::Undo).as_deref(),
                Some("origin/main")
            );
        }

        #[test]
        fn branch_moves_warn_only_when_the_tip_they_leave_is_on_a_remote() {
            let (local, other) = connected();
            let git = &local.git;
            let before = git.resolve_commit("HEAD").unwrap();
            let pulled = publish(&other, "upstream.txt");
            git.run(&["pull", "--quiet", "--ff-only"]).unwrap();
            let mut journal = Journal::default();
            journal.record_branch_move(
                "pull",
                "pull into main".into(),
                "refs/heads/main".into(),
                before,
                pulled,
            );
            assert_eq!(
                published(&journal, git, Direction::Undo).as_deref(),
                Some("origin/main")
            );
            step(&mut journal, git, Direction::Undo).unwrap();
            assert_eq!(published(&journal, git, Direction::Redo), None);
            step(&mut journal, git, Direction::Redo).unwrap();

            other
                .git
                .run(&["switch", "--quiet", "-c", "feature"])
                .unwrap();
            commit(&other, "feature.txt", "feature\n");
            other
                .git
                .run(&["push", "--quiet", "-u", "origin", "feature"])
                .unwrap();
            git.run(&["fetch", "--quiet"]).unwrap();
            commit(&local, "local.txt", "local\n");
            let destination = git.write_identity().unwrap();
            git.merge("origin/feature").unwrap();
            let op = reviewed(Operation::Merge, "origin/feature", destination, None);
            journal.record_operation(git, &op, Before::capture(git, &op));
            assert_eq!(state(&journal, git, Direction::Undo), StepState::Ready);
            assert_eq!(published(&journal, git, Direction::Undo), None);
            git.run(&["push", "--quiet"]).unwrap();
            assert_eq!(
                published(&journal, git, Direction::Undo).as_deref(),
                Some("origin/main")
            );
        }
    }
}
