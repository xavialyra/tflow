#!/usr/bin/env python3
"""Read one picker-preview request; emit only a version-1 data document."""
import json
import sys

request = json.load(sys.stdin)
assert request["version"] == 1 and request["entrypoint"] == "picker-preview"
context = request["context"]
item = context["engine"]["state"]["item"]
preview = None
if item["value"] != "empty":
    preview = {
        "type": "layout",
        "direction": "vertical",
        "constraints": [{"Length": 2}, {"Length": 1}, {"Length": 8}, {"Fill": 1}],
        "children": [
            {"type": "display", "display": {"rows": [
                {"cells": [{"text": item["text"], "slot": "accent"}]},
                {"cells": [{"text": context["parameters"]["owner"], "slot": "badge"}]},
            ]}},
            {"type": "separator"},
            {"type": "layout", "direction": "horizontal", "children": [
                {"type": "image", "path": item["metadata"]["image"]},
                {"type": "paragraph", "border": True, "title": "Details", "spans": [
                    {"text": "Ready. ", "slot": "success"},
                    item["metadata"]["summary"],
                ]},
            ]},
            {"type": "paragraph", "text": "\n".join(
                f"Line {number}: generated preview content" for number in range(1, 21)
            )},
        ],
    }
json.dump({"version": 1, "preview": preview}, sys.stdout)
sys.stdout.write("\n")
