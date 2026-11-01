//! The viewer's windows: a file as text (colored by its syntax) or hex. The
//! windows themselves, on a thread of their own, are `sal-window`'s.

use sal_core::encoding;
use sal_plugin_api::{tr, trf};
use sal_window::egui::{self, RichText};
use sal_window::Content;

use crate::doc::{Colors, Doc};
use crate::highlight::{self, Span};

/// A viewer window's content: a file as text or hex.
pub struct ViewerWin {
    pub doc: Doc,
}

impl Content for ViewerWin {
    fn title(&self) -> String {
        self.doc.title.clone()
    }

    /// The toolbar (Text/Hex, wrap, encoding, syntax, size) and the text or
    /// the bytes.
    fn show(&mut self, ctx: &egui::Context) -> bool {
        let doc = &mut self.doc;
        let wake = {
            let ctx = ctx.clone();
            move || ctx.request_repaint_of(egui::ViewportId::ROOT)
        };
        let colors = doc.colors(ctx.style().visuals.dark_mode, wake);
        if ctx.input(|i| i.key_pressed(egui::Key::F4)) {
            doc.hex = !doc.hex;
        }
        if ctx.input(|i| i.key_pressed(egui::Key::W) && i.modifiers.ctrl) && doc.can_wrap() {
            doc.wrap = !doc.wrap;
        }
        egui::TopBottomPanel::top("viewer_bar").show(ctx, |ui| {
            ui.horizontal(|ui| {
                ui.selectable_value(&mut doc.hex, false, tr("Text"));
                ui.selectable_value(&mut doc.hex, true, tr("Hex")).on_hover_text(tr("F4 toggles"));
                ui.separator();
                ui.add_enabled_ui(!doc.hex && doc.can_wrap(), |ui| {
                    ui.checkbox(&mut doc.wrap, tr("Wrap")).on_hover_text("Ctrl+W");
                });
                ui.separator();
                let mut chosen = None;
                egui::ComboBox::from_id_salt("enc").selected_text(doc.encoding.name()).show_ui(ui, |ui| {
                    for enc in encoding::encoding_choices() {
                        if ui.selectable_label(enc == doc.encoding, enc.name()).clicked() {
                            chosen = Some(enc);
                        }
                    }
                });
                if let Some(enc) = chosen {
                    doc.set_encoding(enc);
                }
                ui.separator();
                let mut picked = None;
                let shown = doc.syntax.clone().unwrap_or_else(|| tr("Plain text"));
                ui.add_enabled_ui(!doc.hex, |ui| {
                    egui::ComboBox::from_id_salt("syntax").selected_text(shown).height(420.0).show_ui(ui, |ui| {
                        if ui.selectable_label(doc.syntax.is_none(), tr("Plain text")).clicked() {
                            picked = Some(None);
                        }
                        for name in highlight::names() {
                            if ui.selectable_label(doc.syntax.as_deref() == Some(name.as_str()), name.as_str()).clicked() {
                                picked = Some(Some(name.clone()));
                            }
                        }
                    })
                    .response
                    .on_hover_text(tr("Colors the text by its format; detected from the file name or its first line"));
                });
                if let Some(syntax) = picked {
                    doc.set_syntax(syntax);
                }
                match colors {
                    Colors::Working => {
                        ui.label(RichText::new(tr("Coloring…")).weak());
                    }
                    Colors::TooLarge => {
                        ui.label(RichText::new(tr("Too large to color")).weak());
                    }
                    Colors::Off | Colors::Ready => {}
                }
                ui.separator();
                let mut info = trf("{bytes} bytes", &[("bytes", &encoding::format_thousands(doc.file_len))]);
                if doc.file_len > doc.len() as u64 {
                    info.push_str(&trf(" (showing first {size})", &[("size", &encoding::format_size(doc.len() as u64))]));
                }
                ui.label(info);
            });
        });
        egui::CentralPanel::default().show(ctx, |ui| {
            if let Some(e) = &doc.error {
                ui.colored_label(ui.visuals().error_fg_color, e);
                return;
            }
            let row_h = ui.text_style_height(&egui::TextStyle::Monospace);
            let bars = sal_ui::scroll_bar_visibility(ui);
            if doc.hex {
                egui::ScrollArea::both().auto_shrink(false).scroll_bar_visibility(bars).show_rows(ui, row_h, doc.hex_rows(), |ui, range| {
                    for r in range {
                        ui.add(egui::Label::new(RichText::new(doc.hex_line(r)).monospace()).extend());
                    }
                });
            } else if doc.wrap {
                egui::ScrollArea::vertical().auto_shrink(false).scroll_bar_visibility(bars).show(ui, |ui| {
                    for (i, l) in doc.lines.iter().enumerate() {
                        let mut job = line_job(ui, l, doc.line_spans(i));
                        job.wrap.max_width = ui.available_width();
                        ui.add(egui::Label::new(job).wrap());
                    }
                });
            } else {
                egui::ScrollArea::both().auto_shrink(false).scroll_bar_visibility(bars).show_rows(ui, row_h, doc.lines.len(), |ui, range| {
                    for i in range {
                        let job = line_job(ui, &doc.lines[i], doc.line_spans(i));
                        ui.add(egui::Label::new(job).extend());
                    }
                });
            }
        });
        true
    }
}

/// A line of text in the monospace font, colored by `spans` when there are
/// any.
fn line_job(ui: &egui::Ui, text: &str, spans: Option<&[Span]>) -> egui::text::LayoutJob {
    use egui::text::{LayoutJob, TextFormat};
    let font = egui::TextStyle::Monospace.resolve(ui.style());
    let plain = TextFormat { font_id: font.clone(), color: ui.visuals().text_color(), ..Default::default() };
    let mut job = LayoutJob::default();
    if text.is_empty() {
        job.append(" ", 0.0, plain);
        return job;
    }
    let mut at = 0;
    for s in spans.unwrap_or_default() {
        let end = (at + s.len).min(text.len());
        let Some(part) = text.get(at..end) else { break };
        let [r, g, b] = s.rgb;
        let underline = if s.underline { egui::Stroke::new(1.0f32, egui::Color32::from_rgb(r, g, b)) } else { egui::Stroke::NONE };
        job.append(part, 0.0, TextFormat { font_id: font.clone(), color: egui::Color32::from_rgb(r, g, b), italics: s.italic, underline, ..Default::default() });
        at = end;
    }
    if at < text.len() {
        job.append(&text[at..], 0.0, plain);
    }
    job
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A colored line is laid out in its colors; the rest stays plain.
    #[test]
    fn lines_in_their_colors() {
        let ctx = egui::Context::default();
        let _ = ctx.run(egui::RawInput::default(), |ctx| {
            egui::CentralPanel::default().show(ctx, |ui| {
                let red = Span { len: 3, rgb: [200, 0, 0], italic: true, underline: false };
                let blue = Span { len: 2, rgb: [0, 0, 200], italic: false, underline: false };
                let job = line_job(ui, "abcdeřest", Some(&[red, blue]));
                let parts: Vec<(&str, egui::Color32, bool)> = job.sections.iter().map(|s| (&job.text[s.byte_range.clone()], s.format.color, s.format.italics)).collect();
                assert_eq!(parts[0], ("abc", egui::Color32::from_rgb(200, 0, 0), true));
                assert_eq!(parts[1], ("de", egui::Color32::from_rgb(0, 0, 200), false));
                assert_eq!((parts[2].0, parts[2].1), ("řest", ui.visuals().text_color()));
                assert_eq!(line_job(ui, "", None).text, " ", "an empty line keeps its height");
                assert_eq!(line_job(ui, "plain", None).sections.len(), 1);
            });
        });
    }
}
