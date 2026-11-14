//! Scripted navigation and render measurements for diagnostic builds.

use super::*;

#[cfg(feature = "native-evidence")]
const STAGE_BENCH_WRITES: usize = 10;

/// Timings of scripted whole-file writes through the real view, for the
/// `stagefiles` capture view: stage the first unstaged file, unstage it,
/// and repeat.
#[cfg(feature = "native-evidence")]
#[derive(Default)]
pub(super) struct StageBench {
    pub(super) remaining: usize,
    path: Option<String>,
    staged_last: bool,
    current: std::rc::Rc<std::cell::RefCell<Option<StageSample>>>,
    samples: std::rc::Rc<std::cell::RefCell<Vec<serde_json::Value>>>,
    /// When the last write settled; requests completing after it, while the
    /// script pauses like a person would, are recorded as idle work.
    settled_at: Option<std::time::Instant>,
}

#[cfg(feature = "native-evidence")]
#[derive(Clone)]
struct StageSample {
    unstage: bool,
    click: std::time::Instant,
    /// Time the click handler held the UI thread.
    handler_ms: f64,
    /// First frame drawn after the click, showing the predicted move.
    first_frame_ms: Option<f64>,
    /// The worker's reply reached the view.
    response_ms: Option<f64>,
    worker_ms: f64,
    queue_ms: f64,
}

#[cfg(feature = "native-evidence")]
impl GitCrab {
    fn stage_bench_write(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(snapshot) = self.state.snapshot.as_ref() else {
            return;
        };
        let unstage = self.stage_bench.path.is_some() && self.stage_bench.staged_last;
        let file = match &self.stage_bench.path {
            Some(path) => snapshot
                .status
                .iter()
                .find(|f| &f.path == path && f.on_side(unstage)),
            None => snapshot
                .status
                .iter()
                .find(|f| f.on_side(false) && !f.is_unmerged()),
        }
        .cloned();
        let Some(file) = file else {
            self.finish_stage_bench("no file to stage or unstage");
            return;
        };
        self.stage_bench.path = Some(file.path.clone());
        self.stage_bench.staged_last = !unstage;
        let _ = self.backend.drain_git_calls();
        let click = std::time::Instant::now();
        self.mutate(
            Action::StageFile {
                reviewed: file,
                unstage,
            },
            window,
            cx,
        );
        *self.stage_bench.current.borrow_mut() = Some(StageSample {
            unstage,
            click,
            handler_ms: click.elapsed().as_secs_f64() * 1000.,
            first_frame_ms: None,
            response_ms: None,
            worker_ms: 0.,
            queue_ms: 0.,
        });
        let current = self.stage_bench.current.clone();
        window.on_next_frame(move |_, _| {
            if let Some(sample) = current.borrow_mut().as_mut() {
                sample.first_frame_ms = Some(sample.click.elapsed().as_secs_f64() * 1000.);
            }
        });
    }

    fn advance_stage_bench(&mut self, lane: Lane, window: &mut Window, cx: &mut Context<Self>) {
        if self.stage_bench.current.borrow().is_none() {
            // Work that arrives while no scripted write runs, such as a file
            // watcher refresh after GitCrab's own write.
            if let Some(settled) = self.stage_bench.settled_at {
                self.stage_bench
                    .samples
                    .borrow_mut()
                    .push(serde_json::json!({
                        "action": "idle",
                        "lane": format!("{lane:?}"),
                        "after_settled_ms": settled.elapsed().as_secs_f64() * 1000.,
                        "worker_ms": self.state.worker_ms,
                    }));
            }
            return;
        }
        if lane == Lane::Mutation {
            if let Some(sample) = self.stage_bench.current.borrow_mut().as_mut() {
                sample.response_ms = Some(sample.click.elapsed().as_secs_f64() * 1000.);
                sample.worker_ms = self.state.worker_ms;
                sample.queue_ms = self.state.queue_ms;
            }
        }
        let responded = self
            .stage_bench
            .current
            .borrow()
            .as_ref()
            .is_some_and(|sample| sample.response_ms.is_some());
        if !responded || self.state.busy || self.state.loading() {
            return;
        }
        // Everything has settled: time the frame that shows it, then go on.
        let Some(sample) = self.stage_bench.current.borrow_mut().take() else {
            return;
        };
        let settled_ms = sample.click.elapsed().as_secs_f64() * 1000.;
        self.stage_bench.settled_at = Some(std::time::Instant::now());
        let calls = self.backend.drain_git_calls();
        let samples = self.stage_bench.samples.clone();
        self.stage_bench.remaining -= 1;
        let more = self.stage_bench.remaining > 0;
        let weak = cx.weak_entity();
        window.on_next_frame(move |window, cx| {
            samples.borrow_mut().push(serde_json::json!({
                "action": if sample.unstage { "unstage file" } else { "stage file" },
                "handler_ms": sample.handler_ms,
                "first_frame_ms": sample.first_frame_ms,
                "response_ms": sample.response_ms,
                "settled_ms": settled_ms,
                "settled_frame_ms": sample.click.elapsed().as_secs_f64() * 1000.,
                "worker_ms": sample.worker_ms,
                "queue_ms": sample.queue_ms,
                "git_processes": calls.len(),
                "git": calls.iter().map(|c| serde_json::json!({
                    "args": c.args, "thread": c.thread, "start_ms": c.start_ms,
                    "spawn_ms": c.spawn_ms, "total_ms": c.total_ms,
                })).collect::<Vec<_>>(),
            }));
            if !more {
                let _ = weak.update(cx, |v, _| v.finish_stage_bench("done"));
                return;
            }
            // Pause like a person between clicks, so anything that follows
            // a write on its own shows up as idle work.
            window
                .spawn(cx, async move |cx| {
                    cx.background_executor()
                        .timer(std::time::Duration::from_millis(800))
                        .await;
                    let _ = weak.update_in(cx, |v, window, cx| v.stage_bench_write(window, cx));
                })
                .detach();
        });
        // Outside of drawing, notifying is what asks GPUI for the next frame.
        cx.notify();
    }

    /// Clicks Stage (or Unstage) on up to five files in a row without
    /// waiting, as a person going down the list would.
    fn rapid_clicks(&mut self, unstage: bool, window: &mut Window, cx: &mut Context<Self>) {
        let Some(snapshot) = self.state.snapshot.as_ref() else {
            return;
        };
        let files: Vec<_> = snapshot
            .status
            .iter()
            .filter(|f| f.on_side(unstage) && !f.is_unmerged() && f.index != '?')
            .take(5)
            .cloned()
            .collect();
        let click = std::time::Instant::now();
        for file in &files {
            self.mutate(
                Action::StageFile {
                    reviewed: file.clone(),
                    unstage,
                },
                window,
                cx,
            );
        }
        let handler_ms = click.elapsed().as_secs_f64() * 1000.;
        let moved_at_once = self.state.pending_paths.len();
        let queued = self.state.queued_writes();
        *self.stage_bench.current.borrow_mut() = Some(StageSample {
            unstage,
            click,
            handler_ms,
            first_frame_ms: None,
            response_ms: None,
            worker_ms: 0.,
            queue_ms: 0.,
        });
        self.stage_bench
            .samples
            .borrow_mut()
            .push(serde_json::json!({
                "action": if unstage { "unstage 5 files rapidly" } else { "stage 5 files rapidly" },
                "files": files.iter().map(|f| f.path.clone()).collect::<Vec<_>>(),
                "handler_ms": handler_ms,
                "moved_at_once": moved_at_once,
                "queued": queued,
            }));
        let current = self.stage_bench.current.clone();
        window.on_next_frame(move |_, _| {
            if let Some(sample) = current.borrow_mut().as_mut() {
                sample.first_frame_ms = Some(sample.click.elapsed().as_secs_f64() * 1000.);
            }
        });
    }

    fn rapid_settled(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(sample) = self.stage_bench.current.borrow_mut().take() else {
            return;
        };
        let unstage = sample.unstage;
        let still_on_side = self.state.snapshot.as_ref().map_or(0, |s| {
            s.status
                .iter()
                .filter(|f| f.on_side(unstage) && !f.is_unmerged() && f.index != '?')
                .count()
        });
        self.stage_bench
            .samples
            .borrow_mut()
            .push(serde_json::json!({
                "action": "settled",
                "first_frame_ms": sample.first_frame_ms,
                "settled_ms": sample.click.elapsed().as_secs_f64() * 1000.,
                "error": self.state.error.then(|| self.state.notice.clone()),
                "left_on_clicked_side": still_on_side,
                "git_processes": self.backend.drain_git_calls().len(),
            }));
        if self.evidence_phase == 8 {
            self.evidence_phase = 9;
            self.rapid_clicks(true, window, cx);
        } else {
            self.finish_stage_bench("done");
        }
    }

    fn finish_stage_bench(&mut self, outcome: &str) {
        self.evidence_phase = 7;
        let Ok(ready) = std::env::var("GITCRAB_NATIVE_READY") else {
            return;
        };
        let data = serde_json::json!({
            "outcome": outcome,
            "notice": self.state.notice,
            "samples": *self.stage_bench.samples.borrow(),
        });
        let _ = std::fs::write(
            format!("{ready}.stage.json"),
            serde_json::to_vec_pretty(&data).unwrap(),
        );
    }
}

impl GitCrab {
    /// Advances the configured capture view as its repository reads complete.
    #[cfg(feature = "native-evidence")]
    pub(super) fn advance_evidence(
        &mut self,
        lane: Lane,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if std::env::var_os("GITCRAB_NATIVE_READY").is_none() {
            return;
        }
        let mode = std::env::var("GITCRAB_CAPTURE_VIEW").unwrap_or_else(|_| "history".into());
        if self.state.error {
            self.evidence_phase = 9;
            return;
        }
        if self.evidence_phase == 6 {
            self.advance_stage_bench(lane, window, cx);
            return;
        }
        if matches!(self.evidence_phase, 8 | 9) && !self.state.busy && !self.state.loading() {
            self.rapid_settled(window, cx);
            return;
        }
        if self.evidence_phase == 0 && lane == Lane::Snapshot && self.state.snapshot.is_some() {
            self.evidence_phase = 1;
            if mode == "stagefiles" {
                self.evidence_phase = 6;
                self.stage_bench.remaining = STAGE_BENCH_WRITES;
                self.stage_bench_write(window, cx);
            } else if mode == "stagerapid" {
                self.evidence_phase = 8;
                self.rapid_clicks(false, window, cx);
            } else if mode == "details" || mode == "commitdiff" {
                let id = self.state.snapshot.as_ref().unwrap().head_id.clone();
                self.select(Some(id), window, cx);
            } else if matches!(mode.as_str(), "diff" | "split" | "stageddiff") {
                self.split = mode == "split";
                if let Some(path) = self
                    .state
                    .snapshot
                    .as_ref()
                    .unwrap()
                    .status
                    .first()
                    .map(|f| f.path.clone())
                {
                    self.file(path, mode == "stageddiff", window, cx);
                }
            } else {
                if mode == "dialog" {
                    self.prompt_branch(None, window, cx);
                }
                self.evidence_phase = 3;
            }
        } else if self.evidence_phase == 1 && lane == Lane::Details {
            if mode == "commitdiff" {
                if let Some(path) = self.state.files.first().map(|file| file.path.clone()) {
                    self.file(path, false, window, cx);
                    self.evidence_phase = 2;
                }
            } else {
                self.evidence_phase = 3;
            }
        } else if matches!(self.evidence_phase, 1 | 2) && lane == Lane::Diff {
            self.evidence_phase = 3;
        }
    }
}

impl GitCrab {
    /// Publishes diagnostic state and drives synthetic scrolling when requested.
    pub(super) fn render_evidence(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(path) = std::env::var_os("GITCRAB_SMOKE_STATE") {
            let data = serde_json::json!({
                "busy": self.state.busy,
                "notice": self.state.notice,
                "dialog": window.has_active_dialog(cx),
                "branch_prompt": self.branch_prompt_open(),
                "focused": self.focus.is_focused(window),
                "selected": self.state.selected,
            });
            window.on_next_frame(move |_, _| {
                let _ = std::fs::write(path, serde_json::to_vec(&data).unwrap());
            });
        }
        if let Some((start, kind)) = self.feedback_start.take() {
            window.on_next_frame(move |_, cx| crate::trace::record(kind, start, cx));
        }
        if self.evidence_phase == 3 {
            self.evidence_phase = 4;
            let data = serde_json::json!({"loaded":self.state.graph.rows.len(),"view":std::env::var("GITCRAB_CAPTURE_VIEW").unwrap_or_else(|_|"history".into()),"width":f32::from(window.viewport_size().width),"height":f32::from(window.viewport_size().height),"scale":window.scale_factor(),"worker_ms":self.state.worker_ms,"queue_ms":self.state.queue_ms,"diff_rows":self.state.patch.as_ref().map(|p|p.content.lines.len()),"dialog_layer":window.has_active_dialog(cx),"branch_prompt":self.branch_prompt_open(),"renderer":"GPUI 0.2.2 native","present_time_measured":false});
            let path = std::env::var("GITCRAB_NATIVE_READY").unwrap();
            window.on_next_frame(move |_, _| {
                let _ = std::fs::write(path, serde_json::to_vec_pretty(&data).unwrap());
            });
            window.request_animation_frame();
        }
        if self.evidence_phase == 4 && std::env::var_os("GITCRAB_NATIVE_BENCH").is_some() {
            if let Some(previous) = self.benchmark.0.replace(std::time::Instant::now()) {
                self.benchmark
                    .1
                    .push(previous.elapsed().as_secs_f64() * 1000.);
            }
            if self.benchmark.1.len() < 240 {
                let i = self.benchmark.1.len();
                if let Some(p) = &self.state.patch {
                    self.diff_scroll.scroll_to_item(
                        (i * 2).min(p.content.lines.len().saturating_sub(1)),
                        ScrollStrategy::Top,
                    );
                } else {
                    self.history_scroll.scroll_to_item(
                        (i * 2).min(self.state.graph.rows.len()),
                        ScrollStrategy::Top,
                    );
                }
                let weak = cx.weak_entity();
                window.on_next_frame(move |_, cx| {
                    let _ = weak.update(cx, |_, cx| cx.notify());
                });
                window.request_animation_frame();
            } else {
                self.evidence_phase = 5;
                let data = serde_json::json!({"samples_ms":self.benchmark.1,"conditions":"240 GPUI render/animation intervals with synthetic scroll; not physical presentation or input latency","loaded":self.state.graph.rows.len(),"diff_rows":self.state.patch.as_ref().map(|p|p.content.lines.len())});
                let path = format!(
                    "{}.bench.json",
                    std::env::var("GITCRAB_NATIVE_READY").unwrap()
                );
                let _ = std::fs::write(path, serde_json::to_vec_pretty(&data).unwrap());
            }
        }
        if let Some(path) = std::env::var_os("GITCRAB_DRAFT_PROBE") {
            let data = serde_json::json!({"draft":self.state.draft,"revision":self.state.draft_revision,"wip":self.wip_summary.read(cx).value().to_string(),"summary":self.draft.read(cx).value().to_string(),"body":self.description.read(cx).value().to_string(),"wip_focused":self.wip_summary.read(cx).focus_handle(cx).is_focused(window),"summary_focused":self.draft.read(cx).focus_handle(cx).is_focused(window),"activity":self.activity.iter().map(|r|r.summary()).collect::<Vec<_>>()});
            let _ = std::fs::write(path, serde_json::to_vec_pretty(&data).unwrap());
        }
    }
}
