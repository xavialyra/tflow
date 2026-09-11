#!/usr/bin/env python3
import json
import sys


request = json.load(sys.stdin)
if request.get("version") != 1 or request.get("entrypoint") != "picker-preview":
    raise ValueError("expected a version-1 picker-preview request")

item = request["context"]["engine"]["state"]["item"]
metadata = item.get("metadata") or {}
children = []
constraints = []

thumbnail = metadata.get("thumbnail")
if isinstance(thumbnail, str) and thumbnail:
    children.append({"type": "image", "path": thumbnail})
    constraints.append({"Fill": 1})

children.append({"type": "separator"})
constraints.append({"Length": 1})

summary = metadata.get("summary", "")
if not isinstance(summary, str):
    summary = str(summary)
children.append({"type": "paragraph", "text": summary})
constraints.append({"Length": 5})

json.dump({
    "version": 1,
    "preview": {
        "type": "layout",
        "direction": "vertical",
        "constraints": constraints,
        "children": children,
    },
}, sys.stdout)
sys.stdout.write("\n")
