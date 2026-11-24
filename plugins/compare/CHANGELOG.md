# Compare plugin: changelog

The plugin's own versions, apart from the program's. Releases are tagged `compare-v<version>` in the RustySalamander repository.

## 0.1.9-alpha (2026-11-24)

- **Line numbers can be turned off:** both texts show their line numbers from the start, as before; **Line numbers** in the toolbar or **Ctrl+L** turns them off and on for both sides together, and the texts then start at the edge. The numbers stand in a column of their own, in the colors of the panels' footer.
- **What changed inside a line:** in a line changed (amber), the words that differ from the other side's line are on a stronger amber, on both sides; a change in spaces too, unless **Ignore spaces** is on.
- **Only lines alike side by side:** a line is shown beside the line that replaced it only when the two are alike (at least half of their characters the same, in runs of three or more, spaces and upper or lower case aside; a long code or hash changed in a few places stays alike, and only the characters changed in it are marked). Lines that aren't are shown as removed (red) and added (green), each with a gap opposite, so an unrelated line no longer looks like a changed one.
- **The current difference is marked:** the difference **Next** and **Previous** (F8, Shift+F8) went to last shows its colors (amber, red, green, and the words changed in it) strong, the others faint, with a bar in the selection color at its left edge, so it stands out from the others. Going to a difference (or a match) already wholly in sight moves only the mark: the text stays still, scrolled only to one out of sight.
- **Only changes:** a box in the toolbar hides the parts that are the same, but for 3 lines around each difference. Each hidden part leaves a line of its own, **+ Same lines: 120**; a click on it shows them, and the line stays above them as **− Same lines: 120** to hide them again. A match Find goes to in a hidden part shows that part too. Turned on again, it hides everything again.
- **Choices remembered:** **Line numbers**, **Only changes** and **Ignore spaces** stay as you left them for the next window and the next session, saved in `settings.toml` in the plugin's folder.

## 0.1.8-alpha (2026-11-24)

- **Lighter on big files:** the compared rows are shared with Find instead of copied for it, and lines are compared without copying them unless **Ignore spaces** is on.

## 0.1.7-alpha (2026-11-24)

- **Find searches beside the window:** the window stays usable while big files are searched, the bar says **Searching…** until it is done (never "Not found" before), and a search for what was typed before is given up as you type on: one search at a time, the latest only, however fast you type. **Compare again** forgets the matches found in the texts before at once, so none is drawn into the new texts or gone to with F3. What is looked for takes at most 1 000 characters.
- **A match far along a long line comes into sight:** going to a match beyond the window's edge moves that side's text sideways so the match is shown (a third in from the left, or further so that a long match ends in sight); a match near the start of its line puts the text back, and so do going to a difference (F8), comparing again and closing the find bar. Before, such a match was counted but stayed out of sight.

## 0.1.6-alpha (2026-11-23)

- **Finds text:** **Ctrl+F**, or the magnifier in the toolbar, opens a find bar, as in the viewer: every match in both files is marked, the current one in orange; **F3** and **Shift+F3** (or Enter and Shift+Enter in the box, or the arrows) go to the next and the previous match, round the end, scrolling it into view. **Match case** and **Wildcards** (`*` for any characters, `?` for one) as in the viewer; Esc closes the bar, a second Esc the window. Works with RustySalamander 0.9.4-beta and later.

## 0.1.5-alpha (2026-11-07)

- **Drawn by the program:** the comparison window is shown and drawn by RustySalamander (plugin interface 9), so the plugin no longer carries a graphics library of its own and is much smaller, and the window draws wherever the program's does: also on the processor over Remote Desktop and on Windows 8.1 / Server 2012 R2. Needs RustySalamander 0.9.4-beta or later.

## 0.1.4-alpha (2026-11-05)

- **Opens on the program's monitor:** the comparison window appears over RustySalamander's window, on the same monitor (with RustySalamander 0.9.1-beta and later; before, it opened where Windows put it). Built for plugin interface 8.

## 0.1.3-alpha (2026-11-03)

- **The program's spacing:** the toolbar has room around its buttons and the same margins and control height as the Viewer's and the program's windows; the texts start at the same edge as the toolbar.

## 0.1.2-alpha (2026-11-02)

- **No window animation with animations off:** with View → Animations turned off in the program (RustySalamander 0.7.4-alpha and later), Windows no longer fades or zooms its windows in and out.

## 0.1.1-alpha (2026-11-01)

- **Animations can be off:** with View → Animations turned off in the program (RustySalamander 0.7.1-alpha and later), its windows don't fade in or scroll smoothly either.

## 0.1.0-alpha (2026-11-01)

- **The first version.** Plugins → Compare compares two files or two folders: two items selected in the active panel, else the item selected in each panel, else the items under the two cursors, else the two panels' folders. **Compare files…** and **Compare folders…** ask for them in a form.
- **Files side by side:** a window of its own shows the two texts lined up line by line, with line numbers; changed lines amber, lines only the left has red, lines only the right has green. F8 and Shift+F8 (or Alt+↓ and Alt+↑) go from one difference to the next. **Ignore spaces** treats lines differing only in spaces and tabs as the same; **Compare again** reads the files again. Texts in any encoding the viewer reads compare by their text (a UTF-8 file and the same text in UTF-16 are the same). Binary files, and files over 32 MB, show their sizes and where their bytes first differ.
- **Folders in both panels:** the left folder opens in the left panel and the right one in the right, each listing everything either side has, so the lists line up. Items are colored: amber when they differ (folders too, when something inside differs), green when only this side has them; what only the other side has is grey, as a placeholder. Going into a folder or up in one panel takes the other along; each side stays in its own panel. Files are the same when their size and date are (two seconds apart allowed), or, chosen in the form, their contents.
- **The real files:** copying (F5) from one side to the other, deleting (F8, to the Recycle Bin) and new folders (F7) change the real folders, and the comparison follows; Enter, F3 and F4 open, view and edit the real files. **Refresh** in the drive selector (under Comparisons) compares again. Closing either side closes both. One comparison at a time: a new one closes the one before.
