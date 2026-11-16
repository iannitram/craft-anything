//! What the builder may do without asking, per kind of action, in the open project
//! (`project_approvals_set`): a menu under the prompt box, and `/approvals <kind> <choice>` typed in
//! it. A kind left on Default follows Auto mode.

use crate::i18n::{tr, trf};
use crate::slash::{self, Command, Typed};
use crate::theme::{self, Tokens};
use crate::{CraftApp, widgets};
use craft_anything_engine::evidence::ToolError;
use craft_anything_engine::workspace::{ACTION_KINDS, ActionKind, Approval};
use egui::{RichText, Ui};
use serde_json::{Value, json};
use std::path::Path;

/// Each kind as the person reads it.
const LABELS: &[(ActionKind, &str)] = &[
    (ActionKind::Web, "Reading the web"),
    (ActionKind::Commands, "Other commands"),
    (ActionKind::OutsideFiles, "Files outside the project"),
    (ActionKind::Publish, "Pushing and publishing"),
    (ActionKind::DeleteOutside, "Deleting outside the project"),
    (ActionKind::System, "System changes"),
    (ActionKind::OtherTools, "Other tools"),
];

/// Other words for a kind after `/approvals`, as well as its own name (`publish`).
const ALIASES: &[(&str, ActionKind)] = &[
    ("push", ActionKind::Publish),
    ("pushing", ActionKind::Publish),
    ("publishing", ActionKind::Publish),
    ("fetch", ActionKind::Web),
    ("internet", ActionKind::Web),
    ("files", ActionKind::OutsideFiles),
    ("delete", ActionKind::DeleteOutside),
    ("deletes", ActionKind::DeleteOutside),
    ("tools", ActionKind::OtherTools),
];

/// The choices, as shown and as typed after `/approvals` (in any case); none is Default.
const CHOICES: &[(&str, Option<Approval>)] =
    &[("Default", None), ("Allow", Some(Approval::Allow)), ("Ask", Some(Approval::Ask)), ("Refuse", Some(Approval::Refuse))];

fn label(kind: ActionKind) -> &'static str {
    LABELS.iter().find(|(k, _)| *k == kind).map_or("", |(_, l)| l)
}

fn choice_label(choice: Option<Approval>) -> &'static str {
    CHOICES.iter().find(|(_, c)| *c == choice).map_or("", |(l, _)| l)
}

/// The kind a word after `/approvals` names: its own name or another word for it.
fn kind_named(word: &str) -> Option<ActionKind> {
    ActionKind::named(word).or_else(|| ALIASES.iter().find(|(w, _)| w.eq_ignore_ascii_case(word)).map(|(_, k)| *k))
}

/// The kinds' names, to say what can be typed: "web, commands, …".
fn kind_names() -> String {
    ACTION_KINDS.iter().map(|r| r.name).collect::<Vec<_>>().join(", ")
}

/// `/approvals <kind> <choice>` (in either order) when `text` is that command: the kind and the
/// choice (none: Default). `None`: it's another command or a message. An error says what to type,
/// and the text stays in the box to be fixed.
pub fn command_in(text: &str) -> Option<Result<(ActionKind, Option<Approval>), String>> {
    let Typed::Command(Command::Approvals, rest) = slash::read(text) else { return None };
    let usage =
        || trf("After /approvals, type a kind and allow, ask, refuse or default, like /approvals publish ask. Kinds: {kinds}.", &[("kinds", &kind_names())]);
    let choice = |w: &str| CHOICES.iter().find(|(n, _)| n.eq_ignore_ascii_case(w)).map(|(_, c)| *c);
    let words: Vec<&str> = rest.split_whitespace().collect();
    let [a, b] = words.as_slice() else { return Some(Err(usage())) };
    let (word, picked) = match (choice(a), choice(b)) {
        (Some(c), None) => (*b, c),
        (None, Some(c)) => (*a, c),
        _ => return Some(Err(usage())),
    };
    Some(match kind_named(word) {
        Some(kind) => Ok((kind, picked)),
        None => Err(trf("There's no kind called {name}. Kinds: {kinds}.", &[("name", word), ("kinds", &kind_names())])),
    })
}

impl CraftApp {
    /// Choose what one kind of the builder's actions gets in this project. Shown at once; saved by
    /// the tool, and from the builder's next action.
    pub fn set_approval(&mut self, root: &Path, kind: ActionKind, choice: Option<Approval>) {
        if let Some(ws) = self.workspace.as_mut().filter(|w| w.root == root) {
            match choice {
                Some(a) => ws.manifest.approvals.insert(kind, a),
                None => ws.manifest.approvals.remove(&kind),
            };
        }
        let mode = choice.map_or(json!("default"), |a| json!(a));
        self.act("project_approvals_set", json!({"project": root, "kind": kind, "mode": mode}), tr("Approvals"));
    }

    /// The tool saved a choice (or couldn't).
    pub(crate) fn approval_set(&mut self, result: Result<Value, ToolError>) {
        match result {
            Ok(v) => {
                // What was saved (the person's other changes on disk come along).
                if let Some(ws) = self.workspace.as_mut().filter(|w| v["project"].as_str().is_some_and(|p| w.root == Path::new(p))) {
                    let _ = ws.reload();
                }
                let set = &v["set"];
                let Some(kind) = set["kind"].as_str().and_then(ActionKind::named) else { return };
                let chosen: Option<Approval> = serde_json::from_value(set["chosen"].clone()).ok();
                let gets: Option<Approval> = serde_json::from_value(set["gets"].clone()).ok();
                let choice = match chosen {
                    Some(c) => tr(choice_label(Some(c))),
                    // Default: say what that is now.
                    None => format!("{} ({})", tr(choice_label(None)), gets.map(|g| tr(choice_label(Some(g)))).unwrap_or_default()),
                };
                self.toast(trf("In this project, {kind}: {choice}.", &[("kind", &tr(label(kind))), ("choice", &choice)]), false);
            }
            Err(e) => {
                // The menu showed the choice before it was saved: show what's really kept again.
                if let Some(ws) = self.workspace.as_mut() {
                    let _ = ws.reload();
                }
                self.toast(format!("{} {}", tr("Couldn't change the approvals:"), e.for_people()), true);
            }
        }
    }
}

/// Under the prompt box: a menu with one row per kind of action and what it gets in this project.
pub(crate) fn menu(app: &mut CraftApp, ui: &mut Ui, root: &Path) {
    let Some(ws) = &app.workspace else { return };
    let chosen = ws.manifest.approvals.clone();
    let t = Tokens::get(ui.ctx());
    let names: Vec<&str> = CHOICES.iter().map(|(n, _)| *n).collect();
    let button = widgets::link(ui, "Approvals").on_hover_text(tr("What the builder may do without asking you, in this project."));
    let mut picked = None;
    // A named id, so tests (and anything else) can tell whether it's open.
    let popup = egui::Popup::from_toggle_button_response(&button).id(egui::Id::new("project-approvals"));
    popup.close_behavior(egui::PopupCloseBehavior::CloseOnClickOutside).show(|ui| {
        egui::Grid::new("approvals").num_columns(2).spacing([12.0, 6.0]).show(ui, |ui| {
            for row in ACTION_KINDS {
                ui.label(RichText::new(tr(label(row.kind))).size(theme::SMALL));
                let now = chosen.get(&row.kind).copied();
                let sel = CHOICES.iter().position(|(_, c)| *c == now).unwrap_or(0);
                // Every row has the same labels, which `segmented` makes its ids from.
                ui.push_id(row.name, |ui| {
                    if let Some(c) = widgets::segmented(ui, &names, sel, theme::ControlSize::Small).and_then(|i| CHOICES.get(i)) {
                        picked = Some((row.kind, c.1));
                    }
                });
                ui.end_row();
            }
        });
        ui.add_space(theme::space::XS);
        let note = tr("Default follows Auto mode: with it on, what can be undone goes ahead and the rest is refused; with it off, the builder asks.");
        ui.add(egui::Label::new(RichText::new(note).size(theme::SMALL).line_height(Some(theme::leading(theme::SMALL))).color(t.text_faint)).wrap());
    });
    if let Some((kind, choice)) = picked {
        app.set_approval(root, kind, choice);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn approvals_takes_a_kind_and_a_choice_in_either_order() {
        assert_eq!(command_in("/approvals publish ask"), Some(Ok((ActionKind::Publish, Some(Approval::Ask)))));
        assert_eq!(command_in("/approvals allow push"), Some(Ok((ActionKind::Publish, Some(Approval::Allow)))));
        assert_eq!(command_in("  /APPROVALS Web Refuse "), Some(Ok((ActionKind::Web, Some(Approval::Refuse)))));
        assert_eq!(command_in("/approvals outside_files default"), Some(Ok((ActionKind::OutsideFiles, None))));
        assert_eq!(command_in("/approvals fetch allow"), Some(Ok((ActionKind::Web, Some(Approval::Allow)))));
    }

    #[test]
    fn approvals_says_what_to_type_when_it_cant_read_it() {
        for wrong in ["/approvals", "/approvals publish", "/approvals allow ask", "/approvals publish ask now"] {
            let why = command_in(wrong).unwrap().unwrap_err();
            assert!(why.contains("/approvals publish ask") && why.contains("delete_outside"), "{wrong}: {why}");
        }
        let why = command_in("/approvals everything allow").unwrap().unwrap_err();
        assert!(why.contains("everything"), "{why}");
    }

    #[test]
    fn other_text_isnt_this_command() {
        for other in ["/btw may it push?", "push it to github", "/approve publish allow"] {
            assert_eq!(command_in(other), None, "{other}");
        }
    }

    #[test]
    fn every_kind_has_a_label() {
        for row in ACTION_KINDS {
            assert!(!label(row.kind).is_empty(), "{}", row.name);
        }
    }
}
