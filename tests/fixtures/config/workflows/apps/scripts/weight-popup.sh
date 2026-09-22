#!/usr/bin/env python3
import json
import os
import sys


def workflow_owner():
    """Member id this package is mounted as; the suite decides it, not us."""
    root = os.environ.get("TFLOW_WORKFLOW_DIR") or os.path.dirname(
        os.path.dirname(os.path.realpath(__file__))
    )
    root = os.path.realpath(root)
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


item = (
    json.load(sys.stdin)
    .get("context", {})
    .get("engine", {})
    .get("state", {})
    .get("item")
)
app = item.get("value") if isinstance(item, dict) else ""
json.dump(
    {
        "version": 1,
        "operation": {
            "type": "call",
            "target": f"{workflow_owner()}:weight",
            "query": {"app": app},
            "presentation": {"mode": "popup", "width": 50, "height": 10},
        },
    },
    sys.stdout,
    separators=(",", ":"),
)
print()
