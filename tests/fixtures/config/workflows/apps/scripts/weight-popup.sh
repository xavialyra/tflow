#!/usr/bin/env python3
import json
import sys

item = (
    json.load(sys.stdin)
    .get("context", {})
    .get("engine", {})
    .get("state", {})
    .get("item")
)
app = item.get("value") if isinstance(item, dict) else ""
json.dump(
    {
        "version": 1,
        "operation": {
            "type": "call",
            "target": "weight",
            "query": {"app": app},
            "presentation": {"mode": "popup", "width": 50, "height": 10},
        },
    },
    sys.stdout,
    separators=(",", ":"),
)
print()
