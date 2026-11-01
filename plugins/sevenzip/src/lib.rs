//! The 7-Zip plugin: archives as folders through 7-Zip's library. A DLL of
//! its own (`plugins\7zip\7zip.spl` next to the program), with `7z.dll` in
//! the same folder; nothing installed on the computer is used.
//!
//! Reading lists the whole archive once; a change (add, delete, rename, new
//! folder) writes a new archive next to the old one, which then replaces it,
//! so a failed or cancelled change leaves the archive as it was.

mod library;

use std::collections::BTreeMap;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::time::SystemTime;

use sal_plugin_api::{export_plugin, join_inner, n_, trf, under, FolderPlugin, Index, Item, Plugin, Progress, Updates, ATTR_DIRECTORY};

use self::library::{Library, OpenArchive, UpdateItem};

/// The library, in the plugin's folder.
pub const LIBRARY: &str = "7z.dll";

export_plugin!(SevenZip);

/// Extensions opened as folders. 7-Zip reads many more formats (exe, docx,
/// msi, …), but those are documents and programs first.
const ARCHIVES: &[&str] = &[
    "7z", "zip", "rar", "tar", "gz", "tgz", "bz2", "tbz", "tbz2", "xz", "txz", "zst", "tzst", "lzma", "z", "taz", "cab", "arj", "lzh", "lha", "iso", "wim", "cpio", "rpm", "deb",
    "001",
];

/// Formats that can be changed (container formats 7-Zip writes).
const WRITABLE: &[&str] = &["7z", "zip", "tar", "wim"];

/// Whether `name`'s extension is one of `list`, any case.
fn ext_in(name: &str, list: &[&str]) -> bool {
    name.rsplit_once('.').is_some_and(|(_, e)| list.iter().any(|x| x.eq_ignore_ascii_case(e)))
}

/// A file's Windows attributes (kept when it is added to an archive).
fn attributes(md: &fs::Metadata) -> u32 {
    use std::os::windows::fs::MetadataExt;
    md.file_attributes()
}

/// 7-Zip's library, loaded into the program.
pub struct SevenZip {
    lib: Library,
}

/// A path from inside an archive, made safe to use under a folder on disk:
/// `.` and `..` parts and empty parts are dropped, and characters Windows
/// doesn't allow in names (a drive's `:` among them) become `_`. Without
/// this, an entry named `..\..\x.exe` or `C:\x.exe` would be extracted
/// outside the target folder.
fn safe_inner(path: &str) -> String {
    let parts: Vec<String> = path
        .split(['\\', '/'])
        .filter(|p| !p.is_empty() && *p != "." && *p != "..")
        .map(|p| p.chars().map(|c| if matches!(c, ':' | '*' | '?' | '"' | '<' | '>' | '|') || (c as u32) < 32 { '_' } else { c }).collect())
        .collect();
    parts.join("\\")
}

impl SevenZip {
    /// The items of an open archive.
    fn index_of(open: &OpenArchive) -> Index {
        let items = open
            .items()
            .into_iter()
            .map(|r| Item {
                path: safe_inner(&r.path),
                dir: r.dir,
                size: if r.dir { 0 } else { r.size },
                modified: r.modified,
                attrs: r.attrib & 0xFFFF | if r.dir { ATTR_DIRECTORY } else { 0 },
                index: Some(r.index),
            })
            .collect();
        Index { items, complete: true }
    }

    /// Write the archive again with `plan` deciding its items. A new archive
    /// is created when the file doesn't exist yet.
    fn update(&self, archive: &Path, progress: &mut Progress, plan: impl FnOnce(&Index) -> io::Result<Vec<UpdateItem>>) -> io::Result<()> {
        if !self.can_write(archive) {
            return Err(io::Error::other(format!("{}: 7-Zip can change only 7z, zip, tar and wim archives", archive.display())));
        }
        let name = archive.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
        let temp = archive.with_file_name(format!(".{name}.{}.new", std::process::id()));
        let result = if archive.exists() {
            let open = self.lib.open(archive)?;
            let items = plan(&Self::index_of(&open))?;
            let r = self.lib.write(Some(&open), open.format, &temp, &items, progress);
            drop(open);
            r
        } else {
            let ext = archive.extension().and_then(|e| e.to_str()).unwrap_or("").to_lowercase();
            let format =
                self.lib.formats.iter().position(|f| f.can_update && f.extensions.contains(&ext)).ok_or_else(|| io::Error::other(format!("no 7-Zip format writes .{ext}")))?;
            let items = plan(&Index::default())?;
            self.lib.write(None, format, &temp, &items, progress)
        };
        match result {
            Ok(()) => fs::rename(&temp, archive),
            Err(e) => {
                let _ = fs::remove_file(&temp);
                Err(e)
            }
        }
    }
}

impl Plugin for SevenZip {
    const ID: &'static str = "7zip";
    const VERSION: &'static str = env!("CARGO_PKG_VERSION");
    const UPDATES: Updates = Updates { repo: "nocturneop15/RustySalamander", tag: "7zip-v", changelog: "plugins/sevenzip/CHANGELOG.md" };
    const NAME: &'static str = n_("7-Zip archives");
    const DESCRIPTION: &'static str =
        n_("Opens zip, 7z, rar, tar, gz, iso, cab and other archives as folders, through 7-Zip's library (7z.dll); 7z, zip, tar and wim archives can also be changed.");

    /// Load `7z.dll` from the plugin's folder.
    fn start(dir: &Path) -> Result<Self, String> {
        let path = dir.join(LIBRARY);
        if !path.is_file() {
            return Err(trf("{file} is missing in {dir}.", &[("file", &LIBRARY), ("dir", &display_path(dir))]));
        }
        let lib = Library::load(&path).map_err(|e| format!("{}: {e}", display_path(&path)))?;
        Ok(SevenZip { lib })
    }
}

/// A path without the `\\?\` prefix.
fn display_path(p: &Path) -> String {
    let s = p.to_string_lossy();
    s.strip_prefix(r"\\?\").map_or_else(|| s.to_string(), str::to_owned)
}

impl FolderPlugin for SevenZip {
    fn id(&self) -> &str {
        Self::ID
    }

    fn handles(&self, name: &str) -> bool {
        ext_in(name, ARCHIVES)
    }

    fn can_write(&self, file: &Path) -> bool {
        file.file_name().and_then(|n| n.to_str()).is_some_and(|n| ext_in(n, WRITABLE))
    }

    fn status(&self) -> String {
        trf("{path} ({n} formats)", &[("path", &display_path(&self.lib.path)), ("n", &self.lib.formats.len())])
    }

    fn list(&self, archive: &Path, _folder: &str) -> io::Result<Index> {
        Ok(Self::index_of(&self.lib.open(archive)?))
    }

    fn extract(&self, archive: &Path, items: &[String], out: &Path, progress: &mut Progress) -> io::Result<()> {
        let open = self.lib.open(archive)?;
        let index = Self::index_of(&open);
        let chosen: BTreeMap<u32, &Item> = index.items.iter().filter(|it| items.iter().any(|w| under(&it.path, w))).filter_map(|it| it.index.map(|i| (i, it))).collect();
        let target = |i: u32| chosen.get(&i).map(|it| (out.join(&it.path), it.dir));
        let times = |i: u32| chosen.get(&i).and_then(|it| it.modified);
        let indices: Vec<u32> = chosen.keys().copied().collect();
        open.extract(&indices, &target, &times, progress)?;
        // folders known only from the paths inside them come with their files;
        // an asked-for folder exists even when it is empty
        for w in items {
            if index.is_dir(w) {
                fs::create_dir_all(out.join(w))?;
            }
        }
        Ok(())
    }

    fn add(&self, archive: &Path, items: &[(PathBuf, String)], progress: &mut Progress) -> io::Result<()> {
        let mut new = Vec::new();
        for (disk, inner) in items {
            collect_new(disk, inner, &mut new)?;
        }
        self.update(archive, progress, |index| {
            let replaced: std::collections::HashSet<&str> = new.iter().filter_map(|n| if let UpdateItem::New { path, .. } = n { Some(path.as_str()) } else { None }).collect();
            let mut plan: Vec<UpdateItem> = index.items.iter().filter(|it| !replaced.contains(it.path.as_str())).filter_map(|it| it.index.map(UpdateItem::Keep)).collect();
            plan.extend(new.iter().cloned());
            Ok(plan)
        })
    }

    fn delete(&self, archive: &Path, items: &[String], progress: &mut Progress) -> io::Result<()> {
        self.update(archive, progress, |index| {
            Ok(index.items.iter().filter(|it| !items.iter().any(|d| under(&it.path, d))).filter_map(|it| it.index.map(UpdateItem::Keep)).collect())
        })
    }

    fn rename(&self, archive: &Path, old: &str, new: &str, progress: &mut Progress) -> io::Result<()> {
        self.update(archive, progress, |index| {
            if index.exists(new) {
                return Err(io::Error::new(io::ErrorKind::AlreadyExists, format!("\"{new}\" already exists in the archive")));
            }
            Ok(index
                .items
                .iter()
                .filter_map(|it| {
                    let i = it.index?;
                    Some(if under(&it.path, old) { UpdateItem::Rename(i, format!("{new}{}", &it.path[old.len()..])) } else { UpdateItem::Keep(i) })
                })
                .collect())
        })
    }

    fn mkdir(&self, archive: &Path, inner: &str, progress: &mut Progress) -> io::Result<()> {
        self.update(archive, progress, |index| {
            if index.exists(inner) {
                return Err(io::Error::new(io::ErrorKind::AlreadyExists, format!("\"{inner}\" already exists in the archive")));
            }
            let mut plan: Vec<UpdateItem> = index.items.iter().filter_map(|it| it.index.map(UpdateItem::Keep)).collect();
            plan.push(UpdateItem::New { path: inner.to_owned(), source: None, dir: true, size: 0, modified: Some(SystemTime::now()), attrib: ATTR_DIRECTORY });
            Ok(plan)
        })
    }
}

/// New archive items for a file or a folder tree on disk.
fn collect_new(disk: &Path, inner: &str, out: &mut Vec<UpdateItem>) -> io::Result<()> {
    let md = fs::metadata(disk)?;
    if md.is_dir() {
        out.push(UpdateItem::New { path: inner.to_owned(), source: None, dir: true, size: 0, modified: md.modified().ok(), attrib: attributes(&md) });
        for e in fs::read_dir(disk)? {
            let e = e?;
            collect_new(&e.path(), &join_inner(inner, &e.file_name().to_string_lossy()), out)?;
        }
    } else {
        out.push(UpdateItem::New { path: inner.to_owned(), source: Some(disk.to_path_buf()), dir: false, size: md.len(), modified: md.modified().ok(), attrib: attributes(&md) });
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The library the build made from the 7-Zip source.
    pub fn library_path() -> Option<PathBuf> {
        let p = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../third_party/7zip-26.05/CPP/7zip/Bundles/Format7zF/_o").join(LIBRARY);
        p.is_file().then_some(p)
    }

    #[test]
    fn names_from_archives_stay_inside_the_target() {
        assert_eq!(safe_inner("..\\..\\Windows\\x.exe"), "Windows\\x.exe");
        assert_eq!(safe_inner("a/./b/../c.txt"), "a\\b\\c.txt");
        assert_eq!(safe_inner("C:\\x.exe"), "C_\\x.exe");
        assert_eq!(safe_inner("\\\\server\\share\\f"), "server\\share\\f");
        assert_eq!(safe_inner("složka 日本\\soubor 🦎.txt"), "složka 日本\\soubor 🦎.txt");
        assert_eq!(safe_inner("a\u{1}b?.txt"), "a_b_.txt");
    }

    #[test]
    fn missing_library() {
        let t = tempfile::tempdir().unwrap();
        let e = SevenZip::start(t.path()).err().unwrap();
        assert!(e.starts_with("7z.dll is missing in "), "{e}");
    }

    #[test]
    fn names() {
        let z = |n: &str| ext_in(n, ARCHIVES);
        assert!(z("Zálohy.7Z") && z("x.tar.gz") && z("a.zip") && !z("zpráva.docx") && !z("7z"));
        assert!(ext_in("a.zip", WRITABLE) && !ext_in("a.rar", WRITABLE));
    }

    #[test]
    fn round_trip_with_the_library() {
        let Some(p) = library_path() else {
            eprintln!("7z.dll not built; skipping");
            return;
        };
        let z = SevenZip { lib: Library::load(&p).unwrap() };
        let names: Vec<String> = z.lib.formats.iter().map(|f| f.name.clone()).collect();
        for f in ["7z", "zip", "Rar5", "tar", "gzip"] {
            assert!(names.iter().any(|n| n.eq_ignore_ascii_case(f)), "{f} missing from {names:?}");
        }
        let t = tempfile::tempdir().unwrap();
        let src = t.path().join("src");
        fs::create_dir_all(src.join("složka 日本").join("hluboko")).unwrap();
        fs::write(src.join("složka 日本").join("soubor 🦎.txt"), "ahoj").unwrap();
        fs::write(src.join("složka 日本").join("hluboko").join("x.bin"), vec![7u8; 100_000]).unwrap();
        fs::write(src.join("-a.txt"), "a").unwrap();
        for ext in ["7z", "zip"] {
            let arc = t.path().join(format!("archiv ž.{ext}"));
            let mut last = (0, 0);
            z.add(&arc, &[(src.join("složka 日本"), "složka 日本".into()), (src.join("-a.txt"), "vnořená\\-a.txt".into())], &mut |d, total| {
                last = (d, total);
                true
            })
            .unwrap();
            assert!(last.1 > 0 && last.0 == last.1, "{ext}: progress {last:?}");
            let ix = z.list(&arc, "").unwrap();
            assert!(ix.exists("složka 日本\\soubor 🦎.txt") && ix.exists("vnořená\\-a.txt") && ix.is_dir("vnořená"), "{ext}: {:?}", ix.items);
            assert_eq!(ix.size_of("složka 日本"), 100_004);
            // adding to an archive that exists replaces it
            z.add(&arc, &[(src.join("-a.txt"), "druhý.txt".into())], &mut |_, _| true).unwrap();
            assert!(z.list(&arc, "").unwrap().exists("druhý.txt"));
            z.rename(&arc, "složka 日本", "nová", &mut |_, _| true).unwrap();
            z.mkdir(&arc, "nová\\prázdná", &mut |_, _| true).unwrap();
            let out = t.path().join(format!("out-{ext}"));
            z.extract(&arc, &["nová".into()], &out, &mut |_, _| true).unwrap();
            assert_eq!(fs::read_to_string(out.join("nová").join("soubor 🦎.txt")).unwrap(), "ahoj");
            assert_eq!(fs::read(out.join("nová").join("hluboko").join("x.bin")).unwrap().len(), 100_000);
            assert!(out.join("nová").join("prázdná").is_dir());
            assert!(!out.join("vnořená").exists(), "only what was asked for");
            z.delete(&arc, &["nová".into(), "druhý.txt".into()], &mut |_, _| true).unwrap();
            let ix = z.list(&arc, "").unwrap();
            assert_eq!(ix.items.iter().filter(|i| !i.dir).map(|i| i.path.as_str()).collect::<Vec<_>>(), ["vnořená\\-a.txt"], "{ext}");
            // cancelling leaves the archive as it was
            let r = z.add(&arc, &[(src.join("složka 日本"), "znovu".into())], &mut |_, _| false);
            assert_eq!(r.unwrap_err().kind(), io::ErrorKind::Interrupted);
            assert_eq!(z.list(&arc, "").unwrap().items.len(), ix.items.len());
            assert!(fs::read_dir(t.path()).unwrap().flatten().all(|e| !e.file_name().to_string_lossy().ends_with(".new")), "no leftovers");
        }
        assert!(z.list(&t.path().join("missing.7z"), "").is_err());
        fs::write(t.path().join("bad.zip"), "not an archive").unwrap();
        assert!(z.list(&t.path().join("bad.zip"), "").is_err());
        assert!(z.add(&t.path().join("x.rar"), &[(src.join("-a.txt"), "a".into())], &mut |_, _| true).is_err(), "rar is read-only");
    }
}
