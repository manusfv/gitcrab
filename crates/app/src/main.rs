#![cfg_attr(
    all(target_os = "windows", not(debug_assertions)),
    windows_subsystem = "windows"
)]

use gitcrab_ui::{assets, preferences, theme, view, workspace};
use gpui::{prelude::*, *};
use gpui_component::{Root, Theme, ThemeMode};
fn main() {
    if std::env::args_os().nth(1).as_deref() == Some(std::ffi::OsStr::new("--version")) {
        println!(
            "GitCrab {} · {}",
            env!("CARGO_PKG_VERSION"),
            gitcrab_ui::BUILD
        );
        return;
    }
    Application::new().with_assets(assets::Assets).run(|cx| {
        gpui_component::init(cx);
        gitcrab_ui::backend::install(std::sync::Arc::new(gitcrab_git::Backend), cx);
        Theme::change(ThemeMode::Dark, None, cx);
        theme::initialize(cx);
        cx.set_menus(vec![Menu {
            name: "GitCrab".into(),
            items: vec![
                MenuItem::action("Open repository…", view::Open),
                MenuItem::action("New tab", workspace::NewTab),
                MenuItem::action("Close tab", workspace::CloseTab),
                MenuItem::action("Move tab left", workspace::MoveTabLeft),
                MenuItem::action("Move tab right", workspace::MoveTabRight),
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
            KeyBinding::new("ctrl-shift-pageup", workspace::MoveTabLeft, None),
            KeyBinding::new("ctrl-shift-pagedown", workspace::MoveTabRight, None),
            KeyBinding::new(&format!("{primary}-q"), view::Quit, None),
            KeyBinding::new(&format!("{primary}-enter"), view::CommitStaged, None),
            KeyBinding::new(&format!("{primary}-f"), view::Find, None),
            KeyBinding::new(&format!("{primary}-shift-f"), view::FindInHistory, None),
            KeyBinding::new("f3", view::SearchNext, None),
            KeyBinding::new("shift-f3", view::SearchPrevious, None),
            KeyBinding::new("f5", view::Refresh, None),
            KeyBinding::new("alt-left", view::SplitBefore, Some("GitCrab")),
            KeyBinding::new("alt-right", view::SplitAfter, Some("GitCrab")),
            KeyBinding::new("alt-s", view::StageSelectedFile, Some("GitCrab")),
            KeyBinding::new("alt-up", view::PreviousHunk, Some("GitCrab")),
            KeyBinding::new("alt-down", view::NextHunk, Some("GitCrab")),
            KeyBinding::new("escape", view::Back, Some("GitCrab")),
            KeyBinding::new("up", view::Previous, Some("GitCrab")),
            KeyBinding::new("down", view::Next, Some("GitCrab")),
            KeyBinding::new(&format!("{primary}-c"), view::Copy, Some("GitCrab")),
            KeyBinding::new(
                &format!("{primary}-shift-c"),
                view::CopyPatch,
                Some("GitCrab"),
            ),
            KeyBinding::new(&format!("{primary}-a"), view::SelectAll, Some("GitCrab")),
            // Text fields bind their own editing undo with higher precedence;
            // the view also ignores these while a text field has focus.
            KeyBinding::new(&format!("{primary}-z"), view::UndoLast, Some("GitCrab")),
            KeyBinding::new(
                &format!("{primary}-shift-z"),
                view::RedoLast,
                Some("GitCrab"),
            ),
            KeyBinding::new("ctrl-y", view::RedoLast, Some("GitCrab")),
        ]);
        #[cfg(not(feature = "native-evidence"))]
        let saved = preferences::Preferences::current(cx).window;
        #[cfg(not(feature = "native-evidence"))]
        let viewport = {
            let (width, height) = saved.map_or((1280., 800.), |s| (s.width, s.height));
            // A size saved on a larger display still fits on this one.
            let display = cx.primary_display().map(|d| d.bounds().size);
            size(
                display.map_or(px(width), |d| px(width).min(d.width)),
                display.map_or(px(height), |d| px(height).min(d.height)),
            )
        };
        #[cfg(feature = "native-evidence")]
        let saved: Option<preferences::WindowSize> = None;
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
                window_bounds: Some(if saved.is_some_and(|s| s.maximized) {
                    WindowBounds::Maximized(bounds)
                } else {
                    WindowBounds::Windowed(bounds)
                }),
                window_min_size: Some(size(
                    px(preferences::WindowSize::MIN_WIDTH),
                    px(preferences::WindowSize::MIN_HEIGHT),
                )),
                titlebar: Some(TitlebarOptions {
                    title: Some("GitCrab".into()),
                    ..Default::default()
                }),
                ..Default::default()
            },
            |window, cx| {
                let view = cx.new(|cx| workspace::Workspace::new(window, cx));
                let workspace = view.downgrade();
                window.on_window_should_close(cx, move |w, cx| {
                    workspace
                        .update(cx, |v, cx| v.should_close_window(w, cx))
                        .unwrap_or(true)
                });
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
