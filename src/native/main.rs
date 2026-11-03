mod assets;
mod preferences;
mod theme;
#[cfg(feature = "native-evidence")]
mod trace;
mod view;
mod watch;
mod workspace;
use gpui::{prelude::*, *};
use gpui_component::{Root, Theme, ThemeMode};
fn main() {
    Application::new().with_assets(assets::Assets).run(|cx| {
        gpui_component::init(cx);
        Theme::change(ThemeMode::Dark, None, cx);
        theme::install(cx);
        cx.on_action(|_: &view::Quit, cx| cx.quit());
        cx.set_menus(vec![Menu {
            name: "GitCrab".into(),
            items: vec![
                MenuItem::action("Open repository…", view::Open),
                MenuItem::action("New tab", workspace::NewTab),
                MenuItem::action("Close tab", workspace::CloseTab),
                MenuItem::action("Refresh", view::Refresh),
                MenuItem::separator(),
                MenuItem::action("Quit", view::Quit),
            ],
        }]);
        let primary = if cfg!(target_os = "macos") {
            "cmd"
        } else {
            "ctrl"
        };
        cx.bind_keys([
            KeyBinding::new(&format!("{primary}-o"), view::Open, None),
            KeyBinding::new(&format!("{primary}-t"), workspace::NewTab, None),
            KeyBinding::new(&format!("{primary}-w"), workspace::CloseTab, None),
            KeyBinding::new("ctrl-tab", workspace::NextTab, None),
            KeyBinding::new("ctrl-shift-tab", workspace::PreviousTab, None),
            KeyBinding::new(&format!("{primary}-q"), view::Quit, None),
            KeyBinding::new(&format!("{primary}-enter"), view::CommitStaged, None),
            KeyBinding::new(&format!("{primary}-f"), view::Find, None),
            KeyBinding::new("f3", view::SearchNext, None),
            KeyBinding::new("shift-f3", view::SearchPrevious, None),
            KeyBinding::new("f5", view::Refresh, None),
            KeyBinding::new("alt-left", view::SplitBefore, Some("GitCrab")),
            KeyBinding::new("alt-right", view::SplitAfter, Some("GitCrab")),
            KeyBinding::new("escape", view::Back, Some("GitCrab")),
            KeyBinding::new("up", view::Previous, Some("GitCrab")),
            KeyBinding::new("down", view::Next, Some("GitCrab")),
            KeyBinding::new(&format!("{primary}-c"), view::Copy, Some("GitCrab")),
            KeyBinding::new(&format!("{primary}-a"), view::SelectAll, Some("GitCrab")),
        ]);
        #[cfg(not(feature = "native-evidence"))]
        let viewport = size(px(1280.), px(800.));
        #[cfg(feature = "native-evidence")]
        let viewport = size(
            px(std::env::var("GITCRAB_CAPTURE_WIDTH")
                .ok()
                .and_then(|s| s.parse::<f32>().ok())
                .filter(|n| n.is_finite())
                .unwrap_or(1280.)
                .clamp(960., 3840.)),
            px(std::env::var("GITCRAB_CAPTURE_HEIGHT")
                .ok()
                .and_then(|s| s.parse::<f32>().ok())
                .filter(|n| n.is_finite())
                .unwrap_or(800.)
                .clamp(600., 2160.)),
        );
        let bounds = Bounds::centered(None, viewport, cx);
        cx.open_window(
            WindowOptions {
                window_bounds: Some(WindowBounds::Windowed(bounds)),
                window_min_size: Some(size(px(960.), px(600.))),
                titlebar: Some(TitlebarOptions {
                    title: Some("GitCrab".into()),
                    ..Default::default()
                }),
                ..Default::default()
            },
            |window, cx| {
                let view = cx.new(|cx| workspace::Workspace::new(window, cx));
                cx.new(|cx| Root::new(view, window, cx))
            },
        )
        .expect("open native GitCrab window");
        cx.on_window_closed(|cx| {
            if cx.windows().is_empty() {
                cx.quit();
            }
        })
        .detach();
        cx.activate(true);
    });
}
