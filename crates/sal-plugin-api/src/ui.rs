//! Commands a plugin adds to the Plugins menu, and what running one can
//! lead to: a form the program shows, a message, or a connection opened in
//! the panel.
//!
//! A plugin doesn't show dialogs itself. It *describes* a form ([`Form`]);
//! the program draws it like its own dialogs, with its Enter/Esc keys and
//! theme, and runs the form's command with what the user entered. (Viewers
//! and editors, [`Opener`], are the exception: their windows are the
//! plugin's own.)
//!
//! These types cross the DLL boundary as text (see [`encode`]/[`decode`]),
//! so they can grow without changing the C interface.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

/// A command in the Plugins menu.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Command {
    /// The plugin's own id for it, passed to [`crate::FolderPlugin::run`].
    pub id: String,
    /// The menu label, translated (use [`crate::tr`]). End it with `…` when
    /// it opens a form.
    pub label: String,
}

/// A way a plugin opens files: as their viewer (F3) or their editor (F4).
/// The program picks the first opener of the right kind whose `masks`
/// match the file's name, unless the user chose a program of their own in
/// Settings, and calls [`crate::FolderPlugin::open_file`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Opener {
    /// The plugin's own id for it, passed to `open_file`.
    pub id: String,
    /// Viewer or editor.
    pub kind: OpenKind,
    /// Its name in Settings, translated ("Text and hex viewer").
    pub label: String,
    /// The file names it opens: masks separated by `;`, `*.txt;*.log`, or
    /// `*` for any file. Case does not matter.
    pub masks: String,
}

/// The kinds of [`Opener`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OpenKind {
    /// Shows a file (F3).
    View,
    /// Changes a file (F4).
    Edit,
}

impl Opener {
    /// Whether it opens a file with this name (no folder).
    pub fn matches(&self, name: &str) -> bool {
        let name = name.to_lowercase();
        self.masks.split(';').map(str::trim).filter(|m| !m.is_empty()).any(|m| wildcard(&m.to_lowercase(), &name))
    }
}

/// `*` (any text) and `?` (one character) matching; `*.*` also matches
/// names without a dot, as in Windows.
fn wildcard(mask: &str, name: &str) -> bool {
    if mask == "*.*" || mask == "*" {
        return true;
    }
    let (m, n): (Vec<char>, Vec<char>) = (mask.chars().collect(), name.chars().collect());
    let (mut mi, mut ni, mut star, mut mark) = (0, 0, None, 0);
    while ni < n.len() {
        if mi < m.len() && (m[mi] == '?' || m[mi] == n[ni]) {
            mi += 1;
            ni += 1;
        } else if mi < m.len() && m[mi] == '*' {
            star = Some(mi);
            mark = ni;
            mi += 1;
        } else if let Some(s) = star {
            mi = s + 1;
            mark += 1;
            ni = mark;
        } else {
            return false;
        }
    }
    m[mi..].iter().all(|&c| c == '*')
}

/// The values of a form's fields by [`Field::id`], plus what the program
/// adds: `panel` (the active panel's path) and `mount` (the root of this
/// plugin's connection shown there, if any).
pub type Values = BTreeMap<String, String>;

/// What running a command leads to.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Reply {
    /// Nothing more to show.
    Done,
    /// Show this text to the user (e.g. "Disconnected.").
    Message(String),
    /// Show a form; OK runs [`Form::command`] with its values.
    Form(Form),
    /// Open this connection in the active panel.
    Mount(Mount),
    /// Show a window with the items of this connection, which the plugin may
    /// still be filling ([`crate::FolderPlugin::activity`]), such as search
    /// results. From the window the user goes to an item's file
    /// ([`crate::FolderPlugin::real_path`]) or opens the connection in a
    /// panel.
    Results(Results),
    /// Show two connections side by side, one in each panel (two folders
    /// being compared); going into a folder in one panel takes the other to
    /// the same folder. Their items are marked ([`crate::FolderPlugin::marks`]).
    Panels(Panels),
}

/// Two connections in the two panels ([`Reply::Panels`]).
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Panels {
    /// A name for them in the drive selector, the path bar and the banner
    /// (`Documents ↔ Backup`); a suffix says which side.
    pub name: String,
    /// The connection for the left panel.
    pub left: Mount,
    /// The connection for the right panel.
    pub right: Mount,
    /// The command the drive selector's "Refresh" runs, with `results` (the
    /// root of `left`) in its values; empty for no such button.
    pub refresh: String,
}

/// How an item compares with its counterpart ([`crate::FolderPlugin::marks`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ItemMark {
    /// The same on both sides.
    Same,
    /// Different on the two sides (a folder: something inside differs).
    Different,
    /// Different, and this side is newer.
    Newer,
    /// Different, and this side is older.
    Older,
    /// Only on this side.
    OnlyHere,
    /// Only on the other side: the item stands in for it, so the two lists
    /// line up. Copying an item of that name here asks nothing: it isn't
    /// there.
    Missing,
}

impl ItemMark {
    /// Its name in the text the interface carries.
    fn code(self) -> &'static str {
        match self {
            ItemMark::Same => "same",
            ItemMark::Different => "different",
            ItemMark::Newer => "newer",
            ItemMark::Older => "older",
            ItemMark::OnlyHere => "only",
            ItemMark::Missing => "missing",
        }
    }

    /// The mark of a name from [`ItemMark::code`].
    fn of(code: &str) -> Option<ItemMark> {
        [ItemMark::Same, ItemMark::Different, ItemMark::Newer, ItemMark::Older, ItemMark::OnlyHere, ItemMark::Missing].into_iter().find(|m| m.code() == code)
    }
}

/// A results window ([`Reply::Results`]).
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Results {
    /// The window title (translated), e.g. what was searched for.
    pub title: String,
    /// A short name for the results in the drive selector and the Plugins
    /// menu (`*.txt "hello" · Documents`); empty for the title.
    pub name: String,
    /// The connection holding the results. Shown in a panel ("Show filtered
    /// folders"), it is usually a filter over the searched folder: its
    /// folders, holding only what was found.
    pub mount: Mount,
    /// The authority of a second connection (same scheme) with all results
    /// in one list, without the folders leading to them ("Show all in one
    /// list"); empty for none. The
    /// program registers and closes it together with `mount`.
    pub flat: String,
    /// The command the window's "New search…" button runs again (usually
    /// the one that showed the form); empty for no such button.
    pub again: String,
    /// The command the window's "Refresh" button runs, with `results` (the
    /// root of `mount`) in its values: it fills the same connections again
    /// as they were first filled (the same search). Empty for no such button.
    pub refresh: String,
}

/// A dialog the program shows for the plugin.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Form {
    /// The window title.
    pub title: String,
    /// The fields, top to bottom.
    pub fields: Vec<Field>,
    /// The label of the OK button ("Connect").
    pub ok: String,
    /// The command run with the values when the user presses OK.
    pub command: String,
}

/// One row of a [`Form`].
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Field {
    /// The name its value has in [`Values`].
    pub id: String,
    /// The label shown before it (translated).
    pub label: String,
    /// What kind of input it is.
    pub kind: FieldKind,
    /// The initial value; for [`FieldKind::Check`] `"1"` or `""`.
    pub value: String,
    /// Enabled only while the field with this id has this value (e.g. the
    /// key file only when the authentication choice is "key").
    pub enabled_if: Option<(String, String)>,
}

/// The kinds of [`Field`].
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub enum FieldKind {
    /// One line of text.
    #[default]
    Text,
    /// Text shown as dots; never saved by the program.
    Password,
    /// A path, with a Browse… button.
    File,
    /// A folder, with a Browse… button that picks folders.
    Folder,
    /// One of these `(value, label)` options, shown as radio buttons.
    Choice(Vec<(String, String)>),
    /// A check box.
    Check,
    /// Text to read, without input (`label` is the text).
    Note,
    /// Not shown; its value is passed back unchanged (state between two
    /// forms of one dialogue).
    Hidden,
}

/// A connection the plugin opened, shown in a panel like a folder.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Mount {
    /// Lowercase name of the protocol (`"scp"`); the panel shows
    /// `scp://<authority>/…`.
    pub scheme: String,
    /// Where it is connected, e.g. `me@host` or `me@host:2222`. Together
    /// with the scheme it identifies the connection; keep it free of `\`
    /// and `/`.
    pub authority: String,
    /// The folder the panel opens first, as an inner path (`home\me`).
    pub folder: String,
}

impl Mount {
    /// The connection's root, the `file` the plugin's methods get for it.
    pub fn root(&self) -> PathBuf {
        crate::mount_root(&self.scheme, &self.authority)
    }
}

/// Encodes strings into one: each as `<length>:<bytes>,` (a "netstring"),
/// so any text survives, separators and line breaks included.
pub fn encode<S: AsRef<str>>(parts: &[S]) -> String {
    let mut out = String::new();
    for p in parts {
        let p = p.as_ref();
        out.push_str(&p.len().to_string());
        out.push(':');
        out.push_str(p);
        out.push(',');
    }
    out
}

/// The strings [`encode`] put into `s`; `None` if it is damaged.
pub fn decode(s: &str) -> Option<Vec<String>> {
    let mut out = Vec::new();
    let mut rest = s;
    while !rest.is_empty() {
        let (len, after) = rest.split_once(':')?;
        let len: usize = len.parse().ok()?;
        let text = after.get(..len)?;
        rest = after.get(len..)?.strip_prefix(',')?;
        out.push(text.to_owned());
    }
    Some(out)
}

/// Commands as text: id and label of each.
pub(crate) fn encode_commands(commands: &[Command]) -> String {
    encode(&commands.iter().flat_map(|c| [c.id.as_str(), c.label.as_str()]).collect::<Vec<_>>())
}

/// Commands from [`encode_commands`].
pub(crate) fn decode_commands(s: &str) -> Vec<Command> {
    decode(s).unwrap_or_default().as_chunks::<2>().0.iter().map(|c| Command { id: c[0].clone(), label: c[1].clone() }).collect()
}

/// Viewers and editors as text: id, kind, label and masks of each.
pub(crate) fn encode_openers(openers: &[Opener]) -> String {
    let kind = |k: OpenKind| if k == OpenKind::View { "view" } else { "edit" };
    encode(&openers.iter().flat_map(|o| [o.id.as_str(), kind(o.kind), o.label.as_str(), o.masks.as_str()]).collect::<Vec<_>>())
}

/// Viewers and editors from [`encode_openers`]; unknown kinds are left out.
pub(crate) fn decode_openers(s: &str) -> Vec<Opener> {
    let parts = decode(s).unwrap_or_default();
    parts
        .as_chunks::<4>()
        .0
        .iter()
        .filter_map(|c| {
            let kind = match c[1].as_str() {
                "view" => OpenKind::View,
                "edit" => OpenKind::Edit,
                _ => return None,
            };
            Some(Opener { id: c[0].clone(), kind, label: c[2].clone(), masks: c[3].clone() })
        })
        .collect()
}

/// Values as text: name and value of each.
/// Marks as text: name and mark of each.
pub(crate) fn encode_marks(marks: &[(String, ItemMark)]) -> String {
    encode(&marks.iter().flat_map(|(n, m)| [n.as_str(), m.code()]).collect::<Vec<_>>())
}

/// Marks from [`encode_marks`]; unknown marks are left out.
pub(crate) fn decode_marks(s: &str) -> Vec<(String, ItemMark)> {
    decode(s).unwrap_or_default().as_chunks::<2>().0.iter().filter_map(|c| Some((c[0].clone(), ItemMark::of(&c[1])?))).collect()
}

pub(crate) fn encode_values(values: &Values) -> String {
    encode(&values.iter().flat_map(|(k, v)| [k.as_str(), v.as_str()]).collect::<Vec<_>>())
}

/// Values from [`encode_values`].
pub(crate) fn decode_values(s: &str) -> Values {
    decode(s).unwrap_or_default().as_chunks::<2>().0.iter().map(|c| (c[0].clone(), c[1].clone())).collect()
}

/// A form field as text.
fn encode_field(f: &Field) -> String {
    let (kind, choices) = match &f.kind {
        FieldKind::Text => ("text", String::new()),
        FieldKind::Password => ("password", String::new()),
        FieldKind::File => ("file", String::new()),
        FieldKind::Folder => ("folder", String::new()),
        FieldKind::Choice(options) => ("choice", encode(&options.iter().flat_map(|(v, l)| [v.as_str(), l.as_str()]).collect::<Vec<_>>())),
        FieldKind::Check => ("check", String::new()),
        FieldKind::Note => ("note", String::new()),
        FieldKind::Hidden => ("hidden", String::new()),
    };
    let (if_id, if_value) = f.enabled_if.clone().unwrap_or_default();
    encode(&[f.id.as_str(), &f.label, kind, &f.value, &if_id, &if_value, &choices])
}

/// A form field from [`encode_field`].
fn decode_field(s: &str) -> Option<Field> {
    let p = decode(s)?;
    let [id, label, kind, value, if_id, if_value, choices] = <[String; 7]>::try_from(p).ok()?;
    let kind = match kind.as_str() {
        "text" => FieldKind::Text,
        "password" => FieldKind::Password,
        "file" => FieldKind::File,
        "folder" => FieldKind::Folder,
        "choice" => FieldKind::Choice(decode(&choices)?.as_chunks::<2>().0.iter().map(|c| (c[0].clone(), c[1].clone())).collect()),
        "check" => FieldKind::Check,
        "note" => FieldKind::Note,
        "hidden" => FieldKind::Hidden,
        _ => return None,
    };
    let enabled_if = (!if_id.is_empty()).then_some((if_id, if_value));
    Some(Field { id, label, kind, value, enabled_if })
}

/// A reply as text: its kind first, then its parts.
pub(crate) fn encode_reply(r: &Reply) -> String {
    match r {
        Reply::Done => encode(&["done"]),
        Reply::Message(text) => encode(&["message", text]),
        Reply::Mount(m) => encode(&["mount", &m.scheme, &m.authority, &m.folder]),
        Reply::Results(r) => encode(&["results", &r.title, &r.mount.scheme, &r.mount.authority, &r.mount.folder, &r.flat, &r.again, &r.name, &r.refresh]),
        Reply::Panels(p) => encode(&["panels", &p.name, &p.left.scheme, &p.left.authority, &p.left.folder, &p.right.scheme, &p.right.authority, &p.right.folder, &p.refresh]),
        Reply::Form(f) => {
            let mut parts = vec!["form".to_owned(), f.title.clone(), f.ok.clone(), f.command.clone()];
            parts.extend(f.fields.iter().map(encode_field));
            encode(&parts)
        }
    }
}

/// A reply from [`encode_reply`]; `None` when damaged.
pub(crate) fn decode_reply(s: &str) -> Option<Reply> {
    let p = decode(s)?;
    Some(match p.first()?.as_str() {
        "done" => Reply::Done,
        "message" => Reply::Message(p.get(1)?.clone()),
        "mount" => Reply::Mount(Mount { scheme: p.get(1)?.clone(), authority: p.get(2)?.clone(), folder: p.get(3)?.clone() }),
        "results" => Reply::Results(Results {
            title: p.get(1)?.clone(),
            mount: Mount { scheme: p.get(2)?.clone(), authority: p.get(3)?.clone(), folder: p.get(4)?.clone() },
            flat: p.get(5)?.clone(),
            again: p.get(6)?.clone(),
            name: p.get(7)?.clone(),
            refresh: p.get(8)?.clone(),
        }),
        "panels" => Reply::Panels(Panels {
            name: p.get(1)?.clone(),
            left: Mount { scheme: p.get(2)?.clone(), authority: p.get(3)?.clone(), folder: p.get(4)?.clone() },
            right: Mount { scheme: p.get(5)?.clone(), authority: p.get(6)?.clone(), folder: p.get(7)?.clone() },
            refresh: p.get(8)?.clone(),
        }),
        "form" => Reply::Form(Form {
            title: p.get(1)?.clone(),
            ok: p.get(2)?.clone(),
            command: p.get(3)?.clone(),
            fields: p[4..].iter().map(|f| decode_field(f)).collect::<Option<Vec<_>>>()?,
        }),
        _ => return None,
    })
}

/// The root of the connection a value names (the program passes `mount` as
/// the root's path text).
pub fn mount_value(values: &Values) -> Option<PathBuf> {
    values.get("mount").filter(|m| !m.is_empty()).map(|m| crate::text_path(m))
}

/// The text a path is passed as in [`Values`].
pub fn path_value(p: &Path) -> String {
    crate::path_text(p)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn encoding_survives_any_text() {
        let parts = ["", "a:b,c", "12:x,", "řádek\nřádek 🦎", "3:abc,"];
        assert_eq!(decode(&encode(&parts)).unwrap(), parts);
        assert_eq!(decode("3:ab,"), None, "damaged");
        assert_eq!(decode("x:"), None);
        assert_eq!(decode(""), Some(vec![]));
    }

    #[test]
    fn replies_round_trip() {
        let form = Reply::Form(Form {
            title: "Připojit".into(),
            ok: "Connect".into(),
            command: "connect".into(),
            fields: vec![
                Field { id: "host".into(), label: "Server:".into(), value: "example.org".into(), ..Default::default() },
                Field {
                    id: "auth".into(),
                    label: "Sign in with:".into(),
                    kind: FieldKind::Choice(vec![("password".into(), "Password".into()), ("key".into(), "Key".into())]),
                    value: "key".into(),
                    enabled_if: None,
                },
                Field { id: "key".into(), label: "Key:".into(), kind: FieldKind::File, enabled_if: Some(("auth".into(), "key".into())), ..Default::default() },
                Field { id: "state".into(), kind: FieldKind::Hidden, value: "1:,".into(), ..Default::default() },
                Field { id: "dir".into(), label: "Look in:".into(), kind: FieldKind::Folder, value: r"C:\Users".into(), ..Default::default() },
            ],
        });
        let results = Reply::Results(Results {
            title: "Hledání: *.txt 🦎".into(),
            name: "*.txt · Dokumenty".into(),
            mount: Mount { scheme: "search".into(), authority: "3".into(), folder: String::new() },
            flat: "3-list".into(),
            again: "find".into(),
            refresh: "refresh".into(),
        });
        let panels = Reply::Panels(Panels {
            name: "Dokumenty ↔ Záloha".into(),
            left: Mount { scheme: "compare".into(), authority: "1-left".into(), folder: "a\\b".into() },
            right: Mount { scheme: "compare".into(), authority: "1-right".into(), folder: String::new() },
            refresh: "again".into(),
        });
        let marks = vec![("a 🦎.txt".to_owned(), ItemMark::Newer), ("b".to_owned(), ItemMark::Missing), ("c".to_owned(), ItemMark::Same)];
        assert_eq!(decode_marks(&encode_marks(&marks)), marks);
        assert_eq!(decode_marks(&encode(&["x", "strange", "y", "only"])), [("y".to_owned(), ItemMark::OnlyHere)]);
        for r in [
            panels,
            Reply::Done,
            Reply::Message("Hotovo, 🦎".into()),
            Reply::Mount(Mount { scheme: "scp".into(), authority: "me@host:22".into(), folder: "home\\me".into() }),
            form,
            results,
        ] {
            assert_eq!(decode_reply(&encode_reply(&r)), Some(r));
        }
        let commands = vec![Command { id: "connect".into(), label: "Connect…".into() }, Command { id: "x".into(), label: "".into() }];
        assert_eq!(decode_commands(&encode_commands(&commands)), commands);
        let values: Values = [("a".to_owned(), "1".to_owned()), ("heslo".to_owned(), "p:ss,".to_owned())].into();
        assert_eq!(decode_values(&encode_values(&values)), values);
        let openers = vec![
            Opener { id: "text".into(), kind: OpenKind::View, label: "Prohlížeč 🦎".into(), masks: "*".into() },
            Opener { id: "ini".into(), kind: OpenKind::Edit, label: "INI".into(), masks: "*.ini; *.inf".into() },
        ];
        assert_eq!(decode_openers(&encode_openers(&openers)), openers);
    }

    #[test]
    fn opener_masks() {
        let o = |masks: &str| Opener { id: String::new(), kind: OpenKind::View, label: String::new(), masks: masks.into() };
        assert!(o("*").matches("cokoli") && o("*.*").matches("bez tečky"));
        assert!(o("*.txt;*.LOG").matches("Zpráva 🦎.TXT") && o("*.txt; *.log").matches("a.log"));
        assert!(!o("*.txt").matches("a.txt.bak") && !o("").matches("a"));
        assert!(o("read?e.*").matches("readme.md") && !o("read?e.*").matches("reade.md"));
        assert!(o("*.tar.gz").matches("a.b.tar.gz"));
    }
}
