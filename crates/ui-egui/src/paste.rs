//! Pictures pasted with Ctrl+V (Cmd+V on a Mac) into a prompt box that takes attachments.
//!
//! egui-winit turns the paste shortcut (Ctrl+V, Shift+Insert on Windows, a Paste key) into a text
//! paste and drops the press when the clipboard holds no text, so a copied screenshot never reaches
//! egui. The key's release still does: one released with no press egui saw, and no text pasted, is
//! a paste of something that isn't text.
//! The clipboard is then read on a worker ([`read_pictures`]): copied picture files are attached
//! as they are, and a copied image is saved as a PNG first.

use std::path::{Path, PathBuf};

/// What the events said about the paste shortcut, kept between passes.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
struct Watch {
    /// The pass this was worked out for, so every caller in a pass gets the same answer (and a
    /// second pass in the same frame, which has no events of its own, doesn't paste again).
    pass: u64,
    /// A paste key went down where egui saw it (typing a "v"), or egui-winit made it a copy or a
    /// cut (Ctrl+Insert): its release is no paste.
    down: bool,
    /// Text was pasted since: the clipboard's text was the paste.
    text_pasted: bool,
    /// Ctrl (Cmd on a Mac) was held since the last paste key came up: a V released after another
    /// program's shortcut (Win+V, the clipboard history) isn't a paste.
    command: bool,
    /// Shift was held since then, for Shift+Insert.
    shift: bool,
    /// This pass's answer.
    wanted: bool,
}

/// The keys egui-winit may read as the paste shortcut.
const PASTE_KEYS: [egui::Key; 3] = [egui::Key::V, egui::Key::Insert, egui::Key::Paste];

/// What `events` mean, after `before`, with `held` the modifiers this pass: a paste of something
/// other than text happened when a paste key came up after its shortcut's modifier was held,
/// without egui having seen the key go down or seen any text pasted.
fn step(before: Watch, events: &[egui::Event], held: egui::Modifiers) -> Watch {
    let mut w = Watch { wanted: false, ..before };
    w.command |= held.command;
    w.shift |= held.shift;
    let fresh = |w: &mut Watch| (w.down, w.text_pasted, w.command, w.shift) = (false, false, false, false);
    for e in events {
        match e {
            egui::Event::Key { key, pressed: true, .. } if PASTE_KEYS.contains(key) => w.down = true,
            egui::Event::Copy | egui::Event::Cut => w.down = true,
            egui::Event::Paste(_) => w.text_pasted = true,
            egui::Event::Key { key, pressed: false, modifiers, .. } if PASTE_KEYS.contains(key) => {
                let shortcut = match key {
                    egui::Key::V => w.command || modifiers.command,
                    egui::Key::Insert => (w.shift || modifiers.shift) && !modifiers.command,
                    _ => true,
                };
                w.wanted |= shortcut && !w.down && !w.text_pasted;
                fresh(&mut w);
            }
            egui::Event::Key { modifiers, .. } => {
                w.command |= modifiers.command;
                w.shift |= modifiers.shift;
            }
            // Focus came or went mid-press: start again rather than guess.
            egui::Event::WindowFocused(_) => fresh(&mut w),
            _ => {}
        }
    }
    w
}

/// The paste shortcut was used this pass with something other than text on the clipboard
/// (maybe a picture). The same answer for every call in a pass; call it on every pass a box that
/// takes pictures shows, so no key's press or release is missed.
pub fn picture_wanted(ctx: &egui::Context) -> bool {
    let id = egui::Id::new("paste-watch");
    let pass = ctx.cumulative_pass_nr();
    let before: Watch = ctx.data(|d| d.get_temp(id)).unwrap_or_default();
    if before.pass == pass && pass != 0 {
        return before.wanted;
    }
    let now = Watch { pass, ..ctx.input(|i| step(before, &i.raw.events, i.modifiers)) };
    ctx.data_mut(|d| d.insert_temp(id, now));
    now.wanted
}

/// The pictures on the clipboard, as files: copied picture files as they are, or a copied image
/// saved as a PNG in `dir` (clearing out old pastes not in `keep`). Empty when it holds neither.
/// A copied picture a message can't take (over 20 MB, or not a picture inside whatever its name
/// says) refuses the paste, rather than showing as attached and being left out when the message
/// goes, or sent as a file the builder can't read. Blocking: run it on a worker.
#[cfg(not(target_arch = "wasm32"))]
pub fn read_pictures(dir: &Path, keep: &[PathBuf]) -> Result<Vec<PathBuf>, String> {
    let mut clipboard = arboard::Clipboard::new().map_err(|e| format!("{} {e}", crate::i18n::tr("The clipboard can't be read:")))?;
    let files: Vec<PathBuf> = clipboard.get().file_list().unwrap_or_default().into_iter().filter(|p| craft_anything_engine::workspace::is_picture(p)).collect();
    if !files.is_empty() {
        return attachable(files);
    }
    let Ok(image) = clipboard.get().image() else { return Ok(Vec::new()) };
    let (Ok(width), Ok(height)) = (u32::try_from(image.width), u32::try_from(image.height)) else {
        return Err(crate::i18n::tr("The picture on the clipboard is too large."));
    };
    craft_anything_engine::workspace::save_pasted_picture(dir, width, height, image.bytes.into_owned(), keep).map(|p| vec![p]).map_err(|e| e.message)
}

/// Copied picture files, if a message can take every one; else which one it can't.
#[cfg(not(target_arch = "wasm32"))]
fn attachable(files: Vec<PathBuf>) -> Result<Vec<PathBuf>, String> {
    use craft_anything_engine::workspace::{attachable_picture, picture_content};
    let name = |f: &Path| f.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
    for f in &files {
        if !attachable_picture(f) {
            return Err(crate::i18n::trf("{name} can't be attached: a message takes pictures up to 20 MB.", &[("name", name(f).as_str())]));
        }
        if !picture_content(f) {
            return Err(crate::i18n::trf("{name} can't be attached: it isn't a PNG, JPEG, GIF or WebP picture.", &[("name", name(f).as_str())]));
        }
    }
    Ok(files)
}

#[cfg(test)]
mod tests {
    use super::*;
    use egui_kittest::kittest::Queryable as _;

    const NONE: egui::Modifiers = egui::Modifiers::NONE;

    fn key_with(key: egui::Key, pressed: bool, modifiers: egui::Modifiers) -> egui::Event {
        egui::Event::Key { key, physical_key: None, pressed, repeat: false, modifiers }
    }

    #[test]
    fn copied_picture_files_a_message_cant_take_refuse_the_paste() {
        let dir = tempfile::tempdir().unwrap();
        let small = dir.path().join("small.png");
        std::fs::write(&small, b"\x89PNG\r\n\x1a\n").unwrap();
        let big = dir.path().join("huge screenshot.png");
        std::fs::File::create(&big).unwrap().set_len(20 * 1024 * 1024 + 1).unwrap();
        assert_eq!(attachable(vec![small.clone()]).unwrap(), vec![small.clone()]);
        let refused = attachable(vec![small.clone(), big]).unwrap_err();
        assert!(refused.contains("huge screenshot.png") && refused.contains("20 MB"), "{refused}");
        // A picture's name on something else: the builder couldn't read it.
        let fake = dir.path().join("notes.png");
        std::fs::write(&fake, b"just some text").unwrap();
        let refused = attachable(vec![small, fake]).unwrap_err();
        assert!(refused.contains("notes.png") && refused.contains("isn't a PNG"), "{refused}");
    }

    /// V with Ctrl (Cmd) held.
    fn key(pressed: bool) -> egui::Event {
        key_with(egui::Key::V, pressed, egui::Modifiers::COMMAND)
    }

    #[test]
    fn a_paste_without_text_shows_as_a_v_released_that_never_went_down() {
        // A picture on the clipboard: egui-winit swallows the press, the release comes through.
        assert!(step(Watch::default(), &[key(false)], NONE).wanted);
        // Typing a "v": egui sees both.
        let down = step(Watch::default(), &[key(true)], NONE);
        assert!(!down.wanted && down.down);
        assert!(!step(down, &[key(false)], NONE).wanted);
        // Text on the clipboard: it's pasted as text, and no picture is looked for.
        let text = step(Watch::default(), &[egui::Event::Paste("hi".into())], NONE);
        assert!(!text.wanted);
        assert!(!step(text, &[key(false)], NONE).wanted);
        // The next paste is looked at afresh.
        let after = step(step(text, &[key(false)], NONE), &[key(false)], NONE);
        assert!(after.wanted);
        // A text paste whose release went to another window doesn't hide the next picture.
        let away = step(text, &[egui::Event::WindowFocused(false)], NONE);
        assert!(step(away, &[key(false)], NONE).wanted);
    }

    #[test]
    fn only_the_paste_shortcut_counts() {
        // Win+V (the clipboard history) swallows the press too, but no Ctrl was held.
        assert!(!step(Watch::default(), &[key_with(egui::Key::V, false, NONE)], NONE).wanted);
        // Ctrl released before V: Ctrl was held on an earlier pass.
        let held = step(Watch::default(), &[], egui::Modifiers::COMMAND);
        assert!(step(held, &[key_with(egui::Key::V, false, NONE)], NONE).wanted);
        // Shift+Insert pastes; Ctrl+Insert is a copy, which egui-winit sends as Copy.
        assert!(step(Watch::default(), &[key_with(egui::Key::Insert, false, egui::Modifiers::SHIFT)], NONE).wanted);
        let copy = [egui::Event::Copy, key_with(egui::Key::Insert, false, egui::Modifiers::COMMAND)];
        assert!(!step(Watch::default(), &copy, egui::Modifiers::COMMAND).wanted);
        // A Paste key needs nothing held.
        assert!(step(Watch::default(), &[key_with(egui::Key::Paste, false, NONE)], NONE).wanted);
    }

    /// A prompt box shown each frame; `.1` counts the frames it asked for a pasted picture.
    fn prompt_box(attach: bool) -> egui_kittest::Harness<'static, (String, usize)> {
        egui_kittest::Harness::new_ui_state(
            move |ui, (text, pastes): &mut (String, usize)| {
                let b = crate::prompt_box::PromptBox {
                    name: "Ask for a change",
                    hint: String::new(),
                    rows: (1, 4),
                    attach: attach.then_some("Attach pictures"),
                    previews: &[],
                    stop_button: None,
                    stopping: false,
                    send: true,
                    recall: false,
                    history: None,
                    font_size: 14.0,
                    settings: None,
                };
                if crate::prompt_box::show(ui, text, &[], b).paste {
                    *pastes += 1;
                }
            },
            (String::new(), 0),
        )
    }

    #[test]
    fn ctrl_v_with_a_picture_asks_the_prompt_box_for_a_paste_where_it_takes_pictures() {
        for attach in [true, false] {
            let mut h = prompt_box(attach);
            h.get_by_label("Ask for a change").focus();
            h.run();
            // What egui-winit sends for Ctrl+V with a screenshot on the clipboard: the release only.
            h.event_modifiers(key(false), egui::Modifiers::COMMAND);
            h.run();
            assert_eq!(h.state().1, usize::from(attach), "attach: {attach}");
            assert!(h.state().0.is_empty(), "nothing is typed into the box");
            // Text pastes are left to the text field.
            h.event(egui::Event::Paste("hello".into()));
            h.event(key(false));
            h.run();
            assert_eq!(h.state().1, usize::from(attach));
            assert_eq!(h.state().0, "hello");
        }
    }

    /// Live: puts a picture on this computer's clipboard (replacing what's there) and reads it back.
    #[test]
    #[ignore = "replaces what's on the system clipboard"]
    fn live_a_picture_on_the_clipboard_is_read_back_as_a_png() {
        let mut c = arboard::Clipboard::new().unwrap();
        c.set_image(arboard::ImageData { width: 3, height: 2, bytes: vec![200u8; 24].into() }).unwrap();
        let dir = tempfile::tempdir().unwrap();
        let files = read_pictures(dir.path(), &[]).unwrap();
        assert_eq!(files.len(), 1, "{files:?}");
        let bytes = std::fs::read(&files[0]).unwrap();
        assert_eq!(craft_anything_engine::tools::web_common::png_dimensions(&bytes).unwrap(), (3, 2));
        c.set_text("just text").unwrap();
        assert!(read_pictures(dir.path(), &[]).unwrap().is_empty(), "text isn't a picture");
    }
}
