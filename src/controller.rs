//! Presentation state and coalesced reads. Independent of any window framework.
use crate::{diff, git::RepoSnapshot, graph::Graph, service::*};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Lane {
    Snapshot = 0,
    Details = 1,
    Diff = 2,
    Mutation = 3,
}
#[derive(Default)]
struct Slot {
    active: Option<(u32, u32)>,
    pending: Option<Request>,
    latest: u32,
}
#[derive(Clone, Debug)]
pub struct Patch {
    pub id: Option<String>,
    pub path: String,
    pub original_path: Option<String>,
    pub staged: bool,
    pub lines: Vec<diff::Line>,
    pub split: Vec<(Option<diff::Line>, Option<diff::Line>)>,
    pub hunks: Vec<String>,
    pub token: u32,
    pub columns: usize,
    pub split_columns: usize,
    pub hunk_rows: Vec<usize>,
    pub split_hunk_rows: Vec<usize>,
}
pub struct Controller {
    pub epoch: u32,
    sequence: u32,
    slots: [Slot; 4],
    pub path: String,
    pub snapshot: Option<RepoSnapshot>,
    pub graph: Graph,
    pub selected: Option<String>,
    pub files: Vec<crate::git::FileChange>,
    pub patch: Option<Patch>,
    /// Selected review context survives asynchronous reads and refreshes.
    pub diff_context: Option<(Option<String>, String, bool)>,
    pub draft: String,
    pub draft_revision: u64,
    submitted_revision: Option<u64>,
    pub busy: bool,
    pub notice: String,
    pub error: bool,
    pub limit: usize,
    pub worker_ms: f64,
    reopen: Option<(Option<String>, String, bool)>,
    reopen_position: usize,
    drafts: std::collections::HashMap<String, String>,
}
impl Default for Controller {
    fn default() -> Self {
        Self {
            epoch: 0,
            sequence: 0,
            slots: Default::default(),
            path: String::new(),
            snapshot: None,
            graph: Graph::default(),
            selected: None,
            files: vec![],
            patch: None,
            diff_context: None,
            draft: String::new(),
            draft_revision: 0,
            submitted_revision: None,
            busy: false,
            notice: "Open a local repository to begin".into(),
            error: false,
            limit: 500,
            worker_ms: 0.,
            reopen: None,
            reopen_position: 0,
            drafts: Default::default(),
        }
    }
}
impl Controller {
    pub fn set_draft(&mut self, value: String) {
        if self.draft != value {
            self.draft = value;
            self.draft_revision += 1;
        }
    }

    fn draft_key(path: &str) -> String {
        let canonical = crate::platform::worktree_identity(std::path::Path::new(path));
        let path = canonical.to_string_lossy();
        #[cfg(windows)]
        let path = path.replace('\\', "/");
        path.trim_end_matches('/').to_string()
    }
    fn invalidate(&mut self, lane: Lane) {
        self.sequence += 1;
        let s = &mut self.slots[lane as usize];
        s.latest = self.sequence;
        s.pending = None;
    }
    pub fn open(&mut self, path: String) -> Option<(Lane, Request)> {
        if self.busy {
            return None;
        }
        self.epoch += 1;
        self.drafts
            .insert(Self::draft_key(&self.path), self.draft.clone());
        self.draft = self
            .drafts
            .get(&Self::draft_key(&path))
            .cloned()
            .unwrap_or_default();
        self.path = path.clone();
        for l in [Lane::Snapshot, Lane::Details, Lane::Diff] {
            self.invalidate(l);
        }
        self.snapshot = None;
        self.graph = Graph::default();
        self.selected = None;
        self.files.clear();
        self.patch = None;
        self.diff_context = None;
        self.reopen = None;
        self.limit = 500;
        self.notice = "Opening repository…".into();
        self.error = false;
        self.read(Lane::Snapshot, Action::Open { path, limit: 500 })
    }
    pub fn select(&mut self, id: Option<String>) -> Option<(Lane, Request)> {
        self.invalidate(Lane::Details);
        self.invalidate(Lane::Diff);
        self.selected = id.clone();
        self.files.clear();
        self.patch = None;
        self.diff_context = None;
        self.reopen = None;
        id.and_then(|id| self.read(Lane::Details, Action::Details { id }))
    }
    pub fn diff(&mut self, path: String, staged: bool) -> Option<(Lane, Request)> {
        self.invalidate(Lane::Diff);
        self.patch = None;
        self.diff_context = Some((self.selected.clone(), path.clone(), staged));
        self.read(
            Lane::Diff,
            Action::Diff {
                id: self.selected.clone(),
                path,
                staged,
            },
        )
    }
    pub fn close_diff(&mut self) {
        self.invalidate(Lane::Diff);
        self.patch = None;
        self.diff_context = None;
        self.reopen = None;
    }
    pub fn refresh(&mut self) -> Option<(Lane, Request)> {
        self.snapshot.as_ref()?;
        self.remember_patch();
        self.invalidate(Lane::Diff);
        self.read(Lane::Snapshot, Action::Refresh { limit: self.limit })
    }
    fn remember_patch(&mut self) {
        self.patch = None;
        if let Some(context) = self.diff_context.clone() {
            self.reopen_position = self
                .snapshot
                .as_ref()
                .map(|s| {
                    s.status
                        .iter()
                        .filter(|f| Self::on_side(f, context.2))
                        .position(|f| f.path == context.1)
                        .unwrap_or(0)
                })
                .unwrap_or(0);
            self.reopen = Some(context);
        }
    }
    pub fn on_side(f: &crate::git::FileChange, staged: bool) -> bool {
        if staged {
            f.index != ' ' && f.index != '?'
        } else {
            f.worktree != ' ' || f.index == '?'
        }
    }
    pub fn diff_loading(&self) -> bool {
        self.diff_context.is_some()
            && (self.slots[Lane::Diff as usize].active.is_some()
                || self.slots[Lane::Snapshot as usize].active.is_some()
                || self.busy)
    }
    fn request(&mut self, lane: Lane, action: Action) -> Option<(Lane, Request)> {
        self.sequence += 1;
        let request = Request {
            epoch: self.epoch,
            sequence: self.sequence,
            action,
        };
        let slot = &mut self.slots[lane as usize];
        slot.latest = request.sequence;
        if slot.active.is_some() {
            slot.pending = Some(request);
            None
        } else {
            slot.active = Some((request.epoch, request.sequence));
            Some((lane, request))
        }
    }
    fn read(&mut self, lane: Lane, action: Action) -> Option<(Lane, Request)> {
        self.request(lane, action)
    }
    pub fn mutate(&mut self, action: Action) -> Option<(Lane, Request)> {
        if self.busy || self.snapshot.is_none() {
            return None;
        }
        self.submitted_revision =
            matches!(action, Action::Commit { .. }).then_some(self.draft_revision);
        self.busy = true;
        self.error = false;
        self.notice = "Running Git operation…".into();
        self.invalidate(Lane::Details);
        self.invalidate(Lane::Diff);
        self.remember_patch();
        self.request(Lane::Mutation, action)
    }
    pub fn complete(&mut self, lane: Lane, response: Response) -> Vec<(Lane, Request)> {
        let slot = &mut self.slots[lane as usize];
        if slot.active != Some((response.epoch, response.sequence)) {
            return vec![];
        }
        slot.active = None;
        let current = response.epoch == self.epoch && slot.latest == response.sequence;
        let mut jobs = vec![];
        if current {
            self.worker_ms = response.elapsed_ms;
            match response.result {
                Ok(Reply::Snapshot {
                    path,
                    snapshot,
                    graph,
                }) => {
                    self.path = path;
                    self.snapshot = Some(snapshot);
                    self.graph = graph;
                    if let Some(id) = self.selected.clone() {
                        if self
                            .snapshot
                            .as_ref()
                            .unwrap()
                            .commits
                            .iter()
                            .any(|c| c.id == id)
                        {
                            if let Some(job) = self.read(Lane::Details, Action::Details { id }) {
                                jobs.push(job);
                            }
                        } else {
                            self.select(None);
                        }
                    }
                    if !self.error {
                        self.notice = String::new();
                    }
                    if let Some((id, mut path, staged)) = self.reopen.take() {
                        if id == self.selected {
                            if id.is_none() {
                                let files = self
                                    .snapshot
                                    .as_ref()
                                    .unwrap()
                                    .status
                                    .iter()
                                    .filter(|f| Self::on_side(f, staged))
                                    .collect::<Vec<_>>();
                                if !files.iter().any(|f| f.path == path) {
                                    if let Some(next) = files.get(
                                        self.reopen_position.min(files.len().saturating_sub(1)),
                                    ) {
                                        path = next.path.clone();
                                    } else {
                                        self.diff_context = Some((None, String::new(), staged));
                                        self.notice = if staged {
                                            "No staged changes remain"
                                        } else {
                                            "All working changes reviewed"
                                        }
                                        .into();
                                        path.clear();
                                    }
                                }
                            }
                            if !path.is_empty() {
                                if let Some(job) = self.diff(path, staged) {
                                    jobs.push(job);
                                }
                            }
                        }
                    }
                }
                Ok(Reply::Details { id, files }) if self.selected.as_ref() == Some(&id) => {
                    self.files = files
                }
                Ok(Reply::Diff {
                    id,
                    path,
                    staged,
                    original_path,
                    lines,
                    split,
                    hunks,
                    token,
                }) if self.selected == id => {
                    let columns = lines
                        .iter()
                        .map(|l| l.text.len().min(8192))
                        .max()
                        .unwrap_or(0);
                    let split_columns = split
                        .iter()
                        .map(|(a, b)| {
                            a.as_ref()
                                .map(|l| l.text.len().min(8192))
                                .unwrap_or(0)
                                .max(b.as_ref().map(|l| l.text.len().min(8192)).unwrap_or(0))
                        })
                        .max()
                        .unwrap_or(0);
                    let hunk_rows = lines
                        .iter()
                        .enumerate()
                        .filter_map(|(i, l)| l.text.starts_with("@@ ").then_some(i))
                        .collect();
                    let split_hunk_rows = split
                        .iter()
                        .enumerate()
                        .filter_map(|(i, (l, _))| {
                            l.as_ref()
                                .is_some_and(|l| l.text.starts_with("@@ "))
                                .then_some(i)
                        })
                        .collect();
                    self.patch = Some(Patch {
                        id,
                        path,
                        staged,
                        original_path,
                        lines,
                        split,
                        hunks,
                        token,
                        columns,
                        split_columns,
                        hunk_rows,
                        split_hunk_rows,
                    });
                }
                Ok(Reply::Done {
                    message,
                    committed_draft,
                }) => {
                    self.notice = message;
                    if committed_draft.as_ref() == Some(&self.draft)
                        && self.submitted_revision == Some(self.draft_revision)
                    {
                        self.draft.clear();
                        self.draft_revision += 1;
                    }
                }
                Err(e) => {
                    self.notice = e.message;
                    self.error = true;
                }
                _ => {}
            }
        }
        if lane == Lane::Mutation {
            self.busy = false;
            if response.epoch == self.epoch {
                if let Some(job) = self.refresh() {
                    jobs.push(job);
                }
            }
        }
        let slot = &mut self.slots[lane as usize];
        if let Some(request) = slot.pending.take().filter(|r| r.epoch == self.epoch) {
            slot.active = Some((request.epoch, request.sequence));
            jobs.push((lane, request));
        }
        jobs
    }
    pub fn loading(&self) -> bool {
        self.slots[0].active.is_some()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[cfg(windows)]
    #[test]
    fn native_picker_path_preserves_a_canonical_repository_draft() {
        let mut c = Controller::default();
        c.path = "C:/work/repo".into();
        c.draft = "Unicode draft 日本語".into();
        c.open("C:\\work\\other".into());
        c.open("C:\\work\\repo\\".into());
        assert_eq!(c.draft, "Unicode draft 日本語");
    }
    fn change(path: &str, index: char, worktree: char) -> crate::git::FileChange {
        crate::git::FileChange {
            path: path.into(),
            index,
            worktree,
            original_path: None,
        }
    }
    fn refresh_review(
        old: Vec<crate::git::FileChange>,
        new: Vec<crate::git::FileChange>,
        staged: bool,
    ) -> (Controller, Vec<(Lane, Request)>) {
        let mut c = Controller::default();
        c.epoch = 1;
        c.snapshot = Some(RepoSnapshot {
            status: old,
            ..Default::default()
        });
        c.diff_context = Some((None, "b".into(), staged));
        let (_, mutation) = c
            .mutate(Action::Stage {
                path: "b".into(),
                unstage: staged,
            })
            .unwrap();
        let refresh = c.complete(
            Lane::Mutation,
            reply(
                &mutation,
                Ok(Reply::Done {
                    message: "ok".into(),
                    committed_draft: None,
                }),
            ),
        );
        let jobs = c.complete(
            Lane::Snapshot,
            reply(
                &refresh[0].1,
                Ok(Reply::Snapshot {
                    path: "repo".into(),
                    snapshot: RepoSnapshot {
                        status: new,
                        ..Default::default()
                    },
                    graph: Default::default(),
                }),
            ),
        );
        (c, jobs)
    }
    #[test]
    fn stage_advances_at_same_position_and_unstage_falls_back() {
        let (_, jobs) = refresh_review(
            vec![
                change("a", ' ', 'M'),
                change("b", ' ', 'M'),
                change("c", ' ', 'M'),
            ],
            vec![
                change("a", ' ', 'M'),
                change("b", 'M', ' '),
                change("c", ' ', 'M'),
            ],
            false,
        );
        assert!(matches!(&jobs[0].1.action,Action::Diff{path,staged:false,..} if path=="c"));
        let (_, jobs) = refresh_review(
            vec![change("a", 'M', ' '), change("b", 'M', ' ')],
            vec![change("a", 'M', ' '), change("b", ' ', 'M')],
            true,
        );
        assert!(matches!(&jobs[0].1.action,Action::Diff{path,staged:true,..} if path=="a"));
    }
    #[test]
    fn final_file_completes_review_without_an_empty_patch_request() {
        let (c, jobs) = refresh_review(
            vec![change("b", ' ', 'M')],
            vec![change("b", 'M', ' ')],
            false,
        );
        assert!(jobs.is_empty());
        assert!(c.patch.is_none());
        assert!(!c.diff_loading());
        assert_eq!(c.diff_context, Some((None, String::new(), false)));
    }
    #[test]
    fn remaining_hunks_keep_the_current_file() {
        let (_, jobs) = refresh_review(
            vec![change("b", ' ', 'M'), change("c", ' ', 'M')],
            vec![change("b", 'M', 'M'), change("c", ' ', 'M')],
            false,
        );
        assert!(matches!(&jobs[0].1.action,Action::Diff{path,..} if path=="b"));
    }
    #[test]
    fn repository_switch_drops_pending_review_but_preserves_each_draft() {
        let mut c = Controller::default();
        c.path = "a".into();
        c.draft = "draft a".into();
        c.diff_context = Some((None, "old".into(), false));
        c.open("b".into());
        assert!(c.diff_context.is_none());
        assert!(c.draft.is_empty());
        c.draft = "draft b".into();
        c.open("c".into());
        c.open("a".into());
        assert_eq!(c.draft, "draft a");
        c.open("b".into());
        assert_eq!(c.draft, "draft b");
    }
    fn reply(r: &Request, result: Result<Reply, Failure>) -> Response {
        Response {
            epoch: r.epoch,
            sequence: r.sequence,
            elapsed_ms: 1.,
            result,
        }
    }
    #[test]
    fn rapid_selection_coalesces_and_rejects_old_replies() {
        let mut c = Controller::default();
        c.epoch = 1;
        let (_, a) = c.select(Some("a".into())).unwrap();
        assert!(c.select(Some("b".into())).is_none());
        assert!(c.select(Some("c".into())).is_none());
        let next = c.complete(
            Lane::Details,
            reply(
                &a,
                Ok(Reply::Details {
                    id: "a".into(),
                    files: vec![crate::git::FileChange {
                        path: "obsolete".into(),
                        ..Default::default()
                    }],
                }),
            ),
        );
        assert!(c.files.is_empty());
        assert_eq!(next.len(), 1);
        assert!(matches!(&next[0].1.action, Action::Details { id } if id == "c"));
    }
    #[test]
    fn repository_switch_opens_only_latest() {
        let mut c = Controller::default();
        let (_, a) = c.open("a".into()).unwrap();
        assert!(c.open("b".into()).is_none());
        assert!(c.open("c".into()).is_none());
        let jobs = c.complete(
            Lane::Snapshot,
            reply(
                &a,
                Ok(Reply::Snapshot {
                    path: "a".into(),
                    snapshot: Default::default(),
                    graph: Default::default(),
                }),
            ),
        );
        assert!(c.snapshot.is_none());
        assert_eq!(jobs.len(), 1);
        assert!(matches!(&jobs[0].1.action, Action::Open { path, .. } if path == "c"));
    }
    #[test]
    fn commit_completion_keeps_a_newer_revision_even_when_text_matches() {
        let mut c = Controller {
            epoch: 1,
            snapshot: Some(Default::default()),
            ..Default::default()
        };
        c.set_draft("submitted".into());
        let (_, r) = c
            .mutate(Action::Commit {
                message: c.draft.clone(),
                reviewed: None,
            })
            .unwrap();
        c.set_draft("new edit".into());
        c.set_draft("submitted".into());
        c.complete(
            Lane::Mutation,
            reply(
                &r,
                Ok(Reply::Done {
                    message: "verified".into(),
                    committed_draft: Some("submitted".into()),
                }),
            ),
        );
        assert_eq!(c.draft, "submitted");
    }
    #[test]
    fn mutation_preserves_new_draft_and_refreshes_failure() {
        let mut c = Controller::default();
        c.epoch = 1;
        c.snapshot = Some(Default::default());
        c.draft = "old".into();
        let (_, a) = c
            .mutate(Action::Commit {
                message: "old".into(),
                reviewed: None,
            })
            .unwrap();
        assert!(c
            .mutate(Action::Stage {
                path: ".".into(),
                unstage: false
            })
            .is_none());
        c.draft = "new".into();
        let jobs = c.complete(
            Lane::Mutation,
            reply(
                &a,
                Ok(Reply::Done {
                    message: "ok".into(),
                    committed_draft: Some("old".into()),
                }),
            ),
        );
        assert_eq!(c.draft, "new");
        assert!(!c.busy);
        assert_eq!(jobs.len(), 1);
        c.complete(
            Lane::Snapshot,
            reply(
                &jobs[0].1,
                Ok(Reply::Snapshot {
                    path: "r".into(),
                    snapshot: Default::default(),
                    graph: Default::default(),
                }),
            ),
        );
        let (_, b) = c
            .mutate(Action::Operation {
                op: Operation::Merge,
                target: "x".into(),
            })
            .unwrap();
        assert_eq!(
            c.complete(
                Lane::Mutation,
                reply(
                    &b,
                    Err(Failure {
                        message: "conflict".into(),
                        refresh: true
                    })
                )
            )
            .len(),
            1
        );
        assert!(c.error);
    }
}
