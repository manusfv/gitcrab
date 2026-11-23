//! The Clone and Create repository dialogs.
//!
//! Both ask where the repository goes, a parent folder and a folder name, and
//! Clone also asks for the URL. Git runs off the UI thread; once the
//! repository exists the dialog closes and hands its folder to the caller,
//! which opens it.
use crate::components::{
    self,
    modal::{self, field},
    prelude::*,
    ButtonKind,
};
use crate::theme::Role;
use crate::theme::*;
use gitcrab_core::git::{
    clone_folder_name, validate_branch_name, validate_clone_url, validate_folder_name,
    DEFAULT_BRANCH,
};
use gitcrab_core::presentation::display_path;
use gpui::component::{
    button::Button,
    input::{Input, InputEvent, InputState},
    WindowExt,
};
use gpui::{prelude::*, *};
use std::{
    path::PathBuf,
    rc::Rc,
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc,
    },
};

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Setup {
    Clone,
    Create,
}

impl Setup {
    fn title(self) -> &'static str {
        match self {
            Self::Clone => "Clone a repository",
            Self::Create => "Create a repository",
        }
    }
    fn description(self) -> &'static str {
        match self {
            Self::Clone => "Copy a remote repository to this computer and open it.",
            Self::Create => "Start a new Git repository with a README and open it.",
        }
    }
    fn action(self) -> &'static str {
        match self {
            Self::Clone => "Clone",
            Self::Create => "Create",
        }
    }
    fn destination(self) -> &'static str {
        match self {
            Self::Clone => "Clones into",
            Self::Create => "Creates",
        }
    }
    fn progress(self) -> &'static str {
        match self {
            Self::Clone => "Cloning…",
            Self::Create => "Creating…",
        }
    }
}

/// Receives the new repository's folder once it exists.
pub type OnCreated = Rc<dyn Fn(PathBuf, &mut Window, &mut App)>;

struct SetupForm {
    kind: Setup,
    url: Entity<InputState>,
    name: Entity<InputState>,
    parent: Entity<InputState>,
    /// The branch a new repository starts on, for Create.
    branch: Entity<InputState>,
    /// Whether the user typed a name, which stops Clone deriving it from the URL.
    name_edited: bool,
    /// Set while Git runs; storing `true` stops it.
    running: Option<Arc<AtomicBool>>,
    error: Option<String>,
    on_created: OnCreated,
    _subscriptions: Vec<Subscription>,
}

impl SetupForm {
    fn new(
        kind: Setup,
        parent: String,
        on_created: OnCreated,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let url = cx.new(|cx| {
            InputState::new(window, cx).placeholder("https://github.com/owner/repository.git")
        });
        let name = cx.new(|cx| InputState::new(window, cx).placeholder("my-project"));
        let parent = cx.new(|cx| InputState::new(window, cx).default_value(parent));
        let branch = cx.new(|cx| InputState::new(window, cx).default_value(DEFAULT_BRANCH));
        let subscriptions = vec![
            cx.subscribe_in(&url, window, |this, url, event: &InputEvent, window, cx| {
                if let InputEvent::Change = event {
                    if !this.name_edited {
                        let derived = clone_folder_name(&url.read(cx).value()).unwrap_or_default();
                        this.name
                            .update(cx, |name, cx| name.set_value(derived, window, cx));
                    }
                }
                this.input_event(event, window, cx);
            }),
            cx.subscribe_in(
                &name,
                window,
                |this, name, event: &InputEvent, window, cx| {
                    if let InputEvent::Change = event {
                        this.name_edited = !name.read(cx).value().is_empty();
                    }
                    this.input_event(event, window, cx);
                },
            ),
            cx.subscribe_in(
                &parent,
                window,
                |this, _, event: &InputEvent, window, cx| {
                    this.input_event(event, window, cx);
                },
            ),
            cx.subscribe_in(
                &branch,
                window,
                |this, _, event: &InputEvent, window, cx| {
                    this.input_event(event, window, cx);
                },
            ),
        ];
        Self {
            kind,
            url,
            name,
            parent,
            branch,
            name_edited: false,
            running: None,
            error: None,
            on_created,
            _subscriptions: subscriptions,
        }
    }

    fn input_event(&mut self, event: &InputEvent, window: &mut Window, cx: &mut Context<Self>) {
        match event {
            InputEvent::Change => {
                self.error = None;
                cx.notify();
            }
            InputEvent::PressEnter {
                secondary: false, ..
            } => self.submit(window, cx),
            _ => {}
        }
    }

    /// Whether every field the dialog needs has text, so it can be submitted.
    fn filled(&self, cx: &App) -> bool {
        let has = |input: &Entity<InputState>| !input.read(cx).value().trim().is_empty();
        let source = match self.kind {
            Setup::Clone => &self.url,
            Setup::Create => &self.branch,
        };
        has(source) && has(&self.name) && has(&self.parent)
    }

    fn destination(&self, cx: &App) -> Option<PathBuf> {
        let parent = self.parent.read(cx).value();
        let name = self.name.read(cx).value();
        (!parent.trim().is_empty() && !name.trim().is_empty())
            .then(|| PathBuf::from(parent.trim()).join(name.as_ref()))
    }

    fn validated(&self, cx: &App) -> Result<PathBuf, &'static str> {
        if self.kind == Setup::Clone {
            validate_clone_url(&self.url.read(cx).value())?;
        }
        if self.parent.read(cx).value().trim().is_empty() {
            return Err("Choose the folder to put the repository in.");
        }
        validate_folder_name(&self.name.read(cx).value())?;
        if self.kind == Setup::Create {
            validate_branch_name(self.branch.read(cx).value().trim())?;
        }
        self.destination(cx)
            .ok_or("Choose the folder to put the repository in.")
    }

    fn submit(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.running.is_some() {
            return;
        }
        let destination = match self.validated(cx) {
            Ok(destination) => destination,
            Err(error) => {
                self.error = Some(error.into());
                cx.notify();
                return;
            }
        };
        let cancelled = Arc::new(AtomicBool::new(false));
        self.running = Some(cancelled.clone());
        self.error = None;
        let backend = crate::backend::get(cx);
        let kind = self.kind;
        let url = self.url.read(cx).value().to_string();
        let branch = self.branch.read(cx).value().trim().to_owned();
        let target = destination.clone();
        let task = cx.background_executor().spawn(async move {
            match kind {
                Setup::Clone => backend.clone_repository(&url, &target, cancelled),
                Setup::Create => backend.init(&target, &branch),
            }
        });
        cx.spawn_in(window, async move |form, cx| {
            let result = task.await;
            let _ = form.update_in(cx, |this, window, cx| {
                let stopped = this
                    .running
                    .take()
                    .is_some_and(|cancelled| cancelled.load(Ordering::Acquire));
                // A stopped run's dialog has already closed.
                if stopped {
                    return;
                }
                match result {
                    Ok(()) => {
                        window.close_dialog(cx);
                        (this.on_created)(destination, window, cx);
                    }
                    Err(error) => this.error = Some(error),
                }
                cx.notify();
            });
        })
        .detach();
        cx.notify();
    }

    fn stop(&mut self) {
        if let Some(cancelled) = &self.running {
            cancelled.store(true, Ordering::Release);
        }
    }

    fn choose_parent(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let rx = cx.prompt_for_paths(PathPromptOptions {
            files: false,
            directories: true,
            multiple: false,
            prompt: Some("Choose folder".into()),
        });
        cx.spawn_in(window, async move |form, cx| {
            let Ok(Ok(Some(paths))) = rx.await else {
                return;
            };
            let Some(path) = paths.into_iter().next() else {
                return;
            };
            let _ = form.update_in(cx, |this, window, cx| {
                let text = path.display().to_string();
                this.parent
                    .update(cx, |parent, cx| parent.set_value(text, window, cx));
                this.error = None;
                cx.notify();
            });
        })
        .detach();
    }
}

/// Opens the Clone or Create dialog with `parent` as the suggested parent
/// folder. `on_created` runs with the repository's folder once it exists.
pub fn open(kind: Setup, parent: String, on_created: OnCreated, window: &mut Window, cx: &mut App) {
    let form = cx.new(|cx| SetupForm::new(kind, parent, on_created, window, cx));
    let scroll = ScrollHandle::new();
    let first = {
        let form = form.read(cx);
        match kind {
            Setup::Clone => form.url.clone(),
            Setup::Create => form.name.clone(),
        }
    };
    window.open_modal(cx, move |dialog, _, cx| {
        let state = form.read(cx);
        let running = state.running.is_some();
        let ready = state.filled(cx) && !running;
        let (url, name, parent, branch) = (
            state.url.clone(),
            state.name.clone(),
            state.parent.clone(),
            state.branch.clone(),
        );
        let error = state.error.clone();
        let preview = state
            .destination(cx)
            .map(|path| display_path(&path.display().to_string()));
        let choose = form.clone();
        let submit = form.clone();
        let cancel = form.clone();
        let confirm = form.clone();
        let closed = form.clone();
        let body = modal_body()
            .when(kind == Setup::Clone, |body| {
                body.child(field("Repository URL", Input::new(&url).disabled(running)))
            })
            .child(field(
                "Location",
                div()
                    .w_full()
                    .flex()
                    .items_center()
                    .gap_2()
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .child(Input::new(&parent).disabled(running)),
                    )
                    .child(
                        components::style(
                            Button::new("setup-choose-parent")
                                .label("Choose…")
                                .tab_stop(true),
                            ButtonKind::Secondary,
                            ControlSize::Regular,
                            cx,
                        )
                        .set_disabled(running)
                        .on_click(move |_, window, cx| {
                            choose.update(cx, |form, cx| form.choose_parent(window, cx))
                        }),
                    ),
            ))
            .child(field("Name", Input::new(&name).disabled(running)))
            .when(kind == Setup::Create, |body| {
                body.child(field(
                    "Default branch",
                    Input::new(&branch).disabled(running),
                ))
            })
            .when_some(preview, |body, path| {
                body.child(field(kind.destination(), modal::subject(path, None)))
            })
            .when_some(error, |body, error| {
                body.child(modal::callout(Tone::Warning, error))
            });
        let footer = ModalFooter::new()
            .when(running, |footer| {
                footer.leading(
                    div()
                        .text_size(scaled(TEXT_SIZE_SMALL))
                        .text_role(Role::TextMuted)
                        .child(kind.progress()),
                )
            })
            .action(components::dialog_cancel(cx))
            .action(
                components::style(
                    Button::new("setup-submit")
                        .label(kind.action())
                        .tab_stop(true),
                    ButtonKind::Primary,
                    ControlSize::Regular,
                    cx,
                )
                .set_disabled(!ready)
                .on_click(move |_, window, cx| {
                    submit.update(cx, |form, cx| form.submit(window, cx))
                }),
            );
        dialog
            .frame(
                ModalHeader::new(kind.title()).description(kind.description()),
                ModalWidth::Medium,
                cx,
            )
            .body(&scroll, body)
            .footer(footer)
            // Enter submits the form; the dialog closes itself once Git succeeds.
            .on_ok(move |_, window, cx| {
                confirm.update(cx, |form, cx| form.submit(window, cx));
                false
            })
            // While Git runs only Cancel stops it, so a stray click or
            // Escape does not throw away a clone in progress.
            .on_cancel(move |_, _, cx| cancel.read(cx).running.is_none())
            .on_close(move |_, _, cx| closed.update(cx, |form, _| form.stop()))
    });
    window.on_next_frame(move |window, cx| first.update(cx, |input, cx| input.focus(window, cx)));
}
