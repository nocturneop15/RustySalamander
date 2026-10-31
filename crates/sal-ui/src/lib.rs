//! What the program's windows and its plugins' windows share, so a
//! plugin's window (a viewer) looks like the program: the fonts and the
//! scroll bars.

pub mod fonts;

/// Solid always-visible scrollbars, or egui's thin ones that appear on hover.
pub fn apply_scrollbars(ctx: &egui::Context, always: bool) {
    let style = if always {
        // egui's "solid" bars are invisible until hovered; keep track and
        // handle drawn, a bit wider, like classic Windows scrollbars
        egui::style::ScrollStyle {
            bar_width: 10.0,
            dormant_background_opacity: 0.6,
            active_background_opacity: 0.8,
            dormant_handle_opacity: 0.75,
            active_handle_opacity: 0.9,
            ..egui::style::ScrollStyle::solid()
        }
    } else {
        egui::style::ScrollStyle::floating()
    };
    ctx.all_styles_mut(|s| s.spacing.scroll = style);
}

/// Scroll areas follow the scroll style chosen in [`apply_scrollbars`].
pub fn scroll_bar_visibility(ui: &egui::Ui) -> egui::scroll_area::ScrollBarVisibility {
    if ui.style().spacing.scroll.floating {
        egui::scroll_area::ScrollBarVisibility::VisibleWhenNeeded
    } else {
        egui::scroll_area::ScrollBarVisibility::AlwaysVisible
    }
}

