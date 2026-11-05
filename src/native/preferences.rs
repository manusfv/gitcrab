//! User-owned native appearance settings. The Git backend has no settings dependency.
use gpui::{App, Global};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;
#[derive(Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct Preferences {
    pub compact: bool,
    pub font_size: f32,
    pub nav_width: f32,
    pub inspector_width: f32,
    pub split: bool,
    pub code_size: f32,
    pub compact_diff: bool,
    pub collapsed: std::collections::BTreeSet<String>,
    pub recent: Vec<String>,
}
impl Default for Preferences {
    fn default() -> Self {
        Self {
            compact: false,
            font_size: 13.,
            nav_width: 210.,
            inspector_width: 360.,
            split: false,
            code_size: 13.,
            compact_diff: false,
            collapsed: Default::default(),
            recent: vec![],
        }
    }
}
struct SharedPreferences(Preferences);
impl Global for SharedPreferences {}

impl Preferences {
    pub fn current(cx: &mut App) -> Self {
        if let Some(shared) = cx.try_global::<SharedPreferences>() {
            shared.0.clone()
        } else {
            let preferences = Self::load();
            cx.set_global(SharedPreferences(preferences.clone()));
            preferences
        }
    }

    pub fn add_recent(path: &str, cx: &mut App) -> Result<(), String> {
        let mut shared = Self::current(cx);
        if shared.recent.first().is_some_and(|recent| recent == path) {
            return Ok(());
        }
        shared.recent.retain(|recent| recent != path);
        shared.recent.insert(0, path.to_owned());
        shared.recent.truncate(10);
        cx.set_global(SharedPreferences(shared.clone()));
        shared.save()
    }

    // Merge only the fields this tab edited since its last save.
    pub fn save_changes(&self, previous: &Self, cx: &mut App) -> Result<(), String> {
        let mut shared = Self::current(cx);
        macro_rules! merge {
            ($($field:ident),+ $(,)?) => {$(
                if self.$field != previous.$field {
                    shared.$field = self.$field;
                }
            )+};
        }
        merge!(
            compact,
            font_size,
            nav_width,
            inspector_width,
            split,
            code_size,
            compact_diff
        );
        for key in previous.collapsed.difference(&self.collapsed) {
            shared.collapsed.remove(key);
        }
        shared
            .collapsed
            .extend(self.collapsed.difference(&previous.collapsed).cloned());
        shared.save()?;
        cx.set_global(SharedPreferences(shared));
        Ok(())
    }

    fn path() -> Option<PathBuf> {
        if let Some(p) = std::env::var_os("GITCRAB_SETTINGS") {
            return Some(p.into());
        }
        #[cfg(windows)]
        let base = PathBuf::from(std::env::var_os("APPDATA")?);
        #[cfg(target_os = "macos")]
        let base = PathBuf::from(std::env::var_os("HOME")?).join("Library/Application Support");
        #[cfg(target_os = "linux")]
        let base = std::env::var_os("XDG_CONFIG_HOME")
            .map(PathBuf::from)
            .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".config")))?;
        Some(base.join("GitCrab/appearance.json"))
    }
    fn session_path() -> Option<PathBuf> {
        Some(Self::path()?.with_extension("session.json"))
    }

    pub fn last_repository() -> Option<String> {
        let bytes = std::fs::read(Self::session_path()?).ok()?;
        serde_json::from_slice::<String>(&bytes)
            .ok()
            .filter(|path| !path.is_empty())
    }

    pub fn remember_repository(repository: &str) -> Result<(), String> {
        let path = Self::session_path().ok_or("No configuration directory is available")?;
        let temporary = path.with_extension(format!("{}.tmp", std::process::id()));
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
        }
        let bytes = serde_json::to_vec(repository).map_err(|e| e.to_string())?;
        let result =
            std::fs::write(&temporary, bytes).and_then(|_| std::fs::rename(&temporary, &path));
        if result.is_err() {
            let _ = std::fs::remove_file(&temporary);
        }
        result.map_err(|e| e.to_string())
    }

    pub fn load() -> Self {
        let mut p = Self::path()
            .and_then(|p| std::fs::read(p).ok())
            .and_then(|b| serde_json::from_slice::<Self>(&b).ok())
            .unwrap_or_default();
        p.font_size = if p.font_size.is_finite() {
            p.font_size.clamp(11., 18.)
        } else {
            13.
        };
        p.nav_width = if p.nav_width.is_finite() {
            p.nav_width.clamp(150., 330.)
        } else {
            210.
        };
        p.inspector_width = if p.inspector_width.is_finite() {
            p.inspector_width.clamp(260., 560.)
        } else {
            360.
        };
        p.code_size = if p.code_size.is_finite() {
            p.code_size.clamp(11., 20.)
        } else {
            13.
        };
        p.recent.truncate(10);
        p
    }
    pub fn save(&self) -> Result<(), String> {
        let path = Self::path().ok_or("No configuration directory is available")?;
        std::fs::create_dir_all(path.parent().unwrap()).map_err(|e| e.to_string())?;
        std::fs::write(
            path,
            serde_json::to_vec_pretty(self).map_err(|e| e.to_string())?,
        )
        .map_err(|e| e.to_string())
    }
}
