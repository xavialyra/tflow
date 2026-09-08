#!/bin/sh
set -eu

python3 -c '
import json
import sys

request = json.load(sys.stdin)
context = request.get("context", {})
engine = context.get("engine", {})
state = engine.get("state", {}) if isinstance(engine, dict) else {}
item = state.get("item") if isinstance(state, dict) else None
if not isinstance(item, dict) or not isinstance(item.get("text"), str):
    raise SystemExit("system command requires a selected item")
json.dump({
    "version": 1,
    "operation": {
        "type": "navigate",
        "target": "sys:output",
        "query": item["text"],
    },
}, sys.stdout, separators=(",", ":"))
sys.stdout.write("\n")
'
