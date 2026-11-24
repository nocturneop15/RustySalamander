# Viewer plugin: changelog

The plugin's own versions, apart from the program's. Releases are tagged `viewer-v<version>` in the RustySalamander repository.

## 0.7.4-alpha (2026-11-24)

- **Line numbers:** the Text view shows each line's number left of it, on from the start; **Line numbers** in the toolbar or **Ctrl+L** turns them off and on. They stand in a column in the colors of the panels' footer, and stay in place when a long line is scrolled sideways, and with Wrap on, each number is beside its line's first row. Of a file too large to show whole, the lines are counted from the start of the part shown.
- **Choices remembered:** **Line numbers** and **Wrap** stay as you left them for the next window and the next session (Wrap only for files small enough for it), saved in `settings.toml` in the plugin's folder.

## 0.7.3-alpha (2026-11-24)

- **Find searches beside the window:** the window stays usable while a big file is searched, the bar says **Searching…** until it is done (never "Not found" before), and a search for what was typed before is given up as you type on: one search at a time, the latest only, however fast you type. When the text changes (read again, a file followed grows), the matches found in it before are forgotten at once, so F3 never goes to a line that isn't there any more. What is looked for takes at most 1 000 characters (a bigger paste keeps the start). Works with RustySalamander 0.9.4-beta and later.

## 0.7.2-alpha (2026-11-20)

- **Selecting and copying text:** in the Text view, drag the mouse over the text (past the window's edge it scrolls along), Shift+click to extend, double-click for a word. **Shift** with the arrows, Home/End and Page Up/Down selects from where you clicked, or from the first line shown; with **Ctrl**, ←/→ go by words and Home/End to the start and the end of the file. **Ctrl+A** selects all, **Ctrl+C** or Ctrl+Insert copies (lines end with CR LF). The keys without Shift scroll as before; while the find box has the keyboard, they are its. Works with RustySalamander 0.9.4-beta and later.

## 0.7.1-alpha (2026-11-18)

- **Scrolls with the keyboard:** ↑ and ↓ move the text a line, Page Up and Page Down a screen, Home and End (also with Ctrl) to the start and the end, ← and → sideways; in the Text and the Hex view alike. While the find box has the keyboard, the keys are its. When following a file, going up stops at what you read; End follows again. Works with RustySalamander 0.9.4-beta and later.

## 0.7.0-alpha (2026-11-16)

- **Follows a file as it grows, like `tail -f`:** the ▶ button at the left of the toolbar (or Ctrl+T) shows what is added to the file as it is written, a log for instance, four times a second; ⏸ (Ctrl+T again) stops it. The view goes to the end of the file and stays there as lines come, unless you scroll up to read; scroll back to the end and it follows again. A line written in pieces is shown whole once it is; a log started anew (shorter, or another file in its place) is read anew; a file deleted meanwhile leaves the text as it was. A file too large to show whole shows its last 32 MB instead of its first (*showing last …*), and the oldest text goes as more comes. Colors, search marks and the Hex view follow too: lines added are colored on from the lines before them, without coloring the whole file again. Works with RustySalamander 0.9.4-beta and later.

## 0.6.4-alpha (2026-11-07)

- **Drawn by the program:** the Viewer's window is shown and drawn by RustySalamander (plugin interface 9), so the plugin no longer carries a graphics library of its own and is much smaller, and the window draws wherever the program's does: also on the processor over Remote Desktop and on Windows 8.1 / Server 2012 R2. Needs RustySalamander 0.9.4-beta or later.

## 0.6.3-alpha (2026-11-05)

- **Opens on the program's monitor:** the Viewer's window appears over RustySalamander's window, on the same monitor, each next one a little lower and to the right (with RustySalamander 0.9.1-beta and later; before, it opened where Windows put it). Built for plugin interface 8.

## 0.6.2-alpha (2026-11-03)

- **The program's spacing:** the toolbar, the find bar and the text use the same margins, button padding and control height as RustySalamander's own windows, and the text starts at the same edge as the toolbar's buttons.

## 0.6.1-alpha (2026-11-03)

- **Room around the controls:** the toolbar and the find bar have padding, so buttons, lists, tick boxes and the find field no longer touch each other or the edges of the bars. The find field is as tall as the buttons beside it.
- **The current match is readable:** its text is dark on the solid orange; before, colored text could disappear in it.

## 0.6.0-alpha (2026-11-03)

- **Find in the file:** Ctrl+F (or the magnifier in the toolbar) opens a find bar. Every match is marked as you type, the current one in orange, with *3 of 12* beside it; Enter or F3 goes to the next match, Shift+Enter or Shift+F3 to the previous one, round the end. **Match case** makes upper and lower case count; without it, case is ignored in every language (`ŽLUŤ` finds `žluť`). **Wildcards** makes `*` stand for any characters and `?` for one (`file*.txt`, `a?b`); without it, they are plain characters. Esc closes the bar, a second Esc the window. A match found while the Hex view is shown is shown in the Text view.

## 0.5.9-alpha (2026-11-02)

- **No window animation with animations off:** with View → Animations turned off in the program (RustySalamander 0.7.4-alpha and later), Windows no longer fades or zooms its windows in and out.

## 0.5.8-alpha (2026-11-01)

- **Animations can be off:** with View → Animations turned off in the program (RustySalamander 0.7.1-alpha and later), its windows don't fade in or scroll smoothly either.

## 0.5.7-alpha (2026-11-01)

- **Plugin interface 7:** built for interface 7 (RustySalamander 0.7.0-alpha and later), which lets plugins mark items and show two connections side by side; no change in what it does.
- **Its windows come from `sal-window`,** the window code the Viewer and Compare plugins share; they look and behave as before.

## 0.5.6-alpha (2026-11-01)

- **Plugin interface 6:** built for interface 6 (RustySalamander 0.6.0-alpha and later), which lets plugins say which program versions they work with; no change in what it does.

## 0.5.5-alpha (2026-11-01)

- **An accent color for each theme:** the windows follow the light and the dark theme's own accent colors chosen in the program (RustySalamander 0.5.9-alpha and later); with older versions, the one color they send.

## 0.5.4-alpha (2026-10-31)

- **Window icon:** the viewer's windows show a document with an eye on a rust-colored square, instead of egui's default "e" logo, which looked like Internet Explorer's.

## 0.5.3-alpha (2026-10-31)

- **Syntax highlighting:** text is colored by its format: JSON, XML, HTML, PowerShell, Bash, Batch, INI, TOML, YAML, Markdown, CSS, JavaScript, Python, C/C++, C#, Rust, SQL, Dockerfile and about 200 more. The format is recognised from the file name (`.json`, `Makefile`, `.bashrc`) or the first line (`#!/bin/bash`, `<?xml`), and the **Syntax** list in the toolbar picks another one, or Plain text. The colors follow the light or dark theme. Coloring runs in the background, so a large file opens at once and gets its colors a moment later; files over 8 MB, and lines over 20,000 characters, stay plain.

## 0.5.2-alpha (2026-10-31)

- **The accent color:** the windows follow the accent color chosen in the program (Settings → General → Appearance), for the selected Text/Hex button, the encoding list and text selections.

## 0.5.1-alpha (2026-10-31)

- **No changes:** a release to try updating a plugin on its own (Help → Check for updates… → Update now, then Restart now).

## 0.5.0-alpha (2026-10-31)

- **The text and hex viewer as a plugin:** the viewer F3 opens, until now built into the program, is this plugin (`plugins\viewer\viewer.spl`). It registers itself as the viewer of every file and opens its windows on a thread of its own; they look like the program's (theme, fonts, scroll bars).
- **Text** in the encoding it detects (UTF-8, UTF-16, the Windows code page) or one chosen from the list, with word wrap (Ctrl+W) for files up to 2 MB; **hex** with F4, binary files open in it. Files over 32 MB show their first 32 MB.
- **Several files at once,** each in a window of its own; closing any of them leaves the others open.
- **Its own version and releases:** updated on its own through Help → Check for updates.
