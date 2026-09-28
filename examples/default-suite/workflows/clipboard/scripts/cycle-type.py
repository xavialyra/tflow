#!/usr/bin/env python3
"""Switch the clipboard content filter while preserving Picker state."""
import json
import sys

CONTENT_TYPES = ("all", "text", "image", "binary")

request = json.load(sys.stdin)
context = request.get("context") or {}
parameters = context.get("parameters") or {}
if not isinstance(parameters, dict):
    parameters = {}

command = context.get("command") or {}
command_id = command.get("id", "") if isinstance(command, dict) else ""
direction = -1 if command_id.rsplit(":", 1)[-1] == "previous_type" else 1

current = parameters.get("content_type", CONTENT_TYPES[0])
try:
    current_index = CONTENT_TYPES.index(current)
except ValueError:
    current_index = 0

query = dict(parameters)
query["content_type"] = CONTENT_TYPES[(current_index + direction) % len(CONTENT_TYPES)]

engine = context.get("engine") or {}
state = engine.get("state") if isinstance(engine, dict) else None
if isinstance(state, dict):
    input_text = state.get("input")
    if isinstance(input_text, str):
        query["search"] = input_text

    item = state.get("item")
    if isinstance(item, dict):
        focus = item.get("value")
        if not isinstance(focus, str) or not focus:
            focus = item.get("text")
        if isinstance(focus, str) and focus:
            query["__focus"] = focus

response = {
    "version": 1,
    "operation": {
        "type": "navigate",
        "target": "main",
        "query": query,
        "replace": True,
    },
}
json.dump(response, sys.stdout, separators=(",", ":"))
sys.stdout.write("\n")
