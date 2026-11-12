//! The build loop in the app: the settings popover beside Send, and the runs that follow each
//! other until a limit ends the loop. What ends it is decided in `craft_anything_ai::rebuild::
//! build_loop` (the CLI's `rebuild --loop` uses the same); this starts each run and keeps count.

use crate::i18n::{tr, trf};
use crate::theme::{self, Tokens};
use crate::{CraftApp, Done, widgets};
use craft_anything_ai::rebuild::Stopped;
use craft_anything_ai::rebuild::build_loop::{self as rules, LoopEnd, LoopState, Next, tokens_words};
use craft_anything_engine::feed::{self, FeedItem, UserItem};
use craft_anything_engine::inbox::now_ms;
use craft_anything_engine::workspace::build_loop::{ClockTime, DEFAULT_LOOP_PROMPT, LoopSettings, MAX_ROUNDS_PER_RUN};
use craft_anything_engine::workspace::{Step, Workspace};
use egui::RichText;
use std::path::{Path, PathBuf};
use std::time::Duration;

/// A loop going on one project.
pub struct ActiveLoop {
    pub state: LoopState,
    /// A run ended well and the project's progress is being read before the next one starts.
    pub between: bool,
    /// A limit came mid-run: the run was stopped, and the loop ends with this once it has.
    pub ending: Option<LoopEnd>,
}

/// The popover's copy of the open project's loop settings while they're edited.
pub struct LoopDraft {
    pub root: PathBuf,
    pub settings: LoopSettings,
    /// What was saved when the draft was taken: a change on disk (a save landing, a builder
    /// switching the loop off) replaces the draft.
    base: LoopSettings,
    /// What was last sent to be saved, so it's sent once.
    sent: Option<LoopSettings>,
}

/// The longest the app waits before looking at a loop's stop time again.
const CLOCK_CHECK: Duration = Duration::from_secs(30);

impl CraftApp {
    /// Start counting a loop on `root`; the run going now (if any) is its first.
    pub(crate) fn begin_loop(&mut self, root: &Path, settings: LoopSettings) {
        let state = LoopState::start(settings, now_ms(), 0);
        self.loops.insert(root.to_path_buf(), ActiveLoop { state, between: false, ending: None });
        self.read_loop_progress(root, false);
    }

    /// Read the project's progress (and its saved loop settings) on a worker.
    fn read_loop_progress(&mut self, root: &Path, after_run: bool) {
        let root = root.to_path_buf();
        self.jobs.spawn(tr("Check the build loop's progress"), false, move |_| {
            let ws = Workspace::open(&root).ok();
            let fingerprint = ws.as_ref().map_or(0, rules::fingerprint);
            let settings = ws.and_then(|w| w.manifest.build_loop);
            Ok(Done::LoopProgress { root, fingerprint, settings, after_run })
        });
    }

    /// The project's progress was read: at the loop's start it's what later runs are compared
    /// with; after a run it decides whether the next one starts.
    pub(crate) fn loop_progress(&mut self, root: PathBuf, fingerprint: u64, settings: Option<LoopSettings>, after_run: bool) {
        let Some(l) = self.loops.get_mut(&root) else { return };
        if !after_run {
            if l.state.runs == 0 && !l.between {
                l.state.set_progress(fingerprint);
            }
            return;
        }
        l.between = false;
        let now = now_ms();
        match settings {
            Some(s) if s.on => l.state.update(s, now),
            // Switched off while the run worked: it ends here.
            _ => return self.end_loop(&root, LoopEnd::Stopped),
        }
        let unreported = l.state.tokens_unreported;
        let next = l.state.after_run(now, fingerprint);
        if !unreported && l.state.tokens_unreported && l.state.settings.tokens.on {
            self.toast(tr("This builder doesn't say how many tokens it uses, so the token budget can't end the loop."), true);
        }
        match next {
            Next::Run => self.next_loop_run(&root),
            Next::End(why) => self.end_loop(&root, why),
        }
    }

    /// Start the loop's next run with a fresh builder. A run already going (a note the person
    /// sent started it) counts as this one.
    fn next_loop_run(&mut self, root: &Path) {
        if self.runs.is_running(root) || self.runs.position(root).is_some() {
            return;
        }
        let Some(l) = self.loops.get(root) else { return };
        let (prompt, rounds, run) = (l.state.settings.prompt().to_string(), l.state.settings.rounds(), l.state.runs.saturating_add(1));
        let open = self.workspace.as_ref().is_some_and(|w| w.root == root);
        let mut ws = match self.workspace.clone().filter(|_| open).map_or_else(|| Workspace::open(root), Ok) {
            Ok(ws) => ws,
            Err(e) => return self.loop_failed(root, &e.message),
        };
        // The AI picked now, as for any other start (else the project's saved one).
        let Some(backend) = self.builder_now(&mut ws, None) else {
            return self.loop_failed(root, &tr("No AI is set up to build with."));
        };
        let shown = trf("Build loop, run {n}: a fresh builder carries on from the last one's handoff.", &[("n", &run.to_string())]);
        let mut opts = self.run_options(Some(backend), Some(prompt), Some(shown.clone()), Vec::new());
        opts.fresh_session = true;
        opts.max_rounds = rounds;
        if open {
            self.pausing.remove(root);
            self.sending_now.remove(root);
            self.state.feed.push(FeedItem::User(UserItem::new(shown, Vec::new())));
        }
        if let Err(e) = self.launch(ws, opts) {
            self.loop_failed(root, &e.message);
        }
        self.refresh_projects();
    }

    fn loop_failed(&mut self, root: &Path, why: &str) {
        self.toast(trf("The build loop couldn't start its next run. {why}", &[("why", why)]), true);
        self.end_loop(root, LoopEnd::Failed);
    }

    /// The loop on `root` is over: say why, in a toast and the project's conversation.
    pub(crate) fn end_loop(&mut self, root: &Path, why: LoopEnd) {
        if self.loops.remove(root).is_none() {
            return;
        }
        let words = end_words(why);
        self.toast(words.clone(), matches!(why, LoopEnd::Failed));
        if why == LoopEnd::Stopped {
            return;
        }
        let item = FeedItem::Step { step: Step::Done, text: words };
        if self.workspace.as_ref().is_some_and(|w| w.root == root) {
            self.state.feed.push(item.clone());
        }
        // A run going saves its own conversation (and keeps this when it next saves).
        if !self.runs.is_running(root)
            && let Ok(ws) = Workspace::open(root)
        {
            let mut items = feed::load(&ws);
            items.push(item);
            if let Err(e) = feed::save(root, &items) {
                log::warn!("the loop's end wasn't saved in the conversation: {}", e.message);
            }
        }
    }

    /// One of a loop's runs ended: pause or a failure ends the loop, else its progress is read
    /// and the next run starts.
    pub(crate) fn loop_run_ended(&mut self, root: &Path, ok: bool, paused: bool) {
        let Some(l) = self.loops.get_mut(root) else { return };
        if let Some(why) = l.ending.take() {
            return self.end_loop(root, why);
        }
        if paused {
            return self.end_loop(root, LoopEnd::Stopped);
        }
        if !ok {
            return self.end_loop(root, LoopEnd::Failed);
        }
        l.between = true;
        self.read_loop_progress(root, true);
    }

    /// A limit reached mid-run (the stop time, the token budget) stops the run now. Called each
    /// frame; asks for a frame in time for the next stop time.
    pub(crate) fn check_loops(&mut self) {
        if self.loops.is_empty() {
            return;
        }
        let now = now_ms();
        let over: Vec<(PathBuf, LoopEnd)> = self
            .loops
            .iter()
            .filter(|(_, l)| !l.between && l.ending.is_none())
            .filter_map(|(root, l)| l.state.over_limit_now(now).map(|why| (root.clone(), why)))
            .collect();
        for (root, why) in over {
            match self.runs.stop(&root) {
                Stopped::Stopping => {
                    if let Some(l) = self.loops.get_mut(&root) {
                        l.ending = Some(why);
                    }
                }
                Stopped::Unqueued | Stopped::Nothing => self.end_loop(&root, why),
            }
        }
        let next = self.loops.values().filter_map(|l| l.state.deadline_ms).min();
        if let (Some(at), Some(ctx)) = (next, self.jobs.repaint_ctx()) {
            ctx.request_repaint_after(Duration::from_millis(at.saturating_sub(now)).min(CLOCK_CHECK));
        }
    }

    /// The person sent a message on `root` while its loop is switched on: the run it starts is
    /// the loop's first.
    pub(crate) fn begin_loop_if_on(&mut self, root: &Path, settings: Option<&LoopSettings>) {
        if let Some(s) = settings.filter(|s| s.on)
            && !self.loops.contains_key(root)
        {
            self.begin_loop(root, s.clone());
        }
    }

    /// A message meant to start a loop's first run didn't start one: there's no loop.
    pub(crate) fn drop_loop_unless_running(&mut self, root: &Path) {
        if !self.runs.is_running(root) && self.runs.position(root).is_none() {
            self.loops.remove(root);
        }
    }

    /// "Start now": a loop on the open project, starting with a fresh builder.
    fn start_loop_now(&mut self, settings: LoopSettings) {
        let Some(root) = self.workspace.as_ref().map(|w| w.root.clone()) else { return };
        if self.runs.is_running(&root) || self.runs.position(&root).is_some() {
            return;
        }
        self.begin_loop(&root, settings);
        self.next_loop_run(&root);
    }
}

/// The popover the settings button beside Send opens: the loop's switch, what each run is told,
/// and the limits that end it.
pub(crate) fn popover(app: &mut CraftApp, button: &egui::Response) {
    let Some(ws) = &app.workspace else { return };
    let root = ws.root.clone();
    let saved = ws.manifest.build_loop.clone().unwrap_or_default();
    let draft = match app.loop_draft.take() {
        Some(d) if d.root == root && d.base == saved => d,
        _ => LoopDraft { root: root.clone(), settings: saved.clone(), base: saved.clone(), sent: None },
    };
    let mut draft = draft;
    let busy = app.runs.is_running(&root) || app.runs.position(&root).is_some();
    let status = app.loops.get(&root).map(|l| if l.between { tr("Starting the next run…") } else { status_words(&l.state) });
    let was_on = draft.settings.on;
    let mut editing = false;
    let mut start_now = false;
    let mut end_now = false;
    let t = Tokens::get(&button.ctx);
    let popup = egui::Popup::from_toggle_button_response(button).id(egui::Id::new("build-loop"));
    let shown = popup.close_behavior(egui::PopupCloseBehavior::CloseOnClickOutside).show(|ui| {
        ui.set_width(360.0);
        let s = &mut draft.settings;
        ui.checkbox(&mut s.on, RichText::new(tr("Loop the build")).strong());
        let note = tr("When a run ends well, a fresh builder (a clean context) carries on from the last one's handoff, until a limit below ends the loop.");
        ui.add(egui::Label::new(RichText::new(note).size(theme::SMALL).color(t.text_faint)).wrap());
        ui.add_space(6.0);
        ui.label(RichText::new(tr("Each run is told")).size(theme::SMALL).color(t.text_dim));
        let prompt = ui.add(egui::TextEdit::multiline(&mut s.prompt).hint_text(DEFAULT_LOOP_PROMPT).desired_rows(4).desired_width(f32::INFINITY));
        editing |= prompt.has_focus();
        if !s.prompt.trim().is_empty() && widgets::link(ui, "Use the built-in prompt").clicked() {
            s.prompt.clear();
        }
        ui.add_space(6.0);
        egui::Grid::new("build-loop-limits").num_columns(2).spacing([10.0, 6.0]).show(ui, |ui| {
            ui.label(RichText::new(tr("Rounds per run")).size(theme::SMALL));
            let r = ui.add(egui::DragValue::new(&mut s.rounds_per_run).range(1..=MAX_ROUNDS_PER_RUN));
            editing |= r.has_focus() || r.dragged();
            ui.end_row();

            ui.checkbox(&mut s.runs.on, RichText::new(tr("Stop after")).size(theme::SMALL));
            ui.horizontal(|ui| {
                let r = ui.add_enabled(s.runs.on, egui::DragValue::new(&mut s.runs.value).range(1..=10_000));
                editing |= r.has_focus() || r.dragged();
                ui.label(RichText::new(tr("runs")).size(theme::SMALL));
            });
            ui.end_row();

            ui.checkbox(&mut s.tokens.on, RichText::new(tr("Token budget")).size(theme::SMALL));
            ui.horizontal(|ui| {
                // In millions: a run uses hundreds of thousands.
                let mut millions = s.tokens.value as f64 / 1_000_000.0;
                let r = ui.add_enabled(s.tokens.on, egui::DragValue::new(&mut millions).range(0.1..=10_000.0).speed(0.1).max_decimals(1));
                if r.changed() {
                    s.tokens.value = ((millions * 10.0).round() as u64).max(1) * 100_000;
                }
                editing |= r.has_focus() || r.dragged();
                ui.label(RichText::new(tr("million tokens")).size(theme::SMALL));
            });
            ui.end_row();

            ui.checkbox(&mut s.stop_at.on, RichText::new(tr("Stop at")).size(theme::SMALL));
            ui.horizontal(|ui| {
                let at = &mut s.stop_at.value;
                let hour = egui::DragValue::new(&mut at.hour)
                    .range(0..=23)
                    .custom_formatter(|h, _| ClockTime { hour: h as u8, minute: 0 }.words())
                    .custom_parser(parse_hour);
                let r = ui.add_enabled(s.stop_at.on, hour);
                editing |= r.has_focus() || r.dragged();
                let minute = egui::DragValue::new(&mut at.minute).range(0..=59).speed(0.25).custom_formatter(|m, _| format!(":{m:02}"));
                let r = ui.add_enabled(s.stop_at.on, minute);
                editing |= r.has_focus() || r.dragged();
                ui.label(RichText::new(at.words()).size(theme::SMALL).color(t.text_dim));
            });
            ui.end_row();

            ui.checkbox(&mut s.stalled_runs.on, RichText::new(tr("Stop after")).size(theme::SMALL));
            ui.horizontal(|ui| {
                let r = ui.add_enabled(s.stalled_runs.on, egui::DragValue::new(&mut s.stalled_runs.value).range(1..=100));
                editing |= r.has_focus() || r.dragged();
                ui.label(RichText::new(tr("runs in a row that change nothing")).size(theme::SMALL));
            });
            ui.end_row();
        });
        if s.on && !s.has_limit() {
            ui.add_space(4.0);
            let warn = tr("No limit is on: the loop runs until you pause it.");
            ui.add(egui::Label::new(RichText::new(warn).size(theme::SMALL).color(t.warning)).wrap());
        }
        ui.add_space(6.0);
        match &status {
            Some(line) => {
                ui.horizontal(|ui| {
                    ui.label(RichText::new(line).size(theme::SMALL).color(t.text_dim));
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        end_now = widgets::secondary(ui, None, "End the loop").on_hover_text(tr("The run going now finishes; no new one starts.")).clicked();
                    });
                });
            }
            None if s.on && !busy => {
                ui.horizontal(|ui| {
                    let hint = tr("It starts with your next message, or now.");
                    ui.label(RichText::new(hint).size(theme::SMALL).color(t.text_faint));
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        start_now = widgets::primary_icon(ui, Some("play"), "Start now", true).clicked();
                    });
                });
            }
            None => {}
        }
    });
    let open = shown.is_some();
    let settings = draft.settings.clone().sanitized();
    // Saved once nothing is being typed or dragged, or as the popover closes.
    if settings != draft.base && draft.sent.as_ref() != Some(&settings) && (!editing || !open) {
        draft.sent = Some(settings.clone());
        app.act("project_loop_set", save_params(&root, &settings), tr("Save the build loop's settings"));
    }
    let now_on = settings.on;
    app.loop_draft = Some(draft);
    if was_on && !now_on || end_now {
        app.end_loop(&root, LoopEnd::Stopped);
    } else if !was_on && now_on && busy {
        // Switched on while a build works: that run is the loop's first.
        app.begin_loop(&root, settings.clone());
    }
    if start_now {
        app.start_loop_now(settings);
    }
}

/// What the popover sends to save a project's loop: its settings, and the project they're for.
fn save_params(root: &Path, settings: &LoopSettings) -> serde_json::Value {
    let mut params = serde_json::to_value(settings).unwrap_or_default();
    params["project"] = serde_json::json!(root);
    params
}

/// "7am", "4pm", "16" or "7" for the hour field.
fn parse_hour(s: &str) -> Option<f64> {
    let s = s.trim().to_ascii_lowercase();
    let (n, half) = match s.strip_suffix("am").or_else(|| s.strip_suffix('a')) {
        Some(n) => (n, Some(false)),
        None => match s.strip_suffix("pm").or_else(|| s.strip_suffix('p')) {
            Some(n) => (n, Some(true)),
            None => (s.as_str(), None),
        },
    };
    let h: u8 = n.trim().split(':').next()?.trim().parse().ok()?;
    let h = match half {
        Some(pm) if (1..=12).contains(&h) => (h % 12) + if pm { 12 } else { 0 },
        Some(_) => return None,
        None if h <= 23 => h,
        None => return None,
    };
    Some(f64::from(h))
}

/// Why a loop ended, in the person's language (`LoopEnd::words` is the CLI's).
fn end_words(why: LoopEnd) -> String {
    match why {
        LoopEnd::Runs(1) => tr("The loop ended: it did its 1 run."),
        LoopEnd::Runs(n) => trf("The loop ended: it did its {n} runs.", &[("n", &n.to_string())]),
        LoopEnd::Tokens(n) => trf("The loop ended: its runs used {n} tokens, its budget.", &[("n", &tokens_words(n))]),
        LoopEnd::Time(t) => trf("The loop ended: it's {time}, its stop time.", &[("time", &t.words())]),
        LoopEnd::Stalled(1) => tr("The loop ended: a run changed nothing."),
        LoopEnd::Stalled(n) => trf("The loop ended: {n} runs in a row changed nothing.", &[("n", &n.to_string())]),
        LoopEnd::Failed => tr("The loop ended: a run didn't finish well."),
        LoopEnd::Stopped => tr("The loop stopped."),
    }
}

/// Where a loop is, for the popover: "Run 3 of 10 · 1.2M tokens · until 7am".
fn status_words(state: &LoopState) -> String {
    let run = state.runs.saturating_add(1).to_string();
    let mut parts = vec![match state.settings.runs.get() {
        Some(n) => trf("Run {run} of {n}", &[("run", &run), ("n", &n.to_string())]),
        None => trf("Run {run}", &[("run", &run)]),
    }];
    parts.push(match state.settings.tokens.get() {
        _ if state.tokens_unreported => tr("tokens not reported by this builder"),
        Some(b) => trf("{used} of {budget} tokens", &[("used", &tokens_words(state.tokens)), ("budget", &tokens_words(b))]),
        None => trf("{used} tokens", &[("used", &tokens_words(state.tokens))]),
    });
    if let Some(at) = state.settings.stop_at.get().filter(|_| state.deadline_ms.is_some()) {
        parts.push(trf("until {time}", &[("time", &at.words())]));
    }
    parts.join(" · ")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_hour_reads_the_way_people_type_it() {
        let p = |s| parse_hour(s);
        assert_eq!(
            [p("7am"), p("7"), p("12am"), p("12pm"), p("4:45pm"), p("16"), p(" 9 p ")],
            [Some(7.0), Some(7.0), Some(0.0), Some(12.0), Some(16.0), Some(16.0), Some(21.0)]
        );
        assert_eq!([p("13pm"), p("24"), p("x")], [None, None, None]);
    }

    /// The popover saves the fields it edits through `project_loop_set`, and the engine reads each
    /// one back. A field the save drops or renames would come back as its default here.
    #[test]
    fn the_settings_the_popover_edits_reach_the_engine() {
        use craft_anything_engine::workspace::TargetRef;
        use craft_anything_engine::workspace::build_loop::Limit;
        use craft_anything_engine::{CallOptions, Caller, Engine};
        let d = tempfile::tempdir().unwrap();
        let ws = Workspace::create(d.path(), "Notes", "A notes app", TargetRef::None).unwrap();
        let settings = LoopSettings {
            on: true,
            prompt: "Keep the app working.".into(),
            rounds_per_run: 3,
            runs: Limit { on: true, value: 4 },
            tokens: Limit { on: true, value: 200_000 },
            stop_at: Limit { on: true, value: ClockTime { hour: 7, minute: 30 } },
            stalled_runs: Limit { on: false, value: 2 },
        };
        let engine = Engine::new(craft_anything_engine::testing::session());
        engine.call("project_loop_set", save_params(&ws.root, &settings), CallOptions::new(Caller::Ui)).unwrap();
        let saved = Workspace::open(&ws.root).unwrap();
        assert_eq!(saved.manifest.build_loop, Some(settings.clone().sanitized()), "every field the popover edits is saved");
        assert_eq!(LoopState::start(settings.clone(), 0, 0).settings, settings.clone().sanitized(), "and a loop started from them uses them");
    }
}
