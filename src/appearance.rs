//! Palette lookup and the policy for selecting an application palette.

use crate::design_tokens::{Role, DARK, LIGHT};

/// Saved theme preference, including values from older or newer settings files.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum ThemePreference {
    #[default]
    Dark,
    /// Follow the system appearance when supported.
    System,
    /// Fallback for an unrecognized preference in saved settings.
    #[serde(other)]
    Unknown,
}
impl ThemePreference {
    /// Returns the approved application palette ID.
    ///
    /// All preferences currently resolve to `"dark"`, regardless of system appearance.
    pub fn resolve(self, _system_dark: bool) -> &'static str {
        "dark"
    }
}
/// Looks up `"light"` or `"dark"`, with dark as the fallback for unknown IDs.
///
/// Colors are packed as `0xRRGGBB` and indexed by [`Role`].
pub fn palette(id: &str) -> &'static [u32] {
    match id {
        "light" => &LIGHT,
        _ => &DARK,
    }
}
/// Returns the packed RGB color for `role` in the palette selected by [`palette`].
pub fn resolve(id: &str, role: Role) -> u32 {
    palette(id)[role as usize]
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn invalid_preference_and_system_use_approved_fallback() {
        let unknown: ThemePreference = serde_json::from_str("\"deleted-palette\"").unwrap();
        assert_eq!(unknown.resolve(false), "dark");
        assert_eq!(ThemePreference::System.resolve(false), "dark");
        assert_eq!(palette("missing"), palette("dark"));
    }
}
