# RustySalamander user manual

RustySalamander is a modern two-panel file manager for Windows 10 and 11 (it also runs on Windows 8.1 and Windows Server 2012 R2), inspired by the long-proven efficiency and functionality of Altap Salamander. It handles file names in every language. This manual explains how to use it: first the everyday tasks, then every setting, menu and key.

Version 0.8.4-beta.

## Contents

1. [Getting started](#1-getting-started)
2. [The main window](#2-the-main-window)
3. [Moving around](#3-moving-around)
4. [Selecting files](#4-selecting-files)
5. [Working with files](#5-working-with-files)
6. [Copying, moving and deleting: the operations window](#6-copying-moving-and-deleting-the-operations-window)
7. [The context menu and the clipboard](#7-the-context-menu-and-the-clipboard)
8. [Folder sizes](#8-folder-sizes)
9. [Viewing and editing files](#9-viewing-and-editing-files)
10. [Archives (ZIP, 7z, RAR, …)](#10-archives-zip-7z-rar-)
11. [Servers: SCP, SFTP and FTP](#11-servers-scp-sftp-and-ftp)
12. [Finding files](#12-finding-files)
13. [Comparing files and folders](#13-comparing-files-and-folders)
14. [Plugins](#14-plugins)
15. [Updates](#15-updates)
16. [Settings reference](#16-settings-reference)
17. [Menu reference](#17-menu-reference)
18. [Keyboard reference](#18-keyboard-reference)
19. [Where things are kept](#19-where-things-are-kept)
20. [Tips and troubleshooting](#20-tips-and-troubleshooting)

---

## 1. Getting started

### Installing

RustySalamander is portable: nothing is installed.

1. Download `RustySalamander-<version>-win64.zip` from the project's GitHub releases page.
2. Unzip it into any folder, for example `D:\Applications\RustySalamander\0.8.0-beta`, or onto a USB stick.
3. Run `rustysalamander.exe`.

Settings are saved in `config.toml` next to the program when you exit. To move your settings to another copy, copy that file. Nothing is written outside the program's folder, unless you switch to multi-user mode (see [Where things are kept](#19-where-things-are-kept)).

You can also give the folders the two panels should open with on the command line:

```
rustysalamander.exe "C:\Projects" "D:\Backup"
```

### The idea of two panels

The window shows two folders side by side. One panel is **active**: its path bar is highlighted and the cursor bar is shown in it. Most commands work on the active panel, and copy and move send things **to the other panel's folder** by default. Press **Tab** to switch panels, or click anywhere in the other one (also below the last item).

### Your first steps

| To… | Do this |
|---|---|
| Open a folder or a file | Double-click it, or press **Enter** |
| Go up one folder | **Backspace**, or click `..` at the top of the list |
| Switch panels | **Tab** |
| Change drive | Click the drive button above the panel, or **Alt+F1** (left) / **Alt+F2** (right) |
| Copy to the other panel | Select files, then **F5** |
| Move to the other panel | **F6** |
| Delete | **F8** or **Delete** |
| Find a file in the list | Just start typing its name |
| See all keys | **F1** |

---

## 2. The main window

From top to bottom:

- **Menu bar:** Files, Directory, Commands, View, Plugins, Options, Help (see [Menu reference](#17-menu-reference)).
- **Drive buttons and path bar** above each panel. The drive button shows the panel's drive; click it to pick another. The path bar shows where the panel is, as clickable parts: `C: › Users › Documents`.
- **The file list** with the columns **Name**, **Ext**, **Size**, **Date** and **Attr**. The first row, `..`, leads to the parent folder.
- **The footer** under each list: what is selected (*Selected: 12.4 MB in 3 files*) or what is under the cursor, and the free space of the drive (*120 GB free of 476 GB*).
- **The status bar:** messages from the last command, and what copy, move and delete operations are running (*Copying 2 · Deleting 1*, with progress and time left). Nothing running says *No operations running*.
- **The function key bar:** buttons for the most used commands (F1 Help, F2 Rename, … F10 Exit). You can choose which commands it shows (see [Keyboard shortcuts](#keyboard-shortcuts)).

### The columns

| Column | Shows |
|---|---|
| Name | The name, without the extension for files, with the file's icon |
| Ext | The extension (`txt`, `jpg`) |
| Size | A file's size; `DIR` for a folder, or its size once calculated (see [Folder sizes](#8-folder-sizes)) |
| Date | When it was last changed |
| Attr | The attributes: **R** read-only, **H** hidden, **S** system, **A** archive, **C** compressed, **E** encrypted |

**Sorting:** click a column header to sort by it, click again for the reverse order. A small arrow shows which column sorts and which way: pointing up for ascending (A to Z, smallest or oldest first), pointing down for descending. Ctrl+F3 to Ctrl+F6 sort by name, extension, time and size. Folders are always listed before files. Names sort in the alphabetical order of your Windows language, and numbers in names by value (`file2` before `file10`).

**Column widths:** drag the edge to the left of **Ext**, **Size** or **Date** in the header row to make that column wider or narrower. **Name** always takes the rest of the width. **Attr** is just as wide as its letters.

**Hidden files** have faded icons. View → *Show hidden files* (Ctrl+H) and *Show system files* hide them.

### Panels that a plugin shows

Inside an archive, on a server, in search results or in a comparison, the panel has a tinted background and a banner under the list naming the plugin. You work there as in any folder. Items can be dragged out of the window from all of them (see [Dragging out of the window](#dragging-out-of-the-window)); in search results and comparisons the clipboard, the context menu and Properties work with the files on disk too (see [The clipboard](#the-clipboard)).

---

## 3. Moving around

### The cursor and opening things

- **Arrow keys, Page Up/Down, Home, End** move the cursor.
- **Enter** or a double-click opens: a folder opens in the panel, an archive opens as a folder, any other file opens with its Windows program.
- **Backspace** or **Ctrl+Page Up** goes to the parent folder; the cursor lands on the folder you came from.
- **Ctrl+Page Down** enters the folder under the cursor.
- **Ctrl+Backspace** or **Ctrl+\\** goes to the root of the drive.
- **Alt+←** and **Alt+→** go back and forward in the panel's history.

### Drives

- **Drive button / drive selector:** click the drive button above a panel, or press **Alt+F1** (left panel) / **Alt+F2** (right panel). The list shows local drives, then network drives, then open server sessions, searches and comparisons, then *Recent paths* you typed. The last row, **From right panel** (⏪, in the left panel) or **From left panel** (⏩, in the right panel), takes the panel to the folder the other panel shows. Each drive has its icon: blue for local disks, green for network drives, orange for USB sticks, purple for CD/DVD.
- **Letter keys:** **Shift+C** goes to drive C:, **Shift+D** to D:, and so on. The modifier can be Shift, Alt, Ctrl+Shift or none (Settings → Keyboard shortcuts → *Go to drive*).
- **Back where you were:** changing to a drive returns to the folder you last had open on it (Settings → General → Drives).
- **All drives at once:** Settings → General → Drives → *Drive buttons* → *All drives* shows a button for every drive above each panel. Next to them come drop-down lists of server sessions and searches (with Disconnect, Refresh and Close buttons) and a button for the comparison. The last button, ⏪ above the left panel or ⏩ above the right one, takes the panel to the folder the other panel shows.

### The path bar

- Click any part of the path to go there.
- Click the end of the path bar, or press **Shift+F7**, to type a path. Enter goes there, Esc cancels. You can type:

| Typed | Goes to |
|---|---|
| `C:\Users\Docs` or `C:/Users/Docs` | that folder (quotes are fine) |
| `..`, `sub\folder`, `..\other` | relative to the current folder |
| `C:\dir\file.txt` | `C:\dir`, with the cursor on `file.txt` |
| `D:` | drive D:, in the folder last used there |
| `%USERPROFILE%\Desktop`, `~\Downloads` | environment variables and your home folder |
| `\\server\share\folder` | a network share |
| `\\server` | the list of the server's shares |

Paths you type are remembered under *Recent paths* in the drive selector (up to 12).

### Hot paths

Ten quick bookmarks. **Ctrl+Shift+1** remembers the active panel's folder as hot path 1; **Ctrl+1** goes back to it later. The same works for 0–9.

### Quick search

Just start typing in a panel: the cursor jumps to the first name that starts with what you typed, and every matching item is highlighted.

- `/` or `\` stand for any characters (`re/port` finds `report` and `red export`).
- **↑ / ↓** jump to the previous / next match (or **Ctrl+↑ / Ctrl+↓**). With Options → Quick search → *Arrows jump between matches* turned off, the arrows and other cursor keys move the cursor as usual and the search stays highlighted.
- **Backspace** takes back the last letter typed.
- **Esc** ends the search.
- Options → Quick search → *Anywhere in name* finds the text anywhere in the name, not just at the start.
- Letters with and without diacritics match (default): `vladce` finds `vládce` and `vládče`, `grusse` finds `Grüße`, `o` finds `ø`. Options → Quick search → *Exact letters* makes `á` match only `á` (upper or lower case still doesn't matter).

---

## 4. Selecting files

Commands work on the **selected** items, or on the item under the cursor when nothing is selected. Selected items are shown in red.

| To… | Do this |
|---|---|
| Select or unselect one item | **Insert** or **Space** (moves down), or **Ctrl+click** |
| Select a range | **Shift+↑/↓**, or **Shift+click** |
| Select by mask | **+**, then a mask such as `*.jpg;*.png` |
| Unselect by mask | **-** |
| Invert the selection | **\*** |
| Select all / unselect all | **Ctrl+A** / **Ctrl+D** |
| Select a folder and calculate its size | **Shift+Space** |

**Masks:** `*` is any characters, `?` one character, `;` separates masks, and masks after `|` are left out: `*.h*|*.html` means "files whose extension starts with h, but not .html". Masks don't care about case, in any language.

---

## 5. Working with files

| Command | Key | What it does |
|---|---|---|
| View | F3 | Opens the file in the viewer (see [Viewing and editing](#9-viewing-and-editing-files)) |
| Edit | F4 | Opens the file in the editor (Notepad by default) |
| Copy | F5 | Copies to the other panel's folder |
| Move | F6 | Moves to the other panel's folder |
| Rename | F2 or Shift+F6 | Renames the item under the cursor |
| Create directory | F7 | Creates a folder |
| Delete | F8 or Delete | Deletes to the Recycle Bin |
| Delete permanently | Shift+F8 or Shift+Delete | Deletes without the Recycle Bin |
| Copy full paths | Ctrl+Shift+C | Puts the full paths of the items on the clipboard as text |
| Properties | Alt+Enter | The Windows Properties window |
| Open command prompt here | Shift+F1 | A terminal in the panel's folder |

### Copy and move (F5, F6)

The dialog shows what is copied and the target, which is the other panel's folder. The target can be:

- **a folder:** the items go into it;
- **a new name:** a single item is copied or moved under that name;
- **a mask** such as `*.bak`: each item gets a name made by the mask (`report.txt` → `report.bak`).

The buttons decide when it runs:

- **Copy** (or **Move**) starts at once, alongside anything already running.
- **Run first** pauses what is running, runs this one, then lets the others go on.
- **Queue last** waits until everything running or waiting has finished.

Options below the target. The dialog opens with the defaults from Settings → General → Files (*Copying (F5)*, *Moving (F6)*); what you change in the dialog is for this operation only and isn't remembered.

- **Run in background:** the operation starts without its progress window (the status bar shows the progress; click it to see the window). If it needs an answer (a file exists already, an error), its progress window comes back together with the question, and goes away again once you have answered.
- **Show the result when finished:** when the operation ends, the *Operation result* window tells how it went: what was copied or moved, how long it took and how fast, how the copies were checked, and every file that failed or was skipped, with the reason. With a safe copy or move it is always shown (the tick is fixed on).
- **Safe copy / Safe move** checks each copy right after it is written:
  - **Off:** no check.
  - **Compare sizes:** the copy must have as many bytes as the original.
  - **SHA-256 checksums:** the copy is read back and its SHA-256 checksum compared with the original's. This takes about as long again as the copy. The checksums of all files are also written to a file named `RustySalamander <date> <time>.sha256`, one in the target folder and one in the source folder, in the format `sha256sum -c` checks (`<checksum> *<path>`).

  A copy that fails the check is removed and you are asked to **Retry**, **Skip** or **Cancel**. A safe move deletes an original only after its copy passed the check; the checksum file in the source folder stays as a record. A move within the same drive only renames, so no data is copied: its files are listed in the checksum files but there is nothing to compare. A safe copy always shows the *Operation result* window when it ends. Inside archives and on servers (plugins) nothing is checked.

### Drag and drop

Drag a file or folder from a panel onto the other panel, or onto a folder's row (`..` included) in either panel. Dragging a selected item takes the whole selection. While you drag, the target folder is outlined and a label at the pointer says what will happen: *Move 3 items to Photos* or *Copy …*. Esc cancels the drag.

- **On the same drive** the items are **moved**, onto **another drive** (or into an archive, or to a server) **copied**, as in Explorer.
- **Shift** moves and **Ctrl** copies, whatever the drives. Hold the key when you release the button.
- The **Copy or Move dialog** opens with the target filled in, so you can check it, pick a safe copy or run it in the background. Settings → General → Files → *Drag and drop* can start the operation at once instead.
- **Out of the window:** drag past the window's edge and the items go to Explorer, the desktop or any program that takes files. See [Dragging out of the window](#dragging-out-of-the-window).
- **Where nothing can be dropped:** search results, and archives that can't be changed (such as RAR), take no dropped items.
- **Into the window:** files dragged from Explorer or another program can be dropped on a panel or a folder's row, the same way. RustySalamander does the copy or move itself, with its own dialog and progress window.

### Dragging out of the window

As soon as the pointer leaves the window with the button held, the drag becomes a normal Windows drag, showing the item's icon. Drop the items on Explorer, the desktop, a mail or chat program, an upload field or any other program that takes files.

- **Files in a folder** go out as they are. The program you drop them on copies or moves them by its own rule (in Explorer: moved on the same drive, copied onto another one; Shift moves, Ctrl copies).
- **Search results and compared folders** go out as the files on disk they stand for, even when they come from many folders. A comparison's grey placeholders (what only the other side has) stay behind.
- **Items in an archive or on a server** go out as files on disk too, and are always **copied**, never moved:
  - Nothing is extracted or downloaded when you start dragging, so the drag starts at once even for gigabytes.
  - When you drop them, the program they are dropped on copies them as ordinary files, and only what it copies is read from the archive or server, as it copies it. Explorer shows its own progress window, asks about files that already exist and can cancel.
  - A folder comes with everything inside it.
  - The files are shown to other programs in a folder of RustySalamander's under `%TEMP%\RustySalamander`. What was read stays there, so dragging it again is instant, until RustySalamander ends and removes the folder. Don't close RustySalamander while a copy from an archive or server is still running.
  - Names Windows doesn't allow (with `:` or `?`, say, from Linux) are left out; copy those with F5.

**Archives and servers need the Windows feature *Windows Projected File System*.** It comes with Windows 10 and 11 but is off unless you turn it on, once:

1. Open **Turn Windows features on or off** (Start, type *Windows features*, or run `optionalfeatures.exe`).
2. Tick **Windows Projected File System** and click OK. Restart Windows if it asks.

Or run this in PowerShell as administrator: `Enable-WindowsOptionalFeature -Online -FeatureName Client-ProjFS`

While the feature is off, dragging items of an archive or server out of the window shows a message saying so; nothing else happens. Copying them with F5, or dragging them to the other panel, works without it.

### Create directory (F7)

Type a name. You can create several levels at once: `photos/2026/summer` creates all three. Folders that already exist are reused.

### Delete (F8)

Asks for confirmation (unless Options → *Confirm delete* is off) and moves the items to the Recycle Bin (unless Options → *Delete to Recycle Bin* is off). **Shift+F8** or **Shift+Delete** deletes permanently.

---

## 6. Copying, moving and deleting: the operations window

Copy, move and delete run in the background. You can keep working in the panels while they run.

**The *File operations* window** shows each running operation with two progress bars (data and files), the current file, the speed, the time spent and the time left. Each operation has **Pause / Resume** and **Cancel**.

**Waiting operations** are listed below: **⏶ / ⏷** change their order (⏶ on the first one runs it at once and pauses the running ones until it finishes), **Start now** runs one at once, **×** removes it.

**At the bottom:** *Pause all* / *Resume all*, *Cancel all* (also empties the queue) and *Run in background*.

**In the background:** *Run in background*, Esc or the window's close button hides the window; the operations go on and the status bar shows their progress. Click the status bar to bring the window back.

**When a file already exists** at the target, a window shows both side by side (name, folder, size, date; the larger and the newer one marked) and asks what to do: **Overwrite**, **Overwrite All**, **Skip**, **Skip All** or **Cancel**. Enter skips (nothing is overwritten by accident).

**When something fails** (access denied, file in use, a copy that fails its safe-copy check), you get **Retry**, **Skip**, **Skip All** or **Cancel**; Enter retries. These questions appear also while the window is hidden: the window comes back with the question, and goes away again once you have answered.

**The *Operation result* window** opens when an operation started with *Show the result when finished*, or a safe copy or move, ends (see [Copy and move](#copy-and-move-f5-f6)). It is modal: the main window waits until you close it (operations that are still running go on). Results of several operations are listed in it, the newest first; **Close** or Esc closes it. **Save protocol…** saves them to a text file you choose (it starts in the target folder): for each operation what was copied or moved from where to where, when it ended, how it went, how long it took, how the copies were checked, every problem, and, after a check with checksums, every file with its SHA-256 checksum. The window stays open, so you can still remove the checksum files after saving. After a safe copy with checksums, **Close and remove checksum files** also deletes the `.sha256` files it lists, in the target and the source folders (when you only wanted the check, not the record). Like every window of the program, it opens without Windows' animation when View → *Animations* is off, also when it opens while you work in another program.

---

## 7. The context menu and the clipboard

### The context menu

**Right-click** an item for its menu. With items selected, the menu is for all selected items. Right-clicking the empty space below the list gives the menu of the folder itself. **Shift+F10** opens the menu from the keyboard.

There are three styles (Settings → General → Context menu → *Style*):

- **Windows 11** (default): a short menu with Cut, Copy, Paste, Rename, Delete, Open, Open with…, Calculate size, Copy as path, Open in Terminal and Properties. **Show more options** opens the full Windows menu.
- **Windows 11, full:** the same commands, and below them everything the full Windows menu has (Send to, 7-Zip, antivirus, cloud storage and other programs' items) in one menu.
- **Classic:** the full Windows menu as in Windows 10, with every item. Hold **Shift** while opening it for extra items (such as *Copy as path* on older Windows).

The full menus can take a moment to open: Windows asks every installed program that adds menu items. RustySalamander loads them in the background when it starts, so the first menu is quicker.

If you'd rather select with the right button (as in Norton Commander and Salamander), choose *Long right-click* under *Opens on*: a short right-click then selects, and holding the button opens the menu.

### The clipboard

- **Ctrl+C** copies the selected items (or the one under the cursor) to the Windows clipboard.
- **Ctrl+X** cuts them (they are moved when pasted).
- **Ctrl+V** pastes files from the clipboard into the panel's folder.

This is the Windows clipboard, so you can copy in RustySalamander and paste in Explorer or any other program, and the other way round. It isn't available inside archives or on servers; use F5 and F6 there, or drag the items out of the window.

**Search results and comparisons** (panels a plugin fills with files that are on disk) put those files themselves on the clipboard: Ctrl+C, Ctrl+X, the context menu (right-click, Shift+F10), Properties, Copy full paths and dragging out of the window all work with the real files, even when they come from many folders. Pasting into such a panel isn't possible, and the context menu has no Rename, Paste or Calculate size there.

---

## 8. Folder sizes

Folders show `DIR` until their size is calculated.

- **Shift+Space** selects the folder under the cursor and calculates its size in the background. The size counts up in italics while it is calculated.
- **Alt+F10** (Files → *Calculate selection size*) calculates every selected folder, or the folder under the cursor, and the status bar then shows the total: size, bytes, files and subfolders.
- Hover over a size for the number of files and folders inside.
- A `*` after a size means some subfolders couldn't be read (usually access denied), so the folder may be bigger.
- Sorting by size puts sized folders in their place.
- Sizes are kept until you refresh (**Ctrl+R**) or a copy, move or delete finishes.

Prefer the Salamander way, where plain Space does this? In Settings → Keyboard shortcuts, choose the preset `Space` for *Select + calculate folder size*.

---

## 9. Viewing and editing files

### The viewer (F3)

F3 opens the file in RustySalamander's own viewer, in a window of its own (the Viewer plugin):

- **Text** or **Hex** (F4 switches).
- **Wrap** long lines (Ctrl+W).
- **Encoding:** detected automatically (UTF-8, UTF-16, your Windows code page); the list lets you switch to Windows-125x, ISO-8859-2, KOI8-R, CP866, Shift-JIS, EUC-JP, GBK, Big5 or EUC-KR.
- **Coloring:** source code and other text formats (about 200: JSON, XML, HTML, PowerShell, C, Python, …) are colored by their format, recognized from the name or the first line. Pick another one or *Plain text* in the list.
- **Find** (Ctrl+F, or the magnifier in the toolbar): type what you look for. Every match is marked as you type, the current one in orange, and *3 of 12* says where you are. **Enter** or **F3** goes to the next match, **Shift+Enter** or **Shift+F3** to the previous one, round the end of the file.
  - **Match case:** upper and lower case must be the same. Without it, case doesn't matter, in any language (`ŽLUŤ` finds `žluť`).
  - **Wildcards:** `*` stands for any characters and `?` for one, within a line: `file*.txt` finds `file1.txt` and `file22.txt`, `a?b` finds `axb`. Without it, `*` and `?` are found as they are.
  - The search is in the text; a match found while the Hex view is shown is shown in the Text view.
  - **Esc** closes the find bar; a second Esc closes the window.
- **Esc** or **F10** closes the window. Several viewer windows can be open at once.

### The editor (F4)

F4 opens the file in Notepad by default.

### Choosing your own

In Settings → **Viewers and editors** you can pick any program for F3 and F4, and give file types their own: for example `.md` files in Typora, `.ini` in Notepad (see [Viewers and editors settings](#viewers-and-editors)).

---

## 10. Archives (ZIP, 7z, RAR, …)

Archives open like folders: press **Enter** on `photos.zip` and browse what is inside. `..` at the top leaves the archive.

- **Reading** works for zip, 7z, rar, tar, gz/tgz, bz2, xz, zst, lzma, Z, cab, arj, lzh, iso, wim, cpio, rpm, deb and split archives (.001).
- **Changing** (adding, deleting, renaming, new folders) works for 7z, zip, tar and wim archives.
- **F3, F4, Enter** on a file inside open a temporary copy. Changes to that copy don't go back into the archive.
- **F5 / F6** copy or move items out of an archive into the other panel, or into an archive shown in the other panel.
- **Drag and drop:** drag items onto the other panel to copy them out, or files onto the archive's panel to add them. Dragged out of the window, they go to Explorer or any program, read from the archive only as that program copies them (this needs *Windows Projected File System*; see [Dragging out of the window](#dragging-out-of-the-window)). Archives that can't be changed, such as RAR, take no dropped files.
- **F8** deletes from the archive, **F7** creates a folder in it, **F2** renames.
- Folder sizes inside an archive are shown at once.

Changing a large archive takes a while, because the whole archive is written again. If it fails or you cancel, the archive stays as it was. Password-protected archives aren't supported yet. The context menu, Properties and the clipboard aren't available inside archives.

---

## 11. Servers: SCP, SFTP and FTP

Files on servers open in a panel through two plugins: **SCP** for Linux, BSD and macOS servers over SSH, and **FTP / SFTP** for FTP, FTPS and SFTP servers. Once connected, both work the same way.

### SCP

1. **Plugins → Connect to server…**
2. Fill in the server, the port (22), the user name, and either a password or a private key file (with its passphrase if it has one).
3. The server's files open in the active panel, in your home folder: `scp://user@server/home/user`.

You can then copy and move files in and out (F5, F6), delete (F8), rename (F2), create folders (F7), view and edit (F3, F4), with the usual progress window. Drag and drop works as with archives: onto the other panel, from Explorer onto the server's panel, and out of the window to Explorer or another program, which then downloads only what it copies (see [Dragging out of the window](#dragging-out-of-the-window)).

- **Server keys:** the first time you connect, the server's key fingerprint is shown for you to confirm; it is remembered after that. If a server's key changes, the connection is refused with a warning. Keys you already trust in `%USERPROFILE%\.ssh\known_hosts` are accepted too.
- **Remembered:** the last server, port, user and key file, never the password.
- **Getting back to a server:** the drive selector lists open sessions under *Connections*.
- **Disconnecting:** Plugins → *Disconnect from …*, the **Disconnect** button in the drive selector (or Delete on it), or quitting the program. Panels showing the server go back to where they were.
- Ctrl+R refreshes a server's folder; it doesn't refresh on its own.

### FTP, FTPS and SFTP

1. **Plugins → Connect to FTP or SFTP server…**
2. Choose the protocol:
   - **SFTP (SSH):** file transfer over SSH, which nearly every Linux, BSD and macOS server offers, and many Windows servers too. Sign in with a password or a private key file.
   - **FTPS (FTP with TLS):** FTP encrypted with TLS (explicit, `AUTH TLS` on the usual port 21).
   - **FTP (unencrypted):** plain FTP. Everything goes over the network readable, the password too, so use it only where that doesn't matter.
3. Fill in the server, the user name and the password. Leave the port empty for the protocol's usual one (22 for SFTP, 21 for FTP and FTPS). `user@server:port` typed as the server works too. With FTP, an empty user name signs in as *anonymous*.
   - **Data connection** (FTP and FTPS only): how files and listings travel. **Automatic** tries passive mode first and active mode when passive doesn't get through; **Passive** (the computer connects to the server, which works through most firewalls and home routers) and **Active** (the server connects back to the computer) use only that one.
   - **Sign in with** (SFTP only): the password, or a private key file with its passphrase. FTP and FTPS always sign in with the password, so the choice is greyed out for them.
4. The server's files open in the active panel, in the folder the server starts you in: `sftp://user@server/home/user`, `ftps://…` or `ftp://…`.

Copying and moving in and out, deleting, renaming, new folders, viewing, editing and drag and drop work as on SCP servers, with the same progress window. Files keep their times both ways where the server allows it (SFTP always; FTP servers that offer `MFMT`).

- **SFTP server keys:** checked as with SCP. Keys you trusted in the SCP plugin or in `%USERPROFILE%\.ssh\known_hosts` are trusted here too.
- **FTPS certificates:** a certificate a public certification authority vouches for, for that server's name, is accepted at once. Any other (self-signed, issued for another name, expired) is shown with its SHA-256 fingerprint for you to confirm, and is remembered after that. If a server's certificate later changes to another one no authority vouches for, the connection is refused with a warning.
- **Data connections, in detail:** passive mode asks with EPSV first and with PASV when the server doesn't know EPSV; when PASV names an address other than the server's (a server behind NAT), the server's own address is used. The first way that works is used for the rest of the session. Active mode needs this computer to accept the server's connection: Windows Firewall may ask once whether RustySalamander may do that.
- **Several transfers at once:** an FTP connection does one thing at a time, so the plugin opens up to three to a server: you can browse while a copy runs. A connection the server closed while idle is opened again without asking.
- **Remembered:** the last protocol, server, port, user, key file and data connection mode, never the password.

---

## 12. Finding files

**Plugins → Find files…** or **Alt+F7** opens the search form:

- **Names:** masks such as `*.txt;*.md`, with masks to leave out after `|` (`*.log|old*`), or a regular expression.
- **Look in:** the folder to search, the active panel's folder to start with, with or without subfolders. Hidden and system items can be left out.
- **Containing text:** optional. Found in any case unless *Match case* is on, in UTF-8, UTF-16 and ANSI files; or a regular expression.
- **Find folders too:** folders whose names match are results as well (on to start with).

The **results window** fills while the search runs. Columns sort with a click.

- **Open** (Enter, double-click) opens a file with its program, or a folder in the active panel. **F3** views and **F4** edits the selected file.
- **Go to file** shows the file in its folder in the active panel.
- **Show filtered folders** shows the searched folder in the panel, with only what was found in its folders.
- **Show all in one list** shows everything found in one flat list in the panel.
- **Stop** ends the search, **New search…** opens the form again, **Refresh** searches again with the same settings, **Close** closes the window.

In both panel views you can copy (F5) the results elsewhere, delete them (F8, to the Recycle Bin), rename them (F2), and view or edit them. Ctrl+C, Ctrl+X, the context menu, Properties, *Copy full paths* and dragging out of the window work with the found files themselves (see [The clipboard](#the-clipboard)). Nothing can be dropped or pasted into results. Results shown in a panel stay available in the drive selector under *Searches*, with **Refresh** and **Close**.

---

## 13. Comparing files and folders

**Plugins → Compare** compares what you chose:

- two items selected in the active panel; or
- the selected item in each panel; or
- the items under the two cursors; or
- the two panels' folders.

**Compare files…** and **Compare folders…** ask for the two paths instead.

**Two files** open side by side in a window, lined up line by line: changed lines amber, lines only in the left file red, only in the right file green. **Next** / **Previous** (F8 / Shift+F8, or Alt+↓ / Alt+↑) go between differences. **Ignore spaces** ignores differences only in spaces and tabs; **Compare again** rereads the files. Binary files and files over 32 MB show their sizes and where they first differ.

**Two folders** open in both panels at once, the lists lined up:

- **amber:** different (for a folder: something inside differs);
- **green:** only this side has it;
- **grey, no size:** only the other side has it;
- **no color:** the same.

Entering a folder or going up in one panel takes the other along. F5 from one side copies to the other side, F8 deletes, F7 creates a folder, and the colors follow. The clipboard, the context menu, Properties and dragging out of the window work with the compared files themselves. The folder form asks how files count as the same: by size and date (fast), or by their contents (slow). The comparison is listed in the drive selector under *Comparisons*, with **Refresh** to compare again; closing either side closes both.

---

## 14. Plugins

Archives, servers, search, the viewer and comparison are plugins. They are files in the `plugins` folder next to the program, each in its own folder.

- **Settings → Plugins** lists them, with what each does, its version and its state. Untick one to turn it off; it applies at once.
- **The Plugins menu** has the commands of the plugins that are on, the open connections and searches (to disconnect or close them), and *Plugin settings…*.
- **Installing a plugin:** copy its folder into `plugins` and restart. **Removing:** delete its folder.
- Plugins get updates of their own (see [Updates](#15-updates)).

---

## 15. Updates

When the program starts, it checks GitHub for newer versions of itself and of its plugins. If there are any, a window lists them, each with **Show changelog** to see what changed first.

- **Update now** for the program downloads the new version into a new folder next to the current one (for example `…\RustySalamander\0.8.0-beta`) and copies your settings there. **Start the new version** opens it. The old version stays, so you can go back.
- **Update now** for a plugin downloads it; it is installed at the next start (**Restart now** does it at once).
- **Update all** installs every available update.
- An update made for a different program version is shown greyed, with the reason.
- **New plugins:** official plugins you don't have yet are offered in the same window, with what they do. **Install** downloads one; it is added at the next start (**Restart now** does it at once). **Don't offer** stops offering it when the program starts; Help → Check for updates still shows it. **Update all** doesn't install new plugins: only **Install** does.

**Help → Check for updates…** checks at any time. Settings → General → Updates turns the check at start-up off.

---

## 16. Settings reference

Open Settings with **Options → Settings…**. While it is open, the main window waits. Every change applies at once and is saved when you exit the program. Settings has four tabs.

### General

#### Appearance

| Setting | Choices | Default | What it does |
|---|---|---|---|
| Language | System default, or any installed language (English, Čeština) | System default | The language of the whole program. *System default* follows Windows and falls back to English. |
| Theme | System default, Light, Dark | System default | Light or dark look. *System default* follows Windows. |
| Accent color | for the light and the dark theme each: *From the theme (default)* or *Custom:* with a color picker | From the theme | The color of what is selected or active: the cursor bar, the active path bar, selected tabs and buttons. Panels shown by a plugin are tinted with it. A preview shows a selected row. |
| Font | any installed font, with a search box | Segoe UI | The font of the whole program. Characters the font lacks (Chinese, Arabic, emoji, …) still show, from other fonts. |
| Font size | 10–24 pt | 14 pt | The size of all text. |
| Viewer font | any installed font | Consolas | The font of the viewer's text and hex view. |
| File icons | *Show Windows file icons* | on | The real Windows icon of each file type. Off: simple drawn icons (faster on slow network drives). |
| Misc | *Always show scrollbars* | off | Solid scrollbars that are always visible, instead of thin ones that appear when the mouse is over the list. |
| | *Animations* | on | Fading windows, menus and tooltips, and smooth scrolling. Off: everything appears and moves at once, also in the viewer and compare windows. |
| | *Show function key bar* | on | The buttons at the bottom of the window. |

#### Files

| Setting | Default | What it does |
|---|---|---|
| Visibility → *Show hidden files* | on | Lists items with the hidden attribute (shown faded). Also View menu, Ctrl+H. |
| Visibility → *Show system files* | on | Lists protected system items. An item that is both hidden and system needs both on. |
| Deleting → *Delete to Recycle Bin* | on | Off: F8 and Delete delete permanently. |
| Deleting → *Confirm delete* | on | Asks before deleting. |
| Drag and drop → *Confirm copying and moving by drag and drop* | on | Dropped items open the Copy or Move dialog first; off, the operation starts at once. |
| *Copying (F5)*, *Moving (F6)* → *Run in background* | off | What the Copy and the Move dialog offer when they open: start without the progress window. A change in the dialog is for that operation only. |
| *Copying (F5)*, *Moving (F6)* → *Show the result when finished* | off | Show the *Operation result* window at the end. Always on with a safe copy or move. |
| *Copying (F5)*, *Moving (F6)* → *Safe copy* / *Safe move* | Off | What the dialogs offer: *Off*, *Compare sizes* or *SHA-256 checksums* (see [Copy and move](#copy-and-move-f5-f6)). |

#### Quick search

| Setting | Choices | Default | What it does |
|---|---|---|---|
| Match | From first letter, Anywhere in name | From first letter | Whether typed text must start the name or may be anywhere in it. |
| Letters | *Letters with or without diacritics*, *Exact letters* | with or without diacritics | With or without diacritics, accents and other marks don't matter: `vladce`, `vládce` and `vládče` find each other, and letters like `ø`, `ł`, `ß`, `æ` match `o`, `l`, `ss`, `ae`. Exact: only the same letters match. |
| Arrow keys | *Arrows jump between matches* | on | While a quick search is active, ↑/↓ go to the previous/next match, and other cursor keys (Page Up/Down, Home, End) end the search. Off: all cursor keys move the cursor as usual and the search stays, matches still highlighted, until Esc; Ctrl+↑/↓ still jump between matches. |

#### Drives

| Setting | Choices | Default | What it does |
|---|---|---|---|
| Drive buttons | Drive selector, All drives | Drive selector | One button for the panel's drive that opens the drive list, or a button for every drive (plus lists of server sessions and searches). |
| Changing drive | *Return to the last directory on that drive* | on | Changing drive opens the folder you last had open there, not its root. |
| Typed paths | *Remember typed paths in the drive selector* | on | Paths you type into the path bar are offered under *Recent paths* (up to 12). Turning it off forgets them. |

#### Context menu

| Setting | Choices | Default | What it does |
|---|---|---|---|
| Opens on | *Right-click*; *Long right-click: hold, then release (a short right-click selects)*, with a *hold time* of 250–1500 ms | Right-click, 500 ms | How the right mouse button opens the menu. |
| Style | *Windows 11*, *Windows 11, full*, *Classic system menu* | Windows 11 | Which menu opens (see [The context menu](#the-context-menu)). |

#### External programs

| Setting | Choices | Default | What it does |
|---|---|---|---|
| Command prompt | *Default terminal*, *Command Prompt (cmd)*, *PowerShell*, *Custom:* with a command and **Browse…** | Default terminal | What *Open command prompt here* (Shift+F1) starts. *Default terminal* is the one Windows uses (usually Windows Terminal). *PowerShell* is PowerShell 7 if installed. In a custom command, `{dir}` is replaced by the panel's folder, e.g. `alacritty.exe --working-directory "{dir}"`. |

#### Updates

| Setting | Default | What it does |
|---|---|---|
| New versions → *Look for a new version on GitHub at start-up* | on | Checks for updates of the program and its plugins when it starts. **Check now** checks at once. |

### Keyboard shortcuts

A table of every command that has a shortcut, in three groups: *File & folder operations*, *Navigation* and *System & view* (the *Jump to* links at the top go to them). For each command:

- **The tick box** before its name puts it on the **function key bar**, as a button showing the command's first shortcut and its name (*F5 Copy*, *Alt+F7 Find files*). The buttons are in key order: F1…F12 first, then other keys, then commands without one. Hover over a button for its full name.
- **Preset:** quick choices: *Default* (Salamander's key), one key of the default set only, alternatives (such as `Ctrl+L` for *Change directory*), or *None (disabled)*. It says *Custom* once you change the keys yourself.
- **Shortcuts:** the current keys. Click one to remove it. **+ Add…** records a new one: press the keys (Esc cancels).
- A key used by two commands is shown in red; hover over it to see the other command.

Also on this tab:

- **Go to drive (A–Z):** the modifier for drive letters: *Shift + letter* (default), *Alt + letter*, *Ctrl+Shift + letter* or *Disabled*. With Shift, a letter still goes to a quick search in progress, and when no drive has that letter.
- **Fixed** keys of each group, which can't be changed, are listed under it.
- **Reset all shortcuts to defaults** and **Reset function key bar**.

Your own keys take precedence over fixed keys.

### Viewers and editors

| Setting | Choices | Default |
|---|---|---|
| Viewer (F3) | any viewer a plugin offers (*Viewer*), *Notepad*, or *Program:* with your own command line and **Browse…** | the Viewer plugin |
| Editor (F4) | any editor a plugin offers, *Notepad*, or *Program:* | Notepad |

In a command line, `{file}` is replaced by the file's path; without it, the path is added at the end (`code.cmd --reuse-window`).

**By file suffix:** files ending with a suffix can have a viewer and editor of their own. **Add a suffix…** asks for the suffix (`md`, `tar.gz`, with or without the dot) and a viewer and editor for it; *Default* keeps the general one. **Edit…** changes a suffix, **Remove** gives it back to the defaults. When suffixes overlap, the longer one wins (`tar.gz` before `gz`).

### Plugins

Each plugin, with a tick box to turn it on or off, what it does, its version, what it views or edits, and its state (or why it can't start, in orange). Files in the plugins folder that can't be used are listed at the bottom with the reason. Changes apply at once.

---

## 17. Menu reference

Menu items show their current keys.

**Files:** View · Edit · Copy… · Move… · Rename… · Delete… · Copy full path · Select… (+) · Unselect… (−) · Select all · Unselect all · Invert selection (\*) · Calculate selection size · Exit

**Directory:** Create directory… · Change directory… (type a path) · Parent directory · Root directory · Back · Forward · Left drive… · Right drive…

**Commands:** Open command prompt here · Swap panels · Refresh

**View:** Sort by name / extension / time / size · Theme: System default, Light, Dark · Show hidden files · Show system files · Show Windows file icons · Always show scrollbars · Animations · Show function key bar

**Plugins:** the commands of each plugin that is on (Connect to server…, Find files…, Compare, Compare files…, Compare folders…) · open connections (*Disconnect from …*), searches and comparisons (*Close …*) · Plugin settings…

**Options:** Delete to Recycle Bin · Confirm delete · Quick search: From first letter / Anywhere in name / Letters with or without diacritics / Exact letters / Arrows jump between matches · Language · Settings…

**Help:** Keyboard (the key overview, F1) · Check for updates… · About

---

## 18. Keyboard reference

These are the default keys. Everything in the first three tables can be changed in Settings → Keyboard shortcuts.

### File & folder operations

| Command | Default keys | Other preset |
|---|---|---|
| View file | F3 | |
| Edit file | F4 | |
| Copy | F5 | |
| Move | F6 | |
| Rename | F2, Shift+F6 | |
| Create directory | F7 | |
| Delete | F8, Delete | |
| Delete permanently | Shift+F8, Shift+Delete | |
| Copy full paths | Ctrl+Shift+C | |
| Copy to clipboard | Ctrl+C | Ctrl+Insert |
| Cut to clipboard | Ctrl+X | |
| Paste from clipboard | Ctrl+V | Shift+Insert |
| Select all | Ctrl+A | |
| Unselect all | Ctrl+D | |
| Select + calculate folder size | Shift+Space | Space, Ctrl+Space |
| Calculate selection size | Alt+F10 | Ctrl+Q |
| Context menu | Shift+F10 | |
| Properties | Alt+Enter | |
| Find files | Alt+F7 | Ctrl+F |

### Navigation

| Command | Default keys | Other preset |
|---|---|---|
| Change directory (type a path) | Shift+F7 | Ctrl+L |
| Parent directory | Backspace, Ctrl+Page Up | Alt+↑ |
| Enter directory | Ctrl+Page Down | |
| Root of current drive | Ctrl+Backspace, Ctrl+\\ | |
| History back / forward | Alt+← / Alt+→ | |
| Drive menu (left / right panel) | Alt+F1 / Alt+F2 | |
| Switch quick search mode | none | Ctrl+Shift+Q |
| Go to drive | Shift+letter | Alt+letter, Ctrl+Shift+letter |

### System & view

| Command | Default keys | Other preset |
|---|---|---|
| Sort by name / extension / time / size | Ctrl+F3 / Ctrl+F4 / Ctrl+F5 / Ctrl+F6 | |
| Refresh | Ctrl+R | |
| Show hidden files | Ctrl+H | |
| Show system files | none | Ctrl+Shift+H |
| Switch dark/light mode | none | Ctrl+Shift+D |
| Swap panels | Ctrl+U | |
| Open command prompt here | Shift+F1 | Ctrl+Shift+T |
| Settings | none | Ctrl+, |
| Keyboard help | F1 | |
| Exit | F10 | Alt+X |

### Fixed keys

| Keys | What they do |
|---|---|
| ↑ ↓ Page Up Page Down Home End | Move the cursor |
| Enter, double-click | Open a folder or file |
| Tab | Switch panel |
| Insert, Space, Ctrl+click | Select / unselect an item |
| Shift+↑/↓, Shift+click | Select a range |
| + / − / \* | Select by mask / unselect by mask / invert |
| typing | Quick search (Ctrl+↑/↓ next match) |
| Ctrl+0…9 / Ctrl+Shift+0…9 | Go to / set a hot path |
| click a path part / the end of the path bar | Go there / type a path |
| Esc | End a quick search, close a dialog |

### In dialogs

**Enter** presses the button tinted with the accent color (the default: *Delete* in the Delete dialog, *Copy* in Copy), **Esc** or the window's close button cancels.

### In the viewer

F4 text/hex · Ctrl+W wrap · Ctrl+F find · F3 / Shift+F3 (or Enter / Shift+Enter in the find bar) next / previous match · Esc close the find bar, then the window · F10 close.

### In the file comparison window

F8 / Shift+F8 or Alt+↓ / Alt+↑: next / previous difference.

---

## 19. Where things are kept

Everything is in the program's folder:

| File or folder | Holds |
|---|---|
| `config.toml` | All settings, keys, hot paths, remembered paths; written when you exit |
| `lang\` | Languages (`en.toml`, `cs.toml`) |
| `plugins\<name>\` | Each plugin, with its own languages and changelog |
| `plugins\scp\known_hosts` | The server keys you trusted (SCP) |
| `plugins\ftp\known_hosts`, `known_certificates` | The SFTP server keys and FTPS certificates you trusted |
| `plugins\.updates\` | Plugin updates waiting for the next start |

- If the program's folder can't be written to (for example under `Program Files`), the program tells you at start that settings won't be saved. Move it to a folder you can write to, or switch to multi-user mode.
- **Multi-user mode:** Settings → General → Settings location → **Move to %APPDATA% (multi-user mode)…** moves `config.toml` and the plugins' files above to `%APPDATA%\RustySalamander` (the plugins' in `plugins\<name>\` there) and restarts. Each Windows user then has their own settings. The choice is kept in `multiuser.toml` next to the program and is permanent. If the program's folder is protected, Windows asks for administrator rights once.
- The environment variable `RUSTYSALAMANDER_CONFIG` can name another settings file.

**Adding a language:** copy `lang\en.toml` to a new file named by the language code (`de.toml`), translate the texts on the right of each `=`, and restart. It then appears in Options → Language. Untranslated texts stay in English.

---

## 20. Tips and troubleshooting

- **A name looks odd or has `?` in Explorer:** RustySalamander shows every name correctly, including Chinese, Arabic and emoji, and names with characters Windows normally forbids (from Macs or network storage). Copy, rename and delete work on them as on any file.
- **The full context menu is slow:** a program that adds menu items is slowing it down; Explorer's menu is just as slow. The *Windows 11* style opens at once; *Show more options* opens the full menu only when you need it.
- **A network folder takes long to open:** Windows waits for the server; an unreachable server can take up to 30 seconds.
- **A folder size has a `*`:** some subfolders couldn't be read (no permission). Running the program as administrator lets it read more.
- **Archives open with Windows instead of in the panel:** the 7-Zip plugin is off or missing; see Settings → Plugins.
- **F3 says there is no viewer:** the Viewer plugin is off; turn it on in Settings → Plugins, or choose another viewer in Settings → Viewers and editors.
- **Ctrl+C / Ctrl+V do nothing in an archive or on a server:** the Windows clipboard only works with real folders; use F5 and F6 there, or drag the items out of the window.
- **Dragging out of an archive says *Windows Projected File System* is needed:** turn that Windows feature on once (see [Dragging out of the window](#dragging-out-of-the-window)). Until then, F5 and dragging to the other panel work.
- **A program refuses items dragged from an archive or server, or gets incomplete files:** RustySalamander must keep running until the copy has finished; the files are read from the archive or server while it copies.
- **Things move too much:** View → *Animations* off.
- **Over Remote Desktop:** the program draws its windows on the processor there and sends only what changed, which keeps it quick also in large windows; animations are off then. It does the same on Windows without Direct3D 12 (8.1, Server 2012 R2). The environment variable `SAL_RENDER` chooses: `gdi` always so, `wgpu` always with the graphics card.
- **Going back to an older version:** after an update the previous version's folder is still there; just run it.
- **The program doesn't start, or something else goes wrong:** run it with `-debug` and attach the log to the bug report (see below).

### Debug log

When something goes wrong, start the program as `rustysalamander.exe -debug`. It then writes a log of everything it does to `rustysalamander-debug-<date>-<time>.log` next to `rustysalamander.exe` (or in the temporary folder, when it can't write there; `-debug=<file>` chooses the file). The title bar shows *[debug log]*, and the status line where the log is. Each thing you do is numbered (`>>> #12 USER key Ctrl+C`), followed by what the program did (`#12 DO`) and what came of it (`<<< #12 RESULT`). Passwords are not written; of what you type into a dialog, only how many characters. Attach the log to the bug report.

### Not available yet

Password-protected archives, saved server sites, and other panel views (thumbnails, brief).
