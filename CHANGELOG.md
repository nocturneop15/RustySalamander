# Changelog

## 0.5.3-alpha (2026-10-31)

- **Plugin panel tint:** Settings → General → Appearance chooses the color of panels a plugin shows (archives, servers, search results) and of their banner: from the theme, as before, or a custom color picked with a color picker, with a preview. Applies at once.

## 0.5.2-alpha (2026-10-31)

- **Plugins named as such:** in Settings → Viewers and editors, a viewer or editor that a plugin provides reads "Plugin Viewer: Text and hex viewer", so it is clear it comes from a plugin; one whose plugin is off or missing reads "Plugin …: off or missing".

## 0.5.1-alpha (2026-10-31)

- **Settings → Viewers and editors (new tab):** the default viewer (F3) and editor (F4), moved out of General → External programs. Each can be any viewer or editor a running plugin registers, Notepad, or a program of your own (with Browse…). A custom viewer or editor from an older config stays in use.
- **A viewer or editor per file suffix:** a table of suffixes with their own viewer or editor (or Default for either), overriding the defaults; Add a suffix…, Edit… and Remove. The longest matching suffix wins (`tar.gz` before `gz`).
- **The Viewer plugin 0.5.1-alpha** is bundled (no changes; it was released on its own to try plugin updates).

## 0.5.0-alpha (2026-10-31)

### Plugins

- **The viewer is a plugin:** the F3 text and hex viewer moved out of the program into the Viewer plugin (`plugins\viewer\viewer.spl`), with the same encodings, hex view, word wrap and limits. Its windows are its own and follow the program's theme, fonts and scroll bars; several files open in several windows. Turned off, F3 says how to get a viewer back.
- **Any plugin can view or edit files:** plugins register as the viewer (F3) or editor (F4) of the files their masks match; F3 and F4 use the first one, unless Settings names a program of your own. Without an editor plugin F4 opens Notepad, as before. Settings → Plugins shows what each plugin views or edits.
- **Plugins have their own versions:** each plugin has a version of its own and its own changelog, shown in Settings → Plugins with the interface it was built for. The 7-Zip, SCP, search and Viewer plugins start at 0.5.0-alpha.
- **Plugin interface version 5:** the descriptor tells the plugin's version and where it is published; `openers` and `open_file` are new. From now on the interface only grows and the program keeps running plugins built for older versions it supports; plugins built for 0.4 must be rebuilt once more.

### Updates

- **Plugins are updated too:** the update check also looks for newer releases of each plugin (tagged `<plugin>-v<version>`, with a zip for the plugin interface this program has). A plugin's update is downloaded into `plugins\.updates` and installed at the next start (**Restart now**), keeping what the plugin saved in its folder.
- **Show changelog:** each update in the update window can show what changed between your version and the new one before you install it, read from the changelog at the new release.
- **One window for all:** the update window lists the program and the plugins with updates, each with its versions, its release page, its changelog and its own Update now.

## 0.4.1-alpha (2026-10-31)

- **New search replaces the old one:** New search… in the results window (or Find files again) no longer adds both views of the previous search to Searches in the drive selector. The previous results go as if their window was closed; a view stays only if a panel shows it.

## 0.4.0-alpha (2026-10-30)

### Search plugin (new)

- **Find files** (Plugins → Find files…, Alt+F7): by name, with wildcard masks (`*.txt;*.md|old*`) or a regular expression; in a folder, with or without subfolders; folders too if wanted; hidden and system items optionally left out. Symbolic links and junctions are never entered, so a search can't loop.
- **Text inside files:** plain text, found in UTF-8, ANSI and UTF-16 files (any case unless Match case is on), or a regular expression. Large files are read in pieces, and a search stops at once when asked.
- **Results window:** the results appear while the search runs, with the folder being searched and the counts; sort by name, folder, size or date; Open (Enter, double-click) opens a found file with its program or a found folder in the panel, F3 views and F4 edits; Go to file shows a result in its folder in the active panel; Stop; New search… with the last values; Refresh searches again with the same settings into the same results.
- **Results in a panel, two ways:** Show filtered folders opens them as `search://1`, the searched folder as a filter (its folders, holding only what was found); Show all in one list opens `search://1-list`, everything found, files and folders, in one list (repeated names numbered; a found folder opens to its real contents). Enter, F3 and F4 open the real files; F5 copies the results out, F8 moves them to the Recycle Bin, F2 renames them.
- **Named in the path bar:** a panel showing search results says what they are, `🔍 *.txt "hello" · Documents (filtered folders) › docs`, instead of `search://1/docs`, and so do its drive button and banner; the address is in the tooltips and is what a typed path uses.
- **Searches, not connections:** the drive selector and the Plugins menu list search results under **Searches**, by a name saying what was looked for and where (`*.txt "hello" · Documents (filtered folders)`), with **Refresh** (the same search again) and **Close** instead of Disconnect. A new search keeps the earlier ones; a session keeps the last 10.

### Plugins

- **Plugin interface version 4:** results windows (`Reply::Results`, with `activity`, `stop` and `real_path`) and folder fields in forms (`FieldKind::Folder`, with a folder picker). Plugins built for 0.3 must be rebuilt. `docs/PLUGINS.md` has a new section on results windows.
- **Drive selector shows every row:** it is as tall as its drives, connections, searches and recent paths, and moves up over the window when the room below the path bar isnt enough; it scrolls only when the list is taller than the whole window, also with display scaling and after rows were added (it used to keep the height it had when first shown, at most 400 points).
- **Update window:** the download progress bar has square corners, like the other progress bars.
- **Turning a plugin off** ends its connections and searches at once: they leave the drive selector and the Plugins menu.
- **Real files behind plugin items:** where a plugin says which file on disk an item is, Enter, F3 and F4 open that file itself instead of a temporary copy.

## 0.3.1-alpha (2026-10-30)

- **No changes:** a release to try the update from 0.3.0 (Help → Check for updates… → Update now).

## 0.3.0-alpha (2026-10-30)

### Updates (new)

- **Looks for new versions:** at start-up the program asks GitHub whether a newer release exists (alpha releases count). If one does, a window shows the new version and a link to its release page. **Help → Check for updates…** and **Settings → General → Updates → Check now** check at any time; the start-up check can be turned off there (`check_updates` in `config.toml`).
- **Update now:** downloads the new version, unpacks it into a folder named by the version next to this one (for example `…\RustySalamander\0.2.1-alpha` → `…\RustySalamander\0.3.0-alpha`) and copies the settings there: `config.toml` and what plugins saved in their folders (such as the SCP plugin's `known_hosts` and last connection). **Start the new version** then opens it and closes this one. The running version is never changed, a folder that already exists is never overwritten, and a failed or damaged download leaves nothing behind.
- **How it works:** downloads go through Windows' WinHTTP, so the system's proxy settings and certificates apply; each file in the zip is checked against its CRC before it is written.

## 0.2.1-alpha (2026-10-30)

### Fixes

- **Slow start and sluggish window with a network folder in a panel:** a panel that starts on a network drive (for example `X:\` mapped to a NAS) is read in the background, so the window is usable at once; the panel says "Loading…" until the listing arrives. Before, the program waited for the whole folder, then read it a second time.
- **No more waiting for the server on every redraw:** free space was asked for on every frame (over 100 ms each on a network drive) and the folder's date every 1.5 s; both are now read in the background, and network folders are checked every 5 s.
- **Faster folder listing:** folders are read in large batches (as `cmd`'s `dir` does), about three times faster on a network share (2,245 items: 7.3 s → 2–3 s).

### For developers

- **Timing log:** set `RUSTYSALAMANDER_LOG` (a file path, or `1` for `%TEMP%\rustysalamander.log`) to log each start-up step and every slow frame with its parts.

## 0.2.0-alpha (2026-10-30)

Plugins can now add commands, ask for input and open connections, and the first plugin that does is **SCP**: files on SSH servers, in a panel.

### SCP plugin (new)

- **Servers in a panel:** **Plugins → Connect to server…** asks for the server, port, user name and a password or a private key (with its passphrase). The server's files open in the active panel as `scp://user@host/home/user`; copying in and out, deleting, renaming, new folders, viewing and editing work as in any folder, with progress, pausing and cancelling.
- **Sign-in:** password (servers that use keyboard-interactive prompts get it too) or a private key in OpenSSH or PEM format.
- **Server keys:** keys are checked against `%USERPROFILE%\.ssh\known_hosts` and the plugin's own `known_hosts`. A new server shows its key fingerprint before you trust it; a changed key is refused with a warning.
- **Several sessions at once:** each is listed in the Plugins menu and in the drive selector.
- **Remembered:** the last server, port, user and key file, never a password.
- **How it works:** SSH in pure Rust (`russh`, with `ring`); transfers use the SCP protocol, listings and file management `ls`, `rm`, `mv` and `mkdir` on the server. No SFTP yet.

### Plugins

- **Plugins menu:** the commands of the plugins that are on, under each plugin's name; *Disconnect from …* for every open session, one per row (the menu widens to fit); *Plugin settings…*.
- **Forms drawn by the program:** plugins describe dialogs (text, passwords, a file with Browse…, options, check boxes, notes) and the program draws them like its own.
- **Connections:** plugins can open servers and other places in a panel, shown and typed as `scheme://…` paths. Closing one, turning its plugin off or quitting returns the panel to the folder it showed before.
- **Plugin panels:** panels a plugin shows, archives included, have a tinted background and a banner with the plugin's name below the list.
- **Plugin interface version 3:** `list` takes the folder to list, so plugins can list one folder at a time; `commands`, `run` and `close` are new. Plugins built for 0.1 must be rebuilt. The guide (`docs/PLUGINS.md`) has a new section on commands, forms and connections.

### Drive selector

- **Connections:** open sessions are listed under *Connections*; choosing one opens it in that panel at its start folder. Each row has a **Disconnect** button (or press Delete). Long names are shortened with "…", and the selector widens to fit them.
- **Fits the window:** a long list in a low window no longer covers the menu bar; the selector stays below the path bar and scrolls, keeping the chosen row in view.
- **Simpler:** no Cancel button and no empty space below the list (Esc or a click outside closes it).

### Fixes

- **Copying into a connection:** the copy dialog keeps `scp://` targets, and a typed `scp://…` path works in the path bar.

## 0.1.0-alpha (2026-10-30)

The first public release: an alpha of a Rust rewrite of Open Salamander with full Unicode support. Everyday file management works, but many of Salamander's features are still missing (see *Not yet* below), and the plugin interface may change between releases.

### Unicode everywhere

- **Every file name works.** Names outside the system code page (Chinese, emoji, …) and names with unpaired UTF-16 surrogates can be shown, opened, copied, renamed and deleted.
- **Long paths:** there is no 260-character `MAX_PATH` limit.
- **Matching:** masks, quick search and sorting are Unicode-aware (case folding, numeric-aware "logical" sort as in Salamander).
- **Right-to-left text:** Hebrew and Arabic names are displayed in the right order.
- **Fonts:** a fallback chain covers CJK, Korean, Indic, Thai, symbols and emoji, whatever font is chosen.
- **Forbidden characters:** names from Macs, NAS shares and WSL that contain characters Windows forbids are shown as their original characters.

### Panels and navigation

- **Panels:** two panels with sorting by name, extension, date and size, directories first, `..` on top, and hidden and system files switchable.
- **Keyboard:** Salamander's keyboard model, with every command shortcut configurable in Settings. The editor shows conflicts, offers presets and can reset to defaults. F1 help is generated from the current bindings.
- **Quick search:** typing in a panel searches from the first letter or anywhere in the name. Arrows jump between matches, and the matched part is highlighted.
- **Selection:** by mask (`+`, `-`, `*`), Insert/Space, select all and invert.
- **Path bar:** clickable breadcrumbs that turn into an editable path. It accepts relative paths, `%VARIABLES%`, `~`, `"quoted"` paths, a file to jump to, and UNC paths including a server's share list.
- **Drives:** buttons or a drive selector with an icon per drive type, local and network drives listed separately, recent typed paths, and the option to return to the last folder on each drive.
- **History and hot paths:** history (Alt+←/→) and hot paths (Ctrl+0..9).
- **Folder sizes:** calculated in the background (Shift+Space, Alt+F10), with file and folder counts and a running total in the footer.
- **Icons:** the real Windows file icons, loaded in the background.
- **Auto-refresh:** a panel reloads when its folder changes.

### File operations

- **Operations:** copy, move, delete (to the Recycle Bin or permanently), create folders including nested paths (`a/b/c`), and rename. A copy target can be a folder, a new name or a mask such as `*.bak`.
- **Progress:** bytes and files, speed and time left. Each operation can be paused, resumed and cancelled.
- **Queue:** *Run first*, *Queue last* and *Start now*, reordering, *Pause all* and *Cancel all*, and running in the background with progress in the status bar.
- **Conflicts and errors:** a conflict window compares both items side by side. Errors offer *Retry*, *Skip*, *Skip all* and *Cancel*.

### Archives (the 7-Zip plugin)

- **Browsing:** archives open as folders. It reads zip, 7z, rar, tar, gz, bz2, xz, zst, lzma, cab, arj, lzh, iso, wim, cpio, rpm, deb and split archives.
- **Changing archives:** 7z, zip, tar and wim archives can be copied into, deleted from and renamed inside, and new folders created in them. Every change is written safely: a failed or cancelled change leaves the archive as it was.
- **Copying:** copy and move out of, into and between archives, with conflict questions and progress.
- **Opening files:** view, edit or open a file inside an archive. It is extracted to a temporary folder, which is cleaned up on exit.
- **Library:** uses 7-Zip 26.05 (`7z.dll`, built from source) inside the program; nothing installed on the computer is needed.

### Plugins

- **Separate DLLs:** plugins are DLLs (`plugins\<name>\<name>.spl`), found when the program starts and switched on or off in Settings → Plugins. Settings says why a plugin can't start (a missing or damaged library, another interface version).
- **Interface:** a documented plugin interface (`sal-plugin-api`) for *folder plugins*. The C ABI is generated from Rust traits, with UTF-8 text and UTF-16 paths. Each plugin has its own language files.
- **For developers:** a guide for plugin authors (`docs/PLUGINS.md`) and an example plugin (`plugins/example-filelist`).

### Viewer, editor, terminal

- **Viewer (F3):** built-in text and hex viewer in its own window. It detects UTF-8, UTF-16 and ANSI, and can switch to Windows-125x, ISO-8859-2, KOI8-R, CP866, Shift-JIS, EUC-JP, GBK, Big5 and EUC-KR.
- **External programs:** a custom viewer or editor (F4, Notepad by default) can be set, with `{file}` placeholders.
- **Command prompt here:** opens the Windows default terminal, cmd, PowerShell or a custom command.

### Windows integration

- **Context menus:** a compact Windows 11 style menu, or the classic full shell menu with *Send to*, *Open with* and third-party extensions. The menu can open on right-click or long right-click.
- **Properties:** Alt+Enter, and copying full paths to the clipboard.
- **Themes:** light, dark or following Windows, including the title bar and native menus.

### Interface

- **Dialogs:** in their own windows, which can be moved to another monitor, centred, and modal where needed.
- **Fonts:** a choice of UI and viewer fonts.
- **Layout:** always-visible scrollbars, a function-key bar that can be hidden, and columns that adapt to the window width.
- **Languages:** English and Czech, switchable at once. Translations are TOML files, partial translations work, and plural forms are supported.
- **Portable mode:** settings live in `config.toml` next to the program, with migration from `%APPDATA%` and a fallback when the program folder isn't writable.

### Not yet

- **Archives:** encrypted (password-protected) archives.
- **Missing features:** FTP and other file-system plugins, the Find dialog, directory compare, panel view modes (brief, thumbnails) and drag and drop.
- **Text shaping:** Arabic joining and Indic conjuncts aren't shaped (an egui limitation).
- **Recycle Bin:** deleting to it may reject paths over 260 characters (permanent delete works).
- **Network:** an unreachable network share blocks the window until Windows times out.
