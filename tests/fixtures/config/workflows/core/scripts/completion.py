#!/usr/bin/env python3
"""Picker items for the core route-completion popup."""
import json
import os
import sys

sys.path.insert(0, os.path.dirname(os.path.realpath(__file__)))
import completion_routes


def main():
    try:
        request = json.load(sys.stdin)
    except Exception:
        request = {}
    context = request.get("context") or {}
    parameters = context.get("parameters") or {}
    if not isinstance(parameters, dict):
        parameters = {}

    needle = parameters.get("filter")
    if not isinstance(needle, str):
        needle = ""

    items = completion_routes.candidates(needle)
    json.dump({"version": 1, "items": items}, sys.stdout, separators=(",", ":"))
    sys.stdout.write("\n")


if __name__ == "__main__":
    main()
