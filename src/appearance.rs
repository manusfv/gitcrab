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
    fn catalog_aliases_resolve_in_every_palette() {
        let doc: serde_json::Value =
            serde_json::from_str(include_str!("design_tokens.json")).unwrap();
        for id in ["dark", "light"] {
            for (i, name) in crate::design_tokens::ROLE_NAMES.iter().enumerate() {
                let alias = doc["color"]["semantic"][name].as_str().unwrap();
                let key = alias
                    .trim_start_matches("{color.primitive.$theme.")
                    .trim_end_matches('}');
                let value = doc["color"]["primitive"][id][key].as_str().unwrap();
                assert_eq!(
                    palette(id)[i],
                    u32::from_str_radix(&value[1..], 16).unwrap(),
                    "{id}: {name}"
                );
            }
        }
        assert_eq!(
            crate::design_tokens::ROLE_NAMES.len(),
            doc["color"]["semantic"].as_object().unwrap().len()
        );
    }
    #[test]
    fn invalid_preference_and_system_use_approved_fallback() {
        let unknown: ThemePreference = serde_json::from_str("\"deleted-palette\"").unwrap();
        assert_eq!(unknown.resolve(false), "dark");
        assert_eq!(ThemePreference::System.resolve(false), "dark");
        assert_eq!(palette("missing"), palette("dark"));
    }
}
