//! Console programs can't be photographed, so they are compared by their runs: with runtime study
//! on, Study runs the original with each input in [`CASES`] (`runtime_walkthrough_record`) and keeps
//! what it printed and how it exited in `capture/runs/`; [`RunsCheck`] runs the rebuild the same
//! way and claims, input by input, that it exits like the original and prints what it prints.

use super::*;
use craft_anything_engine::workspace::Claim;

/// Where the original's runs are kept in the capture folder.
pub const RUNS_DIR: &str = "runs";

/// The inputs both programs are run with: (name, arguments). Add an input with one line.
pub const CASES: &[(&str, &[&str])] = &[("no arguments", &[]), ("--help", &["--help"]), ("--version", &["--version"])];

/// How long one run may take; a program still running then (waiting for input) is a result too.
const RUN_MS: u64 = 8_000;
/// How much of the output's words must match for "prints what the original prints".
const OUTPUT_ALIKE: f64 = 80.0;

/// Libraries a windowed program loads, matched lower-case as part of the name. ELF and Mach-O
/// don't mark a console program, so one that loads none of these is taken for one (and Study
/// checks it really ran like one: [`ran_like_console`]).
const WINDOW_LIBRARIES: &[&str] = &[
    "libx11",
    "libxcb",
    "libwayland",
    "libgtk",
    "libgdk",
    "libqt",
    "libsdl",
    "libglfw",
    "libgl.",
    "libegl",
    "libvulkan",
    "appkit.framework",
    "cocoa.framework",
    "uikit.framework",
    "swiftui.framework",
    "metal.framework",
    "quartzcore.framework",
];

/// The most of a Linux or macOS program read to see what it loads.
const MAX_READ: u64 = 256 << 20;

/// A console program: a Windows program marked for the console, or a Linux or macOS executable
/// that loads no windowing library.
pub fn is_console(program: &Path) -> bool {
    use std::io::Read as _;
    let read = |limit: u64| -> Option<Vec<u8>> {
        let mut bytes = Vec::new();
        std::fs::File::open(program).and_then(|f| f.take(limit).read_to_end(&mut bytes)).ok()?;
        Some(bytes)
    };
    let Some(head) = read(64 * 1024) else { return false };
    if head.starts_with(b"MZ") {
        return craft_anything_formats::pe::headers(&head).is_ok_and(|h| h.subsystem == 3);
    }
    let Some(b) = read(MAX_READ).and_then(|bytes| craft_anything_formats::parse_binary(&bytes).ok()) else { return false };
    loads_no_window(&b)
}

/// An ELF or Mach-O executable that loads none of [`WINDOW_LIBRARIES`].
fn loads_no_window(b: &craft_anything_formats::Binary) -> bool {
    use craft_anything_formats::Format;
    matches!(b.format, Format::Elf | Format::MachO)
        && b.entry.is_some()
        && !b.libraries.iter().any(|l| {
            let l = l.to_lowercase();
            WINDOW_LIBRARIES.iter().any(|w| l.contains(w))
        })
}

/// Whether the kept runs look like a console program's: one of them printed something or ended
/// by itself. A program that sat silent through every input is a windowed one after all (one
/// that opens its windowing library only once it runs).
pub fn ran_like_console(runs: &[Value]) -> bool {
    runs.iter().any(|r| r.get("reason").and_then(Value::as_str) == Some("exited") || r.get("lines").and_then(Value::as_array).is_some_and(|l| !l.is_empty()))
}

/// The original's runs Study kept, by input name (none when it isn't a console program or
/// runtime study was off).
pub fn captured(ws: &Workspace) -> Vec<Value> {
    let dir = ws.capture_dir().join(RUNS_DIR);
    CASES.iter().filter_map(|(name, _)| std::fs::read(dir.join(format!("{}.json", slug(name)))).ok()).filter_map(|b| serde_json::from_slice(&b).ok()).collect()
}

/// The original program's name, which its printed text may contain. Clean room takes it from the
/// picked path; only Deep mode looks inside (a launcher's script, an app bundle's Info.plist).
pub(crate) fn original_name(target: &TargetRef, deep: bool) -> String {
    let TargetRef::Local { path } = target else { return String::new() };
    let program = if deep { locate(path, false).0 } else { path.clone() };
    program.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default()
}

fn slug(name: &str) -> String {
    let s: String = name.chars().map(|c| if c.is_ascii_alphanumeric() { c.to_ascii_lowercase() } else { '-' }).collect();
    s.split('-').filter(|p| !p.is_empty()).collect::<Vec<_>>().join("-")
}

/// One run as kept: its input, how it exited and the lines it left on the terminal.
fn run_record(name: &str, args: &[String], capture: &Value) -> Value {
    json!({
        "name": name,
        "arguments": args,
        "reason": capture.pointer("/exit/reason"),
        "exit_code": capture.pointer("/exit/code"),
        "lines": capture.pointer("/terminal/lines").cloned().unwrap_or_else(|| json!([])),
    })
}

/// The kept form of a `runtime_walkthrough_record` result.
fn result_of(out: &ToolOutput) -> Value {
    out.to_json().get("result").cloned().unwrap_or(Value::Null)
}

impl Worker {
    /// Run the original console program twice with each input and keep both observations in capture/runs/;
    /// returns the README section, or None (and keeps nothing) when it didn't run like a console
    /// program.
    pub(in crate::rebuild) fn capture_runs(&self, program: &Path, capture: &Path) -> Result<Option<String>, ToolError> {
        let dir = capture.join(RUNS_DIR);
        std::fs::create_dir_all(&dir).map_err(|e| ToolError::io(dir.display(), e))?;
        let mut kept = Vec::new();
        let mut records = Vec::new();
        for (name, args) in CASES {
            let args: Vec<String> = args.iter().map(|a| (*a).to_string()).collect();
            let mut params =
                json!({"executable": program.display().to_string(), "arguments": args, "timeout_ms": RUN_MS, "idle_timeout_ms": RUN_MS, "settle_ms": 200});
            if let Some(cwd) = program.parent() {
                params["working_directory"] = json!(cwd.display().to_string());
            }
            match self.call("runtime_walkthrough_record", params.clone()) {
                Ok(out) => {
                    let mut record = run_record(name, &args, &result_of(&out));
                    match self.call("runtime_walkthrough_record", params) {
                        Ok(repeat) => record["repeat"] = run_record(name, &args, &result_of(&repeat)),
                        Err(e) => log::warn!("repeating {} with {name} failed: {}", program.display(), e.message),
                    }
                    let bytes = serde_json::to_vec_pretty(&record).map_err(|e| ToolError::internal(e.to_string()))?;
                    craft_anything_engine::project::write_atomic(&dir.join(format!("{}.json", slug(name))), &bytes)?;
                    kept.push(*name);
                    records.push(record);
                }
                Err(e) => log::warn!("running {} with {name} failed: {}", program.display(), e.message),
            }
        }
        if !kept.is_empty() && !ran_like_console(&records) {
            let _ = std::fs::remove_dir_all(&dir);
            return Ok(None);
        }
        Ok(Some(if kept.is_empty() {
            "\n## How it runs\n\nIt's a console program, but running it to see its output didn't work.\n".to_string()
        } else {
            format!(
                "\n## How it runs\n\nIt's a console program. capture/runs/ holds what it printed and how it exited for each of these inputs: {}. Each file has the arguments, the exit code (or that it was still running after {} s) and the lines it left on the terminal. A second observation, when available, identifies numbers that changed between the original's runs.\n",
                kept.join(", "),
                RUN_MS / 1000
            )
        }))
    }
}

/// Match numeric spelling without rounding large integers through a float.
fn number_pattern() -> Option<&'static regex::Regex> {
    static NUMBERS: std::sync::LazyLock<Option<regex::Regex>> =
        std::sync::LazyLock::new(|| regex::Regex::new(r"(?i)[+-]?(?:0x[0-9a-f]+|0b[01]+|0o[0-7]+|(?:[0-9]+(?:\.[0-9]*)?|\.[0-9]+)(?:e[+-]?[0-9]+)?)").ok());
    NUMBERS.as_ref()
}

/// Separate ordered numbers from the words so variable timing or PIDs do not lower word likeness.
fn numeric_parts(text: &str) -> Option<(Vec<&str>, String)> {
    let pattern = number_pattern()?;
    Some((pattern.find_iter(text).map(|m| m.as_str()).collect(), pattern.replace_all(text, " ").into_owned()))
}

/// Both programs' own names become "program", so a usage line naming the rebuild still matches.
/// Only whole names are replaced: a program named `e2` must not erase the exponent in `1e2`.
fn output_text(run: &Value, names: &[String]) -> String {
    let mut text = run.get("lines").and_then(Value::as_array).into_iter().flatten().filter_map(Value::as_str).collect::<Vec<_>>().join("\n").to_lowercase();
    let Some(pattern) = number_pattern() else { return text };
    for n in names.iter().filter(|n| !n.is_empty()) {
        let n = n.to_lowercase();
        // A bare numeric filename cannot be distinguished from a printed result.
        if pattern.find(&n).is_some_and(|number| number.as_str() == n) {
            continue;
        }
        let part_of_name = |c: char| c.is_alphanumeric() || c == '_';
        let mut normalized = String::with_capacity(text.len());
        let mut copied = 0;
        let numeric_spans: Vec<_> = pattern.find_iter(&text).map(|m| m.range()).collect();
        for (start, name) in text.match_indices(&n) {
            let end = start + name.len();
            let numeric_span = numeric_spans.get(numeric_spans.partition_point(|range| range.end <= start));
            if numeric_span.is_some_and(|range| range.start < start) {
                continue;
            }
            if text[..start].chars().next_back().is_some_and(part_of_name) || text[end..].chars().next().is_some_and(part_of_name) {
                continue;
            }
            normalized.push_str(&text[copied..start]);
            normalized.push_str("program");
            copied = end;
        }
        normalized.push_str(&text[copied..]);
        text = normalized;
    }
    text
}

/// Numbers read back in a note: a few of them, or how many there were.
fn listed(numbers: &[&str]) -> String {
    match numbers {
        [] => "nothing".to_string(),
        [one] => (*one).to_string(),
        [a, b] => format!("{a} and {b}"),
        [a, b, more @ ..] => format!("{a}, {b} and {} more", more.len()),
    }
}

/// Dice over word counts, 0..=100; two empty outputs are alike.
fn alike(a: &std::collections::BTreeMap<String, usize>, b: &std::collections::BTreeMap<String, usize>) -> f64 {
    let (ta, tb): (usize, usize) = (a.values().sum(), b.values().sum());
    if ta + tb == 0 {
        return 100.0;
    }
    let shared: usize = a.iter().map(|(w, n)| (*n).min(b.get(w).copied().unwrap_or(0))).sum();
    200.0 * shared as f64 / (ta + tb) as f64
}

/// What one input's two runs claim: the same way out (exit code, or still running) and the same
/// output. `names`: both programs' file names without extension.
pub fn run_claims(original: &Value, built: &Value, names: &[String]) -> Vec<Claim> {
    run_comparison(original, built, names).0
}

/// Keep the comparison scope for the check note even when successful claims omit their details.
fn run_comparison(original: &Value, built: &Value, names: &[String]) -> (Vec<Claim>, String) {
    let name = original.get("name").and_then(Value::as_str).unwrap_or("its input");
    let how = |r: &Value| match (r.get("reason").and_then(Value::as_str), r.get("exit_code").and_then(Value::as_i64)) {
        (Some("exited"), Some(c)) => format!("exits with code {c}"),
        (Some("exited"), None) => "exits".to_string(),
        (Some(other), _) => format!("is still running ({other})"),
        (None, _) => "didn't run".to_string(),
    };
    let (original_text, built_text) = (output_text(original, names), output_text(built, names));
    let word_score = |a: &str, b: &str| alike(&website::words(a), &website::words(b));
    let legacy_score = word_score(&original_text, &built_text);
    let (score, numbers_match, detail) =
        if let Some(((original_numbers, original_words), (built_numbers, built_words))) = numeric_parts(&original_text).zip(numeric_parts(&built_text)) {
            let repeat = original.get("repeat").filter(|r| r.get("lines").is_some_and(Value::is_array) && how(r) == how(original));
            if original_numbers.is_empty() && built_numbers.is_empty() {
                (legacy_score, true, format!("{legacy_score:.0}% of the words match"))
            } else if let Some(repeat) = repeat {
                let repeat_text = output_text(repeat, names);
                if let Some((repeat_numbers, _)) = numeric_parts(&repeat_text) {
                    let score = word_score(&original_words, &built_words);
                    if original_numbers.len() == repeat_numbers.len() {
                        let varying = original_numbers.iter().zip(&repeat_numbers).filter(|(a, b)| a != b).count();
                        let matches = original_numbers.len() == built_numbers.len()
                            && original_numbers.iter().zip(&repeat_numbers).zip(&built_numbers).all(|((a, repeat), built)| a != repeat || a == built);
                        let detail = format!(
                            "{score:.0}% of the words match; stable numeric output {}; {varying} varying numeric positions ignored",
                            if matches { "matches in order" } else { "differs" }
                        );
                        (score, matches, detail)
                    } else {
                        (score, true, format!("{score:.0}% of the words match; the original's number count varies, so numeric comparison was skipped"))
                    }
                } else {
                    (0.0, false, "numeric output could not be compared".to_string())
                }
            } else if website::words(&original_words).is_empty() && website::words(&built_words).is_empty() {
                // Nothing but numbers, beside punctuation: word likeness has nothing to weigh (it
                // keeps only words of two characters or more), so two different numbers would read
                // as identical output. Without a repeat to say which numbers vary, compare the
                // numbers themselves.
                let matches = original_numbers == built_numbers;
                let detail = if matches {
                    format!("only numbers were printed, and they match in order ({})", listed(&original_numbers))
                } else if let Some(at) = original_numbers.iter().zip(&built_numbers).position(|(a, b)| a != b) {
                    // Name the first difference, not the whole lists: a long output would only say
                    // "and n more" on both sides.
                    format!(
                        "only numbers were printed: number {} is {} where the original prints {}",
                        at + 1,
                        built_numbers.get(at).copied().unwrap_or("missing"),
                        original_numbers.get(at).copied().unwrap_or_default()
                    )
                } else {
                    format!("only numbers were printed: it prints {} numbers where the original prints {}", built_numbers.len(), original_numbers.len())
                };
                (legacy_score, matches, detail)
            } else {
                (legacy_score, true, format!("{legacy_score:.0}% of the words match; numeric stability was not observed, so only words were compared"))
            }
        } else {
            (0.0, false, "numeric output could not be compared".to_string())
        };
    (
        vec![
            Claim::new(format!("With {name}, it {} like the original", how(original)), how(original) == how(built), format!("it {}", how(built))),
            Claim::new(format!("With {name}, it prints what the original prints"), score >= OUTPUT_ALIKE && numbers_match, detail.clone()),
        ],
        detail,
    )
}

/// Runs the rebuild with each input Study ran the original with and compares, claim by claim.
pub struct RunsCheck;

impl RunsCheck {
    pub const ID: &'static str = "app-runs";
}

impl Check for RunsCheck {
    fn id(&self) -> &'static str {
        Self::ID
    }

    fn applies(&self, cx: &CheckCx) -> bool {
        !captured(cx.ws).is_empty()
    }

    /// Run the rebuilt program with each input the original was run with and compare the results.
    fn run(&self, cx: &mut CheckCx) -> Result<CheckResult, ToolError> {
        let (ws, round) = (cx.ws, cx.round);
        let godot = cx.godot();
        let Some((program, base)) = craft_anything_engine::outputs::launch_now(&ws.app_dir(), godot.as_deref()) else {
            return Ok(CheckResult::new(self.id(), round, false, screenshot::NO_PROGRAM));
        };
        let program = craft_anything_engine::outputs::absolute_program(&program);
        let working_directory = craft_anything_engine::workspace::absolute(&ws.app_dir());
        let out = ws.checks_dir().join(format!("round-{round}")).join(RUNS_DIR);
        std::fs::create_dir_all(&out).map_err(|e| ToolError::io(out.display(), e))?;
        let names = vec![original_name(&ws.manifest.target, cx.deep()), program.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default()];
        let (mut claims, mut notes, mut artifacts) = (Vec::new(), Vec::new(), Vec::new());
        for original in captured(ws) {
            let name = original.get("name").and_then(Value::as_str).unwrap_or_default().to_string();
            let mut args = base.clone();
            args.extend(original.get("arguments").and_then(Value::as_array).into_iter().flatten().filter_map(Value::as_str).map(String::from));
            let params = json!({"executable": program.display().to_string(), "arguments": args, "working_directory": working_directory.display().to_string(), "timeout_ms": RUN_MS, "idle_timeout_ms": RUN_MS, "settle_ms": 200});
            let built = match cx.call("runtime_walkthrough_record", params) {
                Ok(o) => run_record(&name, &args, &result_of(&o)),
                Err(e) => {
                    notes.push(format!("with {name} the build couldn't be run: {}", e.message));
                    json!({"name": name})
                }
            };
            let file = out.join(format!("{}.json", slug(&name)));
            if let Ok(bytes) = serde_json::to_vec_pretty(&built) {
                craft_anything_engine::project::write_atomic(&file, &bytes)?;
                artifacts.extend(file.strip_prefix(&ws.root).ok().map(Path::to_path_buf));
            }
            let (compared, detail) = run_comparison(&original, &built, &names);
            notes.push(format!("With {name}: {detail}."));
            claims.extend(compared);
        }
        let failed: Vec<String> = claims.iter().filter(|c| !c.passed).map(|c| format!("{} ({})", c.title, c.detail)).collect();
        notes.insert(
            0,
            if failed.is_empty() {
                format!("It runs like the original with every input ({} claims hold).", claims.len())
            } else {
                format!(
                    "{} of {} claims don't hold yet: {}. Compare your runs in checks/round-{round}/runs/ with the original's in capture/runs/.",
                    failed.len(),
                    claims.len(),
                    failed.join("; ")
                )
            },
        );
        Ok(CheckResult { claims, artifacts, ..CheckResult::new(self.id(), round, true, notes.join(" ")) })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Clean room names the original by the path that was picked; only Deep mode reads the app
    /// bundle's Info.plist for the program inside it.
    #[test]
    fn clean_room_names_the_original_without_looking_inside() {
        let d = tempfile::tempdir().unwrap();
        let app = d.path().join("Tip Calculator.app");
        std::fs::create_dir_all(app.join("Contents/MacOS")).unwrap();
        std::fs::write(
            app.join("Contents/Info.plist"),
            r#"<?xml version="1.0"?><plist version="1.0"><dict><key>CFBundleExecutable</key><string>TipCalc</string></dict></plist>"#,
        )
        .unwrap();
        std::fs::write(app.join("Contents/MacOS/TipCalc"), b"\xcf\xfa\xed\xfe").unwrap();
        let target = TargetRef::Local { path: app };
        assert_eq!(original_name(&target, false), "Tip Calculator");
        assert_eq!(original_name(&target, true), "TipCalc");
        assert_eq!(original_name(&TargetRef::None, true), "");
    }

    /// A run check from a relative project must give its child an absolute executable and working
    /// folder; a relative working folder is refused before the program starts.
    #[cfg(unix)]
    #[test]
    fn a_relative_project_runs_a_console_check() {
        use std::os::unix::fs::PermissionsExt;

        let cwd = std::env::current_dir().unwrap();
        let d = tempfile::tempdir_in(&cwd).unwrap();
        let relative = d.path().strip_prefix(&cwd).unwrap();
        let ws = Workspace::create(relative, "Tool", "a console tool", TargetRef::None).unwrap();
        let app = ws.app_dir();
        std::fs::create_dir_all(app.join("target/debug")).unwrap();
        std::fs::write(app.join("Cargo.toml"), "[package]\nname = \"tool\"\n").unwrap();
        let program = app.join("target/debug/tool");
        std::fs::write(&program, "#!/bin/sh\nprintf 'hello\\n'\n").unwrap();
        std::fs::set_permissions(&program, std::fs::Permissions::from_mode(0o755)).unwrap();
        let capture = ws.capture_dir().join(RUNS_DIR);
        std::fs::create_dir_all(&capture).unwrap();
        std::fs::write(capture.join("no-arguments.json"), r#"{"name":"no arguments","arguments":[],"reason":"exited","exit_code":0,"lines":["hello"]}"#)
            .unwrap();

        let worker = crate::rebuild::tests::check_worker(&ws, false);
        let mut cx = CheckCx { worker: &worker, ws: &ws, round: 1, until: None };
        let result = RunsCheck.run(&mut cx).unwrap();
        assert!(!result.claims.is_empty(), "{}", result.note);
        assert!(result.claims.iter().all(|c| c.passed), "{}", result.note);
    }

    #[test]
    fn successful_run_notes_explain_numeric_comparison_limits() {
        let run = |line: &str| json!({"reason":"exited", "exit_code":0, "lines":[line]});
        let mut original = run("Result 42 completed in 123 ms");
        original["repeat"] = run("Result 42 completed in 456 ms");
        let (claims, detail) = run_comparison(&original, &run("Result 42 completed in 789 ms"), &[]);
        assert!(claims.iter().all(|c| c.passed && c.detail.is_empty()));
        assert!(detail.contains("1 varying numeric positions ignored"), "{detail}");

        original["repeat"] = run("Result 42 completed in ms");
        let (claims, detail) = run_comparison(&original, &run("Result 42 completed in 789 ms"), &[]);
        assert!(claims.iter().all(|c| c.passed));
        assert!(detail.contains("number count varies"), "{detail}");

        for repeat in [Value::Null, json!({}), json!({"reason":"timeout", "lines":["Result 42"]})] {
            original["repeat"] = repeat;
            let (claims, detail) = run_comparison(&original, &run("Result 42 completed in 789 ms"), &[]);
            assert!(claims.iter().all(|c| c.passed));
            assert!(detail.contains("numeric stability was not observed"), "{detail}");
        }
    }

    #[test]
    fn numbers_printed_alone_are_compared_without_a_repeat() {
        let run = |line: &str| json!({"reason":"exited", "exit_code":0, "lines":[line]});
        // Bare `4.5` and `9.0`: word likeness drops one-character words, so this was 100% alike.
        let (claims, detail) = run_comparison(&run("4.5"), &run("9.0"), &[]);
        assert!(!claims[1].passed, "different numbers are different output: {detail}");
        assert!(claims[1].detail.contains("9.0") && claims[1].detail.contains("4.5"), "{}", claims[1].detail);
        let (claims, detail) = run_comparison(&run("4.5"), &run("4.5"), &[]);
        assert!(claims.iter().all(|c| c.passed), "the same number still matches: {detail}");
        assert!(detail.contains("only numbers were printed"), "{detail}");
        // Printing nothing where the original printed a number isn't alike either.
        let (claims, detail) = run_comparison(&run("4.5"), &run(""), &[]);
        assert!(!claims[1].passed, "{detail}");
        assert!(detail.contains("0 numbers where the original prints 1"), "{detail}");
        // Punctuation around the numbers doesn't make them text.
        let (claims, detail) = run_comparison(&run("[4.5]"), &run(""), &[]);
        assert!(!claims[1].passed, "brackets aren't words: {detail}");
        // A long output names where it first differs, not "and n more" on both sides.
        let (claims, detail) = run_comparison(&run("1 2 3 4 5"), &run("1 2 9 4 5"), &[]);
        assert!(!claims[1].passed && detail.contains("number 3 is 9 where the original prints 3"), "{detail}");
        // Words beside the numbers still decide: a timing that differs is not a failed build.
        let (claims, detail) = run_comparison(&run("Result 42 completed in 123 ms"), &run("Result 42 completed in 987 ms"), &[]);
        assert!(claims.iter().all(|c| c.passed), "{detail}");
        assert!(detail.contains("numeric stability was not observed"), "{detail}");
    }

    #[test]
    fn each_input_claims_the_same_exit_and_output() {
        let names = vec!["orig-tool".to_string(), "rebuilt".to_string()];
        let original = json!({"name": "--help", "reason": "exited", "exit_code": 0, "lines": ["Usage: orig-tool [options] <file>", "  -v  print the version", "  -o  output folder"]});
        let same = json!({"name": "--help", "reason": "exited", "exit_code": 0, "lines": ["Usage: rebuilt [options] <file>", "  -v  print the version", "  -o  output folder"]});
        assert!(
            run_claims(&original, &same, &names).iter().all(|c| c.passed),
            "the programs' own names don't count: {:?}",
            run_claims(&original, &same, &names)
        );
        let other = json!({"name": "--help", "reason": "exited", "exit_code": 2, "lines": ["error: unknown option --help"]});
        let got: Vec<(String, bool, String)> = run_claims(&original, &other, &names).into_iter().map(|c| (c.title, c.passed, c.detail)).collect();
        assert_eq!(got[0], ("With --help, it exits with code 0 like the original".to_string(), false, "it exits with code 2".to_string()));
        assert!(!got[1].1 && got[1].0 == "With --help, it prints what the original prints" && got[1].2.ends_with("% of the words match"), "{got:?}");
        // A program that waits for input is still running at the deadline, in both.
        let waits = json!({"name": "no arguments", "reason": "timeout", "lines": []});
        assert!(run_claims(&waits, &waits, &names).iter().all(|c| c.passed));
        assert!(!run_claims(&waits, &json!({"name": "no arguments"}), &names)[0].passed, "a build that didn't run doesn't match");
    }

    #[test]
    fn a_unix_program_is_a_console_one_unless_it_loads_a_windowing_library() {
        let binary = |format, libs: &[&str]| {
            let mut b = craft_anything_formats::Binary::raw(craft_anything_formats::Arch::X86_64, 0, 0, 0);
            b.format = format;
            b.entry = Some(0x1000);
            b.libraries = libs.iter().map(|l| (*l).to_string()).collect();
            b
        };
        use craft_anything_formats::Format;
        assert!(loads_no_window(&binary(Format::Elf, &["libc.so.6", "libssl.so.3"])));
        assert!(!loads_no_window(&binary(Format::Elf, &["libc.so.6", "libX11.so.6"])));
        assert!(!loads_no_window(&binary(Format::Elf, &["libGL.so.1"])));
        assert!(loads_no_window(&binary(Format::MachO, &["/usr/lib/libSystem.B.dylib"])));
        assert!(!loads_no_window(&binary(Format::MachO, &["/System/Library/Frameworks/AppKit.framework/Versions/C/AppKit"])));
        assert!(!loads_no_window(&binary(Format::Pe, &[])), "Windows programs say so themselves");
        let mut library = binary(Format::Elf, &[]);
        library.entry = None;
        assert!(!loads_no_window(&library), "a library isn't run");
    }

    #[test]
    fn only_console_programs_are_run_and_inputs_have_file_names() {
        let d = tempfile::tempdir().unwrap();
        let fake = d.path().join("tool.exe");
        std::fs::write(&fake, b"MZ not really a program").unwrap();
        assert!(!is_console(&fake) && !is_console(&d.path().join("missing.exe")));
        assert!(ran_like_console(&[json!({"reason": "timeout", "lines": ["Usage: tool"]})]), "it printed");
        assert!(ran_like_console(&[json!({"reason": "exited", "lines": []})]), "it ended by itself");
        assert!(
            !ran_like_console(&[json!({"reason": "timeout", "lines": []}), json!({"reason": "idle_timeout", "lines": []})]),
            "silent and still open: a window"
        );
        // Programs made here, read but never run: a console tool and a windowed app.
        let (tool, app) = (d.path().join("tool.exe"), d.path().join("app.exe"));
        std::fs::write(&tool, craft_anything_testkit::pe64_subsystem(3)).unwrap();
        std::fs::write(&app, craft_anything_testkit::pe64_subsystem(2)).unwrap();
        assert!(is_console(&tool) && !is_console(&app));
        assert_eq!(CASES.iter().map(|(n, _)| slug(n)).collect::<Vec<_>>(), ["no-arguments", "help", "version"]);
        let ws = Workspace::create(d.path(), "Tool", "Rebuild this tool", TargetRef::None).unwrap();
        assert!(captured(&ws).is_empty());
        std::fs::create_dir_all(ws.capture_dir().join(RUNS_DIR)).unwrap();
        std::fs::write(
            ws.capture_dir().join(RUNS_DIR).join("help.json"),
            br#"{"name": "--help", "arguments": ["--help"], "reason": "exited", "exit_code": 0, "lines": ["Usage"]}"#,
        )
        .unwrap();
        assert_eq!(captured(&ws).len(), 1);
    }
}
