# Developing RustySalamander

How to build, test and change the program. For writing plugins, see [PLUGINS.md](PLUGINS.md).

## Setup

- **Rust** 1.88 or newer with the GNU toolchain: `rustup default stable-x86_64-pc-windows-gnu`, plus `rustup component add clippy rustfmt`.
- **MinGW-w64** with gcc, g++ and `mingw32-make`, e.g. `winget install BrechtSanders.WinLibs.POSIX.UCRT`.
- In each new PowerShell window, run `. .\env.ps1` in the repository root. It puts cargo and the WinLibs MinGW on `PATH`. If your MinGW is somewhere else, put its `bin` on `PATH` yourself.

```powershell
cargo build                  # debug build: target\debug\rustysalamander.exe (with a console for logs)
cargo build --release        # release build: target\release\…, no console window
cargo test --workspace       # all tests
```

The first build of `sal-app` compiles `7z.dll` from `third_party/7zip-26.05` with `mingw32-make` (a few minutes), and every build after that reuses it. The build script (`crates/sal-app/build.rs`) then:

- copies `crates/sal-app/lang/*.toml` to `target\<profile>\lang`;
- builds the 7-Zip plugin with a nested `cargo build` in `target\plugins-build`;
- installs the plugin into `target\<profile>\plugins\7zip`, together with its `lang` folder, `7z.dll` and 7-Zip's license files.

So `target\<profile>` is a complete portable installation.

## Repository layout

| Path | What it is |
|---|---|
| `crates/sal-core` | Everything that needs no window: listing, sorting, masks, quick search, file operations, drives, encodings, configuration. Unit-tested. |
| `crates/sal-app` | The GUI (egui/eframe): main window, panels, dialogs, worker, viewer, plugin host. Most features are covered by headless UI tests in `app.rs`. |
| `crates/sal-plugin-api` | The interface between the program and plugin DLLs. Documented for plugin authors (`cargo doc -p sal-plugin-api --open`). |
| `plugins/sevenzip` | The 7-Zip plugin. |
| `plugins/example-filelist` | The example plugin for [PLUGINS.md](PLUGINS.md). It is built and tested, but not installed. |
| `third_party/7zip-26.05` | Unmodified 7-Zip source; only `CPP/7zip/Bundles/Format7zF` is built. |
| `docs/` | This guide, the plugin guide and the analysis of the original program. |

## Tests

`cargo test --workspace` runs:

- **unit tests** in `sal-core`, `sal-plugin-api` and the plugins;
- **headless UI tests** in `crates/sal-app/src/app.rs`. They drive the real app with real egui frames and key events, but without an OS window, against temporary folders. A typical test presses F5, types a target, presses Enter, waits for the job and checks the files on disk.
- **archive tests**, which use the plugin and `7z.dll` the build installed into `target\debug\plugins`. Without MinGW they are skipped with a message.

Rules the tests keep, and new tests must keep too:

- **Never touch the desktop.** Tests don't send keystrokes or mouse input to Windows, and don't open real programs. `shell_open` in `app.rs` only records what would have been opened (`tests::OPENED`), so tests can check that Enter on a file would open it.
- **Use temporary folders** (`tempfile::tempdir()`), never the working directory.
- **Plugin DLLs stay loaded** until the test process ends, so tests that copy plugins put them under `target\<profile>\test-plugins\<test name>`, not in a temporary folder that would have to be deleted.

**Snapshot tests** render parts of the UI to PNG files so you can look at them. They are `#[ignore]`d:

```powershell
$env:SNAPSHOT_DIR = "D:\tmp\snapshots"
cargo test -p sal-app snapshot_ -- --ignored
```

Examples are `snapshot_czech` (the main window and Settings in Czech), `snapshot_operations`, `snapshot_context_menu` and `snapshot_quick_search`.

## Code style

- `cargo fmt --all` uses `rustfmt.toml`: long lines (up to 180) are the house style.
- `cargo clippy --workspace --all-targets` should report nothing.
- **Comments** explain *why*, and the non-obvious *what*. Every file starts with a `//!` comment describing its part of the program. `sal-plugin-api` keeps `#![warn(missing_docs)]`: everything public there is documented.
- **Unicode:** paths are `PathBuf` (lossless), display text is `String`. Never convert paths through ANSI code pages, and never compare file names with `to_lowercase` on bytes. `sal_core::encoding` and `sal_core::masks` have the helpers.
- **UI text** goes through `tr("…")`, `trf("… {x}", &[("x", &v)])` or `trn("{n} file", "{n} files", n)`; constants are marked with `n_("…")`. After adding or changing texts:

  ```powershell
  cargo test -p sal-app export_english_strings -- --ignored   # rewrites every en.toml
  ```

  Then add the Czech translation to `crates/sal-app/lang/cs.toml` (or the plugin's `lang/cs.toml`). The test `language_files_match_the_code` fails while any language file lacks a string or has an unused one.

## Environment variables

| Variable | Effect |
|---|---|
| `RUSTYSALAMANDER_CONFIG` | Use this configuration file instead of `config.toml` next to the program. |
| `RUSTYSALAMANDER_PLUGINS` | Look for plugins in this folder instead of `plugins` next to the program. |
| `SAL_DIAG_DIALOG` | `mkdir`, `settings`, `help`, `copy` or `delete`: open that dialog shortly after start-up. Any other value opens a message box. |
| `SAL_DIAG_DIALOG_CLOSE` | `cancel` or `ok`: close that dialog again after about 3 s. |
| `SAL_DIAG_ANSWER` | Answer the first conflict or error question with *Skip* after about 6 s. |
| `SAL_DIAG_MENU`, `SAL_DIAG_MENU_MS` | Open both context-menu styles once after start-up, close them by timer after `SAL_DIAG_MENU_MS` ms (default 1000), and log each step to stderr. |

The `SAL_DIAG_*` variables exist to check dialogs, focus and menus in the real program, for example by capturing the program's own window, without synthetic input. Combine them with `RUSTYSALAMANDER_CONFIG` pointing at a scratch configuration, so a diagnostic run doesn't change your settings.

## Making a release

1. Run `cargo clippy --workspace --all-targets`, `cargo test --workspace` and `cargo build --release`.
2. Package `target\release` without build leftovers: `rustysalamander.exe`, `lang\`, `plugins\`, plus `README.md`, `LICENSE` and `THIRD-PARTY-NOTICES.md`. Leave out `config.toml`, `build\`, `deps\`, `incremental\`, `.fingerprint\`, `*.d` and `*.pdb`.
3. Bump `version` in the root `Cargo.toml`. Bump `sal_plugin_api::abi::VERSION` whenever the plugin interface changed.
