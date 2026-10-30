//! Internal viewer (F3) in its own OS window: text with encoding detection
//! and manual encoding choice, or hex (replaces `viewer*.cpp`).

use std::io::Read;
use std::path::PathBuf;

use eframe::egui::{self, RichText, ViewportBuilder, ViewportId};
use sal_core::encoding::{self, display_path, TextEncoding};

use crate::i18n::{tr, trf};

/// Files larger than this are shown truncated.
const MAX_BYTES: u64 = 32 << 20;
/// Word wrap lays out all text at once, so it's limited to smaller files.
const MAX_WRAP_BYTES: usize = 2 << 20;

pub struct Viewer {
    pub id: ViewportId,
    path: PathBuf,
    pub(crate) title: String,
    bytes: Vec<u8>,
    file_len: u64,
    encoding: TextEncoding,
    bom: usize,
    lines: Vec<String>,
    hex: bool,
    wrap: bool,
    pub open: bool,
    error: Option<String>,
}

impl Viewer {
    pub fn open(path: PathBuf, serial: u64) -> Viewer {
        let title = format!("{} - {}", encoding::display_name(&display_path(&path)), tr("Viewer"));
        let mut v = Viewer {
            id: ViewportId::from_hash_of(("viewer", serial)),
            path,
            title,
            bytes: Vec::new(),
            file_len: 0,
            encoding: TextEncoding::Utf8,
            bom: 0,
            lines: Vec::new(),
            hex: false,
            wrap: false,
            open: true,
            error: None,
        };
        match std::fs::File::open(&v.path) {
            Ok(f) => {
                v.file_len = f.metadata().map(|m| m.len()).unwrap_or(0);
                let mut buf = Vec::new();
                if let Err(e) = f.take(MAX_BYTES).read_to_end(&mut buf) {
                    v.error = Some(e.to_string());
                }
                v.bytes = buf;
                let (enc, bom) = encoding::detect(&v.bytes);
                v.bom = bom;
                v.set_encoding(enc);
                // binary files open in hex mode
                v.hex = v.bytes.iter().take(8192).filter(|&&b| b == 0).count() > 0 && !matches!(enc, TextEncoding::Utf16Le | TextEncoding::Utf16Be);
            }
            Err(e) => v.error = Some(e.to_string()),
        }
        v
    }

    fn set_encoding(&mut self, enc: TextEncoding) {
        self.encoding = enc;
        let bom = if matches!((enc, self.bom), (TextEncoding::Utf8, 3) | (TextEncoding::Utf16Le | TextEncoding::Utf16Be, 2)) { self.bom } else { 0 };
        let text = encoding::decode(&self.bytes[bom..], enc);
        self.lines = text.lines().map(|l| encoding::visual_order(&l.replace('\t', "    "), false).into_owned()).collect();
    }

    fn hex_line(&self, row: usize) -> String {
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

    /// Draw the viewer in its own native window.
    pub fn show(&mut self, ctx: &egui::Context) {
        let builder = ViewportBuilder::default().with_title(&self.title).with_inner_size([900.0, 650.0]);
        ctx.show_viewport_immediate(self.id, builder, |ctx, _class| {
            if ctx.input(|i| i.viewport().close_requested() || i.key_pressed(egui::Key::Escape) || i.key_pressed(egui::Key::F10)) {
                self.open = false;
            }
            if ctx.input(|i| i.key_pressed(egui::Key::F4)) {
                self.hex = !self.hex;
            }
            if ctx.input(|i| i.key_pressed(egui::Key::W) && i.modifiers.ctrl) && self.bytes.len() <= MAX_WRAP_BYTES {
                self.wrap = !self.wrap;
            }
            egui::TopBottomPanel::top("viewer_bar").show(ctx, |ui| {
                ui.horizontal(|ui| {
                    ui.selectable_value(&mut self.hex, false, tr("Text"));
                    ui.selectable_value(&mut self.hex, true, tr("Hex")).on_hover_text(tr("F4 toggles"));
                    ui.separator();
                    ui.add_enabled_ui(!self.hex && self.bytes.len() <= MAX_WRAP_BYTES, |ui| {
                        ui.checkbox(&mut self.wrap, tr("Wrap")).on_hover_text("Ctrl+W");
                    });
                    ui.separator();
                    let mut chosen = None;
                    egui::ComboBox::from_id_salt("enc").selected_text(self.encoding.name()).show_ui(ui, |ui| {
                        for enc in encoding::encoding_choices() {
                            if ui.selectable_label(enc == self.encoding, enc.name()).clicked() {
                                chosen = Some(enc);
                            }
                        }
                    });
                    if let Some(enc) = chosen {
                        self.set_encoding(enc);
                    }
                    ui.separator();
                    let mut info = trf("{bytes} bytes", &[("bytes", &encoding::format_thousands(self.file_len))]);
                    if self.file_len > self.bytes.len() as u64 {
                        info.push_str(&trf(" (showing first {size})", &[("size", &encoding::format_size(self.bytes.len() as u64))]));
                    }
                    ui.label(info);
                });
            });
            egui::CentralPanel::default().show(ctx, |ui| {
                if let Some(e) = &self.error {
                    ui.colored_label(ui.visuals().error_fg_color, e);
                    return;
                }
                let row_h = ui.text_style_height(&egui::TextStyle::Monospace);
                let bars = crate::app::scroll_bar_visibility(ui);
                if self.hex {
                    let rows = self.bytes.len().div_ceil(16);
                    egui::ScrollArea::both().auto_shrink(false).scroll_bar_visibility(bars).show_rows(ui, row_h, rows, |ui, range| {
                        for r in range {
                            ui.add(egui::Label::new(RichText::new(self.hex_line(r)).monospace()).extend());
                        }
                    });
                } else if self.wrap {
                    egui::ScrollArea::vertical().auto_shrink(false).scroll_bar_visibility(bars).show(ui, |ui| {
                        for l in &self.lines {
                            ui.add(egui::Label::new(RichText::new(l).monospace()).wrap());
                        }
                    });
                } else {
                    egui::ScrollArea::both().auto_shrink(false).scroll_bar_visibility(bars).show_rows(ui, row_h, self.lines.len(), |ui, range| {
                        for l in &self.lines[range] {
                            ui.add(egui::Label::new(RichText::new(if l.is_empty() { " " } else { l }).monospace()).extend());
                        }
                    });
                }
            });
        });
    }
}
