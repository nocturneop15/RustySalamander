# Open Salamander → RustySalamander: Analysis

Source analysed: `upstream/` (shallow clone of https://github.com/OpenSalamander/salamander, GPL-2.0-or-later).

## 1. Size and structure

| Component | Path | Lines (C/C++) | Notes |
|---|---|---:|---|
| Core application | `src/*.cpp, *.h` | ~222,000 | ~110 translation units, pure WinAPI, no MFC/STL |
| Shared libraries | `src/common` | ~322,000 | includes third-party deps (zlib, bzip2, 7-zip bits, …) |
| Plugins | `src/plugins` | ~712,000 | 37 plugins: archivers (zip, 7zip, tar, rar, cab, iso, lha, arj, chm, mime, ole, pak), FS (ftp, winscp, nethood, regedt, wmobile, undelete, unfat), viewers (pictview, mmviewer, peviewer, ieviewer, dbviewer), tools (filecomp, checksum, diskmap, renamer, splitcbn, automation) |
| Helpers | salmon, salopen, salspawn, shellext, tserver, reglib, setup, translator | ~51,000 | crash reporter, process helpers, shell extension, trace server, installer, translation tooling |
| **Total** | | **~1.3 million** | |

Core subsystems (by file prefix):

- `mainwnd*`, `fileswn*`, `filesbx*`, `filesmap` – main window, the two file panels, list box, drag-select map
- `salamdr*` – app entry, global helpers, path handling
- `worker.cpp` – background copy/move/delete engine (with progress dialog)
- `sort.cpp` – panel sorting incl. "logical" (numeric-aware) name compare
- `masks.cpp` – wildcard masks (`*`, `?`, `#`), mask groups (`;`, `|` exclusion, `;;` escape), `MaskName` (copy-style rename), quick-search
- `find*.cpp`, `filter.cpp` – Find dialog, panel filter
- `viewer*.cpp`, `codetbl.cpp`, `/convert` – internal text/hex viewer and codepage conversion tables
- `pack*.cpp`, `zip.cpp`, `sal*zlib/bzip2/inflt` – external/internal archiver support
- `plugins*.cpp` + `plugins/shared/spl_*.h` – plugin host and the plugin ABI
- `toolbar*`, `menu*`, `dialogs*`, `stswnd`, `tabwnd`, `drivelst` – UI chrome
- `regwork.cpp`, `cfgdlg*` – configuration stored in the Windows registry
- `icncache`, `geticon`, `thumbnl`, `shiconov` – icon/thumbnail/overlay reading on worker threads

## 2. Text encoding: the core problem

The whole program — core *and* the plugin ABI — is **ANSI (`char*`, system codepage)**:

- `struct CFileData` (`plugins/shared/spl_com.h`) — the record behind every panel item — stores `char* Name`, `char* Ext`, `char* DosName`. Every plugin consumes and produces this struct.
- `CFileData::NameLen` is a **9-bit** bitfield → hard max name length `MAX_PATH - 5`.
- 2,236 uses of `MAX_PATH` in core alone (fixed `char buf[MAX_PATH]` stack buffers everywhere).
- 63 `FindFirstFile` (ANSI-mapped) calls vs. 6 `CreateFileW`; 52 `CP_ACP` conversions vs. 3 `CP_UTF8`.
- Case-insensitive matching uses a 256-entry `LowerCase[]` table indexed by `char` (`masks.cpp` `AgreeMask`) — only correct for single-byte codepages.
- Sorting uses `CompareString(LOCALE_USER_DEFAULT, …)` on ANSI strings.
- `\\?\` long paths are explicitly rejected in places (`salamdr3.cpp:427`).

Consequences today:

1. Any file whose name contains characters outside the active codepage (e.g. Chinese on a Czech system, emoji anywhere) is shown as `?` and **cannot be opened, copied, renamed or deleted** — the ANSI name does not round-trip.
2. Paths longer than 260 characters fail.
3. Fixing this in place means changing the plugin ABI (all 37 plugins) and every string buffer in the core — effectively a rewrite of the string layer.

### Why a Rust rewrite fixes this structurally

- Rust `String`/`&str` are UTF-8 by construction; `std::path::PathBuf`/`OsString` on Windows store names as **WTF-8**, which round-trips *every* NTFS name losslessly — including invalid UTF-16 (unpaired surrogates), which pure UTF-8 cannot represent.
- Design rule adopted in RustySalamander: **identity = `PathBuf` (lossless), display/compare = `String` (UTF-8, lossy only for display)**. Operations always use the `PathBuf`, so a name that can't be displayed perfectly can still be opened/copied/deleted.
- std uses the `W` (UTF-16) Win32 APIs and automatically adds the `\\?\` prefix for long paths, removing the `MAX_PATH` limit.
- Case folding and comparison use Unicode-aware lowercasing instead of a codepage table.

## 3. Rewrite feasibility and strategy

A line-for-line port of ~1.3M lines is a multi-person-year effort. The rewrite is therefore **phased**, keeping behaviour (masks, sort order, keyboard model) compatible while replacing architecture:

| Phase | Scope | Status |
|---|---|---|
| 0 | Analysis, workspace, Unicode-safe data model | ✅ done |
| 1 | `sal-core`: listing, sorting (logical compare), masks/mask groups/MaskName, quick-search, file operations (copy/move/delete/mkdir/rename) with progress, conflicts, retry/skip and cancel, drives, free space, encoding detection, bidi | ✅ done, unit-tested |
| 2 | `sal-app`: dual-pane GUI (egui), Salamander keyboard model, background worker with progress/prompts, F3 text/hex viewer with encoding switching, Unicode font fallback, auto-refresh on directory change | ✅ done, covered by headless UI tests |
| 3 | Configuration (TOML instead of registry), history, hot paths | ✅ done. Still missing: panel view modes (brief/icons/thumbnails), column configuration, user menu |
| 4 | Archives as virtual file systems | ✅ done: the 7-Zip plugin (7z.dll in-process) reads ~25 formats and changes 7z, zip, tar, wim. Still missing: encrypted archives |
| 5 | New plugin interface: separate DLLs (`*.spl`), a C ABI generated from Rust traits, UTF-8 text and UTF-16 paths, no `CFileData` compatibility | ✅ folder plugins done (`sal-plugin-api`, docs/PLUGINS.md). Still missing: file-system plugins (FTP) and viewer plugins |
| 6 | Find dialog, directory compare, change attributes, occupied space, pack/unpack, user menu, clipboard copy/paste of files | planned |
| 7 | FS plugins (FTP/SFTP, network, registry); image viewer (`image` crate / WIC) | started: plugin commands, forms and connections in the interface; the SCP plugin (SSH servers). FTP, SFTP, registry and the image viewer planned |
| 8 | Shell integration: context menus, shell icons (`SHGetFileInfoW`), drag & drop, IFileOperation-based Recycle Bin for long paths | partly: context menus (Windows 11 and classic) and shell icons done; drag & drop and the Recycle Bin for long paths planned |
| 9 | Complex script shaping (Arabic joining, Indic conjuncts) via a shaping text renderer | planned (egui limitation) |

Not ported deliberately: crash reporter (`salmon` → Rust panics + logging), trace server (`tserver` → `tracing`), installer (`setup` → MSI/cargo-dist), the ANSI plugin ABI.

## 4. Behaviour preserved from upstream

- **Masks** (`masks.cpp`): `*`, `?`, `#` (extended mode); group separator `;`, literal `;` via `;;`; `|` starts exclude masks; leading `|` implies `*`; `*.*` matches names without extension; `**`→`*`, `*?`→`?*` normalisation; leading/trailing spaces trimmed.
- **MaskName**: Win2K `copy`-style rename (`*.bak`, `a?c.*`, …), trailing dots trimmed.
- **Logical sort** (`sort.cpp` `StrCmpLogicalEx`): numbers compared by value, `"001" < "01"` tie-break, dots as separators; directories first; `..` on top; sort by name/ext/time/size/attr with name+ext as tie-breakers.
- Extension rule: text after the last dot; a leading dot counts as an extension (Windows semantics, e.g. `.cvspass`).
