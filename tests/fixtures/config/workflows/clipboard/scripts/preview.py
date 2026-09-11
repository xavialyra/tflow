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


def add(document, constraint):
    children.append(document)
    constraints.append(constraint)


title = metadata.get("title", "")
if isinstance(title, str):
    add({"type": "paragraph", "text": title}, {"Length": 1})

summary = metadata.get("summary", "")
if isinstance(summary, str):
    add({"type": "paragraph", "text": summary}, {"Length": 2})

if children:
    add({"type": "separator"}, {"Length": 1})

thumbnail = metadata.get("thumbnail")
if isinstance(thumbnail, str) and thumbnail:
    add({"type": "image", "path": thumbnail}, {"Fill": 1})

content = metadata.get("content")
if isinstance(content, str):
    add({"type": "paragraph", "text": content}, {"Fill": 1})

preview = None if not children else {
    "type": "layout",
    "direction": "vertical",
    "constraints": constraints,
    "children": children,
}
json.dump({"version": 1, "preview": preview}, sys.stdout)
sys.stdout.write("\n")
