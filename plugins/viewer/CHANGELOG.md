# Viewer plugin: changelog

The plugin's own versions, apart from the program's. Releases are tagged `viewer-v<version>` in the RustySalamander repository.

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
