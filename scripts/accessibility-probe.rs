//! Bounded F02 specimen, deliberately separate from the production host.
use gpui::{prelude::*, *};
use gpui_component::{
    button::Button,
    input::{Input, InputState},
    Disableable, Root, Selectable, WindowExt,
};
struct Probe {
    input: Entity<InputState>,
}
impl Render for Probe {
    fn render(&mut self, w: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let dialogs = Root::render_dialog_layer(w, cx);
        div()
            .size_full()
            .p_4()
            .flex()
            .flex_col()
            .gap_3()
            .child("F02 native semantics probe")
            .child(
                Button::new("enabled")
                    .label("Open repository")
                    .on_click(|_, w, cx| {
                        w.open_dialog(cx, |d, _, _| {
                            d.title("Review operation").confirm().child("Modal content")
                        });
                    }),
            )
            .child(Button::new("disabled").label("Stage file").disabled(true))
            .child(Button::new("selected").label("Split").selected(true))
            .child(Button::new("busy").label("Committing").loading(true))
            .child(Input::new(&self.input))
            .child(
                div()
                    .id("files")
                    .child("File list")
                    .child("Selected: archive.txt")
                    .child("notes.txt"),
            )
            .children(dialogs)
    }
}
fn main() {
    Application::new().run(|cx| {
        gpui_component::init(cx);
        cx.open_window(
            WindowOptions {
                titlebar: Some(TitlebarOptions {
                    title: Some("GitCrab accessibility probe".into()),
                    ..Default::default()
                }),
                ..Default::default()
            },
            |w, cx| {
                let input = cx.new(|cx| {
                    InputState::new(w, cx)
                        .placeholder("Commit summary")
                        .default_value("Rename the archive")
                });
                let probe = cx.new(|_| Probe { input });
                cx.new(|cx| Root::new(probe, w, cx))
            },
        )
        .unwrap();
        cx.activate(true);
    });
}
