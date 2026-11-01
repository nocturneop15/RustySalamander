# Search plugin: changelog

The plugin's own versions, apart from the program's. Releases are tagged `search-v<version>` in the RustySalamander repository.

## 0.5.3-alpha (2026-11-01)

- **The program's masks:** name masks are read as the program's Select by mask reads them: `;;` stands for a `;` in a name, and a mask that can't be read (`*.txt|`, `a<b`) gets a message saying where, instead of being guessed at. Matching many `*` no longer slows down on long names.
- **Results sorted alphabetically** in the user's language (with RustySalamander 0.7.3-alpha and later): names with accents among their plain letters, numbers by value.

## 0.5.2-alpha (2026-11-01)

- **Plugin interface 7:** built for interface 7 (RustySalamander 0.7.0-alpha and later), which lets plugins mark items and show two connections side by side; no change in what it does.

## 0.5.1-alpha (2026-11-01)

- **Plugin interface 6:** built for interface 6 (RustySalamander 0.6.0-alpha and later), which lets plugins say which program versions they work with; no change in what it does.

## 0.5.0-alpha (2026-10-31)

- **Its own version and releases:** the plugin has a version of its own and is updated on its own through Help → Check for updates. Built for plugin interface 5; no change in what it does.
