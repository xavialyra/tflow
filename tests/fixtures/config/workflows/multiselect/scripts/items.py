#!/usr/bin/env python3
import json
import sys

CANDIDATES = [
    {"value": "ripgrep", "name": "ripgrep", "desc": "Fast line-oriented search tool"},
    {"value": "bat", "name": "bat", "desc": "A cat clone with syntax highlighting"},
    {"value": "fd-find", "name": "fd-find", "desc": "Simple, fast and user-friendly alternative to find"},
    {"value": "fzf", "name": "fzf", "desc": "General-purpose command-line fuzzy finder"},
    {"value": "tmux", "name": "tmux", "desc": "Terminal multiplexer"},
]


def main():
    request = json.load(sys.stdin)
    if request.get("entrypoint") != "picker-items":
        raise ValueError("expected a picker-items producer request")

    context = request.get("context", {})
    parameters = context.get("parameters", {})
    selected = set(parameters.get("selected", []))

    engine = context.get("engine", {})
    state = engine.get("state", {}) if isinstance(engine, dict) else {}
    query = state.get("input", "") if isinstance(state, dict) else ""
    tokens = query.casefold().split()

    items = []
    for c in CANDIDATES:
        name = c["name"]
        desc = c["desc"]
        searchable = f"{name} {desc}".casefold()
        if not all(token in searchable for token in tokens):
            continue

        val = c["value"]
        is_sel = val in selected
        mark = "[x] " if is_sel else "[ ] "

        if is_sel:
            display = {
                "constraints": [{"Length": 4}, {"Fill": 1}, {"Length": 10}],
                "cells": [
                    {"text": mark, "slot": "checked"},
                    {"text": name},
                    {"text": "selected", "align": "right", "slot": "checked"},
                ],
            }
        else:
            display = {
                "constraints": [{"Length": 4}, {"Fill": 1}],
                "cells": [
                    {"text": mark, "slot": "unchecked"},
                    {"text": name},
                ],
            }

        items.append({
            "value": val,
            "display": display,
            "metadata": {
                "name": name,
                "desc": desc,
                "selected": is_sel,
            },
        })

    json.dump({"version": 1, "items": items}, sys.stdout, separators=(",", ":"))
    sys.stdout.write("\n")


if __name__ == "__main__":
    main()
