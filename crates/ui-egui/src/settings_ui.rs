//! Settings: General (mode, theme, language), Engines (Ghidra, tool paths), AI, Builds (when a
//! build is good enough, generated art), Plugins (MCP servers), Safety.

use crate::i18n::{LANGUAGES, tr, trf};
use crate::state::SettingsTab;
use crate::theme::{self, ThemeKind, Tokens};
use crate::{CraftApp, Pick, icons, widgets};
use craft_anything_ai::BackendKind;
use craft_anything_engine::Mode;
use egui::{RichText, Ui, Vec2};
use std::path::PathBuf;

/// Draws the Settings modal and the content for its selected tab.
pub fn show(app: &mut CraftApp, ctx: &egui::Context) {
    if !app.state.settings_open {
        return;
    }
    let t = Tokens::get(ctx);
    let mut open = true;
    // A modal (dimmed backdrop, Esc or a click outside closes) with a full-size close button;
    // egui's window close box is 14 px.
    let resp = egui::Modal::new(egui::Id::new("settings-window"))
        .frame(egui::Frame::new().fill(t.card).corner_radius(t.r_lg()).inner_margin(theme::pad::CARD).stroke(egui::Stroke::new(1.0, t.card_border)))
        .show(ctx, |ui| {
            ui.set_width((ctx.content_rect().width() - 2.0 * theme::space::XXL).clamp(560.0, 860.0));
            ui.horizontal(|ui| {
                ui.label(theme::heading(tr("Settings"), theme::SECTION));
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if icons::button(ui, "x", theme::ControlSize::Medium, false, "Close").clicked() {
                        open = false;
                    }
                });
            });
            // An error (a save that failed) shows here too: the message bar is behind this window.
            if let Some(i) = app.state.toasts.iter().rposition(|(_, error)| *error) {
                let text = app.state.toasts[i].0.clone();
                ui.add_space(theme::space::SM);
                ui.horizontal(|ui| {
                    icons::show(ui, "triangle-alert", theme::icon::MD, t.danger);
                    ui.add(egui::Label::new(RichText::new(text).color(t.danger)).wrap());
                    if icons::button(ui, "x", theme::ControlSize::Small, false, "Dismiss").clicked() {
                        app.state.toasts.remove(i);
                    }
                });
            }
            ui.add_space(theme::space::MD);
            ui.horizontal_top(|ui| {
                ui.vertical(|ui| {
                    ui.set_width(150.0);
                    for (tab, icon, title) in [
                        (SettingsTab::General, "settings", "General"),
                        (SettingsTab::Engines, "cpu", "Engines"),
                        (SettingsTab::Ai, "bot", "AI"),
                        (SettingsTab::Builds, "hammer", "Builds"),
                        (SettingsTab::Plugins, "puzzle", "Plugins"),
                        (SettingsTab::Safety, "shield", "Safety"),
                        (SettingsTab::About, "info", "About"),
                    ] {
                        // About sits at the bottom of the list, level with the end of the page.
                        if tab == SettingsTab::About {
                            ui.add_space((page_height(ctx) - ui.min_rect().height() - TAB_H).max(theme::space::SM));
                        }
                        let on = app.state.settings_tab == tab;
                        let (rect, r) = ui.allocate_exact_size(Vec2::new(150.0, TAB_H), egui::Sense::click());
                        r.widget_info(|| egui::WidgetInfo::selected(egui::WidgetType::SelectableLabel, true, on, tr(title)));
                        if on {
                            ui.painter().rect_filled(rect, t.radius, t.accent_soft);
                        } else if r.hovered() {
                            ui.painter().rect_filled(rect, t.radius, t.hover);
                        }
                        icons::paint(
                            ui,
                            egui::Rect::from_min_size(egui::pos2(rect.left() + 10.0, rect.center().y - 8.0), Vec2::splat(16.0)),
                            icon,
                            16.0,
                            if on { t.accent_ink } else { t.icon },
                        );
                        ui.painter().text(
                            egui::pos2(rect.left() + 36.0, rect.center().y),
                            egui::Align2::LEFT_CENTER,
                            tr(title),
                            theme::medium(theme::BODY),
                            if on { t.text } else { t.text_dim },
                        );
                        if r.clicked() {
                            app.state.settings_tab = tab;
                            if tab == SettingsTab::Ai && app.detected.is_none() {
                                app.detect_backends();
                            }
                        }
                    }
                });
                ui.add_space(theme::space::SM);
                ui.separator();
                ui.add_space(theme::space::LG);
                ui.vertical(|ui| {
                    // Exactly what's left of the window, with room for the scroll bar a long tab
                    // gets, so no tab can widen the window (#68).
                    let w = ui.available_width();
                    ui.set_width(w);
                    let content_w = w - ui.spacing().scroll.allocated_width();
                    let h = page_height(ctx);
                    // Not shrunk to the tab's content: that gave each tab its own width and put the
                    // scroll bar in a different place on every page.
                    let tab = app.state.settings_tab;
                    egui::ScrollArea::vertical().auto_shrink([false, false]).min_scrolled_height(h).max_height(h).max_width(w).show(ui, |ui| {
                        // The bar sits at the edge of the area, so the air between it and the
                        // settings is taken from the settings' side.
                        ui.set_max_width(content_w - 14.0);
                        match tab {
                            SettingsTab::General => general(app, ui),
                            SettingsTab::Engines => engines(app, ui),
                            SettingsTab::Ai => ai(app, ui),
                            SettingsTab::Builds => builds(app, ui),
                            SettingsTab::Plugins => plugins(app, ui),
                            SettingsTab::Safety => safety(app, ui),
                            SettingsTab::About => about(ui),
                        }
                    });
                });
            });
        });
    if !open || resp.should_close() {
        app.state.settings_open = false;
    }
}

fn row(ui: &mut Ui, title: &str, sub: &str, add: impl FnOnce(&mut Ui)) {
    let t = Tokens::get(ui.ctx());
    ui.horizontal(|ui| {
        ui.vertical(|ui| {
            ui.set_width(280.0);
            ui.label(RichText::new(tr(title)).font(theme::medium(theme::BODY)));
            if !sub.is_empty() {
                ui.add(
                    egui::Label::new(RichText::new(tr(sub)).color(t.text_faint).size(theme::CAPTION).line_height(Some(theme::leading(theme::CAPTION)))).wrap(),
                );
            }
        });
        add(ui);
    });
    ui.add_space(theme::space::MD);
}

/// A tab's row in Settings' list.
const TAB_H: f32 = 34.0;

/// How tall a tab's page is: as tall as the window allows, so a tab's settings aren't cut off at
/// the fold.
fn page_height(ctx: &egui::Context) -> f32 {
    (ctx.content_rect().height() - 220.0).clamp(280.0, 560.0)
}

/// The app's name and version, on every system (only macOS has an About in its menu bar). The
/// version can be selected, or copied with the button beside it, which the keyboard reaches too
/// (egui leaves selectable labels out of Tab), for a report.
fn about(ui: &mut Ui) {
    let t = Tokens::get(ui.ctx());
    ui.label(theme::heading("Craft Anything", theme::SECTION));
    ui.add_space(theme::space::XS);
    let version = format!("{} {}", tr("Version"), env!("CARGO_PKG_VERSION"));
    ui.horizontal(|ui| {
        ui.add(egui::Label::new(RichText::new(&version).color(t.text_dim).size(theme::BODY)).selectable(true));
        if icons::button(ui, "copy", theme::ControlSize::Small, false, "Copy the version").clicked() {
            ui.ctx().copy_text(version.clone());
        }
    });
}

fn general(app: &mut CraftApp, ui: &mut Ui) {
    row(ui, "Mode", "Simple: say what you want and it's handled for you. Advanced: every tool, the evidence and the logs.", |ui| {
        let sel = usize::from(app.settings.mode == Mode::Experienced);
        if let Some(i) = widgets::segmented(ui, &["Simple", "Advanced"], sel, theme::ControlSize::Medium) {
            app.set_mode(if i == 0 { Mode::Beginner } else { Mode::Experienced });
        }
    });
    let dir = app.settings.projects_dir();
    row(ui, "New projects go in", &dir.display().to_string(), |ui| {
        if widgets::secondary(ui, Some("folder-open"), "Change…").clicked() {
            app.pick(crate::Pick::ProjectsDir);
        }
    });
    row(ui, "Also list projects from", "Folders of projects, and projects started in a folder you picked. Removing one only takes it off the list.", |ui| {
        if widgets::secondary(ui, Some("plus"), "Add a folder…").clicked() {
            app.pick(crate::Pick::ExtraProjectsDir);
        }
    });
    let mut remove = None;
    for f in &app.settings.project_folders {
        ui.horizontal(|ui| {
            ui.add_space(theme::space::MD);
            crate::icons::show(ui, "folder", theme::icon::SM, Tokens::get(ui.ctx()).text_faint);
            ui.add(egui::Label::new(RichText::new(f.display().to_string()).font(theme::mono(theme::CODE))).truncate());
            if crate::icons::button(ui, "x", theme::ControlSize::Small, false, &trf("Stop listing {folder}", &[("folder", &f.display().to_string())])).clicked()
            {
                remove = Some(f.clone());
            }
        });
    }
    if let Some(f) = remove {
        app.remove_project_folder(&f);
    }
    row(ui, "When a folder you pick has files", "Whether they're the app to edit in place, or material the app starts from beside them.", |ui| {
        let current = crate::folders::EXISTING_CHOICES.iter().find(|(_, v)| *v == app.settings.existing_files).map_or("Ask each time", |(l, _)| *l);
        widgets::dropdown("existing-files").selected_text(tr(current)).show_ui(ui, |ui| {
            for (label, value) in crate::folders::EXISTING_CHOICES {
                if widgets::menu_item(ui, app.settings.existing_files == value, tr(label)).clicked() {
                    app.settings.existing_files = value;
                    app.settings_changed();
                }
            }
        });
    });
    row(ui, "Theme", "", |ui| {
        // System first: it's the default, and the other two are what it picks between.
        let sel =
            if theme::follows_system(&app.settings.theme) { 0 } else { 1 + usize::from(ThemeKind::from_id(&app.settings.theme) == ThemeKind::StudioLight) };
        if let Some(i) = widgets::segmented(ui, &["System", "Studio", "Studio Light"], sel, theme::ControlSize::Medium) {
            app.settings.theme = match i.checked_sub(1).and_then(|k| ThemeKind::ALL.get(k)) {
                Some(k) => k.id().into(),
                None => theme::SYSTEM.into(),
            };
            app.settings_changed();
        }
    });
    // A choice of one isn't a choice: the row appears once there's a second language.
    if LANGUAGES.len() > 1 {
        language(app, ui);
    }
    row(ui, "Show the welcome again", "Runs the first-run steps next time.", |ui| {
        if widgets::secondary(ui, Some("refresh-cw"), "Reset").clicked() {
            app.settings.first_run_done = false;
            app.state.first_run = crate::state::FirstRunStep::Welcome;
            app.state.settings_open = false;
            app.settings_changed();
        }
    });
}

fn language(app: &mut CraftApp, ui: &mut Ui) {
    row(ui, "Language", "", |ui| {
        let selected = (!app.settings.language.trim().is_empty()).then(|| crate::i18n::language_from_tag(&app.settings.language).unwrap_or("en"));
        let current = LANGUAGES.iter().find(|l| Some(l.code) == selected).map(|l| l.name).unwrap_or("System");
        widgets::dropdown("lang").selected_text(tr(current)).show_ui(ui, |ui| {
            if widgets::menu_item(ui, app.settings.language.trim().is_empty(), tr("System")).clicked() {
                app.settings.language.clear();
                crate::i18n::set_language("");
                app.settings_changed();
            }
            for l in LANGUAGES {
                if widgets::menu_item(ui, selected == Some(l.code), l.name).clicked() {
                    app.settings.language = l.code.into();
                    crate::i18n::set_language(l.code);
                    app.settings_changed();
                }
            }
        });
    });
}

/// A path setting; `name` is what a screen reader calls it.
fn path_field(ui: &mut Ui, value: &mut Option<PathBuf>, name: &str, hint: &str) -> bool {
    // While typing, the text as typed: the path kept is trimmed, and showing that instead would
    // drop the space typed in "C:\Program Files" before the next letter.
    let typed = ui.next_auto_id().with("typed");
    let shown = value.as_ref().map(|p| p.display().to_string()).unwrap_or_default();
    let mut s = ui.data(|d| d.get_temp::<String>(typed)).unwrap_or(shown);
    let r = ui.add(
        egui::TextEdit::singleline(&mut s)
            .hint_text(tr(hint))
            // Up to 240, leaving room for a Choose… button after it, so a row never widens the window (#68).
            .desired_width((ui.available_width() - 34.0).clamp(120.0, 240.0))
            .font(theme::mono(theme::CODE))
            .frame(widgets::text_field_frame_compact(&Tokens::get(ui.ctx()))),
    );
    widgets::named(r.clone(), &tr(name));
    if r.has_focus() {
        ui.data_mut(|d| d.insert_temp(typed, s.clone()));
    } else {
        ui.data_mut(|d| d.remove::<String>(typed));
    }
    if r.changed() {
        *value = if s.trim().is_empty() { None } else { Some(PathBuf::from(s.trim())) };
        return true;
    }
    false
}

fn engines(app: &mut CraftApp, ui: &mut Ui) {
    let t = Tokens::get(ui.ctx());
    row(
        ui,
        "Clean room / Deep",
        "Clean room never reads the original's code, files or page source: it works from what you can see. Deep reads them with every tool, and with Ghidra when it's installed. Every finding records which was used.",
        |ui| {
            let sel = usize::from(app.settings.deep_mode);
            if let Some(i) = widgets::segmented(ui, &["Clean room", "Deep"], sel, theme::ControlSize::Medium) {
                app.set_deep(i == 1);
            }
        },
    );
    ui.add_space(theme::space::MD);
    row(ui, "Ghidra", "Adds its decompiler to Deep mode. Craft Anything can download and set it up for you.", |ui| {
        crate::engine_ui::ghidra_status(app, ui);
    });
    row(
        ui,
        "Memory for Ghidra",
        "More memory makes Ghidra's first look at a big program faster; leave some for the rest of the computer. Used from the next time Ghidra starts.",
        |ui| crate::engine_ui::ghidra_memory_setting(app, ui),
    );
    ui.label(RichText::new(crate::engine_ui::ghidra_memory_guide()).color(t.text_faint).size(theme::CAPTION));
    ui.add_space(theme::space::SM);
    row(ui, "Ghidra folder", "Leave empty to use the one Craft Anything sets up.", |ui| {
        if path_field(ui, &mut app.settings.engines.ghidra.install_dir, "Ghidra folder", "managed") {
            app.settings_changed();
        }
        if icons::button(ui, "folder-open", theme::ControlSize::Small, false, "Choose…").clicked() {
            app.pick(Pick::GhidraDir);
        }
    });
    row(ui, "Java runtime", "Only for Ghidra. Empty = managed.", |ui| {
        if path_field(ui, &mut app.settings.engines.ghidra.java_home, "Java runtime", "managed") {
            app.settings_changed();
        }
    });
    ui.add_space(theme::space::MD);
    crate::engine_ui::settings_section(app, ui);
    ui.add_space(theme::space::MD);
    widgets::caption(ui, "Your own tool installs (optional)");
    ui.label(RichText::new(tr("Leave a path empty to use the managed install. We never need environment variables.")).color(t.text_faint).size(theme::CAPTION));
    ui.add_space(theme::space::SM);
    let mut changed = false;
    egui::Grid::new("tool-paths").num_columns(2).spacing([12.0, 6.0]).show(ui, |ui| {
        // Names and purposes come from the engine table; the override list matches it (tested).
        for (id, v) in app.settings.engines.tools.entries_mut() {
            let (name, needed_for) = craft_anything_engines::engine(id).map(|e| (e.name, e.needed_for)).unwrap_or((id, ""));
            ui.label(RichText::new(tr(name)).size(theme::CAPTION)).on_hover_text(format!("{}\n{id}", tr(needed_for)));
            changed |= path_field(ui, v, name, "managed");
            ui.end_row();
        }
    });
    if changed {
        app.settings_changed();
    }
}

/// Keep both selectors in place while their model-specific choices load or fail. Loading is
/// scheduled after drawing, and a failed lookup stays visible until explicitly retried.
fn model_options_loading(ui: &mut Ui, backend: &str) {
    ui.add_enabled_ui(false, |ui| {
        ui.horizontal_wrapped(|ui| {
            ui.label(tr("Context window"));
            widgets::dropdown(("model-context", backend)).selected_text(tr("Default")).show_ui(ui, |_| {});
        });
    });
}

/// Draw AI settings and model pickers, preserving drafts during contention and invalidating confirmed context changes.
/// Connection checks are delegated to the shared worker API.
fn ai(app: &mut CraftApp, ui: &mut Ui) {
    if app.agent_model_picker.as_ref().is_some_and(|p| app.hub.model_context(&p.context.backend.config.name).is_ok_and(|current| current != p.context)) {
        app.agent_model_picker = None;
    }
    let t = Tokens::get(ui.ctx());
    crate::engine_ui::assets_settings(app, ui);
    ui.add_space(theme::space::LG);
    widgets::caption(ui, "Your brains");
    ui.add_space(theme::space::XS);
    if app.backends.is_empty() {
        ui.label(RichText::new(tr("No AI is set up yet. The built-in engines still work; an AI adds plain-words answers.")).color(t.text_dim));
    }
    let (mut pick, mut builds, mut extracts, mut toggle, mut remove) = (None, None, None, None, None);
    let (mut choose_program, mut usual_program, mut use_program) = (None, None, None);
    let mut check_agent = None;
    let mut save_folder = None;
    // Taken out while the rows borrow the app, and put back after them.
    let mut folder_drafts = std::mem::take(&mut app.folder_inputs);
    let mut key_drafts = std::mem::take(&mut app.backend_key_inputs);
    let (mut pick_model, mut chosen_model, mut close_picker) = (None, None, false);
    let mut chosen_effort = None;
    let mut save_request_rate = None;
    let mut chosen_tools = None;
    let (mut pick_agent_model, mut chosen_agent_model, mut remote_program) = (None, None, None);
    let (mut tune_models, mut save_tuning) = (Vec::new(), None);
    // A saved API key being replaced or forgotten (the keychain work happens off this frame).
    let (mut save_key, mut forget_key) = (None, None);
    // The AI whose Remove was pressed once: asked again before its key is deleted.
    let confirm_id = egui::Id::new("remove-ai");
    let confirming: Option<String> = ui.data(|d| d.get_temp(confirm_id));
    for b in &app.backends {
        widgets::panel(&t).show(ui, |ui| {
            ui.set_width(ui.available_width());
            // One left edge for the whole card. The header: the name (shortened, full text on
            // hover), and at its end whether it's the one in use and the way to remove it.
            ui.horizontal(|ui| {
                icons::show(ui, "bot", theme::icon::LG, if b.enabled { t.accent } else { t.text_faint });
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if confirming.as_ref() != Some(&b.name)
                        && icons::button(ui, "trash-2", theme::ControlSize::Small, false, &trf("Remove {name}", &[("name", &b.name)])).clicked()
                    {
                        ui.data_mut(|d| d.insert_temp(confirm_id, b.name.clone()));
                    }
                    if app.state.backend.as_ref() == Some(&b.name) {
                        widgets::pill(ui, "In use", t.on_accent, t.accent);
                    }
                    ui.with_layout(egui::Layout::left_to_right(egui::Align::Center), |ui| {
                        ui.add(egui::Label::new(RichText::new(&b.name).font(theme::semibold(theme::BODY))).truncate());
                    });
                });
            });
            {
                {
                    let model_in_use = craft_anything_ai::model_in_use(b, app.backend_options.get(&b.name));
                    let live_catalog = b.kind == BackendKind::AgentCli && craft_anything_ai::cli_agents::adapter_for(&b.model).has_model_catalog();
                    // An API's model has its own line below; an agent app is named by its `model`.
                    let what = if model_in_use.is_some() && b.kind != BackendKind::AgentCli { String::new() } else { format!(" · {}", b.model) };
                    let ssh = b.ssh_host.as_ref().map(|h| format!(" · ssh {h}")).unwrap_or_default();
                    ui.add(egui::Label::new(RichText::new(format!("{:?}{what}{ssh}", b.kind)).color(t.text_faint).size(theme::CAPTION)).truncate());
                    // An agent app on this computer: which program runs it, for one installed
                    // where it isn't looked for (#53).
                    if b.kind == BackendKind::AgentCli && b.ssh_host.as_deref().is_none_or(|h| h.trim().is_empty()) {
                        let program = app.backend_options.get(&b.name).and_then(|o| o.command.as_ref());
                        ui.horizontal_wrapped(|ui| {
                            // The links at the caption's size, so the line reads as one.
                            ui.style_mut().override_font_id = Some(egui::FontId::proportional(theme::CAPTION));
                            let text = program.map_or_else(|| tr("Program: found automatically"), |p| trf("Program: {path}", &[("path", p)]));
                            ui.add(egui::Label::new(RichText::new(text).color(t.text_faint).size(theme::CAPTION)).truncate());
                            if widgets::link(ui, "Choose…").clicked() {
                                choose_program = Some(b.name.clone());
                            }
                            if program.is_some() && widgets::link(ui, "Find it automatically").clicked() {
                                usual_program = Some(b.name.clone());
                            }
                        });
                        // The saved program is gone (an update moved it): say what runs instead,
                        // and offer to save that one when it's a file of its own (#721).
                        if let Some(instead) = app.program_replaced.get(&b.name) {
                            ui.horizontal_wrapped(|ui| {
                                ui.style_mut().override_font_id = Some(egui::FontId::proportional(theme::CAPTION));
                                let text = trf("It isn't there any more, so {program} runs in its place.", &[("program", instead)]);
                                ui.add(
                                    egui::Label::new(
                                        RichText::new(text).color(t.warning).size(theme::CAPTION).line_height(Some(theme::leading(theme::CAPTION))),
                                    )
                                    .wrap(),
                                );
                                let path = std::path::Path::new(instead);
                                if path.parent().is_some_and(|p| !p.as_os_str().is_empty()) && widgets::link(ui, "Use it").clicked() {
                                    use_program = Some((b.name.clone(), instead.clone()));
                                }
                            });
                        }
                    }
                    // The folder it keeps its sign-in in (Claude Code's CLAUDE_CONFIG_DIR), typed
                    // so `~/` and hidden folders work, here or over SSH: another folder is another
                    // account. Saved on Enter or when the field is left; empty is its usual one.
                    if craft_anything_ai::folder_var(b).is_some() {
                        let saved = app.backend_options.get(&b.name).and_then(|o| o.config_dir.clone()).unwrap_or_default();
                        let id = egui::Id::new(("agent-folder", &b.name));
                        // What was typed stays until it's saved (or refused, so it can be fixed).
                        if folder_drafts.get(&b.name).is_some_and(|d| d.trim() == saved) && !ui.memory(|m| m.has_focus(id)) {
                            folder_drafts.remove(&b.name);
                        }
                        let mut text = folder_drafts.get(&b.name).cloned().unwrap_or_else(|| saved.clone());
                        ui.add_space(theme::space::XS);
                        let label = ui.label(RichText::new(tr("Claude Code folder (CLAUDE_CONFIG_DIR)")).size(theme::CAPTION).color(t.text_dim));
                        ui.label(
                            RichText::new(tr("Its own sign-in, settings and conversations: another folder is another account. Empty: its usual folder."))
                                .color(t.text_faint)
                                .size(theme::CAPTION),
                        );
                        let r = ui
                            .add(
                                egui::TextEdit::singleline(&mut text)
                                    .id(id)
                                    .hint_text("~/.claude")
                                    .desired_width(260.0)
                                    .frame(widgets::text_field_frame_compact(&t)),
                            )
                            .labelled_by(label.id);
                        if r.changed() {
                            folder_drafts.insert(b.name.clone(), text.clone());
                        }
                        if r.lost_focus() && text.trim() != saved {
                            save_folder = Some((b.name.clone(), text.trim().to_string()));
                        }
                        ui.add_space(theme::space::XS);
                    }
                    // How hard it thinks: every Claude level for Claude, OpenAI's for the OpenAI-style APIs.
                    let effort = craft_anything_ai::effort_in_use(b, app.backend_options.get(&b.name));
                    let effort_levels = craft_anything_ai::effort_levels(b);
                    if b.kind == BackendKind::AgentCli && b.ssh_host.as_deref().is_some_and(|h| !h.trim().is_empty()) {
                        let id = egui::Id::new(("remote-agent-program", &b.name));
                        let mut program = ui
                            .data_mut(|d| d.get_temp::<String>(id))
                            .unwrap_or_else(|| app.backend_options.get(&b.name).and_then(|o| o.command.clone()).unwrap_or_default());
                        ui.horizontal_wrapped(|ui| {
                            ui.label(tr("Remote program"));
                            ui.add(
                                egui::TextEdit::singleline(&mut program).hint_text(&b.model).desired_width(200.0).frame(widgets::text_field_frame_compact(&t)),
                            );
                            if widgets::link(ui, "Apply").clicked() {
                                remote_program = Some((b.name.clone(), program.clone()));
                            }
                        });
                        ui.data_mut(|d| d.insert_temp(id, program));
                    }
                    // Which model it uses, and the ones it can be set to (#123).
                    if live_catalog {
                        let options = app.backend_options.get(&b.name);
                        let current = options
                            .and_then(|o| o.cli_model.as_deref())
                            .map(str::to_string)
                            .unwrap_or_else(|| if b.model == "opencode" { tr("OpenCode default") } else { tr("Model default") });
                        ui.label(trf("Model: {model}", &[("model", &current)]));
                        let thinking = options.and_then(|o| o.cli_thinking).map(|d| d.title().to_string()).unwrap_or_else(|| tr("Model default"));
                        ui.label(format!("{}: {thinking}", tr("Thinking depth")));
                        if let Some(picker) = app.agent_model_picker.as_mut().filter(|p| p.context.backend.config.name == b.name) {
                            let previous = picker.model.clone();
                            let apply = crate::builders::agent_model_picker(ui, picker);
                            if picker.model != previous && craft_anything_ai::cli_agents::adapter_for(&b.model).model_catalog_depends_on_selection() {
                                pick_agent_model = Some(b.name.clone());
                            } else if apply {
                                chosen_agent_model = Some((b.name.clone(), picker.model.clone(), picker.thinking));
                            }
                            ui.horizontal(|ui| {
                                if widgets::link(ui, "Reload models").clicked() {
                                    pick_agent_model = Some(b.name.clone());
                                }
                                if widgets::link(ui, "Cancel").clicked() {
                                    close_picker = true;
                                }
                            });
                        } else if widgets::link(ui, "Change…").clicked() {
                            pick_agent_model = Some(b.name.clone());
                        }
                    } else if let Some(current) = model_in_use {
                        ui.horizontal_wrapped(|ui| {
                            ui.add(
                                egui::Label::new(RichText::new(trf("Model: {model}", &[("model", &current)])).color(t.text_faint).size(theme::CAPTION))
                                    .truncate(),
                            );
                            match app.model_picker.as_ref().filter(|(n, _)| *n == b.name) {
                                None => {
                                    if widgets::caption_link(ui, "Change…").clicked() {
                                        pick_model = Some(b.name.clone());
                                    }
                                }
                                Some((_, None)) => {
                                    ui.add(crate::widgets::Spinner::new().size(theme::CAPTION).color(t.accent));
                                    ui.label(RichText::new(tr("Asking for its models…")).color(t.text_faint).size(theme::CAPTION));
                                }
                                Some((_, Some(models))) => {
                                    // Claude Code or Codex with no choice saved passes no model, so it
                                    // uses the one its own settings name: `default` is what's shown. An
                                    // API backend's model is its saved `config.model`.
                                    let own_default =
                                        b.kind == BackendKind::AgentCli && craft_anything_ai::cli_agents::adapter_for(&b.model).offers_own_default();
                                    let picked = app.backend_options.get(&b.name).and_then(|o| o.cli_model.clone()).unwrap_or_else(|| {
                                        if own_default { craft_anything_ai::anthropic::CLAUDE_CODE_DEFAULT.to_string() } else { b.model.clone() }
                                    });
                                    let shown = |m: &str| {
                                        if m == craft_anything_ai::anthropic::CLAUDE_CODE_DEFAULT {
                                            tr("Default (its own settings)").to_string()
                                        } else {
                                            m.to_string()
                                        }
                                    };
                                    widgets::dropdown(("model", &b.name)).selected_text(shown(&picked)).show_ui(ui, |ui| {
                                        for m in models {
                                            if widgets::menu_item(ui, *m == picked, shown(m)).clicked() && *m != picked {
                                                chosen_model = Some((b.name.clone(), m.clone()));
                                            }
                                        }
                                    });
                                    if widgets::caption_link(ui, "Cancel").clicked() {
                                        close_picker = true;
                                    }
                                }
                            }
                            if !effort_levels.is_empty() {
                                ui.label(RichText::new(tr("Effort:")).color(t.text_faint).size(theme::CAPTION));
                                let shown = effort.map_or("default", craft_anything_ai::Effort::as_str);
                                widgets::dropdown(("effort", &b.name)).selected_text(shown).show_ui(ui, |ui| {
                                    // Claude always has a level; an OpenAI-style API is sent none until one is chosen.
                                    let unset = (effort_levels != craft_anything_ai::Effort::ALL).then_some(None);
                                    for e in unset.into_iter().chain(effort_levels.iter().map(|&e| Some(e))) {
                                        if widgets::menu_item(ui, e == effort, e.map_or("default", craft_anything_ai::Effort::as_str)).clicked() && e != effort
                                        {
                                            chosen_effort = Some((b.name.clone(), e));
                                        }
                                    }
                                });
                            }
                        });
                    }
                    if craft_anything_ai::model_options::supported(b) {
                        if let Some(draft) = app.model_tuning.get_mut(&b.name) {
                            if let Some(choices) = &draft.choices {
                                let mut changed = false;
                                ui.add_enabled_ui(!draft.saving, |ui| {
                                    ui.horizontal_wrapped(|ui| {
                                        ui.label(tr("Context window"));
                                        let label = draft
                                            .context
                                            .and_then(|value| choices.context.iter().find(|choice| choice.value == value))
                                            .map(|choice| tr(&choice.label))
                                            .unwrap_or_else(|| tr("Default"));
                                        widgets::dropdown(("model-context", &b.name)).selected_text(label).show_ui(ui, |ui| {
                                            changed |= widgets::menu_choice(ui, &mut draft.context, None, tr("Default"));
                                            for choice in &choices.context {
                                                changed |= widgets::menu_choice(ui, &mut draft.context, Some(choice.value), tr(&choice.label));
                                            }
                                        });
                                    });
                                });
                                if changed {
                                    save_tuning = Some(draft.params());
                                }
                            } else {
                                model_options_loading(ui, &b.name);
                                if let Some(error) = &draft.error {
                                    ui.add(egui::Label::new(RichText::new(error).color(t.danger)).wrap());
                                    if icons::button(ui, "refresh-cw", theme::ControlSize::Small, false, "Load model options").clicked() {
                                        tune_models.push(b.name.clone());
                                    }
                                } else {
                                    ui.spinner();
                                }
                            }
                        } else {
                            model_options_loading(ui, &b.name);
                            tune_models.push(b.name.clone());
                        }
                    }
                }
            }
            if b.kind != BackendKind::AgentCli {
                let list = app.backend_options.get(&b.name).and_then(|o| o.tools).unwrap_or_default();
                // What Auto works out to for this AI, beside the choice (R5.4).
                let auto = if craft_anything_ai::runs_here(b) {
                    tr("Auto: building tools only below a 64k window, every tool from there.")
                } else {
                    tr("Auto: every tool, as this AI is a paid service.")
                };
                // One change at a time: jobs finish in any order, so a second change's refresh could land before the
                // first's and Settings would show the older choice. The row waits for the saving job, as the rate does.
                let job_id = ui.make_persistent_id(("tool-list-job", &b.name));
                let pending_job = ui.data(|data| data.get_temp::<u64>(job_id));
                let pending = app.jobs.running().any(|job| Some(job.id) == pending_job);
                if !pending && pending_job.is_some() {
                    ui.data_mut(|data| data.remove::<u64>(job_id));
                }
                ui.add_enabled_ui(!pending, |ui| {
                    ui.horizontal_wrapped(|ui| {
                        let label = ui.label(tr("Builder's tools"));
                        widgets::dropdown(("tool-list", &b.name))
                            .selected_text(list.as_str())
                            .show_ui(ui, |ui| {
                                for l in craft_anything_ai::ToolList::ALL {
                                    if widgets::menu_item(ui, l == list, l.as_str()).clicked() && l != list {
                                        chosen_tools = Some((b.name.clone(), l, job_id));
                                    }
                                }
                            })
                            .response
                            .labelled_by(label.id);
                        ui.label(RichText::new(auto).color(t.text_faint).size(theme::CAPTION));
                    });
                });
                let saved = app.backend_options.get(&b.name).and_then(|o| o.requests_per_minute).unwrap_or_default();
                let id = ui.make_persistent_id(("request-rate", &b.name));
                let saved_text = if saved == 0 { String::new() } else { saved.to_string() };
                let (mut text, pending_job) = ui.data(|data| data.get_temp::<(String, Option<u64>)>(id)).unwrap_or((saved_text.clone(), None));
                let pending = app.jobs.running().any(|job| Some(job.id) == pending_job);
                ui.add_enabled_ui(!pending, |ui| {
                    ui.horizontal_wrapped(|ui| {
                        let label = ui.label(tr("Requests per minute"));
                        ui.add(egui::TextEdit::singleline(&mut text).hint_text("0").desired_width(90.0))
                            .labelled_by(label.id)
                            .on_hover_text(tr("Set to 0 for no limit."));
                        let rate = if text.trim().is_empty() { Some(0) } else { text.trim().parse::<u32>().ok() };
                        let changed_rate = rate.filter(|rate| *rate != saved);
                        let apply = widgets::primary_icon(ui, None, "Apply", changed_rate.is_some());
                        if apply.clicked()
                            && let Some(rate) = changed_rate
                        {
                            save_request_rate = Some((b.name.clone(), rate, id));
                        }
                        if rate.is_none() {
                            ui.label(RichText::new(tr("Enter a whole number, or 0 for no limit.")).color(t.danger));
                        }
                    });
                });
                if pending || text != saved_text {
                    ui.data_mut(|data| data.insert_temp(id, (text, pending_job)));
                } else {
                    ui.data_mut(|data| data.remove::<(String, Option<u64>)>(id));
                }
            }
            // A key saved in the app silently wins over the environment's, so it can be replaced
            // here without removing the AI (#1358). Every API backend can need one; agent apps
            // sign in their own way. The field stays even when a key is saved, so a replacement
            // doesn't need Forget first.
            if b.kind != BackendKind::AgentCli {
                // A save or forget for this backend is still running: no later action may be
                // submitted until it finishes, so workers can't overtake one another. Other
                // backends keep their controls.
                ui.add_space(theme::space::SM);
                ui.add_enabled_ui(!app.backend_key_pending.contains(&b.name), |ui| {
                    let saved = app.backend_keys.as_ref().and_then(|keys| keys.get(&b.name).copied()) == Some(true);
                    let key = key_drafts.entry(b.name.clone()).or_default();
                    ui.horizontal_wrapped(|ui| {
                        let r = ui.add(
                            egui::TextEdit::singleline(key)
                                .password(true)
                                .hint_text(tr("API key (stored in your system keychain)"))
                                .desired_width(260.0)
                                .frame(widgets::text_field_frame_compact(&t)),
                        );
                        widgets::named(r, &trf("{name} key", &[("name", &b.name)]));
                        let k = key.trim().to_string();
                        if widgets::primary_icon(ui, None, "Save", !k.is_empty()).clicked() && !k.is_empty() {
                            // Keep the draft until the save succeeds; a failure must not lose it.
                            save_key = Some((b.name.clone(), k));
                        }
                        if saved {
                            widgets::pill(ui, "Key saved", t.success, t.raised);
                            if widgets::ghost(ui, None, "Forget the key").clicked() {
                                forget_key = Some(b.name.clone());
                            }
                        }
                    });
                });
            }
            ui.add_space(theme::space::MD);
            ui.horizontal_wrapped(|ui| {
                if app.state.backend.as_ref() != Some(&b.name) && widgets::secondary(ui, None, "Use").clicked() {
                    pick = Some(b.name.clone());
                }
                // Who builds: the chosen one says so (click to go back to automatic).
                if app.builder.as_ref() == Some(&b.name) {
                    if widgets::secondary(ui, Some("hammer"), "Builds").on_hover_text(tr("Builds use this one. Click to choose automatically again.")).clicked()
                    {
                        builds = Some(None);
                    }
                } else if b.enabled && widgets::secondary(ui, None, "Build with this").clicked() {
                    builds = Some(Some(b.name.clone()));
                }
                // Who takes a game's files out in Deep mode (an API backend, e.g. Kimi K3).
                if b.kind != BackendKind::AgentCli {
                    if app.extractor.as_ref() == Some(&b.name) {
                        if widgets::secondary(ui, Some("package"), "Extracts")
                            .on_hover_text(tr("In Deep mode this one takes a game's own files out after the study. Click to stop."))
                            .clicked()
                        {
                            extracts = Some(None);
                        }
                    } else if b.enabled && widgets::secondary(ui, None, "Extract with this").clicked() {
                        extracts = Some(Some(b.name.clone()));
                    }
                }
                if b.kind == BackendKind::AgentCli && b.ssh_host.as_deref().is_none_or(|host| host.trim().is_empty()) {
                    let pending = app.agent_connections.get(&b.name) == Some(&None);
                    ui.add_enabled_ui(!pending, |ui| {
                        if widgets::secondary(ui, None, if pending { "Checking" } else { "Check" }).clicked() {
                            check_agent = Some(b.name.clone());
                        }
                    });
                }
                // Off: kept with its key, but nothing uses it.
                let (label, tip) = if b.enabled {
                    ("Turn off", "Nothing uses it until it's on again; it keeps its key.")
                } else {
                    ("Turn on", "Builds and answers may use it again.")
                };
                if widgets::secondary(ui, None, label).on_hover_text(tr(tip)).clicked() {
                    toggle = Some((b.name.clone(), !b.enabled));
                }
                if confirming.as_ref() == Some(&b.name) {
                    // A key save or forget for this backend is still running: the save could
                    // write the key back after the backend and its key are gone.
                    ui.add_enabled_ui(!app.backend_key_pending.contains(&b.name), |ui| {
                        if widgets::danger(ui, Some("trash-2"), "Remove it and its key").clicked() {
                            remove = Some(b.name.clone());
                        }
                    });
                    if widgets::ghost(ui, None, "Keep it").clicked() {
                        ui.data_mut(|d| d.remove::<String>(confirm_id));
                    }
                }
            });
            if !b.enabled {
                ui.label(RichText::new(tr("Off")).color(t.text_faint).size(theme::CAPTION));
            }
            if let Some(Some(result)) = app.agent_connections.get(&b.name) {
                let (message, color) = match result {
                    Ok(message) => (message, t.text_dim),
                    Err(message) => (message, t.danger),
                };
                ui.label(RichText::new(message).color(color).size(theme::CAPTION));
            }
        });
        ui.add_space(theme::space::SM);
    }
    key_drafts.retain(|name, _| app.backends.iter().any(|backend| &backend.name == name));
    app.backend_key_inputs = key_drafts;
    app.folder_inputs = folder_drafts;
    if let Some((name, rate, id)) = save_request_rate {
        let job = app.set_requests_per_minute(&name, rate);
        let text = if rate == 0 { String::new() } else { rate.to_string() };
        ui.data_mut(|data| data.insert_temp(id, (text, Some(job))));
    }
    if let Some((name, list, id)) = chosen_tools {
        let job = app.set_tool_list(&name, list);
        ui.data_mut(|data| data.insert_temp(id, job));
    }
    if let Some((name, key)) = save_key {
        app.save_backend_key(name, key);
    }
    if let Some(name) = forget_key {
        app.remove_backend_key(name);
    }
    if let Some((name, on)) = toggle {
        app.set_backend_enabled(&name, on);
    }
    if let Some(name) = check_agent {
        app.check_agent_connection(name);
    }
    if let Some(name) = choose_program {
        app.picking_program_for = Some(name);
        app.pick(Pick::AgentProgram);
    }
    if let Some(name) = usual_program {
        app.set_agent_program(&name, None);
    }
    if let Some((name, program)) = use_program {
        app.set_agent_program(&name, Some(program));
    }
    if let Some((name, folder)) = save_folder {
        app.set_agent_folder(&name, Some(folder));
    }
    if let Some(name) = pick_model {
        app.pick_model(&name);
    }
    if let Some((name, program)) = remote_program {
        app.set_agent_program(&name, Some(program));
    }
    if let Some(name) = pick_agent_model {
        app.reload_agent_model(&name);
    }
    if let Some((name, model, thinking)) = chosen_agent_model {
        app.set_agent_model(&name, model, thinking);
    }
    if let Some((name, model)) = chosen_model {
        app.set_model(&name, &model);
    }
    if let Some((name, effort)) = chosen_effort {
        app.set_effort(&name, effort);
    }
    if close_picker {
        app.model_picker = None;
        app.agent_model_picker = None;
    }
    for name in tune_models {
        app.pick_model_options(&name);
    }
    if let Some(params) = save_tuning {
        app.set_model_options(params);
    }
    if let Some(name) = remove {
        ui.data_mut(|d| d.remove::<String>(confirm_id));
        app.remove_backend(&name);
    }
    if let Some(p) = pick {
        app.choose_backend(Some(p));
    }
    if let Some(b) = builds {
        app.set_role(craft_anything_ai::Role::Builder, b);
    }
    if let Some(e) = extracts {
        app.set_role(craft_anything_ai::Role::Extractor, e);
    }
    ui.add_space(theme::space::SM);
    let detected = app.detected.clone().unwrap_or_default();
    widgets::caption(ui, "Found on this computer");
    let tab = ui
        .horizontal_wrapped(|ui| {
            let tab = crate::detect_tabs::buttons(ui, &mut app.state.detect_tab, &detected);
            if app.detecting {
                ui.add(crate::widgets::Spinner::new().color(t.accent));
            } else if widgets::secondary(ui, Some("radar"), "Detect").clicked() {
                app.detect_backends();
            }
            tab
        })
        .inner;
    ui.add_space(theme::space::XS);
    if detected.is_empty() && !app.detecting {
        ui.label(
            RichText::new(tr("Nothing found yet. We look for API keys, local model servers (Ollama, LM Studio) and agent apps you already have. One on another port or computer can be added by its address below."))
                .color(t.text_faint),
        );
    }
    let shown = detected.iter().filter(|d| crate::detect_tabs::tab_of(d) == tab).count();
    if shown == 0 && !detected.is_empty() && !app.detecting {
        ui.label(RichText::new(tr("Nothing of this kind was found.")).color(t.text_faint));
    }
    for d in detected.into_iter().filter(|d| crate::detect_tabs::tab_of(d) == tab) {
        let configured = app.backends.iter().any(|b| b.name == d.suggestion.name);
        widgets::panel(&t).show(ui, |ui| {
            ui.set_width(ui.available_width());
            // The buttons first, in the top right corner, then the name and reason wrapped in the
            // width they leave: a label in a horizontal row doesn't wrap, so a long reason ran on
            // under the Add button.
            // Child uis (not scopes, which would move the cursor down after each), then one advance.
            let top = ui.available_rect_before_wrap();
            let mut right_ui = ui.new_child(egui::UiBuilder::new().max_rect(top).layout(egui::Layout::right_to_left(egui::Align::Min)));
            if configured {
                widgets::pill(&mut right_ui, "Added", t.success, t.raised);
                // Claude Code once more, to give it another account's folder.
                if craft_anything_ai::folder_var(&d.suggestion).is_some()
                    && widgets::link(&mut right_ui, "Add another").on_hover_text(tr("Add it again, to choose another account's folder for it.")).clicked()
                {
                    let taken = |n: &String| app.backends.iter().any(|b| &b.name == n);
                    if let Some(name) = (2..100).map(|i| format!("{} {i}", d.suggestion.name)).find(|n| !taken(n)) {
                        app.add_backend(craft_anything_ai::BackendConfig { name, ..d.suggestion.clone() }, None);
                    }
                }
            } else if !d.needs_key && widgets::primary(&mut right_ui, "Add").clicked() {
                app.add_backend(d.suggestion.clone(), None);
            }
            let right = right_ui.min_rect();
            // Nothing on the right (a key to paste below instead): the text takes the whole width.
            let text_right = if right.width() > 0.0 { right.left() - ui.spacing().item_spacing.x } else { top.right() };
            let mut left_ui =
                ui.new_child(egui::UiBuilder::new().max_rect(top.with_max_x(text_right.max(top.left()))).layout(egui::Layout::top_down(egui::Align::Min)));
            left_ui.add(egui::Label::new(RichText::new(&d.suggestion.name).font(theme::medium(theme::SMALL))).wrap());
            left_ui.add(
                egui::Label::new(RichText::new(&d.reason).color(t.text_faint).size(theme::CAPTION).line_height(Some(theme::leading(theme::CAPTION)))).wrap(),
            );
            ui.advance_cursor_after_rect(left_ui.min_rect().union(right));
            // A subscription key is worth pasting even when the environment already has one, so a
            // different key can be used without restarting the app.
            let pastable = d.needs_key || d.suggestion.base_url.as_deref().is_some_and(craft_anything_ai::openai::is_opencode_go);
            if pastable && !configured {
                ui.horizontal(|ui| {
                    let key = app.key_inputs.entry(d.suggestion.name.clone()).or_default();
                    ui.add(
                        egui::TextEdit::singleline(key)
                            .password(true)
                            .hint_text(tr("API key (stored in your system keychain)"))
                            .desired_width(260.0)
                            .frame(widgets::text_field_frame_compact(&Tokens::get(ui.ctx()))),
                    );
                    let k = key.trim().to_string();
                    if widgets::primary_icon(ui, None, "Add", !k.is_empty()).clicked() && !k.is_empty() {
                        // The key goes to the keychain; the app keeps no copy of it.
                        app.key_inputs.remove(&d.suggestion.name);
                        app.add_backend(d.suggestion.clone(), Some(k));
                    }
                });
            }
        });
        ui.add_space(theme::space::SM);
    }
    server_form(app, ui);
}

/// An AI that "Look for AI" can't find: a server on another computer or a hosted service, added by
/// its address (#54, #98). The server is asked for its models before it's added.
/// The first run's AI step shows it too.
pub(crate) fn server_form(app: &mut CraftApp, ui: &mut Ui) {
    let t = Tokens::get(ui.ctx());
    ui.add_space(theme::space::LG);
    widgets::caption(ui, "Add an AI by its address");
    ui.label(RichText::new(tr("Ollama or LM Studio on another computer, or any service that speaks the OpenAI API.")).color(t.text_faint).size(theme::CAPTION));
    ui.add_space(theme::space::XS);
    let form = &mut app.server_form;
    // `width` is the text's; the compact frame adds 8 points a side and its border.
    const FRAME: f32 = 18.0;
    let field = |ui: &mut Ui, text: &mut String, hint: &str, width: f32, name: &str, secret: bool| {
        let r = ui.add(egui::TextEdit::singleline(text).password(secret).hint_text(tr(hint)).desired_width(width).frame(widgets::text_field_frame_compact(&t)));
        widgets::named(r, &tr(name));
    };
    ui.horizontal(|ui| {
        field(ui, &mut form.name, "name, e.g. Studio box", 150.0, "AI name", false);
        let w = ui.available_width() - FRAME;
        field(ui, &mut form.address, "address, e.g. http://192.168.1.20:11434/v1", w, "Server address", false);
    });
    // A model picked from one server's list goes once the address is another, before Add can send
    // it there (a model the server hasn't got may start a download). One typed by hand stays.
    if let Some((from, picked)) = form.picked.take() {
        let still_picked = form.model.trim() == picked;
        if from == form.address.trim() && still_picked {
            form.picked = Some((from, picked));
        } else if still_picked {
            form.model.clear();
        }
    }
    // One server at a time: pressing Add again while it's asked would only start a second wait.
    let ready = !form.name.trim().is_empty() && !form.address.trim().is_empty() && form.connecting.is_none();
    let mut add = false;
    ui.horizontal(|ui| {
        field(ui, &mut form.model, "model (optional)", 150.0, "Model", false);
        // Room for the Add button after it.
        let w = (ui.available_width() - FRAME - 90.0).max(120.0);
        field(ui, &mut form.key, "API key, if it needs one", w, "API key", true);
        add = widgets::primary_icon(ui, Some("plus"), "Add", ready).clicked() && ready;
    });
    // The server listed several chat models and didn't say which is loaded: the person chooses
    // (#1434). A list for another address is out of date and isn't offered.
    if let Some((address, models)) = form.choices.as_ref().filter(|(a, _)| *a == form.address.trim()) {
        ui.horizontal(|ui| {
            ui.label(RichText::new(tr("This server lists several chat models. Choose one:")).color(t.text_faint).size(theme::CAPTION));
            let shown = if form.model.trim().is_empty() { tr("Choose a model") } else { form.model.clone() };
            let r = widgets::dropdown("server-model-choice").selected_text(shown).show_ui(ui, |ui| {
                for m in models {
                    if widgets::menu_item(ui, form.model.trim() == m, m.as_str()).clicked() {
                        form.model = m.clone();
                        form.picked = Some((address.clone(), m.clone()));
                    }
                }
            });
            // Named for screen readers (and found by it in tests); the value is the model shown.
            widgets::named(r.response, "Choose a model");
        });
    }
    // A server still starting or loading a model may take up to a minute to answer: say it's being
    // asked and for how long, so the form doesn't look like nothing happened (#1429).
    let mut stop = None;
    if let Some((job, name, started)) = &form.connecting {
        let seconds = started.elapsed().as_secs().to_string();
        ui.horizontal(|ui| {
            ui.add(widgets::Spinner::new().size(theme::CAPTION).color(t.accent));
            ui.label(
                RichText::new(trf("Connecting to {name}… {seconds} s", &[("name", name), ("seconds", &seconds)])).color(t.text_faint).size(theme::CAPTION),
            );
            // Waiting out the minute isn't the only way: what was typed stays to fix and add again.
            if widgets::secondary(ui, Some("x"), "Stop").clicked() {
                stop = Some(*job);
            }
        });
        // The seconds count on even when the spinner is out of view.
        ui.ctx().request_repaint_after(std::time::Duration::from_secs(1));
    }
    if let Some(job) = stop {
        app.jobs.cancel(job);
    }
    if add {
        app.add_server();
    }
}

/// A number field; true when it changed.
fn number<T: egui::emath::Numeric>(ui: &mut Ui, value: &mut T, range: std::ops::RangeInclusive<T>, prefix: &str, suffix: &str) -> bool {
    ui.add(egui::DragValue::new(value).range(range).prefix(prefix).suffix(suffix)).changed()
}

fn builds(app: &mut CraftApp, ui: &mut Ui) {
    let mut changed = false;
    let b = &mut app.settings.build;
    row(ui, "Good enough", "How alike a rebuild must look to the original before refining stops.", |ui| {
        changed |= number(ui, &mut b.good_enough, 50.0..=100.0, "", " %");
    });
    row(ui, "Refining rounds", "At most this many rounds of fixes after the first build.", |ui| {
        changed |= number(ui, &mut b.max_rounds, 0..=20, "", "");
    });
    row(ui, "Website pages", "Other pages of a website studied and compared besides the one you gave.", |ui| {
        changed |= number(ui, &mut b.max_pages, 0..=craft_anything_ai::rebuild::kinds::website::MAX_ROUTES, "", "");
    });
    row(ui, "Match my pictures", "Apps and games: keep refining until they look this alike to your pictures. Off, the likeness is only reported.", |ui| {
        let mut on = b.picture_good_enough.is_some();
        if widgets::toggle(ui, &mut on, "Use as a goal").changed() {
            b.picture_good_enough = on.then_some(80.0);
            changed = true;
        }
        if let Some(v) = &mut b.picture_good_enough {
            changed |= number(ui, v, 30.0..=100.0, "", " %");
        }
    });
    row(ui, "Builds at once", "How many projects may build at the same time. More wait their turn.", |ui| {
        changed |= number(ui, &mut b.parallel, 1..=8, "", "");
    });
    ui.add_space(theme::space::SM);
    if let Some(bytes) = app.pinned_copy_bytes {
        row(ui, "Pinned builds", "Size", |ui| {
            ui.label(craft_anything_engine::evidence::human_bytes(bytes));
        });
    }
    widgets::caption(ui, "Generated art (games)");
    let a = &mut app.settings.assets;
    row(
        ui,
        "Budget for a new project",
        "What a new game may spend on generated pictures and 3D models before you're asked. Each project keeps its own.",
        |ui| {
            changed |= number(ui, &mut a.budget_usd, 0.0..=10_000.0, "$", "");
        },
    );
    let table = craft_anything_engines::assets::Catalogue::built_in();
    row(ui, "Picture model", "What pictures and textures are made with.", |ui| {
        let current = table.image_model(a.image_model.as_deref()).or_else(|_| table.image_model(None)).map(|m| m.name.clone()).unwrap_or_default();
        widgets::dropdown("image-model").selected_text(current).show_ui(ui, |ui| {
            for m in table.images() {
                let price = crate::usage::asset_cost_words(m.cost(craft_anything_engines::assets::Shape::Square));
                let label = format!("{} · {} {}", m.name, price, tr("a picture"));
                let on = a.image_model.as_deref() == Some(m.id.as_str());
                if widgets::menu_item(ui, on, label).on_hover_text(&m.licence).clicked() && !on {
                    a.image_model = Some(m.id.clone());
                    changed = true;
                }
            }
        });
    });
    if changed {
        app.settings_changed();
    }
}

/// Plugins: external MCP servers (`[[plugins.mcp]]`) and WASM components (`[[plugins.wasm]]`),
/// listed, turned on or off, added and removed.
fn plugins(app: &mut CraftApp, ui: &mut Ui) {
    let t = Tokens::get(ui.ctx());
    ui.add(
        egui::Label::new(
            RichText::new(tr(
                "Plugins add tools Craft Anything offers as its own: MCP servers (a command or an address; read their tools after adding) or WebAssembly components (a .wasm file, which can't touch your files or the network).",
            ))
            .color(t.text_faint)
            .size(theme::CAPTION),
        )
        .wrap(),
    );
    ui.add_space(theme::space::MD);
    let plugins = app.settings.plugins.mcp.clone();
    let wasm = app.settings.plugins.wasm.clone();
    if plugins.is_empty() && wasm.is_empty() {
        ui.label(RichText::new(tr("No plugins yet.")).color(t.text_faint));
    }
    let mut changed = false;
    for (i, p) in plugins.iter().enumerate() {
        widgets::panel(&t).show(ui, |ui| {
            ui.horizontal(|ui| {
                ui.vertical(|ui| {
                    ui.set_width(300.0);
                    ui.label(RichText::new(p.display_name()).font(theme::medium(theme::SMALL)));
                    ui.label(RichText::new(p.place()).font(theme::mono(theme::CAPTION)).color(t.text_faint));
                    if let Some(problem) = p.problem() {
                        ui.label(RichText::new(problem).color(t.warning).size(theme::CAPTION));
                    }
                    if !p.consent {
                        ui.label(RichText::new(tr("Runs its tools without asking")).color(t.warning).size(theme::CAPTION));
                    }
                });
                if widgets::toggle(ui, &mut app.settings.plugins.mcp[i].enabled, "On").changed() {
                    changed = true;
                }
                if icons::button(ui, "refresh-cw", theme::ControlSize::Small, false, "Read its tools").clicked() {
                    app.act("plugins_refresh", serde_json::json!({"id": p.id}), tr("Read the plugin's tools"));
                }
                if icons::button(ui, "trash-2", theme::ControlSize::Small, false, "Remove").clicked() {
                    app.act("plugins_remove", serde_json::json!({"id": p.id}), tr("Remove a plugin"));
                }
            });
        });
        ui.add_space(theme::space::SM);
    }
    for (i, p) in wasm.iter().enumerate() {
        widgets::panel(&t).show(ui, |ui| {
            ui.horizontal(|ui| {
                ui.vertical(|ui| {
                    ui.set_width(300.0);
                    ui.label(RichText::new(p.display_name()).font(theme::medium(theme::SMALL)));
                    ui.label(RichText::new(p.path.display().to_string()).font(theme::mono(theme::CAPTION)).color(t.text_faint));
                    if let Some(problem) = p.problem() {
                        ui.label(RichText::new(problem).color(t.warning).size(theme::CAPTION));
                    }
                });
                if widgets::toggle(ui, &mut app.settings.plugins.wasm[i].enabled, "On").changed() {
                    changed = true;
                }
                if icons::button(ui, "trash-2", theme::ControlSize::Small, false, "Remove").clicked() {
                    app.act("plugins_remove", serde_json::json!({"id": p.id}), tr("Remove a plugin"));
                }
            });
        });
        ui.add_space(theme::space::SM);
    }
    if changed {
        app.settings_changed();
    }
    if !plugins.is_empty() || !wasm.is_empty() {
        ui.label(RichText::new(tr("Turning a plugin on or off takes effect from the next start.")).color(t.text_faint).size(theme::CAPTION));
    }
    ui.add_space(theme::space::LG);
    widgets::caption(ui, "Add a plugin");
    ui.add_space(theme::space::XS);
    let form = &mut app.state.plugin_form;
    ui.horizontal(|ui| {
        let id =
            ui.add(egui::TextEdit::singleline(&mut form.id).hint_text(tr("id, e.g. docs")).desired_width(120.0).frame(widgets::text_field_frame_compact(&t)));
        widgets::named(id, "Plugin id");
        let place = ui.add(
            egui::TextEdit::singleline(&mut form.place)
                .hint_text(tr("command, https:// address or .wasm file"))
                .desired_width(200.0)
                .frame(widgets::text_field_frame_compact(&t)),
        );
        widgets::named(place, "Plugin command or address");
        let args = ui.add(
            egui::TextEdit::singleline(&mut form.args).hint_text(tr("arguments (optional)")).desired_width(160.0).frame(widgets::text_field_frame_compact(&t)),
        );
        widgets::named(args, "Plugin arguments");
    });
    ui.add_space(theme::space::SM);
    let params = app.state.plugin_form.params();
    if widgets::primary_icon(ui, Some("plus"), "Add", params.is_some()).clicked()
        && let Some(params) = params
    {
        app.act("plugins_add", params, tr("Add a plugin"));
        app.state.plugin_form = Default::default();
    }
    ui.label(RichText::new(tr("Plugins added here always ask before each call.")).color(t.text_faint).size(theme::CAPTION));
}

fn safety(app: &mut CraftApp, ui: &mut Ui) {
    let t = Tokens::get(ui.ctx());
    row(
        ui,
        "Ask before running anything",
        "Tools that run code from a target or change a phone always ask first, unless you allowed them for a project.",
        |ui| {
            if widgets::toggle(ui, &mut app.settings.ask_before_running, "Ask first").changed() {
                app.settings_changed();
            }
        },
    );
    if !app.settings.ask_before_running {
        ui.horizontal(|ui| {
            icons::show(ui, "triangle-alert", theme::icon::MD, t.warning);
            ui.label(RichText::new(tr("Tools may now run code from what you open without asking. Turn this back on unless you're sure.")).color(t.warning));
        });
    }
    row(
        ui,
        "Auto mode",
        "Builds keep going without stopping to ask you before an action. The builder still never deletes outside its project, changes system settings or publishes anything, and stays within each project's asset budget.",
        |ui| {
            if widgets::toggle(ui, &mut app.settings.auto_mode, "Keep going without asking").changed() {
                app.settings_changed();
            }
        },
    );
    row(
        ui,
        "The builder's questions",
        "When the builder needs a choice made: show it to you and wait, or let it decide and say what it chose. Each project can choose for itself under its prompt box.",
        |ui| {
            use craft_anything_engine::workspace::Questions;
            let sel = usize::from(app.settings.questions() == Questions::Decide);
            if let Some(i) = widgets::segmented(ui, &["Ask me", "Decide yourself"], sel, theme::ControlSize::Medium) {
                app.settings.questions = Some(if i == 0 { Questions::Ask } else { Questions::Decide });
                app.settings_changed();
            }
        },
    );
    row(
        ui,
        "Study apps by running them",
        "When you rebuild an app or game, start it once to picture its window and compare the rebuild with it. It runs that program.",
        |ui| {
            if widgets::toggle(ui, &mut app.settings.runtime_study, "Run to study").changed() {
                app.settings_changed();
            }
        },
    );
    if let Some(p) = app.view.project.clone() {
        ui.add_space(theme::space::SM);
        widgets::caption(ui, &format!("{} {}", tr("Allowed in"), p));
        if app.view.project_allows.is_empty() {
            ui.label(RichText::new(tr("Nothing allowed yet.")).color(t.text_faint));
        }
        for tool in app.view.project_allows.clone() {
            ui.label(RichText::new(tool).font(theme::mono(theme::CODE)));
        }
    }
}

/// What to tell the person at startup when their settings couldn't be loaded: `why` names the
/// file, and the rest says what that means until it's fixed. `None` when they loaded. English for
/// now: the wording needs a native speaker's check before it joins the catalogs.
pub(crate) fn not_loaded_warning(settings: &craft_anything_engine::Settings) -> Option<String> {
    let why = settings.not_loaded.as_deref()?;
    Some(format!("{why}. Until that's fixed, Craft Anything uses its default settings and won't save over it."))
}

/// What a settings save that failed says: a line in the person's language first, then what the
/// engine said and what to do about it. The engine's message is English; its remediation is the
/// part that tells the person how to get their settings saved again, so it is never left out.
pub(crate) fn settings_not_saved(e: &craft_anything_engine::evidence::ToolError) -> String {
    let mut said = format!("{} {}", tr("Couldn't save your settings."), e.message);
    if let Some(fix) = e.remediation.as_deref().filter(|f| !f.trim().is_empty()) {
        said.push(' ');
        said.push_str(fix);
    }
    said
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Done, builders::AgentModelPicker, demo, jobs::Finished};
    use craft_anything_ai::{BackendConfig, BackendKind};

    /// Settings that couldn't be loaded say so where the person is looking, with the file's name.
    #[test]
    fn settings_that_didnt_load_are_said_at_startup() {
        let loaded = craft_anything_engine::Settings::default();
        assert_eq!(not_loaded_warning(&loaded), None);
        let why = r"C:\Users\me\AppData\Roaming\Craft Anything\config.toml couldn't be read (access denied)";
        let standing_in = craft_anything_engine::Settings { not_loaded: Some(why.into()), ..Default::default() };
        let said = not_loaded_warning(&standing_in).unwrap();
        assert!(said.contains("config.toml") && said.contains("access denied"), "{said}");
        assert!(said.contains("won't save over it"), "{said}");
    }

    /// A refused save was shown as the engine's English message alone, without what to do: it
    /// leads with a translated line now, and keeps the remediation.
    #[test]
    fn a_refused_settings_save_says_so_with_what_to_do() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");
        std::fs::create_dir(&path).unwrap(); // Something in config.toml's place that can't be read.
        let standing_in = craft_anything_engine::Settings::load_from(&path);
        let e = standing_in.save_to(&path).unwrap_err();
        let fix = e.remediation.clone().unwrap();
        let said = settings_not_saved(&e);
        assert!(said.starts_with(&tr("Couldn't save your settings.")), "{said}");
        assert!(said.contains(&e.message) && said.ends_with(&fix), "{said}");
        // No remediation: the line and the message, with nothing dangling after them.
        let bare = craft_anything_engine::evidence::ToolError::invalid("disk full");
        assert_eq!(settings_not_saved(&bare), format!("{} disk full", tr("Couldn't save your settings.")));
        // Korean, the language this was found in, has the line.
        let ko = crate::i18n::LANGUAGES.iter().find(|l| l.code == "ko").unwrap();
        assert!(ko.source.contains("Couldn't save your settings.\t설정을 저장하지 못했습니다."));
    }

    #[test]
    /// A non-Codex local agent gets the same Check action and result presentation.
    fn agent_connection_check_and_its_result_are_visible_in_settings() {
        let mut app = demo::app(demo::engine(craft_anything_engine::Mode::Experienced, true));
        app.backends = vec![BackendConfig {
            name: "Agent test".into(),
            kind: BackendKind::AgentCli,
            model: "plain-test-agent".into(),
            base_url: None,
            ssh_host: None,
            enabled: true,
        }];
        app.agent_connections.insert("Agent test".into(), Some(Err("Agent test is installed but is not signed in.".into())));
        let context = egui::Context::default();
        theme::install_fonts(&context);
        let mut output = context.run_ui(Default::default(), |ui| ai(&mut app, ui));
        output.textures_delta.clear();
        let text = output
            .shapes
            .iter()
            .filter_map(|shape| match &shape.shape {
                egui::epaint::Shape::Text(text) => Some(text.galley.text().to_string()),
                _ => None,
            })
            .collect::<Vec<_>>()
            .join("\n");
        assert!(text.contains("Check"));
        assert!(text.contains("Agent test is installed but is not signed in."));
    }

    /// #1358: an API backend with a saved key can have it replaced or forgotten in place, while an
    /// agent app (which signs in its own way) has no key row.
    #[test]
    fn a_saved_backend_key_can_be_replaced_or_forgotten() {
        use std::collections::BTreeMap;

        let mut app = demo::app(demo::engine(craft_anything_engine::Mode::Experienced, true));
        app.backends = vec![BackendConfig {
            name: "OpenAI test".into(),
            kind: BackendKind::Openai,
            model: "gpt-test".into(),
            base_url: None,
            ssh_host: None,
            enabled: true,
        }];
        let render = |app: &mut CraftApp| {
            let context = egui::Context::default();
            theme::install_fonts(&context);
            let mut output = context.run_ui(Default::default(), |ui| ai(app, ui));
            output.textures_delta.clear();
            output
                .shapes
                .iter()
                .filter_map(|shape| match &shape.shape {
                    egui::epaint::Shape::Text(text) => Some(text.galley.text().to_string()),
                    _ => None,
                })
                .collect::<Vec<_>>()
                .join("\n")
        };

        app.backend_keys = Some(BTreeMap::from([("OpenAI test".to_string(), true)]));
        let text = render(&mut app);
        assert!(text.contains("Key saved"), "a saved key says so: {text}");
        assert!(text.contains("Forget the key"), "and can be forgotten without removing the AI: {text}");
        assert!(text.contains("API key (stored in your system keychain)"), "and replaced without forgetting it first: {text}");

        app.backend_keys = Some(BTreeMap::from([("OpenAI test".to_string(), false)]));
        let text = render(&mut app);
        assert!(text.contains("API key (stored in your system keychain)"), "a backend without one asks for it: {text}");
        assert!(text.contains("Save"));
        assert!(!text.contains("Key saved"));

        // Every API backend gets the row, including a local OpenAI-compatible server.
        app.backends = vec![BackendConfig {
            name: "Local test".into(),
            kind: BackendKind::OpenaiCompatible,
            model: "local-test".into(),
            base_url: Some("http://127.0.0.1:9/v1".into()),
            ssh_host: None,
            enabled: true,
        }];
        app.backend_keys = Some(BTreeMap::from([("Local test".to_string(), false)]));
        let text = render(&mut app);
        assert!(text.contains("API key (stored in your system keychain)"), "a local server can take a key too: {text}");

        app.backends = vec![BackendConfig {
            name: "Agent test".into(),
            kind: BackendKind::AgentCli,
            model: "plain-test-agent".into(),
            base_url: None,
            ssh_host: None,
            enabled: true,
        }];
        app.backend_keys = Some(BTreeMap::from([("Agent test".to_string(), false)]));
        let text = render(&mut app);
        assert!(!text.contains("Key saved"));
        assert!(!text.contains("Forget the key"));
        assert!(!text.contains("API key (stored in your system keychain)"), "an agent app has no key row: {text}");
    }

    /// A key check that started before a save or a newer check must not put the old state back.
    #[test]
    fn a_stale_backend_key_check_does_not_replace_a_newer_one() {
        use std::collections::BTreeMap;

        let mut app = demo::app(demo::engine(craft_anything_engine::Mode::Experienced, true));
        app.backend_keys_request = 2;
        app.backend_keys = Some(BTreeMap::from([("OpenAI".to_string(), true)]));
        app.handle_done(Finished { job: 1, result: Ok(Done::BackendKeys { request: 1, keys: BTreeMap::from([("OpenAI".to_string(), false)]) }) });
        assert_eq!(app.backend_keys.as_ref().and_then(|keys| keys.get("OpenAI")), Some(&true), "the newer state stays");
        app.handle_done(Finished { job: 2, result: Ok(Done::BackendKeys { request: 2, keys: BTreeMap::from([("OpenAI".to_string(), false)]) }) });
        assert_eq!(app.backend_keys.as_ref().and_then(|keys| keys.get("OpenAI")), Some(&false), "the current check applies");
    }

    /// A failed key save keeps what was typed; a success clears it only when it is still the same
    /// text, so an edit made while the save was running is not lost.
    #[test]
    fn a_key_draft_survives_a_failed_save_and_a_newer_edit() {
        use std::collections::BTreeMap;

        let mut app = demo::app(demo::engine(craft_anything_engine::Mode::Experienced, true));
        app.backend_keys_request = 1;
        app.backend_key_inputs.insert("OpenAI".into(), "sk-draft".into());
        app.handle_done(Finished {
            job: 1,
            result: Ok(Done::BackendKeyResult {
                request: 1,
                name: "OpenAI".into(),
                keys: BTreeMap::from([("OpenAI".to_string(), false)]),
                error: Some("keychain is locked".into()),
                saved_key: None,
                forgot: false,
            }),
        });
        assert_eq!(app.backend_key_inputs.get("OpenAI").map(String::as_str), Some("sk-draft"), "a failure keeps the draft");
        assert_eq!(app.backend_keys.as_ref().and_then(|keys| keys.get("OpenAI")), Some(&false), "a failure still refreshes the shown status");

        app.backend_key_inputs.insert("OpenAI".into(), "sk-edited".into());
        app.handle_done(Finished {
            job: 2,
            result: Ok(Done::BackendKeyResult {
                request: 1,
                name: "OpenAI".into(),
                keys: BTreeMap::from([("OpenAI".to_string(), true)]),
                error: None,
                saved_key: Some("sk-saved".into()),
                forgot: false,
            }),
        });
        assert_eq!(app.backend_key_inputs.get("OpenAI").map(String::as_str), Some("sk-edited"), "a newer edit stays");

        app.backend_key_inputs.insert("OpenAI".into(), "sk-saved ".into());
        app.handle_done(Finished {
            job: 3,
            result: Ok(Done::BackendKeyResult {
                request: 1,
                name: "OpenAI".into(),
                keys: BTreeMap::from([("OpenAI".to_string(), true)]),
                error: None,
                saved_key: Some("sk-saved".into()),
                forgot: false,
            }),
        });
        assert!(!app.backend_key_inputs.contains_key("OpenAI"), "the saved draft is cleared");

        app.backend_key_inputs.insert("OpenAI".into(), "sk-next".into());
        app.handle_done(Finished {
            job: 4,
            result: Ok(Done::BackendKeyResult {
                request: 1,
                name: "OpenAI".into(),
                keys: BTreeMap::from([("OpenAI".to_string(), true)]),
                error: Some("keychain is locked".into()),
                saved_key: None,
                forgot: true,
            }),
        });
        assert_eq!(app.backend_key_inputs.get("OpenAI").map(String::as_str), Some("sk-next"), "a failed forget keeps the draft");

        app.handle_done(Finished {
            job: 5,
            result: Ok(Done::BackendKeyResult {
                request: 1,
                name: "OpenAI".into(),
                keys: BTreeMap::from([("OpenAI".to_string(), false)]),
                error: None,
                saved_key: None,
                forgot: true,
            }),
        });
        assert!(!app.backend_key_inputs.contains_key("OpenAI"), "a successful forget clears it");
    }

    /// A failed worker frees only its own Check button; unrelated pending/results remain intact.
    #[test]
    fn failed_agent_check_workers_can_be_retried_without_clearing_other_checks() {
        let mut app = demo::app(demo::engine(craft_anything_engine::Mode::Experienced, true));
        app.agent_connections.insert("Failed agent".into(), None);
        app.agent_connections.insert("Running agent".into(), None);
        app.agent_connections.insert("Finished agent".into(), Some(Ok("already checked".into())));
        app.agent_connection_jobs.insert(41, "Failed agent".into());
        app.agent_connection_jobs.insert(42, "Running agent".into());
        // Jobs::spawn emits this same Err completion if the thread cannot start (or panics).
        app.handle_done(Finished { job: 41, result: Err("Couldn't start a worker thread: test failure".into()) });
        assert!(matches!(app.agent_connections.get("Failed agent"), Some(Some(Err(error))) if error.contains("Couldn't start a worker thread")));
        assert!(!app.agent_connection_jobs.contains_key(&41));
        assert_eq!(app.agent_connections.get("Running agent"), Some(&None));
        assert_eq!(app.agent_connection_jobs.get(&42).map(String::as_str), Some("Running agent"));
        assert_eq!(app.agent_connections.get("Finished agent"), Some(&Some(Ok("already checked".into()))));
        app.handle_done(Finished { job: 99, result: Err("unrelated job failed".into()) });
        assert_eq!(app.agent_connections.get("Running agent"), Some(&None));
        app.handle_done(Finished { job: 42, result: Ok(Done::AgentConnection("Running agent".into(), Ok("checked".into()))) });
        assert!(app.agent_connection_jobs.is_empty());
        assert_eq!(app.agent_connections.get("Running agent"), Some(&Some(Ok("checked".into()))));
        // The backend is intentionally absent: no real agent or credential access is needed.
        app.check_agent_connection("Failed agent".into());
        assert_eq!(app.agent_connections.get("Failed agent"), Some(&None));
        assert_eq!(app.agent_connection_jobs.len(), 1, "retry was scheduled");
        let count = app.jobs.list.len();
        app.check_agent_connection("Failed agent".into());
        assert_eq!(app.jobs.list.len(), count, "a pending retry cannot start a duplicate");
    }

    #[test]
    fn identical_model_names_are_shown_once_in_the_shared_picker() {
        use craft_anything_ai::{AgentModel, BackendEntry, ModelContext};
        use egui_kittest::{Harness, kittest::Queryable};
        let context = ModelContext {
            backend: BackendEntry {
                config: BackendConfig {
                    name: "Test agent".into(),
                    kind: BackendKind::AgentCli,
                    model: "copilot".into(),
                    base_url: None,
                    ssh_host: None,
                    enabled: true,
                },
                options: Default::default(),
            },
            directory: None,
        };
        let mut picker = AgentModelPicker {
            request: 1,
            context,
            model: None,
            thinking: None,
            result: Some(Ok(vec![
                AgentModel { id: "auto".into(), name: "auto".into(), thinking: Vec::new() },
                AgentModel { id: "provider/model".into(), name: "Friendly name".into(), thinking: Vec::new() },
            ])),
        };
        let mut harness = Harness::builder().build_ui(move |ui| {
            theme::install_fonts(ui.ctx());
            crate::builders::agent_model_picker(ui, &mut picker);
        });
        harness.get_by_label("Model").click();
        harness.run_steps(3);
        assert!(harness.query_by_label("auto").is_some());
        assert!(harness.query_by_label("auto · auto").is_none());
        assert!(harness.query_by_label("Friendly name · provider/model").is_some());
    }

    #[test]
    fn shared_picker_warnings_name_the_configured_agent() {
        use craft_anything_ai::{BackendEntry, ModelContext};
        use egui_kittest::{Harness, kittest::Queryable};
        for (agent, name) in [("copilot", "GitHub Copilot"), ("opencode", "OpenCode")] {
            let context = ModelContext {
                backend: BackendEntry {
                    config: BackendConfig {
                        name: "Test".into(),
                        kind: BackendKind::AgentCli,
                        model: agent.into(),
                        base_url: None,
                        ssh_host: None,
                        enabled: true,
                    },
                    options: Default::default(),
                },
                directory: None,
            };
            let mut picker = AgentModelPicker { request: 1, context, model: Some("missing-model".into()), thinking: None, result: Some(Ok(Vec::new())) };
            let harness = Harness::builder().build_ui(move |ui| {
                theme::install_fonts(ui.ctx());
                crate::builders::agent_model_picker(ui, &mut picker);
            });
            harness.get_by_label(format!("No chat models are visible in {name}. Check its setup, then reload.").as_str());
            harness.get_by_label(format!("The saved model is no longer visible. Choose another model or the {name} default.").as_str());
            if agent == "copilot" {
                assert!(harness.query_by_label("The saved model is no longer visible. Choose another model or the OpenCode default.").is_none());
            }
        }
    }

    /// Busy frames retain matching catalogue results, ignore older requests and defer context invalidation until readable.
    #[test]
    fn a_busy_session_keeps_the_picker_and_its_matching_result() {
        let mut app = demo::app(demo::engine(craft_anything_engine::Mode::Experienced, true));
        app.hub
            .add_backend(
                BackendConfig { name: "OpenCode".into(), kind: BackendKind::AgentCli, model: "opencode".into(), base_url: None, ssh_host: None, enabled: true },
                None,
            )
            .unwrap();
        app.backends = app.hub.backends();
        app.backend_options = app.hub.options();
        let context = app.hub.model_context("OpenCode").unwrap();
        app.agent_model_picker = Some(AgentModelPicker { request: 2, context: context.clone(), result: None, model: None, thinking: None });
        let ctx = egui::Context::default();
        theme::install_fonts(&ctx);
        let draw = |app: &mut CraftApp| {
            let mut output = ctx.run_ui(Default::default(), |ui| ai(app, ui));
            output.textures_delta.clear();
        };
        let engine = app.engine.clone();
        let session = engine.session();
        app.handle_done(Finished { job: 0, result: Ok(Done::AgentModelChoices { request: 1, context: context.clone(), result: Ok(Vec::new()) }) });
        assert!(app.agent_model_picker.as_ref().unwrap().result.is_none(), "an older reload must still be ignored");
        app.handle_done(Finished { job: 0, result: Ok(Done::AgentModelChoices { request: 2, context, result: Ok(Vec::new()) }) });
        draw(&mut app);
        assert!(app.agent_model_picker.as_ref().unwrap().result.as_ref().unwrap().is_ok(), "a busy frame must keep the completed catalogue");
        drop(session);
        draw(&mut app);
        assert!(app.agent_model_picker.is_some(), "unlocking an unchanged session keeps the draft");
        app.hub.set_enabled("OpenCode", false).unwrap();
        draw(&mut app);
        assert!(app.agent_model_picker.is_none(), "a confirmed settings change still invalidates the picker");
    }

    /// An AI that finishes connecting is chosen, unless another choice (here "No AI for now") was
    /// made while it connected.
    #[test]
    fn a_late_add_does_not_undo_a_choice_made_meanwhile() {
        let mut app = demo::app(demo::engine(craft_anything_engine::Mode::Beginner, false));
        let added = |choices| Ok(Done::BackendAdded { name: "Home server".into(), result: Ok(()), choices, address: String::new() });
        let started = app.backend_choices;
        app.choose_backend(None);
        app.handle_done(Finished { job: 0, result: added(started) });
        assert_eq!(app.state.backend, None, "the late add must not override No AI for now");
        app.handle_done(Finished { job: 0, result: added(app.backend_choices) });
        assert_eq!(app.state.backend.as_deref(), Some("Home server"), "with no choice meanwhile, the added AI is chosen");
        // Two adds at once: the first one's selection isn't a choice, so the second is selected too.
        let started = app.backend_choices;
        let other = |choices| Ok(Done::BackendAdded { name: "Laptop".into(), result: Ok(()), choices, address: String::new() });
        app.handle_done(Finished { job: 0, result: added(started) });
        app.handle_done(Finished { job: 0, result: other(started) });
        assert_eq!(app.state.backend.as_deref(), Some("Laptop"), "the add that finished last is selected");
    }

    /// A chat started while an AI connects is a choice too: the add doesn't switch AI under it.
    #[test]
    fn a_late_add_does_not_stop_a_chat_started_meanwhile() {
        let mut app = demo::app(demo::engine(craft_anything_engine::Mode::Experienced, true));
        // A closed local port: the chat starts, and its request fails without leaving this computer.
        let config = BackendConfig {
            name: "Box".into(),
            kind: BackendKind::OpenaiCompatible,
            model: "m".into(),
            base_url: Some("http://127.0.0.1:9/v1".into()),
            ssh_host: None,
            enabled: true,
        };
        app.hub.add_backend(config, None).unwrap();
        app.state.backend = Some("Box".into());
        let started = app.backend_choices;
        app.send("Hi");
        assert!(app.conversation.is_some(), "the chat started");
        app.handle_done(Finished {
            job: 0,
            result: Ok(Done::BackendAdded { name: "Laptop".into(), result: Ok(()), choices: started, address: String::new() }),
        });
        assert_eq!(app.state.backend.as_deref(), Some("Box"), "the chat's AI stays chosen");
        assert!(app.conversation.is_some(), "the chat keeps going");
        // A follow-up in a chat that started before the add counts the same way.
        let started = app.backend_choices;
        app.send("And then?");
        app.handle_done(Finished {
            job: 0,
            result: Ok(Done::BackendAdded { name: "Laptop".into(), result: Ok(()), choices: started, address: String::new() }),
        });
        assert_eq!(app.state.backend.as_deref(), Some("Box"), "a follow-up keeps its AI");
        assert!(app.conversation.is_some(), "the follow-up's answer keeps going");
    }
}

#[cfg(test)]
mod model_options_tests {
    use super::*;
    use craft_anything_ai::config::{CliContext, CliReasoningEffort};
    use craft_anything_ai::model_options::{ModelOption, ModelTuning};

    fn app_with_backend() -> CraftApp {
        let mut app = crate::demo::app(crate::demo::engine(craft_anything_engine::Mode::Beginner, false));
        app.engine.session().settings.ai = Default::default();
        app.backends = vec![craft_anything_ai::BackendConfig {
            name: "Copilot test".into(),
            kind: BackendKind::AgentCli,
            model: "copilot".into(),
            base_url: None,
            ssh_host: None,
            enabled: true,
        }];
        app
    }

    fn render(app: &mut CraftApp) -> String {
        let context = egui::Context::default();
        crate::theme::install_fonts(&context);
        let mut output = context.run_ui(egui::RawInput::default(), |ui| ai(app, ui));
        output.textures_delta.clear();
        output
            .shapes
            .iter()
            .filter_map(|shape| match &shape.shape {
                egui::epaint::Shape::Text(text) => Some(text.galley.text().to_string()),
                _ => None,
            })
            .collect::<Vec<_>>()
            .join("\n")
    }

    /// The text the address form draws.
    fn draw(app: &mut CraftApp) -> String {
        let context = egui::Context::default();
        crate::theme::install_fonts(&context);
        let mut output = context.run_ui(egui::RawInput::default(), |ui| server_form(app, ui));
        output.textures_delta.clear();
        output
            .shapes
            .iter()
            .filter_map(|shape| match &shape.shape {
                egui::epaint::Shape::Text(text) => Some(text.galley.text().to_string()),
                _ => None,
            })
            .collect::<Vec<_>>()
            .join("\n")
    }

    /// #1434: a server that lists several chat models and doesn't say which is loaded offers them
    /// to choose from; the choice fills the model field, and a list for another address isn't shown.
    #[test]
    fn a_server_with_several_models_offers_them_to_choose_from() {
        const A: &str = "http://127.0.0.1:0/v1";
        let mut app = app_with_backend();
        app.server_form.name = "Npu".into();
        app.server_form.address = A.into();
        let refused = || {
            craft_anything_engine::evidence::ToolError::invalid("lists 2 chat models")
                .with_details(serde_json::json!({ "choose_model": ["deepseek-r1:8b", "qwen3:8b"] }))
        };
        // The form's request, as `add_server` records it, and its answer for the address asked.
        let ask = |app: &mut CraftApp, job| app.server_form.connecting = Some((job, "Npu".into(), std::time::Instant::now()));
        let answer = |job, address: &str, result| crate::jobs::Finished {
            job,
            result: Ok(crate::Done::BackendAdded { name: "Npu".into(), result, choices: 0, address: address.into() }),
        };
        ask(&mut app, 7);
        app.handle_done(answer(7, A, Err(refused())));
        let choices = app.server_form.choices.clone().expect("offered");
        assert_eq!(choices, (A.to_string(), vec!["deepseek-r1:8b".to_string(), "qwen3:8b".to_string()]));
        let text = draw(&mut app);
        assert!(text.contains("This server lists several chat models. Choose one:") && text.contains("Choose a model"), "{text}");
        app.server_form.model = "qwen3:8b".into();
        assert!(draw(&mut app).contains("qwen3:8b"), "the choice shows");
        app.server_form.address = "http://127.0.0.1:0/other/v1".into();
        assert!(!draw(&mut app).contains("Choose one"), "another address's list isn't offered");

        // Review of #1436: the address is changed to B while A is still answering. A's list is
        // A's, so it isn't offered for B (where picking from it would ask B for A's model).
        app.server_form = crate::ServerForm { name: "Npu".into(), address: A.into(), ..Default::default() };
        ask(&mut app, 8);
        app.server_form.address = "http://127.0.0.1:0/b/v1".into();
        app.handle_done(answer(8, A, Err(refused())));
        assert!(app.server_form.choices.is_none(), "a list for the address that was asked, not the one shown");
        assert!(!draw(&mut app).contains("Choose one"));

        // An answer to an older Add, replaced by a newer one, is stale and offers nothing.
        app.server_form = crate::ServerForm { name: "Npu".into(), address: A.into(), ..Default::default() };
        ask(&mut app, 10);
        app.handle_done(answer(9, A, Err(refused())));
        assert!(app.server_form.choices.is_none(), "a stale request's list isn't offered");
        assert!(app.server_form.connecting.is_some(), "the newer request is still waited for");

        // Another failure of the form's request clears a list it offered before.
        app.handle_done(answer(10, A, Err(refused())));
        assert!(app.server_form.choices.is_some());
        ask(&mut app, 11);
        app.handle_done(answer(11, A, Err(craft_anything_engine::evidence::ToolError::invalid("down"))));
        assert!(app.server_form.choices.is_none(), "an unrelated failure leaves no old list");
    }

    /// #1429: while a server is asked for its models the form says so, with the seconds waited, and
    /// Add waits; the line goes when the job ends, whether the server was added or not.
    #[test]
    fn a_server_being_added_shows_that_it_is_connecting() {
        let mut app = app_with_backend();
        assert!(!draw(&mut app).contains("Connecting to"), "nothing to say before Add");
        // Port zero refuses at once: the job never reaches a network.
        app.server_form.name = "Home box".into();
        app.server_form.address = "http://127.0.0.1:0/v1".into();
        app.add_server();
        let job = app.server_form.connecting.as_ref().map(|(job, ..)| *job).expect("connecting");
        let text = draw(&mut app);
        assert!(text.contains("Connecting to Home box… 0 s"), "{text}");
        // A different job finishing leaves it alone; this one's failure clears it.
        app.handle_done(crate::jobs::Finished { job: job.wrapping_add(1000), result: Err("other".into()) });
        assert!(app.server_form.connecting.is_some());
        let refused = craft_anything_engine::evidence::ToolError::invalid("refused");
        app.handle_done(crate::jobs::Finished {
            job,
            result: Ok(crate::Done::BackendAdded {
                name: "Home box".into(),
                result: Err(refused),
                choices: app.backend_choices,
                address: "http://127.0.0.1:0/v1".into(),
            }),
        });
        assert!(app.server_form.connecting.is_none());
        assert!(!draw(&mut app).contains("Connecting to"));
        assert_eq!(app.server_form.address, "http://127.0.0.1:0/v1", "what was typed is kept to fix it");
    }

    /// #1429: the wait can be stopped from the form; a stop is said plainly, not as an error, and
    /// what was typed stays.
    #[test]
    fn adding_a_server_can_be_stopped_from_the_form() {
        let mut app = app_with_backend();
        app.server_form.name = "Home box".into();
        app.server_form.address = "http://127.0.0.1:0/v1".into();
        app.add_server();
        let job = app.server_form.connecting.as_ref().map(|(job, ..)| *job).expect("connecting");
        let context = egui::Context::default();
        crate::theme::install_fonts(&context);
        let mut output = context.run_ui(egui::RawInput::default(), |ui| server_form(&mut app, ui));
        output.textures_delta.clear();
        let shown = output.shapes.iter().any(|shape| matches!(&shape.shape, egui::epaint::Shape::Text(t) if t.galley.text() == "Stop"));
        assert!(shown, "a Stop button beside the wait");
        app.handle_done(crate::jobs::Finished {
            job,
            result: Ok(crate::Done::BackendAdded {
                name: "Home box".into(),
                result: Err(craft_anything_engine::evidence::ToolError::cancelled()),
                choices: app.backend_choices,
                address: "http://127.0.0.1:0/v1".into(),
            }),
        });
        assert!(app.server_form.connecting.is_none());
        let (text, error) = app.state.toasts.last().cloned().expect("a note");
        assert_eq!((text.as_str(), error), ("Stopped adding Home box.", false));
        assert_eq!(app.server_form.name, "Home box", "what was typed is kept to add it again");
    }

    #[test]
    fn model_controls_use_shared_thinking_and_separate_context_without_loading_a_cli() {
        let mut app = app_with_backend();
        let mut draft = crate::ModelTuningDraft {
            backend: "Copilot test".into(),
            cli_model: Some("test-model".into()),
            choices: None,
            context: Some(CliContext::Long),
            saving: false,
            error: None,
        };
        draft.set_choices(ModelTuning {
            model: "test-model".into(),
            context: vec![ModelOption { value: CliContext::Long, label: "1M".into(), description: None }],
            reasoning: vec![ModelOption { value: CliReasoningEffort::High, label: "high".into(), description: None }],
        });
        app.model_tuning.insert(draft.backend.clone(), draft);
        let text = render(&mut app);
        assert!(text.contains("Context window") && text.contains("Thinking depth") && text.contains("1M") && !text.contains("Reasoning"), "{text}");
        assert_eq!(app.model_tuning["Copilot test"].params().context, Some(CliContext::Long));
    }

    #[test]
    fn model_controls_are_always_visible_and_discovery_is_not_requeued_each_frame() {
        let mut app = app_with_backend();
        let text = render(&mut app);
        assert!(text.contains("Context window") && text.contains("Thinking depth"), "{text}");
        let jobs = app.jobs.list.len();
        assert!(app.model_tuning.contains_key("Copilot test"));
        render(&mut app);
        assert_eq!(app.jobs.list.len(), jobs);
        app.model_tuning.get_mut("Copilot test").unwrap().error = Some("Discovery unavailable".into());
        let text = render(&mut app);
        assert!(text.contains("Context window") && text.contains("Thinking depth") && text.contains("Discovery unavailable"), "{text}");
        assert_eq!(app.jobs.list.len(), jobs, "failed discovery waits for an explicit retry");
    }

    #[test]
    fn model_option_changes_save_without_closing_the_controls() {
        let mut app = app_with_backend();
        app.hub.add_backend(app.backends[0].clone(), None).unwrap();
        app.backend_options = app.hub.options();
        let state = crate::ModelTuningDraft {
            backend: "Copilot test".into(),
            cli_model: None,
            choices: None,
            context: Some(CliContext::Long),
            saving: false,
            error: None,
        };
        let params = state.params();
        app.model_tuning.insert(state.backend.clone(), state);
        app.set_model_options(params);
        assert!(app.model_tuning["Copilot test"].saving);
        let started = std::time::Instant::now();
        while app.model_tuning["Copilot test"].saving && started.elapsed() < std::time::Duration::from_secs(3) {
            for finished in app.jobs.poll() {
                app.handle_done(finished);
            }
            std::thread::park_timeout(std::time::Duration::from_millis(5));
        }
        assert!(!app.model_tuning["Copilot test"].saving);
        assert_eq!(app.hub.options()["Copilot test"].cli_context, Some(CliContext::Long));
        assert_eq!(app.backend_options["Copilot test"].cli_thinking, None);
    }

    #[test]
    fn choosing_a_model_automatically_repopulates_its_options() {
        let mut app = app_with_backend();
        app.hub
            .add_backend_with(
                craft_anything_ai::BackendEntry {
                    config: app.backends[0].clone(),
                    options: craft_anything_ai::BackendOptions {
                        cli_model: Some("old-model".into()),
                        command: Some("craft-anything-no-such-test-agent".into()),
                        ..Default::default()
                    },
                },
                None,
            )
            .unwrap();
        app.backend_options = app.hub.options();
        app.model_tuning.insert(
            "Copilot test".into(),
            crate::ModelTuningDraft {
                backend: "Copilot test".into(),
                cli_model: Some("old-model".into()),
                choices: None,
                context: None,
                saving: false,
                error: Some("Old choices".into()),
            },
        );
        let mut options = app.backend_options["Copilot test"].clone();
        options.cli_model = Some("new-model".into());
        app.hub.add_backend_with(craft_anything_ai::BackendEntry { config: app.backends[0].clone(), options }, None).unwrap();
        let result = crate::backends(&app.hub);
        app.handle_done(crate::jobs::Finished { job: 0, result: Ok(result) });
        assert_eq!(app.backend_options["Copilot test"].cli_model.as_deref(), Some("new-model"));
        assert!(!app.model_tuning.contains_key("Copilot test"));
        let text = render(&mut app);
        assert!(text.contains("new-model") && text.contains("Context window") && text.contains("Thinking depth"), "{text}");
        assert_eq!(app.model_tuning["Copilot test"].cli_model.as_deref(), Some("new-model"));
        assert_eq!(app.model_tuning["Copilot test"].error, None);
    }
}
