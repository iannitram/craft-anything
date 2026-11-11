//! The rebuild workflow: study a target, have a coding agent build it in the workspace's `app/`
//! folder, check the build against the target, and refine — asking the person when needed.
//!
//! ```text
//! Study   website: web_site_open + web_site_capture → capture/   (screenshots, blueprint, HTML)
//!         local:   triage (+ app graph for JS apps)  → capture/README.md
//! Build   the coding agent (an agent CLI such as Claude Code) works in the workspace with our
//!         MCP server; edits outside app/ and unusual commands ask the person (core_action_approve),
//!         questions arrive as cards (core_person_ask). Both go through the workspace inbox.
//! Check   website: serve app/dist on loopback, capture it the same way, compare screenshots
//!         (layout similarity per width). Other stacks: the build must succeed.
//! Refine  measured differences go back to the agent, up to `max_rounds`.
//! ```
//!
//! Everything runs on one worker thread; the UI drains [`RunEvent`]s with [`RebuildRun::poll`].
//!
//! Modules: `kinds` (what is made: [`TargetKind`]s and [`Check`]s), `study`, `brief` (the builder's prompts), `check`,
//! `locate` (what to study in a local target) and `output` (what a build produced).

use crate::cli_agents::{Builder, CliSpec};
use crate::{AgentEvent, AiHub, BackendKind, Conversation, Role};
use craft_anything_engine::inbox::{Answer, Inbox, InboxDir, Request};
use craft_anything_engine::project::{Project, Purpose};
use craft_anything_engine::questions::AskedQuestion;
use craft_anything_engine::workspace::progress::{AppFingerprintCache, BuildProgress};
use craft_anything_engine::workspace::{CheckResult, ProjectUsage, Status, TargetRef, Workspace};
use craft_anything_engine::{CallOptions, Caller, CancelToken, Engine, ToolOutput};
use craft_anything_engines::game;
use craft_anything_evidence::{ErrorCode, ToolError};
pub use craft_anything_platform::serve::StaticServer;
use craft_anything_platform::serve::serve_dir;
use craft_anything_platform::sync::Mutex;
use serde_json::{Value, json};
use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::mpsc::{Receiver, Sender};
use std::sync::{Arc, PoisonError};
use std::time::{Duration, Instant};

pub mod activity;
mod brief;
mod check;
pub mod feed;
mod kept_check;
pub mod kinds;
mod locate;
pub mod notes;
mod output;
pub mod project;
pub mod question;
pub mod runs;
pub mod self_check;
pub mod stacks;
pub mod stand_in;
mod study;
#[cfg(test)]
mod tests;
pub mod tools;

use brief::*;
pub use kinds::{
    AppKind, BriefCx, BuildCheck, Check, CheckCx, CheckStop, DetectCx, GameKind, KindRegistry, KindWords, MechanicsCheck, TargetKind, WebVisualCheck,
    WebsiteKind,
};
pub use locate::game_folder;
use locate::*;
use output::*;
pub use output::{built_program, launch, site_dir};
pub use runs::{RunSet, Started, Stopped, keep_queue_moving};
pub use stand_in::StandIns;

pub use craft_anything_engine::workspace::Step;

/// What the UI shows, in order.
#[derive(Debug, Clone, PartialEq)]
pub enum RunEvent {
    /// A stage started (with a plain-words line on what's happening).
    Step { step: Step, text: String },
    /// Something from the coding agent (text, tool calls, errors).
    Agent(AgentEvent),
    /// A note the person added while it worked, now read by the agent (as the feed shows it),
    /// with the pictures attached to it (their copies in the project).
    Note { shown: String, pictures: Vec<PathBuf> },
    /// A question or approval waiting for the person.
    Ask(Request),
    /// A check finished.
    Checked(CheckResult),
    /// The build can be previewed here.
    Preview(String),
    Finished {
        ok: bool,
        message: String,
        /// The person paused it: not a failure.
        paused: bool,
    },
}

/// Called after each event a run sends, so a window draws it at once instead of checking on a
/// timer.
#[derive(Clone, Default)]
pub struct Wake(Option<Arc<dyn Fn() + Send + Sync>>);

impl Wake {
    pub fn new(f: impl Fn() + Send + Sync + 'static) -> Wake {
        Wake(Some(Arc::new(f)))
    }

    fn call(&self) {
        if let Some(f) = &self.0 {
            f();
        }
    }
}

impl std::fmt::Debug for Wake {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(if self.0.is_some() { "Wake(set)" } else { "Wake(none)" })
    }
}

/// Options for one run.
#[derive(Debug, Clone)]
pub struct RunOptions {
    /// The agent backend by name; `None` picks the Builder role, else the first agent app.
    pub backend: Option<String>,
    /// A follow-up request on an existing build (skips Study).
    pub message: Option<String>,
    /// How the person's message is shown in the project's feed (it can differ from `message`,
    /// which carries attachment paths). `None` adds nothing to the feed.
    pub shown: Option<String>,
    /// The pictures attached to `shown` (their copies in the project), so the feed keeps them
    /// with the message for Up to bring back.
    pub shown_pictures: Vec<PathBuf>,
    /// Pick up a build whose last run stopped part-way (paused, the app closed, the computer
    /// stopped): Study isn't repeated when it was done, and an agent with a saved session is
    /// told to carry on rather than given the brief again. A run with no message on a project
    /// whose builder has already worked does the same, so "Try again" never starts over.
    pub continuing: bool,
    /// Refinement rounds after the first build (each fixes what the last check found).
    pub max_rounds: u32,
    /// Stop refining once the build looks at least this alike (percent).
    pub good_enough: f64,
    pub beginner: bool,
    /// The kinds of thing this run can make (website, app, game, and any a fork registers).
    pub kinds: KindRegistry,
    /// How each coding agent is driven (Claude Code, Codex, Gemini CLI, and any a fork registers).
    pub adapters: crate::cli_agents::AgentAdapters,
    /// Told after each event (the app repaints).
    pub wake: Wake,
    /// The run's mode when the caller knows it and the session may be busy (a tool starts a run).
    /// A free session is read first; this is the fallback while it is held; the saved settings
    /// are the last fallback.
    pub clean_room: Option<bool>,
}

impl Default for RunOptions {
    fn default() -> Self {
        RunOptions {
            backend: None,
            message: None,
            shown: None,
            shown_pictures: Vec::new(),
            continuing: false,
            max_rounds: 6,
            good_enough: 90.0,
            beginner: true,
            kinds: KindRegistry::built_in(),
            adapters: Default::default(),
            wake: Wake::default(),
            clean_room: None,
        }
    }
}

/// Something the person added while a run works, until the agent reads it.
#[derive(Debug, Clone, PartialEq)]
pub struct Note {
    /// Unique in this process, so the app can take a note back.
    pub id: u64,
    /// What the agent reads.
    pub text: String,
    /// How it appears in the project's feed.
    pub shown: String,
    /// The pictures attached to it (their copies in the project), for the feed's history.
    pub pictures: Vec<PathBuf>,
    /// The Just-ask answers the note's hand-over file carries (see [`RebuildRun::add_note`]), so
    /// they're marked told when the builder reads the note or when the after-turn path sends it.
    /// The raw note text stays in [`text`](Self::text), so a later run filters them by its mode.
    pub questions: Vec<AskedQuestion>,
}

/// The note text with the latest Just-ask answers this mode may show. `skip` holds answers an
/// earlier waiting note already carries, so two notes don't repeat the same ones.
fn note_with_just_asked(text: String, ws: &Workspace, clean_room: bool, skip: &[AskedQuestion]) -> (String, Vec<AskedQuestion>) {
    let asked: Vec<AskedQuestion> =
        craft_anything_engine::questions::untold(ws, !clean_room).into_iter().filter(|q| !skip.iter().any(|s| same_question(s, q))).collect();
    if asked.is_empty() {
        return (text, asked);
    }
    (format!("{}{text}", just_asked(&asked)), asked)
}

fn same_question(a: &AskedQuestion, b: &AskedQuestion) -> bool {
    a.at_ms == b.at_ms && a.question == b.question
}

/// A conversation the project kept: in this run's mode, in the other mode, or both.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
struct KeptHistory {
    /// A conversation is kept for this run's mode.
    in_mode: bool,
    /// A conversation is kept for the other mode (a saved session or a transcript).
    elsewhere: bool,
}

/// Whether a run without a new request is carrying on work already under way in this mode.
/// `history_in_mode` says the project has a conversation kept for this mode (an API transcript or
/// a coding agent's session); one from the other mode isn't carried on, so the run studies again.
fn is_carrying_on(opts: &RunOptions, history_in_mode: bool) -> bool {
    history_in_mode && (opts.continuing || opts.message.is_none())
}

/// Whether the project's `capture/` was made in this run's mode. A capture with no recorded mode
/// (a failed study, or one copied in) isn't usable: the run studies again. A capture from before
/// the mode was kept was recorded as Deep by the version-5 migration, like a saved session.
pub(super) fn capture_in_mode(ws: &Workspace, deep: bool) -> bool {
    ws.capture_dir().exists() && ws.manifest.capture_deep == Some(deep)
}

/// The mode a run starts in: the session's when it can be read, else the run's own choice, else
/// the saved setting. Shared with the queue, so it and the run agree about the capture.
pub(super) fn run_deep_mode(hub: &AiHub, opts: &RunOptions) -> bool {
    !hub.engine
        .try_session()
        .map(|s| s.clean_room())
        .or(opts.clean_room)
        .unwrap_or_else(|| !craft_anything_engine::Settings::load_from(&hub.settings_path()).deep_mode)
}

/// Whether a run starts by studying: its capture isn't from this mode, or the project's kept
/// conversation is for the other mode. A capture from the other mode must not be handed to this
/// run's builder. A follow-up with no conversation at all studies when the capture isn't usable;
/// with a usable one it keeps the old no-study path.
fn starts_with_study(opts: &RunOptions, kept: KeptHistory, usable_capture: bool) -> bool {
    !usable_capture || (!kept.in_mode && (opts.message.is_none() || kept.elsewhere))
}

/// A running (or finished) rebuild.
pub struct RebuildRun {
    rx: Receiver<RunEvent>,
    stop: CancelToken,
    inbox: Inbox,
    running: Arc<AtomicBool>,
    preview: Arc<Mutex<Option<StaticServer>>>,
    /// What the person added while it works; the agent reads them at its next step.
    notes: Arc<Mutex<Vec<Note>>>,
    /// The mode this run started in, so a note carries only the Just-ask answers that mode may see.
    clean_room: bool,
    /// The project's conversation, kept in its folder.
    feed: Arc<Mutex<feed::FeedLog>>,
    pub root: PathBuf,
    started: Instant,
    activity: Arc<activity::Activity>,
}

impl RebuildRun {
    /// Start a run on a worker thread. Fails fast (before any work) when no coding agent is set up.
    pub fn start(hub: &AiHub, mut ws: Workspace, opts: RunOptions) -> Result<RebuildRun, ToolError> {
        let cfg = hub.config();
        let entry = builder_backend(&cfg, opts.backend.as_deref().or_else(|| project_builder(&cfg, &ws)))?;
        // The person's choice of mode is a session setting. A run that waited its turn uses the mode
        // now, not the one when it was queued; a tool holds the session while it starts a run, so it
        // passes the mode instead, and the saved settings are the last fallback.
        let run_deep = run_deep_mode(hub, &opts);
        let clean_room = !run_deep;
        // A study is only carried on in the mode it was made in. A session kept in the other mode
        // may have read inside the original, and its capture must not be handed to this run.
        let saved_in_mode = ws.builder_session_in(run_deep);
        let transcript_in_mode = crate::context::load(&ws.transcript_path(), run_deep).is_some();
        let kept_without_api = KeptHistory {
            in_mode: saved_in_mode || transcript_in_mode,
            elsewhere: (ws.manifest.agent_session.is_some() && !saved_in_mode) || (ws.transcript_path().is_file() && !transcript_in_mode),
        };
        // An API backend builds with our own agent loop and the Build tools; its conversation is
        // kept in the project and continued by follow-ups (a fresh build starts a new one).
        let api = match entry.config.kind {
            BackendKind::AgentCli => None,
            _ => {
                // A run carrying on keeps its transcript, so `ApiConversation::resumed` can tell
                // the run which conversation it continues. Only a genuinely fresh build (no
                // continuation and no study kept in this mode) starts a new one.
                let continuing = opts.continuing || (opts.message.is_none() && kept_without_api.in_mode);
                if !continuing && !ws.capture_dir().exists() {
                    let _ = std::fs::remove_file(ws.transcript_path());
                }
                Some(hub.start_builder(&entry, opts.beginner, Some(ws.transcript_path()), run_deep)?)
            }
        };
        // A chosen extractor (an API backend, e.g. Kimi K3) takes a game's files out after a fresh
        // study; the run only uses it in Deep mode on a game (see Worker::extract).
        let fresh = opts.message.is_none() || !capture_in_mode(&ws, run_deep);
        let extractor = match hub.chosen(Role::Extractor).and_then(|n| hub.config().get(&n).cloned()) {
            Some(e) if fresh && e.config.kind != BackendKind::AgentCli && !clean_room => {
                let _ = std::fs::remove_file(ws.root.join(EXTRACT_TRANSCRIPT));
                Some((e.config.name.clone(), hub.start_extractor(&e, opts.beginner, Some(ws.root.join(EXTRACT_TRANSCRIPT)))?))
            }
            _ => None,
        };
        let (tx, rx) = std::sync::mpsc::channel();
        let stop = CancelToken::default();
        let running = Arc::new(AtomicBool::new(true));
        let preview: Arc<Mutex<Option<StaticServer>>> = Arc::default();
        let notes: Arc<Mutex<Vec<Note>>> = Arc::default();
        let inbox = Inbox::new(ws.inbox_dir());
        // Notes an earlier run never handed over (the app closed while they waited, #1620) wait for
        // this run's builder; their old files go before the app can add any to this one.
        let left = notes::take_left(&notes::notes_dir(&ws.inbox_dir()), opts.message.is_none());
        if !left.is_empty() {
            left_notes_are_a_request(&mut ws);
        }
        let root = ws.root.clone();
        // The project's conversation: the run keeps it in the folder as it goes.
        let feed = Arc::new(Mutex::new(feed::FeedLog::open(&ws)));
        if let Some(shown) = opts.shown.clone() {
            feed.lock().unwrap_or_else(PoisonError::into_inner).push(feed::FeedItem::User(feed::UserItem::new(shown, opts.shown_pictures.clone())));
        }
        let ctx = Worker {
            wake: opts.wake.clone(),
            engine: hub.engine.clone(),
            tx,
            stop: stop.clone(),
            preview: preview.clone(),
            seen: HashSet::new(),
            inbox: inbox.clone(),
            editor: None,
            notes: notes.clone(),
            feed: feed.clone(),
            assets: false,
            usage: Some(UsageClock { root: root.clone(), since: Instant::now() }),
            self_check: None,
            reset_progress_after_notes: false,
            keys: hub.keys.clone(),
            kept_checks: Mutex::default(),
        };
        let r2 = running.clone();
        let key_store = hub.keys.clone();
        std::thread::Builder::new()
            .name("rebuild".into())
            .spawn(move || {
                let mut ctx = ctx;
                let mut ws = ws;
                // A clone can run for hours: keep the computer from sleeping until it ends.
                let awake = craft_anything_platform::keep_awake();
                let _here = self_check::BuildingHere::new(&ws.root);
                let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                    ctx.load_asset_keys(&key_store);
                    ctx.run(&mut ws, entry, api, extractor, kept_without_api, &opts)
                }));
                let (ok, message, paused) = run_outcome(result, &mut ws);
                ctx.record_usage(None);
                // The CLI can exit as soon as it sees completion: release the helper while its
                // supervisor threads are still available to stop it.
                drop(awake);
                ctx.emit(RunEvent::Finished { ok, message, paused });
                r2.store(false, Ordering::SeqCst);
            })
            .map_err(|e| ToolError::internal(e.to_string()))?;
        let activity = feed.lock().unwrap_or_else(PoisonError::into_inner).activity();
        let run = RebuildRun { rx, stop, inbox, running, preview, notes, clean_room, feed, root, started: Instant::now(), activity };
        // Waiting again with files of their own: the builder takes them at its next step or after
        // its turn, and a run that ends first hands them to the next one, as with any note.
        for n in left {
            run.add_note(n.text, n.shown, n.pictures);
        }
        Ok(run)
    }

    /// Add something while it works; the agent reads it at its next step, and only then does it
    /// join the feed (as `shown`, with `pictures`). Returns its id, to take it back with `unsend`
    /// until then.
    ///
    /// The hand-over file also carries the latest Just-ask answers this mode may show, so a
    /// hand-over that bypasses the next message (Claude Code's tool hook) still has the context.
    /// The in-memory text stays raw, and the answers are marked told only when the note is read or
    /// sent.
    pub fn add_note(&self, text: String, shown: String, pictures: Vec<PathBuf>) -> u64 {
        static NEXT: AtomicU64 = AtomicU64::new(1);
        let id = NEXT.fetch_add(1, Ordering::Relaxed);
        let mut file_text = text.clone();
        let mut questions = Vec::new();
        if !text.trim().is_empty() {
            // A note already waiting carries what it carries; this one adds only what's new.
            let waiting: Vec<AskedQuestion> = {
                let notes = self.notes.lock().unwrap_or_else(PoisonError::into_inner);
                notes.iter().flat_map(|n| n.questions.iter().cloned()).collect()
            };
            if let Ok(ws) = Workspace::open(&self.root) {
                (file_text, questions) = note_with_just_asked(file_text, &ws, self.clean_room, &waiting);
            }
        }
        // Also left for an agent that takes notes at its next step; without it, after the turn.
        // The in-memory note keeps the raw text: if a later run takes it, that run filters the
        // answers by its own mode instead of inheriting this run's.
        let left = notes::Left { text: text.clone(), shown: shown.clone(), pictures: pictures.clone() };
        if let Err(e) = notes::hand_over(&notes::notes_dir(self.inbox.dir()), id, &file_text, Some(&left)) {
            log::warn!("A note waits for the builder's turn to end: {}", e.message);
        }
        self.notes.lock().unwrap_or_else(PoisonError::into_inner).push(Note { id, text, shown, pictures, questions });
        id
    }

    /// The notes still waiting for the agent, oldest first.
    pub fn waiting(&self) -> Vec<Note> {
        self.notes.lock().unwrap_or_else(PoisonError::into_inner).clone()
    }

    /// Take back a note the agent hasn't read yet; `None` once it has.
    pub fn unsend(&self, id: u64) -> Option<Note> {
        let mut notes = self.notes.lock().unwrap_or_else(PoisonError::into_inner);
        let i = notes.iter().position(|n| n.id == id)?;
        let dir = notes::notes_dir(self.inbox.dir());
        // Taking its file back first means the agent can't claim it after this.
        if !notes::take_back(&dir, id) && notes::was_read(&dir, id) {
            return None;
        }
        Some(notes.remove(i))
    }

    /// How many notes are still waiting for the agent.
    pub fn queued(&self) -> usize {
        self.notes.lock().unwrap_or_else(PoisonError::into_inner).len()
    }

    /// Notes the agent never got to (the run ended first), to send as the next message. Their
    /// raw text leaves the Just-ask answers to the next run's own mode and message.
    pub fn take_notes(&self) -> Vec<Note> {
        std::mem::take(&mut *self.notes.lock().unwrap_or_else(PoisonError::into_inner))
    }

    /// Notes a project's earlier run left waiting on disk, when no run of this process has them
    /// (the app was closed and opened again), oldest first, taken off the disk. A message sent to
    /// the project carries them: the run it starts clears the folder.
    pub fn take_left_notes(ws: &Workspace) -> Vec<Note> {
        notes::take_left(&notes::notes_dir(&ws.inbox_dir()), true)
            .into_iter()
            .map(|l| Note { id: 0, text: l.text, shown: l.shown, pictures: l.pictures, questions: Vec::new() })
            .collect()
    }

    pub fn poll(&mut self) -> Vec<RunEvent> {
        self.rx.try_iter().collect()
    }

    /// The project's conversation as the run has it now, for showing a project that is already
    /// building. Events not yet polled are in it, so they're dropped from the queue.
    pub fn feed_now(&mut self) -> Vec<feed::FeedItem> {
        let feed = self.feed.lock().unwrap_or_else(PoisonError::into_inner);
        while self.rx.try_recv().is_ok() {}
        feed.items().to_vec()
    }

    /// Answer a question or approval card.
    pub fn answer(&self, id: &str, answer: &Answer) -> Result<(), ToolError> {
        self.answer_as(id, answer, &answer.as_text())
    }

    /// Answer a card; `shown` is how the answer appears in the project's feed.
    pub fn answer_as(&self, id: &str, answer: &Answer, shown: &str) -> Result<(), ToolError> {
        self.inbox.answer(id, answer)?;
        self.feed.lock().unwrap_or_else(PoisonError::into_inner).answered(id, shown);
        Ok(())
    }

    pub fn stop(&self) {
        self.stop.cancel();
    }

    pub fn is_running(&self) -> bool {
        self.running.load(Ordering::SeqCst)
    }

    /// When it began working (a queued run begins when its turn comes).
    pub fn started(&self) -> Instant {
        self.started
    }

    /// How long since the run last had anything to say. Cheap: safe to read while drawing.
    pub fn quiet_for(&self) -> Duration {
        self.activity.quiet_for()
    }

    /// What it's on now. Cheap: safe to read while drawing.
    pub fn activity_now(&self) -> activity::ActivityNow {
        self.activity.now()
    }

    /// How many lines of its activity are at hand for [`RebuildRun::activity_lines`].
    pub fn activity_len(&self) -> usize {
        self.activity.len()
    }

    /// Some of its latest activity. Cheap: safe to read while drawing.
    pub fn activity_lines(&self, range: std::ops::Range<usize>) -> Vec<activity::ActivityLine> {
        self.activity.lines_in(range)
    }

    /// The file with all of it.
    pub fn activity_log(&self) -> PathBuf {
        self.root.join(craft_anything_engine::workspace::ACTIVITY_LOG)
    }

    pub fn preview_url(&self) -> Option<String> {
        self.preview.lock().unwrap_or_else(PoisonError::into_inner).as_ref().map(StaticServer::url)
    }
}

impl Drop for RebuildRun {
    fn drop(&mut self) {
        self.stop.cancel();
    }
}

/// Serve a workspace's built site for previewing (without running the workflow).
pub fn preview(ws: &Workspace) -> Result<StaticServer, ToolError> {
    let site = site_dir(&ws.app_dir()).ok_or_else(|| {
        ToolError::new(ErrorCode::NotFound, "There's no built website to show yet.").with_remediation("Ask for a rebuild, or open the project folder.")
    })?;
    serve_dir(&site).map_err(|e| ToolError::io(site.display(), e))
}

/// Deep mode on a program requires decompiling, so a build turn that ends without the builder
/// ever having decompiled in this project gets [`DECOMPILE_FIRST`] (once per run). A decompile in
/// an earlier run counts: each request from the person starts a new run.
fn needs_decompile_reminder(deep: bool, manifest: &craft_anything_engine::workspace::Manifest) -> bool {
    deep && matches!(manifest.target, TargetRef::Local { .. }) && !manifest.decompiled
}

struct Worker {
    /// Told after each event.
    wake: Wake,
    engine: Arc<Engine>,
    tx: Sender<RunEvent>,
    stop: CancelToken,
    preview: Arc<Mutex<Option<StaticServer>>>,
    seen: HashSet<String>,
    inbox: Inbox,
    /// The game engine's editor for this run (Unity, Unreal or Godot stacks).
    editor: Option<PathBuf>,
    notes: Arc<Mutex<Vec<Note>>>,
    feed: Arc<Mutex<feed::FeedLog>>,
    /// An asset provider's key is set up, so the builder can generate pictures and 3D models if the
    /// project allows it.
    assets: bool,
    /// Where this run adds its working time, tokens and cost (`None` in tests that don't build).
    usage: Option<UsageClock>,
    /// The kind and bar the builder's own checks (`rebuild_check`) are judged by, once known.
    self_check: Option<(Arc<dyn kinds::TargetKind>, f64)>,
    /// A hook consumed a new request; persist its reset only after the builder finishes.
    reset_progress_after_notes: bool,
    /// The backends' stored keys, for an agent CLI that asks the host to authenticate (Devin).
    keys: crate::keys::KeyStore,
    /// The last check round in which everything worked, kept while nothing it looks at changes.
    kept_checks: Mutex<kept_check::KeptChecks>,
}

/// The project a run records its usage in, and when it last did.
struct UsageClock {
    root: PathBuf,
    since: Instant,
}

/// How often a long agent turn saves the time worked so far, so a crash loses little of it.
const USAGE_FLUSH: Duration = Duration::from_secs(60);

/// Record `e` in the feed, send it and wake the window, under one lock.
fn emit_event(feed: &Mutex<feed::FeedLog>, tx: &Sender<RunEvent>, wake: &Wake, e: RunEvent) {
    let mut feed = feed.lock().unwrap_or_else(PoisonError::into_inner);
    feed.record(&e);
    let _ = tx.send(e);
    drop(feed);
    wake.call();
}

/// The sessions saved Claude Code `--resume`/`-r` arguments name, in order.
fn configured_resumes(args: &[String]) -> Vec<&str> {
    let mut out = Vec::new();
    let mut rest = args.iter();
    while let Some(arg) = rest.next() {
        if arg == "--resume" || arg == "-r" {
            if let Some(id) = rest.next() {
                out.push(id.as_str());
            }
        } else if let Some(id) = arg.strip_prefix("--resume=") {
            out.push(id);
        }
    }
    out
}

/// Whether saved Claude Code arguments ask for the most recent conversation (`-c`/`--continue`).
fn configured_continues(args: &[String]) -> bool {
    args.iter().any(|a| a == "--continue" || a == "-c")
}

/// Refuse saved Claude Code arguments that would continue the project's Deep session in Clean
/// room: a `--resume` of its id or a `--continue`/`-c` of the latest conversation bypasses the
/// mode filter and carries what that session read.
fn refuse_deep_session_reuse_in_clean_room(ws: &Workspace, model: &str, args: &[String], deep: bool) -> Result<(), ToolError> {
    if deep || !crate::cli_agents::adapter_for(model).ids().contains(&"claude-code") {
        return Ok(());
    }
    let Some(saved) = ws.manifest.agent_session_for(model).filter(|_| ws.manifest.agent_session_deep.unwrap_or(true)) else {
        return Ok(());
    };
    if configured_resumes(args).contains(&saved) {
        return Err(ToolError::new(
            ErrorCode::BadSettings,
            format!("The saved Claude Code arguments resume session {saved}, which was saved in Deep mode, so this Clean room run is refused: that session may have read inside the original."),
        )
        .with_remediation(format!("Remove `--resume {saved}` from the Claude Code backend's extra arguments (config.toml), or run this in Deep mode.")));
    }
    if configured_continues(args) {
        return Err(ToolError::new(
            ErrorCode::BadSettings,
            "The saved Claude Code arguments continue the most recent conversation, which may be this project's Deep session, so this Clean room run is refused: that session may have read inside the original.",
        )
        .with_remediation("Remove `--continue`/`-c` from the Claude Code backend's extra arguments (config.toml), or run this in Deep mode."));
    }
    Ok(())
}

impl Worker {
    /// Add the time worked since the last record (and an agent turn's tokens and reported cost)
    /// to the project's usage.json. A question can add its own cost at the same time; a failure is
    /// logged and the build carries on.
    fn record_usage(&mut self, turn: Option<(u64, u64, Option<f64>, bool)>) {
        let Some(clock) = self.usage.as_mut() else { return };
        let ms = u64::try_from(clock.since.elapsed().as_millis()).unwrap_or(u64::MAX);
        clock.since = Instant::now();
        let saved = ProjectUsage::update(&clock.root, |u| {
            u.work_ms = u.work_ms.saturating_add(ms);
            if let Some((input, output, cost, estimate)) = turn {
                u.add_turn(input, output, cost, estimate);
            }
        });
        if let Err(e) = saved {
            log::warn!("Couldn't save the project's usage in {}: {}", clock.root.display(), e.message);
        }
    }

    /// Recorded and sent under one lock, so [`RebuildRun::feed_now`] sees each event once.
    fn emit(&self, e: RunEvent) {
        emit_event(&self.feed, &self.tx, &self.wake, e);
    }

    fn step(&self, ws: &mut Workspace, step: Step, status: Status, text: &str) -> Result<(), ToolError> {
        self.stop.check()?;
        ws.set_status(status, text)?;
        self.emit(RunEvent::Step { step, text: text.to_string() });
        Ok(())
    }

    fn call(&self, tool: &str, params: Value) -> Result<ToolOutput, ToolError> {
        self.stop.check()?;
        // The person started this run for this target, which is the consent for opening it.
        let opts = CallOptions::new(Caller::Ui).with_consent(craft_anything_engine::Consent::Granted).with_cancel(self.stop.clone());
        // Said as it happens, so a long step shows what it's doing and the feed counts it.
        let call_id = format!("run-{tool}");
        self.emit(RunEvent::Agent(AgentEvent::ToolStarted { call_id: call_id.clone(), tool: tool.to_string(), input: Value::Null }));
        let result = self.engine.call(tool, params, opts);
        self.emit(RunEvent::Agent(AgentEvent::ToolFinished { call_id, tool: tool.to_string(), ok: result.is_ok(), summary: String::new(), evidence_id: None }));
        result
    }

    /// An API backend's builder runs its tools in this session, not in a tool server that loads the
    /// keys itself (#923). Read on the worker because the keychain can block, and for each run so a
    /// key saved outside the app since the last one counts. The session stays held through the
    /// read, so a key removed meanwhile can't be put back by keys read before it went. What the
    /// brief says about assets comes from the same read, so the two agree.
    fn load_asset_keys(&mut self, store: &crate::KeyStore) {
        let mut session = self.engine.session();
        session.label("Load the asset keys");
        let providers = session.ext.get::<craft_anything_engines::assets::AssetProviders>().map(|p| p.as_ref().clone()).unwrap_or_default();
        let keys = crate::keys::asset_keys(store, &providers);
        // The brief offers the built-in models, so a key for another provider doesn't count.
        self.assets = craft_anything_engines::assets::AssetProviders::built_in().ids().into_iter().any(|id| keys.get(id).is_some());
        session.ext.insert(Arc::new(keys));
    }

    /// Point the engine at this workspace: its investigation project and inbox.
    fn attach(&self, ws: &Workspace) -> Result<(), ToolError> {
        let dir = ws.investigation_dir();
        let project = Project::open(&dir).or_else(|_| Project::create(&dir, &ws.manifest.name, Purpose::default()))?;
        let mut s = self.engine.session();
        s.open_project(project)?;
        s.ext.insert(Arc::new(InboxDir(ws.inbox_dir())));
        Ok(())
    }

    /// The prompt for notes the person added while the agent worked, if there are any, and the
    /// Just-ask answers it carries so they're marked told once the turn finishes.
    fn added(&self, ws: &mut Workspace, deep: bool, sees_pictures: bool) -> Option<(String, Vec<AskedQuestion>)> {
        let mut notes = std::mem::take(&mut *self.notes.lock().unwrap_or_else(PoisonError::into_inner));
        if notes.is_empty() {
            return None;
        }
        // They go in this message, so no hook hands them over again.
        let dir = notes::notes_dir(&ws.inbox_dir());
        for n in &notes {
            notes::forget(&dir, n.id);
        }
        notes.retain(|n| !n.text.trim().is_empty());
        if notes.is_empty() {
            return None;
        }
        for n in &notes {
            self.emit(RunEvent::Note { shown: n.shown.clone(), pictures: n.pictures.clone() });
        }
        // A cancelled step can't be reported, but the notes go in either way.
        let _ = self.step(ws, Step::Build, Status::Building, "Reading what you added…");
        let attached: Vec<AskedQuestion> = notes.iter().flat_map(|n| n.questions.iter().cloned()).collect();
        // A note taken before the person asked something carries no answer for it; put the
        // remaining ones before the notes so the builder has both in this turn.
        let extra: Vec<AskedQuestion> =
            craft_anything_engine::questions::untold(ws, deep).into_iter().filter(|q| !attached.iter().any(|a| same_question(a, q))).collect();
        let mut questions = attached;
        questions.extend(extra);
        let context = just_asked(&questions);
        let texts: Vec<String> = notes.into_iter().map(|n| n.text).collect();
        Some((context + &added_prompt(&texts) + &pictures_reminder(ws, sees_pictures), questions))
    }

    /// Decides an ACP builder's permission requests with the same policy Claude Code's approval
    /// tool uses: allowed, refused, or asked of the person through this project's inbox.
    fn approver(&self, ws: &Workspace) -> crate::cli_agents::acp::Approver {
        let inbox = ws.inbox_dir();
        let (auto_mode, clean_room) = {
            let s = self.engine.session();
            (s.settings.auto_mode, s.clean_room())
        };
        let stop = self.stop.clone();
        Arc::new(move |tool: &str, input: &Value| {
            let r = craft_anything_engine::tools::interaction_tools::approve_with(
                Some(Inbox::new(inbox.clone())),
                auto_mode,
                clean_room,
                tool,
                input,
                &|| stop.is_cancelled(),
                &mut || {},
            );
            matches!(r, Ok(Ok(())))
        })
    }

    /// The editor for a game-engine stack. An engine that isn't ready stops the run before any
    /// work, with the way to get it.
    fn find_editor(&self, stack: &str) -> Result<Option<PathBuf>, ToolError> {
        let Some(b) = game::Builder::from_stack(stack).filter(|b| *b != game::Builder::Rust) else { return Ok(None) };
        let store = self.engine.session().settings.engine_store_dir();
        let inv = game::inventory(&game::Places::here(store));
        match inv.best(b) {
            Some(i) if inv.ready(b) => Ok(Some(i.program.clone())),
            _ => Err(ToolError::new(ErrorCode::BadSettings, format!("{} isn't ready on this computer yet.", b.name())).with_remediation(
                "Get it (Settings → Engines in the app), or pick another engine (\"Build with\" in the app, --build-with in the CLI) and start again.",
            )),
        }
    }

    /// How the builder's agent runs. Reads the session once: a lock taken twice in one statement
    /// is held by the first until the statement ends, and the second then waits forever. A saved
    /// Claude `--resume` that would continue the project's Deep session in Clean room is refused.
    fn agent_spec(&self, ws: &Workspace, entry: &crate::BackendEntry, opts: &RunOptions) -> Result<CliSpec, ToolError> {
        let (deep, config_dir, targets) = {
            let session = self.engine.session();
            (!session.clean_room(), session.config_dir(), session.shared_targets())
        };
        refuse_deep_session_reuse_in_clean_room(ws, &entry.config.model, &entry.options.args, deep)?;
        Ok(CliSpec {
            builder: Some(Builder { investigation: Some(ws.investigation_dir()), inbox: ws.inbox_dir() }),
            deep,
            targets,
            logs: Some(crate::failure_log::dir_in(&config_dir)),
            key: crate::cli_agents::acp::key_for(&self.keys, &entry.config.model, &entry.config.name, entry.config.kind),
            cost_totals: Some(crate::cli_agents::claude_cost::path_in(&config_dir)),
            resume: ws.manifest.agent_session_for_mode(&entry.config.model, deep).map(str::to_string),
            preamble: Some(
                "You work inside Craft Anything for a person who doesn't code. The message below is your task.

"
                .into(),
            ),
            adapters: opts.adapters.clone(),
            ..CliSpec::new(entry.config.clone(), entry.options.clone(), opts.beginner, Some(ws.root.clone()))
        })
    }

    /// The whole run: checks the builder can start, studies a new project, then builds and checks
    /// in rounds until it's good enough or the rounds run out. A coding agent's saved session is
    /// put back from the project first when this computer doesn't have it, and the project keeps
    /// a copy after every turn; when it can't be resumed, a new one starts from the brief and what
    /// the project kept. Returns the summary shown when it's ready.
    fn run(
        &mut self,
        ws: &mut Workspace,
        entry: crate::BackendEntry,
        api: Option<crate::agent::ApiConversation>,
        extractor: Option<(String, crate::agent::ApiConversation)>,
        kept_without_api: KeptHistory,
        opts: &RunOptions,
    ) -> Result<String, ToolError> {
        let mut progress = BuildProgress::load(&ws.root)?;
        *self.kept_checks.lock().unwrap_or_else(PoisonError::into_inner) = kept_check::KeptChecks::from_saved(&progress, ws);
        let mut progress_available = true;
        let mut fingerprints = AppFingerprintCache::default();
        // Try again has no message. Only an explicit new request starts another attempt on
        // unchanged files; restarting the app or resuming a paused run keeps the saved count.
        if opts.message.as_deref().is_some_and(|m| !m.trim().is_empty()) {
            progress.reset();
            progress.save(&ws.root)?;
        }
        // Fail before any work when the agent's tools are missing or it can't sign in.
        let remote_host = entry.config.ssh_host.as_deref().map(str::trim).filter(|h| !h.is_empty());
        let remote = remote_host.is_some();
        if api.is_none() {
            let adapter = opts.adapters.for_backend(&entry.config.model, &entry.options);
            if let Some(host) = remote_host {
                adapter.check_remote_tool_server(host, &self.stop)?;
            } else {
                crate::cli_agents::check_builder_tools(adapter.as_ref(), &entry.config.model, &entry.options, &ws.root, &self.stop)?;
                let program = crate::cli_agents::program_of(&entry.config, &entry.options)?;
                // A saved path that's gone (an update moved it) or would start the wrong app runs another program: say which (#721).
                if let Some(saved) = crate::cli_agents::replaced_program(&entry.options, &program) {
                    self.emit(RunEvent::Agent(AgentEvent::Status {
                        text: format!("Settings → AI names {saved}, but {program} runs instead. To use a different one, choose it in Settings → AI."),
                    }));
                }
                if adapter.signed_in(&program) == Some(false) {
                    return Err(ToolError::new(ErrorCode::BadSettings, not_signed_in(&entry.config.model, adapter.signed_in_before()))
                        .with_remediation(adapter.sign_in_fix().unwrap_or_default()));
                }
            }
        }
        self.attach(ws)?;
        let run_deep = !self.engine.session().clean_room();
        std::fs::create_dir_all(ws.app_dir()).map_err(|e| ToolError::io(ws.app_dir().display(), e))?;
        let no_kind = || {
            ToolError::new(ErrorCode::BadSettings, "No kind of project matches this request.")
                .with_remediation("Describe what to make: a website, an app or a game.")
        };
        if ws.manifest.stack.is_empty() {
            ws.manifest.stack = project::first_stack(&opts.kinds, ws, run_deep).ok_or_else(no_kind)?.to_string();
            ws.save()?;
        }
        let kind = opts.kinds.detect(&ws.manifest.request, &ws.manifest.target, &ws.manifest.stack).ok_or_else(no_kind)?;
        self.self_check = Some((kind.clone(), opts.good_enough));
        let kind = kind.as_ref();
        self.editor = self.find_editor(&ws.manifest.stack)?;
        let api_resumed = api.as_ref().is_some_and(crate::agent::ApiConversation::resumed);
        let kept = KeptHistory { in_mode: kept_without_api.in_mode || api_resumed, elsewhere: kept_without_api.elsewhere };
        let carrying_on = is_carrying_on(opts, kept.in_mode);
        let first = starts_with_study(opts, kept, capture_in_mode(ws, run_deep));
        if first {
            self.study(ws, kind, run_deep)?;
            if let Some((name, ex)) = extractor {
                self.extract(ws, kind, &name, ex)?;
            }
        } else {
            // An API builder over SSH still calls tools in this process, so only a coding agent
            // on the other computer has a tool server this one can't hand the program to.
            self.reopen_target(ws, kind, study::tools_run_remotely(remote, api.is_some()), run_deep)?;
        }
        // A coding agent resumes its own session; our loop resumes its kept transcript.
        let resumable = api.as_ref().is_none_or(crate::agent::ApiConversation::resumed);
        let api_loop = api.is_some();
        // Agent apps open pictures with their Read tool; the API loop sends text only, so its
        // builder is told what to use instead (R5.4: models that can see get pictures later).
        let sees_pictures = !api_loop;
        // A session another coding agent saved can't be continued by this one (#469).
        if !api_loop && ws.manifest.agent_session_by.as_deref() != Some(entry.config.model.as_str()) {
            ws.manifest.agent_session = ws.manifest.agent_session_for(&entry.config.model).map(str::to_string);
            ws.manifest.agent_session_by = Some(entry.config.model.clone());
            if ws.manifest.agent_session.is_none() {
                ws.manifest.agent_session_deep = None;
            }
            ws.save()?;
        }
        // The run's mode comes from the same read as the resume decision: a settings change while
        // the run starts can't make the brief, the tool server and the recorded session disagree.
        let spec = self.agent_spec(ws, &entry, opts)?;
        let deep = spec.deep;
        // A session from the other mode is not resumed (it may have read the original there),
        // so the builder needs the brief as if it had none.
        let resumed = spec.resume.is_some();
        // A coding agent on this computer whose session the project keeps a copy of (#451).
        let keeper = (!api_loop && !remote).then(|| opts.adapters.for_backend(&entry.config.model, &entry.options));
        if resumed && let Some(a) = &keeper {
            self.restore_session(ws, a.as_ref());
        }
        self.step(ws, Step::Build, Status::Building, "Getting the builder ready…")?;
        // A coding agent that never saved a session starts over, so it needs the brief.
        let has_history = resumable && (api_loop || resumed);
        let mut agent: Box<dyn Conversation> = match api {
            Some(a) => Box::new(a),
            None => crate::cli_agents::builder_conversation(spec, self.approver(ws)),
        };
        // The builder's tool server is started in this run's mode (`--deep` or `--clean-room`).
        let s = self.engine.session();
        let ghidra = s.settings.ghidra_launcher().is_some();
        let image_model = s.settings.assets.image_model.clone();
        drop(s);
        let cx = BriefCx { ws, deep, ghidra, editor: self.editor.as_deref(), assets: self.assets, image_model: image_model.as_deref(), sees_pictures };
        // The project's settings as they are now, for an agent that carries on its session.
        let now = || kind.follow_up_sections(&cx).iter().map(|s| format!("\n\n{s}")).collect::<String>() + &pictures_reminder(ws, sees_pictures);
        let mut prompt = match &opts.message {
            // An agent with a saved session gets the follow-up; one without (an API loop) starts
            // from the brief with the request added.
            Some(m) if !first && has_history => follow_up(m, kind, api_loop) + &now(),
            // Carrying on after a stop: the same, told to finish what it was doing.
            None if carrying_on && !first && has_history => continue_prompt(kind, api_loop) + &now(),
            _ => {
                let b = brief(kind, &cx, opts.message.as_deref());
                if api_loop { format!("{API_BUILDER}{}\n\n{b}", if sees_pictures { "" } else { BLIND_BUILDER }) } else { b }
            }
        };
        // The extraction step left the original's files in app/Original/: the builder starts from them.
        if first && ws.app_dir().join(ORIGINALS_MANIFEST).is_file() {
            prompt.push_str(ORIGINALS_READY);
        }
        // A saved session may not be on this computer (the project was copied here): then a new
        // one starts from the brief and what the project already has.
        let mut fresh = (!api_loop && resumed).then(|| {
            let handoff = keeper.as_ref().and_then(|a| a.handoff(&kept_session(ws)?, HANDOFF_CHARS));
            resumed_elsewhere(&brief(kind, &cx, opts.message.as_deref()), &ws.manifest.last_reply, handoff.as_deref())
        });
        // "Just ask" runs apart from the builder's session, and the person's message may follow on from it (#857).
        let new_request = opts.message.as_deref().is_some_and(|m| !m.trim().is_empty());
        let mut asked = if new_request { craft_anything_engine::questions::untold(ws, deep) } else { Vec::new() };
        let just_asked = just_asked(&asked);
        prompt.insert_str(0, &just_asked);
        fresh = fresh.map(|f| format!("{just_asked}{f}"));
        let mut round = ws.manifest.checks.iter().map(|c| c.round).max().unwrap_or(0);
        // A new request starts the count of checks that found no working build again. "Try again" on
        // a build that already reached the limit stops before another builder turn is paid for (#736).
        if opts.message.is_some() {
            if ws.manifest.not_working_checks != 0 {
                ws.manifest.not_working_checks = 0;
                ws.save()?;
            }
        } else if ws.manifest.not_working_checks >= NOT_WORKING_CHECK_LIMIT {
            return Err(not_working(ws));
        }
        let mut refinements = 0;
        let mut reminded = false;
        // The builder finished and only the check didn't: the build is there to check.
        let mut only_check = carrying_on && !first && opts.message.is_none() && ws.manifest.awaiting_check;
        loop {
            if !std::mem::take(&mut only_check) {
                progress = BuildProgress::load(&ws.root)?;
                if let Some(fingerprint) = self.progress_fingerprint(ws, &mut progress, &mut progress_available, &mut fingerprints)? {
                    progress.before_turn(fingerprint);
                    if progress.stalled() {
                        return Err(no_build_progress(ws));
                    }
                    progress.save(&ws.root)?;
                }
                let doing = match (round, carrying_on) {
                    (0, false) => "Building the first version…",
                    (0, true) => "Carrying on with the build…",
                    _ => "Improving the build…",
                };
                self.step(ws, Step::Build, Status::Building, doing)?;
                ws.manifest.awaiting_check = false;
                let reply = self.turn(ws, agent.as_mut(), &prompt, deep, fresh.take());
                // Kept even when the turn failed: the conversation is still worth carrying.
                if let Some(a) = &keeper {
                    self.keep_session(ws, a.as_ref());
                }
                let reply = match reply {
                    Ok(reply) => reply,
                    Err(error) => {
                        // Consumed instructions start a fresh attempt even if this turn fails
                        // or is cancelled. The provider retry loop has ended; stop any remaining
                        // work before touching the record, and retain the original failure.
                        agent.stop();
                        drop(agent);
                        if self.reset_progress_after_notes
                            && let Err(reset_error) = self.progress_after_notes(ws)
                        {
                            self.emit(RunEvent::Agent(AgentEvent::Status {
                                text: format!("The new request couldn't reset the unchanged-turn count ({}).", reset_error.message),
                            }));
                        }
                        return Err(error);
                    }
                };
                // Only a finished turn counts: telling the builder twice costs little, and a turn that
                // failed may not have told it at all.
                let told = std::mem::take(&mut asked);
                if !told.is_empty()
                    && let Err(e) = craft_anything_engine::questions::mark_told(ws, &told)
                {
                    log::warn!("couldn't record which questions the builder was told: {}", e.message);
                }
                if !reply.trim().is_empty() {
                    ws.manifest.last_reply = reply.trim().to_string();
                }
                // The stack is recorded as the one the build uses, once it shows: left to the builder,
                // or another than the default because the request asked for it. A game engine stays
                // (it says the project is a game, and its editor is found for it).
                if game::Builder::from_stack(&ws.manifest.stack).is_none()
                    && let Some(s) = stacks::used_in(&ws.app_dir())
                {
                    ws.manifest.stack = s.name.clone();
                }
                ws.manifest.awaiting_check = true;
                ws.save()?;
                // Keep the finished turn before scanning: a cancellation during the scan
                // must not make Try again repeat work that only needs its checks.
                // A consumed note starts another attempt, but its record I/O must not
                // interrupt a live builder or enter the provider retry path.
                progress = self.progress_after_notes(ws)?;
                if let Some(fingerprint) = self.progress_fingerprint(ws, &mut progress, &mut progress_available, &mut fingerprints)? {
                    progress.after_turn(fingerprint);
                    progress.save(&ws.root)?;
                }
                // What the person added while the agent worked goes in before anything else.
                if let Some((p, more)) = self.added(ws, deep, sees_pictures) {
                    asked.extend(more);
                    progress.reset();
                    progress.save(&ws.root)?;
                    ws.manifest.not_working_checks = 0;
                    prompt = p;
                    continue;
                }
                if !reminded && needs_decompile_reminder(deep, &ws.manifest) {
                    reminded = true;
                    prompt = DECOMPILE_FIRST.to_string();
                    continue;
                }
            }
            round += 1;
            let checking = if ws.manifest.target == TargetRef::None { "Checking the build…" } else { "Checking the build against the original…" };
            self.step(ws, Step::Check, Status::Checking, checking)?;
            let checks = self.checks_with_a_retry(ws, kind, round)?;
            ws.manifest.checks.extend(checks.iter().cloned());
            ws.manifest.awaiting_check = false;
            // A round in which every check works (built and served) ends the streak; any other
            // round adds to it, across runs, until the limit stops the build.
            ws.manifest.not_working_checks = if checks.iter().all(|c| c.passed) { 0 } else { ws.manifest.not_working_checks.saturating_add(1) };
            ws.save()?;
            // Kept for the next prompt when every check worked; saved after craft.json has the round.
            progress.checked = self.kept_checks.lock().unwrap_or_else(PoisonError::into_inner).saved_for(round);
            progress.save(&ws.root)?;
            for c in &checks {
                self.emit(RunEvent::Checked(c.clone()));
            }
            if let Some((p, more)) = self.added(ws, deep, sees_pictures) {
                asked.extend(more);
                progress.reset();
                progress.save(&ws.root)?;
                ws.manifest.not_working_checks = 0;
                prompt = p;
                continue;
            }
            let kind_checks = kind.checks(ws);
            let not_good = checks.iter().find(|c| !c.good(opts.good_enough) && kinds::can_refine(&kind_checks, c));
            if not_good.is_some() && progress_available && progress.stalled() {
                // A check-only resume skipped the builder's before-turn scan. A real edit
                // while the project was closed (or during this check) still starts an attempt.
                if let Some(fingerprint) = self.progress_fingerprint(ws, &mut progress, &mut progress_available, &mut fingerprints)? {
                    progress.before_turn(fingerprint);
                    progress.save(&ws.root)?;
                    if progress.stalled() {
                        return Err(no_build_progress(ws));
                    }
                }
            }
            let Some(check) = not_good else { break };
            if ws.manifest.not_working_checks >= NOT_WORKING_CHECK_LIMIT {
                return Err(not_working(ws));
            }
            if refinements >= opts.max_rounds {
                break;
            }
            refinements += 1;
            // Nothing written yet: a fix for a build that doesn't exist sends the builder looking for one.
            // It is asked to write the project and to show its build output (#736).
            let refined = if app_is_empty(ws) { NO_PROJECT_PROMPT.to_string() } else { kind.refine(check, sees_pictures) };
            prompt = refined + &pictures_reminder(ws, sees_pictures);
            if !api_loop {
                prompt.push_str(CHECK_BEFORE_REPLYING);
            }
        }
        if let Some(url) = self.preview.lock().unwrap_or_else(PoisonError::into_inner).as_ref().map(StaticServer::url) {
            self.emit(RunEvent::Preview(url));
        }
        let last = ws.manifest.checks.last().cloned();
        // A site that builds but isn't close enough is still usable; no working build is not.
        let usable = last.as_ref().is_some_and(|c| c.passed);
        if !usable {
            let why = last.map(|c| c.note).unwrap_or_default();
            return Err(ToolError::new(ErrorCode::ProgramFailed, format!("The build isn't working yet. {why}").trim().to_string())
                .with_remediation("Ask for a fix in the box below, or press Try again."));
        }
        let last = ws.manifest.checks.last().cloned();
        // The last round's checks, all of them: the last one alone (say a mechanics sheet) isn't the verdict.
        let last_round: Vec<CheckResult> = last.map(|l| ws.manifest.checks.iter().filter(|c| c.round == l.round).cloned().collect()).unwrap_or_default();
        let mut summary = if last_round.is_empty() { "Ready.".to_string() } else { format!("Ready: {}.", self_check::summary(&last_round, opts.good_enough)) };
        // What the builder couldn't find out is said, not hidden.
        match self_check::open_questions(ws).len() {
            0 => {}
            1 => summary.push_str(" 1 question about the original is still open."),
            n => summary.push_str(&format!(" {n} questions about the original are still open.")),
        }
        ws.set_status(Status::Ready, summary.clone())?;
        self.emit(RunEvent::Step { step: Step::Done, text: summary.clone() });
        Ok(summary)
    }

    /// Read progress after the turn has ended. A consumed request resets the allowance on
    /// success, failure or cancellation; a failed save never acknowledges the pending reset.
    fn progress_after_notes(&mut self, ws: &Workspace) -> Result<BuildProgress, ToolError> {
        let mut progress = BuildProgress::load(&ws.root)?;
        if self.reset_progress_after_notes {
            progress.reset();
            progress.save(&ws.root)?;
            self.reset_progress_after_notes = false;
        }
        Ok(progress)
    }

    /// File scanning is an optional guard, not a prerequisite for building or checking. Once
    /// it is unavailable, skip it for this run and start a new comparison on the next run.
    fn progress_fingerprint(
        &self,
        ws: &Workspace,
        progress: &mut BuildProgress,
        available: &mut bool,
        fingerprints: &mut AppFingerprintCache,
    ) -> Result<Option<String>, ToolError> {
        self.stop.check()?;
        if !*available {
            return Ok(None);
        }
        match fingerprints.scan(ws, &self.stop) {
            Ok(fingerprint) => Ok(Some(fingerprint)),
            Err(e) if e.code == ErrorCode::Cancelled => Err(e),
            Err(e) => {
                *available = false;
                progress.invalidate();
                // Record errors remain errors: never overwrite a broken or newer record.
                progress.save(&ws.root)?;
                self.emit(RunEvent::Agent(AgentEvent::Status {
                    text: format!("Build progress couldn't be checked ({}). Continuing without the unchanged-turn limit for this run.", e.message),
                }));
                Ok(None)
            }
        }
    }

    /// One builder turn. Its session is kept even when the turn fails, so nothing is lost; a turn
    /// cut short (a time limit, a crash, a dropped connection) is picked up where it stopped, up to
    /// three times. Problems only the person can fix (sign-in, credit, a missing app) end the run.
    /// Deep mode on a game: the chosen extractor takes the game's own files out into app/Original/
    /// before the build (the person owns the game and chose this; nothing is decrypted).
    fn extract(&mut self, ws: &mut Workspace, kind: &dyn kinds::TargetKind, name: &str, mut ex: crate::agent::ApiConversation) -> Result<(), ToolError> {
        let TargetRef::Local { path } = ws.manifest.target.clone() else { return Ok(()) };
        if kind.id() != kinds::GameKind::ID || self.engine.session().clean_room() {
            return Ok(());
        }
        let folder = locate::game_folder(&path).unwrap_or(path);
        self.step(ws, Step::Study, Status::Inspecting, &format!("{name} is taking the game's own files out…"))?;
        std::fs::create_dir_all(ws.app_dir().join("Original")).map_err(|e| ToolError::io(ws.app_dir().display(), e))?;
        // A failed extraction doesn't stop the build: it says why, and the builder uses what's there.
        match self.converse(ws, &mut ex, &extract_brief(&folder)) {
            Err(e) if e.code == ErrorCode::Cancelled => Err(e),
            Err(e) => {
                let why = e.message.chars().take(200).collect::<String>();
                self.step(ws, Step::Study, Status::Inspecting, &format!("{name} couldn't finish taking the files out ({why}). Building with what's there."))
            }
            Ok(_) => Ok(()),
        }
    }

    /// The round's checks. A check that fails says what's wrong with the build; one that returns
    /// an error couldn't look at all (a browser that went away), which says nothing about the
    /// build, so it gets a second go before the run stops, and the run says what is really wrong.
    fn checks_with_a_retry(&self, ws: &Workspace, kind: &dyn kinds::TargetKind, round: u32) -> Result<Vec<CheckResult>, ToolError> {
        self.checks_unless_unchanged(ws, kind, round, || self.checks_with_a_retry_now(ws, kind, round))
    }

    fn checks_with_a_retry_now(&self, ws: &Workspace, kind: &dyn kinds::TargetKind, round: u32) -> Result<Vec<CheckResult>, ToolError> {
        let e = match self.run_checks(ws, kind, round) {
            Err(e) if e.code != ErrorCode::Cancelled => e,
            done => return done,
        };
        self.emit(RunEvent::Agent(AgentEvent::Status { text: format!("The check couldn't finish ({}). Trying it once more…", e.message) }));
        self.run_checks(ws, kind, round).map_err(|e| {
            if e.code == ErrorCode::Cancelled {
                return e;
            }
            ToolError::new(e.code, format!("The build is finished, but it couldn't be checked: {}", e.message))
                .with_remediation("Press Try again to check it again. Nothing needs building again.")
        })
    }

    /// A copy of the builder's session in the project, so the conversation travels with the
    /// folder (#451). A copy that can't be made doesn't stop the build, but it's said.
    fn keep_session(&self, ws: &Workspace, adapter: &dyn crate::cli_agents::AgentAdapter) {
        let Some(from) = ws.manifest.agent_session.as_deref().and_then(|id| adapter.session_file(id)) else { return };
        let kept = match std::fs::metadata(&from) {
            Ok(m) if m.len() > MAX_KEPT_SESSION => Err(format!("it's over {} MB", MAX_KEPT_SESSION / 1_000_000)),
            Ok(_) => std::fs::read(&from)
                .map_err(|e| e.to_string())
                .and_then(|b| craft_anything_platform::fs::write_atomic_private(&ws.agent_session_path(), &b).map_err(|e| e.message)),
            Err(e) => Err(e.to_string()),
        };
        if let Err(why) = kept {
            let text =
                format!("Couldn't keep a copy of the builder's conversation in the project ({why}), so it won't go with the folder to another computer.");
            self.emit(RunEvent::Agent(AgentEvent::Status { text }));
        }
    }

    /// The project came from another computer with its builder's session kept in it: put it back
    /// where the agent looks, so the same conversation carries on (#451). If that fails, a new one
    /// starts from what the project has when the turn finds the session missing.
    fn restore_session(&self, ws: &Workspace, adapter: &dyn crate::cli_agents::AgentAdapter) {
        let (Some(id), Some(saved)) = (ws.manifest.agent_session.as_deref(), kept_session_bytes(ws)) else { return };
        let text = match adapter.restore_session(id, &saved, &ws.root) {
            Ok(false) => return,
            Ok(true) => "Brought the builder's earlier conversation over from the project folder.".to_string(),
            Err(e) => format!("Couldn't bring the builder's earlier conversation over from the project folder ({}).", e.message),
        };
        self.emit(RunEvent::Agent(AgentEvent::Status { text }));
    }

    /// `fresh`: what to send instead when the agent's saved session turns out not to be on this
    /// computer.
    fn turn(&mut self, ws: &mut Workspace, agent: &mut dyn Conversation, prompt: &str, deep: bool, fresh: Option<String>) -> Result<String, ToolError> {
        self.turn_with_wait(ws, agent, prompt, deep, fresh, |stop, seconds| {
            for _ in 0..seconds {
                stop.check()?;
                std::thread::sleep(Duration::from_secs(1));
            }
            stop.check()
        })
    }

    /// Keep the session the agent reported, with the mode it ran in; a lost one leaves neither.
    fn remember_session(ws: &mut Workspace, session: Option<String>, deep: bool) -> Result<(), ToolError> {
        ws.manifest.agent_session_deep = session.is_some().then_some(deep);
        ws.manifest.agent_session = session;
        ws.save()
    }

    /// Run one turn, retrying what the adapter calls retryable, and keep the session it reports
    /// with the mode this run started in.
    fn turn_with_wait(
        &mut self,
        ws: &mut Workspace,
        agent: &mut dyn Conversation,
        prompt: &str,
        deep: bool,
        mut fresh: Option<String>,
        mut wait_for: impl FnMut(&CancelToken, u64) -> Result<(), ToolError>,
    ) -> Result<String, ToolError> {
        let mut prompt = prompt.to_string();
        let mut tries = 0u64;
        let mut retry = false;
        loop {
            let result = self.converse_turn(ws, agent, &prompt, retry);
            let lost = result.is_err() && agent.session_lost();
            if lost {
                // The id is for a session this computer doesn't have: never ask for it again.
                Self::remember_session(ws, None, deep)?;
            } else if let Some(id) = agent.session_id() {
                Self::remember_session(ws, Some(id), deep)?;
            }
            match result {
                Err(_) if lost && let Some(f) = fresh.take() => {
                    self.step(
                        ws,
                        Step::Build,
                        Status::Building,
                        "The builder's earlier conversation isn't on this computer. Starting a new one from what the project already has…",
                    )?;
                    prompt = f;
                    retry = false;
                }
                // A missing session can only start fresh above, never resume. Only backend
                // Error events become ProgramFailed here; a simultaneous backend failure must
                // not make an inbox or workspace write failure retryable.
                Err(e) if !lost && e.code == ErrorCode::ProgramFailed && tries < 3 && agent.last_failure().is_some_and(crate::TurnFailure::retryable) => {
                    tries += 1;
                    let why = e.message.chars().take(160).collect::<String>();
                    // Typed status decides whether to retry. A display-only wait remains a
                    // compatibility hint, and never overrides a structured provider delay.
                    let delay = agent.last_failure().and_then(|f| f.retry_after_secs).or_else(|| crate::failure::retry_after(&e.message));
                    let seconds = match delay {
                        Some(secs) => {
                            let note = format!("The AI service is busy or its rate limit was reached, and asked to wait {secs} s. Trying again then…");
                            self.step(ws, Step::Build, Status::Building, &note)?;
                            secs.saturating_add(1).min(crate::failure::MAX_RETRY_WAIT)
                        }
                        None => {
                            self.step(ws, Step::Build, Status::Building, &format!("The builder stopped ({why}). Picking up where it left off…"))?;
                            30 * tries
                        }
                    };
                    wait_for(&self.stop, seconds)?;
                    self.stop.check()?;
                    retry = true;
                }
                r => return r,
            }
        }
    }

    /// Send one message and pump the agent until its turn ends, relaying events and inbox
    /// requests. Returns the agent's reply text.
    fn converse(&mut self, ws: &mut Workspace, agent: &mut dyn Conversation, text: &str) -> Result<String, ToolError> {
        self.converse_turn(ws, agent, text, false)
    }

    fn converse_turn(&mut self, ws: &mut Workspace, agent: &mut dyn Conversation, text: &str, retry: bool) -> Result<String, ToolError> {
        if retry {
            agent.retry(text);
        } else {
            agent.send(text);
        }
        let mut reply = String::new();
        let mut failed = None;
        loop {
            if self.stop.is_cancelled() {
                agent.stop();
            }
            for e in agent.poll() {
                if matches!(e, AgentEvent::ToolFinished { .. } | AgentEvent::TurnDone { .. }) {
                    let mut session = self.engine.session();
                    let project = session.project.clone().filter(|project| project.root == ws.investigation_dir());
                    if let Some(project) = project
                        && let Err(error) = session.open_project(project)
                    {
                        log::warn!("Couldn't refresh the investigation: {}", error.message);
                    }
                }
                match &e {
                    AgentEvent::TextDelta { text } => reply.push_str(text),
                    // Our own loop asks before tools that need consent. The Build tools are the
                    // builder's job (they apply the approval policy themselves); nothing else runs.
                    AgentEvent::ConsentNeeded { call_id, tool, .. } => {
                        let build = self.engine.registry().get(tool).is_some_and(|s| s.toolset == craft_anything_engine::Toolset::Build);
                        agent.answer_consent(call_id, build);
                    }
                    // Decided on the tool's own id (its CLI and `mcp__…__` names included).
                    AgentEvent::ToolStarted { tool, .. }
                        if self.engine.registry().canonical(tool) == Some(craft_anything_engine::tools::native_function::PSEUDOCODE_TOOL) =>
                    {
                        ws.manifest.decompiled = true
                    }
                    AgentEvent::Error { message, remediation, .. } => failed = Some((message.clone(), remediation.clone())),
                    _ => {}
                }
                if let AgentEvent::TurnDone { input_tokens, output_tokens, cost_usd, cost_is_estimate, .. } = &e {
                    self.record_usage(Some((*input_tokens, *output_tokens, *cost_usd, *cost_is_estimate)));
                }
                let done = matches!(e, AgentEvent::TurnDone { .. });
                // A saved session that isn't on this computer isn't the build failing: the turn
                // starts a new one (or, with nothing to start it from, the run's error says why).
                if !(matches!(e, AgentEvent::Error { .. }) && agent.session_lost()) {
                    self.emit(RunEvent::Agent(e));
                }
                if done {
                    self.relay_read_notes(ws);
                    self.stop.check()?;
                    // An agent-level error (sign-in, crash, limits) ends the run: checking a
                    // build that never happened would only mislead.
                    if let Some((m, r)) = failed {
                        return Err(ToolError::new(ErrorCode::ProgramFailed, m).with_remediation(r.unwrap_or_default()));
                    }
                    return Ok(reply);
                }
            }
            self.relay_inbox();
            self.relay_read_notes(ws);
            self.answer_self_checks(ws)?;
            if self.usage.as_ref().is_some_and(|c| c.since.elapsed() >= USAGE_FLUSH) {
                self.record_usage(None);
            }
            std::thread::sleep(Duration::from_millis(120));
        }
    }

    /// Notes the builder took at a step of this turn (its hook claimed them): they join the feed
    /// now and aren't sent again after the turn.
    fn relay_read_notes(&mut self, ws: &Workspace) {
        let dir = notes::notes_dir(&ws.inbox_dir());
        let read: Vec<Note> = {
            let mut notes = self.notes.lock().unwrap_or_else(PoisonError::into_inner);
            let (read, waiting) = std::mem::take(&mut *notes).into_iter().partition(|n| notes::was_read(&dir, n.id));
            *notes = waiting;
            read
        };
        let new_request = read.iter().any(|n| !n.text.trim().is_empty());
        for n in read {
            // The agent's hook took the note, so the Just-ask answers in its text were told.
            if !n.questions.is_empty()
                && let Err(e) = craft_anything_engine::questions::mark_told(ws, &n.questions)
            {
                log::warn!("couldn't record which questions the builder was told: {}", e.message);
            }
            notes::forget(&dir, n.id);
            self.emit(RunEvent::Note { shown: n.shown, pictures: n.pictures });
        }
        self.reset_progress_after_notes |= new_request;
    }

    fn relay_inbox(&mut self) {
        for r in self.inbox.pending() {
            if self.seen.insert(r.id.clone()) {
                self.emit(RunEvent::Ask(r));
            }
        }
    }
}

/// Notes an earlier run left are a request the person made, as a message is: they start the counts
/// of unchanged turns and of checks that found no working build again, or a run carrying on from a
/// build that reached either limit would stop before its builder reads them. Their files are gone
/// by now, so a count that can't be saved is logged and the run starts anyway. The count is saved
/// on the project as it is on disk: a run that waited in the queue holds a copy from before the
/// run ahead of it finished, and saving that would undo what that run recorded (its session).
fn left_notes_are_a_request(ws: &mut Workspace) {
    let progress = BuildProgress::load(&ws.root).and_then(|mut p| {
        p.reset();
        p.save(&ws.root)
    });
    ws.manifest.not_working_checks = 0;
    let checks = Workspace::open(&ws.root).and_then(|mut now| {
        if now.manifest.not_working_checks == 0 {
            return Ok(());
        }
        now.manifest.not_working_checks = 0;
        now.save()
    });
    if let Err(e) = progress.and(checks) {
        log::warn!("Couldn't start the build's counts again for the notes left waiting: {}", e.message);
    }
}

/// What the run says, and the status it leaves on the project, when it ends: paused, failed, or
/// a caught bug. A caught bug must not leave the saved status as it last was ("Checking" while
/// nothing runs), or the project looks stuck rather than stopped.
fn run_outcome(result: Result<Result<String, ToolError>, Box<dyn std::any::Any + Send>>, ws: &mut Workspace) -> (bool, String, bool) {
    match result {
        Ok(Ok(m)) => (true, m, false),
        Ok(Err(e)) if e.code == ErrorCode::Cancelled => {
            let _ = ws.set_status(Status::Stopped, "Paused. Send a message to continue.");
            (false, "Paused.".to_string(), true)
        }
        Ok(Err(e)) => {
            let msg = e.for_people();
            let _ = ws.set_status(Status::NeedsYou, msg.clone());
            (false, msg, false)
        }
        Err(_) => {
            let msg = "The workflow hit a bug and stopped safely. Your files are untouched.".to_string();
            let _ = ws.set_status(Status::NeedsYou, msg.clone());
            (false, msg, false)
        }
    }
}

/// Checks in a row that may find the build not working before the run stops and asks for a new
/// request. Each "Try again" used to start another `max_rounds` of checks, with nothing counting
/// across runs, so a build that never came up was checked 40 and 49 times (#736).
pub const NOT_WORKING_CHECK_LIMIT: u32 = 10;

fn not_working(ws: &Workspace) -> ToolError {
    ToolError::new(ErrorCode::OverLimit, format!("The build still isn't working after {} checks in a row, so it stopped.", ws.manifest.not_working_checks))
        .with_remediation("Send a new request with a concrete correction to start again. The last check's note in the feed says what it found.")
}

/// The builder's next request when app/ holds no files: write the project first, then say what the
/// build printed. The reporter's agent claimed `npm run build` had passed while app/ was empty (#736).
const NO_PROJECT_PROMPT: &str = "The check found no project: app/ has no files, so nothing has been written yet and nothing can be built. Create the project's files in app/ now (for a website, a package.json and an index.html or the source files), then build it. Reply with the exact command you ran and the last lines of its output. Say something is built only when that output says so.";

fn no_build_progress(ws: &Workspace) -> ToolError {
    let stop = format!("The build stopped after two builder turns changed no files in {}.", ws.app_place());
    // #736: an empty app/ means the builder never wrote the project, whatever its replies said.
    if app_is_empty(ws) {
        ToolError::new(ErrorCode::OverLimit, format!("{stop} {}", no_files_yet(ws)))
            .with_remediation("Send a new request that asks for the project's files in app/ (such as package.json and index.html), or try another builder.")
    } else {
        ToolError::new(ErrorCode::OverLimit, stop)
            .with_remediation("Send a new request with a concrete correction, or edit the app's files before trying again.")
    }
}

/// Whether the builder has written nothing into app/ (#736). A project folder always holds the
/// workspace's own files, so only app/ can be empty.
fn app_is_empty(ws: &Workspace) -> bool {
    !ws.manifest.app_in_root && folder_is_empty(&ws.app_dir())
}

/// Whether a folder holds nothing: no files or folders, or no folder at all.
fn folder_is_empty(dir: &Path) -> bool {
    std::fs::read_dir(dir).map(|mut entries| entries.next().is_none()).unwrap_or(true)
}

/// Said when app/ is empty, so a builder that claims it built the project can't hide that nothing
/// was written (#736).
fn no_files_yet(ws: &Workspace) -> String {
    format!("{} has no files yet, so the builder hasn't written the project.", ws.app_place())
}

/// Why a coding agent can't build: never signed in, or signed in `before` with a sign-in that no
/// longer works (expired, or signed out), which the person may not expect.
fn not_signed_in(agent: &str, before: bool) -> String {
    let who = crate::cli_agents::display_for(agent);
    if before { format!("{who}'s sign-in on this computer has expired or was signed out.") } else { format!("{who} isn't signed in on this computer.") }
}

/// Largest builder session the project keeps a copy of.
const MAX_KEPT_SESSION: u64 = 64_000_000;
/// How much of a kept session a new one is handed when the old one isn't on this computer.
const HANDOFF_CHARS: usize = 16_000;

/// The builder's session as the project kept it, if it did.
fn kept_session_bytes(ws: &Workspace) -> Option<Vec<u8>> {
    let path = ws.agent_session_path();
    if std::fs::metadata(&path).ok()?.len() > MAX_KEPT_SESSION {
        return None;
    }
    std::fs::read(path).ok()
}

/// The kept session as text, for a handoff (a broken byte doesn't lose the rest).
fn kept_session(ws: &Workspace) -> Option<String> {
    kept_session_bytes(ws).map(|b| String::from_utf8_lossy(&b).into_owned())
}

/// What an API-backend builder is told first: its tools differ from an agent app's.
const API_BUILDER: &str = "Your tools: build_files_list, build_file_read, build_file_write and build_file_edit for the project's files (paths relative to the project folder, e.g. app/src/main.rs), and build_command_run for commands, which run in app/ (never start dev servers or watchers). Where the steps below mention Read, Write, Edit or Bash, use these. For the checklist step use build_checklist (the same shape as TodoWrite: todos with content, activeForm and status) and call it again as you go. Ask the person with core_person_ask only when a choice really changes the result. Other Craft Anything tools are available too; enable a toolset with enable_toolset when you need it. rebuild_check isn't available in this loop: Craft Anything checks your build after each turn instead.";

/// Added to the API loop's note for a builder that can't look at pictures.
const BLIND_BUILDER: &str = " You can't look at pictures: use capture/README.md, capture/capture.json and the evidence instead.";

/// Added to a refinement for a builder that can check its own work.
const CHECK_BEFORE_REPLYING: &str = " Before you reply, check your build again with rebuild_check. Fix the results it says another builder turn can resolve, and state any remaining unverified comparisons.";

/// The backend the person chose for this project, while it's still set up and on (else the
/// default builds it).
pub fn project_builder<'a>(cfg: &crate::AiConfig, ws: &'a Workspace) -> Option<&'a str> {
    ws.manifest.builder.as_deref().filter(|n| cfg.get(n).is_some_and(|b| b.config.enabled))
}

/// A project's saved builder that is no longer usable, and the AI that builds in its place.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BuilderReplacement {
    /// The AI the project saved, which is off or gone.
    pub saved: String,
    /// The AI that builds in its place.
    pub used: String,
}

/// When a project's saved builder is off or gone, who builds instead. An explicitly named backend
/// is not a replacement: it is refused if unusable, and nothing falls back.
pub fn builder_replacement(cfg: &crate::AiConfig, ws: &Workspace, explicit: Option<&str>) -> Option<BuilderReplacement> {
    if explicit.is_some() {
        return None;
    }
    let saved = ws.manifest.builder.as_deref().map(str::trim).filter(|s| !s.is_empty())?;
    if project_builder(cfg, ws).is_some() {
        return None;
    }
    let used = builder_backend(cfg, None).ok()?.config.name;
    (used != saved).then(|| BuilderReplacement { saved: saved.to_string(), used })
}

/// The words for a saved builder that was turned off or removed and the one building instead.
/// English for now: the wording needs a native speaker's check before it joins the catalogs.
pub fn replaced_builder_notice(saved: &str, used: &str) -> String {
    format!("The AI chosen for this project ({saved}) is turned off or no longer set up. {used} builds instead.")
}

/// The backend that builds: the one named while it is set up and on, else the Builder role's,
/// else an agent app, else any enabled AI backend (which builds with our own agent loop and the
/// Build tools).
pub fn builder_backend(cfg: &crate::AiConfig, named: Option<&str>) -> Result<crate::BackendEntry, ToolError> {
    if let Some(n) = named {
        let Some(entry) = cfg.get(n) else {
            return Err(
                ToolError::invalid(format!("No AI backend named {n:?}")).with_remediation("See `craft-anything-cli ai list`, or add one in Settings → AI.")
            );
        };
        if !entry.config.enabled {
            return Err(ToolError::new(ErrorCode::BadSettings, format!("{} is turned off.", entry.config.name))
                .with_remediation("Turn it on in Settings → AI, or choose another backend for this build."));
        }
        return Ok(entry.clone());
    }
    cfg.backend_for(Role::Builder)
        .or_else(|| cfg.enabled().find(|b| b.config.kind == BackendKind::AgentCli))
        .or_else(|| cfg.enabled().next())
        .cloned()
        .ok_or_else(|| {
            ToolError::new(ErrorCode::BadSettings, "Building needs a coding agent (such as Claude Code) or an AI backend with an API key.")
                .with_remediation("Install a coding agent or add an API key in Settings → AI, then try again.")
        })
}
