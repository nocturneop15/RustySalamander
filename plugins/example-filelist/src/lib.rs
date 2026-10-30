//! Example plugin: a `*.filelist` file is a text file with one path per
//! line, and the plugin shows it as a folder of those files.
//!
//! It is small on purpose and shows every part a folder plugin has:
//! listing, copying out (with progress and cancelling), copying in,
//! deleting, refusing what it can't do, and translated texts. The guide that
//! walks through it is `docs/PLUGINS.md`.
//!
//! ```text
//! # my.filelist — lines starting with # are comments
//! C:\Users\me\Documents\report.docx
//! D:\Photos\2024\beach.jpg
//! ```
//!
//! Inside the plugin each listed file appears under its own name, so the
//! "folder" is flat; files that no longer exist are left out.

use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use sal_plugin_api::{export_plugin, n_, tr, FolderPlugin, Index, Item, Plugin, Progress};

/// The plugin's state. It has none: everything is read from the file each
/// time, which also makes it trivially thread-safe.
pub struct FileList;

// Makes this DLL a plugin: exports `rustysalamander_plugin`.
export_plugin!(FileList);

impl Plugin for FileList {
    const ID: &'static str = "filelist";
    const NAME: &'static str = n_("File lists");
    const DESCRIPTION: &'static str = n_("Shows *.filelist files (one path per line) as folders of the files they list.");

    fn start(_dir: &Path) -> Result<Self, String> {
        // a plugin that needs a library or settings would load them from
        // its folder here, and return an error explaining what is missing
        Ok(FileList)
    }
}

/// The files a list names, in order: (line number, path on disk).
fn entries(file: &Path) -> io::Result<Vec<(u32, PathBuf)>> {
    let text = fs::read_to_string(file)?;
    Ok(text
        .lines()
        .enumerate()
        .map(|(n, line)| (n as u32, line.trim()))
        .filter(|(_, line)| !line.is_empty() && !line.starts_with('#'))
        .map(|(n, line)| (n, PathBuf::from(line)))
        .collect())
}

/// The name a file has inside the list.
fn name_of(path: &Path) -> String {
    path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default()
}

/// Write the list again with `paths`; through a temporary file, so a
/// failure leaves the old list as it was.
fn save(file: &Path, paths: &[PathBuf]) -> io::Result<()> {
    let mut text = String::new();
    for p in paths {
        text.push_str(&p.to_string_lossy());
        text.push_str("\r\n");
    }
    let temp = file.with_extension("filelist.new");
    fs::write(&temp, text)?;
    fs::rename(&temp, file)
}

/// The error for an operation a file list can't do; `msg` is a whole
/// sentence marked with `n_`, so translators see it as it is shown.
fn unsupported(msg: &'static str) -> io::Error {
    io::Error::new(io::ErrorKind::Unsupported, tr(msg))
}

impl FolderPlugin for FileList {
    fn id(&self) -> &str {
        Self::ID
    }

    fn handles(&self, name: &str) -> bool {
        name.rsplit_once('.').is_some_and(|(_, ext)| ext.eq_ignore_ascii_case("filelist"))
    }

    fn can_write(&self, _file: &Path) -> bool {
        true
    }

    fn status(&self) -> String {
        // shown under the plugin in Settings; nothing to report here
        String::new()
    }

    fn list(&self, file: &Path) -> io::Result<Index> {
        let mut items = Vec::new();
        for (line, path) in entries(file)? {
            // files that are gone are left out; folders aren't listed
            let Ok(md) = fs::metadata(&path) else { continue };
            if md.is_file() {
                items.push(Item { path: name_of(&path), dir: false, size: md.len(), modified: md.modified().ok(), attrs: 0, index: Some(line) });
            }
        }
        Ok(Index { items })
    }

    fn extract(&self, file: &Path, items: &[String], out: &Path, progress: &mut Progress) -> io::Result<()> {
        let chosen: Vec<PathBuf> = entries(file)?.into_iter().map(|(_, p)| p).filter(|p| items.contains(&name_of(p))).collect();
        let total: u64 = chosen.iter().filter_map(|p| fs::metadata(p).ok()).map(|m| m.len()).sum();
        let mut done = 0;
        for path in chosen {
            // returning false from progress means the user cancelled
            if !progress(done, total) {
                return Err(io::Error::new(io::ErrorKind::Interrupted, tr("Cancelled.")));
            }
            done += fs::copy(&path, out.join(name_of(&path)))?;
        }
        progress(done, total);
        Ok(())
    }

    fn add(&self, file: &Path, items: &[(PathBuf, String)], progress: &mut Progress) -> io::Result<()> {
        let mut paths: Vec<PathBuf> = if file.exists() { entries(file)?.into_iter().map(|(_, p)| p).collect() } else { Vec::new() };
        for (i, (disk, inner)) in items.iter().enumerate() {
            if !progress(i as u64, items.len() as u64) {
                return Err(io::Error::new(io::ErrorKind::Interrupted, tr("Cancelled.")));
            }
            // the list is flat, and it lists files, not their copies:
            // a file keeps its own name inside
            if inner.contains('\\') || disk.is_dir() {
                return Err(unsupported(n_("A file list can't contain folders.")));
            }
            if *inner != name_of(disk) {
                return Err(unsupported(n_("A file list can't rename files.")));
            }
            // an item already there is replaced
            paths.retain(|p| name_of(p) != *inner);
            paths.push(std::path::absolute(disk)?);
        }
        save(file, &paths)
    }

    fn delete(&self, file: &Path, items: &[String], _progress: &mut Progress) -> io::Result<()> {
        // removes the lines only; the files themselves stay where they are
        let paths: Vec<PathBuf> = entries(file)?.into_iter().map(|(_, p)| p).filter(|p| !items.contains(&name_of(p))).collect();
        save(file, &paths)
    }

    fn rename(&self, _file: &Path, _old: &str, _new: &str, _progress: &mut Progress) -> io::Result<()> {
        Err(unsupported(n_("A file list can't rename files.")))
    }

    fn mkdir(&self, _file: &Path, _inner: &str, _progress: &mut Progress) -> io::Result<()> {
        Err(unsupported(n_("A file list can't contain folders.")))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use sal_plugin_api::abi::{HostApi, Str, StrOut, VERSION};
    use std::ffi::c_void;

    /// Tests can call the plugin directly, as above, or through the C
    /// interface exactly as the program does; this does the latter.
    fn through_the_program() -> std::sync::Arc<dyn FolderPlugin> {
        unsafe extern "C" fn translate(_: *mut c_void, msg: Str, out: *const StrOut) {
            (*out).send(msg.get());
        }
        static HOST: HostApi = HostApi { version: VERSION, ctx: std::ptr::null_mut(), translate };
        let desc = unsafe { &*rustysalamander_plugin() };
        unsafe { sal_plugin_api::host::start(desc, &HOST, Path::new(".")) }.unwrap()
    }

    #[test]
    fn a_list_as_a_folder() {
        let t = tempfile::tempdir().unwrap();
        let (a, b) = (t.path().join("zpráva 🦎.txt"), t.path().join("b.bin"));
        fs::write(&a, "ahoj").unwrap();
        fs::write(&b, [7u8; 1000]).unwrap();
        let list = t.path().join("mé soubory.filelist");
        fs::write(&list, format!("# comment\r\n{}\r\n\r\n{}\r\nC:\\gone\\missing.txt\r\n", a.display(), b.display())).unwrap();

        let p = through_the_program();
        assert!(p.handles("x.FileList") && !p.handles("x.txt"));
        let names: Vec<String> = p.list(&list).unwrap().items.into_iter().map(|i| i.path).collect();
        assert_eq!(names, ["zpráva 🦎.txt", "b.bin"], "missing files are left out");

        // copy out, with progress, and cancelled
        let out = t.path().join("out");
        fs::create_dir(&out).unwrap();
        let mut last = (0, 0);
        p.extract(&list, &["b.bin".into()], &out, &mut |d, total| {
            last = (d, total);
            true
        })
        .unwrap();
        assert_eq!((fs::read(out.join("b.bin")).unwrap().len(), last), (1000, (1000, 1000)));
        let e = p.extract(&list, &["b.bin".into()], &out, &mut |_, _| false).unwrap_err();
        assert_eq!(e.kind(), io::ErrorKind::Interrupted);

        // copy in, delete, and what it refuses
        let c = t.path().join("c.txt");
        fs::write(&c, "c").unwrap();
        p.add(&list, &[(c.clone(), "c.txt".into())], &mut |_, _| true).unwrap();
        p.delete(&list, &["zpráva 🦎.txt".into()], &mut |_, _| true).unwrap();
        let names: Vec<String> = p.list(&list).unwrap().items.into_iter().map(|i| i.path).collect();
        assert_eq!(names, ["b.bin", "c.txt"]);
        assert!(a.is_file(), "deleting from the list keeps the file");
        let e = p.mkdir(&list, "new", &mut |_, _| true).unwrap_err();
        assert_eq!(e.to_string(), "A file list can't contain folders.");
        assert!(p.add(&list, &[(c, "other.txt".into())], &mut |_, _| true).is_err());
    }
}
