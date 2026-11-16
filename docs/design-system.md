# Design system

The desktop app draws every screen from one scale and one set of components, so sizes, buttons
and gaps are the same everywhere. The scale lives in `crates/ui-egui/src/theme.rs`; the
components live in `crates/ui-egui/src/widgets.rs` (icon buttons in `icons.rs`). Screens never
write a size, a padding or a colour as a number: a test in `theme.rs`
(`screens_take_sizes_and_colours_from_the_scale`) walks the source and fails when one does.

## The scale (`theme.rs`)

| What | Names | Values |
| --- | --- | --- |
| Headings (Figtree semibold, `theme::heading`) | `TITLE`, `SECTION`, `CARD` | 26, 18, 16 |
| Text (Figtree regular, or medium through `theme::medium` / `label_text`) | `LEAD`, `BODY`, `SMALL`, `LABEL`, `CAPTION`, `TINY` | 17, 15, 14, 13, 12, 11 |
| Code, paths and numbers (JetBrains Mono, `theme::mono`) | `CODE` | 12.5 |
| A wrapped paragraph's line height | `theme::leading(size)` | 1.45 × the size |
| Gaps (`ui.add_space`, item spacing) | `space::XXS` … `space::XXL` | 2, 4, 8, 12, 16, 24, 32 |
| Padding inside frames | `pad::MENU`, `TIGHT`, `PANEL`, `CARD`, `ROOMY`, `ROW`, `BAR`, `CHIP`, `GUTTER`, `FIELD`, `FIELD_COMPACT` | built from the gaps |
| Icons | `icon::XS` … `icon::XXL` | 12, 14, 16, 20, 24, 32 |
| Corners | `Tokens::radius_sm`, `radius`, `radius_lg` | 4 (segments, rows of a list), 6 (controls, fields), 8 (cards, dialogs); status pills are fully round |
| Colours | `Tokens` (`Tokens::get(ctx)`) | one set per theme; contrast is tested |

`TINY` is for the Advanced workbench's dense lists; everything else is 12 pt or more.

### Fonts

Text, labels and headings are Figtree, one variable font file set to three weights (400, 500,
600). Code is JetBrains Mono. Both are in `assets/fonts` with their licences
(`ATTRIBUTION.md`). Figtree covers Latin and extended Latin only; every other script comes from
craft-fonts, which release and CI builds embed (`CRAFT_FONTS_DIR`, see
[localisation](localization.md)). A local build without it shows those scripts as boxes.

### Control sizes

`ControlSize` ties a control's height, label size, side padding and icon size together, so two
controls of one size always line up.

| Size | Height | Label | Icon | Used for |
| --- | --- | --- | --- | --- |
| `Small` | 24 | `LABEL` | 14 | toolbars, rows of a list, the workbench |
| `Medium` (default) | 32 | `BODY` | 16 | buttons, switches, dropdowns and their rows, compact fields |
| `Large` | 40 | `BODY` | 16 | a screen's main switch, full text fields |
| `Hero` | 48 | `CARD` | 20 | the one action a screen is for (Start) |

A button's label, a dropdown's value, a row of its list and the text typed into a field are all
`BODY`, the size of the text around them. egui's own controls (dropdowns, menu rows) are set up
in `theme::apply` to match: `Medium` in Simple mode, `Small` in Advanced.

### Surfaces and edges

- The page is `canvas`, a card on it is `card`, and anything to type in or press is `field`: a
  shade lighter than the card (white on the paper card in the light theme), so it reads as
  something to use. `hover` and `pressed` are a step lighter again.
- Every control has the same edge: `card_border` at rest, `field_border` under the pointer, the
  app colour while it is pressed, open or typed in. `field_border` is the 3:1 colour; at rest a
  field is told apart by its lighter fill and soft edge, not by a 3:1 border.
- Keyboard focus is a hairline in `accent_ink` on the control's edge and a soft 3 px halo
  outside it (`focus_ring`). A text field gets the same, faded in and out
  (`text_focus_ring`, drawn once a frame for whichever field has focus).

## Components (`widgets.rs`)

| Need | Use |
| --- | --- |
| The main action of a screen or card | `primary`, `primary_icon` |
| Any other action | `secondary` |
| A quiet action (no box until hovered) | `ghost` |
| Something that deletes or can't be undone | `danger` (filled in the danger colour) |
| Another size, a disabled state, or the full width of a panel | `Button::new(kind, label).icon(..).size(..).enabled(..).wide()` |
| An icon on its own | `icons::button(ui, name, ControlSize, selected, tooltip)` |
| A list of things to do with what's shown (a panel's actions) | `action_row`, one per row, in a `card` with `pad::TIGHT` |
| A link in text | `link`; inside a caption, `caption_link` |
| One of a few choices | `segmented(ui, options, selected, ControlSize)`: the chosen segment's card slides to the one picked. Big choices with words: `choice_tile` |
| One of many choices | `dropdown(id)` (egui's combo box, set up) with a `menu_item` or `menu_choice` per row |
| A row of a menu | `menu_item` |
| On/off | `toggle` |
| A text field | `egui::TextEdit` with `.frame(text_field_frame(&t))` (`Large`) or `text_field_frame_compact` (`Medium`). One inside a bigger box that shows focus itself calls `frameless_field` |
| A status word | `pill`; how sure we are: `confidence_badge` |
| A box around content | `card` (screens), `panel` (workbench) |
| A section's caption | `caption` |
| A name and its value | `stat_row` |
| Nothing to show yet | `empty_state` |
| Work in progress | `Spinner`, `ProgressBar`, `working` (never egui's own: they redraw every frame) |

Every component paints its hover and pressed states with a short ease (`hover_t`), a focus ring,
a disabled state where it has one, and names itself for screen readers. Labels are translated
inside the component: pass the English key. A row of a list (`menu_item`) shows its text as
given.

## Behaviour

- **Scrolling.** One notch of the wheel moves `theme::WHEEL_STEP` points (100) and eases there.
  Scroll bars are a slim thumb with no track that thickens under the pointer
  (`theme::scroll_style`); the thumb keeps 3:1 against every surface.
- **Lists that open.** A dropdown's list and a menu fade in (`POPUP_FADE`), show about nine rows
  before they scroll, and are as wide as their widest row.
- **Messages.** A message to the person (`CraftApp::toast`) shows as a compact card under the
  title bar, in the middle. The card lets clicks through to what is under it; only its close
  button takes one. A confirmation goes away by itself, and a new one takes the place of the
  one before it. Errors stay until dismissed.
- **Text.** Glyphs sit on whole pixels (`subpixel_binning` is off), which keeps small text
  sharp on a display at 100 %.
- **The window.** Without a native frame the app draws a 1 px edge in `card_border`. On Windows
  the top side is left to the line Windows draws itself.

## Rules

- A new size, gap or colour is a change to the scale, not a number in a screen. If nothing on
  the scale fits, the design probably wants one of the existing steps.
- A size worked out from another (`body.size + 2.0`, a title bar size times its zoom) is fine.
- Padding with four different sides (a prompt box that leaves room for its buttons) is written
  as `Margin { .. }` where it is used; even padding comes from `pad`.
- A paragraph that wraps sets its line height with `theme::leading`; a single line doesn't.
- Rows of a dropdown or a menu are `menu_item`s; don't use `ui.button` or `selectable_label`
  for them.
- The title bar follows the window's chrome (and, on macOS, keeps its size when the app zooms),
  so it scales the same tokens instead of using `ControlSize`.
- Check a change in both themes: `cargo run -p craft-anything-ui-egui --example snapshot -- --out-dir <dir>`
  renders every scene, and `--light` renders them in Studio Light. `--scale 1` shows what a
  display at 100 % shows. The README's pictures come from the same scenes (the example's header
  says which).
