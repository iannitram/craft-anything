//! Study: look at the target before the first build (capture a website, triage a program, start
//! Ghidra's analysis in Deep mode) and write the notes the builder reads.

use super::*;

/// An Electron program is the Chromium and Node runtime, and analysing that takes hours: the app's
/// own logic is the JavaScript in `resources/app.asar` beside it, which the study maps (#947).
pub(super) fn is_electron_runtime(program: &Path) -> bool {
    program.parent().is_some_and(|dir| dir.join("resources/app.asar").is_file())
}

/// The program a later run reopens for its builder: the one the study looked at, in Deep mode on a
/// local target, when it is still there. None otherwise (Clean room never opens the original).
pub(super) fn program_to_reopen(target: &TargetRef, kind: &dyn TargetKind, deep: bool) -> Option<PathBuf> {
    // Clean room is decided first: locating the program reads the original's folder.
    if !deep {
        return None;
    }
    let TargetRef::Local { path } = target else { return None };
    let program = locate(path, kind.studies_folder()).0;
    program.exists().then_some(program)
}

/// The builder's tools run on another computer: a coding agent over SSH starts its own tool
/// server there. An API builder over SSH (a tunnelled model) still calls tools in this process.
pub(super) fn tools_run_remotely(over_ssh: bool, api_loop: bool) -> bool {
    over_ssh && !api_loop
}

impl Worker {
    /// A run after the first skips the study, so its session has nothing open and the builder's
    /// first `native_*` calls said "Nothing is open yet" (#1421). Open what the study looked at, so
    /// the builder's tool server gets it. A failure only says why: the builder can still open it.
    /// `remote`: the builder's tools run on another computer ([`tools_run_remotely`]), whose tool
    /// server isn't handed this computer's targets, so opening (and hashing) it here helps nothing.
    /// `deep`: this run's mode, the same read that decided its capture is usable.
    pub(super) fn reopen_target(&mut self, ws: &mut Workspace, kind: &dyn TargetKind, remote: bool, deep: bool) -> Result<(), ToolError> {
        if remote {
            return Ok(());
        }
        // Nothing to look for when this project's program is open already (another project's
        // doesn't count: the builder is handed only its own).
        let already_open = match &ws.manifest.target {
            TargetRef::Local { path } => !self.engine.session().shared_targets_within(path).is_empty(),
            _ => false,
        };
        if already_open {
            return Ok(());
        }
        let Some(program) = program_to_reopen(&ws.manifest.target, kind, deep) else { return Ok(()) };
        if let Err(e) = self.call("core_target_open", json!({"path": program.display().to_string()})) {
            if e.code == ErrorCode::Cancelled {
                return Err(e);
            }
            self.step(ws, Step::Build, Status::Building, &format!("Couldn't open {} again for the builder: {}", program.display(), e.message))?;
        }
        Ok(())
    }

    /// Look at the target before the first build. `deep` is this run's mode, fixed at its start:
    /// the notes and the capture's recorded mode both come from it.
    pub(super) fn study(&mut self, ws: &mut Workspace, kind: &dyn TargetKind, deep: bool) -> Result<(), ToolError> {
        // The capture about to be written belongs to this run's mode. Clear the old record first:
        // a study that fails partway must not leave a capture a later run trusts.
        ws.manifest.capture_deep = None;
        ws.save()?;
        if !deep {
            remove_deep_only(&ws.capture_dir())?;
        }
        match ws.manifest.target.clone() {
            TargetRef::Website { url } => {
                self.step(ws, Step::Study, Status::Inspecting, &format!("Opening {url} in a private browser…"))?;
                self.call("web_site_open", json!({"url": url}))?;
                self.step(ws, Step::Study, Status::Inspecting, "Photographing the page and reading its layout…")?;
                let capture = ws.capture_dir();
                if capture.exists() {
                    let _ = std::fs::remove_dir_all(&capture);
                }
                // The widths the check compares at, so every picture has its pair.
                let widths = self.engine.session().settings.build.widths();
                self.call("web_site_capture", json!({"output_directory": capture.display().to_string(), "widths": widths, "noise_pass": true}))?;
                // Menus, pop-ups and tabs, one at a time, on a computer and on a phone (where a menu
                // button often replaces the navigation); a page without them (or a failure) is fine.
                self.step(ws, Step::Study, Status::Inspecting, "Opening its menus, pop-ups and tabs…")?;
                let mut views = vec![(kinds::website::STATES_DIR.to_string(), widths.first().copied().unwrap_or(1440))];
                views.extend(kinds::website::phone_width(&widths).map(|w| (kinds::website::phone_states_dir(w), w)));
                for (dir, width) in views {
                    let states = capture.join(dir);
                    if let Err(e) = self.call("web_page_states_capture", json!({"output_directory": states.display().to_string(), "width": width})) {
                        self.step(ws, Step::Study, Status::Inspecting, &format!("Couldn't open its menus and pop-ups at {width} px: {}", e.message))?;
                    }
                }
                self.capture_routes(ws, &url, &widths)?;
                if deep {
                    self.how_it_works(ws, &url)?;
                }
            }
            TargetRef::Local { path } if !deep => self.study_from_outside(ws, &path)?,
            TargetRef::Local { path } => {
                self.step(ws, Step::Study, Status::Inspecting, "Looking inside it (nothing is run)…")?;
                let (program, folder) = locate(&path, kind.studies_folder());
                let mut notes = format!("# What we found in {}\n\n", path.display());
                if program != path {
                    notes.push_str(&format!("{} starts {}, so that's what we looked at.\n\n", path.display(), program.display()));
                }
                match self.call("core_target_triage", json!({"path": program.display().to_string()})) {
                    Ok(out) => notes.push_str(&describe(&out)),
                    Err(e) => notes.push_str(&format!("A first look failed: {}\n", e.message)),
                }
                if !program.is_file() && deep {
                    self.step(
                        ws,
                        Step::Study,
                        Status::Inspecting,
                        "Couldn't find the program to decompile, so Ghidra can't be used. Pick the game's or app's main folder or its program.",
                    )?;
                    notes.push_str(&format!("\n## Ghidra\n\nNo program was found in {}, so Ghidra couldn't be used.\n", path.display()));
                }
                if let Some(done) = self.prepare_ghidra(&program) {
                    // Deep mode always waits for Ghidra: the builder then decompiles with it straight away.
                    let started = std::time::Instant::now();
                    // One line: the app's progress card shows how long it has been going.
                    self.step(ws, Step::Study, Status::Inspecting, "Ghidra is analysing the program (only the first time; a big game takes a while)…")?;
                    let result = loop {
                        match done.recv_timeout(Duration::from_secs(30)) {
                            Ok(r) => break r,
                            Err(std::sync::mpsc::RecvTimeoutError::Timeout) => self.stop.check()?,
                            Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => break Err("the analysis stopped unexpectedly".into()),
                        }
                    };
                    notes.push_str(&match result {
                        Ok(()) => format!(
                            "\n## Ghidra\n\nDeep mode: Ghidra has analysed {} ({} min), so decompiling it is quick.\n",
                            program.display(),
                            started.elapsed().as_secs() / 60
                        ),
                        Err(e) => format!("\n## Ghidra\n\nGhidra couldn't analyse {} ({e}), so decompiling uses the built-in decompiler.\n", program.display()),
                    });
                }
                if let Some(f) = folder {
                    self.step(ws, Step::Study, Status::Inspecting, "Listing its maps, pictures, models and sounds…")?;
                    if let Some(found) = game::detect(&f) {
                        notes.push_str(&format!("\n## Made with {}\n\nThe giveaway: {}.", found.label(), found.clue));
                        if let Some(n) = &found.note {
                            notes.push_str(&format!(" {n}"));
                        }
                        notes.push('\n');
                    }
                    notes.push_str(&inventory(&f));
                }
                // A program's app.asar or package.json sits beside it, so look in its folder too.
                let js_root = if program.is_file() { program.parent().filter(|p| !p.as_os_str().is_empty()).map(Path::to_path_buf) } else { None };
                let packaged = |root: &Path| root.join("resources/app.asar").exists() || root.join("package.json").exists();
                let js_input =
                    if path.extension().is_some_and(|e| e == "asar") || packaged(&path) { Some(path.clone()) } else { js_root.filter(|r| packaged(r)) };
                if let Some(input) = js_input
                    && let Ok(out) = self.call("js_app_map", json!({"input_path": input.display().to_string(), "limit": 50}))
                {
                    notes.push_str("\n## JavaScript application\n\n");
                    notes.push_str(&describe(&out));
                }
                let capture = ws.capture_dir();
                std::fs::create_dir_all(&capture).map_err(|e| ToolError::io(capture.display(), e))?;
                let runtime = self.engine.session().settings.runtime_study;
                let mut console = false;
                if runtime && program.is_file() && kinds::runs::is_console(&program) {
                    // A console program draws no window: run it with each input instead.
                    self.step(ws, Step::Study, Status::Inspecting, "Running it with a few inputs to see what it prints…")?;
                    if let Some(section) = self.capture_runs(&program, &capture)? {
                        notes.push_str(&section);
                        console = true;
                    }
                }
                if !console && runtime && program.is_file() && is_program(&program) {
                    notes.push_str(&self.picture_window(ws, &program, &capture)?);
                }
                craft_anything_engine::project::write_atomic(&capture.join("README.md"), notes.as_bytes())?;
            }
            TargetRef::None => {
                let capture = ws.capture_dir();
                std::fs::create_dir_all(&capture).map_err(|e| ToolError::io(capture.display(), e))?;
                craft_anything_engine::project::write_atomic(&capture.join("README.md"), b"# No target\n\nBuild from the request alone.\n")?;
            }
        }
        ws.manifest.capture_deep = Some(deep);
        ws.save()?;
        Ok(())
    }

    /// Deep mode: how the site works, not only how it looks, into capture/how-it-works.md: what its
    /// JavaScript is made of (with readable sources where its maps allow) and ten seconds of what the
    /// start page does (requests, console, WebSocket messages). A part that fails is said, not fatal.
    fn how_it_works(&mut self, ws: &mut Workspace, url: &str) -> Result<(), ToolError> {
        self.step(ws, Step::Study, Status::Inspecting, "Reading its JavaScript and watching what it does…")?;
        // The routes left the browser on another page; these look at the start page.
        let _ = self.call("web_site_open", json!({"url": url}));
        let mut notes = String::from("# How it works\n\nDeep mode: what the site's JavaScript is made of and what its start page did while we watched.\n");
        let looks = [
            ("Its JavaScript", "web_page_js_analyze", json!({"load_source_maps": true})),
            ("Ten seconds of it", "web_page_watch", json!({"watch_ms": 10_000, "with_websocket_outlines": true})),
        ];
        for (title, tool, params) in looks {
            notes.push_str(&format!("\n## {title}\n\n"));
            notes.push_str(&match self.call(tool, params) {
                Ok(out) => describe(&out),
                Err(e) => format!("{tool} didn't work: {}\n", e.message),
            });
        }
        craft_anything_engine::project::write_atomic(&ws.capture_dir().join(HOW_IT_WORKS), notes.as_bytes())
    }

    /// Clean room: nothing inside the original is read (no files, folder or code). When Settings
    /// allows running it and the person picked the program itself, it is started once and its
    /// window pictured, as anyone could see it.
    fn study_from_outside(&mut self, ws: &mut Workspace, path: &Path) -> Result<(), ToolError> {
        self.step(ws, Step::Study, Status::Inspecting, "Clean room: nothing inside it is read…")?;
        let capture = ws.capture_dir();
        std::fs::create_dir_all(&capture).map_err(|e| ToolError::io(capture.display(), e))?;
        let mut notes = format!(
            "# Clean room\n\nNothing inside {} was read: its files, its folder and its code stay closed, and the tools that look inside are off. Build from the person's request and description and the pictures in reference/, and ask them with core_person_ask when something isn't clear.\n",
            path.display()
        );
        let runtime = self.engine.session().settings.runtime_study;
        if runtime && path.is_file() && is_program(path) {
            notes.push_str(&self.picture_window(ws, path, &capture)?);
        }
        craft_anything_engine::project::write_atomic(&capture.join("README.md"), notes.as_bytes())
    }

    /// Runtime study (Settings): start the program once and picture its window into capture/, for
    /// the notes.
    fn picture_window(&mut self, ws: &mut Workspace, program: &Path, capture: &Path) -> Result<String, ToolError> {
        self.step(ws, Step::Study, Status::Inspecting, "Starting it for a moment to picture its window…")?;
        let shot = capture.join("window.png");
        let tree = capture.join("ui-tree.json");
        let params =
            json!({"program": program.display().to_string(), "wait_secs": 15, "output": shot.display().to_string(), "ui_tree": tree.display().to_string()});
        let picture = "capture/window.png is a picture of its window 15 seconds after it started: match its layout, colours and proportions.\n";
        let mut notes = String::from("\n## How it looks running\n\n");
        notes.push_str(&match self.call("native_window_capture", params) {
            Ok(_) if tree.is_file() => format!(
                "{picture}capture/ui-tree.json lists the controls in its windows at that moment, in order (role, title, identifier, value, position and size in pixels): use it for the names, order and placement of its menus, buttons and fields.\n"
            ),
            Ok(_) => picture.to_string(),
            Err(e) => format!("Starting it for a picture didn't work ({}), so there's no picture of it running.\n", e.message),
        });
        Ok(notes)
    }

    /// The site's other pages (up to `build.max_pages` same-site pages: links from the start
    /// page, then links from those pages while there's room), each captured into
    /// capture/routes/<name>/ and listed in capture/routes.json. A page that can't be captured is
    /// left out; it never stops the study.
    fn capture_routes(&mut self, ws: &mut Workspace, start: &str, widths: &[u32]) -> Result<(), ToolError> {
        let capture = ws.capture_dir();
        let read = |dir: &Path| std::fs::read(dir.join("capture.json")).ok().and_then(|b| serde_json::from_slice::<Value>(&b).ok());
        let found = read(&capture).unwrap_or(Value::Null);
        let max = self.engine.session().settings.build.max_pages.min(kinds::website::MAX_ROUTES);
        let picked = kinds::website::pick_routes(&found, start, max);
        let mut kept = self.capture_pages(ws, &picked, widths)?;
        // One level deeper: pages linked from the ones just captured, while there's room.
        if kept.len() < max && !kept.is_empty() {
            let captures: Vec<Value> = kept.iter().filter_map(|r| read(&capture.join("routes").join(&r.slug))).collect();
            let deeper = kinds::website::pick_deeper_routes(&captures, start, &kept, max - kept.len());
            kept.extend(self.capture_pages(ws, &deeper, widths)?);
        }
        if !kept.is_empty() {
            let json = serde_json::to_vec_pretty(&kept).map_err(|e| ToolError::internal(e.to_string()))?;
            craft_anything_engine::project::write_atomic(&capture.join(kinds::website::ROUTES_FILE), &json)?;
        }
        Ok(())
    }

    /// Capture each page into capture/routes/<slug>/; returns the ones that worked.
    fn capture_pages(&mut self, ws: &mut Workspace, picked: &[kinds::website::Route], widths: &[u32]) -> Result<Vec<kinds::website::Route>, ToolError> {
        let capture = ws.capture_dir();
        let mut kept = Vec::new();
        if !picked.is_empty() {
            // web_site_capture creates its own folder, inside one that exists.
            std::fs::create_dir_all(capture.join("routes")).map_err(|e| ToolError::io(capture.display(), e))?;
        }
        for (i, r) in picked.iter().enumerate() {
            self.step(ws, Step::Study, Status::Inspecting, &format!("Photographing more pages ({} of {}): {}…", i + 1, picked.len(), r.path))?;
            let dir = capture.join("routes").join(&r.slug);
            let got = self.call("web_site_open", json!({"url": r.url})).and_then(|_| {
                self.call(
                    "web_site_capture",
                    json!({"output_directory": dir.display().to_string(), "include_html": false, "widths": widths, "noise_pass": true}),
                )
            });
            match got {
                Ok(_) => kept.push(r.clone()),
                // Said in the feed, not hidden: the page is left out and the study goes on.
                Err(e) => self.step(ws, Step::Study, Status::Inspecting, &format!("Couldn't photograph {}: {}", r.path, e.message))?,
            }
        }
        Ok(kept)
    }

    /// Deep mode with Ghidra installed: run Ghidra's one-time analysis of `program` on its own
    /// thread; the receiver gets its result. The builder's tools share the analysis (waiting for it
    /// rather than starting another), and it carries on if the person pauses, so nothing is lost.
    pub(super) fn prepare_ghidra(&self, program: &Path) -> Option<Receiver<Result<(), String>>> {
        // A folder (nothing in it recognised as the program) isn't something Ghidra can open.
        if !program.is_file() || is_electron_runtime(program) {
            return None;
        }
        let settings = {
            let s = self.engine.session();
            if s.clean_room() || s.settings.ghidra_launcher().is_none() {
                return None;
            }
            s.settings.clone()
        };
        let program = program.to_path_buf();
        let (tx, rx) = std::sync::mpsc::channel();
        let (feed, events, wake) = (self.feed.clone(), self.tx.clone(), self.wake.clone());
        std::thread::Builder::new()
            .name("ghidra-prepare".into())
            .spawn(move || {
                let result = std::fs::read(&program).map_err(|e| e.to_string()).and_then(|bytes| {
                    let sha = craft_anything_evidence::sha256_hex(&bytes);
                    // Said in the feed (once per change), so a wait for an earlier analysis shows.
                    let last = Mutex::new(String::new());
                    let said = |m: &str| {
                        let mut last = last.lock().unwrap_or_else(PoisonError::into_inner);
                        if *last != m {
                            *last = m.to_string();
                            emit_event(&feed, &events, &wake, RunEvent::Step { step: Step::Study, text: m.to_string() });
                        }
                    };
                    craft_anything_engine::tools::deep::prepare(&settings, &bytes, &sha, &said)
                });
                if let Err(e) = &result {
                    log::warn!("Ghidra's analysis of {} failed: {e}", program.display());
                }
                let _ = tx.send(result);
            })
            .ok()?;
        Some(rx)
    }
}

/// Deep mode's notes on how a website works, in capture/.
pub(super) const HOW_IT_WORKS: &str = "how-it-works.md";

/// The rendered HTML a Deep website capture keeps; a Clean room capture records only what a
/// visitor sees.
const PAGE_HTML: &str = "page.html";

/// Remove what only a Deep study writes before a Clean room study replaces the capture: its
/// builder must not find what only Deep mode may see. A missing file is fine; any other failure
/// stops the study rather than leave it behind.
fn remove_deep_only(capture: &Path) -> Result<(), ToolError> {
    for file in [HOW_IT_WORKS, PAGE_HTML] {
        let path = capture.join(file);
        match std::fs::remove_file(&path) {
            Ok(()) => {}
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => return Err(ToolError::io(path.display(), e)),
        }
    }
    let runs = capture.join(kinds::runs::RUNS_DIR);
    match std::fs::remove_dir_all(&runs) {
        Ok(()) => {}
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
        Err(e) => return Err(ToolError::io(runs.display(), e)),
    }
    Ok(())
}

/// A short text view of a tool result for the study notes.
pub(super) fn describe(out: &ToolOutput) -> String {
    match out {
        ToolOutput::Evidence(e) => {
            let body = serde_json::to_string_pretty(&e.result).unwrap_or_default();
            format!("{}\n\nEvidence {}.\n\n```json\n{}\n```\n", e.summary, e.id, body.chars().take(20_000).collect::<String>())
        }
        ToolOutput::Value(v) => format!("```json\n{}\n```\n", serde_json::to_string_pretty(v).unwrap_or_default().chars().take(20_000).collect::<String>()),
    }
}
