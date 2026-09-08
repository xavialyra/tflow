#!/bin/sh
set -eu

exec python3 -c '
import json
import sys

request = json.load(sys.stdin)
context = request.get("context", {})
engine = context.get("engine", {})
state = engine.get("state", {}) if isinstance(engine, dict) else {}
item = state.get("item") if isinstance(state, dict) else None
command = item.get("metadata", {}).get("command") if isinstance(item, dict) else None
if not isinstance(command, dict) or not isinstance(command.get("view"), str) or not isinstance(command.get("id"), str):
    raise SystemExit("command selector result has no command reference")
json.dump({
    "version": 1,
    "operation": {"type": "return", "value": command},
}, sys.stdout, separators=(",", ":"))
sys.stdout.write("\n")
'
