#!/usr/bin/env python3
"""Open the route chosen in the completion popup, replacing the popup itself."""
import json
import sys

request = json.load(sys.stdin)
context = request.get("context") or {}
state = (context.get("engine") or {}).get("state") or {}

item = state.get("item") if isinstance(state, dict) else None
if not isinstance(item, dict) or not isinstance(item.get("value"), str):
    raise SystemExit("completion selection has no route")

operation = {"type": "navigate", "target": item["value"], "replace": True}
json.dump({"version": 1, "operation": operation}, sys.stdout, separators=(",", ":"))
sys.stdout.write("\n")
