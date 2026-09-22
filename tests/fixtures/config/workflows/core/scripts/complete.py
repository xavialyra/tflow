#!/usr/bin/env python3
"""Tab command for the core workflow.

A typed alias or reference routes straight to it, and so does a single fuzzy
candidate; anything else opens the popup so the user can choose. Every branch
returns the same `navigate` operation type, because a command's declared type
is static. The popup is opened as a popup-presentation push, and its own accept
command replaces it with the chosen route. Candidates come from the shared
`completion_routes` lookup rather than being pushed by the host.
"""
import json
import os
import sys

sys.path.insert(0, os.path.dirname(os.path.realpath(__file__)))
import completion_routes

request = json.load(sys.stdin)
context = request.get("context") or {}
state = (context.get("engine") or {}).get("state") or {}
prefix = state.get("input", "") if isinstance(state, dict) else ""
if not isinstance(prefix, str):
    prefix = ""

matches = completion_routes.candidates(prefix)
# An exact alias or reference is unambiguous, so it wins even when other
# routes merely contain it as a substring (for example `pass` inside
# `pass:unlock`).
route = completion_routes.find_route(prefix.strip())
if route is None and len(matches) == 1:
    route = matches[0]

if route is not None:
    operation = {
        "type": "navigate",
        "target": route["value"],
        "clear_input": True,
    }
else:
    operation = {
        "type": "navigate",
        "target": completion_routes.view_ref("completion"),
        "query": {"filter": prefix},
        "presentation": {"mode": "popup", "width": 72, "height": 16},
        "clear_input": True,
    }
json.dump({"version": 1, "operation": operation}, sys.stdout, separators=(",", ":"))
sys.stdout.write("\n")
