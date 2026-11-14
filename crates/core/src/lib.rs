//! Repository and presentation models for the GitCrab desktop client.
//!
//! Use [`platform::RepositoryLocation::discover`] to identify a worktree for
//! trust confirmation. Once confirmed, a [`controller::Controller`] tracks
//! the tab's state and produces requests for its [`service::Service`] worker.
//! The controller applies the worker's responses and returns follow-up requests
//! for the caller to dispatch.
//!
//! Patch presentation can also be used on its own.
//!
//! ```
//! use gitcrab_core::diff::{copy_source, parse, prepare, Side};
//!
//! let patch = "@@ -1 +1 @@\n-before\n+after\n";
//! let content = prepare(parse(patch));
//! let after = copy_source(&content.lines, Some((&content.split, Side::After)), 0..content.split.len());
//! assert_eq!(after, "after");
//! ```

pub mod commands;
pub mod controller;
pub mod diff;
pub mod git;
pub mod graph;
pub mod highlight;
pub mod history_columns;
pub mod journal;
pub mod platform;
pub mod presentation;
pub mod service;

#[rustfmt::skip]
pub mod design_tokens;
pub mod appearance;

pub mod hunk_edit;
mod process;
/// Per-Git-process timings, for latency measurements only.
#[cfg(feature = "git-trace")]
pub use process::trace as git_trace;

#[cfg(test)]
mod test_repo;
