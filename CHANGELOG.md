# Changelog

## 0.2.0-alpha (2026-10-30)

Plugins can now add commands, ask for input and open connections, and the first plugin that does is **SCP**: files on SSH servers, in a panel.

### SCP plugin (new)

- **Servers in a panel:** **Plugins → Connect to server…** asks for the server, port, user name and a password or a private key (with its passphrase). The server's files open in the active panel as `scp://user@host/home/user`; copying in and out, deleting, renaming, new folders, viewing and editing work as in any folder, with progress, pausing and cancelling.
- **Sign-in:** password (servers that use keyboard-interactive prompts get it too) or a private key in OpenSSH or PEM format.
- **Server keys:** keys are checked against `%USERPROFILE%\.ssh\known_hosts` and the plugin's own `known_hosts`. A new server shows its key fingerprint before you trust it; a changed key is refused with a warning.
- **Several sessions at once:** each is listed in the Plugins menu and in the drive selector.
- **Remembered:** the last server, port, user and key file, never a password.
- **How it works:** SSH in pure Rust (`russh`, with `ring`); transfers use the SCP protocol, listings and file management `ls`, `rm`, `mv` and `mkdir` on the server. No SFTP yet.

### Plugins

- **Plugins menu:** the commands of the plugins that are on, under each plugin's name; *Disconnect from …* for every open session, one per row (the menu widens to fit); *Plugin settings…*.
- **Forms drawn by the program:** plugins describe dialogs (text, passwords, a file with Browse…, options, check boxes, notes) and the program draws them like its own.
- **Connections:** plugins can open servers and other places in a panel, shown and typed as `scheme://…` paths. Closing one, turning its plugin off or quitting returns the panel to the folder it showed before.
- **Plugin panels:** panels a plugin shows, archives included, have a tinted background and a banner with the plugin's name below the list.
- **Plugin interface version 3:** `list` takes the folder to list, so plugins can list one folder at a time; `commands`, `run` and `close` are new. Plugins built for 0.1 must be rebuilt. The guide (`docs/PLUGINS.md`) has a new section on commands, forms and connections.

### Drive selector

- **Connections:** open sessions are listed under *Connections*; choosing one opens it in that panel at its start folder. Each row has a **Disconnect** button (or press Delete). Long names are shortened with "…", and the selector widens to fit them.
- **Fits the window:** a long list in a low window no longer covers the menu bar; the selector stays below the path bar and scrolls, keeping the chosen row in view.
- **Simpler:** no Cancel button and no empty space below the list (Esc or a click outside closes it).

### Fixes

- **Copying into a connection:** the copy dialog keeps `scp://` targets, and a typed `scp://…` path works in the path bar.

## 0.1.0-alpha (2026-10-30)

The first public release: an alpha of a Rust rewrite of Open Salamander with full Unicode support. Everyday file management works, but many of Salamander's features are still missing (see *Not yet* below), and the plugin interface may change between releases.

### Unicode everywhere

- **Every file name works.** Names outside the system code page (Chinese, emoji, …) and names with unpaired UTF-16 surrogates can be shown, opened, copied, renamed and deleted.
- **Long paths:** there is no 260-character `MAX_PATH` limit.
- **Matching:** masks, quick search and sorting are Unicode-aware (case folding, numeric-aware "logical" sort as in Salamander).
- **Right-to-left text:** Hebrew and Arabic names are displayed in the right order.
- **Fonts:** a fallback chain covers CJK, Korean, Indic, Thai, symbols and emoji, whatever font is chosen.
- **Forbidden characters:** names from Macs, NAS shares and WSL that contain characters Windows forbids are shown as their original characters.

### Panels and navigation

- **Panels:** two panels with sorting by name, extension, date and size, directories first, `..` on top, and hidden and system files switchable.
- **Keyboard:** Salamander's keyboard model, with every command shortcut configurable in Settings. The editor shows conflicts, offers presets and can reset to defaults. F1 help is generated from the current bindings.
- **Quick search:** typing in a panel searches from the first letter or anywhere in the name. Arrows jump between matches, and the matched part is highlighted.
- **Selection:** by mask (`+`, `-`, `*`), Insert/Space, select all and invert.
- **Path bar:** clickable breadcrumbs that turn into an editable path. It accepts relative paths, `%VARIABLES%`, `~`, `"quoted"` paths, a file to jump to, and UNC paths including a server's share list.
- **Drives:** buttons or a drive selector with an icon per drive type, local and network drives listed separately, recent typed paths, and the option to return to the last folder on each drive.
- **History and hot paths:** history (Alt+←/→) and hot paths (Ctrl+0..9).
- **Folder sizes:** calculated in the background (Shift+Space, Alt+F10), with file and folder counts and a running total in the footer.
- **Icons:** the real Windows file icons, loaded in the background.
- **Auto-refresh:** a panel reloads when its folder changes.

### File operations

- **Operations:** copy, move, delete (to the Recycle Bin or permanently), create folders including nested paths (`a/b/c`), and rename. A copy target can be a folder, a new name or a mask such as `*.bak`.
- **Progress:** bytes and files, speed and time left. Each operation can be paused, resumed and cancelled.
- **Queue:** *Run first*, *Queue last* and *Start now*, reordering, *Pause all* and *Cancel all*, and running in the background with progress in the status bar.
- **Conflicts and errors:** a conflict window compares both items side by side. Errors offer *Retry*, *Skip*, *Skip all* and *Cancel*.

### Archives (the 7-Zip plugin)

- **Browsing:** archives open as folders. It reads zip, 7z, rar, tar, gz, bz2, xz, zst, lzma, cab, arj, lzh, iso, wim, cpio, rpm, deb and split archives.
- **Changing archives:** 7z, zip, tar and wim archives can be copied into, deleted from and renamed inside, and new folders created in them. Every change is written safely: a failed or cancelled change leaves the archive as it was.
- **Copying:** copy and move out of, into and between archives, with conflict questions and progress.
- **Opening files:** view, edit or open a file inside an archive. It is extracted to a temporary folder, which is cleaned up on exit.
- **Library:** uses 7-Zip 26.05 (`7z.dll`, built from source) inside the program; nothing installed on the computer is needed.

### Plugins

- **Separate DLLs:** plugins are DLLs (`plugins\<name>\<name>.spl`), found when the program starts and switched on or off in Settings → Plugins. Settings says why a plugin can't start (a missing or damaged library, another interface version).
- **Interface:** a documented plugin interface (`sal-plugin-api`) for *folder plugins*. The C ABI is generated from Rust traits, with UTF-8 text and UTF-16 paths. Each plugin has its own language files.
- **For developers:** a guide for plugin authors (`docs/PLUGINS.md`) and an example plugin (`plugins/example-filelist`).

### Viewer, editor, terminal

- **Viewer (F3):** built-in text and hex viewer in its own window. It detects UTF-8, UTF-16 and ANSI, and can switch to Windows-125x, ISO-8859-2, KOI8-R, CP866, Shift-JIS, EUC-JP, GBK, Big5 and EUC-KR.
- **External programs:** a custom viewer or editor (F4, Notepad by default) can be set, with `{file}` placeholders.
- **Command prompt here:** opens the Windows default terminal, cmd, PowerShell or a custom command.

### Windows integration

- **Context menus:** a compact Windows 11 style menu, or the classic full shell menu with *Send to*, *Open with* and third-party extensions. The menu can open on right-click or long right-click.
- **Properties:** Alt+Enter, and copying full paths to the clipboard.
- **Themes:** light, dark or following Windows, including the title bar and native menus.

### Interface

- **Dialogs:** in their own windows, which can be moved to another monitor, centred, and modal where needed.
- **Fonts:** a choice of UI and viewer fonts.
- **Layout:** always-visible scrollbars, a function-key bar that can be hidden, and columns that adapt to the window width.
- **Languages:** English and Czech, switchable at once. Translations are TOML files, partial translations work, and plural forms are supported.
- **Portable mode:** settings live in `config.toml` next to the program, with migration from `%APPDATA%` and a fallback when the program folder isn't writable.

### Not yet

- **Archives:** encrypted (password-protected) archives.
- **Missing features:** FTP and other file-system plugins, the Find dialog, directory compare, panel view modes (brief, thumbnails) and drag and drop.
- **Text shaping:** Arabic joining and Indic conjuncts aren't shaped (an egui limitation).
- **Recycle Bin:** deleting to it may reject paths over 260 characters (permanent delete works).
- **Network:** an unreachable network share blocks the window until Windows times out.
