//! Simple mode's front door, for people new to computers: two numbered questions in plain words
//! ("What do you want to recreate?", "Say what you want"), big targets and one big Start button. Below it,
//! the projects made so far.

use crate::i18n::tr;
use crate::state::{MakeDraft, Screen};
use crate::theme::{self, Tokens};
use crate::{CraftApp, Pick, icons, widgets};
use craft_anything_engine::Mode;
use craft_anything_engine::workspace::{ExistingFiles, Status, TargetRef};
use egui::{RichText, Sense, Ui, Vec2};

const SUGGESTIONS_WEB: &[&str] = &["Recreate this website", "Recreate it with darker colours"];
const SUGGESTIONS_APP: &[&str] = &["Make my own version of this app", "Recreate just its main feature"];
const SUGGESTIONS_GAME: &[&str] = &["Make a game that plays like this", "Make a simpler version of this game"];
const SUGGESTIONS_NONE: &[&str] = &["A website for my bakery", "A simple snake game"];

/// The first http(s) address in some text.
pub fn find_url(text: &str) -> Option<String> {
    text.split_whitespace()
        .map(|w| w.trim_matches(|c: char| matches!(c, '"' | '\'' | '(' | ')' | '<' | '>' | ',' | '.' | ';')))
        .find(|w| w.starts_with("http://") || w.starts_with("https://"))
        .map(str::to_string)
}

/// Why a typed web address can't be used, or `None` when it's fine (or empty).
pub fn url_problem(typed: &str) -> Option<&'static str> {
    let t = typed.trim();
    if t.is_empty() {
        return None;
    }
    let url = normalize_url(t);
    let host = url.split("://").nth(1).unwrap_or_default().split(['/', '?', '#']).next().unwrap_or_default();
    let host = host.rsplit('@').next().unwrap_or_default().split(':').next().unwrap_or_default();
    let ok = (url.starts_with("http://") || url.starts_with("https://"))
        && !t.chars().any(char::is_whitespace)
        && (host == "localhost" || (host.contains('.') && !host.starts_with('.') && !host.ends_with('.')));
    (!ok).then_some("Enter a full web address, like example.com or https://example.com/page.")
}

/// "example.com" → "https://example.com"; anything with a scheme is kept.
pub fn normalize_url(s: &str) -> String {
    let t = s.trim();
    if t.is_empty() || t.contains("://") { t.to_string() } else { format!("https://{t}") }
}

/// The target the draft will use: the picked one, the typed address, or an address in the request.
/// "Work on my project" has none: an address in its request is a link to add, not an original.
pub fn resolve_target(d: &MakeDraft) -> TargetRef {
    if d.mine {
        return TargetRef::None;
    }
    if d.target != TargetRef::None {
        return d.target.clone();
    }
    if !d.url.trim().is_empty() {
        return TargetRef::parse(&normalize_url(&d.url));
    }
    find_url(&d.request).map(|url| TargetRef::Website { url }).unwrap_or(TargetRef::None)
}

/// A game is being copied or made, so it's built with a game engine ("Build with").
pub fn is_game(kinds: &craft_anything_ai::rebuild::KindRegistry, d: &MakeDraft) -> bool {
    use craft_anything_ai::rebuild::GameKind;
    if let Some(stack) = own_stack(kinds, d) {
        return craft_anything_engines::game::Builder::from_stack(stack).is_some();
    }
    d.found.is_some()
        || (d.game && matches!(d.target, TargetRef::Local { .. }))
        || kinds.choose(&d.request, &resolve_target(d), false).is_some_and(|(k, _)| k.id() == GameKind::ID)
}

/// What the person's own files are built with, when they're the app and the request names no
/// other stack (the run decides the same way: `project::first_stack`).
pub fn own_stack<'a>(kinds: &craft_anything_ai::rebuild::KindRegistry, d: &'a MakeDraft) -> Option<&'a str> {
    let own = d.existing == Some(ExistingFiles::Edit) && kinds.named(&d.request, &resolve_target(d)).is_none();
    d.folder_stack.as_deref().filter(|_| own)
}

/// The front door: make your own version of something, work on a project you already have, or
/// just look inside something.
pub fn hero(app: &mut CraftApp, ui: &mut Ui, w: f32) {
    let t = Tokens::get(ui.ctx());
    let door = if app.state.make.look { 2 } else { usize::from(app.state.make.mine) };
    // The ways in, with Simple / Advanced at the row's end (it adds no height above Start), or
    // on a line of its own when the window is too narrow for both.
    let (picked, fits) = ui
        .horizontal(|ui| {
            let picked = widgets::segmented(ui, &["Make my own version", "Work on my project", "Just look inside"], door, theme::ControlSize::Large);
            let fits = ui.available_width() >= mode_choice_width(ui);
            if fits {
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| mode_choice(app, ui));
            }
            (picked, fits)
        })
        .inner;
    if !fits {
        ui.add_space(theme::space::SM);
        ui.horizontal(|ui| ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| mode_choice(app, ui)));
    }
    let d = &mut app.state.make;
    if let Some(i) = picked {
        let left = d.mine && i != 1;
        (d.mine, d.look) = (i == 1, i == 2);
        if d.mine {
            // The person's own project: there's nothing to recreate.
            (d.target, d.editing_url, d.found, d.builder, d.game) = (TargetRef::None, false, None, None, false);
            d.url.clear();
        }
        if left && d.existing == Some(ExistingFiles::Edit) {
            // A version made on Make never edits the project picked here.
            (d.folder, d.folder_files, d.existing) = (None, None, None);
        }
    }
    ui.add_space(theme::space::LG);
    if app.state.make.look {
        look(app, ui, w);
        return;
    }
    if app.state.make.mine {
        mine(app, ui, w);
        return;
    }
    ui.label(theme::heading(tr("What do you want to make?"), theme::TITLE));
    ui.add_space(theme::space::SM);
    ui.label(RichText::new(tr("Choose what to recreate, say what you want, then press Start.")).size(theme::LEAD).color(t.text_dim));
    ui.add_space(theme::space::LG);
    widgets::card(&t).inner_margin(theme::pad::ROOMY).show(ui, |ui| {
        ui.set_width(w - 50.0);
        step_title(ui, 1, "What do you want to recreate?");
        ui.add_space(theme::space::MD);
        choose_target(app, ui);
        // How closely the original may be looked at: in view from the start, chosen once there is one.
        ui.add_space(theme::space::LG);
        reach_choice(app, ui);
        ui.add_space(theme::space::LG);
        step_title(ui, 2, "Say what you want");
        ui.add_space(theme::space::MD);
        // The example matches the tile picked above (a "recreate this website" hint under
        // "Nothing — start fresh" contradicts the choice).
        let d = &app.state.make;
        let hint = match &d.target {
            TargetRef::Local { .. } if d.game => "For example: the same game, with a high-score table",
            TargetRef::Local { .. } => "For example: the same app, but simpler",
            TargetRef::Website { .. } => "For example: recreate this website",
            TargetRef::None if d.editing_url || !d.url.is_empty() => "For example: recreate this website",
            TargetRef::None => "For example: a website for my bakery",
        };
        // Websites are photographed for the builder; anything else looks right from pictures.
        let website = matches!(resolve_target(d), TargetRef::Website { .. }) || d.editing_url;
        let submit = request_box(app, ui, hint, (!website).then_some("Add pictures of the original (optional: screenshots help it look right)"));
        if app.state.make.request.trim().is_empty() {
            ui.add_space(theme::space::SM);
            suggestions(app, ui);
        }
        engine_choices(app, ui);
        ui.add_space(theme::space::LG);
        ui.horizontal(|ui| {
            crate::builders::built_by(app, ui);
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                let ready = !app.state.make.request.trim().is_empty() && url_problem(&app.state.make.url).is_none() && app.creating.is_none();
                let start =
                    ui.add(widgets::Button::new(widgets::ButtonKind::Primary, "Start").icon(Some("sparkles")).size(theme::ControlSize::Hero).enabled(ready));
                let start = if ready { start } else { start.on_disabled_hover_text(tr("Say what you want first.")) };
                if (start.clicked() || submit) && ready {
                    app.start_make();
                }
            });
        });
        // Where it's saved: secondary to Start, so under it.
        ui.add_space(theme::space::SM);
        crate::folders::make_line(app, ui);
    });
}

/// "Work on my project": the folder of a project the person already has, whose files the builder
/// edits in place, and what to change. There's no original to pick, so no Clean room / Deep choice.
fn mine(app: &mut CraftApp, ui: &mut Ui, w: f32) {
    let t = Tokens::get(ui.ctx());
    ui.label(theme::heading(tr("Work on a project you already have"), theme::TITLE));
    ui.add_space(theme::space::SM);
    ui.label(RichText::new(tr("Pick its folder, say what you want, then press Start.")).size(theme::LEAD).color(t.text_dim));
    ui.add_space(theme::space::LG);
    widgets::card(&t).inner_margin(theme::pad::ROOMY).show(ui, |ui| {
        ui.set_width(w - 50.0);
        step_title(ui, 1, "Where is your project?");
        ui.add_space(theme::space::MD);
        // A folder picked on Make for another use isn't this project's.
        let d = &app.state.make;
        let folder = d.folder.clone().filter(|_| d.existing == Some(ExistingFiles::Edit));
        match &folder {
            None => {
                let size = Vec2::new(ui.available_width(), 60.0);
                if widgets::choice_tile(ui, size, "folder", "Choose your project's folder…", "", false).clicked() {
                    app.pick(Pick::OwnProject);
                }
            }
            Some(f) => {
                let place = f.display().to_string();
                ui.horizontal(|ui| {
                    icons::show(ui, "folder", theme::icon::MD, t.icon);
                    ui.add(egui::Label::new(RichText::new(&place).font(theme::semibold(theme::BODY))).truncate()).on_hover_text(&place);
                });
                ui.horizontal_wrapped(|ui| {
                    ui.add_space(theme::space::XL);
                    ui.label(RichText::new(tr("your files there are the app, edited in place")).size(theme::SMALL).color(t.text_dim));
                    ui.label(RichText::new("·").size(theme::SMALL).color(t.text_faint));
                    if widgets::link(ui, "Pick another folder…").clicked() {
                        app.pick(Pick::OwnProject);
                    }
                });
            }
        }
        ui.add_space(theme::space::LG);
        step_title(ui, 2, "Say what you want");
        ui.add_space(theme::space::MD);
        let submit = request_box(app, ui, "For example: add a dark mode", Some("Attach pictures (or drop or paste them here)"));
        engine_choices(app, ui);
        ui.add_space(theme::space::LG);
        ui.horizontal(|ui| {
            crate::builders::built_by(app, ui);
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                let said = !app.state.make.request.trim().is_empty();
                let ready = folder.is_some() && said && app.creating.is_none();
                let start =
                    ui.add(widgets::Button::new(widgets::ButtonKind::Primary, "Start").icon(Some("sparkles")).size(theme::ControlSize::Hero).enabled(ready));
                let start = if ready {
                    start
                } else {
                    start.on_disabled_hover_text(tr(if folder.is_none() { "Pick your project's folder first." } else { "Say what you want first." }))
                };
                if (start.clicked() || submit) && ready {
                    app.start_make();
                }
            });
        });
    });
}

/// "Say what you want": the request, and the pictures that go with it (`attach` names the
/// paperclip; `None` takes none). True when it was sent from the keyboard.
fn request_box(app: &mut CraftApp, ui: &mut Ui, hint: &str, attach: Option<&str>) -> bool {
    let previews = app.previews(ui.ctx(), &app.state.make.pictures.clone());
    let out = crate::prompt_box::show(
        ui,
        &mut app.state.make.request,
        &app.state.make.pictures,
        crate::prompt_box::PromptBox {
            name: "Say what you want",
            hint: tr(hint),
            rows: (3, 10),
            attach,
            previews: &previews,
            stop_button: None,
            stopping: false,
            send: false,
            recall: false,
            history: None,
            font_size: theme::LEAD,
            settings: None,
        },
    );
    if let Some(i) = out.remove {
        app.state.make.pictures.remove(i);
    }
    if out.attach {
        app.pick(Pick::Pictures);
    }
    if out.paste {
        app.paste_pictures(false);
    }
    out.send
}

/// For a game: the engine it's built with, and whether the builder may generate its assets.
fn engine_choices(app: &mut CraftApp, ui: &mut Ui) {
    if is_game(&app.kinds, &app.state.make) {
        ui.add_space(theme::space::LG);
        crate::engine_ui::build_with(app, ui);
        ui.add_space(theme::space::LG);
        let on = app.state.make.generate_assets;
        crate::engine_ui::assets_toggle(app, ui, on);
    }
}

/// "Just look inside": what to look at, each opening straight away. Nothing is built or run.
fn look(app: &mut CraftApp, ui: &mut Ui, w: f32) {
    let t = Tokens::get(ui.ctx());
    ui.label(theme::heading(tr("What do you want to look inside?"), theme::TITLE));
    ui.add_space(theme::space::SM);
    ui.label(RichText::new(tr("We only read it; nothing runs. Every answer shows how sure we are.")).size(theme::LEAD).color(t.text_dim));
    ui.add_space(theme::space::LG);
    widgets::card(&t).inner_margin(theme::pad::ROOMY).show(ui, |ui| {
        ui.set_width(w - 50.0);
        // Each of these reads the original, which Clean room never does.
        if !app.settings.deep_mode {
            crate::engine_ui::deep_needed(app, ui);
            return;
        }
        let size = Vec2::new(ui.available_width(), 72.0);
        ui.spacing_mut().item_spacing.y = theme::space::SM;
        if widgets::choice_tile(ui, size, "app-window", "An app or a file", "See how a program works: what it does, what it talks to, what's inside.", false)
            .clicked()
        {
            app.pick(Pick::OpenFile);
        }
        if widgets::choice_tile(ui, size, "globe", "A website", "See how a page is built: its layout, styles, scripts and the requests it makes.", false)
            .clicked()
        {
            app.state.modal = Some(crate::state::Modal::Website);
        }
        if widgets::choice_tile(ui, size, "package", "A game's files", "See what a game packs, and save its pictures as PNG files.", false).clicked() {
            app.state.screen = crate::state::Screen::GameFiles;
        }
    });
}

/// "1  What do you want to recreate?" — a numbered circle and a plain question.
fn step_title(ui: &mut Ui, n: u32, text: &str) {
    let t = Tokens::get(ui.ctx());
    ui.horizontal(|ui| {
        let (r, _) = ui.allocate_exact_size(Vec2::splat(28.0), Sense::hover());
        ui.painter().rect(r, r.width() / 2.0, t.raised, egui::Stroke::new(1.0, t.card_border), egui::StrokeKind::Inside);
        let g = widgets::label_galley(ui, &n.to_string(), theme::SMALL, t.text);
        ui.painter().galley_with_override_text_color(r.center() - g.size() / 2.0, g, t.text);
        ui.add_space(theme::space::XS);
        ui.label(theme::heading(tr(text), theme::SECTION));
    });
}

fn choose_target(app: &mut CraftApp, ui: &mut Ui) {
    let t = Tokens::get(ui.ctx());
    let d = &app.state.make;
    let local = matches!(d.target, TargetRef::Local { .. }).then_some(d.game);
    let website = local.is_none() && (d.editing_url || !d.url.is_empty());
    let scratch = local.is_none() && !website;
    let gap = theme::space::MD;
    let cols = if ui.available_width() > 760.0 { 4 } else { 2 };
    let tile_w = ((ui.available_width() - gap * (cols as f32 - 1.0)) / cols as f32).floor();
    let size = Vec2::new(tile_w, 60.0);
    let mut pick = None;
    ui.horizontal_wrapped(|ui| {
        ui.spacing_mut().item_spacing = Vec2::splat(gap);
        for (i, (icon, label, on)) in [
            ("globe", "A website", website),
            ("app-window", "An app", local == Some(false)),
            ("puzzle", "A game", local == Some(true)),
            ("sparkles", "Nothing — start fresh", scratch),
        ]
        .into_iter()
        .enumerate()
        {
            if widgets::choice_tile(ui, size, icon, label, "", on).clicked() {
                pick = Some(i);
            }
        }
    });
    let m = &mut app.state.make;
    if matches!(pick, Some(0) | Some(3)) {
        // A different kind of start: forget the last game's engine and the engine picked for it.
        m.found = None;
        m.builder = None;
    }
    match pick {
        Some(0) => {
            m.target = TargetRef::None;
            m.editing_url = true;
        }
        Some(1) => app.pick(Pick::TargetFile),
        Some(2) => app.pick(Pick::TargetFolder),
        Some(_) => {
            m.target = TargetRef::None;
            m.url.clear();
            m.editing_url = false;
        }
        None => {}
    }
    let m = &mut app.state.make;
    // Recomputed so the address field shows on the very frame "A website" is clicked.
    let website = !matches!(m.target, TargetRef::Local { .. }) && (m.editing_url || !m.url.is_empty());
    if let TargetRef::Local { path } = m.target.clone() {
        ui.add_space(theme::space::LG);
        ui.horizontal(|ui| {
            ui.label(RichText::new(tr("You chose:")).size(theme::BODY).color(t.text_dim));
            // A long name (an APK from a mirror) is cut short, whole on hover, so the row never widens the page.
            let name = m.target.label();
            ui.add(egui::Label::new(RichText::new(name.clone()).font(theme::semibold(theme::BODY))).truncate()).on_hover_text(name);
            ui.add(egui::Label::new(RichText::new(crate::short_path(&path)).color(t.text_faint).size(theme::SMALL)).truncate());
        });
    } else if website {
        ui.add_space(theme::space::LG);
        ui.label(RichText::new(tr("Type the website's address:")).size(theme::BODY).color(t.text_dim));
        ui.add_space(theme::space::SM);
        let r = ui.add(
            egui::TextEdit::singleline(&mut m.url)
                .hint_text("example.com")
                .desired_width(520.0)
                .font(egui::FontId::proportional(theme::LEAD))
                .frame(widgets::text_field_frame(&t).inner_margin(theme::pad::ROW)),
        );
        if pick == Some(0) {
            r.request_focus();
        }
        // Say what's wrong right under the field, once something has been typed.
        if let Some(problem) = url_problem(&m.url) {
            ui.add_space(theme::space::XS);
            ui.horizontal(|ui| {
                icons::show(ui, "triangle-alert", theme::icon::MD, t.danger);
                ui.label(RichText::new(tr(problem)).color(t.danger).size(theme::BODY));
            });
        }
    }
}

/// Clean room or Deep for what is recreated: the title bar's switch, as a plain choice.
fn look_inside(app: &mut CraftApp, ui: &mut Ui) {
    let t = Tokens::get(ui.ctx());
    // The app's one setting (the title bar's switch), not this project's: said where it's chosen.
    ui.horizontal_wrapped(|ui| {
        ui.label(RichText::new(tr("How closely may Craft Anything look at it?")).size(theme::BODY).color(t.text_dim));
        ui.label(RichText::new(tr("For everything you make, like the switch in the title bar.")).size(theme::SMALL).color(t.text_faint));
    });
    ui.add_space(theme::space::SM);
    let deep = app.settings.deep_mode;
    let gap = theme::space::MD;
    let size = Vec2::new(((ui.available_width() - gap) / 2.0).floor(), 72.0);
    let mut pick = None;
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = gap;
        for (on, icon, label, blurb) in [
            (false, "shield-check", "Clean room", "Nothing inside it is read; Craft Anything works from what you can see. Your version gets its own art."),
            (true, "microscope", "Deep", "Craft Anything reads its code and files with every tool it has. For your private use."),
        ] {
            if widgets::choice_tile(ui, size, icon, label, blurb, deep == on).clicked() {
                pick = Some(on);
            }
        }
    });
    if let Some(on) = pick {
        app.set_deep(on);
    }
    // Ghidra reads programs, not websites.
    if !matches!(app.state.make.target, TargetRef::Local { .. }) {
        return;
    }
    if app.settings.deep_mode && !app.ghidra_missing() {
        ui.add_space(theme::space::SM);
        ui.horizontal_wrapped(|ui| {
            icons::show(ui, "cpu", theme::icon::MD, t.icon);
            let line = format!("{} {}", crate::engine_ui::ghidra_memory_words(app), tr("More makes a big program's first look faster."));
            ui.label(RichText::new(line).size(theme::SMALL).color(t.text_dim)).on_hover_text(crate::engine_ui::ghidra_memory_guide());
            if widgets::link(ui, "Change").clicked() {
                app.state.settings_open = true;
                app.state.settings_tab = crate::state::SettingsTab::Engines;
            }
        });
    }
    if app.settings.deep_mode && app.ghidra_missing() {
        ui.add_space(theme::space::SM);
        ui.horizontal(|ui| {
            icons::show(ui, "triangle-alert", theme::icon::MD, t.warning);
            ui.label(RichText::new(tr("Ghidra isn't on this computer yet, so the built-in decompiler stands in.")).size(theme::SMALL).color(t.text_dim));
            if app.ghidra.job.is_none() && widgets::secondary(ui, Some("download"), "Get Ghidra").clicked() {
                app.state.modal = Some(crate::state::Modal::Ghidra);
            }
        });
    }
}

fn suggestions(app: &mut CraftApp, ui: &mut Ui) {
    let t = Tokens::get(ui.ctx());
    let list = match resolve_target(&app.state.make) {
        TargetRef::Website { .. } => SUGGESTIONS_WEB,
        TargetRef::Local { .. } if app.state.make.game => SUGGESTIONS_GAME,
        TargetRef::Local { .. } => SUGGESTIONS_APP,
        TargetRef::None => SUGGESTIONS_NONE,
    };
    ui.horizontal_wrapped(|ui| {
        ui.spacing_mut().item_spacing = Vec2::splat(theme::space::SM);
        ui.label(RichText::new(tr("Ideas:")).size(theme::BODY).color(t.text_dim));
        for s in list.iter().take(2) {
            if widgets::suggestion(ui, s).clicked() {
                app.state.make.request = tr(s);
            }
        }
    });
}

/// The room [`mode_choice`] takes: the Simple / Advanced switch and its name, measured in the
/// language shown (a longer translation needs more), with the gaps beside each.
fn mode_choice_width(ui: &Ui) -> f32 {
    let t = Tokens::get(ui.ctx());
    let switch = widgets::segmented_width(ui, &["Simple", "Advanced"], theme::ControlSize::Medium);
    let name = widgets::label_galley(ui, &tr("Mode"), theme::SMALL, t.text_dim).size().x;
    switch + name + 2.0 * ui.spacing().item_spacing.x + theme::space::SM
}

/// Simple or Advanced, beside the ways in. Simple mode's title bar leaves the switch out, so
/// someone starting here would never see that Advanced exists. It's the app's one setting
/// (`set_mode`, as in Settings and the title bar). Advanced normally opens on the workbench; from
/// here it stays on this screen (Advanced shows it too), so the draft stays in view.
/// Drawn right to left: the switch, then its name.
fn mode_choice(app: &mut CraftApp, ui: &mut Ui) {
    let t = Tokens::get(ui.ctx());
    let sel = usize::from(app.settings.mode == Mode::Experienced);
    if let Some(i) = widgets::segmented(ui, &["Simple", "Advanced"], sel, theme::ControlSize::Medium) {
        let screen = app.state.screen;
        app.set_mode(if i == 0 { Mode::Beginner } else { Mode::Experienced });
        if screen == Screen::Home {
            app.state.screen = Screen::Home;
        }
    }
    ui.label(RichText::new(tr("Mode")).size(theme::SMALL).color(t.text_dim))
        .on_hover_text(tr("Simple: say what you want and it's handled for you. Advanced: every tool, the evidence and the logs."));
}

/// Clean room or Deep, shown from the start so the choice is in view before anything is picked.
/// With no original yet there's nothing to look inside: the tiles are dimmed and say so.
fn reach_choice(app: &mut CraftApp, ui: &mut Ui) {
    let d = &app.state.make;
    let original = matches!(d.target, TargetRef::Local { .. }) || d.editing_url || !d.url.is_empty();
    ui.add_enabled_ui(original, |ui| look_inside(app, ui));
    if !original {
        let t = Tokens::get(ui.ctx());
        ui.add_space(theme::space::XS);
        ui.label(RichText::new(tr("This applies once you choose something to recreate above.")).size(theme::SMALL).color(t.text_faint));
    }
}

/// Time since `ms` in plain words.
pub fn ago(ms: u64) -> String {
    let now = craft_anything_engine::inbox::now_ms();
    let s = now.saturating_sub(ms) / 1000;
    match s {
        0..=59 => tr("just now"),
        60..=3599 => crate::i18n::trf("{n} min ago", &[("n", &(s / 60).to_string())]),
        3600..=86_399 => crate::i18n::trf("{n} h ago", &[("n", &(s / 3600).to_string())]),
        86_400..=172_799 => tr("yesterday"),
        _ => crate::i18n::trf("{n} days ago", &[("n", &(s / 86_400).to_string())]),
    }
}

/// A project's status as the app shows it: waiting its turn, interrupted (its run stopped
/// part-way: the app or the computer closed), or the one it saved. (label, text, fill).
pub fn status_shown(app: &CraftApp, ws: &craft_anything_engine::workspace::Workspace, t: &Tokens) -> (&'static str, egui::Color32, egui::Color32) {
    let s = ws.manifest.status;
    let (label, look) = if app.asking.contains(&ws.root) {
        ("Needs you", Status::NeedsYou)
    } else if app.runs.position(&ws.root).is_some() {
        ("Waiting", s)
    } else if crate::project::interrupted(app, ws) {
        ("Interrupted", Status::NeedsYou)
    } else {
        (s.label(), s)
    };
    let (fg, bg) = status_colors(t, look);
    (label, fg, bg)
}

pub fn status_colors(t: &Tokens, s: Status) -> (egui::Color32, egui::Color32) {
    match s {
        Status::Ready => (t.success, t.raised),
        Status::NeedsYou => (t.warning, t.raised),
        Status::Stopped | Status::New => (t.text_dim, t.raised),
        _ => (t.accent_ink, t.accent_soft),
    }
}

/// A project's icon: a game (built with a game engine), a website, an app, or something new.
pub fn project_icon(m: &craft_anything_engine::workspace::Manifest) -> &'static str {
    if craft_anything_engines::game::Builder::from_stack(&m.stack).is_some() {
        return "puzzle";
    }
    match m.target {
        TargetRef::Website { .. } => "globe",
        TargetRef::Local { .. } => "app-window",
        TargetRef::None => "sparkles",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn web_addresses_are_checked_inline() {
        for ok in ["", "example.com", "https://a.b/c?d", "http://localhost:5173", "sub.example.co.uk/page"] {
            assert!(url_problem(ok).is_none(), "{ok}");
        }
        for bad in ["example", "https://", "two words.com", "ftp://x.y", "https://.com", "x."] {
            assert!(url_problem(bad).is_some(), "{bad}");
        }
    }

    #[test]
    fn the_persons_own_files_say_whether_it_is_a_game() {
        use crate::engine_ui::chosen;
        use craft_anything_engines::game::Builder;
        let kinds = craft_anything_ai::rebuild::KindRegistry::built_in();
        let own = |stack: Option<&str>, request: &str| MakeDraft {
            folder_stack: stack.map(str::to_string),
            existing: Some(ExistingFiles::Edit),
            request: request.into(),
            ..Default::default()
        };
        let godot = own(Some("Godot"), "Fix the jump");
        assert!(is_game(&kinds, &godot), "whatever the request's words say");
        assert_eq!(chosen(&kinds, &godot), Some(Builder::Godot));
        assert!(!is_game(&kinds, &own(Some("Python"), "Fix my game")), "no engine to get for a Python game");
        assert_eq!(chosen(&kinds, &own(Some("Python"), "Fix my game with Godot")), Some(Builder::Godot), "the person's words come first");
        assert!(is_game(&kinds, &own(None, "Fix my game")), "nothing read from the folder: the words decide");
        let start_from = MakeDraft { existing: Some(ExistingFiles::StartFrom), ..own(Some("Python"), "Fix my game") };
        assert!(is_game(&kinds, &start_from), "material to start from doesn't decide");
    }

    #[test]
    fn targets_from_the_draft() {
        let mut d = MakeDraft { request: "Please recreate https://example.com/pricing.".into(), ..Default::default() };
        assert_eq!(resolve_target(&d), TargetRef::Website { url: "https://example.com/pricing".into() });
        d.url = "acme.io".into();
        assert_eq!(resolve_target(&d), TargetRef::Website { url: "https://acme.io".into() }, "the typed address wins");
        d.url.clear();
        d.request = "a snake game".into();
        assert_eq!(resolve_target(&d), TargetRef::None);
        assert_eq!(normalize_url("http://x.y"), "http://x.y");
        let own = MakeDraft { mine: true, request: "Link the footer to https://example.com".into(), ..Default::default() };
        assert_eq!(resolve_target(&own), TargetRef::None, "an address in the request isn't an original");
    }
}
