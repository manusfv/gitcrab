use gpui::{AssetSource, Result, SharedString};
use std::borrow::Cow;

const LOGO_LIGHT: &str = "gitcrab-light.svg";
const LOGO_DARK: &str = "gitcrab-dark.svg";

pub fn logo(on_light_surface: bool) -> gpui::Img {
    gpui::img(gpui::ImageSource::Resource(gpui::Resource::Embedded(
        if on_light_surface {
            LOGO_LIGHT
        } else {
            LOGO_DARK
        }
        .into(),
    )))
}

pub struct Assets;
impl AssetSource for Assets {
    fn load(&self, path: &str) -> Result<Option<Cow<'static, [u8]>>> {
        Ok(match path {
            LOGO_LIGHT => Some(Cow::Borrowed(include_bytes!(
                "../../icons/gitcrab-light.svg"
            ))),
            LOGO_DARK => Some(Cow::Borrowed(include_bytes!(
                "../../icons/gitcrab-dark.svg"
            ))),
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
            "icons/plus.svg" => Some(Cow::Borrowed(include_bytes!("../../icons/native/plus.svg"))),
            "icons/minus.svg" => Some(Cow::Borrowed(include_bytes!(
                "../../icons/native/minus.svg"
            ))),
            "icons/up.svg" => Some(Cow::Borrowed(include_bytes!("../../icons/native/up.svg"))),
            "icons/down.svg" => Some(Cow::Borrowed(include_bytes!("../../icons/native/down.svg"))),
            "icons/back.svg" => Some(Cow::Borrowed(include_bytes!("../../icons/native/back.svg"))),
            "icons/right.svg" => Some(Cow::Borrowed(include_bytes!(
                "../../icons/native/right.svg"
            ))),
            "icons/remote.svg" => Some(Cow::Borrowed(include_bytes!(
                "../../icons/native/remote.svg"
            ))),
            "icons/fetch.svg" => Some(Cow::Borrowed(include_bytes!(
                "../../icons/native/fetch.svg"
            ))),
            "icons/pull.svg" => Some(Cow::Borrowed(include_bytes!("../../icons/native/pull.svg"))),
            "icons/push.svg" => Some(Cow::Borrowed(include_bytes!("../../icons/native/push.svg"))),
            "icons/pop.svg" => Some(Cow::Borrowed(include_bytes!("../../icons/native/pop.svg"))),
            "icons/terminal.svg" => Some(Cow::Borrowed(include_bytes!(
                "../../icons/native/terminal.svg"
            ))),
            "icons/more.svg" => Some(Cow::Borrowed(include_bytes!("../../icons/native/more.svg"))),
            "icons/branch-plus.svg" => Some(Cow::Borrowed(include_bytes!(
                "../../icons/native/branch-plus.svg"
            ))),
            "icons/folder-git-2.svg" => Some(Cow::Borrowed(include_bytes!(
                "../../icons/native/folder-git-2.svg"
            ))),
            _ => None,
        })
    }
    fn list(&self, _: &str) -> Result<Vec<SharedString>> {
        Ok(vec![LOGO_LIGHT.into(), LOGO_DARK.into()])
    }
}
