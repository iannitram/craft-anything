//! The inbox: how an agent working on a project asks the person something — a question with
//! choices, or approval for an action — and waits for the answer the app writes back.
//!
//! It is a folder, so it works across processes (the app, our MCP server started by a coding
//! agent, the in-process agent loop): `<id>.request.json` is written by the asker and
//! `<id>.answer.json` by the app. Both are written atomically, owner-only, because a request can
//! carry the person's own words and an answer can carry a typed reply or an approval; the asker
//! removes both once it has read the answer. Ids are generated here and validated on every path
//! join.

use crate::project::write_atomic;
use craft_anything_evidence::ToolError;
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

/// The session's inbox folder (kept in `Session::ext`). Set by `mcp --inbox <dir>` or by the
/// workflow that runs the in-process agent.
#[derive(Debug, Clone)]
pub struct InboxDir(pub PathBuf);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RequestKind {
    /// A question; the answer is a choice and/or free text.
    Question,
    /// Permission for an action; the answer is allow/deny.
    Approval,
}

/// Something waiting for the person.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Request {
    pub id: String,
    pub kind: RequestKind,
    /// The question, or a one-line description of the action ("Run `npm install`").
    pub title: String,
    /// Optional context in plain words.
    #[serde(default)]
    pub detail: String,
    /// Suggested answers, shown as buttons.
    #[serde(default)]
    pub choices: Vec<String>,
    /// Whether a typed answer is accepted too.
    #[serde(default)]
    pub allow_text: bool,
    /// For approvals: the action's kind, so "always allow" can be remembered per kind.
    #[serde(default)]
    pub action: Option<String>,
    pub created_ms: u64,
}

/// The person's answer.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Answer {
    #[serde(default)]
    pub choice: Option<String>,
    #[serde(default)]
    pub text: Option<String>,
    /// Approvals: true to allow.
    #[serde(default)]
    pub allow: Option<bool>,
    /// Approvals: allow this kind of action for the rest of the project.
    #[serde(default)]
    pub always: bool,
}

impl Answer {
    /// The answer as one line of text for an agent.
    pub fn as_text(&self) -> String {
        match (&self.choice, self.text.as_deref().map(str::trim).filter(|t| !t.is_empty())) {
            (Some(c), Some(t)) => format!("{c} — {t}"),
            (Some(c), None) => c.clone(),
            (None, Some(t)) => t.to_string(),
            (None, None) => String::new(),
        }
    }
}

#[derive(Debug, Clone)]
pub struct Inbox {
    dir: PathBuf,
}

static COUNTER: AtomicU64 = AtomicU64::new(0);

pub fn now_ms() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_millis() as u64).unwrap_or(0)
}

/// A number that never repeats within this process, for names that must stay unique even when
/// two are made in the same millisecond (or a name was just freed by deletion).
pub fn next_counter() -> u64 {
    COUNTER.fetch_add(1, Ordering::SeqCst)
}

/// Ids are `[0-9a-z-]` only, so they are always safe as file names.
fn valid_id(id: &str) -> bool {
    !id.is_empty() && id.len() <= 64 && id.bytes().all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
}

impl Inbox {
    pub fn new(dir: impl Into<PathBuf>) -> Inbox {
        Inbox { dir: dir.into() }
    }

    pub fn dir(&self) -> &Path {
        &self.dir
    }

    fn path(&self, id: &str, suffix: &str) -> Result<PathBuf, ToolError> {
        if !valid_id(id) {
            return Err(ToolError::invalid(format!("{id:?} is not an inbox id")));
        }
        Ok(self.dir.join(format!("{id}.{suffix}.json")))
    }

    /// Post a request; returns its id.
    pub fn post(
        &self,
        kind: RequestKind,
        title: &str,
        detail: &str,
        choices: Vec<String>,
        allow_text: bool,
        action: Option<String>,
    ) -> Result<String, ToolError> {
        std::fs::create_dir_all(&self.dir).map_err(|e| ToolError::io(self.dir.display(), e))?;
        let n = next_counter();
        let id = format!("{}-{}-{n}", now_ms(), std::process::id());
        let req = Request { id: id.clone(), kind, title: title.to_string(), detail: detail.to_string(), choices, allow_text, action, created_ms: now_ms() };
        let bytes = serde_json::to_vec_pretty(&req).map_err(|e| ToolError::internal(e.to_string()))?;
        craft_anything_platform::fs::write_atomic_private(&self.path(&id, "request")?, &bytes)?;
        Ok(id)
    }

    /// Requests without an answer yet, oldest first. Unreadable files are skipped.
    pub fn pending(&self) -> Vec<Request> {
        let Ok(rd) = std::fs::read_dir(&self.dir) else { return Vec::new() };
        let mut out: Vec<Request> = rd
            .flatten()
            .filter_map(|e| e.file_name().to_str().and_then(|n| n.strip_suffix(".request.json")).map(str::to_string))
            .filter(|id| valid_id(id) && !self.dir.join(format!("{id}.answer.json")).exists())
            .filter_map(|id| std::fs::read(self.dir.join(format!("{id}.request.json"))).ok())
            .filter_map(|b| serde_json::from_slice::<Request>(&b).ok())
            .collect();
        out.sort_by_key(|r| r.created_ms);
        out
    }

    /// Write the person's answer.
    pub fn answer(&self, id: &str, answer: &Answer) -> Result<(), ToolError> {
        let bytes = serde_json::to_vec(answer).map_err(|e| ToolError::internal(e.to_string()))?;
        craft_anything_platform::fs::write_atomic_private(&self.path(id, "answer")?, &bytes)
    }

    /// Wait for the answer to `id` (polling), until `timeout` or `cancelled()`. The request and
    /// answer files are removed once read; on timeout or cancel the request is withdrawn.
    pub fn wait(&self, id: &str, timeout: Duration, cancelled: &dyn Fn() -> bool) -> Option<Answer> {
        let (req, ans) = (self.path(id, "request").ok()?, self.path(id, "answer").ok()?);
        let start = Instant::now();
        loop {
            if let Ok(bytes) = std::fs::read(&ans)
                && let Ok(a) = serde_json::from_slice::<Answer>(&bytes)
            {
                let _ = std::fs::remove_file(&ans);
                let _ = std::fs::remove_file(&req);
                return Some(a);
            }
            if cancelled() || start.elapsed() >= timeout {
                let _ = std::fs::remove_file(&req);
                return None;
            }
            std::thread::sleep(Duration::from_millis(200));
        }
    }
}

/// The folder in the inbox where tools say what they are waiting on.
const ACTIVITY: &str = "activity";

/// A line a tool working on a project says while it waits (a Ghidra decompile, #1714), for the
/// run that shows the project's feed. Our MCP server runs inside the coding agent, so a tool's own
/// progress reaches only the agent; this file reaches the app. Removed when the wait ends.
pub struct Activity {
    file: PathBuf,
}

impl Activity {
    pub fn start(inbox: &Path) -> Activity {
        Activity { file: inbox.join(ACTIVITY).join(format!("{}-{}.txt", std::process::id(), next_counter())) }
    }

    /// Keep one notice per approval kind until the next build clears the activity folder.
    pub(crate) fn approval_refused(inbox: &Path, kind: crate::workspace::ActionKind) {
        let path = inbox.join(ACTIVITY).join(format!("approval-{}.txt", kind.name()));
        Self::write(
            &path,
            &format!(
                "The builder was refused permission for {}. To ask before trying again, use Approvals or type /approvals {} ask.",
                kind.what(),
                kind.name()
            ),
        );
    }

    /// Say `line`, in place of what this wait said before.
    pub fn say(&self, line: &str) {
        Self::write(&self.file, line);
    }

    fn write(file: &Path, line: &str) {
        let written = match file.parent() {
            Some(dir) => std::fs::create_dir_all(dir).map_err(|e| e.to_string()).and_then(|()| write_atomic(file, line.as_bytes()).map_err(|e| e.message)),
            None => Ok(()),
        };
        if let Err(e) = written {
            log::warn!("Couldn't tell the app {line:?}: {e}");
        }
    }
}

impl Drop for Activity {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.file);
    }
}

/// What the tools working on the project are saying now: (which wait, its line), oldest first.
pub fn activity(inbox: &Path) -> Vec<(String, String)> {
    let Ok(entries) = std::fs::read_dir(inbox.join(ACTIVITY)) else { return Vec::new() };
    let mut lines: Vec<(String, String)> = entries
        .flatten()
        .filter_map(|e| {
            let name = e.file_name().to_str()?.strip_suffix(".txt")?.to_string();
            let line = std::fs::read_to_string(e.path()).ok()?;
            Some((name, line.trim().to_string())).filter(|(_, l)| !l.is_empty())
        })
        .collect();
    lines.sort();
    lines
}

/// Forget what tools said in an earlier run: a tool that was stopped mid-wait left its line.
pub fn clear_activity(inbox: &Path) {
    let _ = std::fs::remove_dir_all(inbox.join(ACTIVITY));
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_tool_says_what_it_waits_on_until_it_stops() {
        let d = tempfile::tempdir().unwrap();
        assert!(activity(d.path()).is_empty(), "no folder, nothing said");
        let first = Activity::start(d.path());
        first.say("Ghidra is decompiling this function (1 min so far; it gives up at 15 min)");
        first.say("Ghidra is decompiling this function (2 min so far; it gives up at 15 min)");
        let second = Activity::start(d.path());
        second.say("Waiting for Ghidra to finish decompiling another function (1 min so far)");
        let now = activity(d.path());
        assert_eq!(now.len(), 2, "{now:?}");
        assert!(now.iter().any(|(_, l)| l.starts_with("Ghidra is decompiling this function (2 min")), "the latest line replaces the last");
        drop(first);
        let now = activity(d.path());
        assert_eq!(now.len(), 1);
        assert!(now[0].1.starts_with("Waiting for Ghidra"));
        std::mem::forget(second);
        clear_activity(d.path());
        assert!(activity(d.path()).is_empty(), "a wait cut off mid-line is forgotten by the next run");
    }

    #[test]
    fn question_round_trip_across_handles() {
        let dir = tempfile::tempdir().unwrap();
        let asker = Inbox::new(dir.path().join("inbox"));
        let app = Inbox::new(dir.path().join("inbox"));
        let id = asker.post(RequestKind::Question, "Which colour?", "", vec!["Blue".into(), "Red".into()], true, None).unwrap();
        let pending = app.pending();
        assert_eq!(pending.len(), 1);
        assert_eq!(pending[0].choices, vec!["Blue", "Red"]);
        app.answer(&id, &Answer { choice: Some("Red".into()), text: Some(" darker ".into()), ..Default::default() }).unwrap();
        assert!(app.pending().is_empty(), "answered requests are no longer pending");
        let a = asker.wait(&id, Duration::from_secs(2), &|| false).unwrap();
        assert_eq!(a.as_text(), "Red — darker");
        assert_eq!(std::fs::read_dir(dir.path().join("inbox")).unwrap().count(), 0, "both files are cleaned up");
    }

    #[test]
    fn timeout_withdraws_and_bad_ids_are_refused() {
        let dir = tempfile::tempdir().unwrap();
        let inbox = Inbox::new(dir.path());
        let id = inbox.post(RequestKind::Approval, "Run npm install", "", vec![], false, Some("bash".into())).unwrap();
        assert!(inbox.wait(&id, Duration::from_millis(50), &|| false).is_none());
        assert!(inbox.pending().is_empty());
        assert!(inbox.answer("../escape", &Answer::default()).is_err());
        assert!(inbox.answer("A", &Answer::default()).is_err());
    }

    /// A request can carry the person's own words and an answer can carry a typed reply or an
    /// approval; another local account must not be able to read either from the project folder.
    #[cfg(unix)]
    #[test]
    fn requests_and_answers_are_owner_only() {
        use std::os::unix::fs::PermissionsExt as _;
        let dir = tempfile::tempdir().unwrap();
        let inbox = Inbox::new(dir.path());
        let id = inbox.post(RequestKind::Question, "Which folder?", "the one on the desktop", vec![], true, None).unwrap();
        let request = dir.path().join(format!("{id}.request.json"));
        assert_eq!(std::fs::metadata(&request).unwrap().permissions().mode() & 0o777, 0o600, "the request is readable only by its owner");
        inbox.answer(&id, &Answer { text: Some("the blue one".into()), ..Default::default() }).unwrap();
        let answer = dir.path().join(format!("{id}.answer.json"));
        assert_eq!(std::fs::metadata(&answer).unwrap().permissions().mode() & 0o777, 0o600, "the answer is readable only by its owner");
    }
}
