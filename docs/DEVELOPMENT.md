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
tools\stage.ps1              # the built program alone: target\RustySalamander
```

`target\release` is Cargo's folder, full of its build cache. `tools\stage.ps1` copies just the program into `target\RustySalamander`, laid out like the release package (program, `lang`, plugins with their languages and changelogs, documents, licenses); run and test it from there. What the program saves there (`config.toml`, `known_hosts`, debug logs) stays from one build to the next.

The first build of `sal-app` compiles `7z.dll` from `third_party/7zip-26.05` with `mingw32-make` (a few minutes), and every build after that reuses it. The build script (`crates/sal-app/build.rs`) then:

- copies `crates/sal-app/lang/*.toml` to `target\<profile>\lang`;
- builds the 7-Zip plugin with a nested `cargo build` in `target\plugins-build`;
- installs the plugin into `target\<profile>\plugins\7zip`, together with its `lang` folder, `7z.dll` and 7-Zip's license files;
- builds the SCP plugin the same way and installs it into `target\<profile>\plugins\scp`.

So `target\<profile>` is a complete portable installation.

## Repository layout

| Path | What it is |
|---|---|
| `crates/sal-core` | Everything that needs no window: listing, sorting, masks, quick search, file operations, drives, encodings, configuration. Unit-tested. |
| `crates/sal-app` | The GUI (egui/eframe): main window, panels, dialogs, worker, viewer, plugin host. Most features are covered by headless UI tests in `app.rs`. |
| `crates/sal-plugin-api` | The interface between the program and plugin DLLs. Documented for plugin authors (`cargo doc -p sal-plugin-api --open`). |
| `plugins/sevenzip` | The 7-Zip plugin. |
| `plugins/scp` | The SCP plugin: SSH servers in a panel. |
| `plugins/example-filelist` | The example plugin for [PLUGINS.md](PLUGINS.md). It is built and tested, but not installed. |
| `third_party/7zip-26.05` | Unmodified 7-Zip source; only `CPP/7zip/Bundles/Format7zF` is built. |
| `docs/` | This guide, the plugin guide and the analysis of the original program. |

## Tests

`cargo test --workspace` runs:

- **unit tests** in `sal-core`, `sal-plugin-api` and the plugins;
- **headless UI tests** in `crates/sal-app/src/app.rs`. They drive the real app with real egui frames and key events, but without an OS window, against temporary folders. A typical test presses F5, types a target, presses Enter, waits for the job and checks the files on disk.
- **archive tests**, which use the plugin and `7z.dll` the build installed into `target\debug\plugins`. Without MinGW they are skipped with a message.
- **the SCP plugin against a real SSH server**: the test starts a small russh server on `127.0.0.1` that runs each command through Git for Windows' bash (`C:\Program Files\Git\usr\bin`, which has `ls`, `scp`, `rm`, …). Keys are made with Git's `ssh-keygen`. Without Git for Windows it is skipped.

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
- **Spacing and alignment** come from `sal_ui::layout`, in the program, its dialogs and the plugins' windows alike: margins, gaps (`GAP_SMALL`, `GAP`, `GAP_LARGE`), button padding, the height of controls, and the frames and grids built from them (`dialog_frame`, `bar_frame`, `content_frame`, `group_frame`, `form_grid`, `table_grid`, `divider`, `before_buttons`, `buttons`). A dialog is its content, `before_buttons`, then `buttons` (centred). A dialog that asks something passes `buttons` its **default button**, the one Enter presses: it is drawn lightly tinted with the accent color, so it is clear what Enter does, and Enter must do exactly what that button does (the safe answer where one is destructive: *Skip*, not *Overwrite*). A row without a default says why in a comment on the line above; the test `button_rows_have_a_default_or_say_why` fails otherwise. A form has its labels in the first column, left-aligned, and tick boxes in the second with their label to the right. Never write a gap or margin as a number: a measure that is missing gets a name in `layout.rs`. The test `windows_use_these_measures` (in `sal-ui`) fails on a number in `add_space`, `inner_margin`, `Margin::…`, `.spacing([…])` or `item_spacing`/`button_padding` in the program's and the plugins' window code.

## Environment variables

| Variable | Effect |
|---|---|
| `RUSTYSALAMANDER_CONFIG` | Use this configuration file instead of `config.toml` next to the program. |
| `RUSTYSALAMANDER_PLUGIN_DATA` | Set by the program in multi-user mode (`multiuser.toml` next to it): the folder below which plugins save, read by `sal_plugin_api::data_dir`. |
| `RUSTYSALAMANDER_PLUGINS` | Look for plugins in this folder instead of `plugins` next to the program. |
| `RUSTYSALAMANDER_LOG` | A timing log: a file path, or `1` for `%TEMP%
ustysalamander.log`. It records each start-up step (configuration, fonts, plugins, each panel) and every frame that takes over 150 ms, split into its parts (menu, each panel, dialogs…), plus background reads. Use it when the program is slow somewhere. |
| `SAL_DIAG_DIALOG` | `mkdir`, `settings`, `help`, `copy` or `delete`: open that dialog shortly after start-up. Any other value opens a message box. |
| `SAL_DIAG_DIALOG_CLOSE` | `cancel` or `ok`: close that dialog again after about 3 s. |
| `SAL_VSYNC` | `on` or `off`: whether frames wait for the screen's refresh. By default on, and off in a Remote Desktop session (frames then at most about 60 a second). The debug log says which, and for each frame with input how long since the last frame and how long that one took. |
| `SAL_DIAG_VIEW` | Files separated by `;`: open them in the viewer shortly after start-up, as F3 does (the plugins' windows, which the program draws). |
| `SAL_RENDER` | `gdi` or `wgpu`; unset: `gdi` in a Remote Desktop session and where Windows has no Direct3D 12 (8.1, Server 2012 R2), `wgpu` otherwise. `gdi`: the program runs its windows itself (`crates/sal-app/src/gdiwin.rs`, winit and egui-winit as eframe uses them), draws them on the processor (`sal-soft`) and puts only the changed parts on screen with GDI. Otherwise eframe and wgpu. The debug log says for each frame drawn how long drawing and showing took and how much of the window changed (`··· gdi`), and the window's size when it changes (`··· window`). |
| `SAL_DIAG_ANSWER` | Answer the first conflict or error question with *Skip* after about 6 s. |
| `SAL_DIAG_MENU`, `SAL_DIAG_MENU_MS` | Open both context-menu styles once after start-up, close them by timer after `SAL_DIAG_MENU_MS` ms (default 1000), and log each step to stderr. |
| `SAL_DIAG_SEARCH` | A folder: search it for `*.dll` files containing a text that isn't there (a long search), keep the results window open for about 6 s, then quit. |
| `SAL_DIAG_WINDOWS` | Log what each dialog window does, every frame, to stderr: hidden or shown, its owner looked up, resized. A line repeated every frame other than `frame:` means the window is being reset. |
| `SAL_DIAG_FOREGROUND` | `1`: open dialog windows as if the program were in the foreground, as when the user works in it (a program started from a script usually isn't). `0`: as if the user worked in another program (the dialog opens behind, the taskbar button flashes). |

The `SAL_DIAG_*` variables exist to check dialogs, focus and menus in the real program, for example by capturing the program's own window, without synthetic input. Combine them with `RUSTYSALAMANDER_CONFIG` pointing at a scratch configuration, so a diagnostic run doesn't change your settings.

## Making a release

1. Run `cargo clippy --workspace --all-targets`, `cargo test --workspace` and `cargo build --release`.
2. Package the program with `tools\stage.ps1 -Out target\dist\RustySalamander-<version>-win64 -Fresh` and zip that folder (the folder itself in the zip). It holds `rustysalamander.exe`, `lang\`, `plugins\` (each plugin with its `lang\` and `CHANGELOG.md`, 7-Zip with `7z.dll` and its licenses), `README.md`, `CHANGELOG.md`, `LICENSE.txt`, `THIRD-PARTY-NOTICES.md`, `MaterialDesignIcons-LICENSE.txt` and `USER-MANUAL.md`; no settings and no build leftovers.
3. Bump `version` in the root `Cargo.toml`. Bump `sal_plugin_api::abi::VERSION` whenever the plugin interface changed.
