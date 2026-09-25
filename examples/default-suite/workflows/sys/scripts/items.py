#!/usr/bin/env python3
"""Item producer for system and session actions.

Provides system control actions (lock, suspend, logout, reboot, poweroff)
with rich descriptions, token search matching, and preview metadata.
"""
import json
import os
import sys

ACTIONS = [
    {
        "value": "lock",
        "title": "Lock screen",
        "desc": "Lock the current desktop session",
        "keywords": ["lock", "screen", "session"],
    },
    {
        "value": "suspend",
        "title": "Sleep / Suspend",
        "desc": "Put the computer into low-power sleep mode",
        "keywords": ["sleep", "suspend", "standby"],
    },
    {
        "value": "logout",
        "title": "Log out",
        "desc": f"End session for user {os.environ.get('USER', 'current')}",
        "keywords": ["logout", "exit", "sign out"],
    },
    {
        "value": "reboot",
        "title": "Restart",
        "desc": "Reboot the operating system",
        "keywords": ["restart", "reboot"],
    },
    {
        "value": "poweroff",
        "title": "Shut down",
        "desc": "Power off the computer completely",
        "keywords": ["shutdown", "shut down", "poweroff", "turn off"],
    },
]


def matches(query: str, action: dict) -> bool:
    if not query:
        return True
    tokens = query.lower().split()
    target_text = (
        f"{action['value']} {action['title']} {action['desc']} "
        + " ".join(action["keywords"])
    ).lower()
    return all(token in target_text for token in tokens)


def main():
    try:
        request = json.load(sys.stdin)
    except Exception:
        request = {}

    state = request.get("context", {}).get("engine", {}).get("state", {})
    query = state.get("input", "").strip()

    items = []
    for action in ACTIONS:
        if matches(query, action):
            items.append(
                {
                    "value": action["value"],
                    "display": action["title"],
                    "metadata": {
                        "title": action["title"],
                        "description": action["desc"],
                        "action": action["value"],
                    },
                }
            )

    json.dump({"version": 1, "items": items}, sys.stdout, separators=(",", ":"))
    sys.stdout.write("\n")


if __name__ == "__main__":
    main()
