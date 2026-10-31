# Viewer plugin: changelog

The plugin's own versions, apart from the program's. Releases are tagged `viewer-v<version>` in the RustySalamander repository.

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
