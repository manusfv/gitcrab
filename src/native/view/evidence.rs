//! Scripted navigation and render measurements for diagnostic builds.

use super::*;

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
        if self.evidence_phase == 0 && lane == Lane::Snapshot && self.state.snapshot.is_some() {
            self.evidence_phase = 1;
            if mode == "details" || mode == "commitdiff" {
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
