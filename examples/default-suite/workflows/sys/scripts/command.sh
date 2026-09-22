#!/bin/sh
set -eu

python3 -c '
import json
import os
import sys


def workflow_owner():
    """Member id this package is mounted as; the suite decides it, not us."""
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
    if not name:
        raise SystemExit("cannot resolve the workflow owner; set TFLOW_WORKFLOW_DIR")
    return name


request = json.load(sys.stdin)
context = request.get("context", {})
state = context.get("engine", {}).get("state", {})
item = state.get("item") if isinstance(state, dict) else None
if not isinstance(item, dict):
    raise SystemExit("system command requires a selected item")

value = item.get("value")
if value not in {"logout", "reboot", "poweroff"}:
    raise SystemExit("unknown system action")

json.dump({
    "version": 1,
    "operation": {
        "type": "navigate",
        "target": f"{workflow_owner()}:output",
        "query": value,
    },
}, sys.stdout, separators=(",", ":"))
sys.stdout.write("\n")
'
