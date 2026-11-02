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
    pub staged: bool,
    pub lines: Vec<diff::Line>,
    pub split: Vec<(Option<diff::Line>, Option<diff::Line>)>,
    pub hunks: Vec<String>,
    pub token: u32,
    pub columns: usize,
    pub split_columns: usize,
}
pub struct Controller {
    pub epoch: u32,
    sequence: u32,
    slots: [Slot; 4],
    pub path: String,
    pub snapshot: Option<RepoSnapshot>,
    pub graph: Graph,
    pub selected: Option<String>,
    pub files: Vec<String>,
    pub patch: Option<Patch>,
    pub draft: String,
    pub busy: bool,
    pub notice: String,
    pub error: bool,
    pub limit: usize,
    pub worker_ms: f64,
    reopen: Option<(Option<String>, String, bool)>,
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
            draft: String::new(),
            busy: false,
            notice: "Open a local repository to begin".into(),
            error: false,
            limit: 500,
            worker_ms: 0.,
            reopen: None,
        }
    }
}
impl Controller {
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
        for l in [Lane::Snapshot, Lane::Details, Lane::Diff] {
            self.invalidate(l);
        }
        self.snapshot = None;
        self.graph = Graph::default();
        self.selected = None;
        self.files.clear();
        self.patch = None;
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
        self.reopen = None;
        id.and_then(|id| self.read(Lane::Details, Action::Details { id }))
    }
    pub fn diff(&mut self, path: String, staged: bool) -> Option<(Lane, Request)> {
        self.invalidate(Lane::Diff);
        self.patch = None;
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
        self.reopen = None;
    }
    pub fn refresh(&mut self) -> Option<(Lane, Request)> {
        self.snapshot.as_ref()?;
        self.remember_patch();
        self.invalidate(Lane::Diff);
        self.read(Lane::Snapshot, Action::Refresh { limit: self.limit })
    }
    fn remember_patch(&mut self) {
        if let Some(p) = self.patch.take() {
            self.reopen = Some((p.id, p.path, p.staged));
        }
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
                        self.notice = format!("{} commits loaded", self.graph.rows.len());
                    }
                    if let Some((id, path, staged)) = self.reopen.take() {
                        if id == self.selected {
                            if let Some(job) = self.diff(path, staged) {
                                jobs.push(job);
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
                    self.patch = Some(Patch {
                        id,
                        path,
                        staged,
                        lines,
                        split,
                        hunks,
                        token,
                        columns,
                        split_columns,
                    });
                }
                Ok(Reply::Done {
                    message,
                    committed_draft,
                }) => {
                    self.notice = message;
                    if committed_draft.as_ref() == Some(&self.draft) {
                        self.draft.clear();
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
                    files: vec!["obsolete".into()],
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
    fn mutation_preserves_new_draft_and_refreshes_failure() {
        let mut c = Controller::default();
        c.epoch = 1;
        c.snapshot = Some(Default::default());
        c.draft = "old".into();
        let (_, a) = c
            .mutate(Action::Commit {
                message: "old".into(),
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
