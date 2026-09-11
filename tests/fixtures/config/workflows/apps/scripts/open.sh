#!/bin/sh
exec python3 -c '
import json
import os
import sys

request = json.load(sys.stdin)
state = request.get("context", {}).get("engine", {}).get("state", {})
item = state.get("item") if isinstance(state, dict) else None
value = item.get("value") if isinstance(item, dict) else None
if not isinstance(value, str):
    raise SystemExit("apps command requires a selected item with a value")
state_home = os.environ.get("XDG_STATE_HOME") or os.path.expanduser("~/.local/state")
path = os.path.join(state_home, "tlaunch", "app-weights.json")
os.makedirs(os.path.dirname(path), exist_ok=True)
try:
    with open(path, encoding="utf-8") as f:
        weights = json.load(f)
    if not isinstance(weights, dict):
        weights = {}
except (OSError, ValueError):
    weights = {}
key = os.path.basename(value)
entry = weights.setdefault(key, {})
entry["launch_count"] = int(entry.get("launch_count", 0) or 0) + 1
tmp = path + ".tmp"
with open(tmp, "w", encoding="utf-8") as f:
    json.dump(weights, f, separators=(",", ":"))
os.replace(tmp, path)
json.dump({"version": 1, "operation": {"type": "run", "mode": "foreground", "argv": ["setsid", "--wait", "gio", "launch", value], "exit": True}}, sys.stdout, separators=(",", ":"))
sys.stdout.write("\n")
'
