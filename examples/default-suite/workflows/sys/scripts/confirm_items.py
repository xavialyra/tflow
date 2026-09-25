#!/usr/bin/env python3
"""Confirmation items producer for reboot/poweroff actions."""
import json
import sys


def main():
    try:
        request = json.load(sys.stdin)
    except Exception:
        request = {}

    context = request.get("context", {})
    parameters = context.get("parameters") or {}
    if not isinstance(parameters, dict):
        action = str(parameters)
    else:
        action = parameters.get("action", "action")

    action_label = "Restart" if action == "reboot" else "Shut down"

    items = [
        {
            "value": action,
            "display": f"✓ Confirm: {action_label} now",
            "metadata": {"action": action},
        },
        {
            "value": "cancel",
            "display": "✗ Cancel (keep system running)",
            "metadata": {"action": "cancel"},
        },
    ]

    json.dump({"version": 1, "items": items}, sys.stdout, separators=(",", ":"))
    sys.stdout.write("\n")


if __name__ == "__main__":
    main()
