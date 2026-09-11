#!/usr/bin/env python3
import json
import os
import sys

request = json.load(sys.stdin)
state = request.get("context", {}).get("engine", {}).get("state", {})
context = request.get("context", {})
params = context.get("parameters", {})
app = params.get("app")
state = context.get("engine", {}).get("state", {})
if not isinstance(app, str) or not app:
    raise SystemExit("app is required")
root = os.environ.get("XDG_STATE_HOME") or os.path.expanduser("~/.local/state")
path = os.path.join(root, "tlaunch", "app-weights.json")
os.makedirs(os.path.dirname(path), exist_ok=True)
try:
    with open(path, encoding="utf-8") as f:
        data = json.load(f)
    if not isinstance(data, dict): data = {}
except (OSError, ValueError):
    data = {}
entry = data.setdefault(os.path.basename(app), {})
entry["pinned"] = not bool(entry.get("pinned", False))
tmp = path + ".tmp"
with open(tmp, "w", encoding="utf-8") as f: json.dump(data, f, separators=(",", ":"))
os.replace(tmp, path)
json.dump({"version": 1, "operation": {"type": "return", "value": True}}, sys.stdout)
