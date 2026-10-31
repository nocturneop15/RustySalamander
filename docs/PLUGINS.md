# Writing RustySalamander plugins

This guide is for developers who want to extend RustySalamander. It covers what a plugin is, how the program finds and calls it, and how to build, translate, test and ship one.

Three plugins in this repository serve as references:

- **`plugins/example-filelist`**: a small, complete example (about 200 lines with its test). It shows `*.filelist` text files (one path per line) as folders. Read it next to this guide.
- **`plugins/sevenzip`**: the 7-Zip archive plugin, a real-world plugin that drives a third-party library (`7z.dll`).
- **`plugins/scp`**: the SCP plugin. It adds a command to the Plugins menu, asks for the server and password in a form, and shows the server's files in a panel as a *connection*.
- **`plugins/viewer`**: the Viewer plugin, the F3 viewer. It registers as the viewer of every file (`openers`, `open_file`) and shows files in windows of its own, on a thread of its own.
- **`plugins/search`**: the search plugin. Its form starts a search in the background; the program shows the results in a results window while they come in (`Reply::Results`), and they are a connection too, browsed in a panel as a virtual folder of real files (`real_path`).

The API reference is the rustdoc of `crates/sal-plugin-api`. To open it, run `cargo doc -p sal-plugin-api --open`.

## Contents

1. [What a plugin is](#what-a-plugin-is)
2. [How the program finds plugins](#how-the-program-finds-plugins)
3. [Quick start](#quick-start)
4. [Folder plugins: concepts](#folder-plugins-concepts)
5. [The `Plugin` trait](#the-plugin-trait)
6. [The `FolderPlugin` trait, method by method](#the-folderplugin-trait-method-by-method)
7. [Commands, forms and connections](#commands-forms-and-connections)
8. [Errors, progress and cancelling](#errors-progress-and-cancelling)
9. [Translations](#translations)
10. [Testing](#testing)
11. [Building, installing and distributing](#building-installing-and-distributing)
12. [Rules and pitfalls](#rules-and-pitfalls)
13. [Viewers and editors](#viewers-and-editors)
14. [Versions and updates](#versions-and-updates)
15. [The C interface and versioning](#the-c-interface-and-versioning)
16. [Checklist](#checklist)

## What a plugin is

A plugin is a Windows DLL with the extension `.spl`. It lives in its own folder under `plugins` next to `rustysalamander.exe`:

```
rustysalamander.exe
plugins\
  7zip\
    7zip.spl          the plugin
    7z.dll            a library the plugin uses
    lang\
      en.toml         the plugin's texts in English (the template)
      cs.toml         Czech translations
  filelist\
    filelist.spl
    lang\…
```

The program contains no plugins. It only knows the interface in `sal-plugin-api`, so each plugin is built separately and can be added or removed by copying or deleting its folder.

A plugin can do two things, alone or together. It can be a **viewer or editor** of files (F3/F4, see [Viewers and editors](#viewers-and-editors)), and it can be a **folder plugin**. A folder plugin shows things as folders in a panel:

- **files it handles**: archives are the typical case, but anything that holds a list of items works (a playlist, a mailbox, a disk image);
- **connections it opens** from its commands in the **Plugins** menu: a server, a cloud store, a device.

The panel browses either like a folder, tinted and with the plugin's name in a banner below the list. Copy (F5), move (F6), delete (F8), new folder (F7), rename (F2), view (F3) and edit (F4) work inside it through the plugin.

## How the program finds plugins

When the program starts, it looks for `plugins\*.spl` and `plugins\<folder>\*.spl`. Each file is loaded, and the function it exports tells the program the plugin's id, name and description. Changes to the folder take effect at the next start.

- **Settings → Plugins** lists every plugin found, with a switch, its description and its state. When the plugin is running, the state is its `status()` line; otherwise it is the reason the plugin couldn't start.
- **Turning a plugin off** stops it: its state is freed and the DLL stays loaded until the program exits. The program saves the plugin's id in `config.toml` (`disabled_plugins`). Turning it on calls `start` again.
- **Files that can't be used** are listed in Settings with the reason. That covers files that aren't DLLs, DLLs without the entry function, plugins built for another interface version, and a second plugin with an id that is already taken.
- **Another plugins folder:** the `RUSTYSALAMANDER_PLUGINS` environment variable names it. This is useful while developing (see [Testing](#testing)).
- **Dependencies:** while the `.spl` loads, Windows also searches the plugin's own folder for DLLs it links to. A library the plugin loads itself at run time should be loaded by absolute path from the folder given to `start` (the 7-Zip plugin does this for `7z.dll`).

## Quick start

1. **Create a library crate** whose only output is a DLL. Inside this repository, add it under `plugins/<name>` and to the workspace `members` in the root `Cargo.toml`:

   ```toml
   [package]
   name = "sal-plugin-mything"
   version = "0.1.0"
   edition = "2021"

   [lib]
   crate-type = ["cdylib"]

   [dependencies]
   sal-plugin-api = { path = "../../crates/sal-plugin-api" }
   # outside this repository:
   # sal-plugin-api = { git = "<repository URL>", tag = "<release>" }
   ```

2. **Implement the two traits and export the plugin:**

   ```rust
   use std::io;
   use std::path::{Path, PathBuf};
   use sal_plugin_api::{export_plugin, n_, FolderPlugin, Index, Plugin, Progress};

   pub struct MyThing;

   export_plugin!(MyThing);

   impl Plugin for MyThing {
       const ID: &'static str = "mything";
       const NAME: &'static str = n_("My things");
       const DESCRIPTION: &'static str = n_("Shows *.thing files as folders.");
       fn start(_dir: &Path) -> Result<Self, String> {
           Ok(MyThing)
       }
   }

   impl FolderPlugin for MyThing {
       fn id(&self) -> &str { Self::ID }
       fn handles(&self, name: &str) -> bool { name.to_lowercase().ends_with(".thing") }
       fn can_write(&self, _file: &Path) -> bool { false }
       fn status(&self) -> String { String::new() }
       fn list(&self, file: &Path, _folder: &str) -> io::Result<Index> { todo!("read {}", file.display()) }
       fn extract(&self, _: &Path, _: &[String], _: &Path, _: &mut Progress) -> io::Result<()> { todo!() }
       // a read-only plugin: the program never calls these, because
       // can_write is false, but they must exist
       fn add(&self, _: &Path, _: &[(PathBuf, String)], _: &mut Progress) -> io::Result<()> { Err(io::ErrorKind::Unsupported.into()) }
       fn delete(&self, _: &Path, _: &[String], _: &mut Progress) -> io::Result<()> { Err(io::ErrorKind::Unsupported.into()) }
       fn rename(&self, _: &Path, _: &str, _: &str, _: &mut Progress) -> io::Result<()> { Err(io::ErrorKind::Unsupported.into()) }
       fn mkdir(&self, _: &Path, _: &str, _: &mut Progress) -> io::Result<()> { Err(io::ErrorKind::Unsupported.into()) }
   }
   ```

3. **Build and install it:**

   ```powershell
   cargo build --release -p sal-plugin-mything
   mkdir target\release\plugins\mything
   copy target\release\sal_plugin_mything.dll target\release\plugins\mything\mything.spl
   ```

4. **Run `rustysalamander.exe`** and check Settings → Plugins. Press Enter on a `.thing` file.

## Folder plugins: concepts

**Virtual paths.** A path that goes through a file a plugin handles, such as `C:\data\a.zip\docs\report.txt`, is split by the program into:

- **the file:** `C:\data\a.zip`, a real file on disk;
- **the inner path:** `docs\report.txt`. Inner paths are `\`-separated, have no leading separator, and the root of the file is the empty string `""`.

Plugins only ever see the file and inner paths. `sal_plugin_api::join_inner(base, name)` builds inner paths, and `under(path, folder)` tests "is this inside that folder".

**Connections.** A connection a plugin opened (see [Commands, forms and connections](#commands-forms-and-connections)) has a virtual root instead of a file: `\\?\scp\me@host`, which no file system has. The panel shows it as `scp://me@host/…`, and the plugin gets the root as `file`, with inner paths below it as usual. `sal_plugin_api::mount_of(path)` splits a root (or a path inside one) into its scheme, authority and inner path.

**The index.** `list` returns an `Index`: a flat `Vec<Item>` of items, in any order. The program builds the folder tree from it. There are two ways to list:

- **Everything at once** (`complete: true`), for a file read in one go, like an archive. The program keeps the index and calculates folder sizes from it.
- **One folder at a time** (`complete: false`), for a server or anything large. `list` gets the folder the panel opens and returns at least its children. Folder sizes are then unknown.

- **Folders may be implicit.** If the index has `a\b\c.txt` but no item for `a` or `a\b`, both folders still appear. A folder item is needed only to carry a date or attributes, or for an empty folder.
- **Folder sizes** are computed by the program from the files inside them. Report `size: 0` for folders.
- **`Item::index`** is the plugin's own number for the item, such as its position in the archive or a line number. The program doesn't interpret it, but keeps it in the index for the plugin's use. `None` is fine.

**When `list` is called.** A complete index is read when the panel opens the file, and again only when the file's modification time or size changes. A folder of a connection is listed every time the panel shows it, and on Ctrl+R; connections aren't polled for changes. Listing runs on the UI thread, so it should be reasonably fast.

## The `Plugin` trait

`Plugin` is how the DLL describes itself and starts. The `export_plugin!` macro needs it.

| Item | Meaning |
|---|---|
| `const ID` | Unique, stable id, saved in the configuration. Lowercase ASCII is recommended. Never change it after release, or users' "off" settings are lost. |
| `const NAME` | Short name for Settings, in English, marked with `n_` (see [Translations](#translations)). |
| `const DESCRIPTION` | One or two sentences, in English, marked with `n_`. |
| `const VERSION` | The plugin's own version, a semantic version: `env!("CARGO_PKG_VERSION")`. It is independent of the program's; Settings shows it and the update check compares it. |
| `const UPDATES` | Where newer versions are published (`Updates { repo, tag, changelog }`); the default `Updates::NONE` means nowhere. See [Versions and updates](#versions-and-updates). |
| `fn start(dir) -> Result<Self, String>` | Called when the plugin is turned on. `dir` is the plugin's folder. Load libraries or settings from there. On failure, return a message telling the user what is wrong and how to fix it (e.g. "7z.dll is missing in …"). It is shown in Settings, so translate it. |

The program reads `ID`, `NAME`, `DESCRIPTION`, `VERSION` and `UPDATES` without calling `start`, even for plugins that are turned off. Keep them `const`.

## The `FolderPlugin` trait, method by method

All methods take `&self` and may be called **from several threads at once**: the UI thread lists, while background jobs extract or add. The trait requires `Send + Sync`. Keep the plugin stateless, as the example does, or protect shared state with a `Mutex`.

| Method | Called when | Contract |
|---|---|---|
| `id()` | Often | Return `Self::ID`. |
| `handles(name)` | For every file name the user opens or a path passes through | Decide **by name only** (usually the extension); it must be fast. `name` has no folder. Returning true for a file the plugin then can't read is fine: the user gets the `list` error. |
| `can_write(file)` | Before any change | Whether this file's contents can be changed. If false, the program refuses to add, delete, rename and create folders, and the other methods are never called for that file. Copying out still works. |
| `status()` | When Settings → Plugins is shown | One line about the plugin's state (e.g. the library it loaded), or empty. |
| `list(file, folder)` | When the panel opens the file or a folder of a connection | Return the items: everything (`complete: true`) or at least `folder`'s children. Fail with `NotFound` when `folder` isn't a folder. Errors are shown to the user. |
| `extract(file, items, out, progress)` | F5/F6 out of the file, and F3/F4/Enter on an item (extracted to a temporary folder first) | Write each item in `items` to `out\<inner path>`, creating folders as needed. A folder in `items` means the folder **with everything inside it**, and an empty folder must still be created. `out` exists and is empty. |
| `add(file, items, progress)` | F5/F6 into the file | `items` is a list of `(path on disk, inner path)` pairs. Folders on disk come with their contents. An item already at that inner path is **replaced**: the program has already asked the user about conflicts. If `file` doesn't exist yet, create it. |
| `delete(file, items, progress)` | F8 inside the file, and after a move out of it | Remove the items, folders with their contents. |
| `rename(file, old, new, progress)` | F2 inside the file | Rename `old` to `new` (both inner paths). A folder's contents move with it. If `new` exists, fail with `AlreadyExists`. |
| `mkdir(file, inner, progress)` | F7 inside the file | Create the folder `inner`; fail with `AlreadyExists` if something is there. |
| `commands()` | When the Plugins menu opens | The plugin's menu commands (optional; none by default). |
| `run(command, values)` | A menu command is chosen, or a form's OK | What to show next: see below. Runs on a background thread. |
| `close(root)` | The user disconnects, the plugin is turned off, or the program ends | Release the connection with this root. |
| `openers()` | When F3/F4 is pressed, and for Settings → Plugins | The plugin's viewers and editors (optional; none by default). |
| `open_file(opener, file, options)` | F3/F4 on a file an opener matches | Show the file in a window of your own and return at once. See [Viewers and editors](#viewers-and-editors). |

**How the program combines them:**

- **Copy out:** `extract` into a temporary folder, then the normal copy engine moves the files to the target and asks about conflicts there.
- **Move out:** the same, followed by `delete` of the items that were moved. Nothing is deleted if the user cancelled or skipped anything.
- **Copy in:** `list` to find conflicts, the user's answers, then a single `add` call.
- **Copy between two files** (of the same or different plugins): `extract` from one, then `add` to the other.
- **View or edit (F3/F4/Enter):** `extract` of one item into a temporary folder, then the file opens from there. Changes to that copy don't go back into the file.

**Changing a file safely.** Write the new version next to the old one and rename it over the original only when everything has succeeded. The 7-Zip plugin writes `.{name}.{pid}.new`; the example writes `<name>.filelist.new`. A failed or cancelled change then leaves the user's file untouched, and the program relies on this.

## Commands, forms and connections

A plugin can add commands to the **Plugins** menu (`commands()`), each with an id and a translated label; a label ending with `…` promises a dialog. Choosing one calls `run(id, values)` on a background thread, so it may take a while, such as connecting to a server. The status bar says the plugin is working, and only one plugin command runs at a time. `run` returns a `Reply`:

| `Reply` | The program |
|---|---|
| `Done` | Does nothing more. |
| `Message(text)` | Shows the text in a message box titled with the plugin's name. |
| `Form(form)` | Shows the form as a dialog. OK runs `form.command` with the values entered. |
| `Mount(mount)` | Opens the connection in the active panel, at `mount.folder`. |
| `Results(results)` | Opens a results window for the connection `results.mount`, titled `results.title`, which the plugin may still be filling (see [Results windows](#results-windows)). |

An error from `run` is shown as a message, except `Interrupted`.

**Forms.** A plugin never opens windows: it *describes* a dialog, and the program draws it like its own (theme, fonts, Enter and Esc, the window placement). A `Form` has a title, the OK button's label, the command OK runs, and `fields`, one per row:

| `FieldKind` | Shown as | Value |
|---|---|---|
| `Text` | a text box | the text |
| `Password` | a text box showing dots | the text; the program never saves it |
| `File` | a text box with a Browse… button (the Windows "Open" dialog) | the path |
| `Folder` | a text box with a Browse… button that picks a folder | the path |
| `Choice(options)` | radio buttons, one per `(value, label)` | the chosen value |
| `Check` | a check box | `"1"` or `""` |
| `Note` | text to read (`label`), no input | — |
| `Hidden` | nothing | passed back unchanged |

`enabled_if: Some((id, value))` greys a field out unless another field has that value. For example, the SCP plugin enables its key file only when "Private key" is chosen. `Hidden` fields carry state from one form to the next. The SCP plugin's "unknown server" question carries everything entered before, plus the fingerprint the user is asked to accept.

**Values.** `run` gets the form's values by field id, plus two that the program adds:

- `panel`: the active panel's path;
- `mount`: the root of this plugin's connection shown there, if any (`sal_plugin_api::ui::mount_value`).

**Connections.** `Reply::Mount(Mount { scheme, authority, folder })` opens a connection:

- `scheme` is a short lowercase name (`scp`);
- `authority` says where it goes (`me@host:2222`) and must not contain `\` or `/`;
- `folder` is the inner path to start in (`home\me`).

The program remembers the connection and shows it in the active panel. The file operations then reach the plugin with `Mount::root()` as `file`. The menu offers **Disconnect** for a panel showing a connection. Disconnecting, turning the plugin off or quitting calls `close(root)`, and panels showing the connection go back to the folder they showed before. A plugin keeps its connections itself, typically in a `Mutex<HashMap<authority, Connection>>`.

### Results windows

`Reply::Results(Results { title, name, mount, flat, again })` is for work that fills a connection while the user watches, such as a search. The program registers the connection like `Reply::Mount` does, but instead of opening it in a panel it opens a results window: a list of every item `list(root, "")` returns (name, folder, size, date; sortable), refreshed while the plugin works, and the buttons **Go to file**, **Show filtered folders** (the connection in the active panel), **Show all in one list** (the connection `flat`, if not empty), **Stop**, **New search…** (runs `again`, if not empty), **Refresh** (runs `refresh`, if not empty, with `results` set to the root of `mount`; it must fill the same connections again) and **Close**. A new `Results` reply replaces the window's results as if the window was closed (each connection no panel shows is closed). A session keeps the last 10 results; older ones are closed.

`flat` is the authority of a second connection of the same scheme with the same results in one list (the search plugin uses `<n>-list`). The program registers both, and closing the window closes each one no panel shows. Each is closed once (`close(root)`), so a plugin keeping one job behind both counts them. Results connections are listed under **Searches** in the drive selector and the Plugins menu, by `name` (a short name such as `*.txt "hello" · Documents`; the title when empty), and offered for **Close** instead of Disconnect. In the window, Open (Enter, double-click), F3 and F4 open, view and edit the item's `real_path`.

Three methods of `FolderPlugin` serve it; each has a default, so other plugins ignore them:

- `activity(root) -> Activity { running, text }`: what the plugin is doing for the connection. The window shows `text` as its status line and asks again every 0.4 s while `running`, then every 3 s.
- `stop(root)`: the window's Stop button. Keep what was done so far.
- `real_path(file, inner) -> Option<PathBuf>`: the file on disk an item stands for. Go to file shows it in its folder, and Enter, F3 and F4 open, view and edit that file itself instead of a temporary copy (in any connection or archive whose items have one).

A search runs on a thread of the plugin's own, started by `run`, which returns at once. The search plugin keeps its searches in a `Mutex<HashMap<authority, Arc<Job>>>`, with atomic counters and a `Mutex<Vec<Found>>` the search thread fills and `list` and `activity` read.

**Security.** Passwords reach the plugin only in `values`; don't log them, put them in error messages, or save them (the SCP plugin saves the last server, port, user and key path, never a password). Check what a server proves about itself (the SCP plugin verifies host keys against `known_hosts` and asks before trusting a new one), and treat every name a server sends as untrusted (see [Rules and pitfalls](#rules-and-pitfalls)).

## Viewers and editors

Any plugin can open files for the user: `openers()` returns its viewers (`OpenKind::View`, F3) and editors (`OpenKind::Edit`, F4), each with an id, a translated label and the file names it takes (`masks`: `*.txt;*.log`, `*` for any file). On F3 or F4 the program picks the first plugin, in the order of Settings → Plugins, with an opener of that kind matching the file's name, and calls `open_file(opener id, file, options)`; a program the user chose in Settings comes first, and without an editor plugin F4 opens Notepad.

- **`file` is always a file on disk.** An item inside an archive or on a server is extracted to a temporary folder first; `options["temporary"]` is `1` then, and changes to it are not saved back.
- **`options`** say how the program looks, so the window can match it: `theme` (`system`, `light`, `dark`), `ui_font` and `mono_font` (font names, empty for the default), `scrollbars` (`solid`, `floating`), `accent` (the accent color as `#rrggbb`, empty for the theme's; `sal_ui::apply_accent` applies it).
- **Return at once.** `open_file` runs on the program's UI thread: hand the file to your window and return; don't wait for the window to close.
- **The window is yours.** This is the one place a plugin shows UI of its own. The Viewer plugin uses egui (`eframe`) in a thread of its own with an event loop of its own (`with_any_thread`), kept for the whole session because an event loop can be created only once per DLL; `sal-ui` gives it the program's fonts and scroll bars. Close your windows in `Drop`, which runs when the plugin is turned off.

## Versions and updates

A plugin has its **own version** (`Plugin::VERSION`), apart from the program's, and is released on its own. In this repository each plugin's crate has its own `version` in its `Cargo.toml` and its own `CHANGELOG.md`, with a `## <version> (<date>)` heading per version.

`Plugin::UPDATES` says where releases are published, and the program's update check (Help → Check for updates, and at start-up) looks there too:

- **`repo`**: the GitHub repository, `owner/name`. The plugins in this repository use `nocturneop15/RustySalamander`; a plugin in a repository of its own names that one.
- **`tag`**: what the tags of the plugin's releases start with, followed by the version: `viewer-v` → `viewer-v0.6.0`. The program's own releases are `v<version>`.
- **`changelog`**: the changelog's path in the repository (`plugins/viewer/CHANGELOG.md`). The update window shows the sections between the installed version and the new one, read at the release's tag.

**A release** of a plugin has one zip, named `<id>-<version>-api<interface>.zip` (`viewer-0.6.0-api5.zip`), holding the plugin's folder: the `.spl`, its libraries, `lang`, licenses and the changelog. The interface number is `abi::VERSION` the plugin was built for; the program only offers a zip for an interface it has. Several zips for different interfaces may be attached to one release.

**Installing:** the zip is unpacked into `plugins\.updates\<folder>`, and the next start copies it over the plugin's folder before the plugins are loaded (the DLL is in use while the program runs). Files the plugin saved in its folder stay.

## Errors, progress and cancelling

Methods return `std::io::Result`. The error's **kind** crosses the interface as a code, and its **message** (`to_string()`) is shown to the user as it is. Write messages for people, and translate them.

| `io::ErrorKind` | What the program does |
|---|---|
| `Interrupted` | Treats the operation as **cancelled by the user**: no error message. Return it when `progress` returned false. |
| `AlreadyExists`, `NotFound`, `PermissionDenied`, `InvalidData`/`InvalidInput` | Keeps the kind, so tests and future features can tell them apart. |
| anything else | Shows the message. |

**Progress.** Each changing method gets `progress: &mut Progress`, a `FnMut(done, total) -> bool`.

- Call it regularly with **bytes** done and total. The program shows them in the operation's progress window, and `done == total` marks the end.
- If it returns **false**, the user cancelled: stop as soon as possible, leave the file as it was, and return `ErrorKind::Interrupted`.
- The call **blocks** while the user keeps the operation paused. That is how pause works, so don't call it while holding a lock other threads need.
- A plugin that can't measure bytes may report items instead (e.g. `progress(i, n)`), as the example's `add` does.

**Panics** are caught at the interface and reported as "the plugin failed: …". Don't rely on that: a panic can leave half-written files behind.

## Translations

The program translates a plugin's texts from the plugin's own `lang` folder, in the same format as the program's language files:

```toml
[language]
code = "cs"
name = "Čeština"
plural = "cs"

[strings]
"File lists" = "Seznamy souborů"
"A file list can't contain folders." = "Seznam souborů nemůže obsahovat složky."
```

In code, from `sal_plugin_api`:

- **`n_("…")`** marks a constant text for translation without translating it: the `NAME` and `DESCRIPTION` constants, or texts kept in tables. The program translates `NAME` and `DESCRIPTION` itself.
- **`tr("…")`** translates a text now, into the language the user has chosen.
- **`trf("Cannot open {file}.", &[("file", &name)])`** translates, then fills in `{placeholders}`. Translators may move placeholders within the sentence.

Guidelines:

- Translate **whole sentences**. Never glue fragments together, because word order differs between languages.
- A text without a translation is shown in English, so partial translations are fine.
- Only texts in **string literals** passed to `n_`, `tr` or `trf` can be found by the tools below.

Inside this repository, `cargo test -p sal-app export_english_strings -- --ignored` writes each plugin's `lang/en.toml` from its source. The `language_files_match_the_code` test then fails when a language file lacks a string the code uses, or has one it no longer uses. Outside the repository, write `en.toml` by hand in the same format.

## Testing

**Unit tests** can call the plugin's methods directly, as ordinary Rust.

**Through the C interface.** To test exactly what the program will call, use the program's own wrapper:

```rust
use sal_plugin_api::abi::{HostApi, Str, StrOut, VERSION};

unsafe extern "C" fn translate(_: *mut std::ffi::c_void, msg: Str, out: *const StrOut) {
    (*out).send(msg.get()); // no translation in tests
}
static HOST: HostApi = HostApi { version: VERSION, ctx: std::ptr::null_mut(), translate };

let desc = unsafe { &*rustysalamander_plugin() }; // generated by export_plugin!
let plugin = unsafe { sal_plugin_api::host::start(desc, &HOST, Path::new(".")) }.unwrap();
// plugin is an Arc<dyn FolderPlugin>, just as the program sees it
```

`plugins/example-filelist/src/lib.rs` has a complete test written this way. It covers listing, copying out with progress, cancelling, copying in, deleting, and refused operations.

**In the program.** Put the built `.spl` into a folder of its own and start the program with `RUSTYSALAMANDER_PLUGINS` pointing at that folder's parent. You don't need to copy anything next to the exe:

```powershell
$env:RUSTYSALAMANDER_PLUGINS = "D:\dev\myplugins"   # contains mything\mything.spl
target\release\rustysalamander.exe
```

Test with names outside the ASCII range (`složka 日本\soubor 🦎.txt`), large files, cancelling in the middle, and the file opened in both panels.

## Building, installing and distributing

- **Toolchain:** the program is built with the `x86_64-pc-windows-gnu` Rust toolchain. A plugin can use the GNU or the MSVC toolchain, because only the C interface is shared, but it must be **64-bit**.
- **Debug or release:** both work with either build of the program.
- **Install:** copy the DLL as `plugins\<name>\<name>.spl`, with any libraries it needs and its `lang` folder. The program also loads loose `plugins\*.spl` files, but a folder per plugin keeps dependencies apart.
- **Inside this repository:** `crates/sal-app/build.rs` builds the plugins and installs them into `target\<profile>\plugins\<folder>`. To ship a new plugin with the program, add a similar step there. The example plugin is deliberately not installed.
- **Releasing a plugin on its own:** raise `version` in its `Cargo.toml`, add the section to its `CHANGELOG.md`, and publish a release tagged `<tag><version>` with the zip `<id>-<version>-api<interface>.zip` (see [Versions and updates](#versions-and-updates)).
- **Licensing:** RustySalamander is GPL-2.0-or-later. A plugin is loaded into the program's process; take that into account when choosing its license. Ship the licenses of the libraries you bundle; the 7-Zip plugin includes 7-Zip's.

## Rules and pitfalls

- **Don't keep files open between calls.** Windows locks open files, and the program must be able to rename, move or delete the file at any time. Open, work, close, within one call.
- **Don't show any UI** (message boxes, dialogs) except the windows of your viewers and editors. Report problems through errors and `status()`. A plugin that blocks on a window blocks a file operation, or the whole program if it was called from the UI thread.
- **Load libraries quietly.** A damaged DLL makes `LoadLibrary` show a Windows "Bad Image" box that blocks until someone clicks it. Set `SetThreadErrorMode(SEM_FAILCRITICALERRORS)` around the call and restore it afterwards (see `plugins/sevenzip/src/library.rs`).
- **Never trust names inside the file.** Inner paths come from data you read. Reject `..` components, drive letters and absolute paths before writing `out\<inner path>` in `extract`.
- **Keep everything Unicode.** Use `Path`/`PathBuf` for disk paths (they round-trip every Windows name) and `String` for inner paths. Never go through ANSI code pages.
- **Long paths:** paths may exceed 260 characters, and Rust's `std::fs` handles that. A C library may need the `\\?\` prefix.
- **Thread safety:** see [the trait section](#the-folderplugin-trait-method-by-method). `list` may run while a background `add` is writing the same file. The write-then-rename pattern keeps that safe.
- **The DLL is never unloaded** during a session, even when the plugin is turned off. `Drop` of your plugin type runs when it is turned off; release resources there.

## The C interface and versioning

`export_plugin!` generates the C interface from the traits, so Rust plugins never need to see it. It is documented in `sal_plugin_api::abi` for the curious and for plugins written in other languages:

- The DLL exports `const Descriptor *rustysalamander_plugin(void)`. The descriptor holds the interface version, id, name, description, a `start` function, and a table of functions (`VTable`) that each take the pointer `start` returned.
- Strings are UTF-8 pointer and length pairs (`Str`); paths are UTF-16 pointer and length pairs (`Wide`). Neither is NUL-terminated, and both are borrowed for one call only.
- Strings and lists travel back through callbacks (`StrOut`, `ItemOut`) into memory owned by the caller, so each side frees only what it allocated.
- Functions that can fail return `0` or an error code (`ERR_*`) and send a message to the `error` callback.

Commands, replies, forms and values cross the interface as text: lists of strings, each encoded as `<length>:<bytes>,` (`sal_plugin_api::ui::encode`). This way forms can gain new kinds of fields without changing the function table.

**Versioning.** `abi::VERSION` is the interface version (now 5: the plugin's own version and update source in the descriptor, `openers` and `open_file`). From version 5 on, the interface only grows: a new version adds fields at the end of `Descriptor` and `VTable`, each documented with the version that added it, and the program keeps running plugins built for any version from `abi::MIN_VERSION` on. It reads a field or calls a function only when the plugin's version has it, and uses the trait's default otherwise. Plugins built for a newer interface than the program's, or older than `MIN_VERSION`, are skipped and listed in Settings with the reason. Plugins built for 4 or earlier must be rebuilt.

## Checklist

- [ ] `crate-type = ["cdylib"]`, 64-bit, `export_plugin!(YourType)`.
- [ ] Unique, stable `ID`; `NAME` and `DESCRIPTION` marked with `n_`; `VERSION` from `Cargo.toml`, and `UPDATES` if you publish releases.
- [ ] Viewers and editors: masks as narrow as the files you really open; `open_file` returns at once.
- [ ] `start` returns helpful, translated errors (missing library, wrong version).
- [ ] `handles` decides by name only and is fast (or returns false for a plugin that only opens connections).
- [ ] `list` returns a complete index, or the asked-for folder (`complete: false`), and `NotFound` for what isn't a folder.
- [ ] Menu commands have translated labels; forms use `Password` for secrets, which are never saved or logged.
- [ ] Connections are released in `close`; names from a server are checked before they become paths on disk.
- [ ] `can_write` is false for anything you can't change safely.
- [ ] `extract` handles folders recursively and empty folders, and rejects `..` in inner paths.
- [ ] Changes go through a temporary file and a final rename.
- [ ] `progress` is called and respected; cancelling returns `ErrorKind::Interrupted` and leaves the file unchanged.
- [ ] No open file handles between calls, no UI, no panics.
- [ ] `lang/en.toml` (and translations) next to the DLL.
- [ ] Tested with Unicode names, large files and cancelling, through the C interface and in the program.
