//! Configurable keyboard shortcuts, stored in the config as text such as
//! `Ctrl+Backspace` or `Alt+Shift+F5`.

use std::fmt;

use eframe::egui::{Key, Modifiers};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Shortcut {
    pub key: Key,
    pub ctrl: bool,
    pub shift: bool,
    pub alt: bool,
}

impl Shortcut {
    /// The key with these modifiers.
    pub fn new(key: Key, m: Modifiers) -> Shortcut {
        Shortcut { key, ctrl: m.ctrl, shift: m.shift, alt: m.alt }
    }

    /// Parse `Ctrl+Shift+X`; modifier and key names are case-insensitive.
    pub fn parse(s: &str) -> Option<Shortcut> {
        let (mut ctrl, mut shift, mut alt) = (false, false, false);
        let mut key = None;
        for part in s.split('+').map(str::trim) {
            match part.to_ascii_lowercase().as_str() {
                "ctrl" | "control" => ctrl = true,
                "shift" => shift = true,
                "alt" => alt = true,
                _ => {
                    if key.is_some() {
                        return None;
                    }
                    key = Some(*Key::ALL.iter().find(|k| k.name().eq_ignore_ascii_case(part) || k.symbol_or_name().eq_ignore_ascii_case(part))?);
                }
            }
        }
        Some(Shortcut { key: key?, ctrl, shift, alt })
    }

    /// Whether a key press is this shortcut (the modifiers exactly).
    pub fn matches(&self, key: Key, m: Modifiers) -> bool {
        self.key == key && self.ctrl == m.ctrl && self.shift == m.shift && self.alt == m.alt
    }
}

impl fmt::Display for Shortcut {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        for (on, name) in [(self.ctrl, "Ctrl+"), (self.alt, "Alt+"), (self.shift, "Shift+")] {
            if on {
                f.write_str(name)?;
            }
        }
        f.write_str(self.key.name())
    }
}

/// `A`..`Z` keys as drive letters.
pub fn key_letter(key: Key) -> Option<char> {
    let mut chars = key.name().chars();
    match (chars.next(), chars.next()) {
        (Some(c), None) if c.is_ascii_alphabetic() => Some(c.to_ascii_uppercase()),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_and_format() {
        let s = Shortcut::parse("Ctrl+Backspace").unwrap();
        assert_eq!(s, Shortcut { key: Key::Backspace, ctrl: true, shift: false, alt: false });
        assert_eq!(s.to_string(), "Ctrl+Backspace");
        let s = Shortcut::parse(" shift + alt + pageup ").unwrap();
        assert_eq!(s.to_string(), "Alt+Shift+PageUp");
        assert_eq!(Shortcut::parse(&s.to_string()), Some(s));
        assert_eq!(Shortcut::parse("Ctrl+\\").unwrap().key, Key::Backslash);
        assert!(Shortcut::parse("Ctrl+").is_none());
        assert!(Shortcut::parse("Ctrl+A+B").is_none());
        assert!(Shortcut::parse("Ctrl+Nonsense").is_none());
    }

    #[test]
    fn matching_and_letters() {
        let s = Shortcut::parse("Ctrl+Backspace").unwrap();
        assert!(s.matches(Key::Backspace, Modifiers::CTRL));
        assert!(!s.matches(Key::Backspace, Modifiers::NONE));
        assert!(!s.matches(Key::Backspace, Modifiers::CTRL | Modifiers::SHIFT));
        assert_eq!(key_letter(Key::D), Some('D'));
        assert_eq!(key_letter(Key::F1), None);
        assert_eq!(key_letter(Key::Num1), None);
    }
}
