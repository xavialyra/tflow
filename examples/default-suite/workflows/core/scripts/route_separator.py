#!/usr/bin/env python3
"""Space-key route separator for the core entry View.

Space is bound in the View's keymap, so the engine does not insert it. If the
current input names a known route the command jumps to it and consumes the
input. Otherwise it re-mounts this View with a literal space appended, carrying
the current parameters and the selected item (`__focus`) so filtering and
selection survive the round trip.
"""
import json
import os
import sys

# Keep imported bytecode out of the workflow directory; the cache is not
# worth an extra `__pycache__` in a user's configuration tree.
sys.dont_write_bytecode = True
sys.path.insert(0, os.path.dirname(os.path.realpath(__file__)))
import completion_routes

SELF = completion_routes.own_view_ref()
INPUT_FIELD = "search"

request = json.load(sys.stdin)
context = request.get("context") or {}
parameters = context.get("parameters") or {}
if not isinstance(parameters, dict):
    parameters = {}
engine = context.get("engine") or {}
state = engine.get("state") if isinstance(engine, dict) else None
if not isinstance(state, dict):
    state = {}
input_text = state.get("input")
if not isinstance(input_text, str):
    input_text = ""

selector = input_text.strip()
match = completion_routes.find_route(selector)

if match is not None:
    operation = {"type": "navigate", "target": match["value"], "clear_input": True}
else:
    query = dict(parameters)
    query[INPUT_FIELD] = input_text + " "
    item = state.get("item")
    if isinstance(item, dict):
        focus = item.get("value")
        if not isinstance(focus, str) or not focus:
            focus = item.get("text")
        if isinstance(focus, str) and focus:
            query["__focus"] = focus
    operation = {
        "type": "navigate",
        "target": SELF,
        "query": query,
        "replace": True,
    }

json.dump({"version": 1, "operation": operation}, sys.stdout, separators=(",", ":"))
sys.stdout.write("\n")
