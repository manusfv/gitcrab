//! The Git worker: runs Git for a trusted worktree and answers
//! [`gitcrab_core::service::Request`]s.
//!
//! [`Backend`] implements [`GitBackend`] and is the only way the app reaches
//! Git. Everything here is I/O; the decisions it reports are made in
//! `gitcrab-core`.

pub mod git;
pub mod journal;
pub mod platform;
mod process;
pub mod service;
#[cfg(test)]
mod test_repo;

/// Per-Git-process timings, for latency measurements only.
#[cfg(feature = "git-trace")]
pub use process::trace as git_trace;

use gitcrab_core::{diff, graph, hunk_edit};
use gitcrab_core::{
    platform::RepositoryLocation,
    service::{GitBackend, GitWorker, IgnoreCheck, ReadKind, Request, Response},
};
use std::collections::BTreeSet;
use std::path::PathBuf;

impl IgnoreCheck for git::Git {
    fn ignored(&self, paths: &BTreeSet<PathBuf>) -> Result<BTreeSet<PathBuf>, String> {
        self.ignored_paths(paths)
    }
}
use platform::RepositoryLocationExt;
use std::path::Path;
use tokio::sync::oneshot;

impl GitWorker for service::Service {
    fn submit(&self, request: Request) -> Result<oneshot::Receiver<Response>, String> {
        service::Service::submit(self, request)
    }

    fn cancel_read(&self, kind: ReadKind) {
        service::Service::cancel_read(self, kind)
    }

    fn cancel_network(&self) {
        service::Service::cancel_network(self)
    }
}

/// Starts one [`service::Service`] per tab and discovers worktrees on disk.
#[derive(Clone, Copy, Debug, Default)]
pub struct Backend;

impl GitBackend for Backend {
    fn worker(&self) -> Box<dyn GitWorker> {
        Box::new(service::Service::new())
    }

    fn discover(&self, path: &Path) -> Result<RepositoryLocation, String> {
        RepositoryLocation::discover(path)
    }

    fn ignore_check(&self, location: &RepositoryLocation) -> Option<Box<dyn IgnoreCheck>> {
        git::Git::open_confirmed(location.clone(), None)
            .ok()
            .map(|git| Box::new(git) as Box<dyn IgnoreCheck>)
    }

    #[cfg(feature = "git-trace")]
    fn drain_git_calls(&self) -> Vec<gitcrab_core::service::GitCall> {
        git_trace::drain()
    }
}
