use crate::{
    git::{Git, Hunk, RepoSnapshot},
    graph,
};
use eframe::egui::{self, *};
use std::{
    sync::mpsc::{self, Receiver, Sender},
    time::{Duration, Instant},
};

const BG: Color32 = Color32::from_rgb(28, 30, 35);
const PANEL: Color32 = Color32::from_rgb(38, 41, 49);
const LINE: Color32 = Color32::from_rgb(53, 57, 66);
const TEXT: Color32 = Color32::from_rgb(202, 205, 211);
const MUTED: Color32 = Color32::from_rgb(137, 143, 155);
const ACCENT: Color32 = Color32::from_rgb(77, 190, 210);
const COLORS: [Color32; 10] = [
    Color32::from_rgb(21, 160, 191),
    Color32::from_rgb(6, 105, 247),
    Color32::from_rgb(142, 0, 194),
    Color32::from_rgb(197, 23, 182),
    Color32::from_rgb(217, 1, 113),
    Color32::from_rgb(205, 1, 1),
    Color32::from_rgb(242, 93, 46),
    Color32::from_rgb(242, 202, 51),
    Color32::from_rgb(123, 217, 56),
    Color32::from_rgb(46, 206, 157),
];
const ROW: f32 = 28.0;

fn toolbar_button(ui: &mut Ui, label: &str, enabled: bool) -> egui::Response {
    let (rect, response) = ui.allocate_exact_size(
        vec2(58.0, 42.0),
        if enabled {
            Sense::click()
        } else {
            Sense::hover()
        },
    );
    let p = ui.painter();
    let color = if enabled {
        TEXT
    } else {
        MUTED.gamma_multiply(0.4)
    };
    let stroke = Stroke::new(1.5_f32, color);
    if response.hovered() && enabled {
        p.rect_filled(rect, 2.0, Color32::from_rgb(49, 53, 62));
    }
    p.text(
        pos2(rect.center().x, rect.top() + 5.0),
        Align2::CENTER_CENTER,
        label,
        FontId::proportional(11.0),
        color,
    );
    let c = pos2(rect.center().x, rect.top() + 27.0);
    match label {
        "Branch" | "Merge" => {
            let d = if label == "Merge" { -1.0 } else { 1.0 };
            p.line_segment([c + vec2(-4.0, -7.0), c + vec2(-4.0, 7.0)], stroke);
            p.add(Shape::line(
                vec![
                    c + vec2(-4.0, d * 3.0),
                    c + vec2(5.0, -d * 2.0),
                    c + vec2(5.0, -d * 7.0),
                ],
                stroke,
            ));
            for v in [vec2(-4.0, -7.0), vec2(-4.0, 7.0), vec2(5.0, -d * 7.0)] {
                p.circle_filled(c + v, 2.0, PANEL);
                p.circle_stroke(c + v, 2.0, stroke);
            }
        }
        "Checkout" => {
            for d in [-1.0, 1.0] {
                let y = d * 3.0;
                p.line_segment([c + vec2(-7.0, y), c + vec2(7.0, y)], stroke);
                p.add(Shape::line(
                    vec![
                        c + vec2(d * 3.0, y - 3.0),
                        c + vec2(d * 7.0, y),
                        c + vec2(d * 3.0, y + 3.0),
                    ],
                    stroke,
                ));
            }
        }
        "Refresh" => {
            let points = (0..19)
                .map(|i| {
                    let a = (i as f32 * 16.0 - 120.0).to_radians();
                    c + vec2(a.cos() * 7.0, a.sin() * 7.0)
                })
                .collect();
            p.add(Shape::line(points, stroke));
            p.add(Shape::line(
                vec![
                    c + vec2(-4.0, -10.0),
                    c + vec2(-3.0, -5.0),
                    c + vec2(2.0, -7.0),
                ],
                stroke,
            ));
        }
        _ => {
            p.rect_stroke(
                Rect::from_center_size(c, vec2(17.0, 11.0)),
                1.0,
                stroke,
                StrokeKind::Inside,
            );
            p.line_segment([c + vec2(-3.0, 0.0), c + vec2(3.0, 0.0)], stroke);
        }
    }
    response
}

fn paint_code(p: &Painter, pos: Pos2, text: &str, base: Color32) {
    let mut job = egui::text::LayoutJob::default();
    let mut i = 0;
    while i < text.len() {
        let rest = &text[i..];
        let ch = rest.chars().next().unwrap();
        let (len, color) = if rest.starts_with("//") || (i <= 1 && rest.starts_with('#')) {
            (rest.len(), Color32::from_rgb(117, 157, 96))
        } else if ch == '"' || ch == '\'' {
            let mut end = ch.len_utf8();
            let mut escaped = false;
            for (offset, c) in rest[ch.len_utf8()..].char_indices() {
                end = ch.len_utf8() + offset + c.len_utf8();
                if c == ch && !escaped {
                    break;
                }
                escaped = c == '\\' && !escaped;
            }
            (end, Color32::from_rgb(181, 191, 132))
        } else if ch.is_alphabetic() || ch == '_' {
            let len = rest
                .char_indices()
                .take_while(|(_, c)| c.is_alphanumeric() || *c == '_')
                .map(|(i, c)| i + c.len_utf8())
                .last()
                .unwrap();
            let keyword = matches!(
                &rest[..len],
                "fn" | "pub"
                    | "let"
                    | "mut"
                    | "impl"
                    | "struct"
                    | "enum"
                    | "use"
                    | "mod"
                    | "if"
                    | "else"
                    | "match"
                    | "return"
                    | "const"
                    | "async"
                    | "await"
                    | "function"
                    | "class"
                    | "def"
                    | "import"
                    | "from"
                    | "true"
                    | "false"
                    | "None"
                    | "Some"
            );
            (
                len,
                if keyword {
                    Color32::from_rgb(181, 137, 202)
                } else {
                    base
                },
            )
        } else if ch.is_ascii_digit() {
            let len = rest
                .bytes()
                .take_while(|c| c.is_ascii_digit() || *c == b'.')
                .count();
            (len, Color32::from_rgb(114, 174, 200))
        } else {
            (ch.len_utf8(), base)
        };
        job.append(
            &rest[..len],
            0.0,
            TextFormat {
                font_id: FontId::monospace(12.0),
                color,
                ..Default::default()
            },
        );
        i += len;
    }
    let galley = p.layout_job(job);
    p.galley(pos - vec2(0.0, 8.0), galley, base);
}

enum Job {
    Open(String, usize),
    Refresh(usize),
    Select(String),
    Diff(Option<String>, String, bool),
    Stage(String, bool),
    Hunk(Hunk, bool),
    Commit(String),
    Operation(String, String),
}
struct Request {
    epoch: u64,
    job: Job,
}
struct Response {
    epoch: u64,
    is_refresh: bool,
    result: Result<Reply, Failure>,
}
struct Failure {
    message: String,
    refresh: bool,
}
enum Reply {
    Snapshot(String, RepoSnapshot, graph::Graph),
    Details(String, Vec<String>),
    Diff(
        Option<String>,
        String,
        bool,
        Vec<crate::diff::Line>,
        Vec<(Option<crate::diff::Line>, Option<crate::diff::Line>)>,
        Vec<Hunk>,
    ),
    Done(String, Option<String>),
}

fn worker(ctx: Context, rx: Receiver<Request>, tx: Sender<Response>) {
    let mut repo: Option<Git> = None;
    for Request { epoch, job } in rx {
        // Read failures must not trigger another refresh; mutation failures can
        // still have changed Git state (a merge may have entered a conflict).
        let refresh_on_error = matches!(
            &job,
            Job::Stage(..) | Job::Hunk(..) | Job::Commit(..) | Job::Operation(..)
        );
        let is_refresh = matches!(&job, Job::Refresh(..));
        let result: Result<Reply, String> = (|| {
            if let Job::Open(path, limit) = job {
                let g = Git::open(path)?;
                let snap = g.snapshot(limit)?;
                let path = g.path.to_string_lossy().into_owned();
                repo = Some(g);
                let graph = graph::layout_with_head(
                    &snap.commits,
                    (!snap.head_id.is_empty()).then_some(snap.head_id.as_str()),
                );
                return Ok(Reply::Snapshot(path, snap, graph));
            }
            let g = repo.as_ref().ok_or("Open a local repository first")?;
            Ok(match job {
                Job::Refresh(n) => {
                    let snap = g.snapshot(n)?;
                    let graph = graph::layout_with_head(
                        &snap.commits,
                        (!snap.head_id.is_empty()).then_some(snap.head_id.as_str()),
                    );
                    Reply::Snapshot(g.path.to_string_lossy().into_owned(), snap, graph)
                }
                Job::Select(id) => Reply::Details(id.clone(), g.changed_files(&id)?),
                Job::Diff(id, path, staged) => {
                    let diff = g.diff(id.as_deref(), &path, staged)?;
                    let hunks = if id.is_none() {
                        crate::git::split_hunks(&diff)
                    } else {
                        vec![]
                    };
                    let mut lines = crate::diff::parse(&diff);
                    if let Some(first) = lines.iter().position(|l| l.text.starts_with("@@ ")) {
                        lines.drain(..first);
                    }
                    let split = crate::diff::split(&lines);
                    Reply::Diff(id, path, staged, lines, split, hunks)
                }
                Job::Stage(path, unstage) => {
                    if unstage {
                        g.unstage(&path)?
                    } else {
                        g.stage(&path)?
                    };
                    Reply::Done(
                        if unstage {
                            "Unstaged file"
                        } else {
                            "Staged file"
                        }
                        .into(),
                        None,
                    )
                }
                Job::Hunk(h, unstage) => {
                    g.apply_hunk(&h, unstage)?;
                    Reply::Done("Updated index hunk".into(), None)
                }
                Job::Commit(message) => {
                    g.commit(&message)?;
                    Reply::Done("Commit created".into(), Some(message))
                }
                Job::Operation(op, target) => {
                    match op.as_str() {
                        "Create branch" => g.create_branch(&target)?,
                        "Checkout" => g.checkout(&target)?,
                        "Merge" => g.merge(&target)?,
                        "Cherry-pick" => g.cherry_pick(&target)?,
                        "Revert" => g.revert(&target)?,
                        "Stash" => g.stash_with_message(&target)?,
                        "Pop stash" => g.stash_pop()?,
                        _ => return Err("Unsupported operation".into()),
                    };
                    Reply::Done(format!("{op} completed"), None)
                }
                Job::Open(..) => unreachable!(),
            })
        })();
        if tx
            .send(Response {
                epoch,
                is_refresh,
                result: result.map_err(|message| Failure {
                    message,
                    refresh: refresh_on_error,
                }),
            })
            .is_err()
        {
            break;
        }
        ctx.request_repaint();
    }
}

pub struct GitCrab {
    tx: Sender<Request>,
    rx: Receiver<Response>,
    epoch: u64,
    opening: bool,
    refresh_pending: bool,
    path: String,
    open_path: String,
    snapshot: RepoSnapshot,
    graph: graph::Graph,
    selected: Option<usize>,
    files: Vec<String>,
    selected_file: String,
    diff: String,
    diff_lines: Vec<crate::diff::Line>,
    split_lines: Vec<(Option<crate::diff::Line>, Option<crate::diff::Line>)>,
    hunks: Vec<Hunk>,
    staged: bool,
    working: bool,
    busy: usize,
    notice: String,
    error: bool,
    search: String,
    message: String,
    limit: usize,
    open_dialog: bool,
    operation: Option<String>,
    target: String,
    side_by_side: bool,
    ignore_space: bool,
    last_load: Instant,
    capture: Option<String>,
    capture_requested: bool,
    startup_view: Option<String>,
    scroll_selection: bool,
}

impl GitCrab {
    pub fn new(cc: &eframe::CreationContext<'_>, path: String) -> Self {
        // Use the user's OS text fonts when present; no third-party font asset is bundled.
        let mut fonts = FontDefinitions::default();
        for (name, path) in crate::platform::font_files() {
            if let Ok(bytes) = std::fs::read(path) {
                fonts
                    .font_data
                    .insert(name.into(), FontData::from_owned(bytes).into());
                for family in [FontFamily::Proportional, FontFamily::Monospace] {
                    fonts.families.get_mut(&family).unwrap().push(name.into());
                }
            }
        }
        if fonts.font_data.contains_key("system-ui") {
            fonts
                .families
                .get_mut(&FontFamily::Proportional)
                .unwrap()
                .insert(0, "system-ui".into());
        }
        cc.egui_ctx.set_fonts(fonts);
        let mut style = (*cc.egui_ctx.style()).clone();
        style.visuals = Visuals::dark();
        style.visuals.panel_fill = BG;
        style.visuals.window_fill = PANEL;
        style.visuals.extreme_bg_color = BG;
        style.visuals.override_text_color = Some(TEXT);
        style.visuals.selection.bg_fill = Color32::from_rgb(45, 76, 91);
        style.visuals.selection.stroke = Stroke::new(1.0_f32, ACCENT);
        style.visuals.widgets.noninteractive.bg_stroke = Stroke::new(1.0_f32, LINE);
        style.visuals.widgets.inactive.weak_bg_fill = PANEL;
        style.visuals.widgets.inactive.bg_fill = PANEL;
        style.spacing.item_spacing = vec2(7.0, 5.0);
        style
            .text_styles
            .insert(TextStyle::Body, FontId::proportional(13.0));
        style
            .text_styles
            .insert(TextStyle::Button, FontId::proportional(13.0));
        cc.egui_ctx.set_style(style);
        let (tx, job_rx) = mpsc::channel();
        let (reply_tx, rx) = mpsc::channel();
        let ctx = cc.egui_ctx.clone();
        std::thread::spawn(move || worker(ctx, job_rx, reply_tx));
        let mut app = Self {
            tx,
            rx,
            epoch: 0,
            opening: false,
            refresh_pending: false,
            path: String::new(),
            open_path: path.clone(),
            snapshot: Default::default(),
            graph: Default::default(),
            selected: None,
            files: vec![],
            selected_file: String::new(),
            diff: String::new(),
            diff_lines: vec![],
            split_lines: vec![],
            hunks: vec![],
            staged: false,
            working: false,
            busy: 0,
            notice: "Open a repository to explore its history".into(),
            error: false,
            search: String::new(),
            message: String::new(),
            limit: 500,
            open_dialog: path.is_empty(),
            operation: None,
            target: String::new(),
            side_by_side: false,
            ignore_space: false,
            last_load: Instant::now(),
            capture: std::env::var("GITCRAB_SCREENSHOT").ok(),
            capture_requested: false,
            startup_view: std::env::var("GITCRAB_CAPTURE_VIEW").ok(),
            scroll_selection: false,
        };
        if !path.is_empty() {
            app.send(Job::Open(path, app.limit));
        }
        app
    }
    fn send(&mut self, job: Job) {
        if matches!(&job, Job::Open(..)) {
            self.epoch = self.epoch.wrapping_add(1);
            self.opening = true;
            self.refresh_pending = false;
            self.operation = None;
            self.target.clear();
        } else if self.opening {
            return;
        }
        if matches!(&job, Job::Refresh(..)) {
            if self.refresh_pending {
                return;
            }
            self.refresh_pending = true;
        }
        if self
            .tx
            .send(Request {
                epoch: self.epoch,
                job,
            })
            .is_ok()
        {
            self.busy += 1;
        } else {
            self.opening = false;
            self.refresh_pending = false;
            self.notice = "Git worker stopped. Reopen GitCrab to continue.".into();
            self.error = true;
        }
    }
    fn refresh(&mut self) {
        self.send(Job::Refresh(self.limit));
    }
    fn select(&mut self, index: usize) {
        if index >= self.snapshot.commits.len() {
            return;
        }
        if self.opening {
            return;
        }
        self.selected = Some(index);
        self.working = false;
        self.selected_file.clear();
        self.diff.clear();
        self.diff_lines.clear();
        self.split_lines.clear();
        self.hunks.clear();
        self.files.clear();
        self.send(Job::Select(self.snapshot.commits[index].id.clone()));
    }
    fn working(&mut self) {
        if self.opening {
            return;
        }
        self.working = true;
        self.selected = None;
        self.files.clear();
        self.selected_file.clear();
        self.diff.clear();
        self.diff_lines.clear();
        self.split_lines.clear();
        self.hunks.clear();
    }
    fn choose_file(&mut self, path: String, staged: bool) {
        if self.opening {
            return;
        }
        self.selected_file = path.clone();
        self.staged = staged;
        self.diff.clear();
        self.diff_lines.clear();
        self.split_lines.clear();
        self.hunks.clear();
        let id = if self.working {
            None
        } else {
            self.selected.map(|i| self.snapshot.commits[i].id.clone())
        };
        self.send(Job::Diff(id, path, staged));
    }
    fn op(&mut self, name: &str, target: String) {
        self.operation = Some(name.into());
        self.target = target;
    }
    fn receive(&mut self) {
        while let Ok(Response {
            epoch,
            is_refresh,
            result,
        }) = self.rx.try_recv()
        {
            self.busy = self.busy.saturating_sub(1);
            if epoch != self.epoch {
                continue;
            }
            if is_refresh {
                self.refresh_pending = false;
            }
            let reply = match result {
                Ok(reply) => reply,
                Err(Failure { message, refresh }) => {
                    self.opening = false;
                    self.notice = message;
                    self.error = true;
                    if refresh && !self.path.is_empty() {
                        self.refresh();
                    }
                    continue;
                }
            };
            match reply {
                Reply::Snapshot(path, snap, graph) => {
                    let opened = self.opening;
                    self.opening = false;
                    let changed_repo = self.path != path;
                    let selected_id = self
                        .selected
                        .and_then(|i| self.snapshot.commits.get(i))
                        .map(|c| c.id.clone());
                    self.path = path;
                    self.open_path = self.path.clone();
                    self.snapshot = snap;
                    self.graph = graph;
                    self.last_load = Instant::now();
                    if let Some(id) = selected_id {
                        self.selected = self.snapshot.commits.iter().position(|c| c.id == id);
                    }
                    if changed_repo {
                        self.working = false;
                        self.selected = None;
                        self.selected_file.clear();
                        self.diff.clear();
                        self.diff_lines.clear();
                        self.split_lines.clear();
                        self.hunks.clear();
                        self.files.clear();
                    }
                    if opened {
                        self.notice = format!(
                            "{} commits loaded · {} changed files",
                            self.snapshot.commits.len(),
                            self.snapshot.status.len()
                        );
                        self.error = false;
                    }
                    if self.selected.is_none() && !self.working && !self.snapshot.commits.is_empty()
                    {
                        let head = self
                            .snapshot
                            .commits
                            .iter()
                            .position(|c| c.refs.iter().any(|r| r == "HEAD"))
                            .unwrap_or(0);
                        self.select(head);
                    }
                    if self.working && !self.selected_file.is_empty() {
                        self.choose_file(self.selected_file.clone(), self.staged);
                    }
                    if opened
                        && self
                            .startup_view
                            .as_deref()
                            .is_some_and(|v| v == "staging" || v == "diff" || v == "split")
                    {
                        let view = self.startup_view.take().unwrap();
                        self.working();
                        if view != "staging" {
                            self.side_by_side = view == "split";
                            if let Some(s) = self
                                .snapshot
                                .status
                                .iter()
                                .find(|s| s.worktree != ' ')
                                .cloned()
                            {
                                self.choose_file(s.path, false);
                            }
                        }
                    }
                }
                Reply::Details(id, files) => {
                    if self
                        .selected
                        .and_then(|i| self.snapshot.commits.get(i))
                        .is_some_and(|c| c.id == id)
                    {
                        self.files = files;
                        if self.startup_view.as_deref() == Some("commitdiff") {
                            self.startup_view = None;
                            if let Some(path) = self.files.first().cloned() {
                                self.choose_file(path, false);
                            }
                        }
                    }
                }
                Reply::Diff(id, path, staged, lines, split, hunks) => {
                    let current = if self.working {
                        None
                    } else {
                        self.selected
                            .and_then(|i| self.snapshot.commits.get(i))
                            .map(|c| c.id.clone())
                    };
                    if id == current && path == self.selected_file && staged == self.staged {
                        self.diff_lines = lines;
                        self.split_lines = split;
                        self.hunks = hunks;
                    }
                }
                Reply::Done(message, committed_draft) => {
                    self.notice = message;
                    self.error = false;
                    if committed_draft
                        .as_ref()
                        .is_some_and(|draft| draft == &self.message)
                    {
                        self.message.clear();
                    }
                    self.refresh();
                }
            }
        }
    }
    fn top(&mut self, ctx: &Context) {
        TopBottomPanel::top("menu")
            .exact_height(26.0)
            .frame(
                Frame::new()
                    .fill(Color32::from_rgb(26, 28, 33))
                    .inner_margin(Margin::symmetric(10, 3)),
            )
            .show(ctx, |ui| {
                ui.horizontal(|ui| {
                    ui.label(RichText::new("◉  GitCrab").strong().color(ACCENT));
                    ui.menu_button("File", |ui| {
                        if ui
                            .button(format!(
                                "Open repository…   {}",
                                crate::platform::OPEN_SHORTCUT
                            ))
                            .clicked()
                        {
                            self.open_dialog = true;
                            ui.close_menu();
                        }
                        if ui.button("Refresh   F5").clicked() {
                            self.refresh();
                            ui.close_menu();
                        }
                    });
                    ui.menu_button("View", |ui| {
                        ui.checkbox(&mut self.side_by_side, "Side-by-side diff");
                        if ui.button("Working directory").clicked() {
                            self.working();
                            ui.close_menu();
                        }
                    });
                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        ui.label(RichText::new("LOCAL GIT CLIENT").size(10.0).color(MUTED));
                    });
                });
            });
        TopBottomPanel::top("tabs")
            .exact_height(36.0)
            .frame(
                Frame::new()
                    .fill(PANEL)
                    .inner_margin(Margin::symmetric(12, 6)),
            )
            .show(ctx, |ui| {
                ui.horizontal(|ui| {
                    if ui.button("▱").on_hover_text("Open repository").clicked() {
                        self.open_dialog = true;
                    }
                    ui.separator();
                    let name = std::path::Path::new(&self.path)
                        .file_name()
                        .unwrap_or_default()
                        .to_string_lossy();
                    ui.label(
                        RichText::new(if name.is_empty() { "Repository" } else { &name }).strong(),
                    );
                    ui.label(RichText::new("×").color(MUTED));
                    if ui.button("+").clicked() {
                        self.open_dialog = true;
                    }
                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        ui.label(RichText::new("Independent edition").color(MUTED).size(11.0));
                    });
                });
            });
        TopBottomPanel::top("toolbar")
            .exact_height(48.0)
            .frame(
                Frame::new()
                    .fill(PANEL)
                    .inner_margin(Margin::symmetric(18, 2)),
            )
            .show(ctx, |ui| {
                ui.horizontal(|ui| {
                    ui.vertical(|ui| {
                        ui.label(RichText::new("repository").size(10.0).color(MUTED));
                        let name = std::path::Path::new(&self.path)
                            .file_name()
                            .unwrap_or_default()
                            .to_string_lossy();
                        if ui
                            .button(if self.path.is_empty() {
                                "Open local repository  ▾"
                            } else {
                                &name
                            })
                            .on_hover_text(&self.path)
                            .clicked()
                        {
                            self.open_dialog = true;
                        }
                    });
                    ui.separator();
                    ui.vertical(|ui| {
                        ui.label(RichText::new("branch").size(10.0).color(MUTED));
                        ui.label(RichText::new(&self.snapshot.head).strong());
                    });
                    ui.add_space(18.0);
                    for label in ["Refresh", "Branch", "Checkout", "Merge", "Stash"] {
                        if toolbar_button(ui, label, self.busy == 0 && !self.path.is_empty())
                            .clicked()
                        {
                            match label {
                                "Refresh" => self.refresh(),
                                "Branch" => self.op("Create branch", String::new()),
                                "Checkout" => self.op("Checkout", String::new()),
                                "Merge" => self.op("Merge", String::new()),
                                _ => self.op("Stash", "Work in progress".into()),
                            }
                        }
                    }
                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        if self.busy > 0 {
                            ui.spinner();
                        }
                        ui.add(
                            TextEdit::singleline(&mut self.search)
                                .hint_text("Search history…")
                                .desired_width(165.0),
                        );
                    });
                });
            });
        TopBottomPanel::bottom("status")
            .exact_height(if self.error { 60.0 } else { 26.0 })
            .frame(
                Frame::new()
                    .fill(PANEL)
                    .inner_margin(Margin::symmetric(12, 4)),
            )
            .show(ctx, |ui| {
                ui.horizontal_wrapped(|ui| {
                    ui.colored_label(
                        if self.error {
                            Color32::LIGHT_RED
                        } else {
                            MUTED
                        },
                        &self.notice,
                    );
                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        ui.label(RichText::new("GitCrab  0.1.0").size(11.0).color(MUTED));
                    });
                });
            });
    }
    fn sidebar(&mut self, ctx: &Context) {
        if !self.selected_file.is_empty() {
            SidePanel::left("diffrail")
                .exact_width(38.0)
                .frame(Frame::new().fill(PANEL).inner_margin(5))
                .show(ctx, |ui| {
                    if ui.button("›").on_hover_text("Return to graph").clicked() {
                        self.selected_file.clear();
                    }
                    ui.add_space(12.0);
                    ui.label("⑂");
                    ui.small(format!("{}", self.snapshot.branches.len()));
                    ui.add_space(12.0);
                    ui.label("◇");
                });
            return;
        }
        SidePanel::left("branches").default_width(210.0).width_range(165.0..=360.0).resizable(true).frame(Frame::new().fill(PANEL).inner_margin(12)).show(ctx,|ui|{
            ui.set_min_width(ui.available_width());
            ui.horizontal(|ui|{ui.label(RichText::new("◉  Viewing all refs").color(MUTED).size(12.0));});ui.add_space(12.0);
            if ui.selectable_label(self.working,format!("◇  Working directory    {}",self.snapshot.status.len())).clicked(){self.working();}
            ui.add_space(14.0);ui.separator();
            ScrollArea::vertical().id_salt("branchscroll").show(ui,|ui|{
                ui.label(RichText::new(format!("⌄  LOCAL                 {}",self.snapshot.branches.len())).size(11.0).color(MUTED));ui.add_space(5.0);
                for branch in self.snapshot.branches.clone(){
                    let current=branch==self.snapshot.head;
                    let response=ui.selectable_label(current,RichText::new(format!("   ⑂  {branch}")).color(if current{ACCENT}else{TEXT}));
                    if response.double_clicked(){self.op("Checkout",branch.clone());}
                    response.context_menu(|ui|{for label in ["Checkout","Merge"]{if ui.button(label).clicked(){self.op(label,branch.clone());ui.close_menu();}}});
                }
                ui.add_space(18.0);ui.label(RichText::new("⌄  TAGS & REFERENCES").size(11.0).color(MUTED));ui.add_space(5.0);
                let refs:Vec<_>=self.snapshot.commits.iter().enumerate().flat_map(|(i,c)|c.refs.iter().filter(|r|*r!="HEAD"&&!self.snapshot.branches.contains(r)).map(move|r|(i,r.clone()))).collect();
                for (index,name) in refs {if ui.selectable_label(false,format!("   ◇  {name}")).clicked(){self.select(index);}}
                ui.add_space(18.0);ui.label(RichText::new("⌄  STASHES").size(11.0).color(MUTED));
                if ui.small_button("Pop latest stash…").clicked(){self.op("Pop stash",String::new());}
                ui.add_space(22.0);ui.label(RichText::new(format!("Double-click a local branch\nto check it out.\n\n↑ ↓  Navigate history\nF5    Refresh repository\n{}  Open repository",crate::platform::OPEN_SHORTCUT)).size(11.0).color(MUTED));
            });
        });
    }
    fn details(&mut self, ctx: &Context) {
        SidePanel::right("details")
            .default_width(400.0)
            .width_range(260.0..=520.0)
            .resizable(true)
            .frame(Frame::new().fill(PANEL).inner_margin(14))
            .show(ctx, |ui| {
                ui.set_min_width(ui.available_width());
                if self.working {
                    self.staging(ui);
                    return;
                }
                let commit = self
                    .selected
                    .and_then(|i| self.snapshot.commits.get(i))
                    .cloned();
                let Some(c) = commit else {
                    ui.label("Select a commit to inspect its changes");
                    return;
                };
                ui.label(RichText::new("COMMIT").size(11.0).color(MUTED));
                ui.add_space(12.0);
                ui.label(RichText::new(&c.subject).size(18.0).strong());
                ui.add_space(14.0);
                ui.horizontal(|ui| {
                    let (r, _) = ui.allocate_exact_size(vec2(30.0, 30.0), Sense::hover());
                    ui.painter().circle_filled(
                        r.center(),
                        15.0,
                        COLORS[self.graph.rows[self.selected.unwrap()].color % COLORS.len()],
                    );
                    ui.painter().text(
                        r.center(),
                        Align2::CENTER_CENTER,
                        c.author.chars().next().unwrap_or('?'),
                        FontId::proportional(15.0),
                        BG,
                    );
                    ui.vertical(|ui| {
                        ui.label(&c.author);
                        ui.label(RichText::new(&c.date).size(11.0).color(MUTED));
                    });
                });
                ui.add_space(12.0);
                ui.label(RichText::new(&c.id).monospace().size(10.0).color(ACCENT));
                for parent in &c.parents {
                    ui.label(
                        RichText::new(format!("parent  {}", &parent[..parent.len().min(10)]))
                            .monospace()
                            .size(11.0)
                            .color(MUTED),
                    );
                }
                ui.add_space(12.0);
                ui.horizontal_wrapped(|ui| {
                    for label in &c.refs {
                        ui.label(
                            RichText::new(label)
                                .color(ACCENT)
                                .background_color(Color32::from_rgb(34, 65, 74)),
                        );
                    }
                });
                ui.add_space(14.0);
                ui.horizontal(|ui| {
                    ui.menu_button("Commit actions  ▾", |ui| {
                        for name in ["Checkout", "Cherry-pick", "Revert"] {
                            if ui.button(name).clicked() {
                                self.op(name, c.id.clone());
                                ui.close_menu();
                            }
                        }
                    });
                });
                ui.add_space(14.0);
                ui.separator();
                ui.label(
                    RichText::new(format!("CHANGED FILES  ({})", self.files.len()))
                        .size(11.0)
                        .color(MUTED),
                );
                ui.add_space(6.0);
                ScrollArea::vertical()
                    .id_salt("commitfiles")
                    .show(ui, |ui| {
                        for file in self.files.clone() {
                            if ui
                                .selectable_label(self.selected_file == file, format!("▤  {file}"))
                                .clicked()
                            {
                                self.choose_file(file, false);
                            }
                        }
                    });
            });
    }
    fn staging(&mut self, ui: &mut Ui) {
        ui.label(RichText::new("WORKING DIRECTORY").size(11.0).color(MUTED));
        ui.add_space(10.0);
        let conflicts = self.snapshot.status.iter().any(|s| {
            s.index == 'U'
                || s.worktree == 'U'
                || (s.index == 'A' && s.worktree == 'A')
                || (s.index == 'D' && s.worktree == 'D')
        });
        if conflicts {
            ui.colored_label(
                Color32::LIGHT_RED,
                "Conflicts need resolution. Review files, resolve conflict markers, then stage.",
            );
        }
        let enabled = self.busy == 0;
        let list_height = (ui.available_height() * 0.27).max(90.0);
        for staged in [false, true] {
            ui.horizontal(|ui| {
                ui.label(
                    RichText::new(if staged {
                        "STAGED FILES"
                    } else {
                        "UNSTAGED FILES"
                    })
                    .size(11.0)
                    .color(MUTED),
                );
                if ui
                    .add_enabled(
                        enabled,
                        Button::new(if staged { "Unstage all" } else { "Stage all" }).small(),
                    )
                    .clicked()
                {
                    for s in self.snapshot.status.clone() {
                        if if staged {
                            s.index != ' ' && s.index != '?'
                        } else {
                            s.worktree != ' '
                        } {
                            self.send(Job::Stage(s.path, staged));
                        }
                    }
                }
            });
            ScrollArea::vertical()
                .id_salt(("changes", staged))
                .max_height(list_height)
                .min_scrolled_height(75.0)
                .show(ui, |ui| {
                    for s in self.snapshot.status.clone() {
                        if if staged {
                            s.index == ' ' || s.index == '?'
                        } else {
                            s.worktree == ' '
                        } {
                            continue;
                        }
                        ui.horizontal(|ui| {
                            ui.colored_label(
                                if conflicts {
                                    Color32::LIGHT_RED
                                } else {
                                    match if staged { s.index } else { s.worktree } {
                                        'D' => Color32::LIGHT_RED,
                                        'R' => Color32::from_rgb(100, 160, 225),
                                        'A' | '?' => Color32::from_rgb(135, 190, 110),
                                        _ => Color32::from_rgb(219, 174, 89),
                                    }
                                },
                                (if staged { s.index } else { s.worktree }).to_string(),
                            );
                            let width = (ui.available_width() - 28.0).max(40.0);
                            if ui
                                .add_sized(
                                    [width, 23.0],
                                    Button::new(RichText::new(&s.path).size(12.0))
                                        .selected(
                                            self.selected_file == s.path && self.staged == staged,
                                        )
                                        .frame(false),
                                )
                                .on_hover_text(&s.path)
                                .clicked()
                            {
                                self.choose_file(s.path.clone(), staged);
                            }
                            if ui
                                .add_enabled(
                                    enabled,
                                    Button::new(if staged { "−" } else { "+" }).small(),
                                )
                                .clicked()
                            {
                                self.send(Job::Stage(s.path, staged));
                            }
                        });
                    }
                });
            ui.separator();
            ui.add_space(8.0);
        }
        ui.label(RichText::new("COMMIT MESSAGE").size(11.0).color(MUTED));
        ui.add(
            TextEdit::multiline(&mut self.message)
                .hint_text("Summary\n\nDescription (optional)")
                .desired_rows(5)
                .desired_width(f32::INFINITY),
        );
        ui.add_space(8.0);
        let has_staged = self
            .snapshot
            .status
            .iter()
            .any(|s| s.index != ' ' && s.index != '?');
        let can_commit = enabled && has_staged && !conflicts && !self.message.trim().is_empty();
        if ui
            .add_enabled(
                can_commit,
                Button::new(
                    RichText::new(format!("Commit to {}", self.snapshot.head))
                        .color(if can_commit { BG } else { MUTED })
                        .strong(),
                )
                .fill(if can_commit {
                    ACCENT
                } else {
                    Color32::from_rgb(43, 54, 49)
                })
                .min_size(vec2(ui.available_width(), 34.0)),
            )
            .clicked()
        {
            self.send(Job::Commit(self.message.clone()));
        }
    }
    fn history(&mut self, ui: &mut Ui) {
        let reference_width = 140.0;
        let graph_width = (self.graph.lanes as f32 * 22.0 + 50.0).max(150.0);
        let graph_left = reference_width;
        let message_left = reference_width + graph_width;
        ui.horizontal(|ui| {
            ui.add_sized(
                [reference_width - 8.0, 20.0],
                Label::new(RichText::new("BRANCH / TAG").size(10.0).color(MUTED)),
            );
            ui.add_sized(
                [graph_width - 5.0, 20.0],
                Label::new(RichText::new("GRAPH").size(10.0).color(MUTED)),
            );
            ui.label(RichText::new("COMMIT MESSAGE").size(10.0).color(MUTED));
        });
        let mut clicked = None;
        let mut action = None;
        let query = self.search.to_lowercase();
        let mut scroll = ScrollArea::both()
            .id_salt("history")
            .auto_shrink([false, false]);
        if self.scroll_selection {
            scroll = scroll
                .vertical_scroll_offset(self.selected.unwrap_or(0).saturating_sub(4) as f32 * ROW);
            self.scroll_selection = false;
        }
        // show_rows measures spacing before entering its callback. Match the
        // virtual row stride to the painted stride, including while scrolling.
        let previous_spacing = ui.spacing().item_spacing.y;
        ui.spacing_mut().item_spacing.y = 0.0;
        scroll.show_rows(ui, ROW, self.snapshot.commits.len() + 1, |ui, range| {
            ui.spacing_mut().item_spacing.y = 0.0;
            for visible in range {
                let width = ui.available_width().max(message_left + 180.0);
                let (rect, response) = ui.allocate_exact_size(vec2(width, ROW), Sense::click());
                let p = ui.painter();
                if visible == 0 {
                    if self.working {
                        p.rect_filled(rect, 0.0, Color32::from_rgb(39, 51, 77));
                    } else if response.hovered() {
                        p.rect_filled(rect, 0.0, Color32::from_rgb(33, 41, 57));
                    }
                    let x = rect.left() + graph_left + 35.0;
                    if !self.snapshot.commits.is_empty() {
                        p.line_segment(
                            [pos2(x, rect.center().y), pos2(x, rect.bottom())],
                            Stroke::new(2.0_f32, COLORS[0]),
                        );
                    }
                    p.circle_stroke(pos2(x, rect.center().y), 8.0, Stroke::new(1.5_f32, ACCENT));
                    p.text(
                        pos2(rect.left() + message_left + 9.0, rect.center().y),
                        Align2::LEFT_CENTER,
                        format!(
                            "//  {} changes in working directory",
                            self.snapshot.status.len()
                        ),
                        FontId::proportional(12.0),
                        MUTED,
                    );
                    if response.clicked() {
                        self.working();
                    }
                    continue;
                }
                let index = visible - 1;
                let c = &self.snapshot.commits[index];
                let row = &self.graph.rows[index];
                let matches = query.is_empty()
                    || format!("{} {} {} {}", c.subject, c.id, c.author, c.refs.join(" "))
                        .to_lowercase()
                        .contains(&query);
                if self.selected == Some(index) {
                    p.rect_filled(rect, 0.0, Color32::from_rgb(39, 51, 77));
                } else if response.hovered() {
                    p.rect_filled(rect, 0.0, Color32::from_rgb(33, 41, 57));
                }
                p.line_segment(
                    [
                        pos2(rect.left() + reference_width, rect.top()),
                        pos2(rect.left() + reference_width, rect.bottom()),
                    ],
                    Stroke::new(1.0_f32, Color32::from_rgb(34, 37, 44)),
                );
                p.line_segment(
                    [
                        pos2(rect.left() + message_left, rect.top()),
                        pos2(rect.left() + message_left, rect.bottom()),
                    ],
                    Stroke::new(1.0_f32, Color32::from_rgb(43, 47, 55)),
                );
                if index == 0 && !self.snapshot.head_id.is_empty() {
                    let x = rect.left() + graph_left + 35.0;
                    p.line_segment(
                        [pos2(x, rect.top()), pos2(x, rect.center().y)],
                        Stroke::new(2.0_f32, COLORS[0]),
                    );
                }
                // Render only segments intersecting this visible row, including its incoming half.
                for (r, center_y, lower_half) in [
                    (index, rect.center().y, true),
                    (index.saturating_sub(1), rect.center().y - ROW, false),
                ] {
                    if !lower_half && index == 0 {
                        continue;
                    }
                    let clip = Rect::from_min_max(
                        pos2(rect.left() + graph_left, rect.top()),
                        pos2(rect.left() + message_left, rect.bottom()),
                    );
                    let painter = p.with_clip_rect(clip.intersect(p.clip_rect()));
                    for edge in &self.graph.rows[r].edges {
                        let x1 = rect.left() + graph_left + 35.0 + edge.from_lane as f32 * 22.0;
                        let x2 = rect.left() + graph_left + 35.0 + edge.to_lane as f32 * 22.0;
                        let a = pos2(x1, center_y);
                        let b = pos2(x2, center_y + ROW);
                        let color = COLORS[edge.color % COLORS.len()];
                        if x1 == x2 {
                            painter.line_segment([a, b], Stroke::new(2.0_f32, color));
                        } else {
                            painter.add(egui::epaint::CubicBezierShape::from_points_stroke(
                                [
                                    a,
                                    pos2(x1, center_y + ROW * 0.65),
                                    pos2(x2, center_y + ROW * 0.35),
                                    b,
                                ],
                                false,
                                Color32::TRANSPARENT,
                                Stroke::new(2.0_f32, color),
                            ));
                        }
                    }
                }
                let color = COLORS[row.color % COLORS.len()];
                let center = pos2(
                    rect.left() + graph_left + 35.0 + row.lane as f32 * 22.0,
                    rect.center().y,
                );
                let radius = if c.parents.len() > 1 { 5.0 } else { 9.0 };
                if c.refs.iter().any(|r| r == "HEAD") {
                    p.circle_stroke(center, radius + 3.0, Stroke::new(1.5_f32, color));
                }
                p.circle_filled(
                    center,
                    radius,
                    if c.parents.len() > 1 { color } else { PANEL },
                );
                p.circle_stroke(center, radius, Stroke::new(1.5_f32, color));
                if c.parents.len() <= 1 {
                    p.text(
                        center,
                        Align2::CENTER_CENTER,
                        c.author.chars().next().unwrap_or('?'),
                        FontId::proportional(9.0),
                        color,
                    );
                }
                if !c.refs.is_empty() {
                    let refs = c
                        .refs
                        .iter()
                        .filter(|r| r.as_str() != "HEAD")
                        .collect::<Vec<_>>();
                    let label = refs.first().map(|r| r.to_string()).unwrap_or_default();
                    let label = if label.is_empty() {
                        "HEAD".to_string()
                    } else {
                        label
                    };
                    let rr = Rect::from_min_size(
                        pos2(rect.left() + 7.0, rect.top() + 4.0),
                        vec2(reference_width - 16.0, 20.0),
                    );
                    p.rect_filled(rr, 2.0, color.gamma_multiply(0.25));
                    p.line_segment(
                        [rr.right_center(), center],
                        Stroke::new(1.0_f32, color.gamma_multiply(0.65)),
                    );
                    let overflow = refs.len() > 1;
                    let label_rect = if overflow {
                        Rect::from_min_max(rr.min, rr.max - vec2(25.0, 0.0))
                    } else {
                        rr
                    };
                    let label_p = p.with_clip_rect(label_rect);
                    label_p.text(
                        rr.left_center() + vec2(5.0, 0.0),
                        Align2::LEFT_CENTER,
                        format!(
                            "{} {label}",
                            if c.refs.iter().any(|r| r == "HEAD") {
                                "✓"
                            } else {
                                "⑂"
                            }
                        ),
                        FontId::proportional(11.0),
                        color,
                    );
                    if overflow {
                        p.text(
                            rr.right_center() - vec2(3.0, 0.0),
                            Align2::RIGHT_CENTER,
                            format!("+{}", refs.len() - 1),
                            FontId::proportional(10.0),
                            color,
                        );
                    }
                }
                let metadata = if width > 780.0 { 178.0 } else { 82.0 };
                let msgrect = Rect::from_min_max(
                    pos2(rect.left() + message_left + 8.0, rect.top()),
                    pos2(rect.right() - metadata, rect.bottom()),
                );
                p.with_clip_rect(msgrect).text(
                    msgrect.left_center(),
                    Align2::LEFT_CENTER,
                    &c.subject,
                    FontId::proportional(12.5),
                    if matches {
                        TEXT
                    } else {
                        Color32::from_gray(78)
                    },
                );
                p.text(
                    pos2(rect.right() - 10.0, rect.center().y),
                    Align2::RIGHT_CENTER,
                    if metadata > 100.0 {
                        format!("{}   {}", c.author, &c.id[..7])
                    } else {
                        c.id[..7].into()
                    },
                    FontId::monospace(10.5),
                    MUTED,
                );
                if response.clicked() {
                    clicked = Some(index);
                }
                response
                    .on_hover_text(format!("{}\n{}\n{}", c.subject, c.author, c.id))
                    .context_menu(|ui| {
                        for op in ["Checkout", "Cherry-pick", "Revert"] {
                            if ui.button(op).clicked() {
                                action = Some((op.to_string(), c.id.clone()));
                                ui.close_menu();
                            }
                        }
                    });
            }
        });
        ui.spacing_mut().item_spacing.y = previous_spacing;
        if let Some(i) = clicked {
            self.select(i);
        }
        if let Some((op, target)) = action {
            self.op(&op, target);
        }
    }
    fn diff_view(&mut self, ui: &mut Ui) {
        ui.horizontal(|ui| {
            ui.label(RichText::new(&self.selected_file).strong());
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                if ui
                    .small_button("×")
                    .on_hover_text("Return to graph")
                    .clicked()
                {
                    self.selected_file.clear();
                }
                if self.working
                    && ui
                        .add_enabled(
                            self.busy == 0,
                            Button::new(if self.staged {
                                "Unstage file"
                            } else {
                                "Stage file"
                            }),
                        )
                        .clicked()
                {
                    self.send(Job::Stage(self.selected_file.clone(), self.staged));
                }
            });
        });
        ui.separator();
        ui.horizontal(|ui| {
            if self.working {
                for staged in [false, true] {
                    if ui
                        .selectable_label(
                            self.staged == staged,
                            if staged { "Staged" } else { "Unstaged" },
                        )
                        .clicked()
                    {
                        self.choose_file(self.selected_file.clone(), staged);
                    }
                }
            } else {
                ui.label("Committed changes");
            }
            ui.separator();
            ui.checkbox(&mut self.side_by_side, "Split view");
            ui.checkbox(&mut self.ignore_space, "Dim whitespace");
        });
        ui.separator();
        let lines = &self.diff_lines;
        if lines.is_empty() {
            ui.label(
                RichText::new(if self.busy > 0 {
                    "Loading diff…"
                } else {
                    "No text diff. Binary, mode-only, or empty file changes can still be staged."
                })
                .color(MUTED),
            );
            return;
        }
        let split = &self.split_lines;
        let count = if self.side_by_side {
            split.len()
        } else {
            lines.len()
        };
        let mut stage_hunk = None;
        let previous_spacing = ui.spacing().item_spacing.y;
        ui.spacing_mut().item_spacing.y = 0.0;
        ScrollArea::both()
            .id_salt((
                "diff",
                self.selected_file.clone(),
                self.staged,
                self.working,
            ))
            .auto_shrink([false, false])
            .show_rows(ui, 18.0, count, |ui, range| {
                ui.spacing_mut().item_spacing.y = 0.0;
                for i in range {
                    let line = if self.side_by_side {
                        split[i].0.as_ref().or(split[i].1.as_ref()).unwrap()
                    } else {
                        &lines[i]
                    };
                    let plus = line.kind == '+';
                    let minus = line.kind == '-';
                    let header = line.kind == '@';
                    let color = if plus {
                        Color32::from_rgb(158, 209, 155)
                    } else if minus {
                        Color32::from_rgb(225, 152, 155)
                    } else if header {
                        ACCENT
                    } else {
                        TEXT
                    };
                    let bg = if plus {
                        Color32::from_rgb(31, 56, 43)
                    } else if minus {
                        Color32::from_rgb(62, 36, 43)
                    } else {
                        BG
                    };
                    let width = ui.available_width().max(900.0);
                    let (rect, _) = ui.allocate_exact_size(vec2(width, 18.0), Sense::hover());
                    ui.painter().rect_filled(rect, 0.0, bg);
                    let line_color = if self.ignore_space && line.text.trim().len() <= 1 {
                        MUTED
                    } else {
                        color
                    };
                    if self.side_by_side {
                        let mid = rect.left() + width / 2.0;
                        ui.painter().line_segment(
                            [pos2(mid, rect.top()), pos2(mid, rect.bottom())],
                            Stroke::new(1.0_f32, LINE),
                        );
                        for (right, item) in
                            [(false, split[i].0.as_ref()), (true, split[i].1.as_ref())]
                        {
                            if let Some(item) = item {
                                let left = if right { mid } else { rect.left() };
                                let clip = Rect::from_min_max(
                                    pos2(left, rect.top()),
                                    pos2(left + width / 2.0, rect.bottom()),
                                );
                                let painter =
                                    ui.painter().with_clip_rect(clip.intersect(ui.clip_rect()));
                                let n = if right { item.new } else { item.old }
                                    .map(|n| n.to_string())
                                    .unwrap_or_default();
                                painter.text(
                                    pos2(left + 6.0, rect.center().y),
                                    Align2::LEFT_CENTER,
                                    n,
                                    FontId::monospace(10.0),
                                    MUTED,
                                );
                                let c = if item.kind == '+' {
                                    COLORS[8]
                                } else if item.kind == '-' {
                                    Color32::LIGHT_RED
                                } else {
                                    line_color
                                };
                                paint_code(
                                    &painter,
                                    pos2(left + 44.0, rect.center().y),
                                    &item.text,
                                    c,
                                );
                            }
                        }
                    } else {
                        let old = line.old.map(|n| n.to_string()).unwrap_or_default();
                        let new = line.new.map(|n| n.to_string()).unwrap_or_default();
                        ui.painter().text(
                            rect.left_center() + vec2(5.0, 0.0),
                            Align2::LEFT_CENTER,
                            format!("{old:>4} {new:>4}"),
                            FontId::monospace(10.5),
                            MUTED,
                        );
                        paint_code(
                            ui.painter(),
                            rect.left_center() + vec2(78.0, 0.0),
                            &line.text,
                            line_color,
                        );
                    }
                    if line.text.starts_with("@@ ") && self.working {
                        let hunk_index = if self.side_by_side {
                            split[..i]
                                .iter()
                                .filter(|(a, b)| {
                                    a.as_ref()
                                        .or(b.as_ref())
                                        .is_some_and(|l| l.text.starts_with("@@ "))
                                })
                                .count()
                        } else {
                            lines[..i]
                                .iter()
                                .filter(|l| l.text.starts_with("@@ "))
                                .count()
                        };
                        let button = Rect::from_min_size(
                            pos2(ui.clip_rect().right() - 104.0, rect.top()),
                            vec2(100.0, 18.0),
                        );
                        let response = ui
                            .interact(button, ui.id().with(("hunk", i)), Sense::click())
                            .on_hover_text(
                                self.hunks
                                    .get(hunk_index)
                                    .map(|h| h.title.as_str())
                                    .unwrap_or("Update this hunk in the index"),
                            );
                        ui.painter().rect_filled(
                            button,
                            1.0,
                            if response.hovered() {
                                Color32::from_rgb(66, 96, 68)
                            } else {
                                Color32::from_rgb(46, 67, 51)
                            },
                        );
                        ui.painter().text(
                            button.center(),
                            Align2::CENTER_CENTER,
                            if self.staged {
                                "Unstage hunk"
                            } else {
                                "Stage hunk"
                            },
                            FontId::proportional(11.0),
                            TEXT,
                        );
                        if response.clicked() && self.busy == 0 {
                            stage_hunk = self.hunks.get(hunk_index).cloned();
                        }
                    }
                }
            });
        ui.spacing_mut().item_spacing.y = previous_spacing;
        if let Some(hunk) = stage_hunk {
            self.send(Job::Hunk(hunk, self.staged));
        }
    }
}

impl eframe::App for GitCrab {
    fn update(&mut self, ctx: &Context, _frame: &mut eframe::Frame) {
        self.receive();
        if self.busy > 0 {
            ctx.request_repaint_after(Duration::from_millis(50));
        }
        if ctx.input(|i| i.modifiers.command && i.key_pressed(Key::O)) {
            self.open_dialog = true;
        }
        if !self.path.is_empty() && ctx.input(|i| i.key_pressed(Key::F5)) {
            self.refresh();
        }
        if !ctx.wants_keyboard_input() && !self.open_dialog && self.operation.is_none() {
            if ctx.input(|i| i.key_pressed(Key::ArrowDown)) {
                self.select(self.selected.map_or(0, |i| i + 1));
                self.scroll_selection = true;
            }
            if ctx.input(|i| i.key_pressed(Key::ArrowUp)) {
                self.select(self.selected.unwrap_or(1).saturating_sub(1));
                self.scroll_selection = true;
            }
        }
        self.top(ctx);
        self.sidebar(ctx);
        self.details(ctx);
        CentralPanel::default()
            .frame(Frame::new().fill(BG).inner_margin(0))
            .show(ctx, |ui| {
                if self.path.is_empty() {
                    ui.centered_and_justified(|ui| {
                        ui.label("Open a local repository to explore its history");
                    });
                    return;
                }
                if !self.selected_file.is_empty() {
                    self.diff_view(ui);
                    return;
                }
                TopBottomPanel::bottom("pagination")
                    .exact_height(28.0)
                    .show_inside(ui, |ui| {
                        ui.horizontal(|ui| {
                            ui.label(
                                RichText::new(format!("{} commits", self.snapshot.commits.len()))
                                    .size(11.0)
                                    .color(MUTED),
                            );
                            if self.snapshot.commits.len() >= self.limit
                                && ui
                                    .add_enabled(
                                        self.busy == 0,
                                        Button::new("Load 500 more").small(),
                                    )
                                    .clicked()
                            {
                                self.limit += 500;
                                self.refresh();
                            }
                        });
                    });
                self.history(ui);
            });
        if self.open_dialog {
            Window::new("Open local repository")
                .collapsible(false)
                .resizable(false)
                .anchor(Align2::CENTER_CENTER, vec2(0.0, -80.0))
                .show(ctx, |ui| {
                    ui.label("Repository folder");
                    ui.add(TextEdit::singleline(&mut self.open_path).desired_width(480.0));
                    ui.add_space(8.0);
                    ui.horizontal(|ui| {
                        if ui
                            .add_enabled(
                                !self.open_path.trim().is_empty(),
                                Button::new("Open repository").fill(Color32::from_rgb(40, 95, 108)),
                            )
                            .clicked()
                        {
                            self.send(Job::Open(self.open_path.clone(), self.limit));
                            self.open_dialog = false;
                        }
                        if ui.button("Cancel").clicked() {
                            self.open_dialog = false;
                        }
                    });
                });
        }
        if let Some(op) = self.operation.clone() {
            Window::new(format!("{op} — review operation")).collapsible(false).resizable(false).anchor(Align2::CENTER_CENTER,Vec2::ZERO).show(ctx,|ui|{
                ui.label(format!("Repository: {}",self.path));ui.label(format!("Current HEAD: {}",self.snapshot.head));ui.add_space(8.0);
                if op=="Pop stash"{ui.label("Apply the latest stash and remove it if successful.");}else{ui.label(if op=="Stash"{"Stash message"}else{"Target branch or commit"});ui.add(TextEdit::singleline(&mut self.target).desired_width(430.0));}
                if op=="Merge"||op=="Cherry-pick"||op=="Revert"{ui.label("Git will report conflicts. Resolve the files and stage the result before continuing in Git.");}
                ui.horizontal(|ui|{if ui.add_enabled(self.busy==0&&(op=="Pop stash"||op=="Stash"||!self.target.trim().is_empty()),Button::new(format!("Run {op}"))).clicked(){self.send(Job::Operation(op.clone(),self.target.clone()));self.operation=None;}if ui.button("Cancel").clicked(){self.operation=None;}});
            });
        }
        // Optional reproducible evidence capture from the actual native renderer.
        if self.capture.is_some()
            && !self.capture_requested
            && self.busy == 0
            && !self.path.is_empty()
            && self.last_load.elapsed() > Duration::from_millis(800)
        {
            ctx.send_viewport_cmd(ViewportCommand::Screenshot(Default::default()));
            self.capture_requested = true;
        }
        if self.capture.is_some() && !self.capture_requested {
            ctx.request_repaint_after(Duration::from_millis(200));
        }
        for event in ctx.input(|i| i.events.clone()) {
            if let Event::Screenshot { image, .. } = event {
                if let Some(path) = self.capture.take() {
                    let pixels: Vec<u8> = image.pixels.iter().flat_map(|c| c.to_array()).collect();
                    if let Err(e) = image::save_buffer(
                        &path,
                        &pixels,
                        image.width() as u32,
                        image.height() as u32,
                        image::ColorType::Rgba8,
                    ) {
                        self.notice = format!("Screenshot: {e}");
                        self.error = true;
                    } else if std::env::var_os("GITCRAB_CAPTURE_EXIT").is_some() {
                        ctx.send_viewport_cmd(ViewportCommand::Close);
                    }
                }
            }
        }
    }
}
