# Changelog

## 0.9.5-beta (2026-11-11)

- **Plugin windows opened from a command look like the program:** the Compare window took the system's theme, fonts and accent color instead of the ones chosen in the program. For plugin authors: a plugin command's values now carry the program's look (`theme`, fonts, accent colors, animations), as viewers' options do.

## 0.9.4-beta (2026-11-07)

- **Quick over Remote Desktop (#1):** in a Remote Desktop session the program draws its windows on the processor and puts on screen only what changed since the last frame, with Windows' GDI, instead of drawing the whole window with Direct3D each frame. A cursor moving a row is drawn and sent as a strip or two of the window, and a frame that changes nothing costs nothing, so a held arrow key keeps up also in large windows. It looks the same, and the Viewer and Compare windows and the dialogs draw the same way. Animations are off in this mode (fading, smooth scrolling, the blinking text cursor), whatever View → Animations says; the setting is kept for the usual way. The program chooses this by itself in a Remote Desktop session and where Windows has no Direct3D 12; the environment variable `SAL_RENDER` (`gdi`, `wgpu`) chooses otherwise. The debug log says which way the program draws and why, and for each frame how much was drawn and sent and how long it took.
- **Starts on Windows 8.1 and Windows Server 2012 R2:** it stopped at once with *The procedure entry point GetDpiForWindow could not be located*, a function of Windows 10. There the program draws on the processor, as those versions have no Direct3D 12.
- **Drawing through Direct3D 12 instead of OpenGL** (wgpu), in the main window and in the Viewer and Compare windows: the program starts also where Windows has no OpenGL 2.0 driver, such as a Hyper-V virtual machine; there it draws with Windows' software renderer (WARP). `WGPU_BACKEND` (`vulkan`, `dx12`) chooses another way. Without vsync in a Remote Desktop session, when Direct3D is chosen there (`SAL_VSYNC=on|off` chooses). (#1)
- **The program draws the plugins' windows (interface 9):** the Viewer and Compare windows are shown and drawn by the program instead of each plugin carrying graphics of its own. So the graphics library is in the program once, not three times: `viewer.spl` is 7.1 MB instead of 15.9 MB and `compare.spl` 3.7 MB instead of 12.5 MB, and the plugins' windows draw wherever the program's window does, also on the processor.
- **For plugin authors (interface 9):** `HostApi::open_window` and `HostApi::repaint`, with `abi::WindowFns`, let a plugin's window be shown by the program; the `sal-window` crate does it all behind the same `Windows` and `Content` as before (`Content` must now be `Send`). Plugins for interface 5 to 8 still load, and keep their own windows.

## 0.9.3-beta (2026-11-06)

- **Debug log:** `rustysalamander.exe -debug` writes everything the program does to a log next to it: the computer (Windows, Remote Desktop, graphics adapters, OpenGL), each user action numbered and marked with what the program did and what came of it, the messages of the graphics libraries, errors and panics. Passwords are left out. Meant for bug reports, such as a start that fails without a word.
- **Quick search ignores diacritics:** `vladce` finds `vládce` and `vládče`, `grusse` finds `Grüße`, `o` finds `ø`, in Czech, German, Nordic, Polish, French and other names. *Exact letters* (Options → Quick search, or Settings → General → Quick search) brings back matching only the same letters.

## 0.9.2-beta (2026-11-05)

- **FTP, FTPS and SFTP servers:** the new FTP / SFTP plugin (Plugins → Connect to FTP or SFTP server…) opens them in a panel, with everything that works on SCP servers. FTP finds its way to open data connections by itself (passive, or active when passive doesn't get through), or uses the one you choose.
- **New official plugins are offered:** the update check reads the list of official plugins from the repository and offers the ones you don't have, with what they do: **Install** downloads one, and it is added at the next start, without a new program. **Don't offer** stops the offer at start; Help → Check for updates still shows it.
- **Plugin forms show only valid choices:** a field can depend on one of several values of another, and a choice that is greyed out counts as its first option, so what depends on it follows. For plugin authors: see "Commands, forms and connections" in `docs/PLUGINS.md`.

## 0.9.1-beta (2026-11-05)

- **Enter is visible in every question:** the button that Enter presses is lightly tinted with the accent color, in every dialog that asks something (Delete, Copy and Move, new folder, rename, selection masks, plugin forms, messages). In the window about an existing file Enter **skips** it, never overwrites; after an error Enter **retries**.
- **Quick search stays while you move:** with *Arrows jump between matches* turned off (Settings → General → Panels), moving the cursor with the arrows, PgUp, PgDn, Home or End keeps the quick search and its highlight; Ctrl+arrows still jump between matches and typing goes on from the cursor.
- **Plugin windows open on the program's monitor:** the Viewer, Compare and other plugin windows appear over RustySalamander's window, on the same monitor, each next one a little lower and to the right. Viewer 0.6.3-alpha and Compare 0.1.4-alpha are bundled.
- **New description:** RustySalamander is a modern file manager for Windows, inspired by the long-proven efficiency and functionality of Altap Salamander (README, About, manual).
- **For plugin authors:** the program sends `owner` (its window handle) to plugin windows and commands; `sal_ui::layout::buttons` takes the index of the default button. See `docs/PLUGINS.md`.

## 0.9.0-beta (2026-11-04)

- **Real files from plugin panels:** in search results and compared folders, **Ctrl+C** and **Ctrl+X** put the files on disk on the clipboard, **dragging out of the window** gives them to Explorer or any program, and the **context menu** (right-click, Shift+F10) and **Properties** are those of the real files, even when they come from many folders. **Copy full paths** copies their real paths instead of the plugin's.
- **Dragging out of archives and servers:** items of a zip, 7z or any other archive, and files on an SCP server, can be dragged out of the window to Explorer, the desktop or any program that takes files. They appear to that program as files on disk (through the Windows feature *Windows Projected File System*), and only what it copies is read, as it copies it: nothing is extracted when you start dragging. A folder comes with everything inside it, and the items are always copied. When the feature is turned off, the program says how to turn it on and suggests F5 or the other panel.
- **Plugin manual and user manual:** new sections on dragging out of the window, Windows Projected File System, and testing `stream`.
- **For plugin authors (interface 8):** `FolderPlugin::stream` sends a plugin's files to the program without a temporary copy, for dragging them out of the window. The program does the Windows side for every plugin; plugins without `stream` still work, through a temporary copy. The 7-Zip and SCP plugins stream; see "Dragging out: files on disk" in `docs/PLUGINS.md`.
- **Dragging out starts at once:** items dragged past the window's edge go to Windows as soon as the pointer leaves, showing their icon, instead of a "no" cursor for a while. Over a panel the pointer stays an arrow (it was the four-way move arrows).
- **Search results take no dropped files:** dragging files onto search results (from a panel or from another program) is refused, as it is for archives that can't be changed. Compared folders still take them.

## 0.8.5-beta (2026-11-03)

- **Fixed:** **Overwrite All** in the window about an existing file now holds for the whole operation. When folders were merged into folders already at the target (moving, or copying out of an archive), the window came back for every next folder.

## 0.8.4-beta (2026-11-03)

- **Drag and drop:** drag files, folders or the selection from a panel onto the other panel or onto a folder's row. On the same drive they are moved, onto another drive copied; **Shift** moves and **Ctrl** copies. The target is outlined and a label at the pointer says what will happen; the drop opens the Copy or Move dialog with the target filled in (Settings → General → Files → *Drag and drop* can start it at once). Esc cancels.
- **With other programs:** dragged out of the window, the items go to Explorer, the desktop or any program that takes files, which copies or moves them by the same rule. Files dragged in from Explorer can be dropped on a panel or a folder's row the same way.

## 0.8.3-beta (2026-11-03)

- **From the other panel:** the drive selector (Alt+F1, Alt+F2) ends with **From right panel** in the left panel and **From left panel** in the right one: the panel goes straight to the folder the other one shows. With *All drives* in Settings, the same is a button (⏪ / ⏩) at the end of the drive buttons.
- **The sort arrow points the way the list is sorted:** up for ascending (A to Z, smallest, oldest first), down for descending, in every column.
- **One layout for every window:** the dialogs, Settings, the operations and result windows, plugin forms and the plugins' windows (Viewer, Compare) share the same margins, gaps, button padding, control heights, forms and tables. Every dialog ends with its buttons centred, Settings and Keyboard help too.
- **Plugin forms** (Find files, Connect to server) are wider and laid out in two clean columns: labels on the left, tick boxes with their text beside them, fields that end at the same edge, notes across the whole width.
- **Find files** finds folders too by default. Search 0.5.4-alpha, Viewer 0.6.2-alpha and Compare 0.1.3-alpha are bundled.

## 0.8.2-beta (2026-11-03)

- **Find in the viewer:** Ctrl+F in the viewer (F3) opens a find bar. Every match is marked as you type, *3 of 12* says where you are, Enter or F3 goes to the next match and Shift+F3 to the previous one. **Match case** makes upper and lower case count (otherwise case is ignored in every language); **Wildcards** makes `*` stand for any characters and `?` for one. Esc closes the bar, a second Esc the window. The Viewer 0.6.0-alpha is bundled.

## 0.8.1-beta (2026-11-03)

- **Safe copy and safe move:** the Copy and Move dialog checks each copy right after it is written: *Compare sizes*, or *SHA-256 checksums* (each copy is read back and compared with the original). With checksums, every file's checksum is also written to `RustySalamander <date> <time>.sha256` in the target folder and in the source folder, in the format `sha256sum -c` checks. A copy that fails the check is removed and you are asked to Retry, Skip or Cancel; a safe move deletes an original only after its copy passed.
- **Run in background** and **Show the result when finished**, two ticks in the Copy and Move dialog: start without the progress window (the status bar shows the progress), and get the *Operation result* window at the end. A safe copy always shows the result.
- **The Operation result window** tells how each operation went: what was copied or moved, how long it took and how fast, how the copies were checked, and every file that failed or was skipped, with the reason. It is modal. **Save protocol…** saves all of that to a text file, with every file's SHA-256 checksum after a check with checksums; **Close and remove checksum files** deletes the `.sha256` files when you only wanted the check.
- **Defaults for Copy and Move in Settings:** Settings → General → Files → *Copying (F5)* and *Moving (F6)* set what the dialogs offer when they open. What you change in a dialog is for that operation only and isn't remembered.
- **A question brings a background operation forward:** when a copy running in the background has to ask (a file exists already, an error), its progress window appears with the question and hides again once you have answered. Before, the window stayed hidden and the status bar could not bring it back while the question was open.
- **Click anywhere in a panel to make it active**, also on the empty space below the last item.
- **The user manual** (`USER-MANUAL.md`) is in the package.

## 0.8.0-beta (2026-11-02)

The first beta.

- **Ctrl+C, Ctrl+X and Ctrl+V copy, cut and paste files:** in a panel they put the selected items (or the one under the cursor) on the Windows clipboard, or paste what is there into the panel's folder, as Explorer and the context menu do. They did nothing: the window library turned these keys into its text clipboard events and dropped them, as it did Shift+Delete (Delete permanently) and Ctrl+Shift+C (Copy full paths), which work now too. The three are in Settings → Keyboard shortcuts (*Copy to clipboard*, *Cut to clipboard*, *Paste from clipboard*), and the context menu shows their keys.
- **A context menu with everything, Windows 11 style:** Settings → General → Context menu → Style → *Windows 11, full* shows the compact menu's commands and below them every item of the full shell menu (third-party extensions, *Send to*, …), without *Show more options*. The shell extensions now load in the background when the program starts, so the first full or classic menu opens faster.
- **The function key bar is yours to set:** in Settings → Keyboard shortcuts, the tick box before a command gives it a button on the bar, with its first shortcut and its name (*Alt+F7 Find files*), in key order. The default bar is F1 Help … F10 Exit as before; *Reset function key bar* brings it back.
- **Columns don't leave empty space:** after a column was resized, the room it gave up was left empty at the right of the panel. Name now always takes the rest; drag the edge to the left of Ext, Size or Date in the header to resize that column. Attr is just as wide as its letters and Ext starts three letters wide. Your widths stay when the window is resized, unless they no longer fit.
- **Settings keeps the main window waiting** while it is open, as the other dialogs do.
- **Dialogs appear at their size:** Settings grew on screen for a moment when it opened (it looked like an animation with animations off); every dialog is now sized before it is shown. Animations off also stops Windows animating the main window (minimizing, restoring, closing) and every dialog, including one opening behind another program.
- **The search window no longer flickers** while a search runs (with animations off it was hidden and shown again many times a second).
- **All drives: Disconnect, Refresh and Close in the lists:** the server sessions and searches listed beside the drive buttons have the drive selector's buttons.
- **Settings:** *Layout* is called *Misc*; *Accent color* is right below *Theme*, *Font size* right below *Font*.

## 0.7.4-alpha (2026-11-02)

- **Animations off covers the windows too:** with View → Animations off, dialogs and the Viewer and Compare windows no longer fade or zoom in and out. Windows plays that animation for every window it shows, so each such window is now made hidden, told not to animate, and then shown. (A dialog opening while you work in another program still animates: showing it this way would take the focus.) The Viewer 0.5.9-alpha and Compare 0.1.2-alpha are bundled.
- **The status bar tells what operations run:** its left side says *No operations running* when nothing runs or waits, and otherwise how many copy, move and delete (*Copying 2 · Deleting 1*), with the progress, the time left and how many wait. What an operation did (*Moving to Recycle Bin: 1 file*) shows there for 5 seconds after it ends; it was left in the status line until something else replaced it.
- **The status bar is always there,** also with the function key bar turned off (View → Function key bar), and the function keys have as much room below them as at the sides (they touched the window's edge).

## 0.7.3-alpha (2026-11-01)

- **Alphabetical order of your language:** names sort as in Explorer, by Windows in the language set for your account: `Čeština` right after `Cena` (not after `Zeta`), `Ábel` among the A's, in Czech `Chata` after `Hora`; case doesn't matter and numbers go by value (`a2` before `a10`). Symbols come before numbers, numbers before letters. In the panels, inside archives and connections, and in the search results window.
- **Masks and quick search can't freeze the program:** matching a mask with many `*` (or a quick search with many `/`) tried every way of splitting the name and could take minutes on a long name; it now takes at most name length × mask length steps.
- **Quick search tints every match:** the typed text was marked only in the item under the cursor; now every item it matches shows it, also where the match runs into the extension (Ext column), and the rows it matches have a light wash of the accent color (the letters a stronger one), so they stand out from the rest.
- **Find files reads masks as Select does:** the Search plugin 0.5.3-alpha (bundled) uses the program's own masks: `;;` for a `;` in a name, and a message saying where a mask can't be read.

## 0.7.2-alpha (2026-11-01)

- **Sorting by a column header works anywhere in the header:** a click sorted only when it hit the column's title; most of the Ext, Size and Date headers did nothing. The whole header cell sorts now (the pointer turns into a hand), in the panels and in the search results window.
- **Sorted by size, folders go by their sizes:** once a folder's size is calculated (Shift+Space, Alt+F10), or inside an archive, where it is known, sorting by size puts it in its place instead of counting it as empty; a panel sorted by size re-sorts when a calculation finishes (not while it counts, so nothing jumps).

## 0.7.1-alpha (2026-11-01)

- **Calmer copy and move progress:** the speed and the time left are worked out every 2 seconds instead of with every bit of progress, so they no longer jump about; when moving, the progress bars and counts move every 2 seconds too (the name of the file being moved stays current).
- **All drives shows the connections too:** with Settings → Drive buttons → All drives, the drive buttons are followed by a list of the open server sessions (SCP), a list of the searches and a button for the comparison, each only when there is one. Until now that mode had no way to reach them except the drive selector; a panel in one of them shows it marked, and the comparison's button shows its sides in both panels.
- **The version in the title bar:** the main window's title says which version runs ("RustySalamander 0.7.1-alpha").
- **Animations can be turned off:** View → Animations (also Settings → General → Layout). Off, windows, menus and tooltips appear at once instead of fading in, and the list jumps to the cursor (quick search, cursor keys) instead of scrolling smoothly. The plugin windows follow it: the Viewer 0.5.8-alpha and Compare 0.1.1-alpha are bundled.
- **Drive buttons stay below the menu:** a drive button no longer grows by a pixel when hovered or pressed, which pushed its edge under the main menu.

## 0.7.0-alpha (2026-11-01)

- **The Compare plugin 0.1.0-alpha** is new and bundled: it compares two files side by side in a window of its own, or two folders in the two panels, with the differences colored. Plugins → Compare takes two selected items, else the selected item of each panel, else the items under the cursors, else the two panels' folders (see README, Comparing files and folders).
- **Folders compared in both panels:** a plugin can open two connections side by side, one in each panel. Going into a folder or up in one takes the other along, each side stays in its own panel (the drive selector offers a panel only its own side), Refresh refreshes both, and closing one closes both. Their rows are colored by what the plugin says about each item (different, newer, older, only here; what only the other side has is grey and shows no size), and the banner says what the colors mean. The drive selector and the Plugins menu list them under **Comparisons**, with a comparison icon. There is one comparison at a time: a new one closes the one before.
- **Plugin interface 7:** plugins can mark items (`marks`), open two connections side by side (`Reply::Panels`), and are told more about the panels when a command runs (which side is active, the item under each cursor, the selected items of both panels). Plugins built for interfaces 5 and 6 keep working.
- **Copying into a plugin's folder** doesn't ask about overwriting an item the plugin shows only as a placeholder.
- **The plugins** are built for interface 7: 7-Zip, SCP and Search 0.5.2-alpha, Viewer 0.5.7-alpha (its window code is now shared with the Compare plugin, in `crates/sal-window`).

## 0.6.0-alpha (2026-11-01)

- **Update all:** when more than one update can be installed, the update window has an **Update all** button that starts them all.
- **Updates made for other program versions are greyed:** a plugin release says which RustySalamander versions it works with (in its notes); an update this version is outside of is shown greyed with the reason ("needs RustySalamander 0.7.0 or later"), and Update all leaves it out.
- **Plugins say which program versions they work with:** plugin interface version 6 lets a plugin name the oldest and the newest RustySalamander it works with (`PROGRAM_MIN`, `PROGRAM_MAX`), so plugins can be phased out. A program outside the range doesn't load it, and Settings → Plugins says why. Plugins built for interface 5 keep working, with no limits.
- **The plugins** are built for interface 6: 7-Zip, SCP and Search 0.5.1-alpha, Viewer 0.5.6-alpha.

## 0.5.9-alpha (2026-11-01)

- **The exe has an icon:** `rustysalamander.exe` carries the program's icon (two lists side by side on a rust-colored square, from 16 to 256 pixels), so Explorer, shortcuts and the Start menu show it, and version information, so Windows names it "RustySalamander" (file properties, Task Manager).
- **Settings has a toothed wheel** as its window icon.
- **An accent color for each theme:** Settings → General → Appearance → Accent color has its own choice for the light and the dark theme, each the theme's own or a custom color, with a preview of a selected row in that theme. An accent color chosen before becomes both.
- **The Viewer plugin 0.5.5-alpha** is bundled: it follows both accent colors.
- **Faster panels with many folder sizes:** drawing a panel takes only the sizes of its own folders, not a copy of every size calculated so far, on every frame.
- **Tidier code:** every function has a comment saying what it does; code nothing used is gone; clippy and the documentation build are free of warnings.

## 0.5.8-alpha (2026-10-31)

- **Window icons:** the windows have icons of their own, drawn from Material Design Icons on a rust-colored square: two lists side by side for the program's main window and dialogs, a document with an eye for the viewer. Until now they showed egui's default "e" logo, which looked like Internet Explorer's.
- **The Viewer plugin 0.5.4-alpha** is bundled, with its new window icon.

## 0.5.7-alpha (2026-10-31)

- **Smaller program:** the built-in Material Design Icons font holds only the 32 icons the program uses (5 KB instead of 1.3 MB), in the program and in the Viewer plugin. `tools/subset-mdi.py` makes it again when an icon is added, and a test says when one is missing.
- **The Viewer plugin 0.5.3-alpha** is bundled. **Syntax highlighting:** text is colored by its format: JSON, XML, HTML, PowerShell, Bash, Batch, INI, TOML, YAML, Markdown, CSS, JavaScript, Python, C/C++, C#, Rust, SQL, Dockerfile and about 200 more. The format is recognised from the file name (`.json`, `Makefile`, `.bashrc`) or the first line (`#!/bin/bash`, `<?xml`), and the **Syntax** list in the toolbar picks another one, or Plain text. The colors follow the light or dark theme. Coloring runs in the background, so a large file opens at once and gets its colors a moment later; files over 8 MB, and lines over 20,000 characters, stay plain.

## 0.5.6-alpha (2026-10-31)

- **Material Design Icons:** the program's icons come from Material Design Icons (built in, Apache-2.0): drive types (disk, USB stick, CD/DVD, RAM disk, network drive), server connections and searches in the drive selector and on the drive button, the context menu (instead of the Windows icon font), the sort marks of the panels and the results window, the job queue's arrows, the plugin banner, and files, folders and `..` when Windows' file icons are turned off.

## 0.5.5-alpha (2026-10-31)

- **Accent color instead of the plugin panel tint:** Settings → General → Appearance → Accent color colors everything selected or active: the cursor row, the active panel's path bar, selected tabs and buttons, text selections; panels a plugin shows are tinted with a faint wash of it, as before. Text on it stays readable (dark on light colors, white on dark ones). A tint chosen in 0.5.3 or 0.5.4 becomes the accent color.
- **Plugins are told the accent color** (`accent` in the options of `open_file`, `#rrggbb`); the Viewer plugin 0.5.2-alpha, bundled, follows it.

## 0.5.4-alpha (2026-10-31)

- **The whole plugin panel is tinted:** the tint of a panel a plugin shows now covers its path bar row and column header too, not only the list; the path bar of the inactive panel takes a stronger shade of it.
- **No UP-DIR text:** the `..` row no longer shows "UP-DIR" (Czech "NADADR") in the Size column.

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
