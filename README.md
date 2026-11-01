# RustySalamander

A Rust rewrite of [Open Salamander](https://github.com/OpenSalamander/salamander), the dual-pane file manager for Windows, with full Unicode (UTF-8) support.

The original is ~1.3 million lines of ANSI C++ (`char*` + `MAX_PATH`), so files whose names fall outside the system code page show up as `?` and can't be opened. RustySalamander uses lossless paths everywhere. See [docs/ANALYSIS.md](docs/ANALYSIS.md) for the analysis of the original code and the rewrite plan.

> **Status: alpha.** The everyday file manager works (panels, file operations, archives, viewer, settings), but many of Salamander's features are still missing, and the plugin interface may change between releases.

Documentation:

- [docs/PLUGINS.md](docs/PLUGINS.md): writing plugins
- [docs/DEVELOPMENT.md](docs/DEVELOPMENT.md): building, testing and contributing
- [docs/ANALYSIS.md](docs/ANALYSIS.md): the original code and the rewrite plan
- [CHANGELOG.md](CHANGELOG.md): what each release contains

## Unicode model

- **Identity:** every item carries a `PathBuf` (WTF-8 on Windows). This round-trips every NTFS name, including names with unpaired UTF-16 surrogates that aren't valid Unicode. All operations use it.
- **Display/matching:** UTF-8 `String`. Masks and quick search use Unicode case folding (`*.ŽÁK` matches `žák.žák`, `ελ` finds `Ελληνικά`). Names sort in the alphabetical order of the account's language, as Explorer has them (Windows' sort keys): `Čeština` after `Cena`, not after `Zeta`; numbers by value.
- **No MAX_PATH:** std uses the wide Win32 APIs with automatic `\\?\` handling. Tests cover paths longer than 300 characters.
- **Bidi:** Hebrew and Arabic names are reordered with the Unicode Bidirectional Algorithm.
- **Characters Windows forbids:** names from Macs, Samba/NAS shares or WSL may contain `* : ? | < > " \`. Windows stores these as private-use stand-ins (U+F020–F029 or U+F000+char) that no font can draw, so they're shown as the original characters. This is display only; operations use the exact name.
- **Fonts:** a fallback chain over the Windows system fonts covers CJK, Korean, Indic, Thai, symbols and emoji.
- **Icons:** [Material Design Icons](https://pictogrammers.com/library/mdi/) (built in): drives, connections, searches, the context menu, sort marks, and files and folders when Windows' file icons are turned off.
- **Viewer** (the Viewer plugin): colors text by its format (JSON, XML, HTML, PowerShell, Bash and about 200 more, recognised from the file name or the first line, or chosen in the Syntax list) and detects UTF-8, UTF-16 LE/BE (with or without a BOM) and the system ANSI code page. You can switch to Windows-125x, ISO-8859-2, KOI8-R, CP866, Shift-JIS, EUC-JP, GBK, Big5 or EUC-KR.
- **Config:** stored as UTF-8 TOML in `config.toml` next to `rustysalamander.exe` (portable mode) instead of the registry. See [Portable mode](#portable-mode).

## Layout

```
crates/sal-core   Unicode-native core library (no GUI)
  entry.rs        panel item (replaces CFileData)
  listing.rs      directory reading
  sort.rs         panel order: the language's alphabetical order (Windows), numbers by value
  masks.rs        masks / mask groups / MaskName, port of masks.cpp
  quicksearch.rs  quick search, port of the QS code in masks.cpp
  fileops.rs      copy / move / delete / mkdir / rename with progress, conflicts, retry
  encoding.rs     text encoding detection, bidi, formatting
  drives.rs       drive list, free space
  shell.rs        open / edit / terminal / Recycle Bin (wide Win32 APIs)
  config.rs       TOML configuration (portable: next to the program)
crates/sal-app    GUI (egui/eframe)
  app.rs          main window, keyboard model, dialogs, progress, headless UI tests
  panel.rs        file panel state (cursor, selection, history, quick search)
  worker.rs       background file operations
  i18n.rs         translations (tr / trf / trn), language switching
  plugins/        finding and loading plugin DLLs, virtual paths through their files
    mod.rs        the plugins folder, Settings → Plugins state, the program's side of the interface
    ops.rs        operations inside a plugin's files, run by the worker (extract, add, delete, rename)
  lang/*.toml     language files: en.toml (all UI strings), cs.toml; build.rs copies them next to the exe
  update.rs       updates of the program and of the plugins from GitHub, changelogs
  exe_icon.rs     keeps res/rustysalamander.ico (the exe's icon) the same as the window icon
  res/            the exe's icon
  build.rs        copies the languages, builds the plugins into target\<profile>\plugins, gives the exe its icon and version
crates/sal-ui     what the program's and the plugins' windows share: fonts (Unicode fallback), scroll bars,
                  the accent color, the Material Design Icons (mdi.rs) and the window icons drawn from them (icon.rs)
crates/sal-plugin-api   what the program and plugins share (no GUI)
  lib.rs          FolderPlugin / Plugin traits, Item, Index, Updates (where a plugin is published)
  ui.rs           commands, forms, replies, openers (viewers and editors), as text across the interface
  abi.rs          the C interface between the program and a plugin DLL
  export.rs       the plugin's side: export_plugin!, tr / trf through the program
  host.rs         the program's side: a plugin DLL wrapped back into a FolderPlugin
plugins/sevenzip  the 7-Zip plugin, a DLL of its own (plugins\7zip\7zip.spl)
  lib.rs          the plugin (formats, list / extract / add / delete / rename)
  library.rs      7z.dll in-process: interfaces, streams, callbacks
  lang/*.toml     the plugin's own texts: en.toml, cs.toml (copied to plugins\7zip\lang)
plugins/scp       the SCP plugin (plugins\scp\scp.spl): SSH servers in a panel
  lib.rs          the plugin: the Plugins menu command, forms, file operations
  ssh.rs          the SSH connection (russh): host keys, sign-in, the SCP protocol
  remote.rs       server paths, shell quoting, reading ls output
plugins/search    the search plugin (plugins\search\search.spl): Find files, results windows
plugins/viewer    the Viewer plugin (plugins\viewer\viewer.spl): F3 text/hex viewer in windows of its own
plugins/compare   the Compare plugin (plugins\compare\compare.spl): files side by side, folders in both panels
  doc.rs          a file as text in an encoding, or as hex
  window.rs       the viewer windows, on a thread and event loop of the plugin's own
plugins/*/CHANGELOG.md   each plugin's own changelog (its own versions)
plugins/example-filelist   a small example plugin for plugin authors (not installed)
tools/            subset-mdi.py: cuts the icon font down to the icons in use
docs/             plugin guide, development guide, analysis of the original
third_party/7zip-26.05/   7-Zip source (LGPL): built into 7z.dll
```

## Building

The project uses the `x86_64-pc-windows-gnu` Rust toolchain with MinGW-w64, so Visual Studio isn't needed. You need:

- **Rust** 1.88 or newer via [rustup](https://rustup.rs/), with the GNU toolchain: `rustup default stable-x86_64-pc-windows-gnu`.
- **MinGW-w64** (gcc, g++, `mingw32-make`), for example [WinLibs](https://winlibs.com/) (`winget install BrechtSanders.WinLibs.POSIX.UCRT`). It links the program and builds 7-Zip's `7z.dll`. Without it the program still builds, but without the 7-Zip plugin's library.

```powershell
. .\env.ps1                 # puts cargo + the WinLibs MinGW on PATH for this shell
cargo test --workspace      # core, plugin interface, plugins, headless UI
cargo build --release       # target\release\rustysalamander.exe, plus lang\ and plugins\ next to it
```

The first build compiles `7z.dll` from source, which takes a few minutes; later builds reuse it. Run the program with optional start directories: `rustysalamander.exe [left] [right]`. More in [docs/DEVELOPMENT.md](docs/DEVELOPMENT.md).

## Keyboard

All command shortcuts are configurable in **Options → Settings… → Keyboard shortcuts**. The table is split into three framed sections, with *Jump to* links at the top:

- **File & folder operations**: view, edit, copy, move, rename, create, delete, copy paths, selection, folder sizes.
- **Navigation**: change directory, parent/enter, drive root, history, drive menus, and the drive-letter modifier.
- **System & view**: sorting, refresh, hidden files, swap panels, command prompt, settings, help, exit.

Each section also lists its fixed keys. The F1 help is grouped the same way. Each command has:

- **Preset**: Default (the Salamander binding), the single keys from the default set, alternatives such as `Ctrl+L` for Change directory or `Alt+Up` for Parent, or **None (disabled)**. The column shows **Custom** once you've edited the bindings.
- **Shortcuts**: the current bindings. Click one to remove it. **+ Add…** records a custom key combination (Esc cancels).
- **Conflicts**: a binding shared with another command is shown in red, and the tooltip names the other command.

The drive-letter modifier is set at the top of the same table. **Reset all shortcuts to defaults** restores everything. The F1 help and the menus always show the current bindings. Only changes from the defaults are stored, in the `[shortcuts]` table of `config.toml`. An empty list disables a command's shortcut:

```toml
[shortcuts]
copy = ["F5", "Ctrl+K"]
delete = []
```

Default bindings (Salamander-compatible):

| Key | Action |
|---|---|
| F1 / Shift+F1 | help / open command prompt here |
| F2, Shift+F6 | rename |
| F3 / F4 | view / edit |
| F5 / F6 | copy / move (target can be a dir, a new name, or a mask like `*.bak`) |
| F7 / Shift+F7 | create directory / type a path in the path bar |
| F8, Delete | delete (Shift = permanently) |
| F10 | exit |
| Backspace, Ctrl+PgUp / Ctrl+PgDn | parent / enter directory |
| Ctrl+Backspace, Ctrl+\ | root of the current drive |
| Alt+← / Alt+→ | history |
| Alt+F1 / Alt+F2 | drive menu (left / right) |
| Shift+A..Z | go to that drive (modifier configurable: Shift / Alt / Ctrl+Shift / disabled) |
| Ctrl+F3..F6 | sort by name / ext / time / size |
| Ctrl+A / Ctrl+D | select all / unselect all |
| Shift+Space | select + calculate folder size in the background (configurable; preset `Space` makes plain Space do it, Salamander-style) |
| Shift+F10 / Alt+Enter | context menu / properties |
| Alt+F10 | calculate size of the selected folders, or the folder under the cursor (preset: Ctrl+Q) |
| Ctrl+R / Ctrl+H / Ctrl+U | refresh / show hidden files / swap panels |
| (none; preset Ctrl+Shift+H) | show system files |
| Ctrl+Shift+C | copy full paths |

Fixed keys: arrows, PgUp/PgDn, Home/End, Enter, Tab, Insert/Space (select), `+` `-` `*` (select by mask / unselect / invert), typing (quick search, Ctrl+↑↓ for the next match), Esc, and Ctrl+0..9 / Ctrl+Shift+0..9 (hot paths). Configured shortcuts take precedence over fixed keys.

## Archives (the 7-Zip plugin)

Archives open as folders in the panel. **Enter** on `archive.zip` shows what is inside, and you browse it like any folder. `..` at its root leaves it, with the cursor on the archive. A path that goes through an archive works everywhere: typed into the path bar, in the history, as a hot path, or remembered as the panel's folder.

- **7-Zip's library, inside the program.** The plugin (`plugins\7zip\7zip.spl`) loads `7z.dll` from its own folder into the program's process and works with archives through the library's interfaces (`IInArchive`, `IOutArchive`). It is built from the 7-Zip 26.05 source in `third_party/7zip-26.05` (GNU LGPL; the RAR code also under the unRAR restriction). Nothing installed on the computer is used. The build compiles the library once with MinGW and puts it into the plugin's folder.
- **Formats:**
  - **Read:** zip, 7z, rar, tar, gz/tgz, bz2, xz, zst, lzma, Z, cab, arj, lzh, iso, wim, cpio, rpm, deb and split archives (.001).
  - **Changed in place** (add, delete, rename, new folder): 7z, zip, tar and wim.
  - Other files 7-Zip could read (exe, docx, msi, …) still open normally.
- **Reading:** Enter, F3 and F4 extract the file into a temporary folder and open it there, with the program Windows associates, the viewer or the editor. Edits to that copy don't go back into the archive. Folder sizes inside an archive are shown at once (Size column, tooltip, footer).
- **Copying out (F5/F6):** works to any folder, with the usual conflict window and progress. Moving out removes the items from the archive, unless some were skipped.
- **Copying in:** works to any folder of a writable archive. Items that are already there are asked about first (overwrite, skip, cancel). Copying between two archives works too.
- **Changing an archive:** F8 deletes items from the archive (no Recycle Bin), F7 creates folders (`/a/b` too), and F2 renames files and folders. Every change writes a new archive next to the old one, which then replaces it, so large archives take a while. A failed or cancelled change leaves the archive as it was. Archive operations show progress and can be paused and cancelled like copies.
- **Not available inside archives:** the Windows context menu and Properties. Encrypted archives (passwords) aren't supported yet.
- **Turning it off:** in Settings → Plugins (see below). Enter then hands archives to Windows like any other file.
- **Without the plugin or its library:** when `7zip.spl` is missing, or `7z.dll` is missing or damaged, archives are ordinary files. Enter opens them with Windows, F3 shows the file itself, and a panel remembered inside an archive opens the archive's folder instead. Copying into `archive.zip\` is refused with "… is a file, not a folder", and the archive stays untouched. Settings → Plugins says what is missing.

## Servers (the SCP plugin)

**Plugins → Connect to server…** opens a form for the server, the port, the user name, and a password or a private key (OpenSSH or PEM, with its passphrase). The server's files then open in the active panel as `scp://user@host/home/user`, in the user's home folder. Copying in and out (F5/F6), deleting (F8), renaming (F2), new folders (F7), viewing and editing (F3/F4) work as in any folder, with progress, pausing and cancelling. `scp://user@host/path` can also be typed into the path bar of a connected server.

- **How:** SSH in pure Rust (`russh`); files go over the SCP protocol (`scp -f`/`-t` on the server), and listings and file management use `ls`, `rm`, `mv` and `mkdir`, so any Linux, BSD or macOS server works. Nothing installed on the computer is used.
- **Server keys:** a key from `%USERPROFILE%\.ssh\known_hosts` or the plugin's own `known_hosts` is trusted. A new server shows its key's fingerprint first, and the key is remembered once you trust it. A changed key is refused with a warning.
- **Sign-in:** password (servers that ask through keyboard-interactive prompts get it too) or a private key. The last server, port, user and key file are remembered, never a password.
- **Drive selector:** connected servers are listed under *Connections* in the drive selector (Alt+F1 / Alt+F2), so either panel can open one in its start folder; the selector starts on it while the panel shows that server. Each has a **Disconnect** button there (or press Delete on it).
- **Disconnecting:** **Plugins → Disconnect from …** lists every open session, one per row (the menu widens to fit), or use the drive selector's Disconnect button; panels go back to the folder they showed before. Quitting the program disconnects too.
- **Limits:** no SFTP, no saved sites yet, and a lost connection needs a disconnect and a new connection. Panels of a server are refreshed with Ctrl+R, not automatically.

## Finding files (the search plugin)

**Plugins → Find files…** (Alt+F7) opens the search form:

- **Names:** wildcard masks such as `*.txt;*.md`, with masks to leave out after `|` (`*.log|old*`), as in Salamander, or a regular expression (found anywhere in the name; `^…$` for whole names).
- **Look in:** the folder to search (the active panel's folder to start with), with or without its subfolders. Hidden and system items can be left out; symbolic links and junctions are listed but not entered.
- **Containing text:** optional. Plain text is found in UTF-8, ANSI and UTF-16 files, in any case unless **Match case** is on; or the text is a regular expression (`^` and `$` match at line starts and ends).
- **Find folders too:** folders whose names match are results as well (not when looking for text).

The search runs in the background. The results window shows the results as they are found, with what is being searched and the counts; columns sort with a click. **Open** (Enter or a double-click) opens a found file with its program, or a found folder in the active panel; F3 views and F4 edits the selected file. **Go to file** shows a result in its folder in the active panel. **Stop** ends a search early, **New search…** opens the form again with the last values, and **Refresh** runs the same search again into the same results (panels showing them follow).

The results can also be browsed in a panel, in two ways:

- **Show filtered folders** (`search://1`): the searched folder as a filter over the file system. Its folders are all there to browse, but they hold only what was found, so a match deep down is reached through the folders above it.
- **Show all in one list** (`search://1-list`): everything found, files and folders, in one list without the folders leading to them. Items with the same name are numbered (`zpráva (2).txt`); Go to file and the results window show where each one is. A found folder opens to what it really contains.

The path bar names them by what was searched for (`🔍 *.txt "hello" · Documents (filtered folders) › docs`); `search://1/docs` is their address, shown in tooltips and accepted when typed. In both, Enter, F3 and F4 open, view and edit the real files, F5 copies the results out (in the filtered folders, with the folders between), F8 moves them to the Recycle Bin and F2 renames them. Closing the window ends the results unless a panel shows them; then they stay in the drive selector under **Searches**, by name (`🔍 *.txt "hello" · Documents (all in one list)`), with **Refresh** (the same search again) and **Close** (or Delete), until closed there or in the Plugins menu, or until the Search plugin is turned off. A new search replaces the one in the results window: the earlier results stay only where a panel shows them; a session keeps the last 10, closing the oldest.

## Comparing files and folders (the Compare plugin)

**Plugins → Compare** compares what is chosen in the panels: two items selected in the active panel; else the item selected in each panel; else the items under the two cursors (both files or both folders); else the two panels' folders. The left panel's is the left side. **Compare files…** and **Compare folders…** ask for the two paths in a form; the folder form also asks when files count as the same: when their size and date are (two seconds apart allowed, for FAT), or when their contents are (slower). Only files and folders on disk can be compared, not items inside archives or on servers.

**Two files** open in a window of their own, side by side and lined up line by line, with line numbers. Changed lines are amber, lines only the left file has red, lines only the right file has green. The toolbar says which difference of how many is shown; **Next** and **Previous** (F8 and Shift+F8, or Alt+↓ and Alt+↑) go between them. **Ignore spaces** treats lines that differ only in spaces and tabs as the same, and **Compare again** reads the files again. Text is compared by its characters, whatever its encoding. Binary files, and files over 32 MB, show their sizes and the first byte where they differ.

**Two folders** open in both panels at once, the left folder in the left panel and the right one in the right (`compare://1-left`, `compare://1-right`). Each panel lists everything either side has, so the two lists line up, and the rows are colored:

- **amber:** different (a folder: something inside differs);
- **green:** only this side has it;
- **grey text, no size:** only the other side has it; it stands in so the lists line up;
- **no color:** the same.

Going into a folder or up in one panel takes the other panel along. Each side stays in its own panel: the drive selector offers a panel only its own side, and the other side typed or swapped into it turns into the same folder of its own side. The items are the real files: Enter, F3 and F4 open, view and edit them; F5 from one panel copies into the other side's real folder, F8 moves items to the Recycle Bin and F7 creates a folder, and the colors follow. Renaming isn't offered. **Refresh** in the drive selector, where the comparison is listed under **Comparisons**, compares again (after changes made elsewhere), and closing either side closes both, the panels going back to where they were. There is one comparison at a time: a new one closes the one before.

## Plugins

Optional features are plugins: DLLs (`*.spl`) the program doesn't contain. Each lives in its own folder under `plugins` next to `rustysalamander.exe`, with whatever it needs (`plugins\7zip\7zip.spl` and `7z.dll`). When the program starts, it looks at `plugins\*.spl` and `plugins\<folder>\*.spl`, so a plugin is installed by copying its folder there and removed by deleting it. Files that aren't usable plugins (not a DLL, made for another version of the interface, an id already taken) are listed in Settings with the reason. `RUSTYSALAMANDER_PLUGINS` names another plugins folder.

**Settings → Plugins** lists them with a description and their state, for example the library a plugin uses or why it can't start. Each has a switch, and turning one on or off applies at once. A panel showing a folder inside a plugin's file goes back to that file's folder when the plugin is turned off, and a panel showing a plugin's connection goes back to where it was before. Turned-off plugins are saved in `config.toml` (`disabled_plugins`).

**The Plugins menu** lists the commands of the plugins that are on, under each plugin's name, plus *Disconnect* for a panel showing a connection and *Plugin settings…*. A panel that a plugin shows (an archive, a server) has a tinted background and a banner with the plugin's name below the list.

- **Folder plugins:** a *folder plugin* makes certain files, or connections it opens, browsable as folders. The panel lists them, and copy, move, delete, rename and new folder work inside them through the plugin. Plugins can add menu commands and ask for input in forms the program draws.
- **Viewers and editors:** any plugin can register as the viewer (F3) or editor (F4) of files whose names match its masks (`*.txt;*.log`, or `*` for all). Settings → Viewers and editors offers them as the default viewer or editor, or for a file suffix (see [Viewers and editors](#viewers-and-editors)). Settings → Plugins shows what each plugin views or edits.
- **Versions of their own:** each plugin has its own version (Settings → Plugins shows it, with the interface it was built for) and its own releases, so a plugin is updated without a new program (see [Updates](#updates)). The program runs plugins built for its interface version or an older one it still supports.
- **Interface:** the program and a plugin talk only through a C interface (`crates/sal-plugin-api`), so each plugin is built on its own. Each plugin brings its own language files in its `lang` folder (`plugins\7zip\lang\cs.toml`).
- **Shipped:** the 7-Zip archive plugin, the SCP plugin, the search plugin, the Viewer plugin (the F3 viewer) and the Compare plugin. `plugins/example-filelist` is a small example for plugin authors and isn't installed.
- **Writing a plugin:** see [docs/PLUGINS.md](docs/PLUGINS.md).

## Portable mode

Everything the program keeps lives in its own folder, so the folder can be copied or carried on a USB stick:

```
rustysalamander.exe
config.toml        settings, keyboard shortcuts, hot paths, typed paths (written on exit)
lang\en.toml       English UI strings (the template for translations)
lang\cs.toml       Czech
plugins\7zip\      the 7-Zip plugin: 7zip.spl and 7z.dll, 7-Zip's archive library
plugins\scp\       the SCP plugin: scp.spl; known_hosts and the last connection's details
plugins\search\    the search plugin
plugins\viewer\    the Viewer plugin (F3)
plugins\.updates\  plugin updates downloaded and waiting for the next start
```

- **First start:** when the folder has no `config.toml` yet, the settings are read from `%APPDATA%\RustySalamander\config.toml` (the location used before) and saved next to the program on exit. The old file is left untouched.
- **Read-only folder:** if the program folder isn't writable (e.g. under Program Files), settings are saved to `%APPDATA%\RustySalamander\config.toml` instead.
- **Other location:** the `RUSTYSALAMANDER_CONFIG` environment variable can name a different configuration file.
- **Development builds:** the program folder is `target\release` (or `target\debug`), so `cargo clean` deletes the settings saved there. Copy `config.toml` elsewhere first if you want to keep it.

## Updates

At start-up the program asks GitHub whether newer releases exist, of the program and of each plugin (plugins are released on their own, with their own versions). If any do, a window lists them, each with the version you have, the new one, a link to its release page and **Show changelog**, which shows what changed between the two (read from the changelog at the new release) before anything is installed.

**The program:** **Update now** downloads it and unpacks it into a folder named by the version, next to the folder the program runs from (`…\RustySalamander\0.2.1-alpha` → `…\RustySalamander\0.3.0-alpha`), then copies the settings there: `config.toml` and the files plugins saved in their folders (such as the SCP plugin's `known_hosts`). **Start the new version** opens it and closes the old one. The old version stays as it was, so you can go back to it.

**A plugin:** **Update now** downloads it into `plugins\.updates`; it replaces the plugin's files when the program starts again (**Restart now** does that at once). What the plugin saved in its folder stays. A plugin is only offered in a build for an interface version this program has. A plugin release made for other program versions (its notes say which) is shown greyed, with the reason, and can't be installed.

**Update all** appears when more than one update can be installed: it starts all of them at once (not the greyed ones).

**Plugins for other program versions:** a plugin can say which RustySalamander versions it works with. One made for a newer program, or phased out before this one, isn't loaded; Settings → Plugins lists it with the reason.

**Help → Check for updates…** checks at any time. **Settings → General → Updates** turns the start-up check off (`check_updates` in `config.toml`).

## Languages

The UI language is chosen in **Options → Language** or **Settings → General → Appearance → Language**. The choice applies immediately. *System default* follows the Windows display language and falls back to English. English is built in; every other language is a file in the `lang` folder next to the program (Czech ships as `lang\cs.toml`).

- **Language files:** each language is a TOML file, `lang/<code>.toml`. The keys are the English texts and the values are the translations. `crates/sal-app/lang/en.toml` is the complete list of UI strings and the template for new languages. Placeholders such as `{name}` stay as they are.
- **Plural keys:** a key like `"{n} file|{n} files"` is a plural. Its value lists one form per plural category (`plural = "en"`: 1 / other; `plural = "cs"`: 1 / 2–4 / 5+).
- **Partial translations:** missing strings fall back to English, so a partial translation works.
- **Adding or editing a language:** put `<code>.toml` into the `lang` folder next to `rustysalamander.exe`, or edit a file that is already there. It appears in the menu after a restart. A build copies `crates/sal-app/lang/*.toml` over the files in `target\<profile>\lang`.
- **For developers:** wrap UI text in `tr("…")`, `trf("… {x}", &[("x", &v)])` or `trn("{n} item", "{n} items", n)`. Mark constants with `n_("…")` and pass them through `tr()` where they are shown. `cargo test -p sal-app export_english_strings -- --ignored` regenerates `en.toml`. The test `language_files_match_the_code` fails when `en.toml` or `cs.toml` is missing a string or has strings the code no longer uses.

## Dialogs

Dialogs (create directory, copy/move, rename, delete, select by mask, messages, Settings, keyboard help and the file-operation progress) open in their own windows. You can move them outside the main window, even onto another monitor. Each one opens centred over the main window, narrower than it, and sized to its content. It stays above the main window, minimizes with it and has no taskbar button of its own. **Enter** confirms; **Esc** or the window's close button cancels. When a dialog closes, the focus returns to the main window.

Copy/Move, Create directory, Rename, Delete, Select by mask and messages are modal: the main window takes no input until you confirm or cancel. Settings, keyboard help and the file-operation windows are not, so you can keep working while operations run. Dialog buttons are centred. All buttons in the program have the same, roomier padding, and progress bars have square corners.

**F7** (*Create directory*) accepts a path and creates every level of it inside the current folder: `/foo1/foo2/foo3`, `foo1/foo2` and `foo1\foo2` all work. A leading slash still means the current folder, not the drive root. Levels that already exist are reused, and the cursor lands on the first one.

## File operations (copy, move, delete)

- **Starting:** the Copy/Move dialog has *Copy* (or *Move*), *Run first*, *Queue last* and *Cancel*.
  - *Copy* starts at once, alongside anything already running.
  - *Run first* puts the operation on top: the running operations pause, this one runs, and they go on when it finishes. Operations you paused yourself stay paused.
  - *Queue last* waits until the running and the already waiting operations finish. When nothing is running, it starts right away.
  - The selection in the panels is cleared as soon as the operation is started or queued.
- **The *File operations* window:** each running operation has two progress bars (bytes and files), the current file, the elapsed time, the speed and an estimate of the time left. The estimate uses the speed over the last few seconds, and time spent paused or waiting for an answer doesn't count. **Pause / Resume** and **Cancel** apply to each operation.
- **The queue:** below the progress, each waiting operation has ⏶ / ⏷ to reorder it, *Start now* to run it at once alongside the others, and × to remove it. ⏶ on the first waiting operation puts it on top, which works like *Run first*.
- **Controls for everything:** at the bottom of the window, on a strip shaded like the panels' status bar: *Pause all* / *Resume all*, *Cancel all* (stops the running operations and empties the queue) and *Run in background*.
- **Running in the background:** *Run in background*, **Esc** or the window's close button hides the window, and the operations go on. The status bar then shows a progress bar with the percentage, the time left and how many operations are waiting. Click it to show the window again. The panels stay usable while operations run.
- **Conflicts:** when the target already exists, a separate window lists both items side by side: name, folder, size and modification time. The larger and the newer item are marked. Errors show the item and the problem, with *Retry*, *Skip*, *Skip All* and *Cancel*. Until you answer, no other window of the program takes input. If you are working in another program, the question opens behind it and the taskbar button flashes.

## Theme

**View → Theme** offers *System default* (follows Windows), *Light* and *Dark*; the same choice is in **Settings → General**. A *Switch dark/light mode* command is available in the keyboard shortcuts table. It has no default key; `Ctrl+Shift+D` is offered as a preset. The theme covers the panels, the window title bar and the native context menus.

**Accent color** (Settings → General → Appearance) is the color of what is selected or active: the cursor row, the active panel's path bar, selected tabs and buttons, text selections. Panels a plugin shows (archives, servers, search results) are tinted with a faint wash of it, from the path bar to the banner. The light and the dark theme each have their own: *From the theme* uses the theme's own color, *Custom* one you pick, each with a preview of a selected row in that theme. It applies at once, also to the Viewer plugin's windows. Stored in `config.toml` as `accent_light` and `accent_dark` (RGB); one accent color from 0.5.8 or earlier becomes both.

## Fonts

**Settings → General → Appearance** has **Font** (the whole UI) and **Viewer font** (F3 text and hex view). Both list every installed TrueType/OpenType family, machine-wide and per-user, with a search box, and apply immediately. The defaults are Segoe UI and Consolas. Whatever you pick, the Unicode fallback chain stays behind it, so CJK, Indic, Arabic, symbols and emoji still render if the chosen font lacks them. Stored in `config.toml` as `ui_font` / `mono_font`.

## Window layout

- **File icons**: the panels show the real Windows icon for each file type, following your file associations. Programs, shortcuts and icon files (.exe, .lnk, .ico, …) show their own icon, except on network paths. Icons load in the background and are cached. Hidden items are ghosted. **View → Show Windows file icons** switches back to simple drawn icons.
- **Settings → General** is grouped into *Appearance*, *Files*, *Drives*, *Context menu* and *Command prompt*, separated by lines. Menus and Settings use the same names for the same options.
- When the window gets narrower, the panels re-lay out their columns, shrinking Ext/Size/Date/Attr if needed. Column widths you adjusted by hand reset on a window resize.

- The two panels are separated by a wider divider with a visible line.
- **View → Always show scrollbars**: solid, wider scrollbars that are always drawn. By default egui's thin bars appear only when hovered.
- **View → Animations** (also Settings → General → Layout): turned off, nothing is animated: windows, menus and tooltips appear at once instead of fading in, and moving the cursor (quick search, the cursor keys, Page Down) jumps instead of scrolling smoothly. The Viewer and Compare windows follow it.
- **View → Show function key bar** shows or hides the F1…F10 buttons at the bottom. Status messages still appear when there's something to report.

Both options are also in **Settings → General → Layout** and are remembered.

## Context menu

Right-click an item to get its context menu. Like F5/F8, the menu applies to **the whole selection** when anything is selected, otherwise to the item you clicked. Right-clicking the empty area below the list gives the folder's own menu. **Shift+F10** opens the menu for the cursor item, and **Alt+Enter** shows Properties; both are configurable.

Both menu styles are **native Windows menus**, so on Windows 11 they get the system's rounded look and follow the dark/light theme.

In **Options → Settings… → General**:

- **Context menu opens on**:
  - *Right-click* (default).
  - *Long right-click*: hold the right button until a ring appears at the pointer, then release to open the menu. A short right-click selects the item, Norton/Salamander style. The hold time is adjustable (250–1500 ms).
- **Context menu style**:
  - *Windows 11*: a compact menu with Cut, Copy, Paste into folder, Rename, Delete, Open, Open with…, Calculate size, Copy as path, Open in Terminal and Properties. **Show more options** opens the full shell menu. On the background it offers New folder, Paste, Refresh, Select all, Open in Terminal and Properties. Cut, Copy, Paste, Open with and Properties are carried out by the Windows shell, so they work with Explorer's clipboard. Explorer's own new-style menu isn't available to other apps through any public API.
  - *Classic*: the real Windows shell menu, with every verb, *Send to* and *Open with* submenus and third-party extensions. Hold Shift for extended verbs. *Rename* and *Delete* from it use RustySalamander's own dialogs.

Troubleshooting: run with the environment variable `SAL_DIAG_MENU=1` to open both menus once on start-up, close them by timer and log each step to stderr.

## Quick search

Typing in a panel starts a quick search (`/` or `\` match any characters; keys that match nothing are ignored). Every item the typed text matches has a faintly tinted row and the matched part marked, in the name and, where the match runs into it, in the extension. **Options → Quick search** (also in **Settings → General → Quick search**):

- **From first letter** (default) or **Anywhere in name**. With *Anywhere*, `port` finds `alfa report.txt`, and the matched part is highlighted wherever it is. *Switch quick search mode* is in the keyboard shortcuts table; it has no default key, and `Ctrl+Shift+Q` is offered as a preset.
- **Arrows jump between matches** (on by default, in both modes): while a search is active, ↑/↓ skip straight to the previous/next matching item, wrapping around, instead of ending the search. Esc ends the search. Ctrl+↑/↓ always jump between matches.

## Folder sizes

**Shift+Space** toggles the selection of the item under the cursor and moves down, like Space. If the item is a folder being selected, its size is calculated on a background thread. The Size column shows the running total in italics followed by `…`, then the final size; hover for the file and folder counts. A `*` after the size means it is incomplete: some subfolders couldn't be read, usually because access to them is denied (such as `System Volume Information` or another user's profile), so what they hold isn't counted and the real size may be larger. The tooltip says how many folders couldn't be read, and so does the status line after Alt+F10 (*… — 2 folders could not be read*). Running RustySalamander as administrator lets it read more of them. The footer's *Selected:* total includes the folder sizes calculated so far, plus the number of files and subdirectories inside them, e.g. *Selected: 1.2 GB in 3 files, 2 directories · 4 512 files, 87 subdirectories in total*. With the cursor on a sized folder it shows its size, bytes, files and subdirectories. Sorted by size (a click on the Size header), folders go by their calculated sizes, unsized ones as empty; the panel re-sorts when a calculation finishes.

**Alt+F10** (*Calculate selection size*, also in the Files menu) sizes every selected folder at once, or the folder under the cursor, without changing the selection. It always recalculates, even folders sized earlier. When it finishes, the status line reports the total: *Size: 1.20 GB (1 288 490 188 bytes) in 4 512 files, 87 subdirectories*. The subdirectory count covers what is inside, not the selected folders themselves.

Junctions and symbolic links aren't followed, so there are no loops and nothing is counted twice. Sizes are kept until **Ctrl+R** or until a copy, move or delete finishes. The shortcut is the **Select + calculate folder size** row in **Settings → Keyboard shortcuts**. With the `Space` preset, a plain Space still types a space while a quick search is in progress.

## Path bar and UNC paths

Each panel's path bar shows clickable breadcrumbs, e.g. `C: › Users › Docs` or `\\nas › data › projects`:

- Clicking a segment opens that directory and highlights the folder you came from.
- If the path is too long, the middle segments collapse into `…`, which is also clickable.
- Clicking the last segment or the empty space after it, or pressing **Shift+F7** (configurable; Ctrl+L is offered as a preset), turns the bar into a text box with the whole path selected. **Enter** goes there; **Esc** or clicking elsewhere cancels.

The path box accepts:

| Input | Result |
|---|---|
| `C:\Users\Docs`, `C:/Users/Docs`, `"C:\with spaces"` | that directory |
| `..`, `sub\dir`, `../other` | relative to the panel's directory |
| `C:\dir\file.txt` | opens `C:\dir` with `file.txt` highlighted |
| `D:` | switches drive, returning to the last directory there |
| `%USERPROFILE%\Desktop`, `~\Downloads` | environment variables and home directory |
| `\\server\share\dir` | network share path (UNC) |
| `\\server` | the server's shares as folders; hidden `$` shares only when hidden files are shown |

`..` (or Backspace) at the top of a share goes up to the server's share list. Network locations get the network drive icon. Contacting a slow or unreachable server blocks the window until Windows times out (see Known limitations).

## Drives

Drive buttons and the drive menu (Alt+F1 / Alt+F2) use an icon per drive type, so local and network drives can be told apart at a glance: a blue disk for local disks, a green disk on a network line for network drives, an orange stick for removable drives, a purple disc for CD/DVD and a chip for RAM disks. The menu lists local drives first, then network drives under a separator, and shows each mapped drive's share (`\\server\share`). Panels opened on a UNC path get the network icon too.

Set these under **Options → Settings…**:

- **Drive buttons**: *Drive selector* (one button for the current drive that opens the selector) or *All drives* (a button for every drive, then a list of the open server sessions, a list of the searches and a button for the comparison, each only when there is one; the comparison's button shows its sides in both panels).
- **Go to drive**: choose the modifier for the letter shortcuts (Shift by default, Alt, Ctrl+Shift, or disabled). With Shift, a letter still goes into quick search while a search is in progress or when no drive has that letter.
- **Return to the last directory on that drive**: on by default, as in Salamander. This applies to drive shortcuts, drive buttons and the Alt+F1/F2 menu.

These are stored in `config.toml` as `drive_buttons`, `drive_hotkey` and `drive_remember_dir`.

The drive selector (Alt+F1 / Alt+F2, or the drive button) opens as a dropdown right under its panel. It lists local drives, then network drives, then **Recent paths**: up to 12 paths you typed into the path bar, newest first. Choosing a path opens it. **Settings → General → Drives → Remember typed paths in the drive selector** turns this on or off; turning it off deletes the remembered paths. They're stored in `config.toml` as `typed_paths`.

## Viewers and editors

**Settings → Viewers and editors** sets what F3 views and F4 edits files with:

- **Viewer (F3)** and **Editor (F4)**: any viewer or editor a running plugin registers (the Viewer plugin by default for F3), **Notepad** (the default for F4), or **Program:** a command line of your own, with **Browse…** to pick it. The command line may contain `{file}`, which is replaced by the quoted file path; without it the path is added at the end, e.g. `code.cmd --reuse-window`. Choosing something else keeps the command for later.
- **By file suffix:** a table of suffixes with a viewer or editor of their own, overriding the defaults: `.md` with Typora, `.ini` in Notepad. **Add a suffix…** and **Edit…** open a form with the suffix (`md`, `tar.gz`; any case, with or without the dot) and the same choices plus **Default**, which keeps the default for that key. **Remove** gives the suffix back to the defaults. When suffixes overlap, the longest wins (`tar.gz` before `gz`).
- **A plugin that is off or missing:** F3/F4 then use the first running plugin registered for the file, and F4 falls back to Notepad. With no viewer at all, F3 says how to get one.

Config keys: `view_with`, `edit_with`, `suffix_rules` (and `viewer`/`editor`, the last command lines typed). A custom viewer or editor from an older config stays in use.

## External programs

**Settings → General → External programs** has the **Command prompt**, the terminal choice described below.

## Terminal ("Open command prompt here")

Set this under **Options → Settings…**:

- **Default terminal** (default): uses the terminal application Windows is set to use. That's Windows Terminal (`wt.exe -d <dir>`, with its default profile) unless you chose the classic console host in Windows settings, in which case it starts `cmd.exe` in the console host.
- **Command Prompt (cmd)**
- **PowerShell**: PowerShell 7 (`pwsh.exe`) if installed, otherwise Windows PowerShell.
- **Custom command**: a program and its arguments, for example `alacritty.exe --working-directory "{dir}"`. `{dir}` is replaced by the panel's directory, which is also the working directory.

**View** has separate **Show hidden files** (Ctrl+H) and **Show system files** switches, both on by default; they're also in **Settings → General → Files**. As in Explorer, an item that is both hidden and system appears only when both are on. Hidden `$` shares on a server follow the hidden-files switch.

## Known limitations

- **Arabic shaping:** Arabic letters appear in the correct order but without contextual joining forms (egui doesn't shape complex scripts). Indic conjuncts are affected the same way. File operations aren't affected.
- **Missing features:** no encrypted archives, FTP, Find dialog, directory compare, panel view modes (brief, thumbnails) or drag and drop yet. See the roadmap in `docs/ANALYSIS.md`.
- **Recycle Bin:** deleting to the Recycle Bin uses `SHFileOperationW`, which may reject paths longer than `MAX_PATH`. Permanent delete has no such limit.
- **Network folders:** a panel that starts on a network folder reads it in the background (the window is usable at once), and free space and change checks never wait for the server. Opening a network folder later still waits for its listing, and an unreachable `\\server` blocks the window until Windows gives up (up to about 30 s).
- **Untranslated messages:** error messages that come from Windows or from the core library (e.g. "Access is denied") stay in English or in the Windows language.

## License

GPL-2.0-or-later, the same as Open Salamander; see [LICENSE](LICENSE). The 7-Zip plugin's library is 7-Zip (GNU LGPL with the unRAR restriction); see [THIRD-PARTY-NOTICES.md](THIRD-PARTY-NOTICES.md).
