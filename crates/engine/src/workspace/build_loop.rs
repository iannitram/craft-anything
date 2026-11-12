//! A project's build loop settings (`Manifest::build_loop`): the person lets the builder keep
//! working run after run, each run with a fresh builder that picks up from the project's
//! investigation memory, until a limit they chose ends it. The loop itself is driven by the
//! rebuild workflow (`craft_anything_ai::rebuild::build_loop`); this is only what's saved.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// What each run in a loop is told when the person hasn't written their own.
pub const DEFAULT_LOOP_PROMPT: &str = "Carry on from where the last builder stopped. Read the investigation's handoff, plan and open \
questions first. Resolve what you can from the original with evidence, recording findings as you go, then improve the build \
where the findings show it differs from the original. Before you finish, write a handoff saying what you did, what you \
learned and what the next builder should do first.";

/// A limit the person can switch on or off; its value is kept while it's off.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct Limit<T> {
    pub on: bool,
    pub value: T,
}

impl<T: Copy> Limit<T> {
    /// The value when the limit is on.
    pub fn get(&self) -> Option<T> {
        self.on.then_some(self.value)
    }
}

/// A time of day on the person's clock, 24-hour.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ClockTime {
    pub hour: u8,
    pub minute: u8,
}

impl ClockTime {
    /// The same time, kept within a day (a hand-edited file may hold anything).
    pub fn clamped(self) -> ClockTime {
        ClockTime { hour: self.hour.min(23), minute: self.minute.min(59) }
    }

    /// "7am", "4:45pm".
    pub fn words(self) -> String {
        let c = self.clamped();
        let (h, half) = match c.hour {
            0 => (12, "am"),
            h @ 1..=11 => (h, "am"),
            12 => (12, "pm"),
            h => (h - 12, "pm"),
        };
        if c.minute == 0 { format!("{h}{half}") } else { format!("{h}:{:02}{half}", c.minute) }
    }
}

/// The person's build loop for one project.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(default)]
pub struct LoopSettings {
    /// The loop is switched on: a run that ends well starts the next one.
    pub on: bool,
    /// What each run is told; empty means [`DEFAULT_LOOP_PROMPT`].
    pub prompt: String,
    /// Refinement rounds in each run before the next run boots a fresh builder.
    pub rounds_per_run: u32,
    /// End after this many runs.
    pub runs: Limit<u32>,
    /// End once the loop's runs have used this many tokens (input and output) together.
    pub tokens: Limit<u64>,
    /// End at this time on the person's clock (the next time it comes round after the loop starts).
    pub stop_at: Limit<ClockTime>,
    /// End after this many runs in a row that changed nothing: no new findings, questions,
    /// hypotheses or plan steps, and no change to the build's files.
    pub stalled_runs: Limit<u32>,
}

impl Default for LoopSettings {
    fn default() -> Self {
        LoopSettings {
            on: false,
            prompt: String::new(),
            rounds_per_run: 2,
            runs: Limit { on: true, value: 10 },
            tokens: Limit { on: false, value: 20_000_000 },
            stop_at: Limit { on: false, value: ClockTime { hour: 7, minute: 0 } },
            stalled_runs: Limit { on: true, value: 2 },
        }
    }
}

/// Most rounds one run may be given: a loop is many short runs, not one long one.
pub const MAX_ROUNDS_PER_RUN: u32 = 20;

impl LoopSettings {
    /// What each run is told.
    pub fn prompt(&self) -> &str {
        let p = self.prompt.trim();
        if p.is_empty() { DEFAULT_LOOP_PROMPT } else { p }
    }

    /// The rounds each run gets, kept within what a run can do.
    pub fn rounds(&self) -> u32 {
        self.rounds_per_run.clamp(1, MAX_ROUNDS_PER_RUN)
    }

    /// The settings with every value in range (a hand-edited craft.json may hold anything).
    pub fn sanitized(mut self) -> LoopSettings {
        self.rounds_per_run = self.rounds();
        self.runs.value = self.runs.value.max(1);
        self.tokens.value = self.tokens.value.max(1);
        self.stop_at.value = self.stop_at.value.clamped();
        self.stalled_runs.value = self.stalled_runs.value.max(1);
        self
    }

    /// Whether any limit can end the loop. A loop with none would run until stopped.
    pub fn has_limit(&self) -> bool {
        self.runs.on || self.tokens.on || self.stop_at.on || self.stalled_runs.on
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn times_read_the_way_people_write_them() {
        let t = |hour, minute| ClockTime { hour, minute }.words();
        assert_eq!([t(0, 0), t(7, 0), t(12, 0), t(16, 45), t(23, 5)], ["12am", "7am", "12pm", "4:45pm", "11:05pm"]);
        assert_eq!(t(99, 99), "11:59pm", "out of range is clamped");
    }

    #[test]
    fn an_empty_prompt_is_the_default_and_values_stay_in_range() {
        let s = LoopSettings { prompt: "  ".into(), rounds_per_run: 0, runs: Limit { on: true, value: 0 }, ..LoopSettings::default() };
        assert_eq!(s.prompt(), DEFAULT_LOOP_PROMPT);
        let s = s.sanitized();
        assert_eq!((s.rounds_per_run, s.runs.value), (1, 1));
        assert_eq!(LoopSettings { rounds_per_run: 500, ..LoopSettings::default() }.rounds(), MAX_ROUNDS_PER_RUN);
    }

    #[test]
    fn missing_fields_take_their_defaults() {
        let s: LoopSettings = serde_json::from_str(r#"{"on": true, "runs": {"on": false, "value": 3}}"#).unwrap();
        assert!(s.on && !s.runs.on && s.stalled_runs.on);
        assert_eq!((s.runs.value, s.rounds_per_run), (3, 2));
        assert!(s.has_limit());
        let none = LoopSettings { runs: Limit { on: false, value: 1 }, stalled_runs: Limit { on: false, value: 1 }, ..LoopSettings::default() };
        assert!(!none.has_limit());
    }
}
