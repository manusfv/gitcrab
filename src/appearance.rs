//! Palette availability is a product gate, independent of the host UI.
use crate::design_tokens::{Role, DARK, LIGHT};
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum ThemePreference {
    #[default]
    Dark,
    System,
    #[serde(other)]
    Unknown,
}
impl ThemePreference {
    pub fn resolve(self, _system_dark: bool) -> &'static str {
        "dark"
    }
}
pub fn palette(id: &str) -> &'static [u32] {
    match id {
        "light" => &LIGHT,
        _ => &DARK,
    }
}
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
