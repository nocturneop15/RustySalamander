# Viewer plugin: changelog

The plugin's own versions, apart from the program's. Releases are tagged `viewer-v<version>` in the RustySalamander repository.

## 0.5.0-alpha (2026-10-31)

- **The text and hex viewer as a plugin:** the viewer F3 opens, until now built into the program, is this plugin (`plugins\viewer\viewer.spl`). It registers itself as the viewer of every file and opens its windows on a thread of its own; they look like the program's (theme, fonts, scroll bars).
- **Text** in the encoding it detects (UTF-8, UTF-16, the Windows code page) or one chosen from the list, with word wrap (Ctrl+W) for files up to 2 MB; **hex** with F4, binary files open in it. Files over 32 MB show their first 32 MB.
- **Several files at once,** each in a window of its own; closing any of them leaves the others open.
- **Its own version and releases:** updated on its own through Help → Check for updates.
