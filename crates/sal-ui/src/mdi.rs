//! Material Design Icons (<https://pictogrammers.com/library/mdi/>, the
//! `@mdi/font` 7.4.47 font, Apache-2.0): the program's icons. The font is
//! part of every font set [`crate::fonts`] makes, behind the text fonts, and
//! its icons sit in a private-use area no text font has, so an icon can be
//! put into any text: `format!("{} {name}", mdi::MAGNIFY)`.
//!
//! Only the icons in use are named here, and only they are in the font the
//! program carries: the full font (7,400 icons, 1.3 MB) cut down to them by
//! `tools/subset-mdi.py` (a few kilobytes). To use another icon, add it here
//! with its code from the font's `materialdesignicons.css`
//! (`.mdi-<name>::before`) and run that script; the test below fails until
//! the font has it.

/// The font itself, cut down to the icons below.
pub(crate) const FONT: &[u8] = include_bytes!("../fonts/materialdesignicons-subset.ttf");

/// Its name in the font set.
pub const FONT_NAME: &str = "mdi";

// window icons (see crate::icon)
pub const FORMAT_COLUMNS: &str = "\u{F08DF}";
pub const FILE_EYE_OUTLINE: &str = "\u{F0DCB}";
pub const COG: &str = "\u{F0493}";

// drives and connections
pub const HARDDISK: &str = "\u{F02CA}";
pub const FOLDER_NETWORK: &str = "\u{F0870}";
pub const USB_FLASH_DRIVE: &str = "\u{F129E}";
pub const DISC: &str = "\u{F05EE}";
pub const MEMORY: &str = "\u{F035B}";
pub const HELP_CIRCLE_OUTLINE: &str = "\u{F0625}";
pub const SERVER_NETWORK: &str = "\u{F048D}";

// files and folders
pub const FOLDER: &str = "\u{F024B}";
pub const FOLDER_OUTLINE: &str = "\u{F0256}";
pub const FILE_OUTLINE: &str = "\u{F0224}";
pub const ARROW_UP: &str = "\u{F005D}";

// commands
pub const OPEN_IN_NEW: &str = "\u{F03CC}";
pub const OPEN_IN_APP: &str = "\u{F03CB}";
pub const CONTENT_CUT: &str = "\u{F0190}";
pub const CONTENT_COPY: &str = "\u{F018F}";
pub const CONTENT_PASTE: &str = "\u{F0192}";
pub const RENAME_OUTLINE: &str = "\u{F1C1A}";
pub const TRASH_CAN_OUTLINE: &str = "\u{F0A7A}";
pub const LINK_VARIANT: &str = "\u{F0339}";
pub const SIGMA: &str = "\u{F04A0}";
pub const CONSOLE: &str = "\u{F018D}";
pub const INFORMATION_OUTLINE: &str = "\u{F02FD}";
pub const FOLDER_PLUS_OUTLINE: &str = "\u{F0B9D}";
pub const REFRESH: &str = "\u{F0450}";
pub const SELECT_ALL: &str = "\u{F0486}";
pub const DOTS_HORIZONTAL: &str = "\u{F01D8}";

// marks
pub const MAGNIFY: &str = "\u{F0349}";
pub const PUZZLE_OUTLINE: &str = "\u{F0A66}";
pub const MENU_UP: &str = "\u{F0360}";
pub const MENU_DOWN: &str = "\u{F035D}";
pub const CHEVRON_UP: &str = "\u{F0143}";
pub const CHEVRON_DOWN: &str = "\u{F0140}";

#[cfg(test)]
mod tests {
    /// Every icon named in this file is in the cut-down font, and the font
    /// has nothing else to speak of.
    #[test]
    fn every_icon_is_in_the_font() {
        let source = include_str!("mdi.rs");
        let codes: Vec<char> = source
            .split("\\u{")
            .skip(1)
            .filter_map(|rest| rest.split('}').next())
            .filter_map(|hex| u32::from_str_radix(hex, 16).ok())
            .filter_map(char::from_u32)
            .collect();
        assert!(codes.len() >= 30, "found {} icons", codes.len());
        let ctx = egui::Context::default();
        crate::fonts::install(&ctx, "", "");
        let _ = ctx.run(egui::RawInput::default(), |_| {});
        let icons = egui::FontId::new(16.0, egui::FontFamily::Name(crate::fonts::ICON_FONT.into()));
        for c in codes {
            assert!(ctx.fonts(|f| f.has_glyphs(&icons, &c.to_string())), "U+{:X} is not in the font: run tools/subset-mdi.py", c as u32);
        }
        assert!(super::FONT.len() < 64 * 1024, "the full font slipped in: {} bytes", super::FONT.len());
    }
}
