"""Cut the Material Design Icons font down to the icons the program uses.

The icons in use are the `\\u{...}` codes in crates/sal-ui/src/mdi.rs; the
result replaces crates/sal-ui/fonts/materialdesignicons-subset.ttf (a few
kilobytes instead of 1.3 MB). Run it after adding an icon to mdi.rs:

    python tools/subset-mdi.py materialdesignicons-webfont.ttf

The full font is `fonts/materialdesignicons-webfont.ttf` of the npm package
@mdi/font 7.4.47 (https://cdn.jsdelivr.net/npm/@mdi/font@7.4.47/). Needs
fontTools (`pip install fonttools`). The test `every_icon_is_in_the_font` in
sal-ui fails when an icon is missing from the cut-down font.
"""

import pathlib
import re
import sys

from fontTools import subset

root = pathlib.Path(__file__).resolve().parent.parent
icons = (root / "crates" / "sal-ui" / "src" / "mdi.rs").read_text(encoding="utf-8")
codes = sorted({int(h, 16) for h in re.findall(r"\\u\{([0-9A-Fa-f]+)\}", icons)})
out = root / "crates" / "sal-ui" / "fonts" / "materialdesignicons-subset.ttf"

options = subset.Options()
# keep the font's names: they carry its copyright and license
options.name_IDs = ["*"]
options.name_languages = ["*"]
options.notdef_outline = True
font = subset.load_font(sys.argv[1], options)
subsetter = subset.Subsetter(options)
subsetter.populate(unicodes=codes)
subsetter.subset(font)
subset.save_font(font, str(out), options)
print(f"{len(codes)} icons, {out.stat().st_size} bytes: {out}")
