"""Checks every assets/lang/*.json against assets/lang/template.json:
all keys present, nothing empty, placeholders and leading icons preserved, no em dashes.

    python tools/check_translations.py
"""

import json
import re
import sys
from pathlib import Path

sys.stdout.reconfigure(encoding="utf-8")
template = json.loads(Path("assets/lang/template.json").read_text(encoding="utf-8"))["strings"]
placeholder = re.compile(r"\{[a-z_]+\}")
icon = re.compile(r"^([^\w\s\"'(\[{]+\s*)")
problems = 0

for path in sorted(Path("assets/lang").glob("*.json")):
    if path.name == "template.json":
        continue
    data = json.loads(path.read_text(encoding="utf-8"))
    strings = data.get("strings", {})
    issues = []
    missing = [k for k in template if not strings.get(k, "").strip()]
    if missing:
        issues.append(f"{len(missing)} missing, e.g. {missing[0]!r}")
    for key, value in strings.items():
        if key not in template:
            issues.append(f"unknown key {key!r}")
            continue
        if sorted(placeholder.findall(key)) != sorted(placeholder.findall(value)):
            issues.append(f"placeholders differ in {key!r}: {value!r}")
        lead = icon.match(key)
        if lead and not value.startswith(lead.group(1)):
            issues.append(f"leading icon changed in {key!r}: {value!r}")
        if chr(0x2014) in value:  # em dash
            issues.append(f"em dash in {key!r}")
    problems += len(issues)
    print(f"{path.stem:6} {data.get('name', '?'):22} {'OK' if not issues else f'{len(issues)} issue(s)'}")
    for issue in issues[:8]:
        print("       ", issue)

sys.exit(1 if problems else 0)
