//! GPUI views and components for GitCrab.
//!
//! Domain logic lives in [`gitcrab_core`]; this crate renders its state and
//! turns user input into core requests.

pub mod assets;
pub mod backend;
mod components;
pub mod preferences;
mod repository_setup;
pub mod theme;
#[cfg(feature = "native-evidence")]
mod trace;
pub mod updates;
pub mod view;
mod watch;
pub mod workspace;

/// The build's revision label, for the About screen and `--version`.
pub const BUILD: &str = env!("GITCRAB_BUILD");
