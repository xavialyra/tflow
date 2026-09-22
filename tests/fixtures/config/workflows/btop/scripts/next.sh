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


request = json.load(sys.stdin)
context = request.get("context", {})
parameters = context.get("parameters", {})
next_query = parameters.get("next") if isinstance(parameters, dict) else None
if not isinstance(next_query, dict):
    raise SystemExit("btop next query must be an object")
json.dump(
    {
        "version": 1,
        "operation": {
            "type": "navigate",
            "target": f"{workflow_owner()}:main",
            "query": next_query,
            "replace": True,
        },
    },
    sys.stdout,
    separators=(",", ":"),
)
sys.stdout.write("\n")
