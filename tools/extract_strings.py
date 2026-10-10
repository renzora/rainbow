"""Collects every interface string Rainbow translates and writes assets/lang/template.json,
the list each language file should cover. Run from the repository root:

    python tools/extract_strings.py
"""

import json
import re
from pathlib import Path

LIT = r'"((?:[^"\\]|\\.)*)"'
src = {p.name: p.read_text(encoding="utf-8") for p in Path("src").glob("*.rs")}
found = []


def add(text):
    text = bytes(text, "utf-8").decode("unicode_escape").encode("latin-1").decode("utf-8")
    if "assets/" in text or text.endswith((".json", ".ttf", ".png")):
        return
    if re.search(r"[A-Za-z]", text) and text not in found:
        found.append(text)


# Marked strings: t("…"), tr!("…"), n("…").
for code in src.values():
    for m in re.finditer(r'(?<![\w:])(?:t|n)\(\s*' + LIT + r'\s*,?\s*\)', code):
        add(m.group(1))
    for m in re.finditer(r'tr!\(\s*' + LIT, code):
        add(m.group(1))
    for m in re.finditer(r'trn!\([^,]+,\s*' + LIT + r'\s*,\s*' + LIT, code):
        add(m.group(1))
        add(m.group(2))

# Names shown from tables in other modules.
api = src["api.rs"]
for m in re.finditer(r'\("[a-z]+", "([^"]+)", "diff_\w+"\)', api):
    add(m.group(1))  # INSTRUMENTS
for name in re.findall(r'\("(?:expert|hard|medium|easy)", "(\w+)"\)', api):
    add(name)  # DIFFICULTIES
for name in re.findall(r'"(4-Lane(?: Pro)?|5-Lane)"', api):
    add(name)  # DRUM_TYPES
flt = src["filter.rs"]
fields = re.search(r"TEXT_FIELDS: \[&str; 6\] = \[(.*?)\];", flt, re.S)
for name in re.findall(LIT, fields.group(1)):
    add(name)
for name in re.findall(r'\("([^"]+)", flag::\w+, Applies::\w+\)', flt):
    add(name)  # TOGGLES
theme = src["theme.rs"]
for block in re.findall(r"fn (?:name|about)\(self\) -> &'static str \{(.*?)\n    \}", theme, re.S):
    for text in re.findall(r"=> " + LIT, block):
        add(text)

out = Path("assets/lang/template.json")
out.write_text(json.dumps({"name": "", "english_name": "", "strings": {s: "" for s in found}}, ensure_ascii=False, indent=2) + "\n", encoding="utf-8")
print(f"{len(found)} strings -> {out}")
