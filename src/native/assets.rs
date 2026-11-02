use gpui::{AssetSource, Result, SharedString};
use std::borrow::Cow;
pub struct Assets;
impl AssetSource for Assets {
    fn load(&self, path: &str) -> Result<Option<Cow<'static, [u8]>>> {
        Ok(match path {
            "gitcrab.png" => Some(Cow::Borrowed(include_bytes!("../../icons/32x32.png"))),
            "icons/branch.svg" => Some(Cow::Borrowed(include_bytes!(
                "../../icons/native/branch.svg"
            ))),
            "icons/folder.svg" => Some(Cow::Borrowed(include_bytes!(
                "../../icons/native/folder.svg"
            ))),
            "icons/changes.svg" => Some(Cow::Borrowed(include_bytes!(
                "../../icons/native/changes.svg"
            ))),
            "icons/history.svg" => Some(Cow::Borrowed(include_bytes!(
                "../../icons/native/history.svg"
            ))),
            "icons/refresh.svg" => Some(Cow::Borrowed(include_bytes!(
                "../../icons/native/refresh.svg"
            ))),
            "icons/merge.svg" => Some(Cow::Borrowed(include_bytes!(
                "../../icons/native/merge.svg"
            ))),
            "icons/stash.svg" => Some(Cow::Borrowed(include_bytes!(
                "../../icons/native/stash.svg"
            ))),
            "icons/settings.svg" => Some(Cow::Borrowed(include_bytes!(
                "../../icons/native/settings.svg"
            ))),
            "icons/search.svg" => Some(Cow::Borrowed(include_bytes!(
                "../../icons/native/search.svg"
            ))),
            "icons/check.svg" => Some(Cow::Borrowed(include_bytes!(
                "../../icons/native/check.svg"
            ))),
            "icons/tag.svg" => Some(Cow::Borrowed(include_bytes!("../../icons/native/tag.svg"))),
            "icons/copy.svg" => Some(Cow::Borrowed(include_bytes!("../../icons/native/copy.svg"))),
            _ => None,
        })
    }
    fn list(&self, _: &str) -> Result<Vec<SharedString>> {
        Ok(vec!["gitcrab.png".into()])
    }
}
