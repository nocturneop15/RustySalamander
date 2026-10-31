//! Window icons, drawn from the Material Design Icons: a white icon on a
//! rust-colored rounded square. Without one, egui gives every window its
//! own logo, a white "e" that reads as Internet Explorer's.

use std::sync::{Arc, OnceLock};

use ab_glyph::{Font, FontRef, PxScale};

/// The color of the square behind the icons (RustySalamander's rust).
pub const RUST: [u8; 3] = [196, 82, 29];

/// The program's window icon (two lists side by side, its two panels).
pub const PROGRAM: &str = crate::mdi::FORMAT_COLUMNS;
/// The viewer's window icon (a document with an eye).
pub const VIEWER: &str = crate::mdi::FILE_EYE_OUTLINE;

/// Pixels of the window icons; Windows scales them for the title bar and
/// the taskbar.
const SIZE: u32 = 64;

/// The icon of the program's windows (the main window, its dialogs), drawn
/// once.
pub fn program() -> Arc<egui::IconData> {
    static ICON: OnceLock<Arc<egui::IconData>> = OnceLock::new();
    ICON.get_or_init(|| window_icon(PROGRAM, RUST, SIZE)).clone()
}

/// The icon of the viewer's windows, drawn once.
pub fn viewer() -> Arc<egui::IconData> {
    static ICON: OnceLock<Arc<egui::IconData>> = OnceLock::new();
    ICON.get_or_init(|| window_icon(VIEWER, RUST, SIZE)).clone()
}

/// A window icon `size` pixels square: `glyph` (one of [`crate::mdi`]) in
/// white on a rounded square of `color`.
pub fn window_icon(glyph: &str, color: [u8; 3], size: u32) -> Arc<egui::IconData> {
    let n = size as usize;
    let s = size as f32;
    let mut rgba = vec![0u8; n * n * 4];
    // the square, its edges smoothed over a pixel
    let radius = s * 0.22;
    for y in 0..n {
        for x in 0..n {
            let (px, py) = (x as f32 + 0.5, y as f32 + 0.5);
            let qx = (px - s / 2.0).abs() - (s / 2.0 - radius);
            let qy = (py - s / 2.0).abs() - (s / 2.0 - radius);
            let outside = qx.max(0.0).hypot(qy.max(0.0)) + qx.max(qy).min(0.0) - radius;
            let alpha = (0.5 - outside).clamp(0.0, 1.0);
            let i = (y * n + x) * 4;
            rgba[i..i + 3].copy_from_slice(&color);
            rgba[i + 3] = (alpha * 255.0).round() as u8;
        }
    }
    // the icon, centred
    if let (Ok(font), Some(c)) = (FontRef::try_from_slice(crate::mdi::FONT), glyph.chars().next()) {
        let id = font.glyph_id(c);
        if let Some(outline) = font.outline_glyph(id.with_scale_and_position(PxScale::from(s * 0.8), ab_glyph::point(0.0, 0.0))) {
            let b = outline.px_bounds();
            let (ox, oy) = (((s - b.width()) / 2.0).round(), ((s - b.height()) / 2.0).round());
            outline.draw(|gx, gy, coverage| {
                let (x, y) = (ox as i64 + i64::from(gx), oy as i64 + i64::from(gy));
                if x < 0 || y < 0 || x >= n as i64 || y >= n as i64 {
                    return;
                }
                let i = (y as usize * n + x as usize) * 4;
                let c = coverage.clamp(0.0, 1.0);
                for k in 0..3 {
                    rgba[i + k] = (f32::from(rgba[i + k]) * (1.0 - c) + 255.0 * c).round() as u8;
                }
                rgba[i + 3] = rgba[i + 3].max((c * 255.0).round() as u8);
            });
        }
    }
    Arc::new(egui::IconData { rgba, width: size, height: size })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn icons_are_drawn() {
        for glyph in [PROGRAM, VIEWER] {
            let icon = window_icon(glyph, RUST, 64);
            assert_eq!((icon.width, icon.height, icon.rgba.len()), (64, 64, 64 * 64 * 4));
            let px = |x: usize, y: usize| &icon.rgba[(y * 64 + x) * 4..(y * 64 + x) * 4 + 4];
            assert_eq!(px(0, 0)[3], 0, "the corners are round");
            assert_eq!(px(32, 2), &[RUST[0], RUST[1], RUST[2], 255], "the square");
            let white = icon.rgba.chunks(4).filter(|p| p[0] > 240 && p[1] > 240 && p[2] > 240 && p[3] == 255).count();
            assert!(white > 200, "the icon is drawn: {white} white pixels");
        }
        assert_ne!(window_icon(PROGRAM, RUST, 64).rgba, window_icon(VIEWER, RUST, 64).rgba);
    }
}

