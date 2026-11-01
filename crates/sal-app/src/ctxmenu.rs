//! Windows 11 style context menu drawn by the app: an icon row for the
//! common commands, a short list, and "Show more options" for the classic
//! shell menu (Windows offers no API to show Explorer's own new menu).

use eframe::egui::{self, pos2, vec2, Align2, Color32, FontFamily, FontId, Id, Order, Pos2, Rect, Sense, Stroke, Ui};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MenuAction {
    Open,
    OpenWith,
    Cut,
    Copy,
    Paste,
    Rename,
    Delete,
    CopyPath,
    CalcSize,
    Terminal,
    Properties,
    NewFolder,
    Refresh,
    SelectAll,
    MoreOptions,
}

impl MenuAction {
    /// Its icon (Material Design Icons).
    fn glyph(self) -> &'static str {
        use sal_ui::mdi;
        use MenuAction::*;
        match self {
            Open => mdi::OPEN_IN_NEW,
            OpenWith => mdi::OPEN_IN_APP,
            Cut => mdi::CONTENT_CUT,
            Copy => mdi::CONTENT_COPY,
            Paste => mdi::CONTENT_PASTE,
            Rename => mdi::RENAME_OUTLINE,
            Delete => mdi::TRASH_CAN_OUTLINE,
            CopyPath => mdi::LINK_VARIANT,
            CalcSize => mdi::SIGMA,
            Terminal => mdi::CONSOLE,
            Properties => mdi::INFORMATION_OUTLINE,
            NewFolder => mdi::FOLDER_PLUS_OUTLINE,
            Refresh => mdi::REFRESH,
            SelectAll => mdi::SELECT_ALL,
            MoreOptions => mdi::DOTS_HORIZONTAL,
        }
    }
}

const MENU_WIDTH: f32 = 300.0;

pub struct Entry {
    pub action: MenuAction,
    pub label: String,
    pub shortcut: String,
    pub enabled: bool,
}

pub enum Row {
    Entry(Entry),
    Separator,
}

/// The icon of `action`, centred in `rect`.
fn paint_glyph(ui: &Ui, rect: Rect, action: MenuAction, icons: &Option<FontFamily>, color: Color32) {
    let size = rect.height() * 0.62;
    let family = icons.clone().unwrap_or(FontFamily::Proportional);
    ui.painter().text(rect.center(), Align2::CENTER_CENTER, action.glyph(), FontId::new(size, family), color);
}

/// Draws the menu at `pos`. Returns the chosen action and the menu's rect
/// (for closing it when the user clicks elsewhere).
pub fn show(ctx: &egui::Context, pos: Pos2, top: &[Entry], rows: &[Row], icons: &Option<FontFamily>) -> (Option<MenuAction>, Rect) {
    let mut chosen = None;
    // keep the whole menu on screen
    let est = vec2(MENU_WIDTH + 14.0, 50.0 + rows.len() as f32 * 30.0);
    let screen = ctx.screen_rect();
    let pos = pos2(pos.x.min(screen.right() - est.x).max(screen.left()), pos.y.min(screen.bottom() - est.y).max(screen.top()));
    let resp = egui::Area::new(Id::new("context_menu")).order(Order::Foreground).fixed_pos(pos).show(ctx, |ui| {
        let frame = egui::Frame::menu(ui.style()).rounding(8.0).inner_margin(egui::Margin::same(6.0));
        frame.show(ui, |ui| {
            // fixed width: rows fill the available width, which in a free
            // floating area would otherwise be the whole screen
            ui.set_width(MENU_WIDTH);
            ui.spacing_mut().item_spacing = vec2(2.0, 1.0);
            if !top.is_empty() {
                ui.horizontal(|ui| {
                    for e in top {
                        let (rect, r) = ui.allocate_exact_size(vec2(40.0, 34.0), if e.enabled { Sense::click() } else { Sense::hover() });
                        if r.hovered() && e.enabled {
                            ui.painter().rect_filled(rect, 4.0, ui.visuals().widgets.hovered.weak_bg_fill);
                        }
                        let c = if e.enabled { ui.visuals().text_color() } else { ui.visuals().weak_text_color().gamma_multiply(0.6) };
                        paint_glyph(ui, rect, e.action, icons, c);
                        let tip = if e.shortcut.is_empty() { e.label.clone() } else { format!("{} ({})", e.label, e.shortcut) };
                        if r.on_hover_text(tip).clicked() {
                            chosen = Some(e.action);
                        }
                    }
                });
                ui.add_space(2.0);
                ui.separator();
            }
            for row in rows {
                match row {
                    Row::Separator => {
                        ui.add_space(2.0);
                        ui.separator();
                        ui.add_space(2.0);
                    }
                    Row::Entry(e) => {
                        let h = ui.text_style_height(&egui::TextStyle::Body) + 12.0;
                        let (rect, r) = ui.allocate_exact_size(vec2(ui.available_width(), h), if e.enabled { Sense::click() } else { Sense::hover() });
                        let vis = ui.visuals();
                        if r.hovered() && e.enabled {
                            ui.painter().rect_filled(rect, 4.0, vis.widgets.hovered.weak_bg_fill);
                        }
                        let fg = if e.enabled { vis.text_color() } else { vis.weak_text_color().gamma_multiply(0.6) };
                        let icon = Rect::from_min_size(rect.min + vec2(6.0, 0.0), vec2(h, h));
                        paint_glyph(ui, icon, e.action, icons, fg);
                        let font = egui::TextStyle::Body.resolve(ui.style());
                        ui.painter().text(pos2(icon.right() + 8.0, rect.center().y), Align2::LEFT_CENTER, &e.label, font.clone(), fg);
                        if !e.shortcut.is_empty() {
                            ui.painter().text(
                                pos2(rect.right() - 10.0, rect.center().y),
                                Align2::RIGHT_CENTER,
                                &e.shortcut,
                                FontId::new(font.size * 0.85, font.family),
                                vis.weak_text_color(),
                            );
                        }
                        if r.clicked() {
                            chosen = Some(e.action);
                        }
                    }
                }
            }
        });
    });
    let rect = resp.response.rect;
    // subtle outline like the Windows 11 menu
    ctx.layer_painter(egui::LayerId::new(Order::Foreground, Id::new("context_menu"))).rect_stroke(
        rect.shrink(0.5),
        8.0,
        Stroke::new(1.0f32, ctx.style().visuals.widgets.noninteractive.bg_stroke.color),
    );
    (chosen, rect)
}
