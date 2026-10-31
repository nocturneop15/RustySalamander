# Third-party notices

RustySalamander is licensed under the GNU General Public License, version 2 or (at your option) any later version. See [LICENSE](LICENSE).

## Open Salamander

RustySalamander is a rewrite of [Open Salamander](https://github.com/OpenSalamander/salamander) (GPL-2.0-or-later). Its behaviour (keyboard model, mask syntax, sort order, file operations) follows the original, and parts are ports of its C++ code (`masks.cpp`, `sort.cpp` and the quick-search code). Open Salamander is the work of its authors; see the AUTHORS file in its repository.

## 7-Zip (the 7-Zip plugin)

The 7-Zip plugin (`plugins\7zip`) uses `7z.dll`, built unmodified from the 7-Zip 26.05 source in `third_party/7zip-26.05`.

7-Zip Copyright (C) 1999-2026 Igor Pavlov.

- Most of 7-Zip is under the GNU LGPL, version 2.1 or later.
- The RAR decompression code (`CPP/7zip/Compress/Rar*`) is under the GNU LGPL with the **unRAR license restriction**: it may not be used to re-create the RAR compression algorithm, which is proprietary.
- A few files are under BSD licenses or in the public domain.

The full terms are in `third_party/7zip-26.05/DOC/License.txt` and `DOC/unRarLicense.txt`, and the build copies both next to `7z.dll` (`plugins\7zip\License.txt`, `plugins\7zip\unRarLicense.txt`). The source of the library is the 7-Zip source in this repository, or <https://www.7-zip.org/>.

## The SCP plugin

The SCP plugin (`plugins\scp`) contains russh (Apache-2.0), its key handling (ssh-key, MIT or Apache-2.0), tokio (MIT), and ring for cryptography (ISC-style and Apache-2.0 licenses, with parts from BoringSSL). They are listed in `Cargo.lock` like the program's own crates.

## The Viewer plugin's syntax highlighting

The Viewer plugin (`plugins\viewer`) colors text with [syntect](https://github.com/trishume/syntect) (MIT), the syntax definitions and themes of [bat](https://github.com/sharkdp/bat) as packaged by [two-face](https://github.com/CosmicHorrorDev/two-face) (MIT or Apache-2.0), and the [Oniguruma](https://github.com/kkos/oniguruma) regular expression library (BSD-2-Clause, through the `onig` crate). The syntax definitions and color themes come from many authors under their own open licenses (mostly MIT, Apache-2.0 and BSD); two-face lists each one with its license at <https://github.com/CosmicHorrorDev/two-face/blob/v0.4.5/generated/acknowledgements_full.md>.

## Material Design Icons (the program's icons)

The icons (drives, connections, searches, the context menu, sort marks, and files and folders when Windows' file icons are off) come from [Material Design Icons](https://pictogrammers.com/library/mdi/) by Pictogrammers, the `@mdi/font` 7.4.47 font (`materialdesignicons-webfont.ttf`), cut down to the icons in use (`tools/subset-mdi.py`) and built into the program and the Viewer plugin. The font is distributed under the **Apache License 2.0** (<https://www.apache.org/licenses/LICENSE-2.0>), which is compatible with RustySalamander's license through the GNU GPL version 3. Its license file is `crates/sal-ui/fonts/materialdesignicons-LICENSE.txt` in the source:

> Pictogrammers Free License. This icon collection is released as free, open source, and GPL friendly by the Pictogrammers icon group. Icons: Apache 2.0. Fonts: Apache 2.0. Code: MIT.

## Rust crates

The program uses Rust crates from crates.io, among them egui/eframe (MIT or Apache-2.0), windows (MIT or Apache-2.0), serde and toml (MIT or Apache-2.0) and chrono (MIT or Apache-2.0). `Cargo.lock` lists every crate and version, and each crate's license is in its package metadata (`cargo metadata`, or a tool such as `cargo about` or `cargo deny` can produce the full list).
