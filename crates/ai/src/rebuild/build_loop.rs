//! The build loop: runs that follow each other, each with a fresh builder, until a limit the
//! person chose ends it (`Manifest::build_loop`). This is the deciding half, kept free of the
//! app so the CLI and the app end a loop for the same reasons: the caller starts each run
//! (`RunOptions::fresh_session`) and tells the loop what happened.

use craft_anything_engine::workspace::Workspace;
use craft_anything_engine::workspace::build_loop::{ClockTime, LoopSettings};
use std::hash::{DefaultHasher, Hash, Hasher};
use std::path::Path;

/// Why a loop ended.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LoopEnd {
    /// It did the runs it was given.
    Runs(u32),
    /// Its runs used the token budget.
    Tokens(u64),
    /// The stop time came.
    Time(ClockTime),
    /// This many runs in a row changed nothing.
    Stalled(u32),
    /// A run failed, so the next would likely fail the same way.
    Failed,
    /// The person stopped it (paused, or switched the loop off).
    Stopped,
}

impl LoopEnd {
    /// For a toast or the conversation: "The loop ended: it did its 10 runs."
    pub fn words(self) -> String {
        match self {
            LoopEnd::Runs(n) => format!("The loop ended: it did its {n} run{}.", if n == 1 { "" } else { "s" }),
            LoopEnd::Tokens(n) => format!("The loop ended: its runs used {} tokens, its budget.", tokens_words(n)),
            LoopEnd::Time(t) => format!("The loop ended: it's {}, its stop time.", t.words()),
            LoopEnd::Stalled(n) => format!("The loop ended: {n} run{} in a row changed nothing.", if n == 1 { "" } else { "s" }),
            LoopEnd::Failed => "The loop ended: a run didn't finish well.".into(),
            LoopEnd::Stopped => "The loop stopped.".into(),
        }
    }
}

/// "1.2M", "850k", "900".
pub fn tokens_words(n: u64) -> String {
    match n {
        0..1_000 => n.to_string(),
        1_000..1_000_000 => format!("{}k", n / 1_000),
        _ => {
            let tenths = n / 100_000;
            if tenths.is_multiple_of(10) { format!("{}M", tenths / 10) } else { format!("{}.{}M", tenths / 10, tenths % 10) }
        }
    }
}

/// What the loop does after a run.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Next {
    /// Start the next run, fresh.
    Run,
    End(LoopEnd),
}

/// A loop in progress: what its runs have used so far.
#[derive(Debug, Clone)]
pub struct LoopState {
    pub settings: LoopSettings,
    /// When the stop time comes (ms since the epoch), when one is on.
    pub deadline_ms: Option<u64>,
    /// Runs finished.
    pub runs: u32,
    /// Tokens the loop's runs have used, input and output.
    pub tokens: u64,
    /// Runs in a row that changed nothing.
    pub stalls: u32,
    /// A run ended well without its builder reporting any tokens: the token budget can't end
    /// this loop (some builders don't say what they use).
    pub tokens_unreported: bool,
    /// The project's progress fingerprint when the current run started.
    progress: u64,
    /// Tokens counted when the current run started.
    run_tokens_from: u64,
}

impl LoopState {
    /// A loop starting now, with the project as it is (`fingerprint`).
    pub fn start(settings: LoopSettings, now_ms: u64, fingerprint: u64) -> LoopState {
        let settings = settings.sanitized();
        let deadline_ms = settings.stop_at.get().and_then(|at| deadline_ms(now_ms, at));
        LoopState { settings, deadline_ms, runs: 0, tokens: 0, stalls: 0, tokens_unreported: false, progress: fingerprint, run_tokens_from: 0 }
    }

    /// The project's fingerprint as the loop starts (worked out on a worker after `start`).
    pub fn set_progress(&mut self, fingerprint: u64) {
        self.progress = fingerprint;
    }

    /// The person changed the settings mid-loop: later runs use them. A new stop time counts from now.
    pub fn update(&mut self, settings: LoopSettings, now_ms: u64) {
        let settings = settings.sanitized();
        if settings.stop_at != self.settings.stop_at {
            self.deadline_ms = settings.stop_at.get().and_then(|at| deadline_ms(now_ms, at));
        }
        self.settings = settings;
    }

    /// Count a turn's tokens.
    pub fn add_tokens(&mut self, n: u64) {
        self.tokens = self.tokens.saturating_add(n);
    }

    /// A limit that's already reached mid-run (the time came, the tokens ran out), so the run
    /// should stop now rather than at its end.
    pub fn over_limit_now(&self, now_ms: u64) -> Option<LoopEnd> {
        if let Some(budget) = self.settings.tokens.get().filter(|b| self.tokens >= *b) {
            return Some(LoopEnd::Tokens(budget));
        }
        match (self.deadline_ms, self.settings.stop_at.get()) {
            (Some(d), Some(at)) if now_ms >= d => Some(LoopEnd::Time(at)),
            _ => None,
        }
    }

    /// A run finished well with the project now at `fingerprint`: end the loop, or start the
    /// next run.
    pub fn after_run(&mut self, now_ms: u64, fingerprint: u64) -> Next {
        self.runs = self.runs.saturating_add(1);
        if self.tokens == self.run_tokens_from {
            self.tokens_unreported = true;
        }
        self.run_tokens_from = self.tokens;
        self.stalls = if fingerprint == self.progress { self.stalls.saturating_add(1) } else { 0 };
        self.progress = fingerprint;
        if let Some(end) = self.over_limit_now(now_ms) {
            return Next::End(end);
        }
        if let Some(n) = self.settings.stalled_runs.get().filter(|n| self.stalls >= *n) {
            return Next::End(LoopEnd::Stalled(n));
        }
        if let Some(n) = self.settings.runs.get().filter(|n| self.runs >= *n) {
            return Next::End(LoopEnd::Runs(n));
        }
        Next::Run
    }

    /// Where the loop is, for the settings popover: "Run 3 of 10 · 1.2M tokens · until 7am".
    pub fn status(&self) -> String {
        let run = self.runs.saturating_add(1);
        let mut parts = vec![match self.settings.runs.get() {
            Some(n) => format!("Run {run} of {n}"),
            None => format!("Run {run}"),
        }];
        parts.push(match self.settings.tokens.get() {
            Some(b) => format!("{} of {} tokens", tokens_words(self.tokens), tokens_words(b)),
            None => format!("{} tokens", tokens_words(self.tokens)),
        });
        if let Some(at) = self.settings.stop_at.get().filter(|_| self.deadline_ms.is_some()) {
            parts.push(format!("until {}", at.words()));
        }
        parts.join(" · ")
    }
}

/// The next time `at` comes round on this computer's clock after `now_ms` (ms since the epoch).
/// A time that's now or already past today means tomorrow's.
pub fn deadline_ms(now_ms: u64, at: ClockTime) -> Option<u64> {
    let tz = jiff::tz::TimeZone::system();
    deadline_in(&tz, now_ms, at)
}

fn deadline_in(tz: &jiff::tz::TimeZone, now_ms: u64, at: ClockTime) -> Option<u64> {
    let at = at.clamped();
    let now = jiff::Timestamp::from_millisecond(i64::try_from(now_ms).ok()?).ok()?.to_zoned(tz.clone());
    let time = jiff::civil::Time::new(i8::try_from(at.hour).ok()?, i8::try_from(at.minute).ok()?, 0, 0).ok()?;
    let mut day = now.date();
    for _ in 0..3 {
        // A time a clock change skips is taken as the moment after the gap.
        let when = day.to_datetime(time).to_zoned(tz.clone()).ok()?;
        if when.timestamp() > now.timestamp() {
            return u64::try_from(when.timestamp().as_millisecond()).ok();
        }
        day = day.tomorrow().ok()?;
    }
    None
}

/// Most files of the app looked at for progress, so a huge tree stays quick.
const MAX_APP_FILES: usize = 20_000;

/// A fingerprint of the project's progress: the investigation's memory (findings, hypotheses,
/// plan, open questions) and the app's files (names, sizes, times). Two equal fingerprints
/// around a run mean it changed nothing. Reads the disk: call it off the UI thread.
pub fn fingerprint(ws: &Workspace) -> u64 {
    let mut h = DefaultHasher::new();
    let inv = ws.investigation_dir();
    if let Ok(bytes) = std::fs::read(inv.join("investigation").join("state.json")) {
        match serde_json::from_slice::<serde_json::Value>(&bytes) {
            // Only what the builder learned: the handoff and stamps change every run.
            Ok(serde_json::Value::Object(mut map)) => {
                for k in ["handoff", "updated", "revision"] {
                    map.remove(k);
                }
                serde_json::Value::Object(map).to_string().hash(&mut h);
            }
            _ => bytes.hash(&mut h),
        }
    }
    let mut seen = 0usize;
    hash_tree(&inv.join("unknowns"), &mut h, &mut seen, &|_| false, true);
    let app = ws.app_dir();
    // Build output and the checks' captures change on every run whatever the builder did.
    let checks = ws.checks_dir();
    let skip = |p: &Path| {
        p.file_name().and_then(|n| n.to_str()).is_some_and(|n| {
            matches!(n, "target" | "node_modules" | "dist" | "build" | ".git" | "__pycache__" | ".venv" | ".next" | ".godot" | ".gradle" | ".dart_tool")
        }) || p.starts_with(&checks)
            || ws.is_own_record(p)
    };
    let mut seen = 0usize;
    hash_tree(&app, &mut h, &mut seen, &skip, false);
    h.finish()
}

/// Hash a folder's files in name order: their paths, sizes and times, and with `contents`
/// their bytes too.
fn hash_tree(dir: &Path, h: &mut DefaultHasher, seen: &mut usize, skip: &dyn Fn(&Path) -> bool, contents: bool) {
    let Ok(read) = std::fs::read_dir(dir) else { return };
    let mut entries: Vec<_> = read.filter_map(Result::ok).collect();
    entries.sort_by_key(|e| e.file_name());
    for e in entries {
        if *seen >= MAX_APP_FILES {
            return;
        }
        let path = e.path();
        if skip(&path) {
            continue;
        }
        let Ok(kind) = e.file_type() else { continue };
        if kind.is_dir() {
            hash_tree(&path, h, seen, skip, contents);
        } else if kind.is_file() {
            *seen += 1;
            path.hash(h);
            if contents {
                std::fs::read(&path).unwrap_or_default().hash(h);
            } else if let Ok(meta) = e.metadata() {
                meta.len().hash(h);
                meta.modified().ok().hash(h);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use craft_anything_engine::workspace::TargetRef;
    use craft_anything_engine::workspace::build_loop::Limit;

    const HOUR: u64 = 3_600_000;

    fn settings() -> LoopSettings {
        LoopSettings { on: true, ..LoopSettings::default() }
    }

    #[test]
    fn a_loop_ends_after_its_runs() {
        let mut s = LoopState::start(LoopSettings { runs: Limit { on: true, value: 3 }, ..settings() }, 0, 0);
        assert_eq!([s.after_run(0, 1), s.after_run(0, 2)], [Next::Run, Next::Run]);
        assert_eq!(s.status(), "Run 3 of 3 · 0 tokens");
        assert_eq!(s.after_run(0, 3), Next::End(LoopEnd::Runs(3)));
    }

    #[test]
    fn runs_that_change_nothing_end_it() {
        let mut s = LoopState::start(LoopSettings { runs: Limit { on: false, value: 1 }, ..settings() }, 0, 7);
        assert_eq!(s.after_run(0, 7), Next::Run, "one stall of two");
        assert_eq!(s.after_run(0, 8), Next::Run, "progress resets the count");
        assert_eq!(s.after_run(0, 8), Next::Run);
        assert_eq!(s.after_run(0, 8), Next::End(LoopEnd::Stalled(2)));
    }

    #[test]
    fn the_token_budget_and_the_stop_time_end_it_even_mid_run() {
        let tokens = LoopSettings { tokens: Limit { on: true, value: 1_000 }, ..settings() };
        let mut s = LoopState::start(tokens, 0, 0);
        s.add_tokens(999);
        assert_eq!(s.over_limit_now(0), None);
        s.add_tokens(1);
        assert_eq!(s.over_limit_now(0), Some(LoopEnd::Tokens(1_000)));
        assert_eq!(s.after_run(0, 1), Next::End(LoopEnd::Tokens(1_000)));

        let mut s = LoopState::start(settings(), 0, 0);
        s.deadline_ms = Some(HOUR);
        s.settings.stop_at.on = true;
        assert_eq!(s.over_limit_now(HOUR - 1), None);
        assert_eq!(s.over_limit_now(HOUR), Some(LoopEnd::Time(ClockTime { hour: 7, minute: 0 })));
        assert!(s.status().ends_with("until 7am"), "{}", s.status());
    }

    #[test]
    fn the_stop_time_is_the_next_time_it_comes_round() {
        let utc = jiff::tz::TimeZone::UTC;
        let seven = ClockTime { hour: 7, minute: 0 };
        // 1970-01-01 6:00 UTC → 7:00 the same day; 8:00 → 7:00 the next day; 7:00 itself → tomorrow.
        assert_eq!(deadline_in(&utc, 6 * HOUR, seven), Some(7 * HOUR));
        assert_eq!(deadline_in(&utc, 8 * HOUR, seven), Some(31 * HOUR));
        assert_eq!(deadline_in(&utc, 7 * HOUR, seven), Some(31 * HOUR));
        let fixed = jiff::tz::TimeZone::fixed(jiff::tz::offset(-7));
        assert_eq!(deadline_in(&fixed, 0, seven), Some(14 * HOUR), "7am at UTC-7 is 2pm UTC");
    }

    #[test]
    fn token_counts_read_short() {
        assert_eq!([tokens_words(900), tokens_words(85_000), tokens_words(1_000_000), tokens_words(1_250_000)], ["900", "85k", "1M", "1.2M"]);
        assert!(LoopEnd::Runs(1).words().contains("its 1 run."));
    }

    #[test]
    fn the_fingerprint_moves_with_findings_and_files_not_the_handoff() {
        let d = tempfile::tempdir().unwrap();
        let ws = Workspace::create(d.path(), "Game", "A game", TargetRef::None).unwrap();
        let inv = ws.investigation_dir().join("investigation");
        std::fs::create_dir_all(&inv).unwrap();
        let state = |findings: &str, handoff: &str| {
            std::fs::write(inv.join("state.json"), format!(r#"{{"revision": 1, "findings": [{findings}], "handoff": "{handoff}"}}"#)).unwrap();
        };
        state("", "first");
        let a = fingerprint(&ws);
        state("", "second");
        assert_eq!(fingerprint(&ws), a, "a new handoff alone isn't progress");
        state(r#"{"id": 1}"#, "second");
        let b = fingerprint(&ws);
        assert_ne!(b, a, "a finding is");
        std::fs::create_dir_all(ws.app_dir()).unwrap();
        std::fs::write(ws.app_dir().join("main.js"), "x").unwrap();
        assert_ne!(fingerprint(&ws), b, "so is a new file in the app");
    }

    #[test]
    fn a_builder_that_reports_no_tokens_is_noticed() {
        let mut s = LoopState::start(settings(), 0, 0);
        s.add_tokens(500);
        assert_eq!(s.after_run(0, 1), Next::Run);
        assert!(!s.tokens_unreported);
        assert_eq!(s.after_run(0, 2), Next::Run);
        assert!(s.tokens_unreported, "a run that ended well with no tokens counted");
    }

    #[test]
    fn checks_in_an_app_edited_in_place_are_not_progress() {
        let d = tempfile::tempdir().unwrap();
        let mine = d.path().join("mine");
        std::fs::create_dir_all(&mine).unwrap();
        std::fs::write(mine.join("index.html"), "x").unwrap();
        let ws = Workspace::create_at(&mine, "Mine", "Add tabs", TargetRef::None, Some(craft_anything_engine::workspace::ExistingFiles::Edit)).unwrap();
        assert_eq!(ws.app_dir(), ws.root, "the app is the project folder");
        let a = fingerprint(&ws);
        std::fs::create_dir_all(ws.checks_dir().join("round-1")).unwrap();
        std::fs::write(ws.checks_dir().join("round-1").join("shot.png"), "png").unwrap();
        assert_eq!(fingerprint(&ws), a, "a check's capture isn't the builder's progress");
        std::fs::write(mine.join("index.html"), "xy").unwrap();
        assert_ne!(fingerprint(&ws), a, "a changed app file is");
    }
}
