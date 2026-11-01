//! The viewer plugin: shows files as text, in an encoding it detects or
//! one the user picks, or as hex (F3). It registers itself as the viewer of
//! every file ([`Opener`] with the mask `*`); its windows are its own (see
//! `window.rs`).

mod doc;
mod highlight;
mod window;

use std::io;
use std::path::{Path, PathBuf};

use sal_plugin_api::{export_plugin, n_, tr, FolderPlugin, Index, OpenKind, Opener, Plugin, Progress, Updates, Values};

/// The opener's id.
const TEXT: &str = "text";

pub struct Viewer {
    windows: sal_window::Windows,
}

export_plugin!(Viewer);

impl Plugin for Viewer {
    const ID: &'static str = "viewer";
    const VERSION: &'static str = env!("CARGO_PKG_VERSION");
    const UPDATES: Updates = Updates { repo: "nocturneop15/RustySalamander", tag: "viewer-v", changelog: "plugins/viewer/CHANGELOG.md" };
    const NAME: &'static str = n_("Viewer");
    const DESCRIPTION: &'static str = n_("Shows files as text, in the encoding it detects or one you choose, or as hex (F3).");

    fn start(_dir: &Path) -> Result<Self, String> {
        Ok(Viewer { windows: sal_window::Windows::new(sal_ui::icon::viewer, [900.0, 650.0]) })
    }
}

/// What the folder methods answer: a viewer has no folders.
fn unsupported() -> io::Error {
    io::ErrorKind::Unsupported.into()
}

impl FolderPlugin for Viewer {
    fn id(&self) -> &str {
        Self::ID
    }
    fn handles(&self, _: &str) -> bool {
        false
    }
    fn can_write(&self, _: &Path) -> bool {
        false
    }
    fn status(&self) -> String {
        String::new()
    }
    fn list(&self, _: &Path, _: &str) -> io::Result<Index> {
        Err(unsupported())
    }
    fn extract(&self, _: &Path, _: &[String], _: &Path, _: &mut Progress) -> io::Result<()> {
        Err(unsupported())
    }
    fn add(&self, _: &Path, _: &[(PathBuf, String)], _: &mut Progress) -> io::Result<()> {
        Err(unsupported())
    }
    fn delete(&self, _: &Path, _: &[String], _: &mut Progress) -> io::Result<()> {
        Err(unsupported())
    }
    fn rename(&self, _: &Path, _: &str, _: &str, _: &mut Progress) -> io::Result<()> {
        Err(unsupported())
    }
    fn mkdir(&self, _: &Path, _: &str, _: &mut Progress) -> io::Result<()> {
        Err(unsupported())
    }

    fn openers(&self) -> Vec<Opener> {
        vec![Opener { id: TEXT.into(), kind: OpenKind::View, label: tr("Text and hex viewer"), masks: "*".into() }]
    }

    fn open_file(&self, opener: &str, file: &Path, options: &Values) -> io::Result<()> {
        if opener != TEXT {
            return Err(unsupported());
        }
        let file = file.to_path_buf();
        self.windows.open(Box::new(move || Box::new(window::ViewerWin { doc: doc::Doc::open(&file) })), options.clone())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_viewer_of_every_file() {
        let v = Viewer::start(Path::new(".")).unwrap();
        let o = &v.openers()[0];
        assert_eq!((o.kind, o.id.as_str()), (OpenKind::View, TEXT));
        assert!(o.matches("cokoli.bin") && o.matches("bez přípony"));
        assert_eq!(v.open_file("other", Path::new("a.txt"), &Values::new()).unwrap_err().kind(), io::ErrorKind::Unsupported);
    }
}
