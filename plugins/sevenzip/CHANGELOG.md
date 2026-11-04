# 7-Zip archives plugin: changelog

The plugin's own versions, apart from the program's. Releases are tagged `7zip-v<version>` in the RustySalamander repository.

## 0.6.0-alpha (2026-11-04)

- **Dragged out of the window, files are read only as they are copied:** the plugin streams them out of the archive in its own order (a solid archive is decoded once), with no temporary copy. Built for plugin interface 8 (`stream`): needs RustySalamander 0.9.0-beta or later.

## 0.5.2-alpha (2026-11-01)

- **Plugin interface 7:** built for interface 7 (RustySalamander 0.7.0-alpha and later), which lets plugins mark items and show two connections side by side; no change in what it does.

## 0.5.1-alpha (2026-11-01)

- **Plugin interface 6:** built for interface 6 (RustySalamander 0.6.0-alpha and later), which lets plugins say which program versions they work with; no change in what it does.

## 0.5.0-alpha (2026-10-31)

- **Its own version and releases:** the plugin has a version of its own and is updated on its own through Help → Check for updates. Built for plugin interface 5; no change in what it does.
