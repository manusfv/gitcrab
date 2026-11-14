//! The Git backend the app installs, shared by every window and tab.
use gitcrab_core::service::GitBackend;
use gpui::{App, Global};
use std::sync::Arc;

/// Global handle to the [`GitBackend`] the app was started with.
pub struct Backend(pub Arc<dyn GitBackend>);

impl Global for Backend {}

/// Installs the backend. Call once at startup, before opening a window.
pub fn install(backend: Arc<dyn GitBackend>, cx: &mut App) {
    cx.set_global(Backend(backend));
}

/// The installed backend.
///
/// # Panics
///
/// Panics if [`install`] has not run.
pub fn get(cx: &App) -> Arc<dyn GitBackend> {
    cx.global::<Backend>().0.clone()
}
