//! A file as the viewer shows it: its text in an encoding (detected, or
//! chosen), or its bytes as hex. No window here, so it can be tested.

use std::io::Read;
use std::path::Path;

use sal_core::encoding::{self, display_path, TextEncoding};
use sal_plugin_api::tr;

/// Files larger than this are shown truncated.
pub const MAX_BYTES: u64 = 32 << 20;
/// Word wrap lays out all text at once, so it's limited to smaller files.
pub const MAX_WRAP_BYTES: usize = 2 << 20;

pub struct Doc {
    pub title: String,
    bytes: Vec<u8>,
    /// The whole file's length; more than `bytes` when it was too large.
    pub file_len: u64,
    pub encoding: TextEncoding,
    bom: usize,
    pub lines: Vec<String>,
    pub hex: bool,
    pub wrap: bool,
    /// Why the file couldn't be read.
    pub error: Option<String>,
}

impl Doc {
    pub fn open(path: &Path) -> Doc {
        let title = format!("{} - {}", encoding::display_name(&display_path(path)), tr("Viewer"));
        let mut d = Doc {
            title,
            bytes: Vec::new(),
            file_len: 0,
            encoding: TextEncoding::Utf8,
            bom: 0,
            lines: Vec::new(),
            hex: false,
            wrap: false,
            error: None,
        };
        match std::fs::File::open(path) {
            Ok(f) => {
                d.file_len = f.metadata().map(|m| m.len()).unwrap_or(0);
                let mut buf = Vec::new();
                if let Err(e) = f.take(MAX_BYTES).read_to_end(&mut buf) {
                    d.error = Some(e.to_string());
                }
                d.bytes = buf;
                let (enc, bom) = encoding::detect(&d.bytes);
                d.bom = bom;
                d.set_encoding(enc);
                // binary files open in hex mode
                d.hex = d.bytes.iter().take(8192).any(|&b| b == 0) && !matches!(enc, TextEncoding::Utf16Le | TextEncoding::Utf16Be);
            }
            Err(e) => d.error = Some(e.to_string()),
        }
        d
    }

    /// Show the text in `enc`.
    pub fn set_encoding(&mut self, enc: TextEncoding) {
        self.encoding = enc;
        let bom = if matches!((enc, self.bom), (TextEncoding::Utf8, 3) | (TextEncoding::Utf16Le | TextEncoding::Utf16Be, 2)) { self.bom } else { 0 };
        let text = encoding::decode(&self.bytes[bom..], enc);
        self.lines = text.lines().map(|l| encoding::visual_order(&l.replace('\t', "    "), false).into_owned()).collect();
    }

    /// The bytes read (at most [`MAX_BYTES`]).
    pub fn len(&self) -> usize {
        self.bytes.len()
    }

    /// Whether word wrap can be turned on.
    pub fn can_wrap(&self) -> bool {
        self.bytes.len() <= MAX_WRAP_BYTES
    }

    /// The number of hex rows (16 bytes each).
    pub fn hex_rows(&self) -> usize {
        self.bytes.len().div_ceil(16)
    }

    /// Hex row `row`: offset, 16 bytes in hex, the same as text.
    pub fn hex_line(&self, row: usize) -> String {
        let start = row * 16;
        let chunk = &self.bytes[start..(start + 16).min(self.bytes.len())];
        let mut s = format!("{start:08X}  ");
        for i in 0..16 {
            match chunk.get(i) {
                Some(b) => s.push_str(&format!("{b:02X} ")),
                None => s.push_str("   "),
            }
            if i == 7 {
                s.push(' ');
            }
        }
        s.push(' ');
        s.extend(chunk.iter().map(|&b| if (0x20..0x7F).contains(&b) { b as char } else { '.' }));
        s
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn text_and_hex() {
        let t = tempfile::tempdir().unwrap();
        let utf8 = t.path().join("zpráva 🦎.txt");
        std::fs::write(&utf8, "\u{FEFF}Dobrý den\n\tžluťoučký kůň\n").unwrap();
        let d = Doc::open(&utf8);
        assert_eq!((d.encoding, d.hex, d.error.as_deref()), (TextEncoding::Utf8, false, None));
        assert_eq!(d.lines, ["Dobrý den", "    žluťoučký kůň"], "the BOM is left out, tabs are spaces");
        assert!(d.title.ends_with("zpráva 🦎.txt - Viewer"), "{}", d.title);

        // UTF-16 has zero bytes but is text
        let utf16 = t.path().join("u16.txt");
        let mut b = vec![0xFF, 0xFE];
        b.extend("ahoj".encode_utf16().flat_map(u16::to_le_bytes));
        std::fs::write(&utf16, b).unwrap();
        let d = Doc::open(&utf16);
        assert_eq!((d.encoding, d.hex, d.lines.clone()), (TextEncoding::Utf16Le, false, vec!["ahoj".to_owned()]));

        // binary files open as hex
        let bin = t.path().join("data.bin");
        std::fs::write(&bin, (0u8..20).collect::<Vec<_>>()).unwrap();
        let mut d = Doc::open(&bin);
        assert!(d.hex);
        assert_eq!((d.hex_rows(), d.len(), d.file_len), (2, 20, 20));
        assert_eq!(d.hex_line(0), "00000000  00 01 02 03 04 05 06 07  08 09 0A 0B 0C 0D 0E 0F  ................");
        assert_eq!(d.hex_line(1), format!("00000010  10 11 12 13{}  ....", " ".repeat(3 * 12 + 1)));
        // another encoding on demand
        d.set_encoding(TextEncoding::Utf16Le);
        assert_eq!(d.encoding, TextEncoding::Utf16Le);

        let missing = Doc::open(&t.path().join("není.txt"));
        assert!(missing.error.is_some());
    }
}
