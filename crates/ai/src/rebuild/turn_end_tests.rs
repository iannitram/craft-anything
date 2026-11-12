//! #1620: a build that stayed on "Pausing…" after its last round, with the builder's program gone.

use super::*;

fn turn_done() -> AgentEvent {
    AgentEvent::TurnDone { backend: "test".into(), input_tokens: 0, output_tokens: 0, cost_usd: None, cost_is_estimate: false }
}

fn worker(ws: &Workspace) -> (Worker, Receiver<RunEvent>) {
    let (tx, rx) = std::sync::mpsc::channel();
    let w = Worker {
        wake: Default::default(),
        engine: Arc::new(Engine::new(craft_anything_engine::testing::session())),
        tx,
        stop: CancelToken::default(),
        preview: Arc::default(),
        seen: HashSet::new(),
        inbox: Inbox::new(ws.inbox_dir()),
        editor: None,
        notes: Arc::default(),
        feed: Arc::new(Mutex::new(feed::FeedLog::open(ws))),
        keys: crate::KeyStore::file_only(ws.root.join("keys.toml")),
        kept_checks: Mutex::default(),
        assets: false,
        usage: None,
        self_check: None,
        reset_progress_after_notes: false,
    };
    (w, rx)
}

/// A stand-in builder whose program says its last words and exits after the round, but whose turn
/// is never said to be done: nothing is running, and no `TurnDone` comes.
struct ExitsAfterItsRound {
    said: Option<String>,
}

impl Conversation for ExitsAfterItsRound {
    fn send(&mut self, _text: &str) {
        self.said = Some("The round is done and its checks pass.".into());
    }
    fn poll(&mut self) -> Vec<AgentEvent> {
        self.said.take().map(|text| vec![AgentEvent::TextDelta { text }]).unwrap_or_default()
    }
    fn answer_consent(&mut self, _call_id: &str, _allow: bool) {}
    fn stop(&mut self) {}
    fn is_busy(&self) -> bool {
        false
    }
}

/// A stand-in builder whose turn never ends, even when asked to stop.
struct NeverStops;

impl Conversation for NeverStops {
    fn send(&mut self, _text: &str) {}
    fn poll(&mut self) -> Vec<AgentEvent> {
        Vec::new()
    }
    fn answer_consent(&mut self, _call_id: &str, _allow: bool) {}
    fn stop(&mut self) {}
    fn is_busy(&self) -> bool {
        true
    }
}

/// The run waited for the builder's `TurnDone` for good: the feed kept the reply's typing dots,
/// the status stayed Building, its clock was saved every minute, and a waiting note was never
/// sent. The turn now ends, with the reply, and the app hears that it did.
#[test]
fn a_builder_that_exits_without_ending_its_turn_does_not_hold_the_run() {
    let d = tempfile::tempdir().unwrap();
    let mut ws = Workspace::create(d.path(), "Arena", "Recreate this game", TargetRef::None).unwrap();
    let (mut w, rx) = worker(&ws);
    let (done, ended) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let mut builder = ExitsAfterItsRound { said: None };
        let _ = done.send(w.turn(&mut ws, &mut builder, "Build it.", true, None));
    });
    let reply = ended.recv_timeout(Duration::from_secs(20)).expect("the run waited for good on a builder that had exited").unwrap();
    assert_eq!(reply, "The round is done and its checks pass.");
    let events: Vec<RunEvent> = rx.try_iter().collect();
    assert!(events.iter().any(|e| matches!(e, RunEvent::Agent(AgentEvent::TurnDone { .. }))), "the feed and the clocks hear the turn end: {events:?}");
}

/// Pause on a builder whose turn never ends: the run stops anyway, as paused.
#[test]
fn pause_ends_a_turn_the_builder_never_ends() {
    let d = tempfile::tempdir().unwrap();
    let mut ws = Workspace::create(d.path(), "Arena", "Recreate this game", TargetRef::None).unwrap();
    let (mut w, _rx) = worker(&ws);
    let stop = w.stop.clone();
    let (done, ended) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let _ = done.send(w.turn(&mut ws, &mut NeverStops, "Build it.", true, None));
    });
    std::thread::sleep(Duration::from_millis(300));
    stop.cancel();
    let result = ended.recv_timeout(STOP_GRACE + Duration::from_secs(20)).expect("Pause stayed on Pausing");
    assert_eq!(result.unwrap_err().code, ErrorCode::Cancelled, "it ends as paused");
}

/// A builder that ends its turn as it should is still waited for while it works, and its own
/// `TurnDone` is the only one the feed gets.
#[test]
fn a_turn_the_builder_ends_itself_is_ended_once() {
    struct EndsAfterAWhile {
        polls: usize,
    }
    impl Conversation for EndsAfterAWhile {
        fn send(&mut self, _text: &str) {}
        fn poll(&mut self) -> Vec<AgentEvent> {
            self.polls += 1;
            if self.polls == 5 { vec![AgentEvent::TextDelta { text: "Done".into() }, turn_done()] } else { Vec::new() }
        }
        fn answer_consent(&mut self, _call_id: &str, _allow: bool) {}
        fn stop(&mut self) {}
        fn is_busy(&self) -> bool {
            self.polls < 5
        }
    }
    let d = tempfile::tempdir().unwrap();
    let mut ws = Workspace::create(d.path(), "Arena", "Recreate this game", TargetRef::None).unwrap();
    let (mut w, rx) = worker(&ws);
    let reply = w.turn(&mut ws, &mut EndsAfterAWhile { polls: 0 }, "Build it.", true, None).unwrap();
    assert_eq!(reply, "Done");
    let dones = rx.try_iter().filter(|e| matches!(e, RunEvent::Agent(AgentEvent::TurnDone { .. }))).count();
    assert_eq!(dones, 1);
}

/// The run sends `Finished` just before it stops counting as running. Opening the project in that
/// moment read the feed and dropped the queue, `Finished` with it, so the app never saw the run
/// end: Pause stayed on "Pausing…", the notes waiting were never sent and the clock kept going.
#[test]
fn opening_a_project_as_its_run_ends_keeps_the_end_for_the_app() {
    let d = tempfile::tempdir().unwrap();
    let ws = Workspace::create(d.path(), "x", "A thing", TargetRef::None).unwrap();
    let (tx, rx) = std::sync::mpsc::channel();
    let feed = Arc::new(Mutex::new(feed::FeedLog::open(&ws)));
    let activity = feed.lock().unwrap_or_else(PoisonError::into_inner).activity();
    let mut run = RebuildRun {
        rx,
        stop: CancelToken::default(),
        inbox: Inbox::new(ws.inbox_dir()),
        running: Arc::new(AtomicBool::new(true)),
        preview: Arc::default(),
        notes: Arc::default(),
        clean_room: false,
        feed: feed.clone(),
        root: ws.root.clone(),
        started: Instant::now(),
        activity,
        held: Vec::new(),
    };
    let wake = Wake::default();
    let end = RunEvent::Finished { ok: false, message: "Paused.".into(), paused: true };
    emit_event(&feed, &tx, &wake, RunEvent::Step { step: Step::Build, text: "Improving the build…".into() });
    emit_event(&feed, &tx, &wake, end);
    let mut shown = run.feed_now();
    assert_eq!(shown.iter().filter(|i| matches!(i, feed::FeedItem::Paused)).count(), 1, "{shown:?}");
    let events = run.poll();
    assert!(matches!(events.as_slice(), [RunEvent::Finished { paused: true, .. }]), "only the end is kept for the app: {events:?}");
    // The app applies it to the feed it was shown: said once.
    for e in &events {
        feed::apply(&mut shown, e);
    }
    assert_eq!(shown.iter().filter(|i| matches!(i, feed::FeedItem::Paused)).count(), 1, "{shown:?}");
    assert!(run.poll().is_empty(), "read once");
}
