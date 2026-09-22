#!/usr/bin/env python3
"""Launch the selected .desktop entry and record the launch.

`Enter` (`open`) waits for the activation call to return. `Ctrl+O`
(`open_detached`) runs the same script and returns immediately, which keeps the
launcher from sitting in the foreground of an application that takes its time to
register with the session bus.
"""
import json
import os
import sys

request = json.load(sys.stdin)
context = request.get("context") or {}
state = (context.get("engine") or {}).get("state") or {}
item = state.get("item") if isinstance(state, dict) else None
value = item.get("value") if isinstance(item, dict) else None
if not isinstance(value, str):
    raise SystemExit("apps command requires a selected item with a value")

command = context.get("command") or {}
command_id = (command.get("id") or "").split(":")[-1] if isinstance(command, dict) else ""
detached = command_id == "open_detached"

state_home = os.environ.get("XDG_STATE_HOME") or os.path.expanduser("~/.local/state")
path = os.path.join(state_home, "tflow", "app-weights.json")
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

argv = ["setsid"]
if not detached:
    argv.append("--wait")
argv += ["gio", "launch", value]

json.dump(
    {
        "version": 1,
        "operation": {"type": "run", "mode": "foreground", "argv": argv, "exit": True},
    },
    sys.stdout,
    separators=(",", ":"),
)
sys.stdout.write("\n")
