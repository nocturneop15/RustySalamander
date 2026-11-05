//! Spacing and alignment: one set of measures for the program's main
//! window, its dialog windows, and the windows of plugins (the viewer, the
//! comparison) and their forms. A new window or dialog uses these, never
//! numbers of its own, so that everything lines up the same way.
//!
//! The scale, in points:
//!
//! - [`GAP_SMALL`] (4): a note under its control, a heading and what it heads;
//! - [`GAP`] (8): between the parts of a dialog, between group boxes, and
//!   before the dialog's buttons;
//! - [`GAP_LARGE`] (12): between controls on one line that belong to different
//!   things, around a message that stands on its own;
//! - [`MARGIN`] (12): inside a dialog window, along its edges; also the sides
//!   of a toolbar, so the toolbar's controls line up with the content below.
//!
//! Patterns:
//!
//! - a dialog: its content in [`MARGIN`], [`before_buttons`], then
//!   [`buttons`] (centred); a note for the whole dialog goes above the buttons;
//! - a dialog that asks something has a default button: the one Enter
//!   presses, passed to [`buttons`] and tinted with the accent color, so it is
//!   clear what Enter does. Enter must do exactly that button's action; prefer
//!   the safe answer when one is destructive and Enter could be pressed
//!   without reading (Skip, not Overwrite). A row without a default (Enter
//!   means something else in that window, or nothing) says why in a comment
//!   on the line above;
//! - a form (label and input per row): [`form_grid`], labels in the first
//!   column, left-aligned; tick boxes in the second column with their label
//!   to the right;
//! - values (name and value per row): [`table_grid`]; a list of records: the
//!   same, `.striped(true)`;
//! - related controls framed together: [`group_frame`];
//! - a toolbar at the top of a window: [`bar_frame`] and [`controls`];
//! - popup lists and menus: the `MENU_` measures; the main window's own
//!   strips (drive buttons, path bar, status line): the measures after them.

use egui::{vec2, Color32, Frame, Grid, Margin, Style, Ui, Vec2};

/// Inside a dialog window, along its edges.
pub const MARGIN: f32 = 12.0;
/// Small gap: a note under its control, a heading and what it heads.
pub const GAP_SMALL: f32 = 4.0;
/// Gap between the parts of a dialog, and before its buttons.
pub const GAP: f32 = 8.0;
/// Large gap: between unrelated controls on one line, around a message.
pub const GAP_LARGE: f32 = 12.0;
/// Between two texts side by side (the comparison's).
pub const COLUMN_GAP: f32 = 6.0;
/// In running text (a changelog): between paragraphs, and above a small heading.
pub const LINE_GAP: f32 = 2.0;
/// In running text: a bullet's indent.
pub const INDENT: f32 = 6.0;

/// Room around a button's text (egui's own is 4 × 1).
pub const BUTTON_PADDING: Vec2 = vec2(12.0, 4.0);
/// Between controls side by side, and one under another, in a form or a
/// toolbar ([`controls`]).
pub const ITEM_SPACING: Vec2 = vec2(8.0, 4.0);
/// The height buttons, text fields, lists and tick boxes share in a form or
/// a toolbar, so that a row of them lines up.
pub const CONTROL_HEIGHT: f32 = 24.0;
/// Inside a text field.
pub const FIELD_MARGIN: Vec2 = vec2(6.0, 4.0);

/// Inside a group box.
pub const GROUP_MARGIN: f32 = 10.0;
/// A form: between the label column and the inputs, and between rows.
pub const FORM_SPACING: Vec2 = vec2(16.0, 8.0);
/// A table: between columns, and between rows.
pub const TABLE_SPACING: Vec2 = vec2(16.0, 4.0);
/// A toolbar: the sides as a dialog's, a little above and below.
pub const BAR_MARGIN: Margin = Margin { left: MARGIN, right: MARGIN, top: 6.0, bottom: 6.0 };
/// A strip of buttons inside a window (the operations window's): as a toolbar.
pub const STRIP_MARGIN: Margin = Margin { left: GAP, right: GAP, top: 6.0, bottom: 6.0 };

// popup lists and menus (the drive selector, the context menu)

/// Inside a popup list or menu.
pub const MENU_MARGIN: f32 = 6.0;
/// Between a popup's rows.
pub const MENU_ROW_SPACING: f32 = 1.0;
/// Between a separator and the rows around it.
pub const MENU_GAP: f32 = 3.0;
/// A popup's table (a name and its buttons per row).
pub const MENU_GRID_SPACING: Vec2 = vec2(MENU_MARGIN, GAP_SMALL);

// the main window: tighter than a dialog, so the panels keep their room

/// Drive buttons side by side.
pub const CHIP_SPACING: f32 = 2.0;
/// Between the drive buttons and the path bar.
pub const CHROME_GAP: f32 = 6.0;
/// Inside the path bar.
pub const PATH_MARGIN: Margin = Margin { left: 6.0, right: 6.0, top: 3.0, bottom: 3.0 };
/// Between the path bar's parts (folder › folder).
pub const CRUMB_SPACING: f32 = 3.0;
/// Inside a line of text under or over a panel (its status, a plugin's banner).
pub const LINE_MARGIN: Margin = Margin { left: 6.0, right: 6.0, top: 2.0, bottom: 2.0 };

/// The style every window starts from: the button padding. The program
/// applies it to its windows, `sal-window` to the plugins' windows. (The
/// spacing between items stays egui's, which the panels' rows are measured
/// with; forms and toolbars get [`controls`].)
pub fn apply(ctx: &egui::Context) {
    ctx.all_styles_mut(|s| s.spacing.button_padding = BUTTON_PADDING);
}

/// Controls in a form or a toolbar: all [`CONTROL_HEIGHT`] tall, so that
/// buttons, fields, lists and tick boxes on one row line up.
pub fn controls(ui: &mut Ui) {
    let s = ui.spacing_mut();
    s.item_spacing = ITEM_SPACING;
    s.button_padding = BUTTON_PADDING;
    s.interact_size.y = s.interact_size.y.max(CONTROL_HEIGHT);
}

/// A single-line text field with the common inner margin.
pub fn text_field(text: &mut dyn egui::TextBuffer) -> egui::TextEdit<'_> {
    egui::TextEdit::singleline(text).margin(FIELD_MARGIN)
}

/// The frame of a dialog window's content.
pub fn dialog_frame(style: &Style) -> Frame {
    Frame::central_panel(style).inner_margin(MARGIN)
}

/// The main area of a plugin's window (under its toolbar): its edge as a
/// toolbar's, so the content lines up with the controls above.
pub fn content_frame(style: &Style) -> Frame {
    Frame::central_panel(style).inner_margin(MARGIN)
}

/// A toolbar at the top (or bottom) of a window.
pub fn bar_frame(style: &Style) -> Frame {
    Frame::side_top_panel(style).inner_margin(BAR_MARGIN)
}

/// A group box: related controls framed together.
pub fn group_frame(style: &Style) -> Frame {
    Frame::group(style).inner_margin(GROUP_MARGIN)
}

/// A sunken box for text to read (a changelog).
pub fn well_frame(style: &Style) -> Frame {
    Frame::none().fill(style.visuals.extreme_bg_color).inner_margin(GAP)
}

/// A strip of buttons, colored `fill`, inside a window.
pub fn strip_frame(fill: Color32) -> Frame {
    Frame::none().fill(fill).inner_margin(STRIP_MARGIN).rounding(2.0)
}

/// A form: a label and an input per row.
pub fn form_grid(id: impl std::hash::Hash) -> Grid {
    Grid::new(id).num_columns(2).spacing(FORM_SPACING)
}

/// A table: values (name and value per row), or records with
/// `.striped(true)`.
pub fn table_grid(id: impl std::hash::Hash) -> Grid {
    Grid::new(id).spacing(TABLE_SPACING)
}

/// A line between two parts of a dialog.
pub fn divider(ui: &mut Ui) {
    ui.add_space(GAP_SMALL);
    ui.separator();
    ui.add_space(GAP_SMALL);
}

/// The room between a dialog's content and its buttons.
pub fn before_buttons(ui: &mut Ui) {
    ui.add_space(GAP);
}

/// A row of buttons centred in the available width, each with an optional
/// tooltip; the index of the clicked one. `default` is the button Enter
/// presses: it is tinted with the accent color, so it is clear what Enter does.
pub fn buttons(ui: &mut Ui, buttons: &[(&str, &str)], default: Option<usize>) -> Option<usize> {
    let font = egui::TextStyle::Button.resolve(ui.style());
    let pad = ui.spacing().button_padding.x * 2.0;
    let gap = ui.spacing().item_spacing.x;
    let total: f32 = buttons.iter().map(|(l, _)| ui.fonts(|f| f.layout_no_wrap(l.to_string(), font.clone(), Color32::WHITE).size().x) + pad).sum::<f32>()
        + gap * buttons.len().saturating_sub(1) as f32;
    let mut hit = None;
    ui.horizontal(|ui| {
        ui.add_space(((ui.available_width() - total) / 2.0).max(0.0));
        for (i, (label, tip)) in buttons.iter().enumerate() {
            let mut b = egui::Button::new(*label);
            if default == Some(i) {
                let (fill, stroke) = default_button_colors(ui.visuals());
                b = b.fill(fill).stroke(stroke);
            }
            let mut r = ui.add(b);
            if !tip.is_empty() {
                r = r.on_hover_text(*tip);
            }
            if r.clicked() {
                hit = Some(i);
            }
        }
    });
    hit
}

/// The default button's fill and outline: the usual button color with a
/// little of the accent mixed in, outlined in the accent.
pub fn default_button_colors(v: &egui::Visuals) -> (Color32, egui::Stroke) {
    let accent = v.selection.bg_fill;
    let base = v.widgets.inactive.weak_bg_fill;
    let mix = |a: u8, b: u8| (a as f32 + (b as f32 - a as f32) * 0.28).round() as u8;
    let fill = Color32::from_rgb(mix(base.r(), accent.r()), mix(base.g(), accent.g()), mix(base.b(), accent.b()));
    (fill, egui::Stroke::new(1.0_f32, accent))
}

// the scale grows, a toolbar's sides match a dialog's edge, forms and tables
// share their columns' gap, and a control has room for a button's text
const _: () = assert!(GAP_SMALL < GAP && GAP < GAP_LARGE);
const _: () = assert!(BAR_MARGIN.left == MARGIN && BAR_MARGIN.right == MARGIN);
const _: () = assert!(FORM_SPACING.x == TABLE_SPACING.x);
const _: () = assert!(CONTROL_HEIGHT >= BUTTON_PADDING.y * 2.0 + 14.0);

#[cfg(test)]
mod tests {

    /// The program's and the plugins' windows take their spacing from here:
    /// no gap, margin or spacing written as a number of its own (outside
    /// tests). A new window that needs another measure adds it here, named.
    #[test]
    fn windows_use_these_measures() {
        let sources = [
            ("sal-app/src/app.rs", include_str!("../../sal-app/src/app.rs")),
            ("sal-app/src/results.rs", include_str!("../../sal-app/src/results.rs")),
            ("sal-app/src/ctxmenu.rs", include_str!("../../sal-app/src/ctxmenu.rs")),
            ("sal-app/src/dialogwin.rs", include_str!("../../sal-app/src/dialogwin.rs")),
            ("sal-window/src/lib.rs", include_str!("../../sal-window/src/lib.rs")),
            ("plugins/viewer/src/window.rs", include_str!("../../../plugins/viewer/src/window.rs")),
            ("plugins/compare/src/window.rs", include_str!("../../../plugins/compare/src/window.rs")),
        ];
        // a number right after one of these
        let patterns =
            ["add_space(", "inner_margin(", "Margin::same(", "Margin::symmetric(", ".spacing([", "item_spacing = ", "item_spacing.x = ", "item_spacing.y = ", "button_padding = "];
        let mut found = Vec::new();
        for (name, text) in sources {
            let code = text.split("#[cfg(test)]").next().unwrap_or(text);
            for (n, line) in code.lines().enumerate() {
                for p in patterns {
                    let mut rest = line;
                    while let Some(at) = rest.find(p) {
                        rest = &rest[at + p.len()..];
                        let next = rest.trim_start_matches("vec2(").trim_start_matches("egui::vec2(");
                        if next.starts_with(|c: char| c.is_ascii_digit()) {
                            found.push(format!("{name}:{}: {}", n + 1, line.trim()));
                        }
                    }
                }
            }
        }
        assert!(found.is_empty(), "spacing of its own, use sal_ui::layout:\n{}", found.join("\n"));
    }

    #[test]
    fn button_rows_have_a_default_or_say_why() {
        let sources = [
            ("sal-app/src/app.rs", include_str!("../../sal-app/src/app.rs")),
            ("sal-app/src/results.rs", include_str!("../../sal-app/src/results.rs")),
            ("sal-window/src/lib.rs", include_str!("../../sal-window/src/lib.rs")),
            ("plugins/viewer/src/window.rs", include_str!("../../../plugins/viewer/src/window.rs")),
            ("plugins/compare/src/window.rs", include_str!("../../../plugins/compare/src/window.rs")),
        ];
        let mut found = Vec::new();
        for (name, text) in sources {
            let code = text.split("#[cfg(test)]").next().unwrap_or(text);
            let lines: Vec<&str> = code.lines().collect();
            for (n, line) in lines.iter().enumerate() {
                let row = line.contains("layout::buttons(") && line.contains(", None)");
                if row && !(n > 0 && lines[n - 1].trim_start().starts_with("//")) {
                    found.push(format!("{name}:{}: {}", n + 1, line.trim()));
                }
            }
        }
        assert!(found.is_empty(), "a button row without a default button must say why (a comment above):\n{}", found.join("\n"));
    }

    #[test]
    fn the_default_button_is_tinted_with_the_accent() {
        for v in [egui::Visuals::light(), egui::Visuals::dark()] {
            use super::default_button_colors;
            use egui::Color32;
            let (fill, stroke) = default_button_colors(&v);
            let (base, accent) = (v.widgets.inactive.weak_bg_fill, v.selection.bg_fill);
            assert_ne!(fill, base, "tinted");
            assert_ne!(fill, accent, "only lightly");
            assert_eq!(stroke.color, accent);
            // between the two, nearer the button's own color
            let d = |a: Color32, b: Color32| (a.r() as i32 - b.r() as i32).abs() + (a.g() as i32 - b.g() as i32).abs() + (a.b() as i32 - b.b() as i32).abs();
            assert!(d(fill, base) < d(fill, accent));
        }
    }
}
