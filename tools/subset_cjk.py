"""Builds small Korean and Chinese interface fonts containing only the characters Rainbow's
translations use, so the app doesn't need to bundle ~40 MB of CJK fonts.

Also builds a small Japanese one for the web version, which shows it until it has downloaded the
full Japanese font (see theme::noto_sans_jp).

Re-run after changing ja.json, ko.json, zh-CN.json or zh-TW.json:

    pip install fonttools
    python tools/subset_cjk.py <folder with NotoSansKR.ttf, NotoSansSC.ttf, NotoSansTC.ttf>

(The Japanese one is made from assets/fonts/NotoSansJP.ttf.)

(The full fonts are the variable "[wght]" files from github.com/google/fonts, ofl/notosans*.)
"""

import json
import sys
from pathlib import Path

from fontTools import subset
from fontTools.ttLib import TTFont
from fontTools.varLib import instancer

SOURCES = Path(sys.argv[1] if len(sys.argv) > 1 else ".")
JOBS = [
    ("ja.json", Path("assets/fonts/NotoSansJP.ttf"), "NotoSansJP-UI.ttf"),
    ("ko.json", "NotoSansKR.ttf", "NotoSansKR-UI.ttf"),
    ("zh-CN.json", "NotoSansSC.ttf", "NotoSansSC-UI.ttf"),
    ("zh-TW.json", "NotoSansTC.ttf", "NotoSansTC-UI.ttf"),
]

for lang, source, output in JOBS:
    data = json.loads(Path("assets/lang", lang).read_text(encoding="utf-8"))
    text = data["name"] + "".join(data["strings"].values())
    chars = {c for c in text if ord(c) > 0x7F} | {chr(c) for c in range(0x20, 0x7F)}
    font = TTFont(source if isinstance(source, Path) else SOURCES / source)
    # Fix the weight at Regular so a variable font shrinks to one static face.
    if "fvar" in font:
        font = instancer.instantiateVariableFont(font, {"wght": 400})
    options = subset.Options()
    options.layout_features = ["*"]
    options.name_IDs = ["*"]
    options.notdef_outline = True
    sub = subset.Subsetter(options)
    sub.populate(text="".join(sorted(chars)))
    sub.subset(font)
    out = Path("assets/fonts", output)
    font.save(out)
    print(f"{output}: {len(chars)} characters, {out.stat().st_size // 1024} KB")
