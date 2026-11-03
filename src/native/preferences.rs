//! User-owned native appearance settings. The Git backend has no settings dependency.
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
            inspector_width: 330.,
            split: false,
            code_size: 13.,
            compact_diff: false,
            collapsed: Default::default(),
            recent: vec![],
        }
    }
}
impl Preferences {
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
            330.
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
