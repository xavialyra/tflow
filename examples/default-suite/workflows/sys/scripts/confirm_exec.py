#!/usr/bin/env python3
"""Execution handler for confirmed actions."""
import json
import os
import sys


def workflow_owner():
    root = os.path.realpath(os.environ.get("TFLOW_WORKFLOW_DIR") or "")
    try:
        import tomllib

        with open(os.environ["TFLOW_SUITE"], "rb") as handle:
            workflows = tomllib.load(handle).get("workflows") or {}
        base = os.path.dirname(os.path.abspath(os.environ["TFLOW_SUITE"]))
        for member, entry in workflows.items():
            target = (entry or {}).get("dir") or (entry or {}).get("file")
            if isinstance(target, str) and os.path.realpath(
                os.path.join(base, target)
            ) == root:
                return member
    except Exception:
        pass
    name = os.path.basename(root)
    return name if name else "sys"


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
    owner = workflow_owner()

    if value == "cancel":
        operation = {
            "type": "navigate",
            "target": f"{owner}:main",
            "replace": True,
        }
    else:
        operation = {
            "type": "navigate",
            "target": f"{owner}:output",
            "query": value,
            "replace": True,
        }

    json.dump({"version": 1, "operation": operation}, sys.stdout, separators=(",", ":"))
    sys.stdout.write("\n")


if __name__ == "__main__":
    main()
