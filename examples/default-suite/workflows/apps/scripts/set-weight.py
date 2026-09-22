#!/usr/bin/env python3
import json
import os
import sys

request = json.load(sys.stdin)
context = request.get("context", {})
params = context.get("parameters", {})
app = params.get("app")
if not isinstance(app, str) or not app:
    raise SystemExit("app is required")

state = context.get("engine", {}).get("state", {})
if not state.get("valid", False):
    raise SystemExit("Please correct the form fields")

values = state.get("values", {})
raw_weight = values.get("weight")
try:
    weight = int(raw_weight)
except (TypeError, ValueError):
    weight = 0

pinned = bool(values.get("pinned") if "pinned" in values else values.get("pin", False))

root = os.environ.get("XDG_STATE_HOME") or os.path.expanduser("~/.local/state")
path = os.path.join(root, "tflow", "app-weights.json")
os.makedirs(os.path.dirname(path), exist_ok=True)
try:
    with open(path, encoding="utf-8") as f:
        data = json.load(f)
    if not isinstance(data, dict):
        data = {}
except (OSError, ValueError):
    data = {}

entry = data.setdefault(os.path.basename(app), {})
entry["weight"] = weight
entry["pinned"] = pinned

tmp = path + ".tmp"
with open(tmp, "w", encoding="utf-8") as f:
    json.dump(data, f, separators=(",", ":"))
os.replace(tmp, path)

json.dump({"version": 1, "operation": {"type": "return", "value": state.get("values", True)}}, sys.stdout)

