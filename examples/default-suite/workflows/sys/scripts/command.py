#!/usr/bin/env python3
"""Main command router for the system workflow.

Directs safe operations (lock, suspend, logout) directly to execution, and
routes destructive operations (reboot, poweroff) through a confirmation prompt.
"""
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
        raise SystemExit("system command requires a selected item")

    value = item.get("value")
    if value not in {"lock", "suspend", "logout", "reboot", "poweroff"}:
        raise SystemExit(f"unknown system action: {value}")

    # Reboots and shutdowns can discard unsaved work; require confirmation
    if value in {"reboot", "poweroff"}:
        operation = {
            "type": "navigate",
            "target": "confirm",
            "query": {"action": value},
            "presentation": {"mode": "popup", "width": 46, "height": 7},
        }
    else:
        operation = {
            "type": "navigate",
            "target": "output",
            "query": value,
        }

    json.dump({"version": 1, "operation": operation}, sys.stdout, separators=(",", ":"))
    sys.stdout.write("\n")


if __name__ == "__main__":
    main()
