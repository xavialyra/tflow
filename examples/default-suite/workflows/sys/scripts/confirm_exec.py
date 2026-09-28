#!/usr/bin/env python3
"""Execution handler for confirmed actions."""
import json
import sys


def main():
    try:
        request = json.load(sys.stdin)
    except Exception:
        raise SystemExit("failed to parse JSON request")

    context = request.get("context", {})
    state = context.get("engine", {}).get("state", {})
    item = state.get("item") if isinstance(state, dict) else None
    if not isinstance(item, dict):
        raise SystemExit("confirmation requires a selected item")

    value = item.get("value")

    if value == "cancel":
        operation = {
            "type": "navigate",
            "target": "main",
            "replace": True,
        }
    else:
        operation = {
            "type": "navigate",
            "target": "output",
            "query": value,
            "replace": True,
        }

    json.dump({"version": 1, "operation": operation}, sys.stdout, separators=(",", ":"))
    sys.stdout.write("\n")


if __name__ == "__main__":
    main()
