//! Domain logic for the GitCrab desktop client. No UI and no I/O.
//!
//! A [`controller::Controller`] tracks a tab's state and produces
//! [`service::Request`]s. The caller hands them to a [`service::GitWorker`]
//! (implemented by the `gitcrab-git` crate), applies each
//! [`service::Response`] through the controller, and dispatches the follow-up
//! requests it returns. Repositories are found for trust confirmation through
//! [`service::GitBackend::discover`].
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
pub mod preferences;
pub mod presentation;
pub mod service;
pub mod sidebar;
pub mod tabs;
pub mod watch;

#[rustfmt::skip]
pub mod design_tokens;
pub mod appearance;

pub mod hunk_edit;
