#!/usr/bin/env python3
import json
import os
import sys

request = json.load(sys.stdin)
context = request.get("context", {})
params = context.get("parameters", {})
app = params.get("app", "")

root = os.environ.get("XDG_STATE_HOME") or os.path.expanduser("~/.local/state")
path = os.path.join(root, "tlaunch", "app-weights.json")

weight = 0
pinned = False

if app:
    try:
        with open(path, encoding="utf-8") as f:
            data = json.load(f)
        if isinstance(data, dict):
            entry = data.get(os.path.basename(app), {})
            if isinstance(entry, dict):
                raw_weight = entry.get("weight")
                if raw_weight is None:
                    raw_weight = entry.get("launch_count", 0)
                try:
                    weight = int(raw_weight)
                except (ValueError, TypeError):
                    weight = 0
                pinned = bool(entry.get("pinned", False))
    except (OSError, ValueError):
        pass

fields = [
    {
        "name": "weight",
        "label": "Weight",
        "type": "integer",
        "value": weight,
        "required": True,
    },
    {
        "name": "pinned",
        "label": "Pin",
        "type": "boolean",
        "value": pinned,
        "required": True,
    },
]

json.dump({"version": 1, "content": {"fields": fields}}, sys.stdout)
print()
