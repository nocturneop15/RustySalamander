//! The results window of a plugin's connection (`Reply::Results`): search
//! results as they come in, with the plugin's progress line, and the way to
//! each found file or to the whole connection in a panel.
//!
//! The window only shows; what its buttons do is up to the app ([`Act`]).

use std::cmp::Ordering;
use std::io;
use std::path::PathBuf;
use std::sync::mpsc::Receiver;
use std::sync::Arc;
use std::time::{Duration, Instant, SystemTime};

use eframe::egui::{self, Key, RichText};
use egui_extras::{Column, TableBuilder};
use sal_core::encoding::format_thousands;
use sal_plugin_api::{Activity, FolderPlugin, Index};

use crate::i18n::{tr, trn};

/// How often the plugin is asked again while it works, and after.
const BUSY: Duration = Duration::from_millis(400);
const IDLE: Duration = Duration::from_secs(3);

/// One result.
#[derive(Debug, Clone, PartialEq)]
pub struct Row {
    /// Its path inside the connection.
    pub inner: String,
    pub name: String,
    /// The folders leading to it (inside the connection).
    pub folder: String,
    pub dir: bool,
    pub size: u64,
    pub modified: Option<SystemTime>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Sort {
    Name,
    Folder,
    Size,
    Date,
}

/// What the user asked for in the window.
#[derive(Debug, Clone, PartialEq)]
pub enum Act {
    /// Open this result (its inner path): a file with its program, a folder
    /// in the active panel.
    Open(String),
    /// View or edit this result (a file), as F3 and F4 do.
    View(String),
    Edit(String),
    /// Show this result in its folder in the active panel.
    GoTo(String),
    /// Open the results in the active panel as the searched folder,
    /// filtered to what was found.
    ShowFolder,
    /// Open everything found in the active panel in one list.
    ShowList,
    Stop,
    /// Run the plugin's command again (a new search).
    Again,
    /// The same search again, into the same results.
    Refresh,
    Close,
}

pub struct ResultsWin {
    pub plugin: Arc<dyn FolderPlugin>,
    /// The connection's root.
    pub root: PathBuf,
    /// The root of the flat list of the same results, if the plugin has one.
    pub flat: Option<PathBuf>,
    pub title: String,
    /// The command "New search…" runs; empty for no such button.
    pub again: String,
    /// The command "Refresh" runs; empty for no such button.
    pub refresh: String,
    rows: Vec<Row>,
    activity: Activity,
    error: Option<String>,
    loading: Option<Receiver<(Activity, io::Result<Index>)>>,
    loaded: Option<Instant>,
    selected: Option<usize>,
    /// Column and ascending; `None` keeps the plugin's order.
    sort: Option<(Sort, bool)>,
    /// The rows changed since [`ResultsWin::take_changed`] was asked.
    changed: bool,
    /// Scroll to the selected row on the next frame.
    reveal: bool,
}

impl ResultsWin {
    /// A results window for the connection `root` (and its flat list), with
    /// the commands its buttons run.
    pub fn new(plugin: Arc<dyn FolderPlugin>, root: PathBuf, flat: Option<PathBuf>, title: String, again: String, refresh: String) -> ResultsWin {
        ResultsWin {
            plugin,
            root,
            flat,
            title,
            again,
            refresh,
            rows: Vec::new(),
            activity: Activity { running: true, text: String::new() },
            error: None,
            loading: None,
            loaded: None,
            selected: None,
            sort: None,
            changed: false,
            reveal: false,
        }
    }

    /// The plugin is filling the results again: ask it often from now on.
    pub fn refreshing(&mut self) {
        self.activity.running = true;
        self.loaded = None;
    }

    /// Whether the rows changed since the last call (panels showing the
    /// connection then read it again).
    pub fn take_changed(&mut self) -> bool {
        std::mem::take(&mut self.changed)
    }

    /// Keep the rows current: ask the plugin in the background, often while
    /// it works and now and then afterwards (files deleted meanwhile).
    pub fn poll(&mut self, ctx: &egui::Context) {
        if let Some(rx) = &self.loading {
            match rx.try_recv() {
                Ok((activity, list)) => {
                    self.loading = None;
                    self.loaded = Some(Instant::now());
                    let finished = self.activity.running && !activity.running;
                    self.activity = activity;
                    match list {
                        Ok(index) => {
                            let rows = rows_of(index);
                            if rows != self.rows || finished {
                                let keep = self.selected.and_then(|i| self.rows.get(i)).map(|r| r.inner.clone());
                                self.rows = rows;
                                self.sort_rows();
                                self.selected = keep.and_then(|k| self.rows.iter().position(|r| r.inner == k));
                                self.changed = true;
                            }
                            self.error = None;
                        }
                        Err(e) => self.error = Some(e.to_string()),
                    }
                }
                Err(std::sync::mpsc::TryRecvError::Empty) => {}
                Err(std::sync::mpsc::TryRecvError::Disconnected) => self.loading = None,
            }
        }
        let every = if self.activity.running { BUSY } else { IDLE };
        if self.loading.is_none() && self.loaded.is_none_or(|t| t.elapsed() >= every) {
            let (tx, rx) = std::sync::mpsc::channel();
            let (plugin, root, ctx2) = (self.plugin.clone(), self.root.clone(), ctx.clone());
            std::thread::spawn(move || {
                // the activity first: when it says done, the list is complete
                let activity = plugin.activity(&root);
                let list = plugin.list(&root, "");
                let _ = tx.send((activity, list));
                ctx2.request_repaint();
            });
            self.loading = Some(rx);
        }
        ctx.request_repaint_after(every);
    }

    /// Sort the rows by the column chosen.
    fn sort_rows(&mut self) {
        let Some((by, asc)) = self.sort else { return };
        let key = |a: &Row, b: &Row| -> Ordering {
            match by {
                Sort::Name => a.name.to_lowercase().cmp(&b.name.to_lowercase()),
                Sort::Folder => a.folder.to_lowercase().cmp(&b.folder.to_lowercase()).then_with(|| a.name.to_lowercase().cmp(&b.name.to_lowercase())),
                Sort::Size => a.size.cmp(&b.size),
                Sort::Date => a.modified.cmp(&b.modified),
            }
        };
        self.rows.sort_by(|a, b| if asc { key(a, b) } else { key(b, a) });
    }

    /// The results shown, in their order.
    #[cfg(test)]
    pub fn rows(&self) -> &[Row] {
        &self.rows
    }

    /// The selected result's inner path.
    pub fn selected(&self) -> Option<String> {
        self.selected.and_then(|i| self.rows.get(i)).map(|r| r.inner.clone())
    }

    /// The window's contents; `height` is the room for the list.
    pub fn ui(&mut self, ui: &mut egui::Ui, height: f32) -> Option<Act> {
        let mut act = None;
        ui.horizontal(|ui| {
            if self.activity.running {
                ui.spinner();
            }
            let line = if !self.activity.text.is_empty() { self.activity.text.clone() } else { trn("{n} item", "{n} items", self.rows.len() as u64) };
            ui.add(egui::Label::new(line).truncate());
        });
        if let Some(e) = &self.error {
            ui.colored_label(ui.visuals().error_fg_color, e);
        }
        ui.add_space(4.0);

        // the keyboard moves through the list
        let n = self.rows.len();
        let (up, down, home, end, pg_up, pg_down) = ui.input(|i| {
            (
                i.key_pressed(Key::ArrowUp),
                i.key_pressed(Key::ArrowDown),
                i.key_pressed(Key::Home),
                i.key_pressed(Key::End),
                i.key_pressed(Key::PageUp),
                i.key_pressed(Key::PageDown),
            )
        });
        let (f3, f4) = ui.input(|i| (i.key_pressed(Key::F3), i.key_pressed(Key::F4)));
        if let Some(sel) = self.selected().filter(|_| f3 || f4) {
            act = Some(if f3 { Act::View(sel) } else { Act::Edit(sel) });
        }
        if n > 0 && (up || down || home || end || pg_up || pg_down) {
            let cur = self.selected.unwrap_or(0) as i64;
            let page = ((height / 22.0) as i64 - 1).max(1);
            let to = if home {
                0
            } else if end {
                n as i64 - 1
            } else if up {
                cur - 1
            } else if down && self.selected.is_some() {
                cur + 1
            } else if down {
                0
            } else if pg_up {
                cur - page
            } else {
                cur + page
            };
            self.selected = Some(to.clamp(0, n as i64 - 1) as usize);
            self.reveal = true;
        }

        let headers = [(Sort::Name, tr("Name")), (Sort::Folder, tr("Folder")), (Sort::Size, tr("Size")), (Sort::Date, tr("Date"))];
        let row_h = ui.text_style_height(&egui::TextStyle::Body) + 6.0;
        let mut clicked_sort = None;
        let mut clicked_row = None;
        let mut double = None;
        // the list takes the whole room, also while it is still short
        let width = ui.available_width();
        ui.allocate_ui_with_layout(egui::vec2(width, height), egui::Layout::top_down(egui::Align::Min), |ui| {
            ui.set_min_size(egui::vec2(width, height));
            ui.style_mut().wrap_mode = Some(egui::TextWrapMode::Truncate);
            let mut table = TableBuilder::new(ui)
                .striped(true)
                .resizable(false)
                .sense(egui::Sense::click())
                .min_scrolled_height(height)
                .max_scroll_height(height)
                // widths from the window's, every frame: remembered widths would
                // keep the ones of the window's first, narrow sizing pass
                .column(Column::exact(((width - 270.0) * 0.45).max(80.0)).clip(true))
                .column(Column::remainder().clip(true))
                .column(Column::exact(80.0))
                .column(Column::exact(150.0));
            if std::mem::take(&mut self.reveal) {
                if let Some(i) = self.selected {
                    table = table.scroll_to_row(i, None);
                }
            }
            table
                .header(row_h, |mut h| {
                    for (col, label) in &headers {
                        // the whole cell sorts, not only its title
                        let (_, cell) = h.col(|ui| {
                            let mark = match self.sort {
                                Some((c, true)) if c == *col => sal_ui::mdi::MENU_UP,
                                Some((c, false)) if c == *col => sal_ui::mdi::MENU_DOWN,
                                _ => "",
                            };
                            ui.add(egui::Label::new(RichText::new(format!("{label} {mark}")).strong()).selectable(false));
                        });
                        if cell.on_hover_cursor(egui::CursorIcon::PointingHand).clicked() {
                            clicked_sort = Some(*col);
                        }
                    }
                })
                .body(|body| {
                    body.rows(row_h, n, |mut row| {
                        let i = row.index();
                        let r = &self.rows[i];
                        row.set_selected(self.selected == Some(i));
                        row.col(|ui| {
                            ui.label(if r.dir { format!("{} {}", sal_ui::mdi::FOLDER_OUTLINE, r.name) } else { r.name.clone() });
                        });
                        row.col(|ui| {
                            ui.label(&r.folder);
                        });
                        row.col(|ui| {
                            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                                ui.label(if r.dir { tr("DIR").to_owned() } else { format_thousands(r.size) });
                            });
                        });
                        row.col(|ui| {
                            ui.label(crate::app::fmt_time(r.modified));
                        });
                        let resp = row.response();
                        if resp.clicked() {
                            clicked_row = Some(i);
                        }
                        if resp.double_clicked() {
                            double = Some(i);
                        }
                    });
                });
        });
        if let Some(col) = clicked_sort {
            self.sort = match self.sort {
                Some((c, asc)) if c == col => Some((c, !asc)),
                _ => Some((col, true)),
            };
            let keep = self.selected();
            self.sort_rows();
            self.selected = keep.and_then(|k| self.rows.iter().position(|r| r.inner == k));
        }
        if let Some(i) = clicked_row {
            self.selected = Some(i);
        }
        if let Some(i) = double {
            act = self.rows.get(i).map(|r| Act::Open(r.inner.clone()));
        }

        ui.add_space(6.0);
        let mut list: Vec<(String, String, Act)> = Vec::new();
        if let Some(sel) = self.selected() {
            list.push((
                tr("Open").into(),
                tr("Open the file with its program, or the folder in the active panel (Enter, double-click). F3 views, F4 edits.").into(),
                Act::Open(sel.clone()),
            ));
            list.push((tr("Go to file").into(), tr("Show it in its folder in the active panel").into(), Act::GoTo(sel)));
        }
        list.push((
            tr("Show filtered folders").into(),
            tr("Open the results in the active panel as the searched folder, filtered: its folders, with only what was found in them").into(),
            Act::ShowFolder,
        ));
        if self.flat.is_some() {
            list.push((
                tr("Show all in one list").into(),
                tr("Open everything found, files and folders, in the active panel in one list, without the folders leading to it").into(),
                Act::ShowList,
            ));
        }
        if self.activity.running {
            list.push((tr("Stop").into(), String::new(), Act::Stop));
        }
        if !self.again.is_empty() {
            list.push((tr("New search…").into(), String::new(), Act::Again));
        }
        if !self.refresh.is_empty() {
            list.push((tr("Refresh").into(), tr("Search again with the same settings").into(), Act::Refresh));
        }
        list.push((tr("Close").into(), String::new(), Act::Close));
        let labels: Vec<(&str, &str)> = list.iter().map(|(l, t, _)| (l.as_str(), t.as_str())).collect();
        if let Some(i) = crate::app::centered_buttons(ui, &labels) {
            act = Some(list[i].2.clone());
        }
        act
    }
}

/// The results in an index: everything the plugin listed (the folders that
/// only lead to them aren't listed).
fn rows_of(index: Index) -> Vec<Row> {
    index
        .items
        .into_iter()
        .map(|it| {
            let (folder, name) = it.path.rsplit_once('\\').map_or((String::new(), it.path.clone()), |(f, n)| (f.to_owned(), n.to_owned()));
            Row { inner: it.path, name, folder, dir: it.dir, size: it.size, modified: it.modified }
        })
        .collect()
}
