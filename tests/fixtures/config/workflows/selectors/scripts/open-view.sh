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
target = item.get("value") if isinstance(item, dict) else None
if not isinstance(target, str) or not target:
    raise SystemExit("view selector requires a selected view")

json.dump({
    "version": 1,
    "operation": {
        "type": "navigate",
        "target": target,
    },
}, sys.stdout, separators=(",", ":"))
sys.stdout.write("\n")
'
