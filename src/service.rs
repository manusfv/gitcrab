//! Framework-independent application boundary. One bounded worker owns Git;
//! no subprocess runs on the UI/event-loop or async executor threads.
use crate::{
    diff,
    git::{Git, Hunk, RepoSnapshot},
    graph,
};
use serde::{Deserialize, Serialize};
use std::sync::mpsc::{sync_channel, SyncSender};
use tokio::sync::oneshot;

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(tag = "kind", content = "args")]
pub enum Action {
    Open {
        path: String,
        limit: usize,
    },
    Refresh {
        limit: usize,
    },
    Details {
        id: String,
    },
    Diff {
        id: Option<String>,
        path: String,
        staged: bool,
    },
    Stage {
        path: String,
        unstage: bool,
    },
    Hunk {
        token: u32,
        index: usize,
        unstage: bool,
    },
    Commit {
        message: String,
    },
    Operation {
        op: Operation,
        target: String,
    },
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub enum Operation {
    CreateBranch,
    CreateBranchAt { start: String },
    Checkout,
    Merge,
    CherryPick,
    Revert,
    Stash,
    PopStash,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Request {
    pub epoch: u32,
    pub sequence: u32,
    pub action: Action,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(tag = "kind", content = "data")]
pub enum Reply {
    Snapshot {
        path: String,
        snapshot: RepoSnapshot,
        graph: graph::Graph,
    },
    Details {
        id: String,
        files: Vec<String>,
    },
    Diff {
        id: Option<String>,
        path: String,
        staged: bool,
        lines: Vec<diff::Line>,
        split: Vec<(Option<diff::Line>, Option<diff::Line>)>,
        hunks: Vec<String>,
        token: u32,
    },
    Done {
        message: String,
        committed_draft: Option<String>,
    },
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Failure {
    pub message: String,
    pub refresh: bool,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Response {
    pub epoch: u32,
    pub sequence: u32,
    /// Worker execution only; excludes queueing, serialization and IPC.
    pub elapsed_ms: f64,
    pub result: Result<Reply, Failure>,
}
struct Pending {
    request: Request,
    tx: oneshot::Sender<Response>,
}
pub struct Service {
    tx: SyncSender<Pending>,
}
impl Default for Service {
    fn default() -> Self {
        Self::new()
    }
}
impl Service {
    pub fn new() -> Self {
        let (tx, rx) = sync_channel::<Pending>(8);
        std::thread::Builder::new()
            .name("gitcrab-git".into())
            .spawn(move || {
                let mut session = Session::default();
                for Pending { request, tx } in rx {
                    let _ = tx.send(session.execute(request));
                }
            })
            .expect("start Git worker");
        Self { tx }
    }
    pub fn submit(&self, request: Request) -> Result<oneshot::Receiver<Response>, String> {
        let (tx, rx) = oneshot::channel();
        self.tx.try_send(Pending { request, tx }).map_err(|_| {
            "Git worker is busy. Wait for the current operation and retry.".to_string()
        })?;
        Ok(rx)
    }
}
#[derive(Default)]
struct Session {
    repo: Option<Git>,
    epoch: u32,
    patch: Option<(u32, bool, Vec<Hunk>)>,
}
impl Session {
    fn execute(&mut self, request: Request) -> Response {
        let start = std::time::Instant::now();
        let Request {
            epoch,
            sequence,
            action,
        } = request;
        let mutation = matches!(
            action,
            Action::Stage { .. }
                | Action::Hunk { .. }
                | Action::Commit { .. }
                | Action::Operation { .. }
        );
        let result = self
            .run(epoch, sequence, action)
            .map_err(|message| Failure {
                message,
                refresh: mutation && epoch == self.epoch,
            });
        if mutation {
            self.patch = None;
        }
        Response {
            epoch,
            sequence,
            elapsed_ms: start.elapsed().as_secs_f64() * 1000.0,
            result,
        }
    }
    fn snapshot(&self, limit: usize) -> Result<Reply, String> {
        let g = self.repo.as_ref().ok_or("Open a local repository first")?;
        let snapshot = g.snapshot(limit.clamp(1, 100_000))?;
        let graph = graph::layout_with_head(
            &snapshot.commits,
            (!snapshot.head_id.is_empty()).then_some(snapshot.head_id.as_str()),
        );
        Ok(Reply::Snapshot {
            path: g.path.to_string_lossy().into_owned(),
            snapshot,
            graph,
        })
    }
    fn run(&mut self, epoch: u32, sequence: u32, action: Action) -> Result<Reply, String> {
        if let Action::Open { path, limit } = action {
            if epoch <= self.epoch {
                return Err("Obsolete repository request".into());
            }
            self.epoch = epoch;
            self.repo = None;
            self.patch = None;
            self.repo = Some(Git::open(path)?);
            return self.snapshot(limit);
        }
        if epoch != self.epoch {
            return Err(
                "Repository changed. Retry the operation in the current repository.".into(),
            );
        }
        if let Action::Refresh { limit } = action {
            return self.snapshot(limit);
        }
        let g = self.repo.as_ref().ok_or("Open a local repository first")?;
        let reply = match action {
            Action::Details { id } => Reply::Details {
                files: g.changed_files(&id)?,
                id,
            },
            Action::Diff { id, path, staged } => {
                let patch = g.diff(id.as_deref(), &path, staged)?;
                let hunks = if id.is_none() {
                    crate::git::split_hunks(&patch)
                } else {
                    vec![]
                };
                let titles = hunks.iter().map(|h| h.title.clone()).collect();
                self.patch = Some((sequence, staged, hunks));
                let mut lines = diff::parse(&patch);
                if let Some(first) = lines.iter().position(|l| l.text.starts_with("@@ ")) {
                    lines.drain(..first);
                }
                let split = diff::split(&lines);
                return Ok(Reply::Diff {
                    id,
                    path,
                    staged,
                    lines,
                    split,
                    hunks: titles,
                    token: sequence,
                });
            }
            Action::Stage { path, unstage } => {
                if unstage {
                    g.unstage(&path)?
                } else {
                    g.stage(&path)?
                };
                Reply::Done {
                    message: "Index updated".into(),
                    committed_draft: None,
                }
            }
            Action::Hunk {
                token,
                index,
                unstage,
            } => {
                let (current, staged, hunks) = self
                    .patch
                    .as_ref()
                    .ok_or("Reopen the diff before staging this hunk")?;
                if *current != token || *staged != unstage {
                    return Err("Diff changed. Reopen the file and retry.".into());
                }
                g.apply_hunk(hunks.get(index).ok_or("Hunk no longer exists")?, unstage)?;
                Reply::Done {
                    message: "Index hunk updated".into(),
                    committed_draft: None,
                }
            }
            Action::Commit { message } => {
                g.commit(&message)?;
                Reply::Done {
                    message: "Commit created".into(),
                    committed_draft: Some(message),
                }
            }
            Action::Operation { op, target } => {
                match op {
                    Operation::CreateBranch => g.create_branch(&target)?,
                    Operation::CreateBranchAt { start } => g.create_branch_at(&target, &start)?,
                    Operation::Checkout => g.checkout(&target)?,
                    Operation::Merge => g.merge(&target)?,
                    Operation::CherryPick => g.cherry_pick(&target)?,
                    Operation::Revert => g.revert(&target)?,
                    Operation::Stash => g.stash_with_message(&target)?,
                    Operation::PopStash => g.stash_pop()?,
                }
                Reply::Done {
                    message: "Operation completed".into(),
                    committed_draft: None,
                }
            }
            Action::Open { .. } | Action::Refresh { .. } => unreachable!(),
        };
        // Even failed mutations can change the index or worktree. A stale patch
        // must never be reusable after a successful mutation.
        self.patch = None;
        Ok(reply)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn hunk_tokens_expire_after_mutations_and_failures_refresh() {
        let path = std::env::temp_dir().join(format!(
            "gitcrab-service-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir(&path).unwrap();
        let git = Git { path: path.clone() };
        git.run(&["init", "--quiet", "--initial-branch=main"])
            .unwrap();
        std::fs::write(path.join("new ü file.txt"), "new content\n").unwrap();
        let mut session = Session::default();
        session
            .run(
                1,
                1,
                Action::Open {
                    path: path.to_string_lossy().into_owned(),
                    limit: 500,
                },
            )
            .unwrap();
        session
            .run(
                1,
                2,
                Action::Diff {
                    id: None,
                    path: "new ü file.txt".into(),
                    staged: false,
                },
            )
            .unwrap();
        let stale = session.execute(Request {
            epoch: 1,
            sequence: 3,
            action: Action::Hunk {
                token: 1,
                index: 0,
                unstage: false,
            },
        });
        assert!(stale.result.unwrap_err().refresh);
        assert!(git.run(&["diff", "--cached"]).unwrap().is_empty());
        session
            .run(
                1,
                4,
                Action::Diff {
                    id: None,
                    path: "new ü file.txt".into(),
                    staged: false,
                },
            )
            .unwrap();
        session
            .execute(Request {
                epoch: 1,
                sequence: 5,
                action: Action::Hunk {
                    token: 4,
                    index: 0,
                    unstage: false,
                },
            })
            .result
            .unwrap();
        assert!(session.patch.is_none());
        assert!(git
            .run(&["diff", "--cached"])
            .unwrap()
            .contains("new content"));
        assert_eq!(
            std::fs::read_to_string(path.join("new ü file.txt")).unwrap(),
            "new content\n"
        );
        let failed = session.execute(Request {
            epoch: 1,
            sequence: 6,
            action: Action::Commit {
                message: String::new(),
            },
        });
        assert!(failed.result.unwrap_err().refresh);
        std::fs::create_dir(path.join("nested")).unwrap();
        std::fs::write(path.join("nested/another.txt"), "keep worktree\n").unwrap();
        git.stage(".").unwrap();
        std::fs::write(path.join("nested/another.txt"), "newer worktree\n").unwrap();
        git.unstage(".").unwrap();
        assert!(git.run(&["diff", "--cached"]).unwrap().is_empty());
        assert_eq!(
            std::fs::read_to_string(path.join("nested/another.txt")).unwrap(),
            "newer worktree\n"
        );
        std::fs::remove_dir_all(path).unwrap();
    }
    #[test]
    fn rejects_cross_repository_mutation_and_obsolete_patch() {
        let mut session = Session {
            epoch: 5,
            ..Default::default()
        };
        let response = session.execute(Request {
            epoch: 4,
            sequence: 1,
            action: Action::Commit {
                message: "must never run".into(),
            },
        });
        assert!(!response.result.unwrap_err().refresh);
        assert!(session
            .run(
                4,
                2,
                Action::Open {
                    path: "invalid".into(),
                    limit: 500
                }
            )
            .unwrap_err()
            .contains("Obsolete"));
        assert_eq!(session.epoch, 5);
    }
    #[test]
    fn protocol_round_trip_preserves_repository_identity() {
        let request = Request {
            epoch: 42,
            sequence: 93,
            action: Action::Open {
                path: "repository ü 日本語".into(),
                limit: 10000,
            },
        };
        let json = serde_json::to_string(&request).unwrap();
        let decoded: Request = serde_json::from_str(&json).unwrap();
        assert_eq!(decoded.epoch, 42);
        assert_eq!(decoded.sequence, 93);
        assert!(
            matches!(decoded.action, Action::Open { path, limit: 10000 } if path == "repository ü 日本語")
        );
    }
    #[test]
    fn bounded_queue_reports_busy_without_blocking() {
        let (tx, _rx) = sync_channel::<Pending>(8);
        let service = Service { tx };
        let mut receivers = vec![];
        for sequence in 0..8 {
            receivers.push(
                service
                    .submit(Request {
                        epoch: 1,
                        sequence,
                        action: Action::Refresh { limit: 500 },
                    })
                    .unwrap(),
            );
        }
        assert!(service
            .submit(Request {
                epoch: 1,
                sequence: 9,
                action: Action::Refresh { limit: 500 }
            })
            .unwrap_err()
            .contains("busy"));
    }
    #[test]
    fn dropped_window_receiver_does_not_stop_worker() {
        let service = Service::new();
        let first = service
            .submit(Request {
                epoch: 1,
                sequence: 1,
                action: Action::Refresh { limit: 1 },
            })
            .unwrap();
        drop(first);
        let second = service
            .submit(Request {
                epoch: 1,
                sequence: 2,
                action: Action::Refresh { limit: 1 },
            })
            .unwrap()
            .blocking_recv()
            .unwrap();
        assert_eq!(second.sequence, 2);
        assert!(second.result.is_err());
    }
}
