//! User-owned native appearance settings. The Git backend has no settings dependency.
use gpui::{App, Global};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;
#[derive(Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct Preferences {
    pub compact: bool,
    pub theme: gitcrab_core::appearance::ThemePreference,
    pub font_size: f32,
    pub nav_width: f32,
    pub inspector_width: f32,
    pub split: bool,
    pub code_size: f32,
    pub compact_diff: bool,
    pub always_show_file_actions: bool,
    pub collapsed: std::collections::BTreeSet<String>,
    pub recent: Vec<String>,
    pub history_columns: gitcrab_core::history_columns::HistoryColumns,
}
impl Default for Preferences {
    fn default() -> Self {
        Self {
            compact: false,
            theme: Default::default(),
            font_size: 13.,
            nav_width: 210.,
            inspector_width: 360.,
            split: false,
            code_size: 13.,
            compact_diff: false,
            always_show_file_actions: false,
            collapsed: Default::default(),
            recent: vec![],
            history_columns: Default::default(),
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
            theme,
            font_size,
            nav_width,
            inspector_width,
            split,
            code_size,
            compact_diff,
            always_show_file_actions
        );
        if self.history_columns != previous.history_columns {
            shared.history_columns = self.history_columns.clone();
        }
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

    pub fn forget_last_repository(repository: &str) -> std::io::Result<()> {
        if let Some(path) =
            Self::session_path().filter(|_| Self::last_repository().as_deref() == Some(repository))
        {
            std::fs::remove_file(path)?;
        }
        Ok(())
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
            p.font_size.clamp(11., 26.)
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
            p.code_size.clamp(11., 26.)
        } else {
            13.
        };
        p.recent.truncate(10);
        p.history_columns.normalize();
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

#[cfg(test)]
mod tests {
    use super::*;
    #[::core::prelude::v1::test]
    fn old_preferences_keep_values_and_gain_the_default_palette() {
        let p: Preferences = serde_json::from_str(
            r#"{"compact":true,"font_size":15,"recent":["example"],"split":true}"#,
        )
        .unwrap();
        assert!(p.compact && p.split);
        assert_eq!(p.font_size, 15.);
        assert_eq!(p.recent, ["example"]);
        assert_eq!(p.history_columns, Default::default());
        assert_eq!(p.theme, gitcrab_core::appearance::ThemePreference::Dark);
        let p: Preferences = serde_json::from_str(r#"{"theme":"missing","code_size":17}"#).unwrap();
        assert_eq!(p.code_size, 17.);
        assert_eq!(p.theme.resolve(false), "dark");
    }
}
