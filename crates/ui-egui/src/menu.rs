//! The macOS menu bar: the app's own actions, with the shortcuts Mac apps use, beside the system's
//! window and app items. Every label is in the app's language, and follows it when it changes.

use crate::CraftApp;
use crate::i18n::{self, tr};
use crate::palette::{self, AppCommand};
use crate::state::Screen;
use craft_anything_engine::Mode;
use muda::accelerator::{Accelerator, Code, Modifiers};
use muda::{AboutMetadata, CheckMenuItem, Menu, MenuEvent, MenuId, MenuItem, PredefinedMenuItem, Submenu};
use std::sync::mpsc::{Receiver, channel};

const REPO: &str = env!("CARGO_PKG_REPOSITORY");
const NEW_ISSUE: &str = concat!(env!("CARGO_PKG_REPOSITORY"), "/issues/new/choose");

#[derive(Debug, Clone, Copy)]
enum Action {
    App(AppCommand),
    NewProject,
    Palette,
    Mode(Mode),
    Deep(bool),
    Quit,
    Updates,
    Url(&'static str),
}

/// A menu entry with a translated label, retitled when the language changes.
enum Label {
    Item(MenuItem),
    Check(CheckMenuItem),
    Sub(Submenu),
    System(PredefinedMenuItem),
}

impl Label {
    /// Shows `text` as the entry's label.
    fn set(&self, text: &str) {
        match self {
            Label::Item(i) => i.set_text(text),
            Label::Check(i) => i.set_text(text),
            Label::Sub(i) => i.set_text(text),
            Label::System(i) => i.set_text(text),
        }
    }
}

/// The menu bar, kept alive for as long as the app runs.
pub struct MenuBar {
    _menu: Menu,
    actions: Vec<(MenuId, Action)>,
    /// Each label with its English text, and the language they're shown in.
    labels: Vec<(Label, &'static str)>,
    language: String,
    /// What only makes sense once first run is done (the palette's rule).
    gated: Vec<MenuItem>,
    simple: CheckMenuItem,
    advanced: CheckMenuItem,
    clean: CheckMenuItem,
    deep: CheckMenuItem,
    picked: Receiver<MenuId>,
}

fn cmd(key: Code) -> Option<Accelerator> {
    Some(Accelerator::new(Modifiers::META, key))
}

impl MenuBar {
    /// Builds the menu bar and puts it in place of the system's default one. Call on the main
    /// thread once the window is open; `None` (logged) if the system refused it.
    pub fn install(ctx: &egui::Context) -> Option<Self> {
        let mut actions = Vec::new();
        let mut labels = Vec::new();
        let mut item = |label: &'static str, key: Option<Accelerator>, action: Action| {
            let i = MenuItem::new(tr(label), true, key);
            actions.push((i.id().clone(), action));
            labels.push((Label::Item(i.clone()), label));
            i
        };
        let settings = item("Settings", cmd(Code::Comma), Action::App(AppCommand::Settings));
        let quit = item("Quit Craft Anything", cmd(Code::KeyQ), Action::Quit);
        let new_project = item("New project", cmd(Code::KeyN), Action::NewProject);
        let open_file = item("Open a file…", cmd(Code::KeyO), Action::App(AppCommand::OpenFile));
        let open_folder =
            item("Open a folder or app bundle…", Some(Accelerator::new(Modifiers::META | Modifiers::SHIFT, Code::KeyO)), Action::App(AppCommand::OpenFolder));
        let export = item("Export a report (Markdown)", None, Action::App(AppCommand::ExportReport));
        let palette = item("Command palette", cmd(Code::KeyK), Action::Palette);
        let theme = item("Switch light / dark theme", None, Action::App(AppCommand::ToggleTheme));
        let website = item("Craft Anything on GitHub", None, Action::Url(REPO));
        let updates = item("Check for Updates…", None, Action::Updates);
        let report = item("Report a problem…", None, Action::Url(NEW_ISSUE));
        let mut check = |label: &'static str, action: Action| {
            let i = CheckMenuItem::new(tr(label), true, false, None);
            actions.push((i.id().clone(), action));
            labels.push((Label::Check(i.clone()), label));
            i
        };
        let simple = check("Simple", Action::Mode(Mode::Beginner));
        let advanced = check("Advanced", Action::Mode(Mode::Experienced));
        let clean = check("Clean room", Action::Deep(false));
        let deep = check("Deep", Action::Deep(true));

        let mut sub = |label: &'static str| {
            let s = Submenu::new(tr(label), true);
            labels.push((Label::Sub(s.clone()), label));
            s
        };
        // The first submenu is the app menu; macOS titles it with the app's name.
        let app_menu = Submenu::new("Craft Anything", true);
        let file = sub("File");
        let view = sub("View");
        let window = sub("Window");
        let help = sub("Help");
        // The system's own items, named in the app's language rather than the system's.
        let mut system = |make: fn(Option<&str>) -> PredefinedMenuItem, label: &'static str| {
            let i = make(Some(&tr(label)));
            labels.push((Label::System(i.clone()), label));
            i
        };
        let services = system(PredefinedMenuItem::services, "Services");
        let hide = system(PredefinedMenuItem::hide, "Hide Craft Anything");
        let hide_others = system(PredefinedMenuItem::hide_others, "Hide others");
        let show_all = system(PredefinedMenuItem::show_all, "Show all");
        let close_window = system(PredefinedMenuItem::close_window, "Close window");
        let fullscreen = system(PredefinedMenuItem::fullscreen, "Full screen");
        let minimise = system(PredefinedMenuItem::minimize, "Minimise");
        let zoom = system(PredefinedMenuItem::maximize, "Zoom");
        let to_front = system(PredefinedMenuItem::bring_all_to_front, "Bring all to front");
        let about_metadata = AboutMetadata { name: Some("Craft Anything".into()), version: Some(env!("CARGO_PKG_VERSION").into()), ..Default::default() };
        let about = PredefinedMenuItem::about(Some(&tr("About Craft Anything")), Some(about_metadata));
        labels.push((Label::System(about.clone()), "About Craft Anything"));
        let sep = PredefinedMenuItem::separator;
        let menu = Menu::new();
        let built = app_menu
            .append_items(&[&about, &sep(), &settings, &sep(), &services, &sep(), &hide, &hide_others, &show_all, &sep(), &quit])
            .and_then(|()| file.append_items(&[&new_project, &open_file, &open_folder, &sep(), &export, &sep(), &close_window]))
            .and_then(|()| view.append_items(&[&palette, &sep(), &simple, &advanced, &sep(), &clean, &deep, &sep(), &theme, &sep(), &fullscreen]))
            .and_then(|()| window.append_items(&[&minimise, &zoom, &sep(), &to_front]))
            .and_then(|()| help.append_items(&[&updates, &sep(), &website, &report]))
            .and_then(|()| menu.append_items(&[&app_menu, &file, &view, &window, &help]));
        if let Err(e) = built {
            log::warn!("the menu bar couldn't be built: {e}");
            return None;
        }
        menu.init_for_nsapp();
        window.set_as_windows_menu_for_nsapp();
        help.set_as_help_menu_for_nsapp();

        // Picks arrive between frames: pass them on and wake the UI to act on them.
        let (tx, picked) = channel();
        let ctx = ctx.clone();
        MenuEvent::set_event_handler(Some(move |e: MenuEvent| {
            let _ = tx.send(e.id);
            ctx.request_repaint();
        }));
        let gated = vec![settings, new_project, open_file, open_folder, export, palette, theme];
        Some(Self { _menu: menu, actions, labels, language: i18n::language(), gated, simple, advanced, clean, deep, picked })
    }
}

/// Runs what was picked since the last frame and shows the current mode and switches.
pub fn update(app: &mut CraftApp, ctx: &egui::Context) {
    if let Some(bar) = app.menu_bar.as_mut() {
        let language = i18n::language();
        if bar.language != language {
            for (label, text) in &bar.labels {
                label.set(&tr(text));
            }
            bar.language = language;
        }
    }
    let Some(bar) = app.menu_bar.as_ref() else { return };
    let ready = app.settings.first_run_done;
    let picked: Vec<Action> = bar.picked.try_iter().filter_map(|id| bar.actions.iter().find(|(i, _)| *i == id).map(|(_, a)| *a)).collect();
    for i in &bar.gated {
        if i.is_enabled() != ready {
            i.set_enabled(ready);
        }
    }
    let deep = app.settings.deep_mode;
    let beginner = app.beginner();
    for (item, on) in [(&bar.simple, beginner), (&bar.advanced, !beginner), (&bar.clean, !deep), (&bar.deep, deep)] {
        if item.is_enabled() != ready {
            item.set_enabled(ready);
        }
        if item.is_checked() != on {
            item.set_checked(on);
        }
    }
    for action in picked {
        match action {
            Action::Updates => app.updates.check(ctx),
            Action::Quit => ctx.send_viewport_cmd(egui::ViewportCommand::Close),
            Action::Url(url) => ctx.open_url(egui::OpenUrl::new_tab(url)),
            _ if !ready => {}
            Action::App(c) => palette::run_app_command(app, c),
            Action::NewProject => app.state.screen = Screen::Home,
            Action::Palette => palette::toggle(app),
            Action::Mode(m) => app.set_mode(m),
            Action::Deep(on) => app.set_deep(on),
        }
    }
}

#[cfg(test)]
mod tests {
    /// An empty repository field made Help open a local folder instead of the web page.
    #[test]
    fn help_links_are_web_pages() {
        for url in [super::REPO, super::NEW_ISSUE] {
            assert!(url.starts_with("https://github.com/"), "{url}");
        }
    }
}
