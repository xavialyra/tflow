#!/usr/bin/env python3
import json
import sys


try:
    request = json.load(sys.stdin)
except Exception:
    json.dump({"version": 1, "output": None}, sys.stdout)
    sys.exit(0)

if request.get("version") != 1:
    json.dump({"version": 1, "output": None}, sys.stdout)
    sys.exit(0)

context = request.get("context", {})
raw_params = context.get("parameters") if isinstance(context, dict) else {}
if isinstance(raw_params, str):
    try:
        params = json.loads(raw_params)
    except Exception:
        params = {}
elif isinstance(raw_params, dict):
    params = raw_params
else:
    params = {}

item = params.get("item")
if not isinstance(item, dict) and isinstance(context, dict):
    inp = context.get("input")
    if isinstance(inp, dict):
        item = inp

if not isinstance(item, dict):
    json.dump({"version": 1, "output": None}, sys.stdout)
    sys.exit(0)

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
json.dump({"version": 1, "output": preview}, sys.stdout)
sys.stdout.write("\n")
