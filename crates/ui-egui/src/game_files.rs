//! Simple-mode Game files: pick a game's folder, see the archives it packs its art in, look
//! inside one, and save pictures as PNG files, after the person confirms they may unpack the game.
//! Everything runs through the engine's `game_*` tools; this screen only shows and asks.

use crate::i18n::tr;
use crate::state::{GameFiles, GameJob, Modal, PendingPicture};
use crate::theme::{self, Tokens};
use crate::{CraftApp, Pick, icons, widgets};
use craft_anything_engine::evidence::ToolError;
use egui::{RichText, Sense, Ui, Vec2};
use serde_json::{Value, json};
use std::path::{Path, PathBuf};

/// Rows shown at once in an archive (the filter narrows the rest).
const SHOWN: usize = 400;

/// How an archive kind reads, its icon, and whether pictures can be saved from it.
fn kind_label(kind: &str) -> (&'static str, &'static str, bool) {
    match kind {
        "unreal_pak" => ("Unreal pak", "package", true),
        "unreal_iostore" => ("Unreal IoStore", "package", false),
        "unity_assets" => ("Unity assets", "boxes", true),
        "unity_bundle" => ("Unity bundle", "boxes", true),
        _ => ("Archive", "file", false),
    }
}

pub fn show(app: &mut CraftApp, ui: &mut Ui) {
    let t = Tokens::get(ui.ctx());
    egui::ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| {
        let full_w = ui.available_width();
        let w = full_w.min(1000.0) - 48.0;
        let pad = ((full_w - w) / 2.0).max(24.0);
        ui.add_space(theme::space::XXL);
        ui.horizontal(|ui| {
            ui.add_space(pad);
            ui.vertical(|ui| {
                ui.set_width(w);
                ui.label(theme::heading(tr("Game files"), theme::TITLE));
                ui.add_space(theme::space::XS);
                ui.label(
                    RichText::new(tr("See what a game packs and save its pictures as PNG files. Only for games you made, or whose owner lets you."))
                        .size(theme::BODY)
                        .color(t.text_dim),
                );
                ui.add_space(theme::space::LG);
                // Every look at a game's files reads them, which Clean room never does.
                if !app.settings.deep_mode {
                    crate::engine_ui::deep_needed(app, ui);
                    return;
                }
                ui.horizontal(|ui| {
                    let label = if app.state.game.folder.is_some() { "Pick another game" } else { "Pick a game's folder" };
                    if widgets::primary_icon(ui, Some("folder-open"), label, true).clicked() {
                        app.pick(Pick::GameFolder);
                    }
                    if let Some(folder) = &app.state.game.folder {
                        ui.add_space(theme::space::SM);
                        // Like every chosen folder: short, cut to fit the row, whole on hover.
                        let full = folder.display().to_string();
                        ui.add(egui::Label::new(RichText::new(crate::short_path(folder)).color(t.text_faint).size(theme::CAPTION)).truncate())
                            .on_hover_text(full);
                    }
                });
                ui.add_space(theme::space::LG);
                about(app, ui, w);
                archives(app, ui, w);
                contents(app, ui);
                ui.add_space(theme::space::XXL);
            });
        });
    });
}

/// What the picked game is made of, and the way to ask about it.
fn about(app: &mut CraftApp, ui: &mut Ui, w: f32) {
    let t = Tokens::get(ui.ctx());
    let (Some(folder), Some(v)) = (app.state.game.folder.clone(), app.state.game.profile.as_ref()) else { return };
    let g = &v["game"];
    if g.is_null() {
        return;
    }
    let found = &g["found"];
    let mut ask = false;
    widgets::card(&t).show(ui, |ui| {
        ui.set_width(w - 34.0);
        ui.horizontal(|ui| {
            icons::show(ui, "puzzle", theme::icon::LG, t.accent_ink);
            ui.label(theme::heading(g["engine"].as_str().unwrap_or_default(), theme::CARD));
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                ask = widgets::primary_icon(ui, Some("message-square"), "Ask about this game", true).clicked();
            });
        });
        if let Some(note) = found["note"].as_str() {
            ui.add_space(theme::space::XS);
            ui.label(RichText::new(note).color(t.text_dim).size(theme::BODY));
        }
        let stack = g["stack"].as_array().cloned().unwrap_or_default();
        if !stack.is_empty() {
            ui.add_space(theme::space::MD);
            widgets::caption(ui, "Built with");
            ui.horizontal_wrapped(|ui| {
                for s in &stack {
                    widgets::pill(ui, s["name"].as_str().unwrap_or_default(), t.text, t.raised).on_hover_text(s["what"].as_str().unwrap_or_default());
                }
            });
        }
        let own = g["other_groups"].as_array().into_iter().flatten().filter_map(Value::as_str).collect::<Vec<_>>();
        if !own.is_empty() {
            ui.add_space(theme::space::MD);
            widgets::caption(ui, "Its own code");
            ui.label(RichText::new(own.join(", ")).color(t.text_dim).size(theme::SMALL));
        }
        ui.add_space(theme::space::MD);
        let scenes = g["scenes"].as_u64().unwrap_or(0);
        let mut facts =
            vec![format!("{} · {} {}", crate::human_bytes(g["bytes"].as_u64().unwrap_or(0) as usize), g["files"].as_u64().unwrap_or(0), tr("files"))];
        if scenes > 0 {
            facts.push(format!("{scenes} {}", tr(if scenes == 1 { "scene" } else { "scenes" })));
        }
        ui.label(RichText::new(facts.join(" · ")).color(t.text_faint).size(theme::CAPTION));
    });
    ui.add_space(theme::space::LG);
    if ask {
        app.open_path(folder);
    }
}

fn archives(app: &mut CraftApp, ui: &mut Ui, w: f32) {
    let t = Tokens::get(ui.ctx());
    let g = &app.state.game;
    if g.looking {
        widgets::working(ui, "Looking for the game's files…", None);
        return;
    }
    if g.folder.is_none() {
        return;
    }
    if g.archives.is_empty() {
        widgets::empty_state(ui, "package", "No game files here", "No Unreal or Unity archives were found. Pick the folder the game is installed in.");
        return;
    }
    widgets::caption(ui, "Archives");
    ui.add_space(theme::space::SM);
    let mut open = None;
    for a in &g.archives {
        let (path, kind) = (a["path"].as_str().unwrap_or_default(), a["kind"].as_str().unwrap_or_default());
        let (label, icon, _) = kind_label(kind);
        let selected = g.open.as_deref() == Some(path);
        let (rect, resp) = ui.allocate_exact_size(Vec2::new(w, 44.0), Sense::click());
        let fill = if selected {
            t.accent_soft
        } else if resp.hovered() {
            t.raised
        } else {
            t.card
        };
        ui.painter().rect_filled(rect, t.radius, fill);
        let icon_rect = egui::Rect::from_center_size(egui::pos2(rect.left() + 24.0, rect.center().y), Vec2::splat(20.0));
        icons::paint(ui, icon_rect, icon, theme::icon::LG, if selected { t.accent_ink } else { t.text_dim });
        let name = a["name"].as_str().unwrap_or(path);
        ui.painter().text(egui::pos2(rect.left() + 46.0, rect.center().y), egui::Align2::LEFT_CENTER, name, theme::medium(theme::SMALL), t.text);
        let detail = format!("{} · {}", tr(label), crate::human_bytes(a["size"].as_u64().unwrap_or(0) as usize));
        ui.painter().text(
            egui::pos2(rect.right() - 14.0, rect.center().y),
            egui::Align2::RIGHT_CENTER,
            detail,
            egui::FontId::proportional(theme::CAPTION),
            t.text_faint,
        );
        if widgets::named(resp, name).on_hover_cursor(egui::CursorIcon::PointingHand).clicked() {
            open = Some((path.to_string(), kind.to_string()));
        }
        ui.add_space(theme::space::XS);
    }
    if let Some((path, kind)) = open {
        open_archive(app, path, kind);
    }
}

fn open_archive(app: &mut CraftApp, path: String, kind: String) {
    let g = &mut app.state.game;
    g.supersede(GameJob::Listing);
    g.open = Some(path.clone());
    g.open_kind = kind.clone();
    g.rows.clear();
    g.filter.clear();
    g.loading = true;
    // A bundle's pictures are its serialized files' Texture2D objects, listed like an .assets file's.
    let (tool, params) = match kind.as_str() {
        "unreal_pak" => ("game_pak_list", json!({"path": path, "filter": ".uasset", "limit": 20000})),
        "unity_assets" | "unity_bundle" => ("game_unity_list", json!({"path": path, "filter": "texture2d", "limit": 20000})),
        "unreal_iostore" => ("game_iostore_list", json!({"path": path, "limit": 2000})),
        _ => ("game_unity_bundle_list", json!({"path": path, "limit": 2000})),
    };
    let job = app.act(tool, params, tr("Read a game's files"));
    app.state.game.start(job, GameJob::Listing);
}

fn contents(app: &mut CraftApp, ui: &mut Ui) {
    let t = Tokens::get(ui.ctx());
    let g = &app.state.game;
    let Some(open) = g.open.clone() else { return };
    let (_, _, saves) = kind_label(&g.open_kind);
    ui.add_space(theme::space::LG);
    let name = std::path::Path::new(&open).file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
    ui.label(theme::heading(format!("{} {name}", tr("Inside")), theme::SECTION));
    ui.add_space(theme::space::SM);
    if g.loading {
        widgets::working(ui, "Reading what's inside…", None);
        return;
    }
    if g.open_kind == "unreal_iostore" {
        ui.label(RichText::new(tr("Pictures can't be saved from IoStore containers yet; you can see what's inside.")).color(t.text_faint).size(theme::CAPTION));
        ui.add_space(theme::space::SM);
    }
    let r = ui.add(
        egui::TextEdit::singleline(&mut app.state.game.filter)
            .hint_text(tr("Filter by name"))
            .desired_width(320.0)
            .frame(widgets::text_field_frame_compact(&t)),
    );
    widgets::named(r, "Filter the files");
    ui.add_space(theme::space::SM);
    let needle = app.state.game.filter.trim().to_lowercase();
    let rows: Vec<Value> =
        app.state.game.rows.iter().filter(|r| needle.is_empty() || row_name(r).to_lowercase().contains(&needle)).take(SHOWN).cloned().collect();
    if rows.is_empty() {
        widgets::empty_state(ui, "search", "Nothing to show", if needle.is_empty() { "" } else { "Nothing matches the filter." });
        return;
    }
    let mut save = None;
    for r in &rows {
        ui.horizontal(|ui| {
            ui.label(RichText::new(row_name(r)).size(theme::SMALL));
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if saves && widgets::secondary(ui, Some("download"), "Save picture").clicked() {
                    save = Some(r.clone());
                }
                ui.label(RichText::new(row_detail(r)).color(t.text_faint).size(theme::CAPTION));
            });
        });
        ui.add_space(theme::space::XXS);
    }
    let total = app.state.game.rows.len();
    if total > rows.len() && needle.is_empty() {
        ui.label(
            RichText::new(format!("{} {} {}", tr("Showing"), rows.len(), tr("— use the filter to find the rest."))).color(t.text_faint).size(theme::CAPTION),
        );
    }
    if let Some(item) = save {
        ask_to_save(app, open, item);
    }
}

/// A row's name: a path in a pak, an object's name in a Unity file.
fn row_name(r: &Value) -> String {
    r["path"].as_str().or_else(|| r["name"].as_str()).map_or_else(|| format!("#{}", r["path_id"]), str::to_string)
}

fn row_detail(r: &Value) -> String {
    let size = r["size"].as_u64().map(|s| crate::human_bytes(s as usize)).unwrap_or_default();
    // A pak's "none" compression says nothing to a person; a class or a method does.
    match r["class"].as_str().or_else(|| r["compression"].as_str().filter(|c| *c != "none")) {
        Some(c) => format!("{c} · {size}"),
        None => size,
    }
}

/// Saving starts with the person's word that they may unpack this game (asked once per folder),
/// then where to save.
fn ask_to_save(app: &mut CraftApp, archive: String, item: Value) {
    let base = row_name(&item);
    let stem = base.rsplit(['/', '\\']).next().unwrap_or(&base).trim_end_matches(".uasset").to_string();
    let g = &mut app.state.game;
    g.pending = Some(PendingPicture { archive, kind: g.open_kind.clone(), item, name: format!("{stem}.png") });
    if g.permitted.is_some() && g.permitted == g.folder {
        app.pick(Pick::SavePicture);
    } else {
        app.state.modal = Some(Modal::GamePermission);
    }
}

/// The question asked before a game's files are unpacked.
pub fn permission_modal(app: &mut CraftApp, ui: &mut Ui, close: &mut bool) {
    let t = Tokens::get(ui.ctx());
    let (rect, _) = ui.allocate_exact_size(Vec2::splat(52.0), Sense::hover());
    ui.painter().rect_filled(rect, t.radius, t.accent_soft);
    icons::paint(ui, rect, "shield-check", theme::icon::XL, t.accent_ink);
    ui.add_space(theme::space::MD);
    ui.label(theme::heading(tr("Do you have permission?"), theme::SECTION));
    ui.add_space(theme::space::XS);
    ui.label(
        RichText::new(tr(
            "Only take pictures out of games you made, or whose owner allows it. A game's art is usually protected by copyright and by the game's licence.",
        ))
        .color(t.text_dim),
    );
    ui.add_space(theme::space::LG);
    ui.horizontal(|ui| {
        if widgets::primary_icon(ui, Some("shield-check"), "I have permission", true).clicked() {
            app.state.game.permitted.clone_from(&app.state.game.folder);
            *close = true;
            app.pick(Pick::SavePicture);
        }
        if widgets::secondary(ui, None, "Cancel").clicked() {
            app.state.game.pending = None;
            *close = true;
        }
    });
}

/// The game's folder was picked: look for its archives.
pub fn folder_picked(app: &mut CraftApp, folder: PathBuf) {
    // The tools take paths as text, so a name that isn't text can't be looked through; say so
    // instead of sending a lossy path that names something else.
    let Some(path) = folder.to_str() else {
        app.toast(tr("That folder's name has characters Craft Anything can't read. Pick another folder."), true);
        return;
    };
    let path = path.to_string();
    // A job from the folder shown before stays known (so its failure is logged, not said), but
    // the screen no longer waits for it.
    let mut jobs = std::mem::take(&mut app.state.game.jobs);
    for pending in jobs.values_mut() {
        pending.current = false;
    }
    app.state.game = GameFiles { folder: Some(folder), looking: true, permitted: app.state.game.permitted.clone(), jobs, ..Default::default() };
    let archives = app.act("game_archives_list", json!({"path": path.clone()}), tr("Find a game's files"));
    let triage = app.act("core_target_triage", json!({"path": path}), tr("Look at a game"));
    app.state.game.start(archives, GameJob::FolderScan);
    app.state.game.start(triage, GameJob::Triage);
}

/// Where to save the pending picture was picked: save it.
pub fn save_to(app: &mut CraftApp, mut output: PathBuf) {
    let Some(p) = app.state.game.pending.take() else { return };
    if output.extension().is_none() {
        output.set_extension("png");
    }
    let output = output.display().to_string();
    match p.kind.as_str() {
        "unreal_pak" => {
            let params = json!({"path": p.archive, "file": p.item["path"], "output": output, "permission": true});
            app.act("game_unreal_texture_export", params, tr("Save a picture"));
        }
        "unity_assets" | "unity_bundle" => {
            // A bundle's rows say which serialized file inside it holds the object.
            let params = json!({"path": p.archive, "file": p.item["file"], "path_id": p.item["path_id"], "output": output});
            app.act("game_unity_asset_export", params, tr("Save a picture"));
        }
        _ => {}
    }
}

/// The name the save dialog suggests.
pub fn suggested_name(app: &CraftApp) -> String {
    app.state.game.pending.as_ref().map_or_else(|| "picture.png".to_string(), |p| p.name.clone())
}

/// The Game files tools whose answer belongs to a job the screen started.
fn looked_up(tool: &str) -> bool {
    matches!(tool, "game_archives_list" | "core_target_triage" | "game_pak_list" | "game_unity_list" | "game_iostore_list" | "game_unity_bundle_list")
}

/// A finished save belongs to the archive still on screen.
fn still_showing(app: &CraftApp, params: &Value) -> bool {
    let Some(asked) = params.get("path").and_then(Value::as_str) else { return false };
    let asked = craft_anything_engine::workspace::normalized(Path::new(asked));
    app.state.game.open.as_deref().is_some_and(|shown| craft_anything_engine::workspace::normalized(Path::new(shown)) == asked)
}

/// A game tool finished.
pub fn tool_done(app: &mut CraftApp, job: u64, tool: &str, params: &Value, result: Result<Value, ToolError>) {
    if looked_up(tool) {
        // The job id, not the folder or archive path, says whether the screen still waits for
        // this answer: picking A, B and A again leaves the first A's answer stale.
        let Some(pending) = app.state.game.finish(job) else { return };
        if !pending.current {
            return;
        }
    } else if !still_showing(app, params) {
        return;
    }
    let g = &mut app.state.game;
    match (tool, result) {
        ("game_archives_list", Ok(v)) => {
            g.looking = false;
            g.archives = v["archives"].as_array().cloned().unwrap_or_default();
        }
        ("game_archives_list", Err(e)) => {
            g.looking = false;
            app.toast(format!("{} {}", tr("Couldn't look through that folder."), e.message), true);
        }
        ("core_target_triage", Ok(v)) => g.profile = Some(v),
        // The archives' search says what went wrong with the folder.
        ("core_target_triage", Err(_)) => {}
        ("game_unreal_texture_export" | "game_unity_asset_export", Ok(v)) => {
            let msg = format!("{} {}", tr("Saved"), v["output"].as_str().unwrap_or_default());
            app.toast(msg, false);
        }
        // A failed save says so; it isn't the archive's listing failing.
        ("game_unreal_texture_export" | "game_unity_asset_export", Err(e)) => {
            app.toast(e.for_people(), true);
        }
        (_, Ok(v)) => {
            g.loading = false;
            g.rows = v["files"].as_array().or_else(|| v["objects"].as_array()).cloned().unwrap_or_default();
        }
        (_, Err(e)) => {
            g.loading = false;
            app.toast(e.for_people(), true);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::jobs::Finished;
    use crate::state::PendingGameJob;
    use crate::{Done, demo};
    use craft_anything_engine::Mode;
    use serde_json::json;
    use std::path::PathBuf;

    fn app() -> CraftApp {
        demo::app(demo::engine(Mode::Beginner, true))
    }

    fn two_folders() -> (tempfile::TempDir, PathBuf, PathBuf) {
        let d = tempfile::tempdir().unwrap();
        let a = d.path().join("a");
        let b = d.path().join("b");
        std::fs::create_dir_all(&a).unwrap();
        std::fs::create_dir_all(&b).unwrap();
        (d, a, b)
    }

    /// The id of the job of `kind` the screen is (or isn't) still waiting for.
    fn job_of(app: &CraftApp, kind: GameJob, current: bool) -> u64 {
        app.state.game.jobs.iter().find(|(_, pending)| pending.kind == kind && pending.current == current).map(|(id, _)| *id).expect("a game job of that kind")
    }

    #[test]
    fn a_look_for_another_game_folder_is_ignored() {
        let (_d, a, b) = two_folders();
        let mut app = app();
        app.state.game.folder = Some(b.clone());
        app.state.game.looking = true;
        app.state.game.jobs.insert(1, PendingGameJob { kind: GameJob::FolderScan, current: false });
        app.state.game.jobs.insert(2, PendingGameJob { kind: GameJob::Triage, current: false });
        app.state.game.jobs.insert(5, PendingGameJob { kind: GameJob::FolderScan, current: true });
        app.state.game.jobs.insert(6, PendingGameJob { kind: GameJob::Triage, current: true });
        tool_done(
            &mut app,
            1,
            "game_archives_list",
            &json!({"path": a.display().to_string()}),
            Ok(json!({"folder": a.display().to_string(), "archives": [{"path": "a.pak"}]})),
        );
        assert!(app.state.game.archives.is_empty(), "an old look doesn't fill the new game: {:?}", app.state.game.archives);
        assert!(app.state.game.looking, "the new game's look is still running");
        tool_done(&mut app, 2, "core_target_triage", &json!({"path": a.display().to_string()}), Ok(json!({"game": {"engine": "Unreal"}})));
        assert!(app.state.game.profile.is_none(), "an old profile isn't the new game's");

        tool_done(
            &mut app,
            5,
            "game_archives_list",
            &json!({"path": b.display().to_string()}),
            Ok(json!({"folder": b.display().to_string(), "archives": [{"path": "b.pak"}]})),
        );
        assert_eq!(app.state.game.archives.len(), 1);
        assert!(!app.state.game.looking);
        tool_done(&mut app, 6, "core_target_triage", &json!({"path": b.display().to_string()}), Ok(json!({"game": {"engine": "Unity"}})));
        assert_eq!(app.state.game.profile.as_ref().unwrap()["game"]["engine"], "Unity");
    }

    #[test]
    fn a_stale_look_for_the_same_folder_is_ignored() {
        let (_d, a, _b) = two_folders();
        let mut app = app();
        // Both picks run through the screen's own path, and results arrive through the app's dispatch.
        folder_picked(&mut app, a.clone());
        let first = job_of(&app, GameJob::FolderScan, true);
        folder_picked(&mut app, a.clone());
        let second = job_of(&app, GameJob::FolderScan, true);
        assert_ne!(first, second, "the second pick starts a new look");
        app.handle_done(Finished {
            job: first,
            result: Ok(Done::Acted {
                tool: "game_archives_list",
                params: json!({"path": a.display().to_string()}),
                result: Ok(json!({"folder": a.display().to_string(), "archives": [{"path": "old.pak"}]})),
            }),
        });
        assert!(app.state.game.archives.is_empty(), "the first A look is stale after A was picked again: {:?}", app.state.game.archives);
        assert!(app.state.game.looking, "the newer A look is still running");
        app.handle_done(Finished {
            job: second,
            result: Ok(Done::Acted {
                tool: "game_archives_list",
                params: json!({"path": a.display().to_string()}),
                result: Ok(json!({"folder": a.display().to_string(), "archives": [{"path": "new.pak"}]})),
            }),
        });
        assert_eq!(app.state.game.archives.len(), 1);
        assert!(!app.state.game.looking);
    }

    #[test]
    fn a_failed_worker_settles_only_its_own_game_request() {
        let (_d, a, _b) = two_folders();
        let mut app = app();
        folder_picked(&mut app, a.clone());
        let scan = job_of(&app, GameJob::FolderScan, true);
        let triage = job_of(&app, GameJob::Triage, true);
        // A failed triage is said, but it holds neither spinner.
        app.handle_done(Finished { job: triage, result: Err("triage failed".into()) });
        assert!(app.state.game.looking, "a failed triage doesn't stop the folder scan");
        assert_eq!(app.state.toasts.len(), 1, "the current triage's failure is said");
        // The folder scan's own failure settles its spinner.
        app.handle_done(Finished { job: scan, result: Err("scan failed".into()) });
        assert!(!app.state.game.looking, "a failed folder scan stops its own spinner");
        assert_eq!(app.state.toasts.len(), 2);
    }

    #[test]
    fn a_superseded_game_failure_is_logged_without_a_toast() {
        let (_d, a, _b) = two_folders();
        let mut app = app();
        folder_picked(&mut app, a.clone());
        let first = job_of(&app, GameJob::FolderScan, true);
        folder_picked(&mut app, a.clone());
        let second = job_of(&app, GameJob::FolderScan, true);
        app.handle_done(Finished { job: first, result: Err("worker failed".into()) });
        assert!(app.state.toasts.is_empty(), "a superseded failure isn't said: {:?}", app.state.toasts);
        assert!(app.state.game.looking, "the newer look still runs");
        app.handle_done(Finished { job: second, result: Err("worker failed".into()) });
        assert_eq!(app.state.toasts.len(), 1);
        assert!(!app.state.game.looking);
    }

    #[cfg(unix)]
    #[test]
    fn a_non_text_folder_is_refused_before_a_look_starts() {
        use std::os::unix::ffi::OsStringExt;
        let d = tempfile::tempdir().unwrap();
        let mut name = b"game-".to_vec();
        name.push(0xFF);
        let folder = d.path().join(std::ffi::OsString::from_vec(name));
        std::fs::create_dir(&folder).unwrap();
        assert!(folder.to_str().is_none(), "the folder's name isn't text");
        let (_d2, a, _b) = two_folders();
        let mut app = app();
        app.state.game.folder = Some(a.clone());
        folder_picked(&mut app, folder);
        assert!(!app.state.game.looking, "a folder the tools can't name doesn't start a look");
        assert!(app.state.game.jobs.is_empty(), "nothing was sent");
        assert_eq!(app.state.game.folder, Some(a), "the screen stays with the folder it showed");
        assert_eq!(app.state.toasts.len(), 1, "the refusal is said");
    }

    #[test]
    fn a_listing_for_another_archive_is_ignored() {
        let (_d, a, b) = two_folders();
        let mut app = app();
        let open = b.join("b.pak");
        app.state.game.open = Some(open.display().to_string());
        app.state.game.loading = true;
        app.state.game.jobs.insert(1, PendingGameJob { kind: GameJob::Listing, current: false });
        app.state.game.jobs.insert(3, PendingGameJob { kind: GameJob::Listing, current: true });
        tool_done(&mut app, 1, "game_pak_list", &json!({"path": a.join("a.pak").display().to_string()}), Ok(json!({"files": [{"path": "A.uasset"}]})));
        assert!(app.state.game.rows.is_empty(), "an old archive's rows don't fill the new one");
        assert!(app.state.game.loading, "the new archive is still loading");
        tool_done(&mut app, 3, "game_pak_list", &json!({"path": open.display().to_string()}), Ok(json!({"files": [{"path": "B.uasset"}]})));
        assert_eq!(app.state.game.rows.len(), 1);
        assert!(!app.state.game.loading);
    }

    #[test]
    fn an_export_error_does_not_stop_the_archive_listing() {
        let (_d, _a, b) = two_folders();
        let mut app = app();
        let open = b.join("b.pak");
        app.state.game.open = Some(open.display().to_string());
        app.state.game.loading = true;
        app.state.game.jobs.insert(3, PendingGameJob { kind: GameJob::Listing, current: true });
        // The save names the archive on screen; a reopened listing may share that path.
        tool_done(
            &mut app,
            9,
            "game_unreal_texture_export",
            &json!({"path": open.display().to_string(), "output": "x.png"}),
            Err(ToolError::invalid("no room")),
        );
        assert!(app.state.game.loading, "a failed save isn't the listing failing");
        assert_eq!(app.state.toasts.len(), 1, "the save's error is said");
        tool_done(&mut app, 3, "game_pak_list", &json!({"path": open.display().to_string()}), Err(ToolError::invalid("no")));
        assert!(!app.state.game.loading, "the listing's own error ends loading");
        assert_eq!(app.state.toasts.len(), 2);
    }

    #[test]
    fn a_look_error_for_another_game_folder_is_not_shown() {
        let (_d, a, b) = two_folders();
        let mut app = app();
        app.state.game.folder = Some(b.clone());
        app.state.game.jobs.insert(1, PendingGameJob { kind: GameJob::FolderScan, current: false });
        app.state.game.jobs.insert(2, PendingGameJob { kind: GameJob::FolderScan, current: true });
        tool_done(&mut app, 1, "game_archives_list", &json!({"path": a.display().to_string()}), Err(ToolError::invalid("no")));
        assert!(app.state.toasts.is_empty(), "an old folder's error isn't the new game's: {:?}", app.state.toasts);
        tool_done(&mut app, 2, "game_archives_list", &json!({"path": b.display().to_string()}), Err(ToolError::invalid("no")));
        assert_eq!(app.state.toasts.len(), 1, "the shown game's error is said");
    }
}
