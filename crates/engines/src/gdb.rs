//! A GDB session over the machine interface (GDB/MI): one owned `gdb` process (stdin piped,
//! output drained on threads, killed with its process group when closed or dropped), driven with
//! MI commands, `-interpreter-exec console` for any console command, while results come back as
//! structured records. The engine's GDB tools keep sessions by id and turn results into evidence.

use craft_anything_evidence::{ErrorCode, ToolError};
use craft_anything_platform::sync::Mutex;
use craft_anything_platform::{Process, RunningProcess, StdinMode};
use serde::Serialize;
use serde_json::{Value, json};
use std::collections::VecDeque;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::ChildStdin;
use std::sync::{Arc, PoisonError};
use std::time::{Duration, Instant};

/// Unread MI lines kept per session; older ones are dropped.
const MAX_PENDING: usize = 200_000;
/// Text returned per command.
const MAX_TEXT: usize = 1 << 20;

/// MI lines read from GDB's stdout, waiting to be consumed.
#[derive(Default)]
struct Pending {
    lines: VecDeque<String>,
    dropped: u64,
}

/// What one command (or the output since the last one) produced.
#[derive(Debug, Default, Serialize)]
pub struct Collected {
    pub console: String,
    pub log: String,
    pub target_output: String,
    pub events: Vec<Value>,
    /// The result class: done, running, error, exit.
    pub result: Option<String>,
    pub error: Option<String>,
    /// More than 1 MiB of text arrived; the rest was dropped.
    pub truncated: bool,
}

impl Collected {
    fn push(dst: &mut String, s: &str, truncated: &mut bool) {
        if dst.len() + s.len() > MAX_TEXT {
            *truncated = true;
        } else {
            dst.push_str(s);
        }
    }
}

/// One running GDB.
pub struct GdbSession {
    pub program: Option<PathBuf>,
    pub gdb: PathBuf,
    proc: RunningProcess,
    stdin: Option<ChildStdin>,
    pending: Arc<Mutex<Pending>>,
    next_token: u64,
    /// not_started, stopped, running, exited
    pub state: &'static str,
    pub last_stop: Option<Value>,
    pub started: Instant,
}

// ---------------------------------------------------------------------------------------------
// GDB/MI

/// Decode an MI C-string (`"text\n"`) into plain text.
fn unquote(s: &str) -> String {
    let inner = s.strip_prefix('"').and_then(|x| x.strip_suffix('"')).unwrap_or(s);
    let mut out: Vec<u8> = Vec::with_capacity(inner.len());
    let mut it = inner.chars().peekable();
    while let Some(c) = it.next() {
        if c != '\\' {
            out.extend_from_slice(c.encode_utf8(&mut [0u8; 4]).as_bytes());
            continue;
        }
        match it.next() {
            Some('a') => out.push(0x07),
            Some('b') => out.push(0x08),
            Some('f') => out.push(0x0c),
            Some('n') => out.push(b'\n'),
            Some('r') => out.push(b'\r'),
            Some('t') => out.push(b'\t'),
            Some('v') => out.push(0x0b),
            Some('e') => out.push(0x1b),
            Some('x') => match it.peek().and_then(|c| c.to_digit(16)) {
                Some(hi) => {
                    it.next();
                    let v = match it.peek().and_then(|c| c.to_digit(16)) {
                        Some(lo) => {
                            it.next();
                            hi * 16 + lo
                        }
                        None => hi,
                    };
                    out.push(v as u8);
                }
                None => out.push(b'x'),
            },
            Some(d @ '0'..='7') => {
                let mut v = d.to_digit(8).unwrap_or(0);
                for _ in 0..2 {
                    match it.peek().and_then(|c| c.to_digit(8)) {
                        Some(x) => {
                            v = v * 8 + x;
                            it.next();
                        }
                        None => break,
                    }
                }
                out.push(u8::try_from(v).unwrap_or(b'?'));
            }
            Some(other) => out.extend_from_slice(other.encode_utf8(&mut [0u8; 4]).as_bytes()),
            None => out.push(b'\\'),
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// Quote a console command or log line as a single-line MI C-string.
fn quote(s: &str) -> String {
    use std::fmt::Write;
    let mut out = String::with_capacity(s.len() + 2);
    out.push('"');
    for c in s.chars() {
        match c {
            '\\' => out.push_str("\\\\"),
            '"' => out.push_str("\\\""),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if c.is_control() => {
                for &b in c.encode_utf8(&mut [0u8; 4]).as_bytes() {
                    let _ = write!(out, "\\{b:03o}");
                }
            }
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

/// `key="value"` pairs at the top level of an MI record body (nested tuples kept as raw text).
fn fields(body: &str) -> serde_json::Map<String, Value> {
    let mut m = serde_json::Map::new();
    let b = body.as_bytes();
    let mut i = 0usize;
    while i < b.len() {
        let Some(eq) = body.get(i..).and_then(|r| r.find('=')).map(|e| i + e) else { break };
        let key = body.get(i..eq).unwrap_or("").trim_start_matches(',').to_string();
        let mut j = eq + 1;
        let start = j;
        let (mut depth, mut in_str, mut esc) = (0i32, false, false);
        while j < b.len() {
            let c = b.get(j).copied().unwrap_or(b',');
            if in_str {
                if esc {
                    esc = false;
                } else if c == b'\\' {
                    esc = true;
                } else if c == b'"' {
                    in_str = false;
                }
            } else {
                match c {
                    b'"' => in_str = true,
                    b'{' | b'[' => depth += 1,
                    b'}' | b']' => depth = (depth - 1).max(0),
                    b',' if depth == 0 => break,
                    _ => {}
                }
            }
            j += 1;
        }
        let raw = body.get(start..j).unwrap_or("");
        let v = if raw.starts_with('"') { Value::String(unquote(raw)) } else { Value::String(raw.to_string()) };
        if !key.is_empty() {
            m.insert(key, v);
        }
        i = j + 1;
    }
    m
}

/// The token-prefixed result class of a line (`12^done,...` → (12, "done", body)).
fn result_record(line: &str) -> Option<(Option<u64>, &str, &str)> {
    let digits = line.bytes().take_while(u8::is_ascii_digit).count();
    let rest = line.get(digits..)?.strip_prefix('^')?;
    let token = line.get(..digits).and_then(|t| t.parse().ok());
    let (class, body) = rest.split_once(',').unwrap_or((rest, ""));
    Some((token, class, body))
}

/// Fold one MI line into `c`, updating the session's state. When `expected_token` is `Some`,
/// only result records bearing that token update `c.result` and `c.error`.
fn absorb(line: &str, expected_token: Option<u64>, s: &mut GdbSession, c: &mut Collected) {
    let trimmed = line.trim_end();
    if let Some((rec_token, class, body)) = result_record(trimmed) {
        if class == "exit" {
            s.state = "exited";
        }
        if expected_token.is_none() || rec_token == expected_token {
            c.result = Some(class.to_string());
            c.error = if class == "error" { fields(body).get("msg").and_then(Value::as_str).map(str::to_string) } else { None };
        }
        return;
    }
    let Some(first) = trimmed.chars().next() else { return };
    let rest = trimmed.get(1..).unwrap_or("");
    match first {
        '~' => Collected::push(&mut c.console, &unquote(rest), &mut c.truncated),
        '&' => Collected::push(&mut c.log, &unquote(rest), &mut c.truncated),
        '@' => Collected::push(&mut c.target_output, &unquote(rest), &mut c.truncated),
        '*' | '=' => {
            let (class, body) = rest.split_once(',').unwrap_or((rest, ""));
            match class {
                "running" => s.state = "running",
                "stopped" => {
                    let f = Value::Object(fields(body));
                    s.state = if f.get("reason").and_then(Value::as_str).is_some_and(|r| r.starts_with("exited")) { "exited" } else { "stopped" };
                    s.last_stop = Some(f);
                }
                "thread-group-exited" => s.state = "exited",
                _ => {}
            }
            if c.events.len() < 1000 {
                c.events.push(json!({"kind": if first == '*' { "exec" } else { "notify" }, "class": class, "fields": fields(body)}));
            }
        }
        _ if trimmed.starts_with("(gdb)") => {}
        _ => Collected::push(&mut c.target_output, &format!("{line}\n"), &mut c.truncated),
    }
}

/// Take every pending line; stop early (returning the rest to the queue) once `until` matches.
fn drain(s: &mut GdbSession, c: &mut Collected, expected_token: Option<u64>, until: &mut dyn FnMut(&str, &mut GdbSession) -> bool) -> bool {
    let lines: Vec<String> = {
        let mut p = s.pending.lock().unwrap_or_else(PoisonError::into_inner);
        p.lines.drain(..).collect()
    };
    let mut it = lines.into_iter();
    while let Some(l) = it.next() {
        let line = l.trim_end_matches(['\r', '\n']);
        absorb(line, expected_token, s, c);
        if until(line, s) {
            let rest: Vec<String> = it.collect();
            let mut p = s.pending.lock().unwrap_or_else(PoisonError::into_inner);
            for (i, r) in rest.into_iter().enumerate() {
                p.lines.insert(i, r);
            }
            return true;
        }
    }
    false
}

/// Record the result class; `^running` means we keep waiting for a stop.
fn c_result(class: &str, sess: &mut GdbSession, waiting_for_stop: &mut bool) {
    match class {
        "running" => {
            sess.state = "running";
            *waiting_for_stop = true;
        }
        "exit" => sess.state = "exited",
        _ => {}
    }
}

impl GdbSession {
    /// Start GDB (machine interface, no init files, auto-loaded scripts and pagination off) with
    /// `program` loaded (not run), optionally a core file and the program's arguments.
    pub fn start(gdb: &Path, program: Option<&PathBuf>, core: Option<&PathBuf>, args: &[String]) -> Result<Self, ToolError> {
        let mut a: Vec<String> = vec!["-nx".into(), "-q".into(), "--interpreter=mi".into()];
        for set in ["set auto-load off", "set pagination off", "set confirm off", "set width 0", "set height 0", "set debuginfod enabled off"] {
            a.extend(["-iex".to_string(), set.to_string()]);
        }
        if let Some(c) = core {
            a.push(format!("--core={}", c.display()));
        }
        if let Some(p) = program {
            if args.is_empty() {
                a.push(p.display().to_string());
            } else {
                a.push("--args".into());
                a.push(p.display().to_string());
                a.extend(args.iter().cloned());
            }
        }
        let pending = Arc::new(Mutex::new(Pending::default()));
        let sink = pending.clone();
        let push = move |l: &str| {
            let mut p = sink.lock().unwrap_or_else(PoisonError::into_inner);
            if p.lines.len() >= MAX_PENDING {
                p.lines.pop_front();
                p.dropped += 1;
            }
            p.lines.push_back(l.to_string());
        };
        let push_err = push.clone();
        let mut proc = Process::new(gdb)
            .args(&a)
            .env("DEBUGINFOD_URLS", "")
            .stdin(StdinMode::Piped)
            // A debugging session can sit idle for a long time; closing it or the session end stops it.
            .timeout(Duration::from_secs(8 * 3600))
            .stdout_cap(1 << 20)
            .stderr_cap(1 << 20)
            .on_stdout_line(push)
            .on_stderr_line(move |l| push_err(&format!("&{}", quote(&format!("{l}\n")))))
            .label("gdb")
            .remediation("Install GDB and set its path in Settings → Engines.")
            .spawn()?;
        let stdin = proc.take_stdin().ok_or_else(|| ToolError::internal("GDB started without a stdin pipe"))?;
        Ok(GdbSession {
            program: program.cloned(),
            gdb: gdb.to_path_buf(),
            proc,
            stdin: Some(stdin),
            pending,
            next_token: 0,
            state: if core.is_some() { "stopped" } else { "not_started" },
            last_stop: None,
            started: Instant::now(),
        })
    }

    /// Send one MI command and wait for its result (and, after `^running`, for the program to
    /// stop). The flag is true when `timeout` passed or `cancelled` said so first.
    pub fn exec(&mut self, mi: &str, timeout: Duration, cancelled: &dyn Fn() -> bool) -> Result<(Collected, bool), ToolError> {
        self.next_token += 1;
        let token = self.next_token;
        let stdin = self.stdin.as_mut().ok_or_else(|| ToolError::new(ErrorCode::NotFound, "this GDB session has ended"))?;
        stdin.write_all(format!("{token}{mi}\n").as_bytes()).and_then(|_| stdin.flush()).map_err(|e| {
            ToolError::new(ErrorCode::NotFound, format!("could not talk to GDB: {e}")).with_remediation("Close this session and start a new one.")
        })?;
        let deadline = Instant::now() + timeout;
        let mut c = Collected::default();
        let mut got_result = false;
        let mut waiting_for_stop = false;
        loop {
            let finished = drain(self, &mut c, Some(token), &mut |line, sess| {
                if let Some((Some(t), class, _)) = result_record(line.trim_end())
                    && t == token
                {
                    got_result = true;
                    c_result(class, sess, &mut waiting_for_stop);
                }
                got_result && !waiting_for_stop || (waiting_for_stop && sess.state != "running")
            });
            if finished {
                break;
            }
            if self.proc.is_finished() {
                self.state = "exited";
                self.stdin = None;
                break;
            }
            if Instant::now() >= deadline || cancelled() {
                return Ok((c, true));
            }
            std::thread::sleep(Duration::from_millis(15));
        }
        Ok((c, false))
    }

    /// Run one console command line, as typed at GDB's prompt.
    pub fn console(&mut self, command: &str, timeout: Duration, cancelled: &dyn Fn() -> bool) -> Result<(Collected, bool), ToolError> {
        self.exec(&format!("-interpreter-exec console {}", quote(command)), timeout, cancelled)
    }

    /// Everything that arrived since the last command, and how many lines were dropped so far.
    pub fn take_output(&mut self) -> (Collected, u64) {
        let mut c = Collected::default();
        drain(self, &mut c, None, &mut |_, _| false);
        let dropped = self.pending.lock().unwrap_or_else(PoisonError::into_inner).dropped;
        (c, dropped)
    }

    pub fn is_finished(&mut self) -> bool {
        self.proc.is_finished()
    }

    /// Stop GDB and anything it started.
    pub fn kill(&self) {
        self.proc.kill();
    }

    /// Whether GDB is alive, the program's state, the last stop and the uptime.
    pub fn status(&mut self) -> Value {
        let alive = !self.proc.is_finished();
        json!({
            "gdb": self.gdb,
            "gdb_pid": self.proc.pid(),
            "alive": alive,
            "program": self.program,
            "state": if alive { self.state } else { "exited" },
            "last_stop": self.last_stop,
            "uptime_seconds": self.started.elapsed().as_secs(),
        })
    }

    /// Ask GDB to exit, then stop it and anything it started. True when it exited on its own.
    pub fn close(mut self) -> bool {
        let graceful = !self.proc.is_finished() && {
            if let Some(stdin) = self.stdin.as_mut() {
                let _ = stdin.write_all(b"-gdb-exit\n").and_then(|_| stdin.flush());
            }
            self.stdin = None;
            self.proc.wait_timeout(Duration::from_secs(3)).is_some()
        };
        self.proc.kill();
        graceful
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unquote_octal_utf8() {
        assert_eq!(unquote(r#""caf\303\251""#), "café");
        assert_eq!(unquote(r#""a\303\251\nb\342\202\254c""#), "aé\nb€c");
        assert_eq!(unquote(r#""x\377y""#), "x\u{fffd}y");
    }

    #[test]
    fn mi_parsing() {
        assert_eq!(unquote(r#""Hello \"gdb\"\n\t\101""#), "Hello \"gdb\"\n\tA");
        assert_eq!(quote(r#"print "a\b""#), r#""print \"a\\b\"""#);
        assert_eq!(result_record("12^done,value=\"1\""), Some((Some(12), "done", "value=\"1\"")));
        assert_eq!(result_record("^error,msg=\"x\""), Some((None, "error", "msg=\"x\"")));
        assert_eq!(result_record("~\"text\""), None);
        let f = fields(r#"reason="breakpoint-hit",bkptno="1",frame={addr="0x1",func="main",args=[]},thread-id="1""#);
        assert_eq!(f.get("reason"), Some(&json!("breakpoint-hit")));
        assert_eq!(f.get("frame"), Some(&json!(r#"{addr="0x1",func="main",args=[]}"#)));
        assert_eq!(f.get("thread-id"), Some(&json!("1")));
        assert!(fields("").is_empty());
        assert!(fields("garbage,,=,{{").len() <= 2);
    }

    /// Standard C escapes (`\a`, `\b`, `\f`, `\v`, `\'`, `\?`) and hex byte escapes (`\xHH`)
    /// decode to their underlying bytes while preserving UTF-8 and unknown escape pass-through.
    #[test]
    fn unquote_standard_c_and_hex_escapes() {
        assert_eq!(unquote(r#""\a\b\f\v\'\?""#), "\x07\x08\x0c\x0b'?");
        assert_eq!(unquote(r#""\x41\x1B[31m\xc3\xA9\x9g""#), "A\x1b[31mé\tg");
        // Unknown escape and `\x` with no hex digits pass the character through, keeping existing behaviour.
        assert_eq!(unquote(r#""\xz\e\101\x""#), "xz\x1bAx");
    }

    /// Quoting console commands and stderr lines escapes line breaks and control characters so
    /// each MI C-string stays on one line and round-trips through `unquote` without digit bleed.
    #[test]
    fn quote_escapes_newlines_and_control_characters_on_one_line() {
        let sample = "print \"a\\b\"\nnext\r\t\0\x077\x1b[0m\x7f\u{85}café";
        let encoded = quote(sample);
        assert_eq!(encoded, r#""print \"a\\b\"\nnext\r\t\000\0077\033[0m\177\302\205café""#);
        assert!(!encoded.chars().any(char::is_control), "raw control char in MI string: {encoded:?}");
        assert_eq!(unquote(&encoded), sample);
    }

    /// An unmatched closing brace or bracket outside quotes clamps nesting depth at zero so
    /// subsequent top-level MI fields are still parsed.
    #[test]
    fn fields_clamps_unmatched_closing_delimiters() {
        let f = fields(r#"garbage=},reason="exited-normally",bkptno="2",frame={addr="0x401000",args=[{v="a,b}c"}]}"#);
        assert_eq!(f.get("garbage"), Some(&json!("}")));
        assert_eq!(f.get("reason"), Some(&json!("exited-normally")));
        assert_eq!(f.get("bkptno"), Some(&json!("2")));
        assert_eq!(f.get("frame"), Some(&json!(r#"{addr="0x401000",args=[{v="a,b}c"}]}"#)));

        let f2 = fields(r#"bad=],msg="Undefined command",thread-id="1""#);
        assert_eq!(f2.get("bad"), Some(&json!("]")));
        assert_eq!(f2.get("msg"), Some(&json!("Undefined command")));
        assert_eq!(f2.get("thread-id"), Some(&json!("1")));
    }

    /// Guarded stand-in GDB subprocess used by `session_exec_isolates_tokens_and_multiline_quotes`.
    #[test]
    #[ignore = "subprocess fixture invoked via CRAFT_FAKE_GDB=1"]
    fn fake_gdb_child() {
        use std::io::BufRead;
        if std::env::var_os("CRAFT_FAKE_GDB").is_none() {
            return;
        }
        let stdin = std::io::stdin();
        let mut stdout = std::io::stdout();
        for line in stdin.lock().lines().map_while(Result::ok) {
            let line = line.trim_end_matches('\r');
            let digits = line.bytes().take_while(u8::is_ascii_digit).count();
            let token = &line[..digits];
            let rest = &line[digits..];
            if rest == "-gdb-exit" {
                let _ = writeln!(stdout, "{token}^exit");
                let _ = stdout.flush();
                break;
            } else if rest == "-gdb-version" {
                let _ = writeln!(stdout, "~\"GNU gdb (stand-in) 15.1\\n\"");
                let _ = writeln!(stdout, "{token}^done");
            } else if let Some(raw_arg) = rest.strip_prefix("-interpreter-exec console ") {
                if !(raw_arg.starts_with('"') && raw_arg.ends_with('"') && raw_arg.len() >= 2) {
                    let _ = writeln!(stdout, "{token}^error,msg=\"unclosed quote in MI command\"");
                } else if raw_arg.contains("stale-before-done") {
                    // Simulate a late error from an earlier timed-out token arriving right before this command's ^done.
                    let _ = writeln!(stdout, "999^error,msg=\"stale timeout error\"");
                    let _ = writeln!(stdout, "^error,msg=\"untokenized error\"");
                    let _ = writeln!(stdout, "~\"ok\\x21\\b!\\n\"");
                    let _ = writeln!(stdout, "{token}^done");
                } else if raw_arg.contains("run-with-stray") {
                    // Simulate ^running (with trailing horizontal whitespace) followed by a stray ^done from another token
                    // before *stopped arrives, plus trailing background records consumed by take_output().
                    let _ = writeln!(stdout, "{token}^running   ");
                    let _ = writeln!(stdout, "*running,thread-id=\"all\"");
                    let _ = writeln!(stdout, "888^done,value=\"stray\"");
                    let _ = writeln!(stdout, "*stopped,garbage=}},reason=\"breakpoint-hit\",bkptno=\"1\"");
                    let _ = writeln!(stdout, "777^error,msg=\"earlier background error\"");
                    let _ = writeln!(stdout, "778^exit");
                } else {
                    let _ = writeln!(stdout, "~\"echo:{}\\n\"", &raw_arg[1..raw_arg.len() - 1]);
                    let _ = writeln!(stdout, "{token}^done");
                }
            } else {
                let _ = writeln!(stdout, "{token}^error,msg=\"unexpected raw line\"");
            }
            let _ = stdout.flush();
        }
    }

    /// Single-quote a string for POSIX `/bin/sh` so embedded apostrophes never break the quoted argument.
    fn sh_single_quote(s: &str) -> String {
        format!("'{}'", s.replace('\'', "'\\''"))
    }

    /// Write a tiny platform launcher that invokes `fake_gdb_child` in `exe`.
    fn write_stand_in_gdb_for(dir: &Path, exe: &Path) -> PathBuf {
        #[cfg(windows)]
        {
            let script = dir.join("fake-gdb.cmd");
            let body =
                format!("@echo off\r\nset CRAFT_FAKE_GDB=1\r\n\"{}\" --quiet --exact gdb::tests::fake_gdb_child --nocapture --ignored\r\n", exe.display());
            std::fs::write(&script, body).unwrap();
            script
        }
        #[cfg(not(windows))]
        {
            use std::os::unix::fs::PermissionsExt;
            let script = dir.join("fake-gdb");
            let quoted_exe = sh_single_quote(&exe.to_string_lossy());
            let body = format!("#!/bin/sh\nCRAFT_FAKE_GDB=1 exec {quoted_exe} --quiet --exact gdb::tests::fake_gdb_child --nocapture --ignored\n");
            std::fs::write(&script, body).unwrap();
            std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755)).unwrap();
            script
        }
    }

    /// Write a tiny platform launcher that invokes `fake_gdb_child` in this test binary.
    fn write_stand_in_gdb(dir: &Path) -> PathBuf {
        let exe = std::env::current_exe().unwrap();
        write_stand_in_gdb_for(dir, &exe)
    }

    /// Shell-quoting the fixture executable path escapes apostrophes for `/bin/sh` while preserving plain paths.
    #[test]
    fn stand_in_gdb_shell_quoting_escapes_apostrophes() {
        assert_eq!(sh_single_quote("/tmp/plain/craft-anything"), "'/tmp/plain/craft-anything'");
        assert_eq!(sh_single_quote("/tmp/dev's/a'b/craft-anything"), "'/tmp/dev'\\''s/a'\\''b/craft-anything'");
        #[cfg(not(windows))]
        {
            let dir = tempfile::tempdir().unwrap();
            let linked_dir = dir.path().join("dev's");
            std::fs::create_dir_all(&linked_dir).unwrap();
            let linked_exe = linked_dir.join("craft-gdb-child");
            std::os::unix::fs::symlink(std::env::current_exe().unwrap(), &linked_exe).unwrap();
            let gdb = write_stand_in_gdb_for(dir.path(), &linked_exe);
            let script = std::fs::read_to_string(&gdb).unwrap();
            let expected_quoted = sh_single_quote(&linked_exe.to_string_lossy());
            assert!(script.contains(&format!("exec {expected_quoted} ")), "expected escaped apostrophe in launcher: {script:?}");
            let mut sess = GdbSession::start(&gdb, None, None, &[]).unwrap();
            let (ver, timed_out) = sess.exec("-gdb-version", Duration::from_secs(10), &|| false).unwrap();
            assert!(!timed_out);
            assert_eq!(ver.result.as_deref(), Some("done"));
            assert!(sess.close());
        }
    }

    /// End-to-end `GdbSession` check: multi-line console quoting stays on one MI line, stale or
    /// untokenized result records do not taint `result`/`error`, and malformed stop fields still
    /// update session state.
    #[test]
    fn session_exec_isolates_tokens_and_multiline_quotes() {
        let dir = tempfile::tempdir().unwrap();
        let gdb = write_stand_in_gdb(dir.path());
        let mut sess = GdbSession::start(&gdb, None, None, &[]).unwrap();
        let (_, timed_out) = sess.exec("-gdb-version", Duration::from_secs(10), &|| false).unwrap();
        assert!(!timed_out);

        // 1. Multi-line console input must not split the MI command across lines.
        let (multi, timed_out) = sess.console("line1\n-gdb-exit", Duration::from_secs(10), &|| false).unwrap();
        assert!(!timed_out);
        assert_eq!(multi.result.as_deref(), Some("done"), "multiline command split MI line: {multi:?}");
        assert_eq!(multi.error, None);
        assert!(multi.console.contains("line1\n-gdb-exit"), "{:?}", multi.console);

        // 2. Late error records for other tokens must not leave `error` populated on a `done` command.
        let (clean, timed_out) = sess.console("stale-before-done", Duration::from_secs(10), &|| false).unwrap();
        assert!(!timed_out);
        assert_eq!(clean.result.as_deref(), Some("done"));
        assert_eq!(clean.error, None, "stale token error leaked into current command: {clean:?}");
        assert_eq!(clean.console, "ok!\x08!\n");

        // 3. A stray result during `^running` must not clobber `result`, and `}` in a prior field must not hide `reason`.
        let (ran, timed_out) = sess.console("run-with-stray", Duration::from_secs(10), &|| false).unwrap();
        assert!(!timed_out);
        assert_eq!(ran.result.as_deref(), Some("running"));
        assert_eq!(ran.error, None);
        assert_eq!(sess.state, "stopped");
        assert_eq!(sess.last_stop.as_ref().and_then(|v| v.get("bkptno")), Some(&json!("1")));

        // 4. `take_output` (no expected token) clears an earlier `^error` once a non-error record arrives and marks `^exit`.
        let wait_until = Instant::now() + Duration::from_secs(5);
        while sess.pending.lock().unwrap_or_else(PoisonError::into_inner).lines.len() < 2 {
            assert!(Instant::now() < wait_until, "timed out waiting for trailing background MI lines");
            std::thread::sleep(Duration::from_millis(5));
        }
        let (after, _) = sess.take_output();
        assert_eq!(after.result.as_deref(), Some("exit"));
        assert_eq!(after.error, None);
        assert_eq!(sess.state, "exited");

        // 5. Raw CRLF lines queued in `pending` strip `\r\n` without losing meaningful trailing spaces in target output.
        {
            let mut p = sess.pending.lock().unwrap_or_else(PoisonError::into_inner);
            p.lines.push_back("plain target line  \r\n".to_string());
            p.lines.push_back("42^done\r\n".to_string());
        }
        let mut crlf_collected = Collected::default();
        let matched = drain(&mut sess, &mut crlf_collected, Some(42), &mut |line, _| matches!(result_record(line.trim_end()), Some((Some(42), "done", _))));
        assert!(matched);
        assert_eq!(crlf_collected.target_output, "plain target line  \n");
        assert_eq!(crlf_collected.result.as_deref(), Some("done"));

        assert!(sess.close());
    }
}
