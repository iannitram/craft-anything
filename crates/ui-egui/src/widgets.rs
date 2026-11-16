//! Small shared widgets drawn from theme tokens: cards, buttons, pills, badges, empty states.

use crate::i18n::tr;
use crate::icons;
use crate::theme::{self, ControlSize, Tokens, confidence_color, confidence_label};
use craft_anything_engine::evidence::Confidence;
use egui::{Color32, Frame, Response, RichText, Sense, Stroke, Ui, Vec2};

/// Lay out `text` in the label style ([`theme::label_text`]) for painting.
pub fn label_galley(ui: &Ui, text: &str, size: f32, color: Color32) -> std::sync::Arc<egui::Galley> {
    egui::WidgetText::from(theme::label_text(text, size).color(color)).into_galley(
        ui,
        Some(egui::TextWrapMode::Extend),
        f32::INFINITY,
        egui::FontSelection::Default,
    )
}

/// 0..1 as the pointer rests on `resp` (a short ease, so hover states don't snap).
pub fn hover_t(ui: &Ui, resp: &Response) -> f32 {
    ui.ctx().animate_bool_with_time(resp.id.with("hover-t"), resp.hovered() || resp.has_focus(), 0.12)
}

/// The frame every text field uses: a field-coloured box with the edge every control has at
/// rest, and roomy padding.
pub fn text_field_frame(t: &Tokens) -> Frame {
    Frame::new().fill(t.field).stroke(Stroke::new(1.0, t.card_border)).corner_radius(t.r()).inner_margin(theme::pad::FIELD)
}

/// The text-field frame for dense panels (filters, offsets, forms).
pub fn text_field_frame_compact(t: &Tokens) -> Frame {
    text_field_frame(t).inner_margin(theme::pad::FIELD_COMPACT)
}

/// A switch with a label, in place of a checkbox. Flips `on` when clicked.
pub fn toggle(ui: &mut Ui, on: &mut bool, label: &str) -> Response {
    let t = Tokens::get(ui.ctx());
    // The label wraps to the room left, so a long one can't push a narrow panel wider.
    let wrap = (ui.available_width() - 42.0).max(80.0);
    let g = (!label.is_empty()).then(|| ui.painter().layout(tr(label), egui::FontId::proportional(theme::SMALL), t.text, wrap));
    let w = 32.0 + g.as_ref().map_or(0.0, |g| g.size().x + 10.0);
    let h = g.as_ref().map_or(ControlSize::Small.height(), |g| g.size().y.max(ControlSize::Small.height()));
    let (rect, mut resp) = ui.allocate_exact_size(Vec2::new(w, h), Sense::click());
    if resp.clicked() {
        *on = !*on;
        resp.mark_changed();
    }
    let how = ui.ctx().animate_bool_with_time(resp.id, *on, 0.12);
    let sw = egui::Rect::from_min_size(egui::pos2(rect.left(), rect.top() + 3.0), Vec2::new(32.0, 18.0));
    ui.painter().rect(sw, 9.0, theme::mix(t.field, t.accent, how), Stroke::new(1.0, theme::mix(t.field_border, t.accent, how)), egui::StrokeKind::Inside);
    let knob = egui::pos2(egui::lerp(sw.left() + 9.0..=sw.right() - 9.0, how), sw.center().y);
    ui.painter().circle_filled(knob, 6.0, theme::mix(t.text_dim, t.on_accent, how));
    if let Some(g) = g {
        ui.painter().galley(egui::pos2(sw.right() + 10.0, rect.center().y - g.size().y / 2.0), g, t.text);
    }
    focus_ring(ui, &resp, sw);
    let state = *on;
    resp.widget_info(|| egui::WidgetInfo::selected(egui::WidgetType::Checkbox, true, state, tr(label)));
    resp.on_hover_cursor(egui::CursorIcon::PointingHand)
}

/// Every dropdown field: egui's combo box with our arrow, and a list whose rows are
/// [`menu_item`]s sitting close together.
pub fn dropdown(id_salt: impl egui::AsIdSalt) -> egui::ComboBox {
    egui::ComboBox::from_id_salt(id_salt).icon(combo_icon).popup_style(egui::style::StyleModifier::from(|s: &mut egui::Style| {
        s.spacing.item_spacing.y = theme::space::XXS;
    }))
}

/// How long a dropdown's list takes to fade in.
const POPUP_FADE: f64 = 0.12;

/// How visible the list `ui` belongs to is as it fades in. The fade starts again when the list
/// wasn't drawn on the frame before (by frame, not by the clock: frames can be far apart).
fn popup_fade(ui: &Ui) -> f32 {
    let (now, frame) = (ui.input(|i| i.time), ui.ctx().cumulative_frame_nr());
    let id = ui.id().with("popup-fade");
    let (since, seen) = ui.data(|d| d.get_temp::<(f64, u64)>(id)).unwrap_or((now, frame));
    let since = if frame.saturating_sub(seen) > 1 { now } else { since };
    ui.data_mut(|d| d.insert_temp(id, (since, frame)));
    let how = ((now - since) / POPUP_FADE).clamp(0.0, 1.0) as f32;
    if how < 1.0 {
        ui.ctx().request_repaint();
    }
    // Never invisible: the first frame already shows where the list is.
    0.35 + 0.65 * how
}

/// One row of a dropdown's list or of a menu: a soft fill eases in under the pointer, and the chosen row has a
/// tick at its end (so it doesn't rely on colour alone). `text` is shown as given.
pub fn menu_item(ui: &mut Ui, selected: bool, text: impl Into<String>) -> Response {
    let t = Tokens::get(ui.ctx());
    let text = text.into();
    ui.set_opacity(popup_fade(ui));
    // Rows sit close together, whatever list they are in.
    ui.spacing_mut().item_spacing.y = theme::space::XXS;
    let size = ControlSize::Medium;
    let galley = ui.painter().layout_no_wrap(text.clone(), egui::FontId::proportional(size.text()), t.text);
    let tick = size.icon() + theme::space::SM;
    let wanted = galley.size().x + theme::space::SM * 2.0 + tick;
    // As wide as the list, which is as wide as its widest row: while the list is being measured
    // a row asks only for what it needs, or a menu would open at its widest.
    let width = if ui.is_sizing_pass() { wanted } else { wanted.max(ui.available_width()) };
    let (rect, resp) = ui.allocate_exact_size(Vec2::new(width, size.height()), Sense::click());
    let h = hover_t(ui, &resp);
    let fill = if resp.is_pointer_button_down_on() { t.pressed } else { theme::mix(Color32::TRANSPARENT, t.hover, h) };
    ui.painter().rect_filled(rect, t.radius_sm, fill);
    let ink = if selected { t.text } else { theme::mix(t.text_dim, t.text, h) };
    ui.painter().galley_with_override_text_color(egui::pos2(rect.left() + theme::space::SM, rect.center().y - galley.size().y / 2.0), galley, ink);
    if selected {
        let at = egui::Rect::from_center_size(egui::pos2(rect.right() - theme::space::SM - size.icon() / 2.0, rect.center().y), Vec2::splat(size.icon()));
        icons::paint(ui, at, "check", size.icon(), t.accent_ink);
    }
    focus_ring(ui, &resp, rect);
    resp.widget_info(|| egui::WidgetInfo::selected(egui::WidgetType::SelectableLabel, true, selected, &text));
    resp.on_hover_cursor(egui::CursorIcon::PointingHand)
}

/// One of a list of things to do with what is shown (a panel's actions): an icon and a label on
/// a row as wide as the list, with a soft fill under the pointer. `danger` letters it in the
/// danger colour, for the one that deletes.
pub fn action_row(ui: &mut Ui, icon: &str, label: &str, danger: bool) -> Response {
    let t = Tokens::get(ui.ctx());
    let size = ControlSize::Medium;
    let galley = ui.painter().layout_no_wrap(tr(label), egui::FontId::proportional(size.text()), t.text);
    let wanted = galley.size().x + size.icon() + theme::space::SM * 3.0;
    let (rect, resp) = ui.allocate_exact_size(Vec2::new(wanted.max(ui.available_width()), size.height()), Sense::click());
    let h = hover_t(ui, &resp);
    let fill = if resp.is_pointer_button_down_on() { t.pressed } else { theme::mix(Color32::TRANSPARENT, t.hover, h) };
    ui.painter().rect_filled(rect, t.radius_sm, fill);
    let ink = if danger { t.danger } else { theme::mix(t.text_dim, t.text, h) };
    let at = egui::Rect::from_min_size(egui::pos2(rect.left() + theme::space::SM, rect.center().y - size.icon() / 2.0), Vec2::splat(size.icon()));
    icons::paint(ui, at, icon, size.icon(), if danger { t.danger } else { theme::mix(t.icon, t.text, h) });
    ui.painter().galley_with_override_text_color(egui::pos2(at.right() + theme::space::SM, rect.center().y - galley.size().y / 2.0), galley, ink);
    focus_ring(ui, &resp, rect);
    resp.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Button, true, tr(label)));
    resp.on_hover_cursor(egui::CursorIcon::PointingHand)
}

/// A [`menu_item`] that sets `current` to `value` when picked. True when that changed it.
pub fn menu_choice<V: PartialEq>(ui: &mut Ui, current: &mut V, value: V, text: impl Into<String>) -> bool {
    let on = *current == value;
    let picked = menu_item(ui, on, text).clicked() && !on;
    if picked {
        *current = value;
    }
    picked
}

/// The dropdown arrow for every combo box: a Lucide chevron instead of egui's filled triangle.
pub fn combo_icon(ui: &Ui, rect: egui::Rect, _visuals: &egui::style::WidgetVisuals, _open: bool) {
    icons::paint(ui, rect.expand(2.0), "chevron-down", theme::icon::SM, Tokens::get(ui.ctx()).icon);
}

/// The disclosure arrow for every collapsing section: a Lucide chevron.
pub fn disclosure_icon(ui: &mut Ui, openness: f32, resp: &Response) {
    let t = Tokens::get(ui.ctx());
    let r = egui::Rect::from_center_size(resp.rect.center(), Vec2::splat(12.0));
    icons::paint(ui, r, if openness > 0.5 { "chevron-down" } else { "chevron-right" }, theme::icon::XS, t.text_faint);
}

/// Give a field without a visible label a name for screen readers (WCAG 4.1.2); egui doesn't
/// use the hint text for that.
pub fn named(resp: Response, name: &str) -> Response {
    resp.ctx.accesskit_node_builder(resp.id, |b| b.set_label(tr(name)));
    resp
}

/// A suggestion: a sentence in a hairline box that fills in a field when clicked. Wraps when
/// it's wider than the space it has; fills the width in a justified layout (a tidy list).
pub fn suggestion(ui: &mut Ui, text: &str) -> Response {
    let t = Tokens::get(ui.ctx());
    let max_w = ui.available_width().max(60.0);
    let g = ui.painter().layout(tr(text), egui::FontId::proportional(theme::SMALL), t.text, max_w - 26.0);
    let w = if ui.layout().horizontal_justify() { max_w } else { (g.size().x + 26.0).min(max_w) };
    let (rect, resp) = ui.allocate_exact_size(Vec2::new(w, (g.size().y + 14.0).max(32.0)), Sense::click());
    let h = hover_t(ui, &resp);
    ui.painter().rect(
        rect,
        t.radius,
        theme::mix(t.field, t.hover, h),
        Stroke::new(1.0, theme::mix(t.card_border, t.field_border, h)),
        egui::StrokeKind::Inside,
    );
    let pos = egui::pos2(rect.left() + 13.0, rect.center().y - g.size().y / 2.0);
    ui.painter().galley_with_override_text_color(pos, g, theme::mix(t.text_dim, t.text, h));
    focus_ring(ui, &resp, rect);
    resp.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Button, true, tr(text)));
    resp.on_hover_cursor(egui::CursorIcon::PointingHand)
}

/// A big, radio-like choice with an icon and plain words (and an optional line under them,
/// empty for none); the chosen one is in the app colour with a tick (so it doesn't rely on
/// colour alone). It grows taller when the words need it.
pub fn choice_tile(ui: &mut Ui, size: Vec2, icon: &str, label: &str, blurb: &str, on: bool) -> Response {
    let t = Tokens::get(ui.ctx());
    let wrap = size.x - 84.0;
    let g = ui.painter().layout(tr(label), theme::medium(theme::BODY), t.text, wrap);
    let sub = (!blurb.is_empty()).then(|| ui.painter().layout(tr(blurb), egui::FontId::proportional(theme::SMALL), t.text_dim, wrap));
    let text_h = g.size().y + sub.as_ref().map_or(0.0, |s| s.size().y + 3.0);
    let (rect, resp) = ui.allocate_exact_size(Vec2::new(size.x, size.y.max(text_h + 24.0)), Sense::click());
    let h = hover_t(ui, &resp);
    let (fill, stroke) = if on {
        (t.accent_soft, Stroke::new(2.0, t.accent))
    } else {
        (theme::mix(t.field, t.hover, h), Stroke::new(1.0, theme::mix(t.card_border, t.field_border, h)))
    };
    ui.painter().rect(rect, t.radius, fill, stroke, egui::StrokeKind::Inside);
    let icon_rect = egui::Rect::from_center_size(egui::pos2(rect.left() + 28.0, rect.center().y), Vec2::splat(24.0));
    icons::paint(ui, icon_rect, icon, theme::icon::XL, if on { t.accent_ink } else { t.icon });
    let top = egui::pos2(icon_rect.right() + 12.0, rect.center().y - text_h / 2.0);
    let title_h = g.size().y;
    ui.painter().galley(top, g, t.text);
    if let Some(s) = sub {
        ui.painter().galley(top + Vec2::new(0.0, title_h + 3.0), s, t.text_dim);
    }
    if on {
        icons::paint(
            ui,
            egui::Rect::from_center_size(egui::pos2(rect.right() - 20.0, rect.center().y), Vec2::splat(18.0)),
            "check",
            theme::icon::LG,
            t.accent_ink,
        );
    }
    focus_ring(ui, &resp, rect);
    resp.widget_info(|| egui::WidgetInfo::selected(egui::WidgetType::RadioButton, true, on, tr(label)));
    resp.on_hover_cursor(egui::CursorIcon::PointingHand)
}

/// A visible keyboard-focus ring around `rect` when `resp` has focus: a hairline in the app
/// colour on the control's edge and a soft 3 px halo outside it.
pub fn focus_ring(ui: &Ui, resp: &Response, rect: egui::Rect) {
    if resp.has_focus() {
        let t = Tokens::get(ui.ctx());
        ui.painter().rect_stroke(rect, t.radius, Stroke::new(3.0, t.accent_ink.gamma_multiply(0.45)), egui::StrokeKind::Outside);
        ui.painter().rect_stroke(rect, t.radius, Stroke::new(1.0, t.accent_ink), egui::StrokeKind::Inside);
    }
}

/// How long a focus edge takes to fade in or out.
pub const FOCUS_FADE: f32 = 0.12;

/// A text field with no box of its own (it sits in a bigger one that shows focus itself): the
/// ring [`text_focus_ring`] draws would hug its bare text, so it gets none.
pub fn frameless_field(resp: &Response) {
    resp.ctx.data_mut(|d| d.insert_temp(resp.id.with("frameless-field"), true));
}

/// The focus edge of whichever text field is being typed in: the same hairline and halo as
/// [`focus_ring`], faded in and out. Drawn once a frame, after the screens, because a field's
/// own frame can't know it has focus.
pub fn text_focus_ring(ctx: &egui::Context) {
    let key = egui::Id::new("text-focus-ring");
    let focused =
        ctx.memory(|m| m.focused()).filter(|id| ctx.text_edit_focused() && !ctx.data(|d| d.get_temp::<bool>(id.with("frameless-field")).unwrap_or(false)));
    // The field that had it last fades out.
    let Some(id) = focused.or_else(|| ctx.data(|d| d.get_temp::<egui::Id>(key))) else { return };
    let how = ctx.animate_bool_with_time(id.with("focus-ring"), focused == Some(id), FOCUS_FADE);
    match focused {
        Some(now) => {
            ctx.data_mut(|d| d.insert_temp(key, now));
        }
        None if how <= 0.0 => ctx.data_mut(|d| d.remove::<egui::Id>(key)),
        None => {}
    }
    let Some(resp) = ctx.read_response(id).filter(|_| how > 0.0) else { return };
    let t = Tokens::get(ctx);
    // Clipped to what the field's own layer shows of it (a field half scrolled out of a list).
    let painter = ctx.layer_painter(resp.layer_id).with_clip_rect(resp.interact_rect.expand(4.0));
    painter.rect_stroke(resp.rect, t.radius, Stroke::new(3.0, t.accent_ink.gamma_multiply(0.45 * how)), egui::StrokeKind::Outside);
    painter.rect_stroke(resp.rect, t.radius, Stroke::new(1.0, t.accent_ink.gamma_multiply(how)), egui::StrokeKind::Inside);
}

/// A card frame: a soft rounded box with a hairline edge.
pub fn card(t: &Tokens) -> Frame {
    Frame::new().fill(t.card).stroke(Stroke::new(1.0, t.card_border)).corner_radius(t.r_lg()).inner_margin(theme::pad::CARD)
}

/// A quieter, denser panel frame for the workbench.
pub fn panel(t: &Tokens) -> Frame {
    Frame::new().fill(t.card).stroke(Stroke::new(1.0, t.card_border)).corner_radius(t.r()).inner_margin(theme::pad::PANEL)
}

/// What a button is for, which decides its fill and ink.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ButtonKind {
    /// The main action of a screen or card: ink-filled (light on dark in the dark theme).
    Primary,
    /// Any other action: a card-coloured box with a soft edge.
    Secondary,
    /// An action that stays quiet until it's wanted: no box until the pointer is on it.
    Ghost,
    /// An action that deletes or can't be undone: filled in the danger colour.
    Danger,
}

/// Every button in the app: one of four kinds, at one of the control sizes, with an optional icon.
#[must_use = "put it in a ui with `ui.add(widget);`"]
pub struct Button<'a> {
    kind: ButtonKind,
    size: ControlSize,
    icon: Option<&'a str>,
    label: &'a str,
    enabled: bool,
    wide: bool,
}

impl<'a> Button<'a> {
    /// `label` is translated here.
    pub fn new(kind: ButtonKind, label: &'a str) -> Self {
        Self { kind, size: ControlSize::Medium, icon: None, label, enabled: true, wide: false }
    }

    /// As wide as the room it has (the one action of a narrow panel).
    pub fn wide(mut self) -> Self {
        self.wide = true;
        self
    }

    pub fn icon(mut self, icon: Option<&'a str>) -> Self {
        self.icon = icon;
        self
    }

    pub fn size(mut self, size: ControlSize) -> Self {
        self.size = size;
        self
    }

    pub fn enabled(mut self, enabled: bool) -> Self {
        self.enabled = enabled;
        self
    }
}

impl egui::Widget for Button<'_> {
    fn ui(self, ui: &mut Ui) -> Response {
        let Button { kind, size, icon, label, enabled, wide } = self;
        let t = Tokens::get(ui.ctx());
        let galley = label_galley(ui, &tr(label), size.text(), t.text);
        let icon_w = if icon.is_some() { size.icon() + ICON_GAP } else { 0.0 };
        // Taller than its size only when the label is (a zoomed or fallback font).
        let wanted = Vec2::new(galley.size().x + icon_w + size.pad_x() * 2.0, (galley.size().y + theme::space::SM).max(size.height()));
        let wanted = if wide { Vec2::new(wanted.x.max(ui.available_width()), wanted.y) } else { wanted };
        let (rect, resp) = ui.allocate_exact_size(wanted, if enabled { Sense::click() } else { Sense::hover() });
        let h = if enabled { hover_t(ui, &resp) } else { 0.0 };
        let down = enabled && resp.is_pointer_button_down_on();
        let clear = Color32::TRANSPARENT;
        // Fill, edge, label, icon.
        let (fill, edge, fg, tint) = match kind {
            ButtonKind::Primary if !enabled => (t.pressed, clear, t.text_faint, t.text_faint),
            _ if !enabled => (clear, if kind == ButtonKind::Ghost { clear } else { t.card_border }, t.text_faint, t.text_faint),
            ButtonKind::Primary => (theme::mix(t.text, t.canvas, if down { 0.25 } else { 0.14 * h }), clear, t.canvas, t.canvas),
            ButtonKind::Secondary => {
                (if down { t.pressed } else { theme::mix(t.field, t.hover, h) }, theme::mix(t.card_border, t.field_border, h), t.text, t.icon)
            }
            ButtonKind::Ghost => {
                let ink = theme::mix(t.text_dim, t.text, h);
                (if down { t.pressed } else { theme::mix(clear, t.hover, h) }, clear, ink, ink)
            }
            ButtonKind::Danger => (theme::mix(t.danger, t.canvas, if down { 0.2 } else { 0.1 * h }), clear, t.canvas, t.canvas),
        };
        ui.painter().rect(rect, t.radius, fill, Stroke::new(1.0, edge), egui::StrokeKind::Inside);
        let mut x = (rect.center().x - (icon_w + galley.size().x) / 2.0).round();
        if let Some(i) = icon {
            icons::paint(ui, egui::Rect::from_min_size(egui::pos2(x, rect.center().y - size.icon() / 2.0), Vec2::splat(size.icon())), i, size.icon(), tint);
            x += icon_w;
        }
        ui.painter().galley_with_override_text_color(egui::pos2(x, rect.center().y - galley.size().y / 2.0), galley, fg);
        focus_ring(ui, &resp, rect);
        resp.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Button, enabled, tr(label)));
        if enabled { resp.on_hover_cursor(egui::CursorIcon::PointingHand) } else { resp }
    }
}

/// The gap between a button's icon and its label.
const ICON_GAP: f32 = 6.0;

/// The main action: an ink-filled button (light on dark in the dark theme).
pub fn primary(ui: &mut Ui, label: &str) -> Response {
    primary_icon(ui, None, label, true)
}

pub fn primary_icon(ui: &mut Ui, icon: Option<&str>, label: &str, enabled: bool) -> Response {
    ui.add(Button::new(ButtonKind::Primary, label).icon(icon).enabled(enabled))
}

/// Secondary button: a card-coloured box with a soft edge and an optional icon.
pub fn secondary(ui: &mut Ui, icon: Option<&str>, label: &str) -> Response {
    ui.add(Button::new(ButtonKind::Secondary, label).icon(icon))
}

/// A quiet button: no box until the pointer is on it.
pub fn ghost(ui: &mut Ui, icon: Option<&str>, label: &str) -> Response {
    ui.add(Button::new(ButtonKind::Ghost, label).icon(icon))
}

/// A button for something that deletes or can't be undone.
pub fn danger(ui: &mut Ui, icon: Option<&str>, label: &str) -> Response {
    ui.add(Button::new(ButtonKind::Danger, label).icon(icon))
}

/// A borderless text button in the accent colour.
pub fn link(ui: &mut Ui, label: &str) -> Response {
    let t = Tokens::get(ui.ctx());
    let r = ui.add(egui::Label::new(RichText::new(tr(label)).color(t.accent_ink)).sense(Sense::click()));
    let r = r.on_hover_cursor(egui::CursorIcon::PointingHand);
    if r.hovered() {
        let y = r.rect.bottom();
        ui.painter().hline(r.rect.x_range(), y, Stroke::new(1.0, t.accent_ink));
    }
    r
}

/// A [`link`] at the size of the caption it sits in.
pub fn caption_link(ui: &mut Ui, label: &str) -> Response {
    ui.scope(|ui| {
        ui.style_mut().override_font_id = Some(egui::FontId::proportional(theme::CAPTION));
        link(ui, label)
    })
    .inner
}

/// A segmented switch; the selected segment is a raised card on a sunken track. Returns the newly
/// picked index when it changes.
pub fn segmented(ui: &mut Ui, options: &[&str], selected: usize, size: ControlSize) -> Option<usize> {
    segmented_scaled(ui, options, selected, size.height(), size.text(), 1.0)
}

/// [`segmented`] at a given height and label size, with every size in it, padding too, multiplied
/// by `scale` (the macOS title bar's, which keeps its size when the app zooms).
pub fn segmented_scaled(ui: &mut Ui, options: &[&str], selected: usize, height: f32, text: f32, scale: f32) -> Option<usize> {
    let t = Tokens::get(ui.ctx());
    let (height, pad) = (height * scale, SEGMENT_INSET * scale);
    let galleys: Vec<_> = options.iter().map(|o| label_galley(ui, &tr(o), text * scale, t.text)).collect();
    let widths: Vec<f32> = galleys.iter().map(|g| g.size().x + segment_pad(text) * scale).collect();
    let total: f32 = widths.iter().sum::<f32>() + 2.0 * pad;
    let (rect, _) = ui.allocate_exact_size(Vec2::new(total, height), Sense::hover());
    // A sunken track; the chosen segment is a lighter card on it.
    ui.painter().rect(rect, t.radius, t.canvas, Stroke::new(scale, t.card_border), egui::StrokeKind::Inside);
    // The segments. One under the pointer only brightens its words: the card is what moves.
    let mut x = rect.left() + pad;
    let mut segments = Vec::with_capacity(options.len());
    for (i, w) in widths.iter().enumerate() {
        let seg = egui::Rect::from_min_size(egui::pos2(x, rect.top() + pad), Vec2::new(*w, height - 2.0 * pad));
        let name = options.get(i).copied().unwrap_or_default();
        let resp = ui.interact(seg, ui.id().with(("seg", name)), Sense::click());
        let on = i == selected;
        resp.widget_info(|| egui::WidgetInfo::selected(egui::WidgetType::SelectableLabel, true, on, tr(name)));
        segments.push((seg, resp));
        x += w;
    }
    // The card slides to the chosen segment (measured from the switch's own
    // left edge, so moving the whole switch doesn't set it off). Named by its options, not by
    // where it is drawn, so it keeps its place when the screen around it changes.
    let before: f32 = widths.iter().take(selected).sum();
    let slide = |part: &str, to: f32| ui.ctx().animate_value_with_time(egui::Id::new(("seg-slide", part, options)), to, SLIDE);
    let (card_x, card_w) = (slide("x", before), slide("w", widths.get(selected).copied().unwrap_or_default()));
    let card = egui::Rect::from_min_size(egui::pos2(rect.left() + pad + card_x, rect.top() + pad), Vec2::new(card_w, height - 2.0 * pad));
    ui.painter().rect(card, t.radius_sm, t.field, Stroke::new(scale, t.card_border), egui::StrokeKind::Inside);
    let mut picked = None;
    for (i, ((seg, resp), g)) in segments.into_iter().zip(galleys).enumerate() {
        let on = i == selected;
        let ink = if on { t.text } else { theme::mix(t.text_dim, t.text, hover_t(ui, &resp)) };
        ui.painter().galley_with_override_text_color(seg.center() - g.size() / 2.0, g, ink);
        focus_ring(ui, &resp, seg);
        let resp = if on { resp } else { resp.on_hover_cursor(egui::CursorIcon::PointingHand) };
        if resp.clicked() && !on {
            picked = Some(i);
        }
    }
    picked
}

/// The room between the track's edge and the chosen segment's card.
const SEGMENT_INSET: f32 = 4.0;

/// How long the chosen segment's card takes to slide to another segment.
const SLIDE: f32 = 0.2;

/// The room around a segment's label, both sides together.
fn segment_pad(text: f32) -> f32 {
    text * 2.0
}

/// How wide [`segmented`] draws `options` at `size`, for a row that needs to know before drawing them.
pub fn segmented_width(ui: &Ui, options: &[&str], size: ControlSize) -> f32 {
    let t = Tokens::get(ui.ctx());
    options.iter().map(|o| label_galley(ui, &tr(o), size.text(), t.text).size().x + segment_pad(size.text())).sum::<f32>() + 2.0 * SEGMENT_INSET
}

/// A small status label on a tint.
pub fn pill(ui: &mut Ui, text: &str, fg: Color32, bg: Color32) -> Response {
    pill_sized(ui, text, fg, bg, theme::CAPTION)
}

/// [`pill`] with a given label size, its padding scaled to match.
pub fn pill_sized(ui: &mut Ui, text: &str, fg: Color32, bg: Color32, text_size: f32) -> Response {
    let label = tr(text);
    let g = label_galley(ui, &label, text_size, fg);
    let size = Vec2::new(g.size().x + text_size * 18.0 / 11.0, g.size().y + text_size * 6.0 / 11.0);
    let (rect, resp) = ui.allocate_exact_size(size, Sense::hover());
    // A full pill, whatever its size.
    ui.painter().rect_filled(rect, rect.height() / 2.0, bg);
    ui.painter().galley_with_override_text_color(rect.center() - g.size() / 2.0, g, fg);
    // Painted text: say it to screen readers too.
    resp.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Label, true, &label));
    resp
}

/// The "how sure are we" badge: a coloured dot and the plain label.
pub fn confidence_badge(ui: &mut Ui, c: Option<Confidence>) -> Response {
    let col = confidence_color(c);
    let font = theme::medium(theme::CAPTION);
    let g = ui.painter().layout_no_wrap(tr(confidence_label(c)), font, col);
    let size = Vec2::new(g.size().x + 26.0, g.size().y + 7.0);
    let (rect, resp) = ui.allocate_exact_size(size, Sense::hover());
    let bg = col.gamma_multiply(0.13);
    ui.painter().rect(rect, rect.height() / 2.0, bg, Stroke::new(1.0, col.gamma_multiply(0.45)), egui::StrokeKind::Inside);
    ui.painter().circle_filled(egui::pos2(rect.left() + 10.0, rect.center().y), 3.5, col);
    ui.painter().galley_with_override_text_color(egui::pos2(rect.left() + 18.0, rect.center().y - g.size().y / 2.0), g, col);
    let tip = match c {
        Some(Confidence::Observed) => "Seen it: read directly from the file, page or capture.",
        Some(Confidence::Derived) => "Worked it out: computed step by step from things we saw.",
        Some(Confidence::Inferred) => "Best guess: a reasonable reading, but not proven.",
        None => "Unknown: we couldn't establish this yet.",
    };
    resp.on_hover_text(tr(tip))
}

/// A section caption in the label style, faint.
pub fn caption(ui: &mut Ui, text: &str) {
    let t = Tokens::get(ui.ctx());
    ui.label(theme::label_text(&tr(text), theme::LABEL).color(t.text_faint));
}

/// A caption on the left and its value on the right, or the value on a line of its own when the
/// two would overlap: a right-to-left layout doesn't make room for what's already on the row.
pub fn stat_row(ui: &mut Ui, label: &str, value: &str) -> Response {
    let t = Tokens::get(ui.ctx());
    let text = RichText::new(value).font(theme::mono(theme::CODE)).color(t.text);
    let value_width =
        egui::WidgetText::from(text.clone()).into_galley(ui, Some(egui::TextWrapMode::Extend), f32::INFINITY, egui::FontSelection::Default).size().x;
    let caption_width = label_galley(ui, &tr(label), theme::LABEL, t.text_faint).size().x;
    if caption_width + ui.spacing().item_spacing.x + value_width <= ui.available_width() {
        ui.horizontal(|ui| {
            caption(ui, label);
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| ui.label(text));
        })
        .response
    } else {
        ui.vertical(|ui| {
            caption(ui, label);
            ui.with_layout(egui::Layout::top_down(egui::Align::Max), |ui| ui.label(text));
        })
        .response
    }
}

/// A centred, calm empty state.
pub fn empty_state(ui: &mut Ui, icon: &str, title: &str, body: &str) {
    let t = Tokens::get(ui.ctx());
    ui.vertical_centered(|ui| {
        ui.add_space(theme::space::LG);
        let (rect, _) = ui.allocate_exact_size(Vec2::splat(44.0), Sense::hover());
        ui.painter().circle_filled(rect.center(), 22.0, t.raised);
        icons::paint(ui, rect, icon, theme::icon::XL, t.text_faint);
        ui.add_space(theme::space::SM);
        ui.label(RichText::new(tr(title)).font(theme::medium(theme::SMALL)).color(t.text_dim));
        if !body.is_empty() {
            ui.label(RichText::new(tr(body)).color(t.text_faint).size(theme::CAPTION));
        }
        ui.add_space(theme::space::MD);
    });
}

/// A big clickable card with an icon, title and blurb (Beginner Home).
pub fn big_card(ui: &mut Ui, size: Vec2, icon: &str, title: &str, blurb: &str, badge: Option<&str>) -> Response {
    let t = Tokens::get(ui.ctx());
    let (rect, resp) = ui.allocate_exact_size(size, Sense::click());
    let hovered = resp.hovered();
    let fill = t.card;
    let stroke = Stroke::new(1.0, if hovered { t.field_border } else { t.card_border });
    if hovered {
        ui.painter().add(egui::Shadow { offset: [0, 8], blur: 24, spread: 0, color: t.shadow }.as_shape(rect, t.r_lg()));
    }
    ui.painter().rect(rect, t.radius_lg, fill, stroke, egui::StrokeKind::Inside);
    let icon_rect = egui::Rect::from_min_size(rect.min + Vec2::new(22.0, 22.0), Vec2::splat(52.0));
    ui.painter().rect_filled(icon_rect, t.radius, t.accent_soft);
    icons::paint(ui, icon_rect, icon, theme::icon::XL, t.accent_ink);
    if let Some(b) = badge {
        let g = ui.painter().layout_no_wrap(tr(b), theme::medium(theme::TINY), t.text_dim);
        let br = egui::Rect::from_min_size(egui::pos2(rect.right() - g.size().x - 34.0, rect.top() + 22.0), g.size() + Vec2::new(16.0, 8.0));
        ui.painter().rect_filled(br, t.r_sm(), t.hover);
        ui.painter().galley_with_override_text_color(br.center() - g.size() / 2.0, g, t.text_dim);
    }
    let title_pos = egui::pos2(rect.left() + 22.0, icon_rect.bottom() + 18.0);
    // Titles wrap: the display face is wide, and translations are longer.
    let title_g = theme::heading_galley(ui.painter(), tr(title), theme::CARD, t.text, rect.width() - 44.0);
    let title_h = title_g.size().y;
    ui.painter().galley(title_pos, title_g, t.text);
    let wrap = ui.painter().layout(tr(blurb), egui::FontId::proportional(theme::SMALL), t.text_dim, rect.width() - 44.0);
    ui.painter().galley(title_pos + Vec2::new(0.0, title_h + 8.0), wrap, t.text_dim);
    focus_ring(ui, &resp, rect);
    resp.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Button, true, tr(title)));
    resp.on_hover_cursor(egui::CursorIcon::PointingHand)
}

/// A dotted underline under a hoverable glossary word.
pub fn glossary_word(ui: &mut Ui, word: &str, meaning: &str, font: egui::FontId) -> Response {
    let t = Tokens::get(ui.ctx());
    let resp = ui.add(egui::Label::new(RichText::new(word).font(font).color(t.text)).sense(Sense::hover()));
    let y = resp.rect.bottom() - 1.0;
    let mut x = resp.rect.left();
    let trimmed_right = resp.rect.right() - if word.ends_with(' ') { 4.0 } else { 0.0 };
    while x < trimmed_right {
        ui.painter().hline(x..=(x + 2.0).min(trimmed_right), y, Stroke::new(1.0, t.accent));
        x += 4.0;
    }
    resp.on_hover_ui(|ui| {
        ui.label(RichText::new(word.trim()).font(theme::semibold(theme::SMALL)));
        ui.label(tr(meaning));
    })
}

/// Draw an animation's next frame in `every` (about 15 a second is smooth enough for a spinner),
/// or once a second while the window is in the background: a build can run for an hour, and
/// redrawing the whole window 30 times a second for it costs the battery.
pub fn animate(ui: &Ui, every: std::time::Duration) {
    let focused = ui.input(|i| i.viewport().focused.unwrap_or(true));
    ui.ctx().request_repaint_after(if focused { every } else { std::time::Duration::from_secs(1) });
}

/// One frame of an animation at the usual pace.
pub const FRAME: std::time::Duration = std::time::Duration::from_millis(66);

/// A bar's indeterminate sweep at `now` (seconds): a lit quarter that runs along `bar` over its
/// track, for work that is moving but can't say how far it has got.
fn sweep(painter: &egui::Painter, bar: egui::Rect, now: f64, track: Color32, lit: Color32) {
    let radius = bar.height() / 2.0;
    painter.rect_filled(bar, radius, track);
    let seg = bar.width() * 0.25;
    let x = bar.left() - seg + (bar.width() + seg) * ((now * 0.6).fract() as f32);
    let part = egui::Rect::from_x_y_ranges(x.max(bar.left())..=(x + seg).min(bar.right()), bar.y_range());
    if part.width() > 0.0 {
        painter.rect_filled(part, radius, lit);
    }
}

/// A progress bar: filled to `fraction`, or, when that isn't known, a sweep that asks for its next
/// frame at [`animate`]'s pace. egui's animated `ProgressBar` asks to be drawn again at once, every
/// frame, like its spinner (#579), so a job of unknown length redrew the window at the display's
/// rate for as long as it ran, even in the background.
#[must_use = "put it in a ui with `ui.add(widget);`"]
pub struct ProgressBar {
    fraction: Option<f32>,
    size: Vec2,
    color: Option<Color32>,
}

impl ProgressBar {
    /// `fraction` from 0 to 1, or `None` when how far the work has got isn't known.
    pub fn new(fraction: Option<f32>) -> Self {
        Self { fraction, size: Vec2::new(160.0, 8.0), color: None }
    }

    pub fn size(mut self, size: Vec2) -> Self {
        self.size = size;
        self
    }

    pub fn color(mut self, color: Color32) -> Self {
        self.color = Some(color);
        self
    }
}

impl egui::Widget for ProgressBar {
    fn ui(self, ui: &mut Ui) -> Response {
        let (rect, response) = ui.allocate_exact_size(self.size, Sense::hover());
        let fraction = self.fraction.filter(|f| f.is_finite()).map(|f| f.clamp(0.0, 1.0));
        response.widget_info(|| {
            let mut info = egui::WidgetInfo::new(egui::WidgetType::ProgressIndicator);
            info.value = fraction.map(|f| f64::from(f) * 100.0);
            info
        });
        if ui.is_rect_visible(rect) {
            let track = ui.visuals().extreme_bg_color;
            let color = self.color.unwrap_or_else(|| ui.visuals().selection.bg_fill);
            match fraction {
                Some(f) => {
                    let radius = rect.height() / 2.0;
                    ui.painter().rect_filled(rect, radius, track);
                    let fill = egui::Rect::from_min_size(rect.min, Vec2::new(rect.width() * f, rect.height()));
                    if fill.width() > 0.0 {
                        ui.painter().rect_filled(fill, radius, color);
                    }
                }
                None => {
                    sweep(ui.painter(), rect, ui.input(|i| i.time), track, color);
                    animate(ui, FRAME);
                }
            }
        }
        response
    }
}

/// A spinning arc for "working", drawn like egui's own `Spinner` but asking for its next frame at
/// [`animate`]'s pace. egui's asks to be drawn again at once, every frame, for as long as it's on
/// screen: a build or a page capture can run for an hour, and redrawing the whole window at the
/// display's rate for it kept a CPU core (or, drawing in software, all of them) busy (#579).
#[must_use = "put it in a ui with `ui.add(widget);`"]
#[derive(Default)]
pub struct Spinner {
    size: Option<f32>,
    color: Option<Color32>,
}

impl Spinner {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn size(mut self, size: f32) -> Self {
        self.size = Some(size);
        self
    }

    pub fn color(mut self, color: Color32) -> Self {
        self.color = Some(color);
        self
    }
}

impl egui::Widget for Spinner {
    fn ui(self, ui: &mut Ui) -> Response {
        let size = self.size.unwrap_or_else(|| ui.style().spacing.interact_size.y);
        let (rect, response) = ui.allocate_exact_size(Vec2::splat(size), Sense::hover());
        response.widget_info(|| egui::WidgetInfo::new(egui::WidgetType::ProgressIndicator));
        if ui.is_rect_visible(rect) {
            let color = self.color.unwrap_or_else(|| ui.visuals().strong_text_color());
            let radius = (rect.height().min(rect.width()) / 2.0) - 2.0;
            let n_points = (radius.round() as u32).clamp(8, 128);
            let time = ui.input(|i| i.time);
            let start = time * std::f64::consts::TAU;
            let end = start + 240f64.to_radians() * time.sin();
            let points: Vec<egui::Pos2> = (0..n_points)
                .map(|i| {
                    let angle = start + (end - start) * (i as f64 / n_points as f64);
                    let (sin, cos) = angle.sin_cos();
                    rect.center() + radius * egui::vec2(cos as f32, sin as f32)
                })
                .collect();
            ui.painter().add(egui::Shape::line(points, Stroke::new(3.0, color)));
            animate(ui, FRAME);
        }
        response
    }
}

/// A spinner-ish dot row for "thinking".
pub fn thinking(ui: &mut Ui) {
    let t = Tokens::get(ui.ctx());
    let time = ui.input(|i| i.time);
    let (rect, _) = ui.allocate_exact_size(Vec2::new(36.0, 14.0), Sense::hover());
    for i in 0..3 {
        let phase = ((time * 3.0 - i as f64 * 0.5).sin() * 0.5 + 0.5) as f32;
        let c = t.accent.gamma_multiply(0.35 + 0.65 * phase);
        ui.painter().circle_filled(egui::pos2(rect.left() + 6.0 + i as f32 * 12.0, rect.center().y), 3.5, c);
    }
    animate(ui, FRAME);
}

/// How long a run may say nothing before the note points it out.
const QUIET_NOTE_AFTER: std::time::Duration = std::time::Duration::from_secs(60);

fn wait_note(secs: u64, quiet: Option<std::time::Duration>) -> Option<String> {
    match quiet {
        Some(q) if q >= QUIET_NOTE_AFTER => Some(crate::i18n::trf(
            "Nothing new for {time}. Long steps can be quiet; if it stays like this, press Pause.",
            &[("time", &crate::usage::duration_words(q))],
        )),
        Some(_) => None,
        None => match secs {
            0..20 => None,
            20..90 => Some(tr("Still working. Bigger jobs can take a few minutes.")),
            _ => Some(tr("This is a big one. You can leave this open and come back later.")),
        },
    }
}

/// "Still working" for waits longer than a moment: a spinning arc, what's happening, how long
/// it has taken, and a calm note once it runs long (so a quiet wait never looks stuck).
/// `title` is translated here; `detail` arrives already in the user's words.
pub fn working(ui: &mut Ui, title: &str, detail: Option<&str>) {
    working_since(ui, title, detail, None, None);
}

/// [`working`] with its clock from `began` (a build's start), so it doesn't restart each time the
/// row comes back; without it, from when the row was last shown.
///
/// `quiet` is how long since the work last had anything to say, when that's known: then the note
/// says so plainly, because a calm note can't tell a long step from one that has stopped.
pub fn working_since(ui: &mut Ui, title: &str, detail: Option<&str>, began: Option<std::time::Instant>, quiet: Option<std::time::Duration>) {
    let t = Tokens::get(ui.ctx());
    let now = ui.input(|i| i.time);
    // The clock restarts when the row wasn't on screen a moment ago.
    let id = ui.id().with("working-since");
    let (since, seen) = ui.data(|d| d.get_temp::<(f64, f64)>(id)).unwrap_or((now, now));
    let since = if now - seen > 1.0 { now } else { since };
    ui.data_mut(|d| d.insert_temp(id, (since, now)));
    let secs = began.map_or((now - since) as u64, |b| b.elapsed().as_secs());
    let note = wait_note(secs, quiet);
    let sub = [detail.map(str::to_string), note].into_iter().flatten().collect::<Vec<_>>().join("  ·  ");
    let clock = RichText::new(crate::usage::duration_words(std::time::Duration::from_secs(secs))).font(theme::mono(theme::CODE)).color(t.text_faint);
    // The words wrap before the clock, never under it: a right-to-left layout doesn't make room
    // for what's already on the row.
    let clock_width =
        egui::WidgetText::from(clock.clone()).into_galley(ui, Some(egui::TextWrapMode::Extend), f32::INFINITY, egui::FontSelection::Default).size().x;
    card(&t).inner_margin(theme::pad::ROW).show(ui, |ui| {
        ui.set_width(ui.available_width());
        ui.horizontal(|ui| {
            let (r, _) = ui.allocate_exact_size(Vec2::splat(24.0), Sense::hover());
            let (c, radius) = (r.center(), 9.0);
            ui.painter().circle_stroke(c, radius, Stroke::new(2.5, t.separator));
            let start = (now * 4.0) as f32;
            let sweep = std::f32::consts::PI * (0.9 + 0.5 * (now * 1.7).sin() as f32);
            let arc = (0..=20).map(|i| c + radius * Vec2::angled(start + sweep * i as f32 / 20.0)).collect();
            ui.painter().add(egui::Shape::line(arc, Stroke::new(2.5, t.accent)));
            ui.add_space(theme::space::XS);
            ui.vertical(|ui| {
                ui.set_max_width((ui.available_width() - clock_width - ui.spacing().item_spacing.x).max(0.0));
                ui.label(RichText::new(tr(title)).font(theme::semibold(theme::BODY)).color(t.text));
                if !sub.is_empty() {
                    ui.add(egui::Label::new(RichText::new(sub).color(t.text_dim).size(theme::SMALL).line_height(Some(theme::leading(theme::SMALL)))).wrap());
                }
            });
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                ui.label(clock);
            });
        });
        // An indeterminate sweep along the bottom: movement even when nothing new has arrived.
        ui.add_space(theme::space::MD);
        let (bar, _) = ui.allocate_exact_size(Vec2::new(ui.available_width(), 2.0), Sense::hover());
        sweep(ui.painter(), bar, now, t.separator, t.accent);
    });
    animate(ui, FRAME);
}

/// The button that opens a failure's log; hovering shows where the log is. True when clicked.
pub fn open_log_button(ui: &mut Ui, path: &std::path::Path) -> bool {
    ui.add_space(theme::space::SM);
    secondary(ui, None, "Open log").on_hover_text(path.display().to_string()).clicked()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    /// The wait before the next frame a settled screen drawn by `add` asks for.
    fn wait_after(focused: bool, add: &dyn Fn(&mut Ui)) -> Duration {
        let ctx = egui::Context::default();
        let mut raw = egui::RawInput::default();
        raw.viewports.entry(egui::ViewportId::ROOT).or_default().focused = Some(focused);
        // The first frames always ask for another (layout settles); look at a settled one.
        let mut out = egui::FullOutput::default();
        for _ in 0..4 {
            out = ctx.run_ui(raw.clone(), |ui| add(ui));
            out.textures_delta.clear();
        }
        out.viewport_output.get(&egui::ViewportId::ROOT).map(|v| v.repaint_delay).unwrap_or_default()
    }

    /// A wait asked for with `request_repaint_after(every)`: egui takes the frame it expects to
    /// spend drawing (`predicted_dt`, a 60th of a second here) off it.
    fn paced(wait: Duration, every: Duration) -> bool {
        wait <= every && wait + Duration::from_millis(17) >= every
    }

    /// #579: egui's `Spinner` asks to be redrawn at once, every frame, for as long as it shows;
    /// ours waits for the animation's own pace, so an hour-long build isn't drawn at the display's
    /// rate. Both are drawn here for comparison.
    #[test]
    fn a_spinner_on_screen_does_not_ask_for_a_frame_at_once() {
        let egui_spinner = wait_after(true, &|ui| {
            ui.add(egui::Spinner::new());
        });
        assert!(egui_spinner < Duration::from_millis(10), "egui's own asks for the next frame at once: {egui_spinner:?}");
        let ours = wait_after(true, &|ui| {
            ui.add(Spinner::new().size(theme::icon::MD));
        });
        assert!(paced(ours, FRAME), "ours waits for the animation's pace: {ours:?}");
        let background = wait_after(false, &|ui| drop(ui.add(Spinner::new())));
        assert!(paced(background, Duration::from_secs(1)), "once a second in the background: {background:?}");
    }

    /// egui's animated `ProgressBar` asks for a frame at once, every frame, like its spinner; ours
    /// sweeps at the animation's pace, and a bar that knows its fraction asks for nothing.
    #[test]
    fn a_progress_bar_on_screen_does_not_ask_for_a_frame_at_once() {
        let egui_bar = wait_after(true, &|ui| drop(ui.add(egui::ProgressBar::new(0.0).animate(true))));
        assert!(egui_bar < Duration::from_millis(10), "egui's own asks for the next frame at once: {egui_bar:?}");
        let sweeping = wait_after(true, &|ui| drop(ui.add(ProgressBar::new(None))));
        assert!(paced(sweeping, FRAME), "an unknown fraction sweeps at the animation's pace: {sweeping:?}");
        let background = wait_after(false, &|ui| drop(ui.add(ProgressBar::new(None))));
        assert!(paced(background, Duration::from_secs(1)), "once a second in the background: {background:?}");
        let nan = wait_after(true, &|ui| drop(ui.add(ProgressBar::new(Some(f32::NAN)))));
        assert!(paced(nan, FRAME), "a fraction that is no number is unknown, so it sweeps: {nan:?}");
        for f in [Some(0.4), Some(7.0), Some(-1.0)] {
            assert_eq!(wait_after(true, &|ui| drop(ui.add(ProgressBar::new(f)))), Duration::MAX, "a known fraction ({f:?}) doesn't animate");
        }
    }

    /// #579: every screen uses our spinner and progress bar; egui's own would bring the
    /// redraw-every-frame back.
    #[test]
    fn no_screen_uses_eguis_own_spinner_or_progress_bar() {
        let src = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
        let mut used = Vec::new();
        for entry in std::fs::read_dir(&src).unwrap().flatten() {
            let path = entry.path();
            if path.extension().is_some_and(|e| e == "rs") && path.file_name().is_some_and(|n| n != "widgets.rs") {
                let text = std::fs::read_to_string(&path).unwrap();
                if text.lines().any(|l| !l.trim_start().starts_with("//") && (l.contains("egui::Spinner") || l.contains("egui::ProgressBar"))) {
                    used.push(path.file_name().unwrap().to_string_lossy().into_owned());
                }
            }
        }
        assert!(used.is_empty(), "use `widgets::Spinner` and `widgets::ProgressBar`, which don't ask for a frame every frame: {used:?}");
    }

    #[test]
    fn the_wait_note_says_when_nothing_is_happening() {
        let quiet = wait_note(4000, Some(Duration::from_secs(150))).unwrap();
        assert!(quiet.contains("Nothing new for") && !quiet.contains("big one"), "{quiet}");
        assert_eq!(wait_note(4000, Some(Duration::from_secs(5))), None, "it's visibly moving: nothing to add");
        assert!(wait_note(4000, None).unwrap().contains("big one"), "without news to go on, the old note");
    }

    /// The working card's clock keeps its own space: the words wrap before it instead of running
    /// under it, the way `stat_row`'s value does.
    #[test]
    fn the_works_clock_never_paints_over_the_words() {
        use egui_kittest::{Harness, kittest::Queryable};
        let began = std::time::Instant::now() - Duration::from_secs(42);
        let mut h = Harness::new_ui_state(
            move |ui, first: &mut bool| {
                crate::theme::install_fonts(ui.ctx());
                // Installed fonts become active at the next pass, and the card's title is set in
                // the "semibold" family: draw it once they are.
                if std::mem::replace(first, false) {
                    ui.ctx().request_repaint();
                    return;
                }
                ui.set_width(300.0);
                working_since(ui, "Working on it", Some("Now: Running a long command (42s)  ·  12 actions done so far"), Some(began), None);
            },
            true,
        );
        h.run_steps(3);
        let clock = h.get_by_label("42s").rect();
        let words = h.get_by_label_contains("Now: Running a long command").rect();
        assert!(!clock.intersects(words), "the clock paints over the words: {clock:?} vs {words:?}");
        assert!(clock.left() > words.right(), "the clock sits beside the words: {clock:?} vs {words:?}");
    }
}
