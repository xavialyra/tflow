#!/usr/bin/env python3
import json
import sys

request = json.load(sys.stdin)
context = request.get("context", {})
parameters = context.get("parameters", {})
next_query = parameters.get("next") if isinstance(parameters, dict) else None
if not isinstance(next_query, dict):
    raise SystemExit("btop next query must be an object")
json.dump(
    {
        "version": 1,
        "operation": {
            "type": "navigate",
            "target": "btop:main",
            "query": next_query,
            "replace": True,
        },
    },
    sys.stdout,
    separators=(",", ":"),
)
sys.stdout.write("\n")
