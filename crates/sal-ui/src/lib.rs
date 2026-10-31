//! What the program's windows and its plugins' windows share, so a
//! plugin's window (a viewer) looks like the program: the fonts, the scroll
//! bars and the accent color.

pub mod fonts;
pub mod icon;
pub mod mdi;

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

/// The accent color, in both themes: the color of what is selected or
/// active (the cursor row, the active path bar, selected tabs and buttons,
/// text selections), with readable text on it. `None` gives each theme its
/// own.
pub fn apply_accent(ctx: &egui::Context, accent: Option<[u8; 3]>) {
    for theme in [egui::Theme::Dark, egui::Theme::Light] {
        let own = theme.default_visuals().selection;
        let selection = match accent {
            None => own,
            Some([r, g, b]) => {
                let fill = egui::Color32::from_rgb(r, g, b);
                egui::style::Selection { bg_fill: fill, stroke: egui::Stroke::new(own.stroke.width, readable_on(fill)) }
            }
        };
        ctx.style_mut_of(theme, |s| s.visuals.selection = selection);
    }
}

/// Text that reads well on `bg`: near black on light colors, white on dark.
pub fn readable_on(bg: egui::Color32) -> egui::Color32 {
    let luma = 0.299 * f32::from(bg.r()) + 0.587 * f32::from(bg.g()) + 0.114 * f32::from(bg.b());
    if luma > 150.0 {
        egui::Color32::from_gray(20)
    } else {
        egui::Color32::WHITE
    }
}

/// An accent color as text, `#rrggbb` (how the program tells plugins).
pub fn accent_text(accent: Option<[u8; 3]>) -> String {
    accent.map(|[r, g, b]| format!("#{r:02x}{g:02x}{b:02x}")).unwrap_or_default()
}

/// An accent color from `#rrggbb`; `None` for anything else (the theme's).
pub fn parse_accent(s: &str) -> Option<[u8; 3]> {
    let hex = s.trim().strip_prefix('#')?;
    if hex.len() != 6 {
        return None;
    }
    let byte = |i: usize| u8::from_str_radix(hex.get(i..i + 2)?, 16).ok();
    Some([byte(0)?, byte(2)?, byte(4)?])
}

/// Scroll areas follow the scroll style chosen in [`apply_scrollbars`].
pub fn scroll_bar_visibility(ui: &egui::Ui) -> egui::scroll_area::ScrollBarVisibility {
    if ui.style().spacing.scroll.floating {
        egui::scroll_area::ScrollBarVisibility::VisibleWhenNeeded
    } else {
        egui::scroll_area::ScrollBarVisibility::AlwaysVisible
    }
}


#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn accent_colors() {
        assert_eq!(parse_accent(&accent_text(Some([220, 120, 30]))), Some([220, 120, 30]));
        assert_eq!(accent_text(None), "");
        assert_eq!(parse_accent(""), None);
        assert_eq!(parse_accent("#12345"), None);
        assert_eq!(parse_accent("#zz0000"), None);
        assert_eq!(readable_on(egui::Color32::from_rgb(255, 230, 120)), egui::Color32::from_gray(20));
        assert_eq!(readable_on(egui::Color32::from_rgb(20, 60, 140)), egui::Color32::WHITE);

        let ctx = egui::Context::default();
        apply_accent(&ctx, Some([220, 120, 30]));
        for theme in [egui::Theme::Dark, egui::Theme::Light] {
            ctx.set_theme(theme);
            assert_eq!(ctx.style().visuals.selection.bg_fill, egui::Color32::from_rgb(220, 120, 30));
        }
        apply_accent(&ctx, None);
        ctx.set_theme(egui::Theme::Dark);
        assert_eq!(ctx.style().visuals.selection, egui::Visuals::dark().selection);
    }
}
